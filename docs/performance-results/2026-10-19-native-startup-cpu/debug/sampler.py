#!/usr/bin/env python3
"""Read-only Linux process-tree CPU/RSS sampler for a verified app PID."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import time

PROC = Path("/proc")
ROLE_NAMES = ("app", "webview", "decoder", "node_helper", "helper")


def proc_stat(pid):
    try:
        raw = (PROC / str(pid) / "stat").read_text()
        end = raw.rfind(")")
        if end < 0:
            return None
        fields = raw[end + 1 :].split()
        if len(fields) < 20:
            return None
        return {
            "pid": pid,
            "state": fields[0],
            "ppid": int(fields[1]),
            "utime": int(fields[11]),
            "stime": int(fields[12]),
            "start_ticks": int(fields[19]),
        }
    except (OSError, ValueError, IndexError):
        return None


def proc_exe_basename(pid):
    try:
        return os.path.basename(os.readlink(PROC / str(pid) / "exe").removesuffix(" (deleted)"))
    except OSError:
        return ""


def proc_rss_bytes(pid, page_size):
    try:
        values = (PROC / str(pid) / "statm").read_text().split()
        return int(values[1]) * page_size
    except (OSError, ValueError, IndexError):
        return None


def proc_pss_bytes(pid):
    try:
        for line in (PROC / str(pid) / "smaps_rollup").read_text().splitlines():
            if line.startswith("Pss:"):
                return int(line.split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        pass
    return None


def proc_storage_bytes(pid):
    values = {}
    try:
        for line in (PROC / str(pid) / "io").read_text().splitlines():
            key, value = line.split(":", 1)
            if key in ("read_bytes", "write_bytes"):
                values[key] = int(value.strip())
        if len(values) == 2:
            return values
    except (OSError, ValueError):
        pass
    return None


def role_for(pid, root_pid, basename):
    if pid == root_pid:
        return "app"
    if basename in ("WebKitWebProcess", "WebKitNetworkProcess", "WebKitNetworkProcessLegacy"):
        return "webview"
    if basename == "ffmpeg":
        return "decoder"
    if basename in ("node", "nodejs"):
        return "node_helper"
    return "helper"


def process_tree(root_pid):
    all_stats = {}
    try:
        entries = list(PROC.iterdir())
    except OSError:
        return {}
    for entry in entries:
        if entry.name.isdigit():
            stat = proc_stat(int(entry.name))
            if stat is not None:
                all_stats[stat["pid"]] = stat
    children = {}
    for stat in all_stats.values():
        children.setdefault(stat["ppid"], []).append(stat["pid"])
    included = {}
    pending = [root_pid]
    while pending:
        pid = pending.pop()
        if pid in included:
            continue
        stat = all_stats.get(pid)
        if stat is None:
            continue
        included[pid] = stat
        pending.extend(children.get(pid, ()))
    return included


def parse_state(path):
    state = json.loads(Path(path).read_text())
    pid = state.get("pid")
    binary = state.get("binary")
    digest = state.get("sha256")
    if not isinstance(pid, int) or pid <= 1:
        raise ValueError("state pid must be an integer greater than 1")
    if not isinstance(binary, str) or not binary:
        raise ValueError("state binary must be a nonempty path")
    if not isinstance(digest, str) or len(digest) != 64 or any(c not in "0123456789abcdefABCDEF" for c in digest):
        raise ValueError("state sha256 must be 64 hexadecimal characters")
    return pid, Path(binary).resolve(), digest.lower()


def verify_root(pid, binary_path, expected_digest, expected_start=None, verify_hash=True):
    stat = proc_stat(pid)
    if stat is None:
        raise RuntimeError("ROOT_PID_MISSING")
    if expected_start is not None and stat["start_ticks"] != expected_start:
        raise RuntimeError("ROOT_PID_CHANGED")
    try:
        proc_exe = os.readlink(PROC / str(pid) / "exe").removesuffix(" (deleted)")
    except OSError as exc:
        raise RuntimeError("ROOT_EXE_UNAVAILABLE") from exc
    if Path(proc_exe).resolve() != binary_path:
        raise RuntimeError("ROOT_BINARY_MISMATCH")
    if verify_hash:
        try:
            hasher = hashlib.sha256()
            with binary_path.open("rb") as stream:
                for block in iter(lambda: stream.read(1024 * 1024), b""):
                    hasher.update(block)
        except OSError as exc:
            raise RuntimeError("BINARY_HASH_UNAVAILABLE") from exc
        if hasher.hexdigest() != expected_digest:
            raise RuntimeError("ROOT_HASH_MISMATCH")
    return stat


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state", required=True, help="JSON file with pid, binary, sha256")
    parser.add_argument("--output", required=True, help="JSONL output path")
    parser.add_argument("--seconds", type=float, default=30.0, help="capture duration, 0 < value <= 60")
    parser.add_argument("--interval", type=float, default=0.1, help="sample interval in seconds")
    args = parser.parse_args()
    if not (0 < args.seconds <= 60):
        parser.error("--seconds must be greater than 0 and at most 60")
    if not (0 < args.interval <= 60):
        parser.error("--interval must be greater than 0 and at most 60")

    try:
        root_pid, binary_path, expected_digest = parse_state(args.state)
        root_stat = verify_root(root_pid, binary_path, expected_digest)
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"capture refused: {exc}", file=sys.stderr)
        return 2

    ticks_per_second = os.sysconf("SC_CLK_TCK")
    page_size = os.sysconf("SC_PAGE_SIZE")
    capture_start_ticks = int(time.clock_gettime(time.CLOCK_BOOTTIME) * ticks_per_second)
    wall_start_ms = int(time.time() * 1000)
    monotonic_start_ns = time.monotonic_ns()
    deadline = time.monotonic() + args.seconds
    pss_latest = {}
    last_pss_sample = float("-inf")
    previous_by_key = {}
    initial_members = process_tree(root_pid)
    for pid, stat in initial_members.items():
        previous_by_key[(pid, stat["start_ticks"])] = {
            "cpu": stat["utime"] + stat["stime"],
            "storage": proc_storage_bytes(pid),
        }
    last_seen_keys = set(previous_by_key)
    output_path = Path(args.output)
    output_path.parent.mkdir(parents=True, exist_ok=True)

    try:
        with output_path.open("x", encoding="utf-8") as output:
            def write_record(record):
                output.write(json.dumps(record, separators=(",", ":"), sort_keys=True) + "\n")
                output.flush()

            write_record({
                "record_type": "capture_start",
                "root_pid": root_pid,
                "root_start_ticks": root_stat["start_ticks"],
                "binary_basename": binary_path.name,
                "sha256": expected_digest,
                "wall_unix_ms": wall_start_ms,
                "monotonic_start_ns": monotonic_start_ns,
                "seconds_requested": args.seconds,
                "interval_seconds": args.interval,
                "clock_ticks_per_second": ticks_per_second,
                "pss_sampling_max_hz": 1,
                "storage_counter_label": "storage_io_bytes_not_network",
            })

            sample_index = 0
            last_sample_ns = monotonic_start_ns
            while time.monotonic() < deadline:
                target = min(deadline, time.monotonic() + args.interval)
                time.sleep(max(0.0, target - time.monotonic()))
                now_mono = time.monotonic()
                now_mono_ns = time.monotonic_ns()
                now_wall_ms = int(time.time() * 1000)
                try:
                    verify_root(
                        root_pid,
                        binary_path,
                        expected_digest,
                        root_stat["start_ticks"],
                        verify_hash=False,
                    )
                except RuntimeError as exc:
                    write_record({
                        "record_type": "capture_error",
                        "code": str(exc),
                        "wall_unix_ms": now_wall_ms,
                        "monotonic_elapsed_ns": now_mono_ns - monotonic_start_ns,
                    })
                    return 3

                members = process_tree(root_pid)
                if root_pid not in members:
                    write_record({
                        "record_type": "capture_error",
                        "code": "ROOT_NOT_IN_PROCESS_TREE",
                        "wall_unix_ms": now_wall_ms,
                        "monotonic_elapsed_ns": now_mono_ns - monotonic_start_ns,
                    })
                    return 3
                current_keys = {(pid, stat["start_ticks"]) for pid, stat in members.items()}
                exited_between_polls = bool(last_seen_keys - current_keys)
                refresh_pss = now_mono - last_pss_sample >= 1.0
                if refresh_pss:
                    last_pss_sample = now_mono

                groups = {}
                for pid, stat in members.items():
                    key = (pid, stat["start_ticks"])
                    basename = proc_exe_basename(pid)
                    role = role_for(pid, root_pid, basename)
                    rss = proc_rss_bytes(pid, page_size)
                    if refresh_pss:
                        pss = proc_pss_bytes(pid)
                        if pss is not None:
                            pss_latest[key] = pss
                    pss = pss_latest.get(key)
                    storage = proc_storage_bytes(pid)
                    cpu_now = stat["utime"] + stat["stime"]
                    previous = previous_by_key.get(key)
                    if previous is not None:
                        cpu_delta_ticks = max(0, cpu_now - previous["cpu"])
                        if storage is not None and previous.get("storage") is not None:
                            read_delta = max(0, storage["read_bytes"] - previous["storage"]["read_bytes"])
                            write_delta = max(0, storage["write_bytes"] - previous["storage"]["write_bytes"])
                        else:
                            read_delta = write_delta = 0
                    elif stat["start_ticks"] >= capture_start_ticks:
                        cpu_delta_ticks = cpu_now
                        read_delta = storage["read_bytes"] if storage is not None else 0
                        write_delta = storage["write_bytes"] if storage is not None else 0
                    else:
                        # A process discovered after capture began may predate
                        # the capture, so its cumulative CPU/I/O is unknown.
                        cpu_delta_ticks = read_delta = write_delta = 0
                    previous_by_key[key] = {"cpu": cpu_now, "storage": storage}
                    item = groups.setdefault(role, {
                        "cpu_ticks": 0,
                        "rss_bytes": 0,
                        "rss_processes": 0,
                        "pss_bytes": 0,
                        "pss_processes": 0,
                        "storage_read_bytes_not_network": 0,
                        "storage_write_bytes_not_network": 0,
                        "pids": [],
                    })
                    item["cpu_ticks"] += cpu_delta_ticks
                    if rss is not None:
                        item["rss_bytes"] += rss
                        item["rss_processes"] += 1
                    if pss is not None:
                        item["pss_bytes"] += pss
                        item["pss_processes"] += 1
                    item["storage_read_bytes_not_network"] += read_delta
                    item["storage_write_bytes_not_network"] += write_delta
                    item["pids"].append({"pid": pid, "start_ticks": stat["start_ticks"]})

                roles = {}
                for role in ROLE_NAMES:
                    item = groups.get(role)
                    if item is None:
                        continue
                    item["pids"].sort(key=lambda value: (value["pid"], value["start_ticks"]))
                    roles[role] = {
                        "process_count": len(item["pids"]),
                        "pids": item["pids"],
                        "cpu_delta_seconds": item["cpu_ticks"] / ticks_per_second,
                        "rss_bytes": item["rss_bytes"],
                        "rss_processes": item["rss_processes"],
                        "pss_bytes": item["pss_bytes"] if item["pss_processes"] else None,
                        "pss_processes": item["pss_processes"],
                        "storage_read_bytes_not_network": item["storage_read_bytes_not_network"],
                        "storage_write_bytes_not_network": item["storage_write_bytes_not_network"],
                    }
                total_cpu = sum(role["cpu_delta_seconds"] for role in roles.values())
                write_record({
                    "record_type": "sample",
                    "sample_index": sample_index,
                    "wall_unix_ms": now_wall_ms,
                    "monotonic_elapsed_ns": now_mono_ns - monotonic_start_ns,
                    "interval_seconds": (now_mono_ns-last_sample_ns)/1e9,
                    "requested_interval_seconds": args.interval,
                    "exited_between_polls_omitted": exited_between_polls,
                    "pss_refreshed_this_sample": refresh_pss,
                    "process_count": sum(role["process_count"] for role in roles.values()),
                    "cpu_delta_seconds_total": total_cpu,
                    "roles": roles,
                })
                last_seen_keys = current_keys
                last_sample_ns = now_mono_ns
                sample_index += 1
    except OSError as exc:
        print(f"output error: {exc}", file=sys.stderr)
        return 4
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
