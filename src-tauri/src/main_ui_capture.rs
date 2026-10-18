//! Capture only Burrata's fixed main WebView; never reads the desktop or changes focus.
use tauri::Manager;

#[cfg(target_os = "linux")]
pub(crate) async fn capture(app: &tauri::AppHandle) -> Result<serde_json::Value, String> {
    use base64::Engine;
    use std::io::Write;
    use webkit2gtk::WebViewExt;

    struct BoundedPng(Vec<u8>);
    impl Write for BoundedPng {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > 8 * 1024 * 1024 {
                return Err(std::io::Error::other("Main UI PNG exceeds 8 MiB"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let window = app
        .get_webview_window("main")
        .ok_or("Main UI is unavailable")?;
    let size = window.inner_size().map_err(|error| error.to_string())?;
    if size.width == 0 || size.height == 0 || size.width > 3840 || size.height > 2160 {
        return Err("Main UI snapshot requires a viewport no larger than 3840 × 2160".into());
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |platform| {
            platform.inner().snapshot(
                webkit2gtk::SnapshotRegion::Visible,
                webkit2gtk::SnapshotOptions::NONE,
                None::<&webkit2gtk::gio::Cancellable>,
                move |result| {
                    let result = (|| {
                        let surface = result.map_err(|error| error.to_string())?;
                        let image = cairo::ImageSurface::try_from(surface)
                            .map_err(|_| "Main UI snapshot is not an image surface".to_owned())?;
                        let (width, height) = (image.width(), image.height());
                        if width <= 0 || height <= 0 || width > 3840 || height > 2160 {
                            return Err(
                                "Main UI snapshot dimensions exceed the capture limit".into()
                            );
                        }
                        let mut png = BoundedPng(Vec::new());
                        image
                            .write_to_png(&mut png)
                            .map_err(|error| error.to_string())?;
                        Ok(serde_json::json!({
                            "mimeType": "image/png", "width": width, "height": height,
                            "data": base64::engine::general_purpose::STANDARD.encode(&png.0)
                        }))
                    })();
                    let _ = tx.send(result);
                },
            );
        })
        .map_err(|error| error.to_string())?;
    tokio::time::timeout(std::time::Duration::from_secs(5), rx)
        .await
        .map_err(|_| "Main UI snapshot timed out; no desktop fallback was used".to_owned())?
        .map_err(|_| "Main UI snapshot response closed".to_owned())?
}

#[cfg(not(target_os = "linux"))]
pub(crate) async fn capture(_app: &tauri::AppHandle) -> Result<serde_json::Value, String> {
    Err("App-owned UI capture is currently supported only on Linux".into())
}
