#!/usr/bin/env python3
"""Exercise the packaged FFmpeg feature contract using generated local fixtures."""
import json
import math
import pathlib
import struct
import subprocess
import sys
import tempfile
import wave

ffmpeg = pathlib.Path(sys.argv[1])
ffprobe = pathlib.Path(sys.argv[2])


def run(command: list[str]) -> str:
    return subprocess.check_output(command, text=True, stderr=subprocess.STDOUT)


def probe(path: pathlib.Path) -> dict:
    return json.loads(run([str(ffprobe), "-v", "error", "-show_format", "-show_streams", "-show_chapters", "-of", "json", str(path)]))


with tempfile.TemporaryDirectory(prefix="burrata-ffmpeg-smoke-") as tmp:
    root = pathlib.Path(tmp)
    wav_path = root / "fixture.wav"
    sample_rate = 44100
    with wave.open(str(wav_path), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(sample_rate)
        frames = bytearray()
        for index in range(sample_rate):
            sample = int(10000 * math.sin(2 * math.pi * 440 * index / sample_rate))
            frames.extend(struct.pack("<h", sample))
        out.writeframes(frames)

    encoded = {
        "mp3": ["-c:a", "libmp3lame", "-q:a", "4"],
        "opus": ["-c:a", "libopus", "-b:a", "96k"],
        "flac": ["-c:a", "flac"],
    }
    outputs = {}
    for extension, codec_args in encoded.items():
        output = root / f"fixture.{extension}"
        run([str(ffmpeg), "-nostdin", "-v", "error", "-y", "-i", str(wav_path), *codec_args, str(output)])
        metadata = probe(output)
        codecs = {stream.get("codec_name") for stream in metadata["streams"]}
        expected = "opus" if extension == "opus" else extension
        if expected not in codecs:
            raise SystemExit(f"{extension} encode/probe failed: {metadata}")
        run([str(ffmpeg), "-nostdin", "-v", "error", "-i", str(output), "-f", "null", "-"])
        outputs[extension] = output

    metadata_path = root / "chapters.ffmeta"
    metadata_path.write_text(
        """;FFMETADATA1
title=Burrata media fixture
[CHAPTER]
TIMEBASE=1/1
START=0
END=1
title=Chapter fixture
""",
        encoding="utf-8",
    )
    chapter_file = root / "chapters.mka"
    run([str(ffmpeg), "-nostdin", "-v", "error", "-y", "-i", str(wav_path), "-i", str(metadata_path), "-map_metadata", "1", "-map_chapters", "1", "-c:a", "copy", str(chapter_file)])
    chapter_metadata = probe(chapter_file)
    if chapter_metadata.get("format", {}).get("tags", {}).get("title") != "Burrata media fixture":
        raise SystemExit("container metadata muxing failed")
    if len(chapter_metadata.get("chapters", [])) != 1:
        raise SystemExit("chapter muxing/probing failed")

    ppm = root / "fixture.ppm"
    ppm.write_bytes(b"P6\n2 2\n255\n" + bytes((255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255)))
    png = root / "fixture.png"
    run([str(ffmpeg), "-nostdin", "-v", "error", "-y", "-f", "image2", "-i", str(ppm), "-frames:v", "1", str(png)])
    png_codecs = {stream.get("codec_name") for stream in probe(png)["streams"]}
    if "png" not in png_codecs:
        raise SystemExit("PNG artwork encoding/probing failed")

protocols = run([str(ffmpeg), "-hide_banner", "-protocols"])
for required in ("file", "http", "https", "pipe", "tcp", "tls", "udp"):
    if required not in protocols.split():
        raise SystemExit(f"Required media protocol missing: {required}")
print("FFmpeg media contract passed: LAME, Opus, FLAC, PNG, Matroska metadata/chapters, HTTPS and common transports")
