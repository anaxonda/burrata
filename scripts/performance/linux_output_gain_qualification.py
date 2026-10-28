#!/usr/bin/env python3
"""Record two native playback cases through a private PulseAudio null sink.

The controller is expected to launch/control the immutable candidate, select
the requested mode from BURRATA_QUAL_CASE, and play the same qualification
fixture to natural end. This harness never changes the default sink/volume.
"""

from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time
import uuid
import wave


RATE = 48_000
CHANNELS = 2
SAMPLE_WIDTH = 2
WINDOW_SECONDS = 0.01
DEFAULT_EXPECTED_GAIN_DB = -6.0


def run(command: list[str], *, check: bool = True, capture: bool = True) -> str:
    result = subprocess.run(
        command,
        check=False,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
        text=True,
    )
    if check and result.returncode != 0:
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(command)}\n"
            f"{result.stderr or ''}"
        )
    return (result.stdout or "").strip()


def ensure_audio_server() -> None:
    for tool in ("pactl", "parec"):
        if shutil.which(tool) is None:
            raise RuntimeError(f"required executable is missing: {tool}")
    run(["pactl", "info"])


def wait_for_raw_frames(path: Path, minimum_frames: int, timeout_seconds: float = 5.0) -> int:
    deadline = time.monotonic() + timeout_seconds
    minimum_bytes = minimum_frames * CHANNELS * SAMPLE_WIDTH
    while time.monotonic() < deadline:
        if path.exists():
            size = path.stat().st_size
            if size >= minimum_bytes:
                return size // (CHANNELS * SAMPLE_WIDTH)
        time.sleep(0.01)
    size = path.stat().st_size if path.exists() else 0
    raise RuntimeError(
        f"monitor capture ended short: got {size // (CHANNELS * SAMPLE_WIDTH)} frames, "
        f"needed {minimum_frames}"
    )


def raw_s16le_to_wav(raw_path: Path, output_path: Path) -> None:
    with raw_path.open("rb") as source, wave.open(str(output_path), "wb") as wav:
        wav.setnchannels(CHANNELS)
        wav.setsampwidth(SAMPLE_WIDTH)
        wav.setframerate(RATE)
        pending = b""
        while chunk := source.read(1024 * 1024):
            data = pending + chunk
            aligned_size = len(data) - (len(data) % (CHANNELS * SAMPLE_WIDTH))
            if aligned_size:
                wav.writeframesraw(data[:aligned_size])
            pending = data[aligned_size:]
        if pending:
            raise ValueError("PulseAudio recorder ended on a partial stereo frame")
        wav.writeframes(b"")


def create_private_sink(name: str) -> str:
    module_id = run(
        [
            "pactl",
            "load-module",
            "module-null-sink",
            f"sink_name={name}",
            f"rate={RATE}",
            f"channels={CHANNELS}",
            "channel_map=front-left,front-right",
            f"sink_properties=device.description={name}",
        ]
    )
    if not module_id.isdecimal():
        raise RuntimeError(f"PulseAudio returned an invalid module id: {module_id!r}")
    sinks = run(["pactl", "list", "short", "sinks"])
    if not any(line.split("\t")[1:2] == [name] for line in sinks.splitlines()):
        run(["pactl", "unload-module", module_id], check=False)
        raise RuntimeError("private null sink was not created")
    return module_id


def record_case(
    *, case: str, sink: str, module_id: str, output: Path, controller: list[str], timeout: float,
    expected_gain_db: float, user_volume: float,
) -> dict:
    monitor = f"{sink}.monitor"
    output.parent.mkdir(parents=True, exist_ok=True)
    raw_path = output.with_suffix(".s16le")
    recording_started = time.monotonic()
    with raw_path.open("wb") as capture_file:
        recorder = subprocess.Popen(
            [
                "parec",
                f"--device={monitor}",
                "--raw",
                "--format=s16le",
                f"--rate={RATE}",
                f"--channels={CHANNELS}",
                "--latency-msec=20",
                "--process-time-msec=10",
            ],
            stdout=capture_file,
            stderr=subprocess.PIPE,
        )
        try:
            time.sleep(0.35)
            env = os.environ.copy()
            env["PULSE_SINK"] = sink
            env["BURRATA_QUAL_SINK"] = sink
            env["BURRATA_QUAL_MODULE_ID"] = module_id
            env["BURRATA_QUAL_CASE"] = case
            env["BURRATA_QUAL_EXPECTED_GAIN_DB"] = str(expected_gain_db)
            env["BURRATA_QUAL_CAPTURE"] = str(output)
            env["BURRATA_QUAL_EXPECTED_RATE"] = str(RATE)
            env["BURRATA_QUAL_USER_VOLUME"] = str(user_volume)
            result = subprocess.run(controller, env=env, timeout=timeout, check=False)
            controller_elapsed = time.monotonic() - recording_started
            if result.returncode:
                raise RuntimeError(f"{case} controller exited with {result.returncode}")
            captured_before_stop_frames = wait_for_raw_frames(
                raw_path, round((controller_elapsed + 0.15) * RATE)
            )
        finally:
            if recorder.poll() is None:
                recorder.send_signal(signal.SIGINT)
                try:
                    recorder.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    recorder.kill()
                    recorder.wait(timeout=3)
    raw_s16le_to_wav(raw_path, output)
    raw_path.unlink(missing_ok=True)
    result = analyze_wav(output)
    result["capturedBeforeStopFrames"] = captured_before_stop_frames
    result["controllerElapsedSeconds"] = controller_elapsed
    return result


def decode_pcm(data: bytes) -> list[tuple[float, float]]:
    if len(data) % 4:
        raise ValueError("stereo signed-16 PCM has a partial frame")
    samples: list[tuple[float, float]] = []
    for offset in range(0, len(data), 4):
        left = int.from_bytes(data[offset : offset + 2], "little", signed=True) / 32768.0
        right = int.from_bytes(data[offset + 2 : offset + 4], "little", signed=True) / 32768.0
        samples.append((left, right))
    return samples


def analyze_wav(path: Path) -> dict:
    with wave.open(str(path), "rb") as wav:
        channels = wav.getnchannels()
        rate = wav.getframerate()
        width = wav.getsampwidth()
        frames = wav.getnframes()
        pcm = wav.readframes(frames)
    if channels != CHANNELS or rate != RATE or width != SAMPLE_WIDTH:
        raise ValueError(
            f"unexpected capture format: {channels}ch {rate}Hz {width * 8}bit"
        )
    samples = decode_pcm(pcm)
    frames_per_window = max(1, round(RATE * WINDOW_SECONDS))
    windows = []
    for begin in range(0, len(samples), frames_per_window):
        batch = samples[begin : begin + frames_per_window]
        if not batch:
            continue
        left_peak = max(abs(sample[0]) for sample in batch)
        right_peak = max(abs(sample[1]) for sample in batch)
        left_rms = math.sqrt(sum(sample[0] ** 2 for sample in batch) / len(batch))
        right_rms = math.sqrt(sum(sample[1] ** 2 for sample in batch) / len(batch))
        windows.append(
            {
                "startSeconds": begin / RATE,
                "leftPeak": left_peak,
                "rightPeak": right_peak,
                "leftRms": left_rms,
                "rightRms": right_rms,
            }
        )
    audible = [i for i, window in enumerate(windows) if max(window["leftPeak"], window["rightPeak"]) > 1e-4]
    if not audible:
        raise ValueError("capture contains no audible PCM")
    first = audible[0]
    first_audible_frame = next(
        index
        for index, (left, right) in enumerate(samples)
        if max(abs(left), abs(right)) > 1e-4
    )
    return {
        "path": str(path),
        "sampleRateHz": rate,
        "channels": channels,
        "frames": frames,
        "durationSeconds": frames / rate,
        "firstAudibleSeconds": windows[first]["startSeconds"],
        "firstAudibleWindow": windows[first],
        "firstAudibleFrameIndex": first_audible_frame,
        "firstAudibleFrame": {
            "left": samples[first_audible_frame][0],
            "right": samples[first_audible_frame][1],
        },
        "windows": windows,
        "peakPerChannel": {
            "left": max(window["leftPeak"] for window in windows),
            "right": max(window["rightPeak"] for window in windows),
        },
        "clippedFrameCount": sum(
            1 for left, right in samples if abs(left) >= 0.99997 or abs(right) >= 0.99997
        ),
    }


def compare(off: dict, normalized: dict, expected_gain_db: float) -> dict:
    # Playback streams can begin at different monitor frame offsets. Align the
    # actual PCM using a bounded normalized cross-correlation around onset.
    off_samples = read_wav_samples(Path(off["path"]))
    down_samples = read_wav_samples(Path(normalized["path"]))
    off_start = first_audible_sample(off_samples)
    down_start = first_audible_sample(down_samples)
    alignment = align_capture_samples(off_samples, down_samples, off_start, down_start)
    down_start += alignment
    ratios: list[dict] = []
    frames_per_window = max(1, round(RATE * WINDOW_SECONDS))
    for i, start in enumerate(range(0, min(len(off_samples) - off_start, len(down_samples) - down_start), frames_per_window)):
        end = min(start + frames_per_window, len(off_samples) - off_start, len(down_samples) - down_start)
        base = off_samples[off_start + start : off_start + end]
        attenuated = down_samples[down_start + start : down_start + end]
        if not base or not attenuated:
            continue
        entry: dict = {"relativeWindow": i, "startSeconds": start / RATE}
        for channel in ("left", "right"):
            channel_index = 0 if channel == "left" else 1
            base_rms = math.sqrt(sum(frame[channel_index] ** 2 for frame in base) / len(base))
            down_rms = math.sqrt(sum(frame[channel_index] ** 2 for frame in attenuated) / len(attenuated))
            if base_rms > 1e-4 and down_rms > 1e-4:
                entry[f"{channel}DbDelta"] = 20 * math.log10(down_rms / base_rms)
        if "leftDbDelta" in entry or "rightDbDelta" in entry:
            ratios.append(entry)
    if not ratios:
        raise ValueError("no aligned non-silent windows could be compared")
    first_100ms = [row for row in ratios if row["relativeWindow"] < 10]
    steady = [row for row in ratios if 10 <= row["relativeWindow"] < 60]
    all_mean = mean_delta(ratios)
    first_off = off_samples[off_start]
    first_down = down_samples[down_start]
    first_sample_delta = {}
    for channel, index in (("left", 0), ("right", 1)):
        if abs(first_off[index]) > 1e-4 and abs(first_down[index]) > 1e-4:
            first_sample_delta[channel] = 20 * math.log10(abs(first_down[index]) / abs(first_off[index]))
    # The generated album contains two exact 20-second WAV members and the
    # native queue is gapless. Segment each member to verify equal Album gain
    # and unchanged relative levels across the programme boundary.
    per_track = []
    track_rms = {"off": [], "normalized": []}
    for track_index, begin in enumerate((0, 20 * RATE), start=1):
        end = min(begin + 20 * RATE, len(off_samples) - off_start, len(down_samples) - down_start)
        segment_rows = []
        for relative_begin in range(begin, end, frames_per_window):
            relative_end = min(relative_begin + frames_per_window, end)
            base = off_samples[off_start + relative_begin : off_start + relative_end]
            attenuated = down_samples[down_start + relative_begin : down_start + relative_end]
            if not base or not attenuated:
                continue
            row = {"startSeconds": relative_begin / RATE}
            for channel, index in (("left", 0), ("right", 1)):
                base_rms = math.sqrt(sum(frame[index] ** 2 for frame in base) / len(base))
                down_rms = math.sqrt(sum(frame[index] ** 2 for frame in attenuated) / len(attenuated))
                if base_rms > 1e-4 and down_rms > 1e-4:
                    row[f"{channel}DbDelta"] = 20 * math.log10(down_rms / base_rms)
            if "leftDbDelta" in row or "rightDbDelta" in row:
                segment_rows.append(row)
        per_track.append({"trackIndex": track_index, "startSeconds": begin / RATE,
                          "meanGainDeltaDb": mean_delta(segment_rows),
                          "comparedWindows": len(segment_rows)})
        for label, samples, offset in (("off", off_samples, off_start),
                                       ("normalized", down_samples, down_start)):
            segment = samples[offset + begin : offset + end]
            rms = math.sqrt(sum((frame[0] ** 2 + frame[1] ** 2) / 2 for frame in segment) / len(segment)) if segment else 0.0
            track_rms[label].append(20 * math.log10(rms) if rms > 1e-8 else None)
    inter_track_change = None
    if all(value is not None for values in track_rms.values() for value in values):
        off_relative = track_rms["off"][1] - track_rms["off"][0]
        normalized_relative = track_rms["normalized"][1] - track_rms["normalized"][0]
        inter_track_change = normalized_relative - off_relative
    return {
        "expectedDbDelta": expected_gain_db,
        "alignedSampleOffsetFrames": alignment,
        "firstAudibleSampleDbDelta": first_sample_delta,
        "comparedWindows": len(ratios),
        "first100ms": first_100ms,
        "first100msMeanDb": mean_delta(first_100ms),
        "steadyFirst500msMeanDb": mean_delta(steady),
        "allMeanDb": all_mean,
        "perTrackGainDelta": per_track,
        "offTrackMeanRmsDb": track_rms["off"],
        "normalizedTrackMeanRmsDb": track_rms["normalized"],
        "interTrackRelativeLevelChangeDb": inter_track_change,
        "channelsRemainIndependent": all(
            channel in all_mean for channel in ("left", "right")
        )
        and abs(all_mean["left"] - all_mean["right"]) <= 0.15,
    }


def read_wav_samples(path: Path) -> list[tuple[float, float]]:
    with wave.open(str(path), "rb") as wav:
        if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) != (CHANNELS, SAMPLE_WIDTH, RATE):
            raise ValueError(f"unexpected comparison format in {path}")
        return decode_pcm(wav.readframes(wav.getnframes()))


def first_audible_sample(samples: list[tuple[float, float]]) -> int:
    return next(
        (index for index, frame in enumerate(samples) if max(abs(frame[0]), abs(frame[1])) > 1e-4),
        0,
    )


def align_capture_samples(
    off: list[tuple[float, float]],
    down: list[tuple[float, float]],
    off_start: int,
    down_start: int,
) -> int:
    search = RATE // 10
    stride = 16
    span = RATE // 5

    def score(offset: int, sample_stride: int) -> float:
        start = down_start + offset
        if start < 0 or start + span >= len(down) or off_start + span >= len(off):
            return -1.0
        dot = left_energy = right_energy = 0.0
        for relative in range(0, span, sample_stride):
            for channel in (0, 1):
                first = off[off_start + relative][channel]
                second = down[start + relative][channel]
                dot += first * second
                left_energy += first * first
                right_energy += second * second
        denominator = math.sqrt(left_energy * right_energy)
        return dot / denominator if denominator > 0 else -1.0

    best_offset = 0
    coarse_scores: list[tuple[float, int]] = []
    for offset in range(-search, search + 1, stride):
        current = score(offset, stride)
        coarse_scores.append((current, offset))

    # Periodic material can make a stride-aligned coarse search prefer a
    # nearby phase alias over the true source offset. Refine several strong
    # coarse candidates, then select by the same finer correlation score.
    coarse_scores.sort(reverse=True)
    best_score = -1.0
    for _, coarse in coarse_scores[:8]:
        for offset in range(max(-search, coarse - stride), min(search, coarse + stride) + 1):
            current = score(offset, 4)
            if current > best_score:
                best_offset, best_score = offset, current
    if best_score < 0.5:
        raise ValueError(f"could not align output captures (correlation {best_score:.3f})")
    return best_offset


def mean_delta(rows: list[dict]) -> dict | None:
    if not rows:
        return None
    output = {}
    for channel in ("left", "right"):
        values = [row[f"{channel}DbDelta"] for row in rows if f"{channel}DbDelta" in row]
        if values:
            output[channel] = sum(values) / len(values)
    return output


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--output-dir", type=Path, required=True)
    result.add_argument("--controller", nargs=argparse.REMAINDER, required=True)
    result.add_argument("--timeout-seconds", type=float, default=45.0)
    result.add_argument("--expected-gain-db", type=float, default=DEFAULT_EXPECTED_GAIN_DB,
                        help="expected normalized-minus-off gain reported by the product")
    result.add_argument("--user-volume", type=float, default=0.36,
                        help="product volume used identically in Off and Album cases")
    return result


def main() -> int:
    args = parser().parse_args()
    if not args.controller:
        raise SystemExit("provide a controller command after --controller")
    if not math.isfinite(args.expected_gain_db) or not -60 <= args.expected_gain_db <= 18:
        raise SystemExit("expected gain must be finite and between -60 and +18 dB")
    if not math.isfinite(args.user_volume) or not 0 <= args.user_volume <= 1:
        raise SystemExit("user volume must be between zero and one")
    ensure_audio_server()
    sink = f"burrata_qual_{os.getpid()}_{uuid.uuid4().hex[:6]}"
    module_id = create_private_sink(sink)
    try:
        off_path = args.output_dir / "off.wav"
        down_path = args.output_dir / "normalized.wav"
        off = record_case(
            case="off", sink=sink, output=off_path,
            module_id=module_id,
            controller=args.controller, timeout=args.timeout_seconds,
            expected_gain_db=args.expected_gain_db,
            user_volume=args.user_volume,
        )
        normalized = record_case(
            case="normalized", sink=sink, output=down_path,
            module_id=module_id,
            controller=args.controller, timeout=args.timeout_seconds,
            expected_gain_db=args.expected_gain_db,
            user_volume=args.user_volume,
        )
        report = {
            "scope": "private PulseAudio null-sink capture of candidate-controlled playback",
            "sink": sink,
            "sampleRateHz": RATE,
            "appVolume": args.user_volume,
            "normalDefaultSinkChanged": False,
            "userSinkVolumeChanged": False,
            "off": off,
            "normalized": normalized,
            "comparison": compare(off, normalized, args.expected_gain_db),
        }
        args.output_dir.mkdir(parents=True, exist_ok=True)
        report_path = args.output_dir / "output-gain-report.json"
        report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(report_path)
        print(json.dumps(report["comparison"], indent=2))
    finally:
        run(["pactl", "unload-module", module_id], check=False)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"qualification harness failed: {error}", file=sys.stderr)
        raise SystemExit(2)
