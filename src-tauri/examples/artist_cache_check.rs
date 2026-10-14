//! Reproduce artist rendering data from public provider caches without changing the user's library.
use burrata_lib::{
    artists::{ArtistCredentials, ArtistRef, ArtistService},
    Store,
};
use rusqlite::{Connection, OpenFlags};
use std::{path::PathBuf, sync::Arc};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let input = PathBuf::from(args.get(1).ok_or("Pass library.sqlite and output.json")?);
    let output = PathBuf::from(args.get(2).ok_or("Pass output.json")?);
    let source = Connection::open_with_flags(input, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let temp = tempfile::tempdir()?;
    let store = Arc::new(Store::open(&temp.path().join("qa.sqlite"))?);
    let mut statement = source.prepare("SELECT namespace,key,payload FROM provider_cache WHERE namespace LIKE 'artists.%musicbrainz' OR namespace LIKE 'artists.%discogs'")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (namespace, key, payload) = row?;
        store.set_provider_cache(&namespace, &key, &serde_json::from_str(&payload)?, 3600)?;
    }
    let service = ArtistService::new(store)?;
    // A placeholder only enables the cached Discogs branch; it is not a user credential.
    let credentials = ArtistCredentials {
        discogs_token: Some("cache-validation-placeholder".into()),
        lastfm_api_key: None,
        warnings: vec![],
    };
    let profile = service
        .get_artist_profile(
            &ArtistRef {
                provider: "musicbrainz".into(),
                id: "9f81247f-7f57-42f3-a8ba-75bef554e591".into(),
            },
            &credentials,
        )
        .await?;
    let albums = profile
        .releases
        .items
        .iter()
        .filter(|item| item.primary_type.as_deref() == Some("Album"))
        .count();
    eprintln!(
        "Big Thief: {} release rows, {} album editions, next cursor {:?}",
        profile.releases.items.len(),
        albums,
        profile.releases.next_cursor
    );
    std::fs::write(output, serde_json::to_vec(&profile)?)?;
    Ok(())
}
