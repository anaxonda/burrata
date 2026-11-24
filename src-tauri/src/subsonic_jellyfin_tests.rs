use super::*;
use axum::{extract::Query, response::IntoResponse, routing::post, Json};
use std::sync::atomic::{AtomicBool, Ordering};

const USER_ID: &str = "11111111111111111111111111111111";
const SONG_ID: &str = "22222222222222222222222222222222";
const ALBUM_ID: &str = "33333333333333333333333333333333";
const ARTIST_ID: &str = "44444444444444444444444444444444";
const SERVER_ID: &str = "55555555555555555555555555555555";
const PNG_HEADER: &[u8] = b"\x89PNG\r\n\x1a\n";

fn assert_authenticated(headers: &HeaderMap) {
    let value = headers[header::AUTHORIZATION].to_str().unwrap();
    assert!(value.starts_with("MediaBrowser "));
    assert!(value.contains("Token=\"jellyfin-fixture-token\""));
}

fn song_item() -> serde_json::Value {
    json!({
        "Id": SONG_ID, "Type": "Audio", "MediaType": "Audio", "Name": "Fixture track",
        "Album": "Fixture album", "AlbumId": ALBUM_ID, "AlbumArtist": "Fixture artist",
        "Artists": ["Fixture artist"], "ArtistItems": [{"Id": ARTIST_ID, "Name": "Fixture artist"}],
        "RunTimeTicks": 1230000000_u64, "IndexNumber": 2, "ParentIndexNumber": 1,
        "ImageTags": {"Primary": "stale-song-cover"}
    })
}

fn jellyfin_upstream(album_removed: Arc<AtomicBool>) -> Router {
    Router::new()
        .route("/Users/AuthenticateByName", post(|headers: HeaderMap, Json(login): Json<serde_json::Value>| async move {
            assert_eq!(login, json!({"Username":"fixture-user", "Pw":"fixture-password"}));
            let auth = headers[header::AUTHORIZATION].to_str().unwrap();
            assert!(auth.contains("Client=\"Burrata\""));
            assert!(!auth.contains("Token="));
            Json(json!({"User":{"Id":USER_ID},"ServerId":SERVER_ID,"AccessToken":"jellyfin-fixture-token"}))
        }))
        .route("/Items", get(|headers: HeaderMap, Query(query): Query<HashMap<String, String>>| async move {
            assert_authenticated(&headers);
            assert_eq!(query["userId"], USER_ID);
            assert_eq!(query["includeItemTypes"], "Audio");
            assert_eq!(query["recursive"], "true");
            let items = match query["startIndex"].as_str() {
                "0" => vec![song_item()],
                "1" => Vec::new(),
                offset => panic!("Unexpected catalog offset {offset}"),
            };
            Json(json!({"Items": items, "TotalRecordCount": 1}))
        }))
        .route("/Items/{id}", get(move |AxumPath(id): AxumPath<String>, headers: HeaderMap, Query(query): Query<HashMap<String, String>>| async move {
            assert_authenticated(&headers);
            assert_eq!(query["userId"], USER_ID);
            let item = match id.as_str() {
                SONG_ID => {
                    let mut item = song_item();
                    if album_removed.load(Ordering::Relaxed) {
                        item.as_object_mut().unwrap().remove("AlbumId");
                    }
                    item
                }
                ALBUM_ID => json!({"Id": ALBUM_ID, "Type": "MusicAlbum", "ImageTags": {"Primary": "cover-tag"}}),
                _ => return StatusCode::NOT_FOUND.into_response(),
            };
            Json(item).into_response()
        }))
        .route("/Items/{id}/Images/Primary", get(|AxumPath(id): AxumPath<String>, headers: HeaderMap, Query(query): Query<HashMap<String, String>>| async move {
            assert_authenticated(&headers);
            assert_eq!(query["maxWidth"], "256");
            assert_eq!(query["maxHeight"], "256");
            match id.as_str() {
                SONG_ID => StatusCode::NOT_FOUND.into_response(),
                ALBUM_ID => ([(header::CONTENT_TYPE, "image/png")], PNG_HEADER).into_response(),
                _ => panic!("Unexpected cover ID {id}"),
            }
        }))
        .route("/Audio/{id}/stream", get(|AxumPath(id): AxumPath<String>, headers: HeaderMap, Query(query): Query<HashMap<String, String>>| async move {
            assert_authenticated(&headers);
            assert_eq!(id, SONG_ID);
            assert_eq!(query["static"], "true");
            let (content_range, bytes): (&str, &[u8]) = match headers[header::RANGE].to_str().unwrap() {
                "bytes=0-1" => ("bytes 0-1/10", b"01"),
                "bytes=3-5" => ("bytes 3-5/10", b"345"),
                range => panic!("Unexpected fixture range {range}"),
            };
            (StatusCode::PARTIAL_CONTENT, [
                (header::CONTENT_TYPE, "audio/flac"), (header::CONTENT_RANGE, content_range),
                (header::ACCEPT_RANGES, "bytes"),
            ], bytes)
        }))
}

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base_url, server)
}

fn library_ids(store: &Store, source: Option<&str>) -> Vec<String> {
    let (tracks, total, _, _) = store
        .list_tracks_page_with_sort_state(
            "library",
            None,
            20,
            0,
            None,
            Some("sourceOrder"),
            None,
            None,
            None,
            None,
            source,
        )
        .unwrap();
    assert_eq!(tracks.len(), total);
    let mut ids = tracks.into_iter().map(|track| track.id).collect::<Vec<_>>();
    ids.sort();
    ids
}

#[tokio::test]
async fn jellyfin_refresh_publishes_scoped_library_and_plays_through_native_relay() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&root.path().join("library.sqlite")).unwrap());
    let album_removed = Arc::new(AtomicBool::new(false));
    let (endpoint, upstream) = serve(jellyfin_upstream(album_removed.clone())).await;
    let protocol = Protocol::Jellyfin;
    let service = Service::new_for(store.clone(), root.path(), protocol);
    let client = crate::jellyfin_client::Client::connect_with_device_id(
        &endpoint,
        "fixture-user",
        "fixture-password".into(),
        &service.profile,
    )
    .await
    .unwrap();
    assert_eq!(client.account_identity(), format!("{SERVER_ID}:{USER_ID}"));
    let config = Configuration {
        endpoint,
        username: "fixture-user".into(),
        authenticated_account: Some(client.account_identity().to_owned()),
        visible: true,
        allow_fallback: false,
    };
    let account = config.account();
    store
        .set_json_preference(protocol.preference(), &json!(config))
        .unwrap();
    *service.configured_client.lock().await =
        Some((account.clone(), ServerClient::Jellyfin(client)));
    let (relay_base, relay) = serve(service.router()).await;
    service.set_base_url(relay_base.clone());

    // The same raw item and account identifiers under a different protocol
    // cannot attach themselves to Jellyfin's credentials or catalog state.
    let other_track = server_track(
        "subsonic",
        &account,
        &Song {
            id: SONG_ID.into(),
            title: "Other provider".into(),
            ..Song::default()
        },
    );
    store
        .replace_server_library_catalog(
            "subsonic",
            &account,
            "other-revision",
            &[other_track.clone()],
        )
        .unwrap();

    let status = service.refresh().await.unwrap();
    assert_eq!(status.track_count, 1);
    assert!(status.visible);
    let page = service.page("", 0, 10, None).unwrap();
    assert_eq!(page.total, 1);
    let track = &page.tracks[0];
    assert_eq!(
        track.id,
        format!("jellyfin:{account}.{}", URL_SAFE_NO_PAD.encode(SONG_ID))
    );
    assert_eq!(track.artist, "Fixture artist");
    assert_eq!(track.album.as_deref(), Some("Fixture album"));
    assert_eq!(track.provenance["serverArtistId"], ARTIST_ID);
    assert_eq!(track.provenance["serverAlbumId"], ALBUM_ID);
    assert_eq!(
        library_ids(&store, None),
        [track.id.clone(), other_track.id.clone()]
    );
    assert_eq!(library_ids(&store, Some("jellyfin")), [track.id.clone()]);
    assert!(library_ids(&store, Some("saved")).is_empty());
    for (kind, expected) in [("artist", "Fixture artist"), ("album", "Fixture album")] {
        let entities = store
            .list_library_entities_filtered(
                kind,
                None,
                "",
                Some("jellyfin"),
                None,
                "name",
                false,
                10,
                0,
                crate::storage::library::ArtistGrouping::Individual,
            )
            .unwrap();
        assert_eq!(entities.total, 1);
        assert_eq!(entities.items[0].name, expected);
    }
    assert!(service
        .resolve(&other_track, &Settings::default())
        .await
        .is_err());
    assert_eq!(
        service.track_artwork(&track.id).await.unwrap(),
        Some(format!(
            "data:image/png;base64,{}",
            STANDARD.encode(PNG_HEADER)
        ))
    );
    album_removed.store(true, Ordering::Relaxed);
    assert!(service.track_artwork(&track.id).await.unwrap().is_none());
    album_removed.store(false, Ordering::Relaxed);

    let resolved = service.resolve(track, &Settings::default()).await.unwrap();
    assert_eq!(resolved.quality["provider"], "jellyfin");
    assert_eq!(resolved.duration, Some(123.0));
    assert!(resolved.source.headers.is_empty());
    let prefix = format!("{relay_base}/jellyfin-audio/");
    let token = resolved.source.input.strip_prefix(&prefix).unwrap();
    assert_eq!(service.leases.lock().unwrap()[token].id, SONG_ID);
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(&resolved.source.input)
        .header(header::RANGE, "bytes=3-5")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 3-5/10");
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"345");

    assert!(service.fallback_track(track).is_none());
    service.preferences(true, true).await.unwrap();
    assert!(service.fallback_track(track).is_none());

    let playlist = store.create_collection("Keep Jellyfin track").unwrap();
    store
        .append_playlist_entries(&playlist.id, std::slice::from_ref(track))
        .unwrap();
    assert!(service.fallback_track(track).is_some());
    assert!(!service.preferences(false, true).await.unwrap().visible);
    assert_eq!(library_ids(&store, None), [track.id.clone(), other_track.id]);
    assert_eq!(library_ids(&store, Some("jellyfin")), [track.id.clone()]);
    let saved = store.list_tracks("collection", Some(&playlist.id)).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, track.id);
    assert!(service.fallback_track(&saved[0]).is_some());
    assert!(service
        .resolve(&saved[0], &Settings::default())
        .await
        .is_ok());
    service.preferences(false, false).await.unwrap();
    assert!(service.fallback_track(&saved[0]).is_none());

    relay.abort();
    upstream.abort();
}
