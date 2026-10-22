#!/usr/bin/env python3
"""Visible, isolated WebKitGTK connection probe for Monochrome research."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit


ROOT = Path(__file__).resolve().parents[2]
OUTPUT_DIR = ROOT / ".cache"
OUTPUT_PATH = OUTPUT_DIR / "webkit-probe.json"
MAX_SECONDS = 90
VISIBLE_SECONDS = 60

PAGE_CHECK = r"""(() => {
  const bodyText = (document.body?.innerText || '').slice(0, 512 * 1024);
  const text = bodyText.slice(0, 20000).toLowerCase();
  const scripts = Array.from(document.scripts, item => item.src || '');
  const frames = Array.from(document.querySelectorAll('iframe'), item => item.src || '');
  const safeText = value => {
    if (typeof value !== 'string') return null;
    value = value.slice(0, 200);
    if (/https?:\/\/|bearer\s|(?:access[_ -]?token|authorization|password|secret|signature)\s*[:=]|eyJ[a-zA-Z0-9_-]+\.|[a-f0-9]{32,}|[a-zA-Z0-9_-]{60,}/i.test(value)) return null;
    value = value.replace(/[^a-zA-Z0-9 _.,:()\[\]{}+!?\u2019&\/-]/g, ' ').replace(/\s+/g, ' ').trim();
    return value.slice(0, 160) || null;
  };
  let jsonBody = null;
  try { jsonBody = JSON.parse(bodyText); } catch {}
  let apiErrorCategory = null;
  if (jsonBody && typeof jsonBody === 'object' && !Array.isArray(jsonBody)) {
    const errorText = ['error', 'message', 'detail', 'code'].map(key => jsonBody[key]).filter(value => typeof value === 'string').join(' ').slice(0, 3000).toLowerCase();
    if (/unauthorized|\b401\b/.test(errorText)) apiErrorCategory = 'unauthorized';
    else if (/missing.{0,24}(authorization|auth)|(?:authorization|auth).{0,24}missing/.test(errorText)) apiErrorCategory = 'missing_auth';
    else if (/invalid.{0,24}(authorization|auth|credential)|(?:authorization|auth|credential).{0,24}invalid/.test(errorText)) apiErrorCategory = 'invalid_auth';
    else if (/missing.{0,24}(api\s+)?key|(?:api\s+)?key.{0,24}missing/.test(errorText)) apiErrorCategory = 'missing_key';
    else if (/invalid.{0,24}(api\s+)?key|(?:api\s+)?key.{0,24}invalid/.test(errorText)) apiErrorCategory = 'invalid_key';
    else if (/missing.{0,24}token|token.{0,24}missing/.test(errorText)) apiErrorCategory = 'missing_token';
    else if (/invalid.{0,24}token|token.{0,24}invalid/.test(errorText)) apiErrorCategory = 'invalid_token';
  }
  let catalogSummary = null;
  if (jsonBody && Array.isArray(jsonBody.tracks)) {
    const tracks = jsonBody.tracks.slice(0, 10).map(track => {
      if (!track || typeof track !== 'object' || Array.isArray(track)) return null;
      const id = typeof track.id === 'number' && Number.isFinite(track.id) ? track.id : safeText(track.id);
      const title = safeText(track.title);
      const isrc = safeText(track.isrc);
      const durationValue = track.duration ?? track.duration_seconds ?? track.duration_ms;
      const duration = typeof durationValue === 'number' && Number.isFinite(durationValue) ? durationValue : safeText(durationValue);
      const artistValue = track.artists ?? track.artist;
      const artists = Array.isArray(artistValue) ? artistValue : (artistValue == null ? [] : [artistValue]);
      const artist = artists.map(value => safeText(typeof value === 'string' ? value : value?.name)).filter(Boolean).join(', ').slice(0, 512) || null;
      return { id, title, isrc, duration, artist };
    }).filter(Boolean);
    catalogSummary = { count: jsonBody.tracks.length, tracks };
  }
  return JSON.stringify({
    origin: location.origin,
    title: safeText(document.title || ''),
    hasTurnstileScript: scripts.some(src => /turnstile|captcha/i.test(src)),
    hasChallengeFrame: frames.some(src => /turnstile|challenge|captcha|cloudflare/i.test(src)),
    apiErrorCategory,
    catalogSummary,
    indicators: {
      blocked: /access denied|request blocked|temporarily blocked/.test(text),
      originDnsError: /origin dns error/.test(text),
      signature: /signature/.test(text),
      captcha: /captcha|verify you are human|checking your browser/.test(text),
      login: /sign in|log in|login required/.test(text)
    }
  });
})()"""


def safe_title(value: object) -> str | None:
    if not isinstance(value, str):
        return None
    value = value[:200]
    if re.search(r"https?://|bearer\s|(?:access[_ -]?token|authorization|password|secret|signature)\s*[:=]|eyJ[a-zA-Z0-9_-]+\.|[a-f0-9]{32,}|[a-zA-Z0-9_-]{60,}", value, re.I):
        return None
    value = re.sub(r"[^a-zA-Z0-9 _.,:()\[\]{}+!?'’&/-]", " ", value)
    value = re.sub(r"\s+", " ", value).strip()
    return value[:160] or None


API_ERROR_CATEGORIES = {
    "unauthorized", "missing_auth", "invalid_auth", "missing_key", "invalid_key", "missing_token", "invalid_token"
}


def safe_catalog(value: object) -> dict[str, object] | None:
    if not isinstance(value, dict) or not isinstance(value.get("tracks"), list):
        return None
    count = value.get("count")
    if not isinstance(count, int) or isinstance(count, bool) or count < 0:
        return None
    clean_tracks = []
    for track in value["tracks"][:10]:
        if not isinstance(track, dict):
            continue
        track_id = track.get("id")
        if isinstance(track_id, str):
            track_id = safe_title(track_id)
        elif not isinstance(track_id, int) or isinstance(track_id, bool):
            track_id = None
        duration = track.get("duration")
        if isinstance(duration, str):
            duration = safe_title(duration)
        elif not isinstance(duration, (int, float)) or isinstance(duration, bool):
            duration = None
        elif not isinstance(duration, int) and (duration != duration or abs(duration) == float("inf")):
            duration = None
        clean_tracks.append({
            "id": track_id,
            "title": safe_title(track.get("title")),
            "isrc": safe_title(track.get("isrc")),
            "duration": duration,
            "artist": safe_title(track.get("artist")),
        })
    return {"count": min(count, 1_000_000), "tracks": clean_tracks}


def safe_api_resource(uri: str) -> dict[str, str] | None:
    try:
        parsed = urlsplit(uri)
        if parsed.scheme != "https" or not parsed.hostname:
            return None
        path = parsed.path
        if not re.search(r"api|auth|turnstile|resolve|playback", path, re.I):
            return None
        parts = []
        for segment in path.split("/"):
            if re.fullmatch(r"[A-Za-z0-9_-]{24,}", segment) or re.search(r"eyJ|%3[fFdD]|%26", segment, re.I):
                parts.append("[redacted]")
            else:
                parts.append(re.sub(r"[a-fA-F0-9]{32,}", "[redacted]", segment))
        return {"host": parsed.hostname[:253], "path": "/".join(parts)[:512]}
    except Exception:
        return None


def visible_seconds(value: str) -> int:
    try:
        seconds = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be an integer from 5 to 90") from error
    if not 5 <= seconds <= MAX_SECONDS:
        raise argparse.ArgumentTypeError("must be an integer from 5 to 90")
    return seconds


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="https://monochrome.samidy.com/", help="one HTTPS page to open")
    parser.add_argument("--output", default=str(OUTPUT_PATH), help="output file; basename must be webkit-probe.json in workspace .cache")
    parser.add_argument("--seconds", type=visible_seconds, default=VISIBLE_SECONDS, help="keep the visible window open this many seconds (5-90; default 60)")
    args = parser.parse_args()
    try:
        parsed = urlsplit(args.url)
        if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
            parser.error("--url must be one HTTPS URL without embedded credentials")
        if len(args.url) > 2048:
            parser.error("--url is too long")
        output = Path(args.output).expanduser()
        if not output.is_absolute():
            output = ROOT / output
        output = output.resolve()
        if output.name != OUTPUT_PATH.name or output.parent != OUTPUT_DIR.resolve():
            parser.error("--output must be workspace .cache/webkit-probe.json")
        args.output_path = output
    except ValueError:
        parser.error("--url is invalid")
    return args


def main() -> int:
    args = parse_args()
    try:
        import gi

        gi.require_version("Gtk", "3.0")
        gi.require_version("WebKit2", "4.1")
        from gi.repository import GLib, Gtk, WebKit2
    except Exception:
        print("WebKitGTK 4.1 with GTK 3 Python GI is unavailable", file=sys.stderr)
        return 2

    profile_root = Path(tempfile.mkdtemp(prefix="burrata-webkit-probe-"))
    os.chmod(profile_root, 0o700)
    data_dir = profile_root / "data"
    cache_dir = profile_root / "cache"
    data_dir.mkdir(mode=0o700)
    cache_dir.mkdir(mode=0o700)

    report: dict[str, object] = {
        "schema_version": 1,
        "probe": "webkitgtk_connection",
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "url_origin": f"https://{urlsplit(args.url).hostname}",
        "webkit_version": {
            "major": WebKit2.get_major_version(),
            "minor": WebKit2.get_minor_version(),
            "micro": WebKit2.get_micro_version(),
        },
        "load_events": [],
        "failures": [],
        "api_resources": [],
        "main_response_status": None,
        "page": None,
    }

    try:
        Gtk.init(None)
        manager = WebKit2.WebsiteDataManager(
            base_data_directory=str(data_dir), base_cache_directory=str(cache_dir)
        )
        context = WebKit2.WebContext.new_with_website_data_manager(manager)
        view = WebKit2.WebView.new_with_context(context)
        window = Gtk.Window(title="Burrata Monochrome connection test")
        window.set_default_size(1100, 760)
        window.add(view)

        def save_report():
            try:
                args.output_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                temp_path = args.output_path.with_suffix(".json.tmp")
                fd = os.open(temp_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
                os.chmod(temp_path, 0o600)
                with os.fdopen(fd, "w", encoding="utf-8") as stream:
                    json.dump(report, stream, ensure_ascii=False, indent=2)
                    stream.write("\n")
                os.replace(temp_path, args.output_path)
            except Exception:
                print("Could not write the fixed probe report", file=sys.stderr)

        def on_load_changed(webview, event):
            event_name = getattr(event, "value_nick", None) or str(event).lower()
            if event_name not in report["load_events"]:
                report["load_events"].append(event_name)
            if event == WebKit2.LoadEvent.FINISHED:
                try:
                    resource = webview.get_main_resource()
                    response = resource.get_response() if resource else None
                    report["main_response_status"] = response.get_status_code() if response else None
                    report["response_api_error"] = bool(response and response.get_status_code() >= 400)
                except Exception:
                    report["main_response_status"] = None
                    report["response_api_error"] = None
                try:
                    webview.evaluate_javascript(PAGE_CHECK, -1, None, None, None, on_page_check, None)
                except Exception:
                    report["page"] = None

        def on_page_check(webview, result, _user_data):
            try:
                value = webview.evaluate_javascript_finish(result)
                raw = value.to_string() if value else ""
                page = json.loads(raw)
                if not isinstance(page, dict):
                    return
                origin = page.get("origin")
                title = safe_title(page.get("title"))
                indicators = page.get("indicators")
                report["page"] = {
                    "origin": origin if isinstance(origin, str) and re.fullmatch(r"https://[a-zA-Z0-9.-]+", origin) else None,
                    "title": title,
                    "has_turnstile_script": bool(page.get("hasTurnstileScript")),
                    "has_challenge_frame": bool(page.get("hasChallengeFrame")),
                    "response_api_error": report.get("response_api_error"),
                    "api_error_category": page.get("apiErrorCategory") if page.get("apiErrorCategory") in API_ERROR_CATEGORIES else None,
                    "catalog_summary": safe_catalog(page.get("catalogSummary")),
                    "indicators": {key: bool(indicators.get(js_key)) for key, js_key in (("blocked", "blocked"), ("origin_dns_error", "originDnsError"), ("signature", "signature"), ("captcha", "captcha"), ("login", "login"))} if isinstance(indicators, dict) else {},
                }
                save_report()
            except Exception:
                report["page"] = None
                save_report()

        def on_load_failed(_webview, _event, _failing_uri, error):
            failures = report["failures"]
            if len(failures) < 20:
                try:
                    domain = GLib.quark_to_string(error.domain) or str(int(error.domain))
                    code = int(error.code)
                except Exception:
                    domain, code = "unknown", None
                failures.append({"domain": domain[:80], "code": code})
            return False

        def on_resource_started(_webview, resource, _request):
            try:
                entry = safe_api_resource(resource.get_uri() or "")
                entries = report["api_resources"]
                if entry and entry not in entries and len(entries) < 40:
                    entries.append(entry)
            except Exception:
                pass

        finished = False

        def finish(*_args):
            nonlocal finished
            if finished:
                return False
            finished = True
            save_report()
            try:
                window.destroy()
            except Exception:
                pass
            Gtk.main_quit()
            return False

        view.connect("load-changed", on_load_changed)
        view.connect("load-failed", on_load_failed)
        view.connect("resource-load-started", on_resource_started)
        window.connect("destroy", finish)
        window.show_all()
        view.load_uri(args.url)
        GLib.timeout_add_seconds(args.seconds, finish)
        GLib.timeout_add_seconds(MAX_SECONDS, finish)
        Gtk.main()
    finally:
        try:
            shutil.rmtree(profile_root)
        except OSError:
            pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
