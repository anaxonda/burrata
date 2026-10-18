#!/usr/bin/env python3
"""Find common committed credential formats without echoing matched values."""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SKIP_DIRS = {
    ".git", ".cache", ".codex", ".agents", ".tmp", "node_modules", "dist", "target",
    "gen", "build", "coverage", "fixtures", "fixture", "testdata", "snapshots",
}
MAX_FILE_BYTES = 4 * 1024 * 1024

PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("private-key", re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY-----")),
    ("github-token", re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{30,})\b")),
    ("google-api-key", re.compile(r"(?<![A-Za-z0-9_-])AIza[A-Za-z0-9_-]{35}(?![A-Za-z0-9_-])")),
    ("aws-access-key", re.compile(r"\bAKIA[0-9A-Z]{16}\b")),
    ("slack-token", re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b")),
    ("credential-note", re.compile(r"(?i)^\s*(?:api[ _-]*key|shared[ _-]*secret)\s*(?:[:=]|\t)\s*([A-Za-z0-9_./+=-]{16,})")),
    # Require a quoted literal. Bare values at line start are handled by
    # credential-note below. This avoids treating expressions such as
    # `url.searchParams.get(...)` as if the expression text were a secret.
    ("provider-api-key", re.compile(
        r"(?i)\b(?:api[_-]?key|client[_-]?secret|access[_-]?token|auth[_-]?token|password|secret)\b[\"']?"
        r"\s*[:=]\s*([\"'])([^\"'\r\n]{16,})\1"
    )),
    ("provider-api-key-bare", re.compile(
        r"(?i)^\s*(?:export\s+)?(?:api[_-]?key|client[_-]?secret|access[_-]?token|auth[_-]?token|password|secret)"
        r"\s*[:=]\s*([A-Za-z0-9_./+=-]{16,})\s*(?:#.*)?$"
    )),
)
PLACEHOLDER = re.compile(r"(?i)(?:example|sample|dummy|fake|test[-_]?fixture[-_]?credential[-_]?[a-f0-9]{10}|change[-_ ]?(?:me|to)|replace[-_ ]?me|redacted|your[-_ ]|\$\{|\$[A-Z_]+|<[^>]+>|\*{3,})")


def tracked_paths() -> list[Path] | None:
    """Return git's tracked and nonignored untracked files when a worktree exists."""
    try:
        result = subprocess.run(
            ["git", "-C", str(ROOT), "ls-files", "-co", "--exclude-standard", "-z"],
            check=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    return [ROOT / name.decode("utf-8", "surrogateescape") for name in result.stdout.split(b"\0") if name]


def walk_paths() -> list[Path]:
    paths: list[Path] = []
    for current, directories, files in os.walk(ROOT):
        directories[:] = sorted(name for name in directories if name not in SKIP_DIRS)
        base = Path(current)
        paths.extend(base / name for name in files)
    return paths


def ignored_path(path: Path, skip_local_ignored: bool) -> bool:
    try:
        relative = path.relative_to(ROOT)
    except ValueError:
        return True
    ignored_local_service_data = skip_local_ignored and (relative.as_posix().startswith("deploy/slskd/data/") or relative.as_posix()=="lastfm.txt" or relative.name==".env" or relative.name.startswith(".env."))
    return ignored_local_service_data or any(part in SKIP_DIRS for part in relative.parts)


def scan_file(path: Path) -> list[tuple[int, str]]:
    try:
        if path.stat().st_size > MAX_FILE_BYTES:
            return []
        content = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        return []
    findings: list[tuple[int, str]] = []
    for line_number, line in enumerate(content.splitlines(), 1):
        for name, pattern in PATTERNS:
            for match in pattern.finditer(line):
                value = match.group(2) if name == "provider-api-key" else (match.group(1) if name in {"credential-note", "provider-api-key-bare"} else match.group(0))
                if name in {"provider-api-key", "provider-api-key-bare", "credential-note"} and PLACEHOLDER.search(value):
                    continue
                findings.append((line_number, name))
                break
    return findings


def main() -> int:
    candidates = tracked_paths()
    fallback_scan = candidates is None
    if fallback_scan:
        candidates = walk_paths()
        print("Note: git metadata unavailable; scanned the working tree only.", file=sys.stderr)
    findings: list[tuple[str, int, str]] = []
    for path in candidates:
        if ignored_path(path, skip_local_ignored=fallback_scan) or not path.is_file():
            continue
        for line, kind in scan_file(path):
            findings.append((path.relative_to(ROOT).as_posix(), line, kind))
    if findings:
        print("Potential secrets found (matched text is intentionally not shown):")
        for path, line, kind in findings:
            print(f"  {path}:{line}: {kind}")
        return 1
    print("No supported secret patterns found. This scanner is heuristic and does not audit git history.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
