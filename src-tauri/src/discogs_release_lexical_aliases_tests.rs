use super::*;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};

const DISCOGS_RELEASE_ID: &str = "3517186";
const MB_RELEASE_ID: &str = "11111111-1111-4111-8111-111111111111";
const MB_ARTIST_IDS: [&str; 3] = [
    "00000000-0000-4000-8000-000000000001",
    "00000000-0000-4000-8000-000000000002",
    "00000000-0000-4000-8000-000000000003",
];
const DISCOGS_ARTIST_IDS: [&str; 3] = ["9858", "31262", "1420481"];
const DISCOGS_ALIAS_ID: &str = "2698689";

struct Fixture {
    track: Track,
    request: SpotifyRequest,
    mb_release: Value,
    discogs_release: Value,
    mapped_discogs_ids: Vec<String>,
    alias_profiles: HashMap<String, Value>,
}

fn make_fixture() -> Fixture {
    let data: Value = serde_json::from_str(include_str!(
        "../fixtures/travelling-song-spotify-request.json"
    ))
    .unwrap();
    let track: Track = serde_json::from_value(data["tracks"][0].clone()).unwrap();
    let request =
        spotify_request(&track).expect("retained structured Spotify album row is accepted");
    let mb_release = mb_release_one_medium(3, 3, 3, 284_160);
    let discogs_release = discogs_release_one_medium(3, 3, 3, "4:44");
    let mapped_discogs_ids = DISCOGS_ARTIST_IDS
        .iter()
        .map(|id| (*id).to_owned())
        .collect();
    let alias_profiles = HashMap::from([(
        DISCOGS_ARTIST_IDS[2].to_owned(),
        json!({"id":1420481,"aliases":[{"id":2698689,"name":"The Combo Bárbaro"}]}),
    )]);
    Fixture {
        track,
        request,
        mb_release,
        discogs_release,
        mapped_discogs_ids,
        alias_profiles,
    }
}

fn mb_release_one_medium(
    album_tracks: u64,
    target_position: u64,
    target_album_position: u64,
    target_length_ms: u64,
) -> Value {
    let mut tracks = Vec::new();
    for position in 1..=album_tracks {
        let target = position == target_position && position == target_album_position;
        tracks.push(json!({
            "position": position,
            "title": if target { "Travelling Song" } else { "Other song" },
            "length": if target { target_length_ms } else { 60000 }
        }));
    }
    json!({
        "id": MB_RELEASE_ID,
        "title": "Look Around The Corner",
        "relations": [{"target-type":"url","url":{"resource":format!("https://www.discogs.com/release/{DISCOGS_RELEASE_ID}-Look_Around_The_Corner")}}],
        "media": [{"position":1,"tracks":tracks}]
    })
}

fn discogs_release_one_medium(
    album_tracks: u64,
    target_position: u64,
    target_album_position: u64,
    target_duration: &str,
) -> Value {
    let mut rows = Vec::new();
    for position in 1..=album_tracks {
        let target = position == target_position && position == target_album_position;
        let artists = if target {
            json!([
                {"id":9858,"name":"Quantic","join":", "},
                {"id":31262,"name":"Alice Russell","join":" with "},
                {"id":2698689,"name":"The Combo Bárbaro","join":""}
            ])
        } else {
            json!([{"id":9858,"name":"Quantic","join":""}])
        };
        rows.push(json!({
            "id":position,
            "type_":"track",
            "position":position.to_string(),
            "title":if target { "Travelling Song" } else { "Other song" },
            "duration":if target { target_duration } else { "1:00" },
            "artists":artists
        }));
    }
    json!({
        "id":3517186,
        "title":"Look Around The Corner",
        "artists":[{"id":9858,"name":"Quantic","join":" & "}],
        "tracklist":rows
    })
}

fn multi_medium_fixture() -> Fixture {
    let mut value = make_fixture();
    value.track.provenance["position"] = json!(4);
    value.track.provenance["remoteEntryId"] = json!("album_5LvGZBRirAASn8sF0tcWFn_3");
    value.request = spotify_request(&value.track).unwrap();
    value.mb_release["media"] = json!([
        {"position":1,"tracks":[
            {"position":1,"title":"First disc song","length":60000},
            {"position":2,"title":"Second disc song","length":60000},
            {"position":3,"title":"Third disc song","length":60000}
        ]},
        {"position":2,"tracks":[
            {"position":1,"title":"Travelling Song","length":284160}
        ]}
    ]);
    value.discogs_release["tracklist"] = json!([
        {"type_":"track","position":"1","title":"First disc song","duration":"1:00","artists":[{"id":9858,"name":"Quantic","join":""}]},
        {"type_":"track","position":"2","title":"Second disc song","duration":"1:00","artists":[{"id":9858,"name":"Quantic","join":""}]},
        {"type_":"track","position":"3","title":"Third disc song","duration":"1:00","artists":[{"id":9858,"name":"Quantic","join":""}]},
        {"type_":"heading","title":"Second disc"},
        {"type_":"track","position":"1","title":"Travelling Song","duration":"4:44","artists":[
            {"id":9858,"name":"Quantic","join":", "},
            {"id":31262,"name":"Alice Russell","join":" with "},
            {"id":2698689,"name":"The Combo Bárbaro","join":""}
        ]}
    ]);
    value
}

fn verified(fixture: &Fixture) -> Option<VerifiedLexicalCredits> {
    verify_release_track(
        &fixture.request,
        &fixture.mb_release,
        &fixture.discogs_release,
        DISCOGS_RELEASE_ID,
        &fixture.mapped_discogs_ids,
        &fixture.alias_profiles,
    )
}

fn assert_rejected(fixture: &Fixture) {
    assert!(verified(fixture).is_none());
}

#[test]
fn exact_album_request_and_release_id_alias_produce_lexical_names_only() {
    let fixture = make_fixture();
    assert_eq!(fixture.request.spotify_track_id, "2EO1Mu1ynYzzR5v3h7DPmP");
    assert_eq!(fixture.request.spotify_album_id, "5LvGZBRirAASn8sF0tcWFn");
    assert_eq!(fixture.request.album_position, 3);
    assert_eq!(fixture.request.duration_ms, 284_160);
    assert_eq!(fixture.request.credits.len(), 3);

    let result = verified(&fixture).expect("exact track and explicit Discogs alias ID edge");
    assert_eq!(result.release_id, DISCOGS_RELEASE_ID);
    assert_eq!(result.track_position, "3");
    assert_eq!(result.names[0], vec!["Quantic".to_owned()]);
    assert_eq!(result.names[1], vec!["Alice Russell".to_owned()]);
    assert_eq!(
        result.names[2],
        vec![
            "Quantic and His Combo Barbaro".to_owned(),
            "The Combo Bárbaro".to_owned()
        ]
    );
    assert_eq!(
        result.join_phrases,
        vec![", ".to_owned(), " with ".to_owned(), "".to_owned()]
    );
}

#[test]
fn album_global_position_maps_through_multidisc_medium_coordinates() {
    let fixture = multi_medium_fixture();
    let (release_id, medium, position, length) =
        exact_musicbrainz_occurrence(&fixture.request, &fixture.mb_release).unwrap();
    assert_eq!(release_id, DISCOGS_RELEASE_ID);
    assert_eq!((medium, position, length), (2, 1, 284_160));
    let result = verified(&fixture).expect("global album index maps to disc two, track one");
    assert_eq!(result.track_position, "1");
}

#[test]
fn rejects_bad_or_ambiguous_public_release_relations() {
    let mut fixture = make_fixture();
    for resource in [
        "http://www.discogs.com/release/3517186",
        "https://user@www.discogs.com/release/3517186",
        "https://www.discogs.com:444/release/3517186",
        "https://www.discogs.com/release/3517186?redirect=1",
        "https://www.discogs.com/release/3517186#frag",
        "https://evil.example/release/3517186",
        "https://www.discogs.com/artist/3517186",
    ] {
        assert_eq!(discogs_release_id_from_url(resource), None, "{resource}");
    }
    assert_eq!(
        discogs_release_id_from_url("https://www.discogs.com/release/3517186-Look_Around"),
        Some(DISCOGS_RELEASE_ID.to_owned())
    );

    fixture.mb_release["relations"] = json!([]);
    assert_rejected(&fixture);
    fixture.mb_release["relations"] = json!([
        {"target-type":"url","url":{"resource":"https://www.discogs.com/release/3517186"}},
        {"target-type":"url","url":{"resource":"https://www.discogs.com/release/9999999"}}
    ]);
    assert_rejected(&fixture);
}

#[test]
fn rejects_ambiguous_missing_and_wrong_occurrences_or_metadata() {
    let mut fixture = make_fixture();
    fixture.discogs_release["tracklist"][2]["title"] = json!("Different song");
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.discogs_release["title"] = json!("Different album");
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.discogs_release["tracklist"][2]["title"] = json!("Travelling Song (Remix)");
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.mb_release["media"][0]["tracks"][2]["length"] = json!(300_000);
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.discogs_release["tracklist"][2]["duration"] = json!("5:00");
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.mb_release["media"][0]["tracks"][2]["position"] = json!(2);
    assert_rejected(&fixture);
}

#[test]
fn requires_exact_ordered_primary_artist_ids_and_explicit_alias_edge() {
    let mut fixture = make_fixture();
    fixture.discogs_release["tracklist"][2]["artists"][0]["id"] = json!(31262);
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.discogs_release["tracklist"][2]["artists"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.discogs_release["tracklist"][2]["artists"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":777,"name":"Extra performer","join":""}));
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.discogs_release["tracklist"][2]["artists"] = json!([
        {"id":9858,"name":"Quantic","join":", "},
        {"id":31262,"name":"Alice Russell","join":" with "}
    ]);
    fixture.discogs_release["tracklist"][2]["extraartists"] = json!([
        {"id":2698689,"name":"The Combo Bárbaro","role":"Composer","join":""}
    ]);
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.alias_profiles.clear();
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.alias_profiles.insert(
        DISCOGS_ARTIST_IDS[2].into(),
        json!({"id":1420481,"aliases":[{"id":123456,"name":"The Combo Bárbaro"}]}),
    );
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.alias_profiles.insert(
        DISCOGS_ARTIST_IDS[2].into(),
        json!({"id":1420481,"aliases":[{"id":2698689,"name":"Other homonym"}]}),
    );
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.alias_profiles.insert(DISCOGS_ARTIST_IDS[2].into(), json!({"id":1420481,"aliases":[{"id":2698689,"name":"The Combo Bárbaro"},{"id":2698689,"name":"Other homonym"}]}));
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.mapped_discogs_ids[2] = "0".into();
    assert_rejected(&fixture);

    let mut fixture = make_fixture();
    fixture.mapped_discogs_ids[2] = fixture.mapped_discogs_ids[1].clone();
    assert_rejected(&fixture);
}

#[test]
fn request_rejects_manual_range_version_malformed_isrc_and_bad_credit_claims() {
    let mut track = make_fixture().track;
    track.version = Some("Remix".into());
    assert!(spotify_request(&track).is_none());

    let mut track = make_fixture().track;
    track.playback_range = Some(crate::models::PlaybackRange {
        start_time: 1.0,
        end_time: 10.0,
        parent_track_id: "parent".into(),
        chapter_id: "chapter".into(),
        source_url: "https://example.test".into(),
        parent_title: "Parent".into(),
        parent_duration_seconds: None,
    });
    assert!(spotify_request(&track).is_none());

    let mut track = make_fixture().track;
    track.provenance["manualFields"] = json!({"artist":true});
    assert!(spotify_request(&track).is_none());

    let mut track = make_fixture().track;
    track.isrc = Some("NOT-AN-ISRC".into());
    assert!(spotify_request(&track).is_none());

    let mut track = make_fixture().track;
    track.isrc = Some("USABC2400001".into());
    track.provenance["remoteCatalog"] = json!({
        "trackId":track.provider_id,
        "title":track.title,
        "artist":track.artist,
        "durationSeconds":track.duration_seconds,
        "isrc":"USXYZ2400002"
    });
    assert!(spotify_request(&track).is_none());

    let mut track = make_fixture().track;
    let duplicate_id = track.provenance["artistCredits"][1]["providerId"].clone();
    track.provenance["artistCredits"][2]["providerId"] = json!(duplicate_id);
    assert!(spotify_request(&track).is_none());

    let mut track = make_fixture().track;
    track.provenance["artistCredits"][2]["name"] = json!("Wrong Group");
    assert!(spotify_request(&track).is_none());
}

fn seed_warm_proof(store: &Store, fixture: &Fixture) {
    let now = chrono::Utc::now().timestamp();
    for (index, credit) in fixture.request.credits.iter().enumerate() {
        let spotify_url = format!("https://open.spotify.com/artist/{}", credit.id);
        let mb_id = MB_ARTIST_IDS[index];
        let discogs_id = DISCOGS_ARTIST_IDS[index];
        let profile = json!({
            "id":mb_id,
            "name":credit.name,
            "aliases":[],
            "relations":[
                {"target-type":"url","url":{"resource":spotify_url}},
                {"target-type":"url","url":{"resource":format!("https://www.discogs.com/artist/{discogs_id}")}}
            ]
        });
        let lookup = json!({
            "resource":spotify_url,
            "relations":[{"target-type":"artist","artist":{"id":mb_id}}]
        });
        store
            .set_retained_provider_cache(
                PROVIDER_ARTIST_CACHE,
                &format!("spotify:{}", credit.id),
                &json!({"version":1,"provider":"spotify","providerArtistId":credit.id,
                "musicbrainzArtistId":mb_id,"lookup":lookup,"profile":profile,"fetchedAt":now}),
            )
            .unwrap();
        store
            .set_retained_provider_cache(MB_PROFILE_CACHE, mb_id, &profile)
            .unwrap();
        store
            .set_provider_cache(
                MB_FETCHED_AT_CACHE,
                &format!("{MB_PROFILE_CACHE}:{mb_id}"),
                &json!({"fetchedAt":now}),
                3600,
            )
            .unwrap();
    }
    let album_url = format!(
        "https://open.spotify.com/album/{}",
        fixture.request.spotify_album_id
    );
    let album_locator = json!({
        "resource": album_url,
        "relations": [{"target-type":"release","release":{"id":MB_RELEASE_ID}}]
    });
    store
        .set_retained_provider_cache(
            ALBUM_URL_CACHE,
            &fixture.request.spotify_album_id,
            &album_locator,
        )
        .unwrap();
    store
        .set_provider_cache(
            MB_FETCHED_AT_CACHE,
            &format!("{ALBUM_URL_CACHE}:{}", fixture.request.spotify_album_id),
            &json!({"fetchedAt":now}),
            3600,
        )
        .unwrap();
    store
        .set_retained_provider_cache(MB_RELEASE_CACHE, MB_RELEASE_ID, &fixture.mb_release)
        .unwrap();
    store
        .set_provider_cache(
            MB_FETCHED_AT_CACHE,
            &format!("{MB_RELEASE_CACHE}:{MB_RELEASE_ID}"),
            &json!({"fetchedAt":now}),
            3600,
        )
        .unwrap();
    store
        .set_retained_provider_cache(
            artist_credit_aliases::DISCOGS_ALIAS_RELEASE_CACHE,
            DISCOGS_RELEASE_ID,
            &fixture.discogs_release,
        )
        .unwrap();
    store
        .set_provider_cache(
            DISCOGS_FETCHED_AT_CACHE,
            &format!(
                "{}:{DISCOGS_RELEASE_ID}",
                artist_credit_aliases::DISCOGS_ALIAS_RELEASE_CACHE
            ),
            &json!({"fetchedAt":now}),
            3600,
        )
        .unwrap();
    for (id, profile) in &fixture.alias_profiles {
        store
            .set_retained_provider_cache(
                artist_credit_aliases::DISCOGS_ALIAS_PROFILE_CACHE,
                id,
                profile,
            )
            .unwrap();
        store
            .set_provider_cache(
                DISCOGS_FETCHED_AT_CACHE,
                &format!(
                    "{}:{id}",
                    artist_credit_aliases::DISCOGS_ALIAS_PROFILE_CACHE
                ),
                &json!({"fetchedAt":now}),
                3600,
            )
            .unwrap();
    }
    store
        .set_provider_cache(
            LEXICAL_BINDING_CACHE,
            &fixture.request.request_key,
            &json!({
                "version":1,"requestKey":fixture.request.request_key,
                "musicbrainzReleaseId":MB_RELEASE_ID,"discogsReleaseId":DISCOGS_RELEASE_ID,
                "locatorKind":"spotify_album_url"
            }),
            DISCOVERY_TTL_SECONDS,
        )
        .unwrap();
}

#[tokio::test]
async fn cached_album_locator_rejects_wrong_resource_before_using_relations() {
    let fixture = make_fixture();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("wrong-album-url.sqlite")).unwrap();
    seed_warm_proof(&store, &fixture);

    let mut wrong_locator = store
        .get_provider_cache(ALBUM_URL_CACHE, &fixture.request.spotify_album_id)
        .unwrap()
        .unwrap();
    wrong_locator["resource"] = json!("https://open.spotify.com/album/4cOdK2wGLETKBW3PvgPWqT");
    assert_eq!(
        wrong_locator["relations"][0]["release"]["id"].as_str(),
        Some(MB_RELEASE_ID),
        "the stale cross-key payload still contains a tempting release relation"
    );
    store
        .set_retained_provider_cache(
            ALBUM_URL_CACHE,
            &fixture.request.spotify_album_id,
            &wrong_locator,
        )
        .unwrap();

    let (ids, kind) = musicbrainz_release_locators(&store, &fixture.request)
        .await
        .expect("cached locator rejection is offline and non-fatal");
    assert!(ids.is_empty());
    assert_eq!(kind, "spotify_album_url");
}

#[tokio::test]
async fn cached_proof_is_revalidated_and_lookup_uses_pre_hydration_path() {
    let fixture = make_fixture();
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&directory.path().join("lexical.sqlite")).unwrap());
    seed_warm_proof(&store, &fixture);
    assert!(cached_lexical_credits(&store, &fixture.track).is_some());
    assert!(has_cached_lexical_alias(&store, &fixture.track));
    let aliases = crate::artist_credit_aliases::cached_verified_playback_artist_aliases(
        &store,
        &fixture.track,
    );
    assert!(
        !aliases.has_verified_credit_sets(),
        "lexical evidence is not a complete credit-set assertion"
    );
    let lexical_artist = aliases
        .tracks()
        .iter()
        .find(|variant| variant.artist.contains("The Combo Bárbaro"))
        .map(|variant| variant.artist.as_str())
        .expect("request-scoped spelling is available for matching");
    assert!(aliases
        .matching_verified_credit_names(&fixture.track, lexical_artist)
        .is_none());
    assert!(aliases
        .matching_lexical_credit_names(&fixture.track, lexical_artist)
        .is_some());
    assert!(aliases
        .matching_track(&fixture.track, lexical_artist)
        .is_some());
    assert!(aliases
        .discogs_credit_variant(&fixture.track, lexical_artist)
        .is_none());
    assert!(aliases.verified_recording_isrc().is_none());
    assert!(aliases.isrc_enriched().is_empty());
    assert!(store
        .get_library_track(&fixture.track.id)
        .unwrap()
        .is_none());

    let connections = Arc::new(crate::connections::ConnectionManager::new(store.clone()));
    let manager =
        crate::acquisition::AcquisitionManager::new_for_playback_test(store.clone(), connections);
    let trace = crate::playback_sources::SourceTrace::new(None, None, None);
    let (_cancel_sender, cancel) = tokio::sync::watch::channel(false);
    crate::artist_alias_lookup::lookup(
        &manager,
        &fixture.track,
        cancel,
        crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(2)),
        &trace,
    )
    .await
    .unwrap();
    assert!(trace.fresh_exact_alias_track(&fixture.track).is_none());
    assert!(store
        .get_library_track(&fixture.track.id)
        .unwrap()
        .is_none());
}

#[test]
fn cached_lexical_context_resolves_flexible_match_without_closing_exact_credits() {
    let fixture = make_fixture();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("lexical-match-context.sqlite")).unwrap();
    seed_warm_proof(&store, &fixture);
    let candidate_title = "Quantic, Alice Russell - Travelling Song - feat. The Combo Bárbaro";
    let candidate = crate::models::Candidate {
        provider: "youtube".into(),
        id: "youtube:x5JgogqFYOM".into(),
        url: "https://www.youtube.com/watch?v=x5JgogqFYOM".into(),
        title: candidate_title.into(),
        uploader: Some("TruThoughtsJukeBox".into()),
        duration_seconds: Some(284.0),
        source_metadata: Some(json!({
            "provider":"youtube",
            "resolvedProviderId":"x5JgogqFYOM",
            "sourceMetadataOrigin":"ytDlpFullInfo",
            "duration":284.0,
            "artists":null,"artist":null,"album":null,"isrc":null
        })),
        ..Default::default()
    };

    for level in [
        crate::recording_match::MatchLevel::Exact,
        crate::recording_match::MatchLevel::Balanced,
        crate::recording_match::MatchLevel::Flexible,
    ] {
        let search = crate::recording_match_context::assess(
            &store,
            &fixture.track,
            &candidate,
            level,
            crate::recording_match::MatchStage::Search,
        );
        assert_eq!(
            search.action,
            crate::recording_match::MatchAction::Inspect,
            "search stays inspect-only at {level:?}: {search:?}"
        );

        let resolved = crate::recording_match_context::assess(
            &store,
            &fixture.track,
            &candidate,
            level,
            crate::recording_match::MatchStage::Resolved,
        );
        match level {
            crate::recording_match::MatchLevel::Flexible => {
                assert_eq!(
                    resolved.action,
                    crate::recording_match::MatchAction::Accept,
                    "flexible matching can use the exact linked-release spelling: {resolved:?}"
                );
            }
            crate::recording_match::MatchLevel::Exact => {
                assert_ne!(
                    resolved.action,
                    crate::recording_match::MatchAction::Accept,
                    "spelling-only evidence cannot establish exact identity: {resolved:?}"
                );
                let evidence = resolved.evidence.as_ref().unwrap();
                assert!(!evidence.complete_performer_credits);
                assert!(!evidence.requested_performer_credits_complete);
            }
            crate::recording_match::MatchLevel::Balanced => {
                assert_ne!(resolved.action, crate::recording_match::MatchAction::Reject);
            }
        }
    }
}

#[test]
fn malformed_stale_or_future_cache_context_cannot_authorize_cached_lexical_names() {
    let fixture = make_fixture();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("cache.sqlite")).unwrap();
    seed_warm_proof(&store, &fixture);
    let request = &fixture.request;
    let artist_cache_key = format!("spotify:{}", request.credits[0].id);
    let mut binding = store
        .get_provider_cache(PROVIDER_ARTIST_CACHE, &artist_cache_key)
        .unwrap()
        .unwrap();
    binding["lookup"]["resource"] = json!("https://open.spotify.com/artist/other");
    store
        .set_retained_provider_cache(PROVIDER_ARTIST_CACHE, &artist_cache_key, &binding)
        .unwrap();
    assert!(cached_lexical_credits(&store, &fixture.track).is_none());
}

#[test]
fn exact_spotify_artist_id_does_not_authorize_a_contradictory_credit_name() {
    let fixture = make_fixture();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("artist-name.sqlite")).unwrap();
    seed_warm_proof(&store, &fixture);

    assert_eq!(
        cached_discogs_artist_id(&store, &fixture.request.credits[0].id, "Quantic").as_deref(),
        Some(DISCOGS_ARTIST_IDS[0])
    );
    assert!(cached_discogs_artist_id(&store, &fixture.request.credits[0].id, "Queen").is_none());

    let profile = json!({
        "id": MB_ARTIST_IDS[0],
        "name": "Quantic",
        "aliases": [{"name":"Will Holland","type":"Artist name"}]
    });
    assert!(musicbrainz_profile_owns_name(&profile, "Quantic"));
    assert!(musicbrainz_profile_owns_name(&profile, "Will Holland"));
    assert!(!musicbrainz_profile_owns_name(&profile, "Queen"));
    let search_hint_only = json!({
        "id": MB_ARTIST_IDS[0],
        "name": "Quantic",
        "aliases": [{"name":"Queen","type":"Search hint"}]
    });
    assert!(!musicbrainz_profile_owns_name(&search_hint_only, "Queen"));
}

#[test]
fn retained_spotify_artist_name_matches_real_ampersand_alias_only_on_exact_profile() {
    let fixture = make_fixture();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("spotify-artist-spelling.sqlite")).unwrap();
    let credit = &fixture.request.credits[2];
    let spotify_url = format!("https://open.spotify.com/artist/{}", credit.id);
    let profile = json!({
        "id": MB_ARTIST_IDS[2],
        "name": "Quantic and his Combo Bárbaro",
        "aliases": [{"name":"Quantic & His Combo Barbaro","type":"Artist name"}],
        "relations": [
            {"target-type":"url","url":{"resource":spotify_url}},
            {"target-type":"url","url":{"resource":format!("https://www.discogs.com/artist/{}",DISCOGS_ARTIST_IDS[2])}}
        ]
    });
    let lookup = json!({
        "resource": spotify_url,
        "relations": [{"target-type":"artist","artist":{"id":MB_ARTIST_IDS[2]}}]
    });
    let now = chrono::Utc::now().timestamp();
    store
        .set_retained_provider_cache(
            PROVIDER_ARTIST_CACHE,
            &format!("spotify:{}", credit.id),
            &json!({
                "version":1,"provider":"spotify","providerArtistId":credit.id,
                "musicbrainzArtistId":MB_ARTIST_IDS[2],"lookup":lookup,
                "profile":profile,"fetchedAt":now
            }),
        )
        .unwrap();
    store
        .set_retained_provider_cache(MB_PROFILE_CACHE, MB_ARTIST_IDS[2], &profile)
        .unwrap();
    store
        .set_provider_cache(
            MB_FETCHED_AT_CACHE,
            &format!("{MB_PROFILE_CACHE}:{}", MB_ARTIST_IDS[2]),
            &json!({"fetchedAt":now}),
            3600,
        )
        .unwrap();

    assert_eq!(credit.name, "Quantic and His Combo Barbaro");
    assert_eq!(
        cached_discogs_artist_id(&store, &credit.id, &credit.name).as_deref(),
        Some(DISCOGS_ARTIST_IDS[2])
    );
    assert!(cached_discogs_artist_id(&store, &credit.id, "The Combo Barbaro").is_none());

    let mut wrong_mapping = lookup.clone();
    wrong_mapping["relations"][0]["artist"]["id"] = json!(MB_ARTIST_IDS[1]);
    let mut wrong_binding = json!({
        "version":1,"provider":"spotify","providerArtistId":credit.id,
        "musicbrainzArtistId":MB_ARTIST_IDS[2],"lookup":wrong_mapping,
        "profile":profile,"fetchedAt":now
    });
    store
        .set_retained_provider_cache(
            PROVIDER_ARTIST_CACHE,
            &format!("spotify:{}", credit.id),
            &wrong_binding,
        )
        .unwrap();
    assert!(cached_discogs_artist_id(&store, &credit.id, &credit.name).is_none());

    wrong_binding["lookup"] = lookup;
    wrong_binding["profile"]["aliases"] = json!([
        {"name":"Quantic & His Combo Barbaro","type":"Search hint"}
    ]);
    store
        .set_retained_provider_cache(
            PROVIDER_ARTIST_CACHE,
            &format!("spotify:{}", credit.id),
            &wrong_binding,
        )
        .unwrap();
    store
        .set_retained_provider_cache(
            MB_PROFILE_CACHE,
            MB_ARTIST_IDS[2],
            &wrong_binding["profile"],
        )
        .unwrap();
    assert!(cached_discogs_artist_id(&store, &credit.id, &credit.name).is_none());

    seed_warm_proof(&store, &fixture);
    assert!(cached_discogs_artist_id(
        &store,
        &fixture.request.credits[0].id,
        "Quantic and His Combo Barbaro"
    )
    .is_none());
    let key = format!("spotify:{}", credit.id);
    let mut exact_binding = store
        .get_provider_cache(PROVIDER_ARTIST_CACHE, &key)
        .unwrap()
        .unwrap();
    exact_binding["profile"] = profile.clone();
    store
        .set_retained_provider_cache(PROVIDER_ARTIST_CACHE, &key, &exact_binding)
        .unwrap();
    store
        .set_retained_provider_cache(MB_PROFILE_CACHE, MB_ARTIST_IDS[2], &profile)
        .unwrap();
    assert!(cached_lexical_credits(&store, &fixture.track).is_some());

    let mut search_hint_profile = profile;
    search_hint_profile["aliases"] = json!([
        {"name":"Quantic & His Combo Barbaro","type":"Search hint"}
    ]);
    exact_binding["profile"] = search_hint_profile.clone();
    store
        .set_retained_provider_cache(PROVIDER_ARTIST_CACHE, &key, &exact_binding)
        .unwrap();
    store
        .set_retained_provider_cache(MB_PROFILE_CACHE, MB_ARTIST_IDS[2], &search_hint_profile)
        .unwrap();
    assert!(cached_lexical_credits(&store, &fixture.track).is_none());
}

#[test]
fn stale_future_or_malformed_identity_cache_context_is_rejected() {
    let fixture = make_fixture();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("stale-cache.sqlite")).unwrap();
    seed_warm_proof(&store, &fixture);
    let first_artist_key = format!("spotify:{}", fixture.request.credits[0].id);

    let mut binding = store
        .get_provider_cache(PROVIDER_ARTIST_CACHE, &first_artist_key)
        .unwrap()
        .unwrap();
    binding["fetchedAt"] = json!(chrono::Utc::now().timestamp() + 3600);
    store
        .set_retained_provider_cache(PROVIDER_ARTIST_CACHE, &first_artist_key, &binding)
        .unwrap();
    assert!(cached_lexical_credits(&store, &fixture.track).is_none());

    seed_warm_proof(&store, &fixture);
    let wrong_locator = json!({
        "resource": format!("https://open.spotify.com/album/{}", fixture.request.spotify_album_id),
        "relations": [{"target-type":"release","release":{"id":"22222222-2222-4222-8222-222222222222"}}]
    });
    store
        .set_retained_provider_cache(
            ALBUM_URL_CACHE,
            &fixture.request.spotify_album_id,
            &wrong_locator,
        )
        .unwrap();
    assert!(cached_lexical_credits(&store, &fixture.track).is_none());

    seed_warm_proof(&store, &fixture);
    store
        .set_provider_cache(
            MB_FETCHED_AT_CACHE,
            &format!("{MB_RELEASE_CACHE}:{MB_RELEASE_ID}"),
            &json!({"fetchedAt":chrono::Utc::now().timestamp() - DISCOVERY_TTL_SECONDS as i64 - 1}),
            DISCOVERY_TTL_SECONDS,
        )
        .unwrap();
    assert!(cached_lexical_credits(&store, &fixture.track).is_none());

    seed_warm_proof(&store, &fixture);
    store.set_provider_cache(
        LEXICAL_BINDING_CACHE,
        &fixture.request.request_key,
        &json!({"version":1,"requestKey":"wrong","musicbrainzReleaseId":MB_RELEASE_ID,"discogsReleaseId":DISCOGS_RELEASE_ID}),
        DISCOVERY_TTL_SECONDS,
    ).unwrap();
    assert!(cached_lexical_credits(&store, &fixture.track).is_none());
}
