#!/usr/bin/env python3
"""Report FFmpeg Opus timestamp trimming for the bounded offline CI fixture."""

from __future__ import annotations

import json
import pathlib
import shutil
import subprocess
import tempfile


ROOT = pathlib.Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "src-tauri/tests/fixtures/opus-fixture.ogg"


def run(args: list[str]) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(args, check=True, capture_output=True, timeout=30)


def main() -> None:
    ffmpeg = shutil.which("ffmpeg")
    if ffmpeg is None:
        raise SystemExit("ffmpeg is required for the CI diagnostic")

    version_output = run([ffmpeg, "-version"]).stdout.decode("utf-8", "replace")
    version = version_output.splitlines()[0] if version_output else "unknown"

    with tempfile.TemporaryDirectory(prefix="burrata-opus-trim-") as temporary:
        remuxed = pathlib.Path(temporary) / "fixture.webm"
        run(
            [
                ffmpeg,
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-i",
                str(FIXTURE),
                "-map",
                "0:a:0",
                "-c:a",
                "copy",
                "-f",
                "webm",
                str(remuxed),
            ]
        )

        def decode(*filter_args: str) -> int:
            output = run(
                [
                    ffmpeg,
                    "-nostdin",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-rw_timeout",
                    "5000000",
                    "-i",
                    str(remuxed),
                    "-t",
                    "1",
                    *filter_args,
                    "-vn",
                    "-f",
                    "f32le",
                    "-acodec",
                    "pcm_f32le",
                    "-ar",
                    "48000",
                    "-ac",
                    "2",
                    "pipe:1",
                ]
            )
            return len(output.stdout) // 4

        report = {
            "schemaVersion": 1,
            "probe": "opus-webm-one-second-output-timestamp-normalization",
            "ffmpegVersion": version,
            "fixtureBytes": FIXTURE.stat().st_size,
            "remuxBytes": remuxed.stat().st_size,
            "expectedF32Samples": 96_000,
            "defaultF32Samples": decode(),
            "asetptsF32Samples": decode("-af", "asetpts=PTS-STARTPTS"),
            "scope": "offline test fixture, one-second FFmpeg CLI counts; no runtime mutation",
        }
        print(json.dumps(report, separators=(",", ":")))


if __name__ == "__main__":
    main()
