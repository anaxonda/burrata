#!/usr/bin/env python3
"""Reuse an existing matching AMO-signed XPI, or sign a new extension version."""
from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile


API_ROOT = "https://addons.mozilla.org/api/v5/"
WEB_EXT_VERSION = "8.4.0"
RSA_SIGNATURE_FILES = {
    "meta-inf/manifest.mf",
    "meta-inf/mozilla.sf",
    "meta-inf/mozilla.rsa",
}
COSE_SIGNATURE_FILES = {"meta-inf/cose.manifest", "meta-inf/cose.sig"}
SIGNATURE_FILES = RSA_SIGNATURE_FILES | COSE_SIGNATURE_FILES
TRUSTED_DOWNLOAD_HOSTS = {
    "addons.mozilla.org",
    "addons.cdn.mozilla.net",
    "addons.cdn.mozilla.org",
}
MAX_XPI_BYTES = 200_000_000
IGNORE_FILES = ["tests/**", "README.md", "manifest.chromium.json"]
ROOT = pathlib.Path(__file__).resolve().parents[2]


class SignFirefoxError(RuntimeError):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class TrustedDownloadRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        target = validate_download_url(newurl)
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is not None and target.hostname != "addons.mozilla.org":
            for key in list(redirected.headers):
                if key.lower() == "authorization":
                    redirected.remove_header(key)
            for key in list(redirected.unredirected_hdrs):
                if key.lower() == "authorization":
                    redirected.remove_header(key)
        return redirected


API_OPENER = urllib.request.build_opener(NoRedirect())
DOWNLOAD_OPENER = urllib.request.build_opener(TrustedDownloadRedirect())


def _b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def make_jwt(api_key: str, api_secret: str, now: int | None = None) -> str:
    issued = int(time.time()) if now is None else now
    header = _b64url(b'{"alg":"HS256","typ":"JWT"}')
    claims = _b64url(json.dumps({"iss": api_key, "iat": issued, "exp": issued + 300},
                                separators=(",", ":")).encode("utf-8"))
    signing_input = f"{header}.{claims}".encode("ascii")
    signature = hmac.new(api_secret.encode("utf-8"), signing_input, hashlib.sha256).digest()
    return f"{header}.{claims}.{_b64url(signature)}"


def _open_api(request: urllib.request.Request, timeout: int):
    return API_OPENER.open(request, timeout=timeout)


def _open_download(request: urllib.request.Request, timeout: int):
    return DOWNLOAD_OPENER.open(request, timeout=timeout)


def api_get_version(guid: str, version: str, token: str, *, open_api=_open_api) -> dict | None:
    safe_guid = urllib.parse.quote(guid, safe="")
    safe_version = urllib.parse.quote("v" + version, safe="")
    url = f"{API_ROOT}addons/addon/{safe_guid}/versions/{safe_version}/"
    headers = {"Authorization": f"JWT {token}", "Accept": "application/json",
               "User-Agent": f"burrata-release/web-ext-{WEB_EXT_VERSION}"}
    request = urllib.request.Request(url, headers=headers)
    for attempt in range(3):
        try:
            with open_api(request, timeout=30) as response:
                status = response.status
                raw = response.read(2_000_001)
            if status < 200 or status >= 300:
                raise SignFirefoxError(f"AMO version lookup failed (HTTP {status}).")
            if len(raw) > 2_000_000:
                raise SignFirefoxError("AMO version lookup returned an oversized response.")
            try:
                payload = json.loads(raw)
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                raise SignFirefoxError("AMO version lookup returned invalid JSON.") from error
            if not isinstance(payload, dict):
                raise SignFirefoxError("AMO version lookup returned an unexpected response.")
            return payload
        except urllib.error.HTTPError as error:
            error.close()
            if error.code == 404:
                return None
            if error.code in (401, 403):
                raise SignFirefoxError(
                    f"AMO authentication or author authorization failed (HTTP {error.code}); "
                    "check the API credentials and add-on author access."
                ) from None
            if error.code not in (429, 500, 502, 503, 504) or attempt == 2:
                raise SignFirefoxError(f"AMO version lookup failed (HTTP {error.code}).") from None
        except (TimeoutError, urllib.error.URLError, OSError) as error:
            if attempt == 2:
                raise SignFirefoxError("AMO version lookup failed after bounded retries.") from None
        time.sleep(0.5 * (2 ** attempt))
    raise SignFirefoxError("AMO version lookup failed after bounded retries.")


def validate_download_url(url: str) -> urllib.parse.SplitResult:
    parsed = urllib.parse.urlsplit(url)
    try:
        port = parsed.port
    except ValueError:
        raise SignFirefoxError("AMO returned an untrusted signed-file download URL.") from None
    if (parsed.scheme != "https" or parsed.hostname not in TRUSTED_DOWNLOAD_HOSTS
            or port not in (None, 443)
            or parsed.username is not None or parsed.password is not None):
        raise SignFirefoxError("AMO returned an untrusted signed-file download URL.")
    return parsed


def download_signed_file(file_url: str, file_info: dict, destination: pathlib.Path, token: str,
                         *, open_download=_open_download) -> None:
    validate_download_url(file_url)
    headers = {"User-Agent": "burrata-release"}
    if urllib.parse.urlsplit(file_url).hostname == "addons.mozilla.org":
        headers["Authorization"] = f"JWT {token}"
    request = urllib.request.Request(file_url, headers=headers)
    try:
        with open_download(request, timeout=60) as response:
            declared_size = response.headers.get("Content-Length")
            if declared_size and int(declared_size) > MAX_XPI_BYTES:
                raise SignFirefoxError("AMO signed XPI exceeds the allowed download size.")
            data = response.read(MAX_XPI_BYTES + 1)
    except SignFirefoxError:
        raise
    except (OSError, urllib.error.URLError, TimeoutError) as error:
        raise SignFirefoxError("AMO signed XPI download failed.") from None
    if len(data) > MAX_XPI_BYTES:
        raise SignFirefoxError("AMO signed XPI exceeds the allowed download size.")
    expected_hash = file_info.get("hash")
    if not isinstance(expected_hash, str):
        raise SignFirefoxError("AMO signed file record is missing its published hash.")
    match = re.fullmatch(r"sha256:([0-9a-fA-F]{64})", expected_hash)
    if not match or not hmac.compare_digest(hashlib.sha256(data).hexdigest(), match.group(1).lower()):
        raise SignFirefoxError("Downloaded AMO signed XPI did not match its published SHA-256.")
    destination.write_bytes(data)


def archive_payload(path: pathlib.Path) -> dict[str, bytes]:
    try:
        with zipfile.ZipFile(path) as archive:
            result: dict[str, bytes] = {}
            for info in archive.infolist():
                if info.is_dir():
                    continue
                normalized = info.filename.replace("\\", "/")
                if normalized.lower() in SIGNATURE_FILES:
                    continue
                if normalized in result:
                    raise SignFirefoxError(f"Extension archive contains a duplicate path: {normalized}")
                result[normalized] = archive.read(info)
            return result
    except (OSError, zipfile.BadZipFile, RuntimeError) as error:
        if isinstance(error, SignFirefoxError):
            raise
        raise SignFirefoxError(f"Could not inspect extension archive {path.name}.") from None


def assert_signed_payload_matches(source_zip: pathlib.Path, signed_xpi: pathlib.Path) -> None:
    try:
        with zipfile.ZipFile(signed_xpi) as archive:
            names = {name.lower() for name in archive.namelist() if not name.endswith("/")}
    except (OSError, zipfile.BadZipFile):
        raise SignFirefoxError("Existing AMO file is not a readable XPI archive.") from None
    missing = RSA_SIGNATURE_FILES - names
    if missing:
        raise SignFirefoxError("Existing AMO file is missing required Mozilla signature files.")
    try:
        with zipfile.ZipFile(source_zip) as archive:
            source_names = {name.lower() for name in archive.namelist() if not name.endswith("/")}
    except (OSError, zipfile.BadZipFile):
        raise SignFirefoxError("Could not inspect the newly built extension payload.") from None
    if SIGNATURE_FILES & source_names:
        raise SignFirefoxError("Source extension unexpectedly contains Mozilla signature files.")
    if archive_payload(source_zip) != archive_payload(signed_xpi):
        raise SignFirefoxError(
            "AMO already has this extension version with a different payload. "
            "Bump extension/manifest.json version before publishing changed extension code."
        )


def _web_ext_command(*args: str) -> list[str]:
    return ["npx", "--yes", f"web-ext@{WEB_EXT_VERSION}", *args]


def run_web_ext_build(source_dir: pathlib.Path, output_dir: pathlib.Path) -> pathlib.Path:
    command = _web_ext_command("build", "--source-dir", str(source_dir),
                               "--artifacts-dir", str(output_dir), "--ignore-files", *IGNORE_FILES)
    try:
        result = subprocess.run(command, cwd=ROOT, capture_output=True, timeout=300, check=False)
    except (OSError, subprocess.TimeoutExpired):
        raise SignFirefoxError(f"Pinned web-ext {WEB_EXT_VERSION} build failed or timed out.") from None
    if result.returncode != 0:
        raise SignFirefoxError(f"Pinned web-ext {WEB_EXT_VERSION} build failed (exit {result.returncode}).")
    archives = sorted(output_dir.glob("*.zip"))
    if len(archives) != 1:
        raise SignFirefoxError("Pinned web-ext did not produce exactly one comparison ZIP.")
    return archives[0]


def safe_process_diagnostic(stdout: bytes | str | None, stderr: bytes | str | None,
                            sensitive_values: list[str]) -> str:
    chunks = []
    for value in (stdout, stderr):
        if isinstance(value, bytes):
            chunks.append(value.decode("utf-8", errors="replace"))
        elif isinstance(value, str):
            chunks.append(value)
    text = "\n".join(chunks)
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
    for secret in sorted((item for item in sensitive_values if item), key=len, reverse=True):
        text = text.replace(secret, "[REDACTED]")
    text = re.sub(r"(?i)\bJWT\s+[A-Za-z0-9._~-]+", "JWT [REDACTED]", text)
    text = re.sub(r"(?i)(authorization\s*[:=]\s*)(?:JWT\s+)?[^\s,;]+", r"\1[REDACTED]", text)
    text = re.sub(r"(?i)(web_ext_api_(?:key|secret)\s*[:=]\s*)[^\s,;]+", r"\1[REDACTED]", text)
    safe_lines = []
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if (not line or line.startswith(("{", "[", "\""))
                or re.match(r"[\w.-]+\s*:\s*[\[{\"]", line)
                or re.match(r"(?i)(server response|response body|response data)\s*:", line)):
            continue
        safe_lines.append(line[:240])
    return " | ".join(safe_lines[-4:])[:800]


def run_web_ext_sign(source_dir: pathlib.Path, artifacts_dir: pathlib.Path,
                     api_key: str, api_secret: str, token: str) -> pathlib.Path:
    before = {p.resolve() for p in artifacts_dir.glob("*.xpi")}
    env = os.environ.copy()
    env["WEB_EXT_API_KEY"] = api_key
    env["WEB_EXT_API_SECRET"] = api_secret
    command = _web_ext_command("sign", "--source-dir", str(source_dir),
                               "--artifacts-dir", str(artifacts_dir), "--channel", "unlisted",
                               "--ignore-files", *IGNORE_FILES, "--timeout", "900000")
    try:
        result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True,
                                timeout=1800, check=False)
    except subprocess.TimeoutExpired as error:
        detail = safe_process_diagnostic(error.stdout, error.stderr, [api_key, api_secret, token])
        suffix = f" Diagnostics: {detail}" if detail else ""
        raise SignFirefoxError(f"Pinned web-ext {WEB_EXT_VERSION} signing timed out.{suffix}") from None
    except OSError:
        raise SignFirefoxError(f"Pinned web-ext {WEB_EXT_VERSION} signing could not start.") from None
    if result.returncode != 0:
        detail = safe_process_diagnostic(result.stdout, result.stderr, [api_key, api_secret, token])
        suffix = f" Diagnostics: {detail}" if detail else ""
        raise SignFirefoxError(
            f"Pinned web-ext {WEB_EXT_VERSION} signing failed (exit {result.returncode}).{suffix}"
        )
    produced = {p.resolve() for p in artifacts_dir.glob("*.xpi")} - before
    if len(produced) != 1:
        raise SignFirefoxError("AMO signing did not produce exactly one signed Firefox XPI.")
    return next(iter(produced))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-dir", type=pathlib.Path, default=ROOT / "extension")
    parser.add_argument("--artifacts-dir", type=pathlib.Path, default=ROOT / "release-artifacts")
    args = parser.parse_args()
    source_dir = args.source_dir.resolve()
    artifacts_dir = args.artifacts_dir.resolve()
    try:
        api_key = os.environ.get("WEB_EXT_API_KEY", "")
        api_secret = os.environ.get("WEB_EXT_API_SECRET", "")
        if not api_key or not api_secret:
            raise SignFirefoxError("AMO API credentials are required for extension signing or version lookup.")
        manifest = json.loads((source_dir / "manifest.json").read_text(encoding="utf-8"))
        guid = manifest.get("browser_specific_settings", {}).get("gecko", {}).get("id")
        version = manifest.get("version")
        if not isinstance(guid, str) or not guid or not isinstance(version, str) or not version:
            raise SignFirefoxError("Firefox manifest must declare a Gecko ID and extension version.")
        artifacts_dir.mkdir(parents=True, exist_ok=True)
        token = make_jwt(api_key, api_secret)
        if os.environ.get("GITHUB_ACTIONS") == "true":
            for sensitive in (api_key, api_secret, token):
                if "\n" in sensitive or "\r" in sensitive:
                    raise SignFirefoxError("AMO credentials must not contain line breaks.")
                print(f"::add-mask::{sensitive}", flush=True)
        existing = api_get_version(guid, version, token)
        if existing is None:
            with tempfile.TemporaryDirectory(prefix="burrata-firefox-expected-") as temp_name:
                expected_zip = run_web_ext_build(source_dir, pathlib.Path(temp_name))
                signed_xpi = run_web_ext_sign(source_dir, artifacts_dir, api_key, api_secret, token)
                assert_signed_payload_matches(expected_zip, signed_xpi)
            print(f"AMO had no Firefox version {version}; pinned web-ext signing completed.")
            return 0
        if existing.get("version") != version:
            raise SignFirefoxError("AMO version lookup returned a different extension version.")
        file_info = existing.get("file")
        if not isinstance(file_info, dict):
            raise SignFirefoxError("The AMO version exists but has no signed file record; resolve it in AMO before retrying.")
        if file_info.get("status") != "public":
            raise SignFirefoxError(
                f"AMO version {version} exists but its file is not signed and available "
                f"(status: {file_info.get('status', 'unknown')}); resolve that submission before retrying."
            )
        file_url = file_info.get("url")
        if not isinstance(file_url, str):
            raise SignFirefoxError("AMO version exists but has no downloadable signed file URL.")
        with tempfile.TemporaryDirectory(prefix="burrata-firefox-compare-") as temp_name:
            temp = pathlib.Path(temp_name)
            existing_xpi = temp / "amo-existing.xpi"
            download_signed_file(file_url, file_info, existing_xpi, token)
            expected_zip = run_web_ext_build(source_dir, temp)
            assert_signed_payload_matches(expected_zip, existing_xpi)
            signed_bytes = existing_xpi.read_bytes()
        destination = artifacts_dir / f"burrata-firefox-{version}.xpi"
        if destination.exists():
            raise SignFirefoxError(f"Refusing to overwrite existing Firefox artifact: {destination.name}")
        destination.write_bytes(signed_bytes)
        print(f"Reused the verified AMO-signed Firefox XPI for version {version}.")
        return 0
    except (OSError, ValueError, SignFirefoxError) as error:
        print(f"Firefox extension packaging failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
