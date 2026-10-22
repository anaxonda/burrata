//! Run the production read-only BPM/key analyzers over a local validation manifest.
//!
//! Usage: cargo run --example analyze_audio_corpus --release -- <manifest.json> [output.json]
//! No library/store APIs are used; the example reads audio, hashes it, and writes JSON only.

use burrata_lib::{media, models::Settings};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::PathBuf, time::Instant};

#[derive(Debug, Deserialize)]
struct Manifest {
    dataset: String,
    version: String,
    items: Vec<Item>,
}

#[derive(Debug, Deserialize)]
struct Item {
    id: String,
    title: String,
    source_url: String,
    license: String,
    expected_bpm: f64,
    expected_key: String,
    audio_path: PathBuf,
}

#[derive(Debug, Serialize)]
struct Report {
    dataset: String,
    version: String,
    ffmpeg_path: String,
    items: Vec<ItemReport>,
}

#[derive(Debug, Serialize)]
struct ItemReport {
    id: String,
    title: String,
    source_url: String,
    license: String,
    expected_bpm: f64,
    expected_key: String,
    audio_path: PathBuf,
    sha256: Option<String>,
    elapsed_ms: u128,
    bpm: Option<BpmReport>,
    key: Option<KeyReport>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct BpmReport {
    bpm: f64,
    confidence: f64,
    related_tempo_bpm: Option<f64>,
    absolute_error_bpm: f64,
    within_one_bpm: bool,
    octave_equivalent_within_one_bpm: bool,
}

#[derive(Debug, Serialize)]
struct KeyReport {
    key: String,
    confidence: f64,
    whole_track_key: String,
    window_keys: Vec<Option<String>>,
    pitch_class_and_mode_match: bool,
}

fn key_identity(key: &str) -> Option<(u8, bool)> {
    let normalized = key
        .to_ascii_lowercase()
        .replace("sharp", "#")
        .replace("flat", "b")
        .replace('♯', "#")
        .replace('♭', "b")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    let (root, minor) = if let Some(root) = normalized
        .strip_suffix("minor")
        .or_else(|| normalized.strip_suffix("min"))
        .or_else(|| normalized.strip_suffix('m'))
    {
        (root, true)
    } else {
        (
            normalized
                .strip_suffix("major")
                .or_else(|| normalized.strip_suffix("maj"))
                .unwrap_or(&normalized),
            false,
        )
    };
    let mut chars = root.chars();
    let pitch: i16 = match chars.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let accidental = match chars.next() {
        None => 0,
        Some('#') => 1,
        Some('b') => -1,
        _ => return None,
    };
    if chars.next().is_some() {
        return None;
    }
    Some(((pitch + accidental).rem_euclid(12) as u8, minor))
}

#[cfg(test)]
mod key_label_tests {
    use super::key_identity;
    #[test]
    fn compares_pitch_class_and_mode_across_analyzer_and_annotation_labels() {
        assert_eq!(key_identity("Cm"), key_identity("C minor"));
        assert_eq!(key_identity("F#"), key_identity("G flat major"));
        assert_eq!(key_identity("A♭ minor"), key_identity("G#m"));
        assert_ne!(key_identity("C major"), key_identity("C minor"));
        assert_eq!(key_identity("unknown"), None);
        assert_eq!(key_identity("C nonsense"), None);
    }
}

fn sha256(path: &PathBuf) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("cannot open audio: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|e| format!("cannot hash audio: {e}"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let manifest_path = PathBuf::from(args.next().ok_or("pass manifest.json [output.json]")?);
    let output_path = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let manifest: Manifest = serde_json::from_reader(File::open(&manifest_path)?)?;
    let manifest_dir = manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut settings = Settings::default();
    if let Some(ffmpeg) = std::env::var_os("FFMPEG_PATH") {
        settings.ffmpeg_path = PathBuf::from(ffmpeg).to_string_lossy().into_owned();
    }

    let mut reports = Vec::with_capacity(manifest.items.len());
    for mut item in manifest.items {
        if item.audio_path.is_relative() {
            item.audio_path = manifest_dir.join(&item.audio_path);
        }
        let start = Instant::now();
        let hash = sha256(&item.audio_path);
        let mut report = ItemReport {
            id: item.id,
            title: item.title,
            source_url: item.source_url,
            license: item.license,
            expected_bpm: item.expected_bpm,
            expected_key: item.expected_key,
            audio_path: item.audio_path.clone(),
            sha256: hash.as_ref().ok().cloned(),
            elapsed_ms: 0,
            bpm: None,
            key: None,
            error: hash.as_ref().err().cloned(),
        };
        if report.error.is_none() {
            match media::detect_bpm(&item.audio_path, &settings).await {
                Ok(Some(value)) => {
                    let error = (value.bpm - report.expected_bpm).abs();
                    let octave_match = [value.bpm, value.bpm * 2.0, value.bpm / 2.0]
                        .iter()
                        .any(|candidate| (*candidate - report.expected_bpm).abs() <= 1.0);
                    report.bpm = Some(BpmReport {
                        bpm: value.bpm,
                        confidence: value.confidence,
                        related_tempo_bpm: value.related_tempo_bpm,
                        absolute_error_bpm: error,
                        within_one_bpm: error <= 1.0,
                        octave_equivalent_within_one_bpm: octave_match,
                    });
                }
                Ok(None) => report.error = Some("BPM analyzer returned no estimate".into()),
                Err(error) => report.error = Some(format!("BPM analysis: {error}")),
            }
            match media::detect_key(&item.audio_path, &settings).await {
                Ok(Some(value)) => {
                    let pitch_class_and_mode_match = key_identity(&value.key)
                        .zip(key_identity(&report.expected_key))
                        .is_some_and(|(actual, expected)| actual == expected);
                    report.key = Some(KeyReport {
                        key: value.key,
                        confidence: value.confidence,
                        whole_track_key: value.whole_track_key,
                        window_keys: value.window_keys,
                        pitch_class_and_mode_match,
                    });
                }
                Ok(None) => {
                    report
                        .error
                        .get_or_insert_with(|| "key analyzer returned no estimate".into());
                }
                Err(error) => {
                    report
                        .error
                        .get_or_insert_with(|| format!("key analysis: {error}"));
                }
            }
        }
        report.elapsed_ms = start.elapsed().as_millis();
        reports.push(report);
    }

    let report = Report {
        dataset: manifest.dataset,
        version: manifest.version,
        ffmpeg_path: settings.ffmpeg_path,
        items: reports,
    };
    let json = serde_json::to_vec_pretty(&report)?;
    if let Some(path) = output_path {
        std::fs::write(path, json)?;
    } else {
        println!("{}", String::from_utf8(json)?);
    }
    Ok(())
}
