//! Authenticated loopback HTTP serving for registered local audio assets.
use crate::storage::Store;
use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const MAX_CHUNK: u64 = 8 * 1024 * 1024;

pub struct LocalAudioManager {
    store: Arc<Store>,
    token: String,
    base_url: Mutex<Option<String>>,
}

impl LocalAudioManager {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        Arc::new(Self {
            store,
            token: uuid::Uuid::new_v4().to_string(),
            base_url: Mutex::new(None),
        })
    }

    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/local-audio/{id}", get(audio_asset).head(audio_asset))
            .with_state(self.clone())
    }

    pub fn set_base_url(&self, url: String) {
        *self.base_url.lock().unwrap() = Some(url.trim_end_matches('/').to_owned());
    }

    pub fn url_for(&self, asset_id: &str) -> Result<String, String> {
        if !valid_asset_id(asset_id) {
            return Err("Invalid local asset ID".into());
        }
        let base = self
            .base_url
            .lock()
            .map_err(|_| "Local audio URL unavailable")?
            .clone()
            .ok_or("Local audio server is not running")?;
        Ok(format!(
            "{base}/local-audio/{asset_id}?token={}",
            self.token
        ))
    }
}

#[derive(Deserialize)]
struct AuthQuery {
    token: String,
}

async fn audio_asset(
    State(manager): State<Arc<LocalAudioManager>>,
    AxumPath(id): AxumPath<String>,
    Query(auth): Query<AuthQuery>,
    headers: HeaderMap,
    method: Method,
) -> Response<Body> {
    if !valid_asset_id(&id)
        || !constant_time_eq(&auth.token, &manager.token)
        || !allowed_origin(&headers)
    {
        return empty(StatusCode::NOT_FOUND).map(Body::from);
    }
    let mut response = serve(&manager.store, &id, &headers, method == Method::HEAD).await;
    if let Some(origin) = headers.get(header::ORIGIN) {
        if let Ok(origin) = HeaderValue::from_bytes(origin.as_bytes()) {
            response
                .headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            response.headers_mut().insert(
                header::ACCESS_CONTROL_EXPOSE_HEADERS,
                HeaderValue::from_static("Content-Range, Accept-Ranges, Content-Length"),
            );
        }
    }
    response.map(Body::from)
}

fn allowed_origin(headers: &HeaderMap) -> bool {
    match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        None => true,
        Some("tauri://localhost") | Some("http://tauri.localhost") => true,
        _ => false,
    }
}

fn constant_time_eq(candidate: &str, expected: &str) -> bool {
    let a = candidate.as_bytes();
    let b = expected.as_bytes();
    let mut difference = a.len() ^ b.len();
    let n = a.len().max(b.len());
    for i in 0..n {
        difference |= usize::from(*a.get(i).unwrap_or(&0) ^ *b.get(i).unwrap_or(&0));
    }
    difference == 0
}

fn valid_asset_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

async fn serve(
    store: &Store,
    id: &str,
    request_headers: &HeaderMap,
    head: bool,
) -> Response<Vec<u8>> {
    let assets = match store.list_assets() {
        Ok(v) => v,
        Err(_) => return empty(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let Some(asset) = assets.into_iter().find(|a| a.id == id) else {
        return empty(StatusCode::NOT_FOUND);
    };
    let path = PathBuf::from(asset.path);
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(m) => m,
        Err(_) => return empty(StatusCode::NOT_FOUND),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return empty(StatusCode::NOT_FOUND);
    }
    let canonical = match tokio::fs::canonicalize(&path).await {
        Ok(p) => p,
        Err(_) => return empty(StatusCode::NOT_FOUND),
    };
    let file = match open_no_follow(canonical.clone()).await {
        Ok(f) => f,
        Err(_) => return empty(StatusCode::NOT_FOUND),
    };
    let size = match file.metadata().await {
        Ok(m) => m.len(),
        Err(_) => return empty(StatusCode::NOT_FOUND),
    };
    if size == 0 {
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .body(Vec::new())
            .unwrap();
        let h = response.headers_mut();
        let _ = h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(mime_type(&canonical)),
        );
        let _ = h.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
        let _ = h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        return response;
    }
    let range = request_headers
        .get(header::RANGE)
        .and_then(|h| h.to_str().ok());
    let Some((start, end, partial)) = select_range(range, size) else {
        let mut response = empty(StatusCode::RANGE_NOT_SATISFIABLE);
        let _ = response.headers_mut().insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes */{size}")).unwrap(),
        );
        return response;
    };
    let count = end.saturating_sub(start).saturating_add(1);
    let mut response = if head {
        Response::builder()
            .status(if partial {
                StatusCode::PARTIAL_CONTENT
            } else {
                StatusCode::OK
            })
            .body(Vec::new())
            .unwrap()
    } else {
        let mut bytes = vec![0u8; count as usize];
        let mut file = file;
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err()
            || file.read_exact(&mut bytes).await.is_err()
        {
            return empty(StatusCode::NOT_FOUND);
        }
        Response::builder()
            .status(if partial {
                StatusCode::PARTIAL_CONTENT
            } else {
                StatusCode::OK
            })
            .body(bytes)
            .unwrap()
    };
    let h = response.headers_mut();
    let _ = h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(mime_type(&canonical)),
    );
    let _ = h.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&count.to_string()).unwrap(),
    );
    let _ = h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    let _ = h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    let _ = h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if partial {
        let _ = h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")).unwrap(),
        );
    }
    response
}

async fn open_no_follow(path: PathBuf) -> std::io::Result<tokio::fs::File> {
    tokio::task::spawn_blocking(move || {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&path)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(tokio::fs::File::from_std(file))
    })
    .await
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?
}

fn select_range(raw: Option<&str>, size: u64) -> Option<(u64, u64, bool)> {
    if size == 0 {
        return None;
    }
    let Some(raw) = raw else {
        return Some((0, size.min(MAX_CHUNK) - 1, size > MAX_CHUNK));
    };
    let value = raw.strip_prefix("bytes=")?;
    if value.contains(',') {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?.min(MAX_CHUNK).min(size);
        if suffix == 0 {
            return None;
        }
        return Some((size - suffix, size - 1, true));
    }
    let start = start.parse::<u64>().ok()?;
    if start >= size {
        return None;
    }
    let requested_end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>().ok()?
    };
    if requested_end < start {
        return None;
    }
    let end = requested_end
        .min(size - 1)
        .min(start.saturating_add(MAX_CHUNK - 1));
    Some((start, end, true))
}

fn mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "opus" | "ogg" | "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "wav" | "wave" => "audio/wav",
        "m4a" | "mp4" | "m4b" => "audio/mp4",
        "webm" => "audio/webm",
        "aac" => "audio/aac",
        _ => "application/octet-stream",
    }
}

fn empty(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(status).body(Vec::new()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opus_gets_ogg_mime() {
        assert_eq!(mime_type(Path::new("song.opus")), "audio/ogg");
        assert_eq!(mime_type(Path::new("song.ogg")), "audio/ogg");
    }
    #[test]
    fn known_extensions_have_audio_mimes() {
        for (path, mime) in [
            ("track.mp3", "audio/mpeg"),
            ("track.flac", "audio/flac"),
            ("track.wav", "audio/wav"),
            ("track.m4a", "audio/mp4"),
            ("track.webm", "audio/webm"),
        ] {
            assert_eq!(mime_type(Path::new(path)), mime);
        }
    }
    #[test]
    fn ranges_are_satisfiable_and_bounded() {
        assert_eq!(select_range(None, 3), Some((0, 2, false)));
        assert_eq!(select_range(Some("bytes=2-"), 10), Some((2, 9, true)));
        assert_eq!(select_range(Some("bytes=-3"), 10), Some((7, 9, true)));
        assert_eq!(
            select_range(Some("bytes=0-99999999"), MAX_CHUNK * 4),
            Some((0, MAX_CHUNK - 1, true))
        );
        assert_eq!(select_range(Some("bytes=10-"), 10), None);
        assert_eq!(select_range(Some("bytes=1-2,4-5"), 10), None);
    }
    #[test]
    fn token_comparison_checks_all_bytes_and_origin_is_restricted() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secreu", "secret"));
        assert!(!valid_asset_id("../secret"));
        let mut headers = HeaderMap::new();
        assert!(allowed_origin(&headers));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("tauri://localhost"),
        );
        assert!(allowed_origin(&headers));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );
        assert!(!allowed_origin(&headers));
    }

    #[tokio::test]
    async fn loopback_endpoint_authenticates_and_serves_ranges_and_head() {
        use crate::{
            models::{LocalAsset, Track},
            storage::Store,
        };
        use std::sync::Arc;

        let directory = tempfile::tempdir().unwrap();
        let audio_path = directory.path().join("fixture.opus");
        tokio::fs::write(&audio_path, b"0123456789").await.unwrap();
        let store = Arc::new(Store::open(&directory.path().join("library.sqlite")).unwrap());
        let track = Track {
            id: "fixture:recording-1".into(),
            provider: "fixture".into(),
            provider_id: "recording-1".into(),
            title: "Fixture recording".into(),
            ..Track::default()
        };
        store.save_track(&track).unwrap();
        store
            .save_asset(&LocalAsset {
                id: "asset-fixture-1".into(),
                recording_id: track.id.clone(),
                path: audio_path.to_string_lossy().into_owned(),
                hash: "fixture-hash-1".into(),
                source_codec: "opus".into(),
                final_codec: "opus".into(),
                source_bitrate: None,
                final_bitrate: None,
                processing: "copied".into(),
                track,
            })
            .unwrap();

        let manager = LocalAudioManager::new(store);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        manager.set_base_url(format!("http://{address}"));
        let audio_url = manager.url_for("asset-fixture-1").unwrap();
        let router = manager.router();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = reqwest::Client::new();

        let mut wrong_token = url::Url::parse(&audio_url).unwrap();
        wrong_token
            .query_pairs_mut()
            .clear()
            .append_pair("token", "not-the-token");
        let response = client.get(wrong_token).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = client
            .get(&audio_url)
            .header(header::ORIGIN, "https://example.com")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = client
            .get(&audio_url)
            .header(header::RANGE, "bytes=1-3")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "audio/ogg"
        );
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 1-3/10"
        );
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"123");

        let response = client.head(&audio_url).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.bytes().await.unwrap().is_empty());
        server.abort();
    }
}
