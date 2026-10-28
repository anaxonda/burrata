#!/usr/bin/env python3
"""Summarize source-aligned early WebView audio gain from a private-sink report."""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

from linux_output_gain_qualification import (
    RATE,
    align_capture_samples,
    first_audible_sample,
    read_wav_samples,
)


def normalized_correlation(off, normalized, off_start: int, normalized_start: int) -> float:
    frame_count = min(RATE // 5, len(off) - off_start, len(normalized) - normalized_start)
    if frame_count <= 0:
        return 0.0
    dot = off_energy = normalized_energy = 0.0
    for frame in range(0, frame_count, 16):
        for channel in (0, 1):
            first = off[off_start + frame][channel]
            second = normalized[normalized_start + frame][channel]
            dot += first * second
            off_energy += first * first
            normalized_energy += second * second
    denominator = math.sqrt(off_energy * normalized_energy)
    return dot / denominator if denominator > 0 else 0.0


def rms(frames, channel: int) -> float:
    return math.sqrt(sum(frame[channel] ** 2 for frame in frames) / len(frames)) if frames else 0.0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path, help="output-gain-report.json emitted by linux_output_gain_qualification.py")
    parser.add_argument("--expected-gain-db", type=float, help="override report comparison expectedDbDelta")
    parser.add_argument("--tolerance-db", type=float, default=0.25)
    parser.add_argument("--minimum-correlation", type=float, default=0.95)
    args = parser.parse_args()
    if not math.isfinite(args.tolerance_db) or not 0 <= args.tolerance_db <= 3:
        raise SystemExit("tolerance must be finite and in [0, 3] dB")
    if not math.isfinite(args.minimum_correlation) or not 0.5 <= args.minimum_correlation <= 1:
        raise SystemExit("minimum correlation must be finite and in [0.5, 1]")

    report_path = args.report.resolve(strict=True)
    payload = json.loads(report_path.read_text(encoding="utf-8"))
    expected = args.expected_gain_db
    if expected is None:
        expected = float(payload["comparison"]["expectedDbDelta"])
    if not math.isfinite(expected):
        raise SystemExit("expected gain must be finite")
    case_data = [payload.get("off", {}), payload.get("normalized", {})]
    wave_paths = []
    for case in case_data:
        path = Path(case.get("path", "")).resolve(strict=True)
        if path.parent != report_path.parent or path.suffix.lower() != ".wav":
            raise SystemExit("capture WAVs must be beside the report in the private output directory")
        wave_paths.append(path)

    off = read_wav_samples(wave_paths[0])
    normalized = read_wav_samples(wave_paths[1])
    off_start = first_audible_sample(off)
    normalized_start = first_audible_sample(normalized)
    try:
        alignment = align_capture_samples(off, normalized, off_start, normalized_start)
    except ValueError as error:
        raise SystemExit(str(error)) from error
    normalized_start += alignment
    if normalized_start < 0 or off_start < 0:
        raise SystemExit("source alignment starts before a captured frame")
    correlation = normalized_correlation(off, normalized, off_start, normalized_start)
    if correlation < args.minimum_correlation:
        raise SystemExit(f"source alignment correlation {correlation:.5f} is below threshold")

    windows = {}
    passed = True
    for milliseconds in (20, 50, 100, 500):
        frames_count = round(RATE * milliseconds / 1000)
        off_frames = off[off_start : off_start + frames_count]
        normalized_frames = normalized[normalized_start : normalized_start + frames_count]
        if len(off_frames) != frames_count or len(normalized_frames) != frames_count:
            raise SystemExit(f"captures are too short for the {milliseconds} ms comparison")
        channel_results = {}
        for channel, index in (("left", 0), ("right", 1)):
            off_rms = rms(off_frames, index)
            normalized_rms = rms(normalized_frames, index)
            if off_rms <= 1e-4 or normalized_rms <= 1e-4:
                raise SystemExit(f"{channel} is silent in the first {milliseconds} ms")
            delta_db = 20 * math.log10(normalized_rms / off_rms)
            passed = passed and abs(delta_db - expected) <= args.tolerance_db
            channel_results[channel] = {
                "offRms": off_rms,
                "normalizedRms": normalized_rms,
                "gainDeltaDb": delta_db,
                "withinExpectedTolerance": abs(delta_db - expected) <= args.tolerance_db,
            }
        windows[f"{milliseconds}ms"] = channel_results

    summary = {
        "scope": "source-aligned stereo output captured at a private Pulse null sink",
        "engine": payload.get("engine"),
        "expectedGainDb": expected,
        "toleranceDb": args.tolerance_db,
        "sourceAlignedCorrelation": correlation,
        "normalizedStartOffsetFrames": alignment,
        "sampleRateHz": RATE,
        "windows": windows,
        "passed": passed,
        "physicalDeviceQualified": False,
    }
    output = report_path.with_name("webview-gain-window-summary.json")
    output.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=2))
    if not passed:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
