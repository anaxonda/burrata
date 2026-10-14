#!/usr/bin/env python3
"""Require the pinned public corresponding-source assets before artifact collection."""
import hashlib
import json
import pathlib
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
LOCK = json.loads((ROOT / "scripts/release/locked-tools.json").read_text(encoding="utf-8"))
recipe = LOCK["ffmpeg"]["btbnBuildRecipe"]
bundle = recipe["sourceCache"]["sourceBundle"]
manifest_lock = bundle["manifest"]
local_manifest = ROOT / manifest_lock["localPath"]
local_bytes = local_manifest.read_bytes()
local_sha = hashlib.sha256(local_bytes).hexdigest()
if local_sha != manifest_lock["sha256"]:
    raise SystemExit(f"Checked-in BtbN source manifest SHA-256 mismatch: expected {manifest_lock['sha256']}, got {local_sha}")

request = urllib.request.Request(
    manifest_lock["url"], headers={"User-Agent": "Burrata-release-source-verifier"}
)
with urllib.request.urlopen(request, timeout=120) as response:
    remote_manifest = response.read(2 * 1024 * 1024 + 1)
if remote_manifest != local_bytes:
    raise SystemExit("Published BtbN source manifest is missing or differs from the pinned local manifest")

manifest = json.loads(local_bytes.decode("utf-8"))
source_cache = manifest.get("sourceCache", {})
if (manifest.get("commit") != recipe["commit"]
        or manifest.get("releaseTag") != recipe["releaseTag"]
        or source_cache.get("workflowRunId") != recipe["sourceCache"]["workflowRunId"]
        or source_cache.get("artifactSha256") != recipe["sourceCache"]["artifactSha256"]):
    raise SystemExit("BtbN source manifest provenance differs from the pinned build")
expected_parts = [
    {"file": part["asset"], "bytes": part["bytes"], "sha256": part["sha256"]}
    for part in bundle["parts"]
]
if source_cache.get("parts") != expected_parts or source_cache.get("uniqueSourceEntries") != 119:
    raise SystemExit("BtbN source manifest part hashes or source-entry count differ from the lock")

for part in bundle["parts"]:
    request = urllib.request.Request(
        part["url"], headers={"User-Agent": "Burrata-release-source-verifier"}, method="HEAD"
    )
    with urllib.request.urlopen(request, timeout=120) as response:
        content_length = response.headers.get("Content-Length")
        if response.status != 200 or content_length != str(part["bytes"]):
            raise SystemExit(f"Published BtbN source asset is missing or has an unexpected size: {part['asset']}")

print(f"Verified published BtbN corresponding-source bundle {bundle['releaseTag']} ({local_sha})")
