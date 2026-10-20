//! A recording-specific crosswalk from complete, independently fetched albums.
//! Exact millisecond timelines authorize credit/spelling variants only inside
//! these albums. This never creates a global artist alias or accepts a cover.
use super::*;
use std::collections::HashSet;

fn album_name(name: &str) -> String {
    let name = name.trim();
    let base = if let Some((base, suffix)) = name.strip_suffix(')').and_then(|s| s.rsplit_once('('))
    {
        if suffix.split_whitespace().all(|word| {
            word.chars().all(|c| c.is_ascii_digit())
                || matches!(word.to_ascii_lowercase().as_str(), "repress" | "reissue")
        }) {
            base
        } else {
            name
        }
    } else {
        name
    };
    crate::matching::normalize(base)
}
fn comparable_title(name: &str) -> String {
    // Dedications may be absent in another edition. Version labels are kept.
    let base = name
        .strip_suffix(')')
        .and_then(|s| s.rsplit_once('('))
        .filter(|(_, suffix)| {
            let suffix = crate::matching::normalize(suffix);
            !suffix.split_whitespace().any(|word| {
                matches!(
                    word,
                    "live"
                        | "remix"
                        | "mix"
                        | "dub"
                        | "acoustic"
                        | "instrumental"
                        | "demo"
                        | "edit"
                        | "karaoke"
                        | "cover"
                        | "version"
                ) || word.chars().any(|c| c.is_ascii_digit())
            })
        })
        .map_or(name, |(base, _)| base);
    compact(&crate::recording_identity::search_fold(base))
}
fn titles_match(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() || a.len() > 512 || b.len() > 512 {
        return false;
    }
    let comparable_a = comparable_title(a);
    let comparable_b = comparable_title(b);
    let a = crate::matching::normalize(a);
    let b = crate::matching::normalize(b);
    for version in [
        "live",
        "remix",
        "mix",
        "dub",
        "acoustic",
        "instrumental",
        "demo",
        "edit",
        "karaoke",
        "cover",
        "version",
    ] {
        if a.split_whitespace().filter(|word| *word == version).count()
            != b.split_whitespace().filter(|word| *word == version).count()
        {
            return false;
        }
    }
    let digits = |s: &str| s.chars().filter(char::is_ascii_digit).collect::<String>();
    if digits(&a) != digits(&b) {
        return false;
    }
    strsim::normalized_levenshtein(&comparable_a, &comparable_b) >= 0.8
}

fn verify(
    track: &Track,
    candidate: &Candidate,
    reference: &Value,
    apple: &Value,
    bandcamp: &Value,
    expected_album_id: u64,
) -> Option<Value> {
    let rows = apple["results"].as_array()?;
    if rows.len() < 9 || rows.len() > 51 || apple["resultCount"].as_u64() != Some(rows.len() as u64)
    {
        return None;
    }
    let albums = rows
        .iter()
        .filter(|r| r["wrapperType"] == "collection")
        .collect::<Vec<_>>();
    if albums.len() != 1 {
        return None;
    }
    let album = albums[0];
    let album_id = reference["collectionId"].as_u64()?;
    if album["collectionId"].as_u64() != Some(album_id)
        || album["collectionType"] != "Album"
        || bandcamp["current"]["type"] != "album"
        || bandcamp["current"]["id"].as_u64() != Some(expected_album_id)
        || !same_credit(album["artistName"].as_str()?, bandcamp["artist"].as_str()?)
        || album_name(album["collectionName"].as_str()?)
            != album_name(bandcamp["current"]["title"].as_str()?)
        || album["collectionName"] != reference["album"]
    {
        return None;
    }
    let songs = rows
        .iter()
        .filter(|r| r["kind"] == "song")
        .collect::<Vec<_>>();
    let tracks = bandcamp["trackinfo"].as_array()?;
    if songs.len() + 1 != rows.len()
        || songs.len() != tracks.len()
        || album["trackCount"].as_u64() != Some(songs.len() as u64)
    {
        return None;
    }
    if tracks
        .iter()
        .filter_map(|t| t["track_id"].as_u64().filter(|id| *id > 0))
        .collect::<HashSet<_>>()
        .len()
        != tracks.len()
    {
        return None;
    }
    let left = songs
        .iter()
        .map(|s| s["trackTimeMillis"].as_f64().map(|d| d / 1000.0))
        .collect::<Option<Vec<_>>>()?;
    let right = tracks
        .iter()
        .map(|s| s["duration"].as_f64())
        .collect::<Option<Vec<_>>>()?;
    let timeline = crate::recording_identity::complete_album_timeline(&left, &right)?;
    let mut used = HashSet::new();
    let mut ids = HashSet::new();
    let mut vector = vec![];
    let mut target = false;
    for (song_index, song) in songs.into_iter().enumerate() {
        let id = song["trackId"].as_u64()?;
        if !ids.insert(id)
            || song["collectionId"].as_u64() != Some(album_id)
            || song["discCount"] != 1
            || song["discNumber"] != 1
        {
            return None;
        }
        let duration = song["trackTimeMillis"].as_f64()? / 1000.0;
        if !duration.is_finite() || !(1.0..=600.0).contains(&duration) {
            return None;
        }
        let index = timeline[song_index];
        let row = &tracks[index];
        let bandcamp_id = row["track_id"].as_u64()?;
        if bandcamp_id == 0
            || !used.insert(index)
            || !titles_match(song["trackName"].as_str()?, row["title"].as_str()?)
        {
            return None;
        }
        if id == reference["trackId"].as_u64()? {
            if candidate.id != format!("bandcamp:{bandcamp_id}")
                || compact(&track.title) != compact(row["title"].as_str()?)
                || !same_credit(&track.artist, song["artistName"].as_str()?)
            {
                return None;
            }
            target = true;
        }
        vector.push(json!({"catalogTrackId":id,"bandcampTrackId":bandcamp_id,"catalogTitle":song["trackName"],"bandcampTitle":row["title"],"catalogArtist":song["artistName"],"durationMs":song["trackTimeMillis"]}));
    }
    if !target {
        return None;
    }
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&vector).ok()?));
    Some(
        json!({"method":"album-crosswalk-v1","referenceAlbumId":album_id,"bandcampAlbumId":expected_album_id,"albumTrackCount":vector.len(),"albumFingerprint":fingerprint,"albumMatches":vector}),
    )
}

pub(super) async fn resolve(
    track: &Track,
    candidate: &Candidate,
    reference: &Value,
    audio: &crate::acquisition::PreviewAudio,
    cancel: watch::Receiver<bool>,
    deadline: PlaybackDeadline,
) -> Result<Option<Value>, String> {
    let Some(album_id) = reference["collectionId"].as_u64().filter(|id| *id > 0) else {
        return Ok(None);
    };
    let Some(metadata) = audio.source_metadata.as_ref() else {
        return Ok(None);
    };
    let Some(bandcamp_id) = metadata["albumId"].as_u64().filter(|id| *id > 0) else {
        return Ok(None);
    };
    let Some(url) = metadata["albumUrl"]
        .as_str()
        .and_then(|u| crate::bandcamp::validate_album_page_url(u).ok())
    else {
        return Ok(None);
    };
    if reqwest::Url::parse(&candidate.url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .as_deref()
        != url.host_str()
    {
        return Ok(None);
    }
    bounded(cancel, deadline, async {
        let client = client()?;
        let response = client
            .get("https://itunes.apple.com/lookup")
            .query(&[
                ("id", album_id.to_string()),
                ("entity", "song".into()),
                ("limit", "200".into()),
            ])
            .send()
            .await
            .map_err(|_| "Album reference is unavailable")?;
        let apple: Value = serde_json::from_slice(&body(response, MAX_JSON).await?)
            .map_err(|_| "Invalid album reference")?;
        let addresses = crate::acquisition::public_addresses(&url).await?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Burrata/0.1 (public recording playback)")
            .resolve_to_addrs(url.host_str().ok_or("Missing album host")?, &addresses)
            .build()
            .map_err(|_| "Album reference is unavailable")?;
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|_| "Album reference is unavailable")?;
        let bytes = body(response, 2 * 1024 * 1024).await?;
        let Some(bandcamp) =
            crate::collection_sources::extract_tralbum_data(&String::from_utf8_lossy(&bytes))
        else {
            return Ok(None);
        };
        Ok(verify(
            track,
            candidate,
            reference,
            &apple,
            &bandcamp,
            bandcamp_id,
        ))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Track, Candidate, Value, Value, Value) {
        let track = Track {
            title: "Nighttime in the Backyard".into(),
            artist: "Lucas Santtana & Arto Lindsay".into(),
            ..Default::default()
        };
        let candidate:Candidate=serde_json::from_value(json!({"id":"bandcamp:103","provider":"bandcamp","url":"https://artist.bandcamp.com/track/song","title":track.title,"formats":[],"permission":"stream_only","confidence":"review","score":0.0,"reasons":[],"official":false})).unwrap();
        let reference = json!({"collectionId":1,"trackId":3,"album":"Sem Nostalgia"});
        let mut rows = vec![
            json!({"wrapperType":"collection","collectionType":"Album","collectionId":1,"artistName":"Lucas Santtana","collectionName":"Sem Nostalgia","trackCount":8}),
        ];
        let mut tracks = vec![];
        for i in 1..=8 {
            let title = if i == 3 {
                track.title.clone()
            } else {
                format!("Distinct song {i}")
            };
            rows.push(json!({"kind":"song","trackId":i,"collectionId":1,"trackName":title,"artistName":if i==3{track.artist.clone()}else{"Lucas Santtana".into()},"trackTimeMillis":100000+i*10000,"discCount":1,"discNumber":1}));
            tracks.push(json!({"track_id":100+i,"title":if i==3{"Night-time in the backyard".into()}else{title},"duration":100.0+i as f64*10.0}));
        }
        (
            track,
            candidate,
            reference,
            json!({"resultCount":rows.len(),"results":rows}),
            json!({"artist":"Lucas Santtana","current":{"id":2,"type":"album","title":"Sem Nostalgia (2021 repress)"},"trackinfo":tracks}),
        )
    }
    #[test]
    fn reported_nighttime_album_fixture_binds_all_twelve_tracks_without_audio_comparison() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/nighttime-album-crosswalk.json"
        ))
        .unwrap();
        let track = Track {
            title: "Nighttime in the Backyard".into(),
            artist: "Lucas Santtana & Arto Lindsay".into(),
            ..Default::default()
        };
        let candidate = Candidate {
            id: "bandcamp:2065435071".into(),
            ..Default::default()
        };
        let reference =
            json!({"trackId":1677838124u64,"collectionId":1677838121u64,"album":"Sem Nostalgia"});
        let proof = verify(
            &track,
            &candidate,
            &reference,
            &fixture["apple"],
            &fixture["bandcamp"],
            139844721,
        )
        .unwrap();
        assert_eq!(proof["albumTrackCount"], 12);
        let mut wrong = track.clone();
        wrong.artist = "Tostex".into();
        assert!(verify(
            &wrong,
            &candidate,
            &reference,
            &fixture["apple"],
            &fixture["bandcamp"],
            139844721
        )
        .is_none());
    }
    #[test]
    fn complete_timeline_binds_one_recording_despite_guest_credit_and_spelling_differences() {
        let (track, candidate, reference, apple, mut bandcamp) = fixture();
        bandcamp["trackinfo"].as_array_mut().unwrap().reverse();
        assert_eq!(
            verify(&track, &candidate, &reference, &apple, &bandcamp, 2).unwrap()["method"],
            "album-crosswalk-v1"
        );
    }
    #[test]
    fn partial_ambiguous_wrong_artist_version_or_album_identity_never_publish_proof() {
        let (track, candidate, reference, apple, bandcamp) = fixture();
        for (path, value) in [
            (&["artist"][..], json!("Cover Band")),
            (&["current", "id"][..], json!(99)),
            (&["current", "title"][..], json!("Sem Nostalgia (Live)")),
        ] {
            let mut changed = bandcamp.clone();
            let mut field = &mut changed;
            for key in path {
                field = &mut field[*key];
            }
            *field = value;
            assert!(verify(&track, &candidate, &reference, &apple, &changed, 2).is_none());
        }
        for mutate in 0..4 {
            let mut changed = bandcamp.clone();
            match mutate {
                0 => {
                    changed["trackinfo"].as_array_mut().unwrap().pop();
                }
                1 => changed["trackinfo"][0]["duration"] = json!(120.0),
                2 => changed["trackinfo"][0]["title"] = json!("Distinct song 1 (Live)"),
                _ => changed["trackinfo"][0]["duration"] = json!(110.1),
            }
            assert!(
                verify(&track, &candidate, &reference, &apple, &changed, 2).is_none(),
                "mutation {mutate}"
            );
        }
    }
}
