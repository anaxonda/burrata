//! Live, read-only qualification of the packaged runtime and anonymous provider.
//! Usage: cargo run --example check_youtube_support -- RESOURCE_ROOT [VIDEO_URL]
use burrata_lib::{tools, youtube_support::YouTubeSupportManager};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let resources = PathBuf::from(args.next().ok_or("Pass the packaged resource root")?).canonicalize()?;
    let url = args.next();
    tools::initialize(resources.clone());
    let bundled = tools::bundled_executable("yt-dlp").ok_or("Bundled yt-dlp missing")?;
    let node = tools::bundled_executable("node").ok_or("Bundled Node missing")?;
    let data = tempfile::tempdir()?;
    let manager = YouTubeSupportManager::new(
        data.path(), bundled, Some(node),
        Some(resources.join("youtube-support/bgutil/server/build/main.js")),
        Some(resources.join("youtube-support/bgutil/plugin")),
    );
    let result = manager.diagnose_url(url.as_deref()).await;
    manager.shutdown().await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    if !result.ok { return Err("Packaged YouTube support did not pass the live audio check".into()); }
    Ok(())
}
