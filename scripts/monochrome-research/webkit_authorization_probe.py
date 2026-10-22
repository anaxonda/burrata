#!/usr/bin/env python3
"""One-shot normal Turnstile authorization check in an isolated visible WebKitGTK window."""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import shutil
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit


ROOT = Path(__file__).resolve().parents[2]
OUTPUT_PATH = ROOT / ".cache" / "webkit-auth-probe.json"
SITE_URL = "https://monochrome.st/"
API_BASE = "https://music-api.geeked.wtf"
PROBE_SECONDS = 90
API_ERROR_CATEGORIES = {
    "unauthorized", "missing_auth", "invalid_auth", "missing_key", "invalid_key", "missing_token", "invalid_token"
}
SECRET_FIELD = re.compile(r"token|jwt|authorization|cookie|password|secret|signature|(?:^|_)key(?:$|_)|^keys$|keyid|decryptionkey|keyvalue|^kid$|^iv$", re.I)
TEXT_FIELD = re.compile(r"^(title|name|artist|artists|album|version|isrc|provider|source|selected_source|kind|delivery|codec|container|mime_type|quality|quality_requested|scheme|algorithm|method|type|schema_version|status|format|expires_at|expiresAt|expiry|expiration|valid_until|validUntil|id|track_id|recording_id)$", re.I)
REDACTED = "[REDACTED]"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-normal-verification", action="store_true", help="required: show the regular Turnstile widget for a person to complete")
    parser.add_argument("--track", required=True, help="track metadata identifier for one read-only resolve request")
    parser.add_argument("--artist", default="LATIN MAFIA & Fred again..")
    parser.add_argument("--album", default="9 months & 50 hours")
    parser.add_argument("--isrc", default="GBAHS2600508")
    parser.add_argument("--duration", type=int, default=238, help="duration in seconds")
    parser.add_argument("--quality", default="LOSSLESS")
    args = parser.parse_args()
    if not args.run_normal_verification:
        parser.error("pass --run-normal-verification to explicitly enable the visible normal-flow check")
    if not 1 <= len(args.track) <= 128 or re.search(r"https?://|bearer\s|eyJ[a-zA-Z0-9_-]+\.|[a-f0-9]{32,}|[a-zA-Z0-9_-]{60,}", args.track, re.I):
        parser.error("--track must be a short metadata identifier")
    return args


def read_config() -> dict[str, str]:
    raw = sys.stdin.buffer.read(16385)
    if len(raw) > 16384:
        raise ValueError("invalid configuration")
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise ValueError("invalid configuration")
    api_base = value.get("apiBase")
    client_token = value.get("clientToken")
    site_key = value.get("siteKey")
    parsed = urlsplit(api_base if isinstance(api_base, str) else "")
    if (parsed.scheme != "https" or parsed.hostname != "music-api.geeked.wtf" or
            parsed.port not in (None, 443) or parsed.username or parsed.password or
            parsed.path not in ("", "/") or parsed.query or parsed.fragment):
        raise ValueError("invalid configuration")
    if (not isinstance(client_token, str) or not 1 <= len(client_token) <= 1024 or
            any(ord(char) < 32 for char in client_token)):
        raise ValueError("invalid configuration")
    if (not isinstance(site_key, str) or not 10 <= len(site_key) <= 256 or
            not re.fullmatch(r"[A-Za-z0-9_-]+", site_key)):
        raise ValueError("invalid configuration")
    return {"api_base": API_BASE, "client_token": client_token, "site_key": site_key}


def safe_string(value: object) -> str | None:
    if not isinstance(value, str):
        return None
    value = value[:256]
    if re.search(r"https?://|bearer\s|eyJ[a-zA-Z0-9_-]+\.|[a-f0-9]{32,}|[a-zA-Z0-9_-]{60,}", value, re.I):
        return None
    return re.sub(r"[^a-zA-Z0-9 _.,:()\[\]{}+!?&/'-]", " ", value).strip()[:256] or None


def sanitize(value: object, field: str = "", depth: int = 0, key_material: bool = False, budget: list[int] | None = None) -> object:
    if budget is None:
        budget = [0]
    budget[0] += 1
    if budget[0] > 2000:
        return "[NODE LIMIT]"
    if depth > 10:
        return "[DEPTH LIMIT]"
    sensitive = key_material or bool(SECRET_FIELD.search(field))
    if value is None:
        return None
    if isinstance(value, list):
        return [sanitize(item, field, depth + 1, sensitive, budget) for item in value[:40]]
    if isinstance(value, dict):
        result = {}
        for key, child in list(value.items())[:80]:
            safe_key = key if isinstance(key, str) and re.fullmatch(r"[a-zA-Z_][a-zA-Z0-9_-]{0,63}", key) else "[REDACTED FIELD]"
            result[safe_key] = sanitize(child, key if isinstance(key, str) else "", depth + 1, sensitive, budget)
        return result
    if isinstance(value, str) and field.lower() == "encoding":
        return value.lower() if value.lower() in {"hex", "base64", "base64url", "utf-8"} else REDACTED
    if isinstance(value, str) and field.lower() == "iv_source":
        return value.lower() if value.lower() in {"explicit", "derived", "default", "manifest", "segment", "track_id", "key_id", "response", "metadata", "unknown"} else REDACTED
    if sensitive:
        return REDACTED
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return value if math.isfinite(value) else None
    if not isinstance(value, str):
        return REDACTED
    if not TEXT_FIELD.fullmatch(field):
        return REDACTED
    return safe_string(value) or REDACTED


def safe_message(message: object, report: dict[str, object]) -> None:
    if not isinstance(message, dict):
        return
    stage = message.get("stage")
    if stage == "widget":
        report["widget_state"] = message.get("state") if message.get("state") in {"ready", "failed", "expired", "error"} else "unknown"
        category = message.get("error_category")
        report["widget_error_category"] = category if category in {
            "script_load_error", "turnstile_unavailable", "render_exception", "turnstile_error_callback"
        } else None
        error_code = message.get("error_code")
        report["turnstile_error_code"] = error_code if isinstance(error_code, str) and re.fullmatch(r"\d{6}", error_code) else None
    elif stage == "turnstile":
        report["turnstile_token_present"] = bool(message.get("token_present"))
    elif stage == "auth":
        status = message.get("status")
        report["auth_status"] = status if isinstance(status, int) and 100 <= status <= 599 else None
        report["auth_token_present"] = bool(message.get("token_present"))
        exp = message.get("expiration")
        report["auth_expiration"] = exp if isinstance(exp, (int, float)) and not isinstance(exp, bool) and math.isfinite(exp) else None
        category = message.get("error_category")
        report["auth_error_category"] = category if category in API_ERROR_CATEGORIES else None
        report["auth_response"] = sanitize(message.get("response"))
    elif stage == "resolve":
        status = message.get("status")
        report["resolve_status"] = status if isinstance(status, int) and 100 <= status <= 599 else None
        category = message.get("error_category")
        report["resolve_error_category"] = category if category in API_ERROR_CATEGORIES else None
        report["resolve_response"] = sanitize(message.get("response"))
        report["resolve_response_is_json"] = bool(message.get("is_json"))
    elif stage == "complete":
        report["complete"] = True


def main() -> int:
    args = parse_args()
    try:
        config = read_config()
    except Exception:
        print("Expected JSON stdin configuration with apiBase, clientToken, and siteKey", file=sys.stderr)
        return 2

    try:
        import gi
        gi.require_version("Gtk", "3.0")
        gi.require_version("WebKit2", "4.1")
        from gi.repository import GLib, Gtk, WebKit2
    except Exception:
        print("WebKitGTK 4.1 with GTK 3 Python GI is unavailable", file=sys.stderr)
        return 2

    profile_root = Path(tempfile.mkdtemp(prefix="burrata-webkit-auth-probe-"))
    os.chmod(profile_root, 0o700)
    data_dir, cache_dir = profile_root / "data", profile_root / "cache"
    data_dir.mkdir(mode=0o700)
    cache_dir.mkdir(mode=0o700)
    report: dict[str, object] = {
        "schema_version": 1,
        "probe": "normal_turnstile_authorization",
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "site_origin": "https://monochrome.st",
        "api_host": "music-api.geeked.wtf",
        "widget_state": "waiting",
        "widget_error_category": None,
        "turnstile_error_code": None,
        "turnstile_token_present": False,
        "auth_status": None,
        "auth_token_present": False,
        "auth_expiration": None,
        "auth_error_category": None,
        "auth_response": None,
        "resolve_status": None,
        "resolve_error_category": None,
        "resolve_response_is_json": None,
        "resolve_response": None,
        "complete": False,
    }

    try:
        Gtk.init(None)
        manager = WebKit2.WebsiteDataManager(base_data_directory=str(data_dir), base_cache_directory=str(cache_dir))
        context = WebKit2.WebContext.new_with_website_data_manager(manager)
        view = WebKit2.WebView.new_with_context(context)
        user_content = view.get_user_content_manager()
        user_content.connect("script-message-received::burrataAuthProbe", lambda _manager, result: receive_message(result))
        if not user_content.register_script_message_handler("burrataAuthProbe"):
            raise RuntimeError("message handler unavailable")
        window = Gtk.Window(title="Burrata Monochrome authorization research")
        window.set_default_size(1100, 760)
        window.add(view)
        saved = False
        finished = False
        flow_started = False

        def save_report():
            nonlocal saved
            try:
                OUTPUT_PATH.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                temp = OUTPUT_PATH.with_suffix(".json.tmp")
                fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
                os.chmod(temp, 0o600)
                with os.fdopen(fd, "w", encoding="utf-8") as stream:
                    json.dump(report, stream, ensure_ascii=False, indent=2)
                    stream.write("\n")
                os.replace(temp, OUTPUT_PATH)
                saved = True
            except Exception:
                print("Could not write the fixed authorization probe report", file=sys.stderr)

        def receive_message(result):
            try:
                raw = result.get_js_value().to_string()
                if len(raw) > 512 * 1024:
                    return
                safe_message(json.loads(raw), report)
                save_report()
            except Exception:
                pass

        params = {
            "track": args.track,
            "artist": args.artist,
            "album": args.album,
            "isrc": args.isrc,
            "duration": str(max(1, min(args.duration, 3600))),
            "intent": "stream",
            "quality": args.quality,
        }
        if len(args.artist) > 128 or len(args.album) > 128 or len(args.isrc) > 32 or len(args.quality) > 32:
            raise ValueError("metadata exceeds limits")
        config_json = json.dumps({"apiBase": config["api_base"], "clientToken": config["client_token"], "siteKey": config["site_key"], "params": params})
        script = r"""(() => {
          if (location.origin !== 'https://monochrome.st' || window.__burrataAuthProbeStarted) return;
          window.__burrataAuthProbeStarted = true;
          const config = __CONFIG__;
          const channel = window.webkit?.messageHandlers?.burrataAuthProbe;
          const post = value => { try { channel?.postMessage(JSON.stringify(value)); } catch {} };
          const cleanString = value => {
            if (typeof value !== 'string') return '[REDACTED]';
            value = value.slice(0, 256);
            if (/https?:|bearer\s|eyJ[a-zA-Z0-9_-]+\.|[a-f0-9]{32,}|[a-zA-Z0-9_-]{60,}/i.test(value)) return '[REDACTED]';
            return value;
          };
          const secret = /token|jwt|authorization|cookie|password|secret|signature|(?:^|_)key(?:$|_)|^keys$|keyid|decryptionkey|keyvalue|^kid$|^iv$/i;
          const textFields = /^(title|name|artist|artists|album|version|isrc|provider|source|selected_source|kind|delivery|codec|container|mime_type|quality|quality_requested|scheme|algorithm|method|type|schema_version|status|format|expires_at|expiresAt|expiry|expiration|valid_until|validUntil|id|track_id|recording_id)$/i;
          function sanitize(value, field = '', depth = 0, inheritedSecret = false, budget = {nodes: 0}) {
            if (++budget.nodes > 2000) return '[NODE LIMIT]';
            if (depth > 10) return '[DEPTH LIMIT]';
            const sensitive = inheritedSecret || secret.test(field);
            if (value === null) return null;
            if (Array.isArray(value)) return value.slice(0, 40).map(item => sanitize(item, field, depth + 1, sensitive, budget));
            if (typeof value === 'object') {
              const out = {};
              for (const [key, child] of Object.entries(value).slice(0, 80)) {
                const safeKey = /^[a-zA-Z_][a-zA-Z0-9_-]{0,63}$/.test(key) ? key : '[REDACTED FIELD]';
                out[safeKey] = sanitize(child, key, depth + 1, sensitive, budget);
              }
              return out;
            }
            if (field.toLowerCase() === 'encoding' && typeof value === 'string') return /^(hex|base64|base64url|utf-8)$/i.test(value) ? value.toLowerCase() : '[REDACTED]';
            if (field.toLowerCase() === 'iv_source' && typeof value === 'string') return /^(explicit|derived|default|manifest|segment|track_id|key_id|response|metadata|unknown)$/i.test(value) ? value.toLowerCase() : '[REDACTED]';
            if (sensitive) return '[REDACTED]';
            if (typeof value === 'number') return Number.isFinite(value) ? value : null;
            if (typeof value === 'boolean') return value;
            if (typeof value !== 'string' || !textFields.test(field)) return '[REDACTED]';
            return cleanString(value);
          }
          function errorCategory(body) {
            const parts = [];
            for (const key of ['error', 'message', 'detail', 'code']) {
              const value = body && body[key];
              if (typeof value === 'string') parts.push(value);
              else if (value && typeof value === 'object') for (const sub of ['error', 'message', 'detail', 'code']) if (typeof value[sub] === 'string') parts.push(value[sub]);
            }
            const text = parts.join(' ').slice(0, 3000).toLowerCase();
            if (/unauthorized|\b401\b/.test(text)) return 'unauthorized';
            if (/missing.{0,24}(authorization|auth)|(?:authorization|auth).{0,24}missing/.test(text)) return 'missing_auth';
            if (/invalid.{0,24}(authorization|auth|credential)|(?:authorization|auth|credential).{0,24}invalid/.test(text)) return 'invalid_auth';
            if (/missing.{0,24}(api\s+)?key|(?:api\s+)?key.{0,24}missing/.test(text)) return 'missing_key';
            if (/invalid.{0,24}(api\s+)?key|(?:api\s+)?key.{0,24}invalid/.test(text)) return 'invalid_key';
            if (/missing.{0,24}token|token.{0,24}missing/.test(text)) return 'missing_token';
            if (/invalid.{0,24}token|token.{0,24}invalid/.test(text)) return 'invalid_token';
            return null;
          }
          function findJwt(value, depth = 0) {
            if (!value || typeof value !== 'object' || depth > 4) return null;
            for (const key of ['token', 'jwt', 'access_token', 'accessToken', 'turnstile_jwt', 'turnstileJwt']) {
              if (typeof value[key] === 'string' && value[key].length > 20 && value[key].length < 8192) return value[key];
            }
            for (const key of ['data', 'result', 'auth']) { const found = findJwt(value[key], depth + 1); if (found) return found; }
            return null;
          }
          function jwtExpiration(value) {
            try {
              const normalized = value.split('.')[1].replace(/-/g, '+').replace(/_/g, '/');
              const payload = normalized + '='.repeat((4 - (normalized.length % 4)) % 4);
              const decoded = JSON.parse(atob(payload));
              return Number.isFinite(decoded.exp) && decoded.exp > 0 ? decoded.exp : null;
            } catch { return null; }
          }
          let requested = false;
          async function timeoutFetch(url, options, milliseconds) {
            const controller = new AbortController();
            const timer = setTimeout(() => controller.abort(), milliseconds);
            try { return await fetch(url, {...options, signal: controller.signal}); }
            finally { clearTimeout(timer); }
          }
          async function boundedJson(response) {
            try {
              if (!/json/i.test(response.headers.get('content-type') || '')) return null;
              const declared = Number(response.headers.get('content-length'));
              if (declared > 256 * 1024) return null;
              const reader = response.clone().body?.getReader();
              if (!reader) return null;
              const chunks = [];
              let size = 0;
              while (true) {
                const {value, done} = await reader.read();
                if (done) break;
                size += value.byteLength;
                if (size > 256 * 1024) { await reader.cancel(); return null; }
                chunks.push(value);
              }
              const bytes = new Uint8Array(size);
              let offset = 0;
              for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
              return JSON.parse(new TextDecoder().decode(bytes));
            } catch { return null; }
          }
          async function afterChallenge(turnstileToken) {
            if (requested || typeof turnstileToken !== 'string' || !turnstileToken) return;
            requested = true;
            post({stage: 'turnstile', token_present: true});
            let authResponse, authBody = null;
            try {
              authResponse = await timeoutFetch(config.apiBase + '/api/auth/turnstile', {
                method: 'POST',
                headers: {Authorization: 'Bearer ' + config.clientToken, 'Content-Type': 'application/json', Accept: 'application/json'},
                body: JSON.stringify({turnstile_token: turnstileToken}),
                credentials: 'omit',
              }, 15000);
              authBody = await boundedJson(authResponse);
            } catch {
              post({stage: 'auth', status: null, token_present: false, expiration: null, error_category: null, response: null});
              post({stage: 'complete'});
              return;
            }
            const issuedJwt = findJwt(authBody);
            post({stage: 'auth', status: authResponse.status, token_present: !!issuedJwt,
              expiration: issuedJwt ? jwtExpiration(issuedJwt) : null,
              error_category: errorCategory(authBody), response: sanitize(authBody)});
            if (!issuedJwt || !authResponse.ok) { post({stage: 'complete'}); return; }
            const params = new URLSearchParams(config.params);
            let resolveResponse, resolveBody = null, isJson = false;
            try {
              resolveResponse = await timeoutFetch(config.apiBase + '/api/v2/track/?' + params.toString(), {
                method: 'GET', headers: {Authorization: 'Bearer ' + config.clientToken, 'X-Turnstile-JWT': issuedJwt, Accept: 'application/json'},
                cache: 'no-store', credentials: 'omit',
              }, 20000);
              isJson = /json/i.test(resolveResponse.headers.get('content-type') || '');
              if (isJson) resolveBody = await boundedJson(resolveResponse);
            } catch {
              post({stage: 'resolve', status: null, error_category: null, is_json: false, response: null});
              post({stage: 'complete'});
              return;
            }
            post({stage: 'resolve', status: resolveResponse.status, error_category: errorCategory(resolveBody),
              is_json: isJson, response: isJson ? sanitize(resolveBody) : null});
            post({stage: 'complete'});
          }
          function installWidget() {
            const holder = document.createElement('div');
            holder.id = 'burrata-normal-turnstile-widget';
            holder.style.cssText = 'position:fixed;right:16px;bottom:84px;z-index:2147483647;background:white;color:#111;padding:16px;border:1px solid #aaa;border-radius:8px;box-shadow:0 2px 18px #0005;font:14px sans-serif;max-width:380px';
            holder.textContent = 'Complete the normal verification in this window to continue the one-shot authorization check.';
            const widget = document.createElement('div');
            widget.style.marginTop = '12px';
            holder.appendChild(widget);
            (document.body || document.documentElement).appendChild(holder);
            const render = () => {
              if (!widget.isConnected) return;
              if (!window.turnstile) { post({stage: 'widget', state: 'error', error_category: 'turnstile_unavailable'}); return; }
              try {
                const widgetId = window.turnstile.render(widget, {sitekey: config.siteKey, action: 'auth', execution: 'execute', appearance: 'interaction-only', theme: 'auto', retry: 'never',
                  'before-interactive-callback': () => { holder.style.display = 'block'; }, callback: afterChallenge,
                  'expired-callback': () => post({stage: 'widget', state: 'expired'}),
                  'error-callback': code => post({stage: 'widget', state: 'error', error_category: 'turnstile_error_callback', error_code: /^\d{6}$/.test(String(code)) ? String(code) : null})});
                window.turnstile.execute(widgetId);
                post({stage: 'widget', state: 'ready'});
              } catch { post({stage: 'widget', state: 'error', error_category: 'render_exception'}); }
            };
            if (window.turnstile) { render(); return; }
            const script = document.createElement('script');
            script.src = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit';
            script.async = true;
            script.defer = true;
            script.onload = render;
            script.onerror = () => post({stage: 'widget', state: 'failed', error_category: 'script_load_error'});
            document.head.appendChild(script);
          }
          installWidget();
        })();""".replace("__CONFIG__", config_json)

        def start_flow():
            nonlocal flow_started
            if flow_started:
                return
            flow_started = True
            try:
                current = view.get_uri() or ""
                if urlsplit(current).scheme != "https" or urlsplit(current).hostname != "monochrome.st":
                    report["widget_state"] = "site_origin_unavailable"
                    save_report()
                    return
                view.run_javascript(script, None, lambda _view, result, _data: None, None)
            except Exception:
                report["widget_state"] = "injection_failed"
                save_report()

        def on_load_changed(_view, event):
            if event == WebKit2.LoadEvent.FINISHED:
                start_flow()

        def save_and_exit(*_args):
            nonlocal finished
            if finished:
                return False
            finished = True
            save_report()
            try:
                user_content.unregister_script_message_handler("burrataAuthProbe")
                window.destroy()
            except Exception:
                pass
            Gtk.main_quit()
            return False

        view.connect("load-changed", on_load_changed)
        window.connect("destroy", save_and_exit)
        window.show_all()
        view.load_uri(SITE_URL)
        GLib.timeout_add_seconds(PROBE_SECONDS, save_and_exit)
        Gtk.main()
        if not saved:
            save_report()
    except Exception:
        print("WebKit authorization probe could not start", file=sys.stderr)
        return 2
    finally:
        try:
            shutil.rmtree(profile_root)
        except OSError:
            pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
