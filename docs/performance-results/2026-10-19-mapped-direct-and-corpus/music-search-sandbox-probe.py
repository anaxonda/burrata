#!/usr/bin/env python3
"""Bounded anonymous WEB_REMIX search protocol feasibility probe.

No cookies/auth, redirects disabled, HTTPS certificate verification uses urllib defaults.
The response body is only inspected in memory and never printed or persisted.
"""
import hashlib
import json
import re
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

ENDPOINT = "https://music.youtube.com/youtubei/v1/search"
USER_AGENT = "Mozilla/5.0 Burrata/0.1"
MAX_BODY = 1024 * 1024
TIMEOUT_SECONDS = 8
QUERIES = ["John Denver Matthew", "Pengus Victory Lap"]
PARAMS = "EgWKAQIIAWoKEAkQChADEAQQBQ%3D%3D"
CLIENT_VERSION = "1.20250101.01.00"
ID_RE = re.compile(r"^[A-Za-z0-9_-]{11}$")

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None

def text(value):
    if isinstance(value, str):
        result = value.strip()
        return result or None
    if isinstance(value, dict):
        if isinstance(value.get("simpleText"), str):
            return text(value["simpleText"])
        runs = value.get("runs")
        if isinstance(runs, list):
            joined = "".join(r.get("text", "") for r in runs if isinstance(r, dict) and isinstance(r.get("text"), str)).strip()
            return joined or None
    return None

def video_id(node):
    paths = [
        ("videoId",),
        ("navigationEndpoint", "watchEndpoint", "videoId"),
        ("playlistItemData", "videoId"),
        ("overlay", "musicItemThumbnailOverlayRenderer", "content", "musicPlayButtonRenderer", "playNavigationEndpoint", "watchEndpoint", "videoId"),
        ("flexColumns", 0, "musicResponsiveListItemFlexColumnRenderer", "text", "runs", 0, "navigationEndpoint", "watchEndpoint", "videoId"),
    ]
    for path in paths:
        value = node
        try:
            for key in path:
                value = value[key]
        except (KeyError, IndexError, TypeError):
            continue
        if isinstance(value, str) and ID_RE.fullmatch(value.strip()):
            return value.strip()
    return None

def get_path(node, path):
    value = node
    try:
        for key in path:
            value = value[key]
    except (KeyError, IndexError, TypeError):
        return None
    return value

def run_page_type(run):
    return get_path(run, ("navigationEndpoint", "browseEndpoint", "browseEndpointContextSupportedConfigs", "browseEndpointContextMusicConfig", "pageType"))

def duration_seconds(value):
    if not isinstance(value, str) or ":" not in value:
        return None
    parts = value.split(":")
    try:
        total = 0
        for part in parts:
            total = total * 60 + int(part)
        return total
    except ValueError:
        return None

def parse_items(root, limit=10):
    rows = []
    def walk(value):
        if len(rows) >= limit:
            return
        if isinstance(value, dict):
            vid = video_id(value)
            if vid:
                columns = value.get("flexColumns") if isinstance(value.get("flexColumns"), list) else []
                first_runs = get_path(columns[0], ("musicResponsiveListItemFlexColumnRenderer", "text", "runs")) if columns else None
                title = text(value.get("title"))
                if not title and isinstance(first_runs, list) and first_runs:
                    title = text(first_runs[0].get("text")) if isinstance(first_runs[0], dict) else None
                if not title and columns:
                    title = text(get_path(columns[0], ("musicResponsiveListItemFlexColumnRenderer", "text")))
                semantic_runs = []
                for col in columns:
                    runs = get_path(col, ("musicResponsiveListItemFlexColumnRenderer", "text", "runs"))
                    if isinstance(runs, list):
                        semantic_runs.extend(r for r in runs if isinstance(r, dict))
                artists = []
                for run in semantic_runs:
                    if run_page_type(run) == "MUSIC_PAGE_TYPE_ARTIST":
                        name = text(run.get("text"))
                        if name and name not in artists:
                            artists.append(name)
                durations = []
                for run in semantic_runs:
                    d = duration_seconds(text(run.get("text")))
                    if d is not None:
                        durations.append(d)
                if not durations:
                    fixed = get_path(value, ("fixedColumns", 0, "musicResponsiveListItemFixedColumnRenderer", "text", "simpleText"))
                    d = duration_seconds(fixed)
                    if d is not None:
                        durations.append(d)
                if title:
                    rows.append({"videoId": vid, "title": title[:512], "artist": ", ".join(artists)[:512] if artists else None, "durationSeconds": durations[-1] if durations else None})
            for child in value.values():
                walk(child)
        elif isinstance(value, list):
            for child in value:
                walk(child)
    walk(root)
    unique, seen = [], set()
    for row in rows:
        if row["videoId"] not in seen:
            unique.append(row)
            seen.add(row["videoId"])
    return unique

def structural_counts(root):
    counts = {"videoIdCandidates": 0, "musicResponsiveListItemRenderers": 0, "artistTypedRuns": 0, "fixedColumns": 0, "continuationCommands": 0}
    def walk(value):
        if isinstance(value, dict):
            if video_id(value):
                counts["videoIdCandidates"] += 1
            if "musicResponsiveListItemRenderer" in value or "musicResponsiveListItemFlexColumnRenderer" in value:
                counts["musicResponsiveListItemRenderers"] += 1
            if run_page_type(value) == "MUSIC_PAGE_TYPE_ARTIST":
                counts["artistTypedRuns"] += 1
            if "fixedColumns" in value:
                counts["fixedColumns"] += 1
            if any(k in value for k in ("continuationCommand", "nextContinuationData", "reloadContinuationData")):
                counts["continuationCommands"] += 1
            for child in value.values():
                walk(child)
        elif isinstance(value, list):
            for child in value:
                walk(child)
    walk(root)
    return counts

def request_one(query):
    payload = json.dumps({
        "context": {"client": {"clientName": "WEB_REMIX", "clientVersion": CLIENT_VERSION, "hl": "en", "gl": "US"}},
        "query": query,
        "params": PARAMS,
    }, separators=(",", ":")).encode()
    request = urllib.request.Request(ENDPOINT, data=payload, method="POST", headers={"Content-Type": "application/json", "User-Agent": USER_AGENT, "Accept": "application/json"})
    opener = urllib.request.build_opener(NoRedirect())
    started = time.monotonic()
    result = {"query": query, "startedAtUtc": datetime.now(timezone.utc).isoformat(), "endpointHost": "music.youtube.com", "requestBytes": len(payload), "clientName": "WEB_REMIX", "clientVersion": CLIENT_VERSION}
    try:
        with opener.open(request, timeout=TIMEOUT_SECONDS) as response:
            status = response.status
            headers = response.headers
            body = response.read(MAX_BODY + 1)
    except urllib.error.HTTPError as exc:
        status = exc.code
        headers = exc.headers
        body = exc.read(MAX_BODY + 1)
        result["failureCode"] = "HTTP_ERROR"
    except Exception as exc:
        status = None
        headers = None
        body = b""
        name = type(exc).__name__
        result["failureCode"] = "TIMEOUT" if "timeout" in name.lower() else "TRANSPORT_ERROR"
    result["elapsedMs"] = round((time.monotonic() - started) * 1000, 3)
    result["httpStatus"] = status
    result["contentType"] = (headers.get("Content-Type", "").split(";", 1)[0].strip().lower() if headers else None)
    result["responseBytesRead"] = len(body)
    result["responseSha256"] = hashlib.sha256(body).hexdigest() if body else None
    if len(body) > MAX_BODY:
        result["failureCode"] = "BODY_TOO_LARGE"
        result["bodyWithinLimit"] = False
    else:
        result["bodyWithinLimit"] = True
        if result.get("failureCode") is None and status is not None and 200 <= status < 300:
            if result["contentType"] not in ("application/json",) and not (result["contentType"] or "").endswith("+json"):
                result["failureCode"] = "NON_JSON_CONTENT_TYPE"
            else:
                try:
                    parsed = json.loads(body)
                    result["structuralCounts"] = structural_counts(parsed)
                    result["items"] = parse_items(parsed, 10)
                    result["parsedItemCount"] = len(result["items"])
                    result["failureCode"] = None if result["items"] else "NO_PARSEABLE_ITEMS"
                except Exception:
                    result["failureCode"] = "INVALID_JSON"
    return result

def main():
    rows = [request_one(query) for query in QUERIES]
    report = {
        "schemaVersion": 1,
        "scope": "two explicit anonymous WEB_REMIX search POSTs only; feasibility while a separate root Rust compilation was ongoing; not latency qualification",
        "dateUtc": datetime.now(timezone.utc).isoformat(),
        "limits": {"maxRequests": 2, "timeoutSecondsPerRequest": TIMEOUT_SECONDS, "maxResponseBytes": MAX_BODY, "redirects": "disabled", "cookiesOrAuth": "none", "tlsVerification": "urllib default enabled"},
        "requests": rows,
    }
    path = "/tmp/burrata-native-music-search-feasibility.json"
    with open(path, "x", encoding="utf-8") as handle:
        json.dump(report, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    print(json.dumps({"reportPath": path, "requests": [{"query": r["query"], "httpStatus": r.get("httpStatus"), "failureCode": r.get("failureCode"), "elapsedMs": r["elapsedMs"], "parsedItemCount": r.get("parsedItemCount"), "items": r.get("items"), "responseSha256": r.get("responseSha256"), "responseBytesRead": r.get("responseBytesRead"), "structuralCounts": r.get("structuralCounts")} for r in rows]}, indent=2, ensure_ascii=False))

if __name__ == "__main__":
    main()
