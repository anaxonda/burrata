#!/usr/bin/env python3
"""Bounded anonymous YouTube WEB search feasibility probe.
No cookies/auth, redirects disabled, TLS verification uses urllib defaults.
Response bodies are inspected in memory only; reports contain allowlisted fields.
"""
import hashlib, json, re, time, urllib.error, urllib.request
from datetime import datetime, timezone

ENDPOINT = "https://www.youtube.com/youtubei/v1/search?prettyPrint=false"
USER_AGENT = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/131 Safari/537.36"
MAX_BODY = 1024 * 1024
TIMEOUT_SECONDS = 8
QUERIES = ["John Denver Matthew", "Pengus Victory Lap"]
PARAMS = "EgIQAfABAQ=="  # pinned yt-dlp YoutubeSearchIE videos-only filter
CLIENT_VERSION = "2.20260623.01.00"  # checked-in youtubei.js 18.1.0 WEB profile
ID_RE = re.compile(r"^[A-Za-z0-9_-]{11}$")

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None

def plain_text(value):
    if isinstance(value, str):
        return value.strip() or None
    if isinstance(value, dict):
        simple = value.get("simpleText")
        if isinstance(simple, str):
            return simple.strip() or None
        runs = value.get("runs")
        if isinstance(runs, list):
            s = "".join(x.get("text", "") for x in runs if isinstance(x, dict) and isinstance(x.get("text"), str)).strip()
            return s or None
    return None

def duration_seconds(value):
    if not isinstance(value, str) or ":" not in value:
        return None
    try:
        total = 0
        for part in value.split(":"):
            total = total * 60 + int(part)
        return total
    except ValueError:
        return None

def bounded_text(value):
    if not isinstance(value, str):
        return None
    value = " ".join(value.split())
    if not value or len(value) > 512 or any(ord(c) < 32 for c in value):
        return None
    return value

def parse_items(root, limit=20):
    rows, seen = [], set()
    def walk(value):
        if len(rows) >= limit:
            return
        if isinstance(value, dict):
            renderer = value.get("videoRenderer")
            if isinstance(renderer, dict):
                vid = renderer.get("videoId")
                title = bounded_text(plain_text(renderer.get("title")))
                byline = renderer.get("ownerText") or renderer.get("shortBylineText") or renderer.get("longBylineText")
                uploader = bounded_text(plain_text(byline))
                dur = renderer.get("lengthSeconds")
                if not (isinstance(dur, int) and not isinstance(dur, bool) and 0 <= dur <= 86400):
                    dur = duration_seconds(plain_text(renderer.get("lengthText")))
                if isinstance(vid, str) and ID_RE.fullmatch(vid) and title and vid not in seen:
                    rows.append({"videoId": vid, "title": title, "durationSeconds": dur, "uploader": uploader})
                    seen.add(vid)
            for child in value.values():
                walk(child)
        elif isinstance(value, list):
            for child in value:
                walk(child)
    walk(root)
    return rows

def structural_counts(root):
    counts = {"videoRenderer": 0, "continuationItems": 0, "itemSectionRenderers": 0}
    def walk(v):
        if isinstance(v, dict):
            if "videoRenderer" in v: counts["videoRenderer"] += 1
            if "continuationItemRenderer" in v: counts["continuationItems"] += 1
            if "itemSectionRenderer" in v: counts["itemSectionRenderers"] += 1
            for x in v.values(): walk(x)
        elif isinstance(v, list):
            for x in v: walk(x)
    walk(root)
    return counts

def request_one(query):
    payload = json.dumps({"context": {"client": {"clientName": "WEB", "clientVersion": CLIENT_VERSION, "hl": "en", "gl": "US"}}, "query": query, "params": PARAMS}, separators=(",", ":")).encode()
    req = urllib.request.Request(ENDPOINT, data=payload, method="POST", headers={"Content-Type": "application/json", "User-Agent": USER_AGENT, "Accept": "application/json"})
    opener = urllib.request.build_opener(NoRedirect())
    started = time.monotonic()
    result = {"query": query, "startedAtUtc": datetime.now(timezone.utc).isoformat(), "endpointHost": "www.youtube.com", "requestBytes": len(payload), "clientName": "WEB", "clientVersion": CLIENT_VERSION, "params": PARAMS}
    try:
        with opener.open(req, timeout=TIMEOUT_SECONDS) as response:
            status, headers, body = response.status, response.headers, response.read(MAX_BODY + 1)
    except urllib.error.HTTPError as exc:
        status, headers, body = exc.code, exc.headers, exc.read(MAX_BODY + 1)
        result["failureCode"] = "HTTP_ERROR"
    except Exception as exc:
        status, headers, body = None, None, b""
        name = type(exc).__name__
        result["transportExceptionType"] = name if name in {"URLError", "TimeoutError", "SSLError", "gaierror", "ConnectionError", "OSError"} else "OTHER"
        reason = getattr(exc, "reason", None)
        err = getattr(reason, "errno", None) if reason is not None else getattr(exc, "errno", None)
        result["dnsErrno"] = err if isinstance(err, int) and (name == "gaierror" or (name == "URLError" and isinstance(reason, OSError))) else None
        result["failureCode"] = "TIMEOUT" if "timeout" in name.lower() else "TRANSPORT_ERROR"
    result["elapsedMs"] = round((time.monotonic() - started) * 1000, 3)
    result["httpStatus"] = status
    result["contentType"] = headers.get("Content-Type", "").split(";", 1)[0].strip().lower() if headers else None
    result["responseBytesRead"] = len(body)
    result["responseSha256"] = hashlib.sha256(body).hexdigest() if body else None
    if len(body) > MAX_BODY:
        result.update(failureCode="BODY_TOO_LARGE", bodyWithinLimit=False)
    else:
        result["bodyWithinLimit"] = True
        if result.get("failureCode") is None and status is not None and 200 <= status < 300:
            if result["contentType"] != "application/json" and not (result["contentType"] or "").endswith("+json"):
                result["failureCode"] = "NON_JSON_CONTENT_TYPE"
            else:
                try:
                    parsed = json.loads(body)
                    result["structuralCounts"] = structural_counts(parsed)
                    result["items"] = parse_items(parsed)
                    result["parsedItemCount"] = len(result["items"])
                    result["failureCode"] = None if result["items"] else "NO_PARSEABLE_ITEMS"
                except Exception:
                    result["failureCode"] = "INVALID_JSON"
    return result

def main():
    rows = [request_one(q) for q in QUERIES]
    report = {"schemaVersion": 2, "scope": "same two explicit anonymous YouTube WEB videos-only search POSTs as v1; parse up to all 20 renderer rows; feasibility during concurrent root Rust compilation; not latency qualification or playback acceptance", "requestSetId": "burrata-web-search-2026-10-19-JD-Matthew-Pengus-Victory-Lap-v2", "requestIds": ["JD-Matthew", "Pengus-Victory-Lap"], "dateUtc": datetime.now(timezone.utc).isoformat(), "limits": {"maxRequests": 2, "requestsUsed": 2, "timeoutSecondsPerRequest": TIMEOUT_SECONDS, "maxResponseBytes": MAX_BODY, "maxParsedVideoRenderersPerRequest": 20, "redirects": "disabled", "cookiesOrAuth": "none", "tlsVerification": "urllib default enabled"}, "sourceProfiles": {"searchParams": "pinned yt-dlp YoutubeSearchIE videos-only EgIQAfABAQ==", "clientVersion": "checked-in youtubei.js 18.1.0 WEB profile"}, "requests": rows}
    path = "/tmp/burrata-native-web-search-feasibility-host-v2.json"
    with open(path, "x", encoding="utf-8") as f:
        json.dump(report, f, indent=2, ensure_ascii=False)
        f.write("\n")
    print(json.dumps({"reportPath": path, "requests": [{k: r.get(k) for k in ("query", "httpStatus", "failureCode", "elapsedMs", "parsedItemCount", "items", "responseSha256", "responseBytesRead", "structuralCounts", "transportExceptionType", "dnsErrno")} for r in rows]}, indent=2, ensure_ascii=False))

if __name__ == "__main__": main()
