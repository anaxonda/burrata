#!/usr/bin/env python3
"""Run the maintained playback identity checks before desktop compilation.

Uses production Rust modules through the standalone audit crates. This gate does
not replace native integration, provider transport, or packaged audio checks.
Set CARGO_TARGET_DIR to a writable cache outside /tmp for local qualification.
"""
from pathlib import Path
import argparse
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def commands():
    return [
        [sys.executable, "scripts/check_contracts.py"],
        [sys.executable, "scripts/test_matching_stage5.py"],
        [sys.executable, "scripts/matching_stage5.py", "validate-matrix"],
        ["cargo", "fmt", "--manifest-path", "src-tauri/Cargo.toml", "--", "--check"],
        ["cargo", "test", "--locked", "--manifest-path", "scripts/matching-audit/Cargo.toml"],
        ["cargo", "test", "--locked", "--manifest-path", "scripts/heldout-matching/Cargo.toml"],
        ["cargo", "run", "--locked", "--manifest-path", "scripts/matching-audit/Cargo.toml", "--bin", "stage5-matrix", "--", "src-tauri/tests/fixtures/recording-match-stage5-matrix-v1.json"],
        ["cargo", "run", "--locked", "--manifest-path", "scripts/heldout-matching/Cargo.toml", "--", "docs/fixtures/recording-match-stage5-heldout-catalog-v4.json"],
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true", help="show commands without executing")
    args = parser.parse_args()
    for command in commands():
        print("+ " + " ".join(command), flush=True)
        if not args.dry_run:
            subprocess.run(command, cwd=ROOT, check=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
