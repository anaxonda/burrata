//! Read-only provider check using the app's current database and keyring token.
//! Run with `cargo run --example artist_provider_check` from src-tauri.

use burrata_lib::{
    artists::{ArtistCredentials, ArtistService},
    connections::ConnectionManager,
    Store,
};
use std::{env, path::PathBuf, sync::Arc};

fn app_data_dir() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("BURRATA_APP_DATA") {
        return Ok(PathBuf::from(path));
    }
    let mut args = env::args_os().skip(1);
    if let Some(path) = args.next() {
        return Ok(PathBuf::from(path));
    }
    dirs::data_dir()
        .map(|path| path.join("org.burrata.discovery"))
        .ok_or_else(|| "Could not locate the Burrata application data directory".into())
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let dir = app_data_dir()?;
    let database = dir.join("burrata.sqlite");
    if !database.is_file() {
        return Err(format!(
            "Burrata database not found at {}; set BURRATA_APP_DATA or pass the app data directory",
            database.display()
        ));
    }
    let store = Arc::new(Store::open(&database)?);
    let connections = ConnectionManager::new(store.clone());
    let settings = connections.settings()?;
    if !settings.discogs_enabled {
        return Err("Discogs is disabled in the current Burrata connection settings".into());
    }
    // Retrieve the existing secure credential. Never print or include it in errors.
    let token = connections
        .credential("discogs")
        .map_err(|_| "Could not read the Discogs credential from the system keyring")?;
    if !token
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err("No Discogs credential is available in the current keyring/session".into());
    }
    let credentials = ArtistCredentials {
        discogs_token: token,
        lastfm_api_key: None,
        warnings: Vec::new(),
    };
    let service = ArtistService::new(store)?;
    let page = service
        .search_artists("Big Thief", 1, 25, &credentials)
        .await?;
    for result in page.results {
        println!("{}\t{}", result.artist.provider, result.name);
    }
    for warning in page.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(())
}
