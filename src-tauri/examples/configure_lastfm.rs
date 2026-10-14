//! Local setup helper. Credentials are read from a file and saved through Burrata's OS keyring.
use burrata_lib::{
    connections::{ConnectionManager, ConnectionUpdate},
    storage::Store,
};
use std::{path::Path, sync::Arc};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if !(3..=4).contains(&args.len()) {
        return Err("Usage: configure_lastfm KEY_INFO_FILE APP_DATABASE [COLLECTION_URL]".into());
    }
    let input = std::fs::read_to_string(&args[1])?;
    let key = input
        .lines()
        .find_map(|line| line.trim().strip_prefix("API key").map(str::trim))
        .filter(|key| key.len() == 32 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("Expected an API key line with a 32-character key")?;
    let store = Arc::new(Store::open(Path::new(&args[2]))?);
    let connections = ConnectionManager::new(store);
    let mut settings = connections.settings()?;
    settings.lastfm_enabled = true;
    let status = connections.update(ConnectionUpdate {
        settings,
        lastfm_api_key: Some(key.into()),
        ..Default::default()
    })?;
    if !status.lastfm_credential.present || status.lastfm_credential.session_only {
        return Err("Persistent credential could not be verified".into());
    }
    let snapshot = burrata_lib::collection_sources::import_source(
        args.get(3)
            .map(String::as_str)
            .unwrap_or("https://www.last.fm/charts"),
        &connections,
    )
    .await?;
    let chart = burrata_lib::browser_bridge::validate_snapshot(&snapshot)?;
    println!(
        "Last.fm key saved to OS keyring; direct chart verified: {} tracks, complete={}.",
        chart.tracks.len(),
        snapshot.complete
    );
    Ok(())
}
