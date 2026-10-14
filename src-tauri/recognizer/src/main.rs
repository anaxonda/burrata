#![forbid(unsafe_code)]

mod fingerprinting {
    #[path = "../../vendor/songrec/algorithm.rs"]
    pub mod algorithm;
    #[path = "../../vendor/songrec/hanning.rs"]
    pub mod hanning;
    #[path = "../../vendor/songrec/signature_format.rs"]
    pub mod signature_format;
}

use fingerprinting::algorithm::SignatureGenerator;
use hound::{SampleFormat, WavReader};
use serde_json::{json, Value};
use std::{
    env,
    error::Error,
    fs::File,
    io::BufReader,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

fn wav_mono_16k(path: &Path) -> Result<Vec<f32>, Box<dyn Error>> {
    let reader = WavReader::new(BufReader::new(File::open(path)?))?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate < 8_000 || spec.sample_rate > 192_000 {
        return Err(format!(
            "unsupported WAV format: {} Hz, {} channels",
            spec.sample_rate, spec.channels
        )
        .into());
    }
    let interleaved = match spec.sample_format {
        SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| s.map_err(Into::into))
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?,
        SampleFormat::Int if spec.bits_per_sample == 16 => reader
            .into_samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32768.0).map_err(Into::into))
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?,
        _ => return Err("only 16-bit integer or float PCM WAV is supported".into()),
    };
    let channels = spec.channels as usize;
    let mono: Vec<f32> = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect();
    if mono.len() < 128 {
        return Err("audio excerpt is too short".into());
    }
    if spec.sample_rate == 16_000 {
        return Ok(mono);
    }
    // The upstream SongRec file path uses the same simple sample-rate conversion.
    // Linear interpolation is adequate after FFmpeg's normal audio resampling; it
    // also keeps this helper independent of native audio libraries.
    let output_len = mono.len() * 16_000 / spec.sample_rate as usize;
    let mut resampled = Vec::with_capacity(output_len);
    for out_i in 0..output_len {
        let source_pos = out_i as f64 * spec.sample_rate as f64 / 16_000.0;
        let left = source_pos.floor() as usize;
        let right = (left + 1).min(mono.len() - 1);
        let frac = (source_pos - left as f64) as f32;
        resampled.push(mono[left] + (mono[right] - mono[left]) * frac);
    }
    Ok(resampled)
}

async fn run(path: &Path) -> Result<Value, Box<dyn Error>> {
    let samples = wav_mono_16k(path)?;
    let signature_samples: Vec<f32> = if samples.len() < 12 * 16_000 {
        let mut padded = samples;
        padded.resize(12 * 16_000, 0.0);
        padded
    } else if samples.len() > 12 * 16_000 {
        let middle = samples.len() / 2;
        samples[middle - 6 * 16_000..middle + 6 * 16_000].to_vec()
    } else {
        samples
    };
    let signature = SignatureGenerator::make_signature_from_buffer(&signature_samples);
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u32;
    let payload = json!({
        "geolocation": {"altitude":300,"latitude":45,"longitude":2},
        "signature": {
            "samplems": (signature.number_samples as f32 / signature.sample_rate_hz as f32 * 1000.0) as u32,
            "timestamp": timestamp,
            "uri": signature.encode_to_uri()?
        },
        "timestamp": timestamp,
        "timezone": "Europe/Paris"
    });
    let url = format!("https://amp.shazam.com/discovery/v5/en/US/android/-/tag/{}/{}?sync=true&webv3=true&sampling=true&connected=&shazamapiversion=v3&sharehub=true&video=v3", Uuid::new_v4().hyphenated().to_string().to_uppercase(), Uuid::new_v4());
    let response = reqwest::Client::builder()
        .user_agent("SongRec/0.7.5 (Burrata file recognition)")
        .build()?
        .post(url)
        .header("Content-Language", "en_US")
        .json(&payload)
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err("Shazam rate limited the recognition request".into());
    }
    let response = response.error_for_status()?.json::<Value>().await?;
    Ok(response)
}

#[tokio::main]
async fn main() {
    if let Err(err) = entry().await {
        eprintln!("burrata-recognize: {err}");
        std::process::exit(2);
    }
}

fn audio_path(args: &[std::ffi::OsString]) -> Result<&Path, Box<dyn Error>> {
    if args.len() == 3 && args[0] == "recognize" && args[1] == "--json" {
        return Ok(Path::new(&args[2]));
    }
    if args.len() == 2 && args[0] == "audio-file-to-recognized-song" {
        return Ok(Path::new(&args[1]));
    }
    Err(
        "usage: burrata-recognize recognize --json PATH (or audio-file-to-recognized-song PATH)"
            .into(),
    )
}

async fn entry() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let path = audio_path(&args)?;
    let response = run(path).await?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fingerprinting::signature_format::DecodedSignature;

    #[test]
    fn golden_tone_fingerprint_has_stable_signature_roundtrip() {
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let samples: Vec<f32> = (0..16_000 * 12)
                    .map(|i| {
                        let t = i as f32 / 16_000.0;
                        0.6 * (std::f32::consts::TAU * 440.0 * t).sin()
                            + 0.3 * (std::f32::consts::TAU * 880.0 * t).sin()
                    })
                    .collect();
                let signature = SignatureGenerator::make_signature_from_buffer(&samples);
                assert_eq!(signature.sample_rate_hz, 16000);
                assert_eq!(signature.number_samples, 192000);
                let peaks: Vec<_> = signature
                    .frequency_band_to_sound_peaks
                    .iter()
                    .flat_map(|band| band.iter())
                    .take(6)
                    .map(|p| {
                        (
                            p.fft_pass_number,
                            p.peak_magnitude,
                            p.corrected_peak_frequency_bin,
                        )
                    })
                    .collect();
                assert_eq!(
                    peaks,
                    vec![
                        (62, 36182, 3605),
                        (152, 36182, 3605),
                        (281, 36182, 3605),
                        (381, 36182, 3605),
                        (477, 36182, 3605),
                        (546, 11671, 2230)
                    ]
                );
                let decoded =
                    DecodedSignature::decode_from_uri(&signature.encode_to_uri().unwrap()).unwrap();
                assert_eq!(decoded.number_samples, signature.number_samples);
                assert_eq!(
                    decoded
                        .frequency_band_to_sound_peaks
                        .iter()
                        .map(Vec::len)
                        .sum::<usize>(),
                    signature
                        .frequency_band_to_sound_peaks
                        .iter()
                        .map(Vec::len)
                        .sum::<usize>()
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn supports_both_songrec_cli_forms() {
        let radio = vec!["recognize".into(), "--json".into(), "radio.wav".into()];
        assert_eq!(audio_path(&radio).unwrap(), Path::new("radio.wav"));
        let autotag = vec!["audio-file-to-recognized-song".into(), "excerpt.wav".into()];
        assert_eq!(audio_path(&autotag).unwrap(), Path::new("excerpt.wav"));
    }

    #[test]
    fn parse_no_match_shazam_response_as_valid_json() {
        let path = std::env::temp_dir().join(format!(
            "burrata-recognizer-no-match-{}.wav",
            std::process::id()
        ));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..16_000 * 12 {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
        let samples = wav_mono_16k(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let signature = SignatureGenerator::make_signature_from_buffer(&samples);
        assert_eq!(
            signature
                .frequency_band_to_sound_peaks
                .iter()
                .map(Vec::len)
                .sum::<usize>(),
            0
        );
        let api_body = r#"{"matches":[],"tagid":"test"}"#;
        let parsed: Value = serde_json::from_str(api_body).unwrap();
        assert!(parsed.get("track").is_none());
        assert_eq!(parsed["matches"].as_array().unwrap().len(), 0);
    }
}
