//! Bounded display-only transport for files in Burrata's own artwork cache.
use base64::Engine;
use std::{fs::OpenOptions, io::Read, path::Path};
use tauri::Manager;
const MAX_BYTES: u64 = 1024 * 1024;
fn read_display(directory: &Path, path: &Path) -> Result<String, String> {
    let fail = || "Cached artwork display is unavailable".to_owned();
    if path.parent() != Some(directory) {
        return Err(fail());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(fail)?;
    let (hash, extension) = name.rsplit_once('.').ok_or_else(fail)?;
    if hash.len() != 64
        || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !matches!(extension, "jpg" | "png" | "webp")
    {
        return Err(fail());
    }
    if std::fs::symlink_metadata(directory)
        .map_err(|_| fail())?
        .file_type()
        .is_symlink()
    {
        return Err(fail());
    }
    if !std::fs::symlink_metadata(path)
        .map_err(|_| fail())?
        .file_type()
        .is_file()
    {
        return Err(fail());
    }
    let canonical_directory = directory.canonicalize().map_err(|_| fail())?;
    let canonical_path = path.canonicalize().map_err(|_| fail())?;
    if canonical_path.parent() != Some(canonical_directory.as_path()) {
        return Err(fail());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|_| fail())?;
    let metadata = file.metadata().map_err(|_| fail())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_BYTES {
        return Err(fail());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| fail())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(fail());
    }
    let extension = crate::media::artwork_extension(&bytes).map_err(|_| fail())?;
    let mime = match extension {
        "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => return Err(fail()),
    };
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}
#[tauri::command]
pub(crate) async fn read_cached_artwork_display(
    app: tauri::AppHandle,
    path: String,
) -> Result<String, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| "Cached artwork display is unavailable")?
        .join("artwork");
    tauri::async_runtime::spawn_blocking(move || read_display(&directory, Path::new(&path)))
        .await
        .map_err(|_| "Cached artwork display is unavailable".to_owned())?
}
#[cfg(test)]
mod tests {
    use super::*;
    fn cache() -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("artwork");
        std::fs::create_dir(&directory).unwrap();
        (root, directory)
    }
    fn target(directory: &Path) -> std::path::PathBuf {
        directory.join(format!("{}.jpg", "a".repeat(64)))
    }
    #[test]
    fn reads_supported_magic_only_inside_owned_flat_cache() {
        let (_root, directory) = cache();
        let path = target(&directory);
        std::fs::write(&path, [0xff, 0xd8, 0xff, 0]).unwrap();
        assert!(read_display(&directory, &path)
            .unwrap()
            .starts_with("data:image/jpeg;base64,"));
        let outside = directory.parent().unwrap().join("outside.jpg");
        std::fs::write(&outside, [0xff, 0xd8, 0xff, 0]).unwrap();
        assert!(read_display(&directory, &outside).is_err());
        std::fs::write(&path, b"private text").unwrap();
        assert!(read_display(&directory, &path).is_err());
    }
    #[test]
    fn rejects_size_and_non_cache_filenames() {
        let (_root, directory) = cache();
        let path = target(&directory);
        std::fs::write(&path, vec![0xff; (MAX_BYTES + 1) as usize]).unwrap();
        assert!(read_display(&directory, &path).is_err());
        let path = directory.join("notes.jpg");
        std::fs::write(&path, [0xff, 0xd8, 0xff, 0]).unwrap();
        assert!(read_display(&directory, &path).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_files_and_cache_directory() {
        use std::os::unix::fs::symlink;
        let (root, directory) = cache();
        let outside = root.path().join("outside.jpg");
        std::fs::write(&outside, [0xff, 0xd8, 0xff, 0]).unwrap();
        let path = target(&directory);
        symlink(&outside, &path).unwrap();
        assert!(read_display(&directory, &path).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, [0xff, 0xd8, 0xff, 0]).unwrap();
        let alias = root.path().join("alias");
        symlink(&directory, &alias).unwrap();
        assert!(read_display(&alias, &alias.join(path.file_name().unwrap())).is_err());
    }
}
