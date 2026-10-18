#!/usr/bin/env python3
"""Run the native decoder test against a deterministic local WAV fixture.

The Rust test consumes decoded samples without creating an audio output stream,
so native CI can exercise the same Rodio/Symphonia path without audio hardware.
"""
from __future__ import annotations

import argparse
import math
import os
import pathlib
import struct
import subprocess
import tempfile
import wave

ROOT = pathlib.Path(__file__).resolve().parents[2]
DEFAULT_TEST = "native_output::tests::decodes_generated_wav_without_audio_device"


def make_fixture(path: pathlib.Path) -> None:
    sample_rate = 48_000
    frames = bytearray()
    for index in range(sample_rate):
        sample = int(12_000 * math.sin(2 * math.pi * 440 * index / sample_rate))
        frames.extend(struct.pack("<h", sample))
    with wave.open(str(path), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(sample_rate)
        output.writeframes(frames)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cargo-test", default=DEFAULT_TEST, help="focused Rust test filter")
    args = parser.parse_args()

    with tempfile.TemporaryDirectory(prefix="burrata-native-playback-") as temporary:
        fixture = pathlib.Path(temporary) / "sine-440hz.wav"
        make_fixture(fixture)
        environment = os.environ.copy()
        environment["BURRATA_NATIVE_PLAYBACK_FIXTURE"] = str(fixture)
        command = [
            "cargo", "test", "--locked", "--manifest-path",
            str(ROOT / "src-tauri/Cargo.toml"), "--lib", args.cargo_test,
            "--", "--nocapture",
        ]
        result = subprocess.run(command, cwd=ROOT, env=environment, text=True, capture_output=True)
        output = result.stdout + result.stderr
        if result.returncode:
            print(output, end="")
            return result.returncode
        if "decodes_generated_wav_without_audio_device" not in output or " ... ok" not in output:
            raise SystemExit("Cargo completed without reporting the generated-WAV decode test as passing:\n" + output)
    print("Native playback fixture decoded successfully without opening an audio device.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
