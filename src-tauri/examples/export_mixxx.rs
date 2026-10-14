//! Export registered local recordings using the same playlist service as GUI/MCP.
use burrata_lib::{playlist_sync, Store};
fn main() -> Result<(), String> {
    let id = std::env::args()
        .nth(1)
        .ok_or("Usage: cargo run --example export_mixxx -- PLAYLIST_ID")?;
    let directory = std::env::var_os("BURRATA_APP_DATA")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::data_dir().map(|dir| dir.join("org.burrata.discovery")))
        .ok_or("Application data directory is unavailable")?;
    let database = directory.join("burrata.sqlite");
    if !database.is_file() {
        return Err("Start Burrata before exporting playlists".into());
    }
    let store = Store::open(&database)?;
    let exported = playlist_sync::export_mixxx_playlist(&store, &store.settings()?, &id)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&exported).map_err(|error| error.to_string())?
    );
    Ok(())
}
