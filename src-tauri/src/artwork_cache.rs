//! Regenerable artwork: content-addressed images with bounded URL lookup records.
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const BUDGET: u64 = 256 * 1024 * 1024;
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn index(directory: &Path, key: &str) -> PathBuf {
    directory.join(format!("{}.lookup", digest(key.as_bytes())))
}

pub(crate) fn lookup(directory: &Path, key: &str) -> Option<PathBuf> {
    if !safe_cache_directory(directory) {
        return None;
    }
    let lookup = index(directory, key);
    if !safe_regular_file(&lookup) {
        return None;
    }
    let name = String::from_utf8(read_regular_file_limited(&lookup, 80).ok()?).ok()?;
    let (hash, ext) = name.split_once('.')?;
    if hash.len() != 64
        || !hash.bytes().all(|b| b.is_ascii_hexdigit())
        || !["jpg", "png", "webp"].contains(&ext)
    {
        return None;
    }
    let path = directory.join(&name);
    safe_regular_file(&path).then_some(path)
}

/// Reads old URL-hash cache entries once, validates their image signature, and
/// atomically records them under the content-addressed format used by new writes.
pub(crate) fn lookup_or_migrate(directory: &Path, key: &str) -> Option<PathBuf> {
    if let Some(path) = lookup(directory, key) {
        return Some(path);
    }
    if !safe_cache_directory(directory) {
        return None;
    }
    let legacy = directory.join(format!("{}.", digest(key.as_bytes())));
    let legacy = ["jpg", "png", "webp"]
        .iter()
        .map(|extension| legacy.with_extension(extension))
        .find(|path| safe_regular_file(path))?;
    let bytes = read_regular_file(&legacy).ok()?;
    let extension = crate::media::artwork_extension(&bytes).ok()?;
    let migrated = write(directory, key, &bytes, extension).ok()?;
    if migrated != legacy {
        let _ = fs::remove_file(legacy);
    }
    Some(migrated)
}

pub(crate) fn write(
    directory: &Path,
    key: &str,
    bytes: &[u8],
    extension: &str,
) -> Result<PathBuf, String> {
    if !["jpg", "png", "webp"].contains(&extension) {
        return Err("Unsupported artwork format".into());
    }
    if bytes.is_empty() || bytes.len() as u64 > BUDGET {
        return Err("Artwork cache entry exceeds its size limit".into());
    }
    if crate::media::artwork_extension(bytes).map_err(|error| error.to_string())? != extension {
        return Err("Artwork extension does not match its image data".into());
    }
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    if !safe_cache_directory(directory) {
        return Err("Artwork cache directory is unavailable".into());
    }
    let name = format!("{}.{}", digest(bytes), extension);
    let path = directory.join(&name);
    if safe_regular_file(&path) {
        // Reuse only a regular file whose bytes actually match its content key.
        if read_regular_file(&path).map_err(|e| e.to_string())? != bytes {
            return Err("Artwork cache content hash collision".into());
        }
    } else if fs::symlink_metadata(&path).is_ok() {
        return Err("Artwork cache entry is not a regular file".into());
    } else {
        let temporary = directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        drop(file);
        if let Err(error) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            if !safe_regular_file(&path) || read_regular_file(&path).ok().as_deref() != Some(bytes)
            {
                return Err(error.to_string());
            }
        }
    }
    let lookup = index(directory, key);
    if fs::symlink_metadata(&lookup).is_ok() && !safe_regular_file(&lookup) {
        return Err("Artwork lookup entry is not a regular file".into());
    }
    let temporary_lookup = directory.join(format!("{}.lookup.tmp", uuid::Uuid::new_v4()));
    let mut lookup_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_lookup)
        .map_err(|e| e.to_string())?;
    if let Err(error) = lookup_file
        .write_all(name.as_bytes())
        .and_then(|_| lookup_file.sync_all())
    {
        let _ = fs::remove_file(&temporary_lookup);
        return Err(error.to_string());
    }
    drop(lookup_file);
    if let Err(error) = fs::rename(&temporary_lookup, &lookup) {
        // Windows rename does not replace an existing file. Preserve a regular
        // prior record until the new temporary is complete, then replace it.
        if safe_regular_file(&lookup) {
            if read_regular_file(&lookup).ok().as_deref() == Some(name.as_bytes()) {
                let _ = fs::remove_file(&temporary_lookup);
            } else {
                fs::remove_file(&lookup).map_err(|failure| failure.to_string())?;
                if let Err(failure) = fs::rename(&temporary_lookup, &lookup) {
                    let _ = fs::remove_file(&temporary_lookup);
                    return Err(failure.to_string());
                }
            }
        } else {
            let _ = fs::remove_file(&temporary_lookup);
            return Err(error.to_string());
        }
    }
    // Cleanup is best effort; failure must not prevent displaying fetched art.
    let _ = prune(directory, BUDGET, &path);
    Ok(path)
}

fn safe_cache_directory(directory: &Path) -> bool {
    fs::symlink_metadata(directory)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false)
}

fn safe_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

pub(crate) fn cached_image_files(directory: &Path) -> Vec<PathBuf> {
    if !safe_cache_directory(directory) {
        return Vec::new();
    }
    fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let name = entry.file_name();
            let name = name.to_str()?;
            let (hash, extension) = name.rsplit_once('.')?;
            (hash.len() == 64
                && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                && ["jpg", "png", "webp"].contains(&extension))
            .then(|| entry.path())
        })
        .collect()
}

fn read_regular_file(path: &Path) -> std::io::Result<Vec<u8>> {
    read_regular_file_limited(path, BUDGET)
}

fn read_regular_file_limited(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    if !safe_regular_file(path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    use std::io::Read;
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "cache entry too large",
        ));
    }
    Ok(bytes)
}

fn prune(directory: &Path, budget: u64, keep: &Path) -> std::io::Result<()> {
    let mut files = Vec::new();
    let mut total = 0;
    for entry in std::fs::read_dir(directory)?.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some((hash, extension)) = name.split_once('.') else {
            continue;
        };
        if hash.len() != 64
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            || !["jpg", "png", "webp", "lookup"].contains(&extension)
        {
            continue;
        }
        // Never traverse symlinks or directories in the cache.
        if !entry.file_type()?.is_file() {
            continue;
        }
        let metadata = entry.metadata()?;
        total += metadata.len();
        files.push((metadata.modified()?, path, metadata.len()));
    }
    files.sort_by_key(|file| file.0);
    for (_, path, bytes) in files {
        if total <= budget {
            break;
        }
        if path != keep && std::fs::remove_file(path).is_ok() {
            total = total.saturating_sub(bytes);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_images_share_a_file_and_missing_images_are_cache_misses() {
        let directory = tempfile::tempdir().unwrap();
        let image = b"\x89PNG\r\n\x1a\nimage";
        let a = write(directory.path(), "url?a", image, "png").unwrap();
        let b = write(directory.path(), "url?b", image, "png").unwrap();
        assert_eq!(a, b);
        assert_eq!(lookup(directory.path(), "url?a"), Some(a.clone()));
        std::fs::remove_file(a).unwrap();
        assert!(lookup(directory.path(), "url?a").is_none());
    }

    #[test]
    fn updating_a_url_atomically_replaces_its_lookup_record() {
        let directory = tempfile::tempdir().unwrap();
        let old_image = b"\x89PNG\r\n\x1a\nold";
        let new_image = b"\x89PNG\r\n\x1a\nnew";
        let old_path = write(directory.path(), "same-url", old_image, "png").unwrap();
        let new_path = write(directory.path(), "same-url", new_image, "png").unwrap();
        assert_ne!(old_path, new_path);
        assert_eq!(lookup(directory.path(), "same-url"), Some(new_path));
    }
    #[test]
    fn cleanup_preserves_current_file_and_rejects_path_traversal() {
        let directory = tempfile::tempdir().unwrap();
        let a = write(directory.path(), "a", b"\x89PNG\r\n\x1a\na", "png").unwrap();
        let b = write(directory.path(), "b", b"\x89PNG\r\n\x1a\nb", "png").unwrap();
        prune(directory.path(), 0, &b).unwrap();
        assert!(!a.exists());
        assert!(b.exists());
        std::fs::write(index(directory.path(), "bad"), "../../outside.png").unwrap();
        assert!(lookup(directory.path(), "bad").is_none());
    }

    #[test]
    fn migrates_legacy_url_hash_entries_and_deduplicates_identical_images() {
        let directory = tempfile::tempdir().unwrap();
        // Small JPEG-signature fixtures are sufficient for extension detection.
        let image = [0xff, 0xd8, 0xff, 0x00, 0x01];
        let first = "https://example.test/first.jpg";
        let second = "https://example.test/second.jpg";
        let legacy_first = directory
            .path()
            .join(format!("{}.jpg", digest(first.as_bytes())));
        let legacy_second = directory
            .path()
            .join(format!("{}.jpg", digest(second.as_bytes())));
        fs::write(&legacy_first, image).unwrap();
        fs::write(&legacy_second, image).unwrap();

        let migrated_first = lookup_or_migrate(directory.path(), first).unwrap();
        let migrated_second = lookup_or_migrate(directory.path(), second).unwrap();
        assert_eq!(migrated_first, migrated_second);
        assert_eq!(
            migrated_first.file_name().unwrap().to_str().unwrap(),
            format!("{}.jpg", digest(&image))
        );
        assert!(!legacy_first.exists());
        assert!(!legacy_second.exists());
        assert_eq!(
            lookup(directory.path(), first),
            Some(migrated_first.clone())
        );
        assert_eq!(lookup(directory.path(), second), Some(migrated_first));
    }

    #[test]
    fn rejects_unsupported_extensions_and_symlinked_cache_entries() {
        let directory = tempfile::tempdir().unwrap();
        assert!(write(directory.path(), "x", b"image", "../../outside").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = directory
                .path()
                .parent()
                .unwrap()
                .join(format!("artwork-outside-{}", uuid::Uuid::new_v4()));
            fs::write(&outside, format!("{}.jpg", "a".repeat(64))).unwrap();
            let lookup_path = index(directory.path(), "poisoned");
            symlink(&outside, &lookup_path).unwrap();
            assert!(lookup(directory.path(), "poisoned").is_none());
            fs::remove_file(&lookup_path).unwrap();
            fs::write(&lookup_path, vec![b'x'; 1024]).unwrap();
            assert!(lookup(directory.path(), "poisoned").is_none());
            fs::remove_file(&lookup_path).unwrap();

            let image = [0xff, 0xd8, 0xff, 0x00];
            let image_name = format!("{}.jpg", digest(&image));
            let outside_image = directory
                .path()
                .parent()
                .unwrap()
                .join(format!("artwork-image-{}", uuid::Uuid::new_v4()));
            fs::write(&outside_image, image).unwrap();
            let image_path = directory.path().join(&image_name);
            symlink(&outside_image, &image_path).unwrap();
            fs::write(index(directory.path(), "image-link"), image_name).unwrap();
            assert!(lookup(directory.path(), "image-link").is_none());
            assert!(write(directory.path(), "image-link", &image, "jpg").is_err());
            assert!(!cached_image_files(directory.path()).contains(&image_path));
            fs::remove_file(outside).unwrap();
            fs::remove_file(outside_image).unwrap();

            let linked_directory = directory
                .path()
                .parent()
                .unwrap()
                .join(format!("artwork-dir-{}", uuid::Uuid::new_v4()));
            symlink(directory.path(), &linked_directory).unwrap();
            assert!(lookup(&linked_directory, "image-link").is_none());
            assert!(write(&linked_directory, "image-link", &image, "jpg").is_err());
            fs::remove_file(linked_directory).unwrap();
        }
    }
}
