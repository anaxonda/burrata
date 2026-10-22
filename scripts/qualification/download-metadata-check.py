#!/usr/bin/env python3
"""Read-only comparison of downloaded audio tags with saved track metadata.

The script never opens a library for writing and never modifies audio files.
It reads a JSON manifest (by default the Bulgaria download manifest), resolves
paths from an optional SQLite `local_assets` table opened in read-only mode,
and inspects media tags/streams with ffprobe. A manifest row can include
`pcmReferencePath`; `--pcm` then compares decoded PCM hashes without writing
decoded audio to disk.

Example:
  python3 scripts/qualification/download-metadata-check.py \
    --db /path/to/burrata.sqlite --format text
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
from pathlib import Path
from typing import Any


DEFAULT_MANIFEST = Path(
    "docs/performance-results/2026-10-21-ui-downloads/bulgaria-downloads.json"
)

FIELD_TAGS: dict[str, tuple[str, ...]] = {
    "artist": ("artist", "performer"),
    "title": ("title", "trackname", "track_name"),
    "album": ("album",),
    "album_artist": ("album_artist", "albumartist", "album artist", "aart", "tpe2"),
    "date": ("date", "originaldate", "original_date", "year", "recordingdate", "recording_date", "releasedate", "release_date"),
    "genre": ("genre",),
    "label": ("label", "publisher", "organization"),
    "isrc": ("isrc", "tsrc"),
}

TRACK_KEYS: dict[str, tuple[str, ...]] = {
    "artist": ("artist",),
    "title": ("title",),
    "album": ("album",),
    "album_artist": ("albumArtist", "album_artist"),
    "date": ("releaseDate", "recordingDate", "date"),
    "genre": ("genre",),
    "label": ("label",),
    "isrc": ("isrc",),
}

SOURCE_KEYS: dict[str, tuple[str, ...]] = {
    "artist": ("artist", "artists", "artistNames", "artist_names", "performer"),
    "title": ("catalogTitle", "catalog_title", "title", "track", "song"),
    "album": ("album", "albumTitle", "album_title", "releaseTitle", "release_title"),
    "album_artist": ("albumArtist", "album_artist", "albumArtistNames", "album_artist_names"),
    "date": ("releaseDate", "release_date", "recordingDate", "recording_date", "date", "year"),
    "genre": ("genre", "genres"),
    "label": ("label", "recordLabel", "record_label", "publisher"),
    "isrc": ("isrc", "ISRC", "tsrc"),
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--db", type=Path, help="Optional Burrata SQLite DB; always opened read-only")
    parser.add_argument("--ffprobe", default="ffprobe", help="ffprobe executable (default: ffprobe)")
    parser.add_argument("--pcm", action="store_true", help="Compare decoded PCM for rows with pcmReferencePath")
    parser.add_argument("--format", choices=("json", "text"), default="json")
    return parser.parse_args()


def read_manifest(path: Path) -> list[dict[str, Any]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, list):
        raise ValueError("Manifest must contain a JSON array")
    rows = [row for row in data if isinstance(row, dict) and row.get("assetId")]
    if not rows:
        raise ValueError("Manifest has no rows with assetId")
    return rows


def open_read_only_db(path: Path) -> sqlite3.Connection:
    uri = path.resolve().as_uri() + "?mode=ro"
    connection = sqlite3.connect(uri, uri=True)
    connection.execute("PRAGMA query_only = ON")
    return connection


def library_asset(connection: sqlite3.Connection, asset_id: str) -> dict[str, Any] | None:
    row = connection.execute(
        "SELECT path, hash, payload FROM local_assets WHERE id = ?", (asset_id,)
    ).fetchone()
    if row is None:
        return None
    try:
        payload = json.loads(row[2]) if row[2] else {}
    except (TypeError, json.JSONDecodeError):
        payload = {}
    return {"path": row[0], "hash": row[1], "payload": payload}


def nonempty(mapping: Any, keys: tuple[str, ...]) -> Any:
    if not isinstance(mapping, dict):
        return None
    for key in keys:
        value = mapping.get(key)
        if value not in (None, "", [], {}):
            return value
    return None


def track_from_payload(payload: dict[str, Any]) -> dict[str, Any]:
    track = payload.get("track")
    return track if isinstance(track, dict) else payload


def candidate_source_metadata(track: dict[str, Any]) -> dict[str, Any] | None:
    provenance = track.get("provenance")
    if not isinstance(provenance, dict):
        return None
    acquisition = provenance.get("acquisition")
    autotag = provenance.get("autotag")
    for source in (acquisition, autotag):
        if isinstance(source, dict):
            for key in ("sourceMetadata", "metadata", "catalogMetadata"):
                value = source.get(key)
                if isinstance(value, dict):
                    return value
    return None


def probe_file(path: Path, ffprobe: str) -> dict[str, Any]:
    executable = shutil.which(ffprobe) if os.path.sep not in ffprobe else ffprobe
    if not executable:
        raise FileNotFoundError(f"ffprobe not found: {ffprobe}")
    completed = subprocess.run(
        [executable, "-v", "error", "-show_streams", "-show_format", "-of", "json", str(path)],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=90,
    )
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr.strip() or f"ffprobe exited {completed.returncode}")
    return json.loads(completed.stdout)


def collect_tags(probe: dict[str, Any]) -> dict[str, list[str]]:
    tags: dict[str, list[str]] = {}
    sources = [probe.get("format", {}).get("tags", {})]
    sources.extend(stream.get("tags", {}) for stream in probe.get("streams", []))
    for mapping in sources:
        if not isinstance(mapping, dict):
            continue
        for key, value in mapping.items():
            if value is None:
                continue
            normalized_key = str(key).strip().casefold()
            values = value if isinstance(value, list) else [value]
            tags.setdefault(normalized_key, []).extend(str(item) for item in values if item is not None)
    return tags


def normalize(value: Any) -> str:
    return " ".join(str(value).strip().casefold().split())


def actual_tag(tags: dict[str, list[str]], field: str) -> list[str]:
    result: list[str] = []
    for name in FIELD_TAGS[field]:
        result.extend(tags.get(name.casefold(), []))
    return list(dict.fromkeys(result))


def source_value(source_metadata: dict[str, Any] | None, field: str) -> Any:
    if source_metadata is None:
        return None
    value = nonempty(source_metadata, SOURCE_KEYS[field])
    if isinstance(value, (list, tuple)):
        return " & ".join(str(item).strip() for item in value if str(item).strip()) or None
    return value


def field_report(
    field: str, track: dict[str, Any], source_metadata: dict[str, Any] | None, tags: dict[str, list[str]]
) -> dict[str, Any]:
    expected = nonempty(track, TRACK_KEYS[field])
    provided = source_value(source_metadata, field)
    observed = actual_tag(tags, field)
    if expected is None:
        status = "no_library_value"
    elif not observed:
        status = "missing_tag"
    elif normalize(expected) in {normalize(value) for value in observed}:
        status = "match"
    else:
        status = "tag_mismatch"

    if provided is None:
        source_status = "not_recorded" if source_metadata is None else "not_provided_in_snapshot"
    elif not observed:
        source_status = "source_value_available_but_tag_missing"
    elif normalize(provided) in {normalize(value) for value in observed}:
        source_status = "source_value_tagged"
    else:
        source_status = "source_value_differs_from_tag"

    return {
        "expected": expected,
        "observed": observed,
        "sourceProvided": provided,
        "sourceStatus": source_status,
        "status": status,
    }


def pcm_digest(path: Path, ffmpeg: str = "ffmpeg", ffprobe: str = "ffprobe") -> dict[str, Any]:
    """Hash decoded PCM through a pipe; never writes a decoded audio file."""
    ffmpeg_exe = shutil.which(ffmpeg)
    ffprobe_exe = shutil.which(ffprobe)
    if not ffmpeg_exe or not ffprobe_exe:
        raise FileNotFoundError("PCM comparison needs both ffmpeg and ffprobe")
    probe = probe_file(path, ffprobe_exe)
    audio = next((stream for stream in probe.get("streams", []) if stream.get("codec_type") == "audio"), None)
    if not audio:
        raise ValueError("No audio stream found")
    pcm_config = {
        "sampleRate": audio.get("sample_rate"),
        "channels": audio.get("channels"),
        "channelLayout": audio.get("channel_layout"),
    }
    digest = hashlib.sha256(json.dumps(pcm_config, sort_keys=True).encode("utf-8"))
    proc = subprocess.Popen(
        [
            ffmpeg_exe,
            "-v",
            "error",
            "-i",
            str(path),
            "-map",
            "0:a:0",
            "-f",
            "s32le",
            "-acodec",
            "pcm_s32le",
            "-",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert proc.stdout is not None
    while chunk := proc.stdout.read(1024 * 1024):
        digest.update(chunk)
    stderr = proc.stderr.read().decode("utf-8", "replace") if proc.stderr else ""
    code = proc.wait(timeout=600)
    if code != 0:
        raise RuntimeError(stderr.strip() or f"ffmpeg exited {code}")
    return {"sha256": digest.hexdigest(), **pcm_config}


def inspect_row(
    row: dict[str, Any], db: sqlite3.Connection | None, ffprobe: str, compare_pcm: bool
) -> dict[str, Any]:
    asset_id = str(row["assetId"])
    asset = library_asset(db, asset_id) if db else None
    track = track_from_payload(asset["payload"]) if asset else row
    path_value = row.get("path") or (asset or {}).get("path")
    if not path_value:
        return {"assetId": asset_id, "title": row.get("title"), "error": "path missing (supply --db or manifest path)"}
    path = Path(path_value)
    if not path.is_file():
        return {"assetId": asset_id, "title": row.get("title"), "path": str(path), "error": "file not found"}

    try:
        probe = probe_file(path, ffprobe)
        tags = collect_tags(probe)
    except Exception as exc:  # report per-file failure and continue
        return {"assetId": asset_id, "title": row.get("title"), "path": path.name, "error": str(exc)}

    fmt = probe.get("format", {})
    audio = next((stream for stream in probe.get("streams", []) if stream.get("codec_type") == "audio"), {})
    source_metadata = candidate_source_metadata(track)
    fields = {
        name: field_report(name, track, source_metadata, tags)
        for name in FIELD_TAGS
    }
    result: dict[str, Any] = {
        "assetId": asset_id,
        "title": track.get("title") or row.get("title"),
        "path": path.name,
        "file": {
            "codec": audio.get("codec_name"),
            "sampleRate": audio.get("sample_rate"),
            "channels": audio.get("channels"),
            "bitsPerSample": audio.get("bits_per_raw_sample") or audio.get("bits_per_sample"),
            "durationSeconds": audio.get("duration") or fmt.get("duration"),
            "sizeBytes": path.stat().st_size,
        },
        "fields": fields,
        "sourceMetadataSnapshotRecorded": source_metadata is not None,
    }

    if compare_pcm and row.get("pcmReferencePath"):
        try:
            current = pcm_digest(path, ffprobe=ffprobe)
            reference = pcm_digest(Path(row["pcmReferencePath"]), ffprobe=ffprobe)
            result["decodedPcm"] = {
                "equal": current["sha256"] == reference["sha256"],
                "current": current,
                "reference": reference,
            }
        except Exception as exc:
            result["decodedPcm"] = {"error": str(exc)}
    elif compare_pcm:
        result["decodedPcm"] = {"status": "no_reference_path_in_manifest"}
    return result


def print_text(results: list[dict[str, Any]]) -> None:
    print(f"Checked {len(results)} audio file(s); read-only metadata inspection.")
    for result in results:
        title = result.get("title") or result.get("assetId")
        if "error" in result:
            print(f"\n{title}: ERROR: {result['error']}")
            continue
        file_info = result["file"]
        print(
            f"\n{title} — {file_info.get('codec')}, {file_info.get('sampleRate')} Hz, "
            f"{file_info.get('channels')} ch, {file_info.get('durationSeconds')} s"
        )
        print(f"  tags: {', '.join(result['fields'][f]['status'] for f in ('artist', 'title', 'album'))} for artist/title/album")
        for field, detail in result["fields"].items():
            print(
                f"  {field}: {detail['status']}; source={detail['sourceStatus']}; "
                f"expected={detail['expected']!r}; observed={detail['observed']!r}"
            )
        if not result["sourceMetadataSnapshotRecorded"]:
            print("  source metadata snapshot: not recorded; missing tags cannot be attributed to provider omission vs tag-writer support")
        if "decodedPcm" in result:
            print(f"  decoded PCM: {result['decodedPcm']}")


def main() -> int:
    args = parse_args()
    try:
        rows = read_manifest(args.manifest)
        db = open_read_only_db(args.db) if args.db else None
        results = [inspect_row(row, db, args.ffprobe, args.pcm) for row in rows]
        if db:
            db.close()
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    if args.format == "json":
        print(json.dumps(results, ensure_ascii=False, indent=2))
    else:
        print_text(results)
    return 1 if any("error" in result for result in results) else 0


if __name__ == "__main__":
    raise SystemExit(main())
