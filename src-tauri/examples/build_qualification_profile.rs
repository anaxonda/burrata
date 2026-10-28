//! Build a disposable local profile for end-to-end backup, restore, waveform,
//! analysis, and normalization qualification.
//!
//! Run from the repository root with a new absolute path:
//! `BURRATA_QUALIFICATION_PROFILE=/tmp/burrata-qualification-unique cargo run --manifest-path src-tauri/Cargo.toml --example build_qualification_profile`
//! The builder refuses any existing destination and accepts only paths beneath
//! the system temporary directory or this repository's `.cache` directory.

use burrata_lib::{
    autotag::{self, AutoTagJob},
    models::{LocalAsset, Settings, Track},
    playback_history::PlaybackHistory,
    playlist_hierarchy::{
        self, OccurrenceMetadata, PlaylistFolder, PlaylistPlacement, PlaylistSection,
    },
    restore_work_policy::{self, AutomaticWorkKind},
    storage::Store,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    env,
    error::Error,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const SAMPLE_RATE: u32 = 44_100;
const DURATION_SECONDS: u32 = 20;
const CHANNELS: u16 = 2;

fn main() {
    if let Err(error) = build() {
        eprintln!("qualification profile builder failed: {error}");
        std::process::exit(1);
    }
}

fn build() -> Result<(), Box<dyn Error>> {
    let second_gain_db = env::var("BURRATA_QUALIFICATION_SECOND_GAIN_DB")
        .unwrap_or_else(|_| "0".into())
        .parse::<f64>()?;
    if !second_gain_db.is_finite() || !(-24.0..=0.0).contains(&second_gain_db) {
        return Err("second fixture gain must be finite and between -24 and 0 dB".into());
    }
    let destination = env::var_os("BURRATA_QUALIFICATION_PROFILE")
        .map(PathBuf::from)
        .ok_or("Set BURRATA_QUALIFICATION_PROFILE to a new path under the system temporary directory or repo .cache")?;
    let profile_root = validate_destination(&destination)?;
    fs::create_dir(&profile_root)?;
    let data_dir = profile_root.join("data");
    let media_dir = profile_root.join("media");
    let backup_destination = profile_root.join("backup-destination");
    fs::create_dir(&data_dir)?;
    fs::create_dir(&media_dir)?;
    fs::create_dir(&backup_destination)?;
    fs::write(
        profile_root.join(".burrata-qualification-profile"),
        b"synthetic fixture only\n",
    )?;

    if env::var("BURRATA_QUALIFICATION_EMPTY_PROFILE").is_ok_and(|value| value == "1") {
        let inventory = json!({
            "format":"org.burrata.qualification-profile",
            "schemaVersion":1,
            "profileRoot":profile_root,
            "appDataDirectory":data_dir,
            "backupDestination":backup_destination,
            "launchEnvironment":{"BURRATA_APP_DATA":data_dir},
            "syntheticOnly":true,
            "externalAudioUsed":false,
            "tracks":[]
        });
        let inventory_path = profile_root.join("qualification-inventory.json");
        let mut inventory_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&inventory_path)?;
        serde_json::to_writer_pretty(&mut inventory_file, &inventory)?;
        inventory_file.write_all(b"\n")?;
        inventory_file.sync_all()?;
        println!("{}", serde_json::to_string_pretty(&inventory)?);
        return Ok(());
    }

    let audio_specs = [
        ("fixture-c-major.wav", [261.63, 329.63, 392.00]),
        ("fixture-a-minor.wav", [220.00, 261.63, 329.63]),
    ];
    let mut audio = Vec::new();
    for (index, (file_name, chord)) in audio_specs.into_iter().enumerate() {
        let path = media_dir.join(file_name);
        let gain_db = if index == 1 { second_gain_db } else { 0.0 };
        write_synthetic_wav(&path, chord, 10.0_f64.powf(gain_db / 20.0))?;
        let canonical = fs::canonicalize(&path)?;
        let (bytes, sha256) = hash_file(&canonical)?;
        let lyrics_path = canonical.with_extension("lrc");
        let lyrics = "[00:00.00]Synthetic qualification fixture\n[00:05.00]120 BPM click reference\n[00:10.00]No external audio\n";
        fs::write(&lyrics_path, lyrics.as_bytes())?;
        audio.push((canonical, bytes, sha256, lyrics_path));
    }

    // All durable rows are created through Burrata's production APIs.
    let store = Store::open(&data_dir.join("burrata.sqlite"))?;
    let history = PlaybackHistory::open(&data_dir.join("playback-history.sqlite"))
        .map_err(io::Error::other)?;
    let mut tracks = audio
        .iter()
        .enumerate()
        .map(|(index, (path, bytes, hash, _))| {
            let (artist, title, bpm, key, origin) = if index == 0 {
                (
                    "Qualification Ensemble",
                    "Synthetic C Major",
                    120.0,
                    "8B",
                    "imported",
                )
            } else {
                (
                    "Qualification Ensemble",
                    "Synthetic A Minor",
                    118.0,
                    "7A",
                    "imported before manual edit",
                )
            };
            let asset_id = format!("fixture-asset-{}", &hash[..16]);
            let track = Track {
                id: format!("local:{hash}"),
                provider: "local".into(),
                provider_id: hash.clone(),
                source_url: format!("local:{hash}"),
                artist: artist.into(),
                title: title.into(),
                album: Some("Isolated Qualification Set".into()),
                album_artist: Some(artist.into()),
                genre: Some("Synthetic".into()),
                duration_seconds: Some(DURATION_SECONDS as f64),
                bpm: Some(bpm),
                key: Some(key.into()),
                local_path: Some(path.to_string_lossy().into_owned()),
                local_availability: "available".into(),
                asset_id: Some(asset_id),
                provenance: json!({
                    "fixture":"burrata-local-qualification-v1",
                    "bpmOrigin":origin,
                    "keyOrigin":origin
                }),
                ..Track::default()
            };
            let _ = bytes;
            track
        })
        .collect::<Vec<_>>();

    store
        .save_library_tracks(&tracks)
        .map_err(io::Error::other)?;
    let mut manual = store
        .get_track(&tracks[1].id)
        .map_err(io::Error::other)?
        .ok_or("manual-metadata track disappeared after save")?;
    manual.bpm = Some(121.5);
    manual.key = Some("8A".into());
    manual.provenance = json!({
        "fixture":"burrata-local-qualification-v1",
        "bpmOrigin":"manual override",
        "keyOrigin":"manual override"
    });
    store.update_track(&manual).map_err(io::Error::other)?;
    tracks[0] = store
        .get_track(&tracks[0].id)
        .map_err(io::Error::other)?
        .ok_or("imported-metadata track disappeared")?;
    tracks[1] = store
        .get_track(&tracks[1].id)
        .map_err(io::Error::other)?
        .ok_or("manual-metadata track disappeared")?;
    store
        .set_favorite(&tracks[0], true)
        .map_err(io::Error::other)?;

    let assets = audio
        .iter()
        .enumerate()
        .map(|(index, (path, bytes, hash, _))| {
            // Track reads acquire their asset association only after save_asset.
            let asset_id = format!("fixture-asset-{}", &hash[..16]);
            let asset = LocalAsset {
                id: asset_id,
                recording_id: store
                    .recording_id(&tracks[index].id)
                    .map_err(io::Error::other)?,
                path: path.to_string_lossy().into_owned(),
                hash: hash.clone(),
                source_codec: "wav".into(),
                final_codec: "wav".into(),
                source_bitrate: Some((bytes * 8) as f64 / DURATION_SECONDS as f64 / 1000.0),
                final_bitrate: Some((bytes * 8) as f64 / DURATION_SECONDS as f64 / 1000.0),
                sample_rate: Some(SAMPLE_RATE),
                channels: Some(CHANNELS as u32),
                bit_depth: Some(16),
                duration_seconds: Some(DURATION_SECONDS as f64),
                size_bytes: Some(*bytes),
                bitrate_origin: Some("file-size-estimate".into()),
                processing: "synthetic qualification fixture".into(),
                track: tracks[index].clone(),
            };
            store.save_asset(&asset).map_err(io::Error::other)?;
            Ok::<_, Box<dyn Error>>(asset)
        })
        .collect::<Result<Vec<_>, _>>()?;

    for track in &mut tracks {
        *track = store
            .get_track(&track.id)
            .map_err(io::Error::other)?
            .ok_or("synthetic track disappeared after asset registration")?;
    }

    let collection = store
        .create_collection("Qualification Duplicate Set")
        .map_err(io::Error::other)?;
    let occurrences = vec![tracks[0].clone(), tracks[1].clone(), tracks[0].clone()];
    store
        .append_playlist_entries(&collection.id, &occurrences)
        .map_err(io::Error::other)?;
    let ordered = store
        .list_playlist_occurrences(&collection.id)
        .map_err(io::Error::other)?;
    if ordered.len() != 3 || ordered[0].listing_id != ordered[2].listing_id {
        return Err("failed to seed duplicate playlist occurrences".into());
    }
    let parent_folder = PlaylistFolder {
        id: Uuid::new_v4().to_string(),
        name: "Qualification Crates".into(),
        parent_id: None,
        position: 0,
    };
    let nested_folder = PlaylistFolder {
        id: Uuid::new_v4().to_string(),
        name: "Two Track Set".into(),
        parent_id: Some(parent_folder.id.clone()),
        position: 0,
    };
    let warmup = PlaylistSection {
        id: Uuid::new_v4().to_string(),
        name: "Warm Up".into(),
        position: 0,
    };
    let close = PlaylistSection {
        id: Uuid::new_v4().to_string(),
        name: "Close".into(),
        position: 1,
    };
    playlist_hierarchy::save(
        &store,
        &playlist_hierarchy::PlaylistHierarchy {
            schema_version: 1,
            folders: vec![parent_folder, nested_folder.clone()],
            playlists: vec![PlaylistPlacement {
                collection_id: collection.id.clone(),
                folder_id: Some(nested_folder.id),
                position: 0,
                sections: vec![warmup.clone(), close.clone()],
                occurrences: ordered
                    .iter()
                    .enumerate()
                    .map(|(index, occurrence)| OccurrenceMetadata {
                        id: occurrence.occurrence_id.clone(),
                        section_id: Some(if index == 2 {
                            close.id.clone()
                        } else {
                            warmup.id.clone()
                        }),
                        note: Some(format!("Synthetic occurrence {}", index + 1)),
                        cue_points: if index == 1 {
                            vec![2.0, 8.0]
                        } else {
                            Vec::new()
                        },
                        playback_range: None,
                    })
                    .collect(),
            }],
        },
    )
    .map_err(io::Error::other)?;

    history
        .record(&Uuid::new_v4().to_string(), "local", &tracks[0], None)
        .map_err(io::Error::other)?;
    history
        .record(&Uuid::new_v4().to_string(), "local", &tracks[1], None)
        .map_err(io::Error::other)?;

    let queue_entry_ids = vec![Uuid::new_v4().to_string(), Uuid::new_v4().to_string()];
    let queue_revision = 7u64;
    let queue = json!({
        "tracks": tracks,
        "entryIds": queue_entry_ids,
        "queueRevision": queue_revision
    });
    let session = json!({
        "tracks": tracks,
        "entryIds": queue_entry_ids,
        "index": 1,
        "currentEntryId": queue_entry_ids[1],
        "queueRevision": queue_revision,
        "position": 4.25,
        "volume": 0.0,
        "repeat": "all",
        "shuffle": false,
        "status": "paused",
        "finished": false
    });
    store
        .save_playback_checkpoint(Some(&queue), Some(&session))
        .map_err(io::Error::other)?;

    let mut settings = Settings::default();
    settings.music_directory = media_dir.to_string_lossy().into_owned();
    settings.auto_preview = false;
    settings.live_bpm_enabled = true;
    settings.theme = "dark".into();
    store.save_settings(&settings).map_err(io::Error::other)?;
    store
        .set_json_preference(
            "loudness.v1",
            &json!({
                "mode":"track",
                "targetLufs":-18.0,
                "ceilingDbtp":-1.0,
                "maximumBoostDb":6.0
            }),
        )
        .map_err(io::Error::other)?;
    store
        .set_json_preference(
            "automation.permissions.v1",
            &json!({"read":true,"control":true,"manage":false}),
        )
        .map_err(io::Error::other)?;

    // Optional, fully synthetic pending AutoTag fixture for restore review
    // qualification. The marker is set before the profile can be launched, so
    // the queued record cannot reach recognition, providers, or file tagging
    // until an explicit review is performed in a restored destination.
    let queued_autotag_fixture =
        if env::var("BURRATA_QUALIFICATION_QUEUED_AUTOTAG").is_ok_and(|value| value == "1") {
            let path = media_dir.join("fixture-untagged-autotag.wav");
            write_synthetic_wav(&path, [220.0, 293.66, 349.23], 0.55)?;
            let canonical = fs::canonicalize(&path)?;
            let (bytes, sha256) = hash_file(&canonical)?;
            let asset_id = format!("fixture-asset-{}", &sha256[..16]);
            let mut track = Track {
                id: format!("local:{sha256}"),
                provider: "local".into(),
                provider_id: sha256.clone(),
                source_url: format!("local:{sha256}"),
                artist: "Synthetic Fixture Artist".into(),
                title: "Synthetic Untagged AutoTag Fixture".into(),
                duration_seconds: Some(DURATION_SECONDS as f64),
                local_path: Some(canonical.to_string_lossy().into_owned()),
                local_availability: "available".into(),
                asset_id: Some(asset_id.clone()),
                provenance: json!({"fixture":"burrata-queued-autotag-v1"}),
                ..Track::default()
            };
            store
                .save_library_tracks(std::slice::from_ref(&track))
                .map_err(io::Error::other)?;
            track = store
                .get_track(&track.id)
                .map_err(io::Error::other)?
                .ok_or("queued AutoTag fixture track disappeared after save")?;
            let asset = LocalAsset {
                id: asset_id.clone(),
                recording_id: store.recording_id(&track.id).map_err(io::Error::other)?,
                path: canonical.to_string_lossy().into_owned(),
                hash: sha256.clone(),
                source_codec: "wav".into(),
                final_codec: "wav".into(),
                source_bitrate: Some((bytes * 8) as f64 / DURATION_SECONDS as f64 / 1000.0),
                final_bitrate: Some((bytes * 8) as f64 / DURATION_SECONDS as f64 / 1000.0),
                sample_rate: Some(SAMPLE_RATE),
                channels: Some(CHANNELS as u32),
                bit_depth: Some(16),
                duration_seconds: Some(DURATION_SECONDS as f64),
                size_bytes: Some(bytes),
                bitrate_origin: Some("file-size-estimate".into()),
                processing: "synthetic qualification fixture".into(),
                track,
            };
            store.save_asset(&asset).map_err(io::Error::other)?;
            let job = AutoTagJob {
                asset_id: asset.id.clone(),
                input_hash: asset.hash.clone(),
                version: autotag::VERSION.into(),
                status: "queued".into(),
                attempts: 0,
                next_attempt_at: None,
                message: None,
                candidates: vec![],
                recognition: None,
                chosen: None,
                approved_fields: vec![],
                excerpt_only: false,
                updated_at: chrono::Utc::now().to_rfc3339(),
            };
            store
                .save_autotag_record(
                    &job.asset_id,
                    &job.input_hash,
                    &job.version,
                    &job.status,
                    &serde_json::to_value(&job)?,
                )
                .map_err(io::Error::other)?;
            store
                .set_json_preference(
                    "autotag.settings",
                    &json!({"enabled":true,"shazamFallback":true}),
                )
                .map_err(io::Error::other)?;
            let policy = restore_work_policy::pause_all_after_restore(
                &store,
                "synthetic-queued-autotag-fixture",
            )
            .map_err(io::Error::other)?;
            let autotag_category = policy
                .categories
                .iter()
                .find(|category| category.kind == AutomaticWorkKind::AssetAutotag)
                .ok_or("AutoTag restore review category is missing")?;
            if !policy.review_required
                || !autotag_category.paused
                || autotag_category.pending_count != 1
            {
                return Err(
                    "queued AutoTag fixture was not protected by the startup review pause".into(),
                );
            }
            Some(json!({
                "trackTitle":asset.track.title,
                "assetId":asset.id,
                "audioSha256":asset.hash,
                "audioTagsAbsent":true,
                "bpm":null,
                "key":null,
                "queuedStatus":job.status,
                "attempts":job.attempts,
                "restoreReviewRequired":policy.review_required,
                "assetAutotagPaused":autotag_category.paused,
                "pendingAssetAutotagCount":autotag_category.pending_count
            }))
        } else {
            None
        };

    let inventory = json!({
        "format":"org.burrata.qualification-profile",
        "schemaVersion":1,
        "profileRoot":profile_root,
        "appDataDirectory":data_dir,
        "backupDestination":backup_destination,
        "launchEnvironment":{"BURRATA_APP_DATA":data_dir},
        "syntheticOnly":true,
        "externalAudioUsed":false,
        "libraryUserVersion":sqlite_user_version(&data_dir.join("burrata.sqlite"))?,
        "tracks":tracks.iter().enumerate().map(|(index,track)| json!({
            "id":track.id,
            "title":track.title,
            "assetId":track.asset_id,
            "sha256":audio[index].2,
            "bytes":audio[index].1,
            "durationSeconds":DURATION_SECONDS,
            "audioPath":audio[index].0,
            "lyricsPath":audio[index].3,
            "expectedTempoBpm":120.0,
            "expectedKey":if index == 0 {"8B"} else {"8A"},
            "metadataCase":if index == 0 {"imported"} else {"manual override"},
            "storedBpm":track.bpm,
            "storedKey":track.key
        })).collect::<Vec<_>>(),
        "playlist":{
            "id":collection.id,
            "name":collection.name,
            "nestedFolders":["Qualification Crates","Two Track Set"],
            "sections":[warmup.name,close.name],
            "occurrenceCount":ordered.len(),
            "duplicateOccurrenceCount":ordered.len()-ordered.iter().map(|row| &row.listing_id).collect::<std::collections::HashSet<_>>().len()
        },
        "favoriteTrackIds":[tracks[0].id],
        "historyEvents":2,
        "queue":{"entries":2,"status":"paused","currentIndex":1,"positionSeconds":4.25,"volume":0.0,"queueRevision":queue_revision},
        "assets":assets.iter().map(|asset| json!({
            "assetId":asset.id,
            "sha256":asset.hash,
            "bytes":asset.size_bytes,
            "codec":"wav",
            "lrcSibling":Path::new(&asset.path).with_extension("lrc")
        })).collect::<Vec<_>>(),
        "settings":{"theme":"dark","autoPreview":false,"liveBpmEnabled":true,"credentialsIncluded":false},
        "queuedAutoTagFixture":queued_autotag_fixture,
        "audioFixtureGainsDb":[0.0,second_gain_db],
        "automationPermissions":{"read":true,"control":true,"manage":false}
    });
    let inventory_path = profile_root.join("qualification-inventory.json");
    let mut inventory_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&inventory_path)?;
    serde_json::to_writer_pretty(&mut inventory_file, &inventory)?;
    inventory_file.write_all(b"\n")?;
    inventory_file.sync_all()?;
    println!("{}", serde_json::to_string_pretty(&inventory)?);
    Ok(())
}

fn validate_destination(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err("qualification profile path must be an absolute new directory".into());
    }
    match fs::symlink_metadata(path) {
        Ok(_) => return Err("qualification profile destination already exists".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path
        .parent()
        .ok_or("qualification profile must have an existing parent directory")?;
    let canonical_parent = fs::canonicalize(parent)?;
    let temp_root = fs::canonicalize(env::temp_dir())?;
    let repo_cache = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("cannot resolve repository root")?
        .join(".cache");
    let canonical_cache = fs::canonicalize(repo_cache).ok();
    if !canonical_parent.starts_with(&temp_root)
        && !canonical_cache
            .as_ref()
            .is_some_and(|cache| canonical_parent.starts_with(cache))
    {
        return Err("qualification profile must be beneath the system temporary directory or repository .cache".into());
    }
    Ok(canonical_parent.join(path.file_name().ok_or("invalid profile name")?))
}

fn write_synthetic_wav(path: &Path, chord_hz: [f64; 3], gain: f64) -> Result<(), Box<dyn Error>> {
    let frame_count = SAMPLE_RATE as usize * DURATION_SECONDS as usize;
    let data_bytes = frame_count
        .checked_mul(CHANNELS as usize)
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or("WAV data length overflow")?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&(36u32 + data_bytes as u32).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&CHANNELS.to_le_bytes())?;
    file.write_all(&SAMPLE_RATE.to_le_bytes())?;
    let byte_rate = SAMPLE_RATE * CHANNELS as u32 * 2;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&(CHANNELS * 2).to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&(data_bytes as u32).to_le_bytes())?;
    for frame in 0..frame_count {
        let time = frame as f64 / SAMPLE_RATE as f64;
        let chord = chord_hz
            .iter()
            .map(|frequency| (std::f64::consts::TAU * frequency * time).sin())
            .sum::<f64>()
            * 0.18;
        let beat_phase = time % 0.5;
        let click = if beat_phase < 0.012 {
            (std::f64::consts::TAU * 1_200.0 * beat_phase).sin()
                * (-beat_phase * 260.0).exp()
                * 0.55
        } else {
            0.0
        };
        let sample = ((chord + click).clamp(-0.95, 0.95) * gain * i16::MAX as f64) as i16;
        for _ in 0..CHANNELS {
            file.write_all(&sample.to_le_bytes())?;
        }
    }
    file.sync_all()?;
    Ok(())
}

fn hash_file(path: &Path) -> Result<(u64, String), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    Ok((bytes.len() as u64, format!("{:x}", Sha256::digest(&bytes))))
}

fn sqlite_user_version(path: &Path) -> Result<i64, Box<dyn Error>> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(connection.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}
