use burrata_lib::{models::Track, Store};
use serde_json::Value;
use std::{collections::HashSet, path::PathBuf};

#[test]
fn legacy_caption_repair_and_atomic_artist_projection_do_not_guess_boundaries() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/legacy-artist-identities.json"))
            .expect("legacy artist fixture is valid JSON");
    let tracks: Vec<Track> =
        serde_json::from_value(fixture["tracks"].clone()).expect("fixture tracks deserialize");
    let temp = tempfile::tempdir().expect("temporary library database");
    let store = Store::open(&PathBuf::from(temp.path()).join("library.sqlite"))
        .expect("open isolated library");
    store
        .save_library_tracks(&tracks)
        .expect("save fixture tracks into isolated library");

    let repaired = store
        .get_track("youtube:LSHM_bkSYQI")
        .expect("read repaired caption track")
        .expect("caption track exists");
    assert_eq!(
        repaired.artist,
        fixture["expected"]["repairedCaptionTrack"]["artist"]
    );
    assert_eq!(
        repaired.title,
        fixture["expected"]["repairedCaptionTrack"]["title"]
    );
    assert_eq!(
        repaired.provenance["artistSource"],
        fixture["expected"]["repairedCaptionTrack"]["artistSource"]
    );

    let artists = store
        .list_library_entities("artist", None, "", 100, 0)
        .expect("project artist cards");
    let names = artists
        .items
        .iter()
        .map(|artist| artist.name.as_str())
        .collect::<HashSet<_>>();
    let expected_atomic = fixture["expected"]["atomicArtistCards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(
        names, expected_atomic,
        "only the structured atomic credit is trusted"
    );
    for fabricated in fixture["expected"]["noArtistCardsFor"].as_array().unwrap() {
        let fabricated = fabricated.as_str().unwrap();
        assert!(
            !names.contains(fabricated),
            "ambiguous or repaired legacy text must not create a fabricated artist card: {fabricated}"
        );
    }

    let atomic = artists
        .items
        .iter()
        .find(|artist| artist.name == "Atomic Band & Friends")
        .expect("one atomic artist card");
    let membership = store
        .list_library_entity_tracks(
            &atomic.id,
            "",
            "sourceOrder",
            false,
            "all",
            None,
            None,
            20,
            0,
        )
        .expect("read atomic artist membership");
    assert_eq!(membership.total, 1);
    assert_eq!(membership.tracks[0].id, "traxsource:atomic-act");
}
