#!/usr/bin/env python3
"""Summarize app CPU sampler JSONL against playback trial wall-clock markers."""
import argparse
import json
import math
from pathlib import Path
import statistics

ROLES = ("app", "webview", "decoder", "node_helper", "helper")


def percentile(values, quantile):
    if not values:
        return None
    ordered = sorted(values)
    position = (len(ordered) - 1) * quantile
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return ordered[lower]
    weight = position - lower
    return ordered[lower] * (1 - weight) + ordered[upper] * weight


def read_capture(path):
    records = [json.loads(line) for line in Path(path).read_text().splitlines() if line.strip()]
    starts = [record for record in records if record.get("record_type") == "capture_start"]
    samples = [record for record in records if record.get("record_type") == "sample"]
    if len(starts) != 1 or not samples:
        raise ValueError(f"capture needs one header and samples: {path}")
    start = starts[0]
    samples.sort(key=lambda row: row.get("monotonic_elapsed_ns", 0))
    return start, samples


def sample_intervals(samples):
    previous_ns = 0
    intervals = []
    for sample in samples:
        end_ns = int(sample["monotonic_elapsed_ns"])
        start_ns = previous_ns
        dt = max(0.0, (end_ns - start_ns) / 1_000_000_000)
        if dt > 0:
            intervals.append((sample, start_ns / 1e9, end_ns / 1e9, dt))
        previous_ns = end_ns
    return intervals


def summarize_window(samples, intervals, capture_start_wall_ms, command_wall_ms, begin_s, end_s):
    # Map the sampler's monotonic elapsed time to command-relative seconds with
    # the sampler's wall-clock start marker; CPU values themselves stay monotonic.
    capture_offset = (capture_start_wall_ms - command_wall_ms) / 1000.0
    window_begin = begin_s
    window_end = end_s
    width = window_end - window_begin
    cpu = {role: 0.0 for role in ROLES}
    peaks = {role: 0.0 for role in ROLES}
    process_counts = {role: [] for role in ROLES}
    pss_values = {role: [] for role in ROLES}
    covered = 0.0
    sample_dt = []
    omitted = 0
    for sample, start_s, end_s, dt in intervals:
        sample_start = capture_offset + start_s
        sample_end = capture_offset + end_s
        overlap = max(0.0, min(sample_end, window_end) - max(sample_start, window_begin))
        if overlap <= 0:
            continue
        fraction = min(1.0, overlap / dt)
        covered += overlap
        sample_dt.append(dt)
        omitted += int(bool(sample.get("exited_between_polls_omitted")))
        for role in ROLES:
            metrics = sample.get("roles", {}).get(role, {})
            delta = float(metrics.get("cpu_delta_seconds", 0.0))
            cpu[role] += delta * fraction
            # Peak uses the observed CPU fraction over the whole actual sample
            # interval, which is preferable to amplifying a boundary-clipped
            # sample into a tiny synthetic interval.
            peaks[role] = max(peaks[role], 100.0 * delta / dt if dt else 0.0)
            process_counts[role].append(int(metrics.get("process_count", 0)))
            pss = metrics.get("pss_bytes")
            if isinstance(pss, (int, float)):
                pss_values[role].append(int(pss))
    cpu_total = sum(cpu.values())
    total_peaks = []
    for sample, start_s, end_s, dt in intervals:
        sample_start = capture_offset + start_s
        sample_end = capture_offset + end_s
        if max(0.0, min(sample_end, window_end) - max(sample_start, window_begin)) <= 0:
            continue
        total_peaks.append(100.0 * float(sample.get("cpu_delta_seconds_total", 0.0)) / dt if dt else 0.0)
    output = {
        "window_command_relative_seconds": [window_begin, window_end],
        "window_seconds": width,
        "covered_seconds": covered,
        "coverage_fraction": min(1.0, covered / width) if width else None,
        "cpu_seconds_by_role": cpu,
        "cpu_seconds_total": cpu_total,
        "one_core_percent_by_role": {role: cpu[role] / width * 100.0 for role in ROLES},
        "one_core_percent_total": cpu_total / width * 100.0,
        "actual_interval_peak_one_core_percent_by_role": peaks,
        "actual_interval_peak_one_core_percent_total": max(total_peaks) if total_peaks else None,
        "process_count_bounds_by_role": {
            role: {"min": min(values), "max": max(values)} if values else None
            for role, values in process_counts.items()
        },
        "pss_bytes_bounds_by_role": {
            role: {"min": min(values), "max": max(values), "samples": len(values)} if values else None
            for role, values in pss_values.items()
        },
        "actual_sample_dt_seconds": {
            "min": min(sample_dt) if sample_dt else None,
            "median": statistics.median(sample_dt) if sample_dt else None,
            "p95": percentile(sample_dt, 0.95),
            "max": max(sample_dt) if sample_dt else None,
            "intervals_overlapping_window": len(sample_dt),
        },
        "samples_with_possible_exit_or_reparent_omission": omitted,
    }
    return output


def summarize_trial(directory, trial_index):
    trial_path = directory / f"trial-{trial_index}.json"
    capture_path = directory / f"trial-{trial_index}.cpu.jsonl"
    trial = json.loads(trial_path.read_text())
    capture, samples = read_capture(capture_path)
    if trial.get("schema") != "burrata.native-cpu-trial/v1":
        raise ValueError(f"unsupported trial schema: {trial_path}")
    if trial.get("binarySha256") != capture.get("sha256"):
        raise ValueError(f"binary hash mismatch in trial {trial_index}")
    markers = trial["markers"]
    command_ms = int(markers["commandSentUnixMs"])
    intervals = sample_intervals(samples)
    capture_wall_ms = int(capture["wall_unix_ms"])
    startup = summarize_window(samples, intervals, capture_wall_ms, command_ms, 0.0, 5.0)
    steady = summarize_window(samples, intervals, capture_wall_ms, command_ms, 15.0, 20.0)
    pause_ms = int(markers["pauseSentUnixMs"])
    pause_begin_s = (pause_ms - command_ms) / 1000.0
    paused = summarize_window(
        samples,
        intervals,
        capture_wall_ms,
        command_ms,
        pause_begin_s,
        pause_begin_s + 2.0,
    )
    all_dt = [entry[3] for entry in intervals]
    return {
        "trial_index": trial_index,
        "binary_sha256": capture["sha256"],
        "root_pid": capture["root_pid"],
        "root_start_ticks": capture["root_start_ticks"],
        "markers_unix_ms": markers,
        "readiness": {
            "first_observed_playing_unix_ms": markers.get("firstObservedPlayingUnixMs"),
            "command_to_first_observed_playing_ms": (
                markers.get("firstObservedPlayingUnixMs", 0) - command_ms
                if markers.get("firstObservedPlayingUnixMs") is not None
                else None
            ),
            "source_ready_ms_after_native_request_start": trial.get("startup", {}).get("sourceReadyMs"),
            "first_audio_ms_after_native_request_start": trial.get("startup", {}).get("firstAudioMs"),
            "decoder_kind": trial.get("startup", {}).get("decoderKind"),
        },
        "actual_sample_dt_seconds_capture_wide": {
            "min": min(all_dt) if all_dt else None,
            "median": statistics.median(all_dt) if all_dt else None,
            "p95": percentile(all_dt, 0.95),
            "max": max(all_dt) if all_dt else None,
            "interval_count": len(all_dt),
        },
        "windows": {
            "startup_0_5s_after_command": startup,
            "steady_15_20s_after_command": steady,
            "paused_first_2s_after_pause": paused,
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", default="/tmp/burrata-native-cpu-current")
    parser.add_argument("--output", default="/tmp/burrata-native-cpu-current/summary.json")
    parser.add_argument("--trials", type=int, nargs="+", default=[1, 2, 3])
    args = parser.parse_args()
    directory = Path(args.directory)
    result = {
        "schema": "burrata.native-cpu-summary/v1",
        "method": {
            "window_cpu": "sample CPU deltas apportioned by actual monotonic sample interval overlap",
            "one_core_percent": "CPU-seconds divided by requested window wall seconds times 100",
            "peak": "maximum full actual sample-interval one-core percentage among intervals overlapping the window",
            "paused_window": "first 2 seconds after pauseSentUnixMs",
            "pss": "endpoint snapshots; refresh at most 1 Hz and may be stale within a window",
        },
        "caveats": [
            "Boundary-overlapping sample CPU is apportioned uniformly over that sample's actual interval.",
            "A process-count/PSS bound uses sample endpoints whose intervals overlap each window; short-lived children can be missed between polls.",
            "PSS values can be stale because the sampler reads smaps_rollup at no more than 1 Hz.",
            "RSS sums double-count shared pages; PSS sums approximate proportional residency for the sampled processes, with cached endpoint values.",
            "Sampler, MCP polling, Vite, and other external client CPU is excluded; no physical audibility is measured.",
        ],
        "trials": [summarize_trial(directory, index) for index in args.trials],
    }
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"output": str(output), "trials": len(result["trials"]), "schema": result["schema"]}))


if __name__ == "__main__":
    main()
