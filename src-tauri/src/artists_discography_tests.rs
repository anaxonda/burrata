use super::*;
use axum::{
    extract::{OriginalUri, Query, State},
    Json, Router,
};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Mutex;

const ARTIST_ID: &str = "00000000-0000-4000-8000-000000000001";
const GROUP_ONE: &str = "00000000-0000-4000-8000-000000000011";
const GROUP_TWO: &str = "00000000-0000-4000-8000-000000000012";
const GROUP_THREE: &str = "00000000-0000-4000-8000-000000000013";

#[derive(Clone, Default)]
struct RequestLog {
    total: Arc<AtomicUsize>,
    recordings: Arc<AtomicUsize>,
    queries: Arc<Mutex<Vec<(String, HashMap<String, String>)>>>,
}

async fn fixture_response(
    State(log): State<RequestLog>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    log.total.fetch_add(1, Ordering::SeqCst);
    let path = uri.path().to_owned();
    log.queries.lock().await.push((path.clone(), query.clone()));
    if path.ends_with("/recording") {
        log.recordings.fetch_add(1, Ordering::SeqCst);
        return Json(json!({"recordings": [], "count": 0}));
    }
    if path.ends_with("/release-group") {
        return if query.get("offset").map(String::as_str) == Some("2") {
            Json(json!({
                "release-group-count": 3,
                "release-groups": [{"id": GROUP_THREE, "title": "Third album"}]
            }))
        } else {
            Json(json!({
                "release-group-count": 3,
                "release-groups": [
                    {"id": GROUP_ONE, "title": "First album", "first-release-date": "1999"},
                    {"id": GROUP_TWO, "title": "Second album", "first-release-date": "2001"}
                ]
            }))
        };
    }
    if path.ends_with("/release") {
        return Json(json!({
            "release-count": 3,
            "releases": [
                {"id": "00000000-0000-4000-8000-000000000021", "title": "First edition"},
                {"id": "00000000-0000-4000-8000-000000000022", "title": "Second edition"}
            ]
        }));
    }
    Json(json!({}))
}

async fn fixture_service() -> (
    ArtistService,
    RequestLog,
    tokio::task::JoinHandle<()>,
    tempfile::TempDir,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let log = RequestLog::default();
    let app = Router::new()
        .fallback(fixture_response)
        .with_state(log.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&directory.path().join("artist-discography.sqlite")).unwrap());
    let mut service = ArtistService::new(store).unwrap();
    service.endpoints.musicbrainz = format!("http://{address}/ws/2");
    (service, log, server, directory)
}

fn artist() -> ArtistRef {
    ArtistRef {
        provider: "musicbrainz".into(),
        id: ARTIST_ID.into(),
    }
}

#[tokio::test]
async fn compact_discography_paginates_groups_and_editions_are_explicit() {
    let (service, log, server, _directory) = fixture_service().await;
    let artist = artist();
    let credentials = ArtistCredentials::default();

    let first = service
        .get_artist_discography(&artist, None, &credentials)
        .await
        .unwrap();
    assert_eq!(first.artist, artist);
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.title.as_str())
            .collect::<Vec<_>>(),
        ["First album", "Second album"]
    );
    assert_eq!(first.next_cursor.as_deref(), Some("g1:2:x"));
    assert_eq!(
        log.recordings.load(Ordering::SeqCst),
        0,
        "index loading must not fetch recordings"
    );
    {
        let requests = log.queries.lock().await;
        let (path, query) = requests
            .iter()
            .find(|(path, _)| path.ends_with("/release-group"))
            .unwrap();
        assert!(path.ends_with("/release-group"));
        assert_eq!(query.get("artist").map(String::as_str), Some(ARTIST_ID));
        assert_eq!(query.get("limit").map(String::as_str), Some("100"));
        assert_eq!(query.get("inc").map(String::as_str), Some("artist-credits"));
        assert!(!requests.iter().any(|(path, _)| path.ends_with("/release")));
    }

    let second = service
        .get_artist_discography(&artist, first.next_cursor.as_deref(), &credentials)
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].release.id, GROUP_THREE);
    assert_eq!(second.next_cursor, None);
    assert_eq!(log.recordings.load(Ordering::SeqCst), 0);

    let release_group = ArtistReleaseRef {
        release: ArtistRef {
            provider: "musicbrainz".into(),
            id: GROUP_ONE.into(),
        },
        title: "First album".into(),
        year: None,
        release_date: None,
        country: None,
        label: None,
        catalog_number: None,
        format: None,
        primary_type: None,
        track_count: None,
        tracks: Vec::new(),
        artwork_urls: Vec::new(),
        source_url: format!("https://musicbrainz.org/release-group/{GROUP_ONE}"),
        credits: Vec::new(),
    };
    let editions = service
        .get_release_editions(&release_group, &artist, None, &credentials)
        .await
        .unwrap();
    assert_eq!(editions.artist, artist);
    assert_eq!(editions.items.len(), 2);
    assert_eq!(editions.items[0].title, "First edition");
    let expected_editions_cursor = format!("e1:{GROUP_ONE}:2");
    assert_eq!(
        editions.next_cursor.as_deref(),
        Some(expected_editions_cursor.as_str())
    );
    {
        let requests = log.queries.lock().await;
        let (path, query) = requests
            .iter()
            .find(|(path, _)| path.ends_with("/release"))
            .unwrap();
        assert!(path.ends_with("/release"));
        assert_eq!(
            query.get("release-group").map(String::as_str),
            Some(GROUP_ONE)
        );
        assert_eq!(query.get("limit").map(String::as_str), Some("100"));
        assert_eq!(
            query.get("inc").map(String::as_str),
            Some("artist-credits+labels+media+release-groups")
        );
    }
    let requests_before_invalid_cursor = log.total.load(Ordering::SeqCst);
    assert!(service
        .get_release_editions(
            &release_group,
            &artist,
            Some("e1:00000000-0000-4000-8000-000000000099:2"),
            &credentials
        )
        .await
        .is_err());
    assert_eq!(
        log.total.load(Ordering::SeqCst),
        requests_before_invalid_cursor,
        "a cursor for another release group must be rejected before HTTP"
    );
    server.abort();
}

#[tokio::test]
async fn identical_discography_requests_share_one_provider_cache_fill() {
    let (service, log, server, _directory) = fixture_service().await;
    let artist = artist();
    let credentials = ArtistCredentials::default();
    let (left, right) = tokio::join!(
        service.get_artist_discography(&artist, None, &credentials),
        service.get_artist_discography(&artist, None, &credentials),
    );
    assert!(left.is_ok() && right.is_ok());
    assert_eq!(
        log.total.load(Ordering::SeqCst),
        1,
        "identical concurrent requests should perform one HTTP fetch/cache fill"
    );
    server.abort();
}

#[derive(Clone)]
struct CancellationFixture {
    requests: Arc<AtomicUsize>,
    first_request_received: Arc<tokio::sync::Notify>,
    first_delay: Duration,
}

async fn cancellation_response(State(state): State<CancellationFixture>) -> Json<Value> {
    let request = state.requests.fetch_add(1, Ordering::SeqCst);
    if request == 0 {
        state.first_request_received.notify_one();
        tokio::time::sleep(state.first_delay).await;
    }
    Json(json!({"request": request}))
}

#[tokio::test]
async fn canceling_cache_fill_releases_gate_for_a_fresh_retry() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = CancellationFixture {
        requests: Arc::new(AtomicUsize::new(0)),
        first_request_received: Arc::new(tokio::sync::Notify::new()),
        first_delay: Duration::from_millis(200),
    };
    let app = Router::new()
        .fallback(cancellation_response)
        .with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(&directory.path().join("artist-canceled-cache.sqlite")).unwrap());
    let service = ArtistService::new(store).unwrap();
    let url = Url::parse(&format!("http://{address}/slow-cache-fill")).unwrap();

    let first_service = service.clone();
    let first_url = url.clone();
    let first = tokio::spawn(async move {
        first_service
            .get_json("artiststest", "cancel-retry", first_url, LIST_TTL, None)
            .await
    });
    state.first_request_received.notified().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());

    let retried = service
        .get_json("artiststest", "cancel-retry", url, LIST_TTL, None)
        .await
        .unwrap();
    assert_eq!(retried["request"], 1);
    assert_eq!(
        state.requests.load(Ordering::SeqCst),
        2,
        "the canceled owner must not strand the gate or cache a partial response"
    );
    server.abort();
}

#[tokio::test]
async fn transient_transport_timeout_retries_once_without_discarding_the_index() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = CancellationFixture {
        requests: Arc::new(AtomicUsize::new(0)),
        first_request_received: Arc::new(tokio::sync::Notify::new()),
        first_delay: Duration::from_secs(1),
    };
    let app = Router::new()
        .fallback(cancellation_response)
        .with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(&directory.path().join("artist-timeout-retry.sqlite")).unwrap());
    let mut service = ArtistService::new(store).unwrap();
    service.client = Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let url = Url::parse(&format!("http://{address}/catalog?api_key=test-secret")).unwrap();
    let result = service
        .get_json(
            "artiststest",
            "transport-retry",
            url.clone(),
            LIST_TTL,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result["request"], 1);
    assert_eq!(state.requests.load(Ordering::SeqCst), 2);
    assert_eq!(
        service
            .get_json("artiststest", "transport-retry", url, LIST_TTL, None)
            .await
            .unwrap(),
        result
    );
    assert_eq!(
        state.requests.load(Ordering::SeqCst),
        2,
        "successful retry must fill the cache"
    );
    server.abort();
}
