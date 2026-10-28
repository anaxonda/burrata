"""Synthetic loopback check for isolated PulseAudio null-sink capture."""

from __future__ import annotations

import json
import math
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
import wave

from linux_output_gain_qualification import raw_s16le_to_wav

RATE = 48_000


def run(command: list[str], *, check: bool = True, env: dict | None = None) -> str:
    result = subprocess.run(command, check=False, capture_output=True, text=True, env=env)
    if check and result.returncode:
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(command)}\n{result.stderr}"
        )
    return result.stdout.strip()


def generate_fixture(path: Path) -> dict:
    # 100 ms silence followed by distinct left/right sine waves. No user audio.
    left_peak = 0.32
    right_peak = 0.17
    with wave.open(str(path), "wb") as output:
        output.setnchannels(2)
        output.setsampwidth(2)
        output.setframerate(RATE)
        pcm = bytearray()
        for index in range(RATE * 2):
            if index < RATE // 10:
                left = right = 0
            else:
                t = index / RATE
                left = round(left_peak * math.sin(2 * math.pi * 440 * t) * 32767)
                right = round(right_peak * math.sin(2 * math.pi * 880 * t) * 32767)
            pcm.extend(left.to_bytes(2, "little", signed=True))
            pcm.extend(right.to_bytes(2, "little", signed=True))
        output.writeframes(pcm)
    return {"durationSeconds": 2, "leftPeakExpected": left_peak, "rightPeakExpected": right_peak}


def captured_peaks(path: Path) -> dict:
    with wave.open(str(path), "rb") as recorded:
        if (recorded.getnchannels(), recorded.getsampwidth(), recorded.getframerate()) != (2, 2, RATE):
            raise RuntimeError("monitor capture did not retain expected stereo s16le/48 kHz format")
        raw = recorded.readframes(recorded.getnframes())
    if len(raw) % 4:
        raise RuntimeError("monitor returned a partial PCM frame")
    channel_peaks = [0.0, 0.0]
    active_frames = 0
    for offset in range(0, len(raw), 4):
        left = int.from_bytes(raw[offset : offset + 2], "little", signed=True) / 32768
        right = int.from_bytes(raw[offset + 2 : offset + 4], "little", signed=True) / 32768
        channel_peaks[0] = max(channel_peaks[0], abs(left))
        channel_peaks[1] = max(channel_peaks[1], abs(right))
        if max(abs(left), abs(right)) > 1e-4:
            active_frames += 1
    return {
        "frames": len(raw) // 4,
        "durationSeconds": len(raw) / 4 / RATE,
        "leftPeak": channel_peaks[0],
        "rightPeak": channel_peaks[1],
        "activeFrames": active_frames,
    }


def wait_for_raw_frames(path: Path, minimum_frames: int, timeout_seconds: float) -> int:
    deadline = time.monotonic() + timeout_seconds
    minimum_bytes = minimum_frames * 4
    while time.monotonic() < deadline:
        if path.exists():
            size = path.stat().st_size
            if size >= minimum_bytes:
                return size // 4
        time.sleep(0.01)
    size = path.stat().st_size if path.exists() else 0
    raise RuntimeError(
        f"monitor did not capture enough PCM before deadline: "
        f"{size // 4} frames, needed {minimum_frames}"
    )


def main() -> int:
    for executable in ("pactl", "parec", "paplay"):
        if shutil.which(executable) is None:
            raise RuntimeError(f"required executable is missing: {executable}")
    run(["pactl", "info"])
    sink = f"burrata_smoke_{os.getpid()}_{uuid.uuid4().hex[:6]}"
    module_id = run(
        [
            "pactl",
            "load-module",
            "module-null-sink",
            f"sink_name={sink}",
            f"rate={RATE}",
            "channels=2",
            "channel_map=front-left,front-right",
            f"sink_properties=device.description={sink}",
        ]
    )
    if not module_id.isdecimal():
        raise RuntimeError("PulseAudio did not return a null-sink module ID")
    try:
        with tempfile.TemporaryDirectory(prefix="burrata-pulse-smoke-") as temporary:
            folder = Path(temporary)
            source = folder / "synthetic-stereo.wav"
            capture = folder / "monitor.wav"
            raw_capture = folder / "monitor.s16le"
            expected = generate_fixture(source)
            raw_output = raw_capture.open("wb")
            recorder = subprocess.Popen(
                [
                    "parec",
                    f"--device={sink}.monitor",
                    "--raw",
                    "--format=s16le",
                    f"--rate={RATE}",
                    "--channels=2",
                    "--latency-msec=20",
                    "--process-time-msec=10",
                ],
                stdout=raw_output,
                stderr=subprocess.PIPE,
            )
            try:
                time.sleep(0.35)
                env = os.environ.copy()
                env["PULSE_SINK"] = sink
                playback_started = time.monotonic()
                run(
                    [
                        "paplay",
                        f"--device={sink}",
                        "--latency-msec=20",
                        "--process-time-msec=10",
                        str(source),
                    ],
                    env=env,
                )
                playback_elapsed = time.monotonic() - playback_started
                recorder_running_after_playback = recorder.poll() is None
                # Require all of the two-second fixture plus 150 ms tail in the
                # monitor timeline; this is bounded and independent of sleeps.
                requested_frames = round((expected["durationSeconds"] + 0.15) * RATE)
                captured_before_stop_frames = wait_for_raw_frames(raw_capture, requested_frames, 5.0)
            finally:
                if recorder.poll() is None:
                    recorder.send_signal(signal.SIGINT)
                    try:
                        recorder_return_code = recorder.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        recorder.kill()
                        recorder_return_code = recorder.wait(timeout=3)
                else:
                    recorder_return_code = recorder.returncode
                raw_output.close()
            raw_s16le_to_wav(raw_capture, capture)
            actual = captured_peaks(capture)
            if actual["activeFrames"] < RATE // 3:
                raise RuntimeError(
                    "synthetic playback did not appear on the null-sink monitor: "
                    f"captured={actual}, paplayElapsed={playback_elapsed:.3f}s, "
                    f"recorderRunningAfterPlayback={recorder_running_after_playback}, "
                    f"recorderReturnCode={recorder_return_code}"
                )
            if actual["leftPeak"] < 0.25 or actual["rightPeak"] < 0.12:
                raise RuntimeError("monitor signal is unexpectedly attenuated or a channel is missing")
            if actual["leftPeak"] > 0.35 or actual["rightPeak"] > 0.2:
                raise RuntimeError("monitor signal is unexpectedly amplified or clipped")
            print(
                json.dumps(
                    {
                        "scope": "synthetic paplay to one private PulseAudio null sink monitor",
                        "sink": sink,
                        "normalDefaultSinkChanged": False,
                        "userSinkVolumeChanged": False,
                        "expected": expected,
                        "captured": actual,
                        "paplayElapsedSeconds": playback_elapsed,
                        "recorderRunningAfterPlayback": recorder_running_after_playback,
                        "capturedBeforeStopFrames": captured_before_stop_frames,
                        "recorderReturnCode": recorder_return_code,
                        "status": "pass",
                        "physicalOutputQualified": False,
                    },
                    indent=2,
                )
            )
    finally:
        run(["pactl", "unload-module", module_id], check=False)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"synthetic PulseAudio null-sink smoke failed: {error}", file=sys.stderr)
        raise SystemExit(2)
