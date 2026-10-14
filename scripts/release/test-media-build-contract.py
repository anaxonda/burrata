#!/usr/bin/env python3
"""Check source-build lock and per-target policy without network access."""
import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
release = ROOT / "scripts/release"
lock = json.loads((release / "locked-tools.json").read_text(encoding="utf-8"))
media = lock["ffmpeg"]
targets = {
    "aarch64-apple-darwin": "build-macos-media-tools.py",
    "x86_64-pc-windows-msvc": "build-windows-media-tools.py",
    "x86_64-unknown-linux-gnu": "build-linux-media-tools.py",
}
assert set(media["tlsBackends"]) == set(targets)
assert media["source"]["archive"].endswith(".tar.xz")
for name, item in [("ffmpeg", media["source"]), *[(key, lock[key]) for key in ("lame", "opus", "zlib", "openssl")]]:
    digest = item["sha256"]
    assert len(digest) == 64 and all(char in "0123456789abcdef" for char in digest), f"invalid {name} hash"
    assert item["url"].startswith("https://"), f"{name} source must use HTTPS"
for target, builder in targets.items():
    assert (release / builder).is_file(), f"missing builder for {target}"
    tls = media["tlsBackends"][target]
    assert tls["flag"] in {"--enable-securetransport", "--enable-openssl", "--enable-schannel"}
    assert sum(flag in tls.values() for flag in ("--enable-securetransport", "--enable-openssl", "--enable-schannel")) == 1
assert media["configureCommon"] == [
    "--disable-autodetect", "--disable-shared", "--enable-static", "--enable-gpl", "--enable-version3",
    "--enable-libmp3lame", "--enable-libopus", "--enable-encoder=png", "--enable-zlib",
]
assert lock["openssl"]["targets"] == ["x86_64-unknown-linux-gnu"]
assert not any(key.lower().startswith("btbn") for key in media)
print("Source-build lock contract is valid for macOS, Windows, and Linux.")
