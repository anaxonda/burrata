#!/usr/bin/env python3
"""Run fixed imported-playback qualifications against explicit cases on an existing isolated app.

Manifest fields: schemaVersion, automationFile, binary, binarySha256, version,
and cases[] with id, trackId, expectedRecordingId, expectedProvider, and
expectedDurationSeconds. This orchestrates only the checked-in MCP qualification;
it never launches Burrata or accepts a command from the manifest.
"""
import argparse
import hashlib
import json
import math
import os
import re
import signal
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
QUALIFIER = ROOT / "scripts/performance/native_imported_playback_qualification.mjs"
ID = re.compile(r"^[A-Za-z0-9._:-]{1,100}$")
PROVIDERS = {"monochrome", "youtube", "soundcloud"}


def validate_manifest(data):
    if not isinstance(data, dict) or data.get("schemaVersion") != 1:
        raise ValueError("manifest_schema")
    for field in ("automationFile", "binary", "binarySha256", "version"):
        if not isinstance(data.get(field), str) or not data[field].strip():
            raise ValueError(f"manifest_{field}")
    if not re.fullmatch(r"[0-9a-f]{64}", data["binarySha256"]):
        raise ValueError("manifest_binary_sha256")
    if not re.fullmatch(r"[A-Za-z0-9._+-]{1,80}", data["version"]):
        raise ValueError("manifest_version")
    cases = data.get("cases")
    if not isinstance(cases, list) or not cases:
        raise ValueError("manifest_cases")
    ids = set()
    allowed_case_fields = {"id", "trackId", "expectedRecordingId", "expectedProvider", "expectedDurationSeconds"}
    for case in cases:
        if not isinstance(case, dict):
            raise ValueError("case_object")
        if set(case) != allowed_case_fields:
            raise ValueError("case_fields")
        for field in ("id", "trackId", "expectedRecordingId"):
            if not isinstance(case.get(field), str) or not ID.fullmatch(case[field]):
                raise ValueError(f"case_{field}")
        if case["id"] in ids:
            raise ValueError("duplicate_case_id")
        ids.add(case["id"])
        if case.get("expectedProvider") not in PROVIDERS:
            raise ValueError("case_expected_provider")
        duration = case.get("expectedDurationSeconds")
        if isinstance(duration, bool) or not isinstance(duration, (int, float)) or not math.isfinite(duration) or duration <= 1:
            raise ValueError("case_expected_duration")
    return data


def source_identity_matches(expected, attempt):
    native = attempt.get("native") if isinstance(attempt, dict) else None
    return isinstance(native, dict) and native.get("sourceRecordingId") == expected


def summarize_case(case, report):
    attempts = list(report.get("warmups", [])) + list(report.get("starts", [])) + list(report.get("seeks", []))
    identity_rows = [{"native": report.get("initialState")}, *attempts]
    wrong_source = sum(not source_identity_matches(case["expectedRecordingId"], row) for row in identity_rows)
    return {
        "caseId": case["id"],
        "expectedProvider": case["expectedProvider"],
        "expectedRecordingId": case["expectedRecordingId"],
        "exploratoryCount": len(report.get("warmups", [])),
        "measuredStartCount": len(report.get("starts", [])),
        "measuredSeekCount": len(report.get("seeks", [])),
        "attemptCount": len(attempts),
        "failureCount": report.get("failureCount", 0),
        "setupFailure": report.get("setupFailure"),
        "wrongOrUnobservedSourceIdentityCount": wrong_source,
        "acousticContentVerified": False,
    }


def isolated_automation_file(value):
    endpoint = Path(value).resolve(strict=True)
    expected_suffix = Path("data/org.burrata.discovery/automation.json")
    if endpoint.name != "automation.json" or endpoint.parts[-3:] != expected_suffix.parts:
        raise ValueError("isolated_profile_required")
    profile = endpoint.parents[2]
    if profile.parent != Path("/tmp") or not re.fullmatch(r"burrata-launch\.[A-Za-z0-9_-]+", profile.name):
        raise ValueError("isolated_profile_required")
    marker = profile / ".burrata-isolated-profile"
    if marker.is_symlink() or not marker.is_file() or marker.read_text() != "burrata isolated memory run\n":
        raise ValueError("isolated_profile_required")
    return endpoint


def run_qualification(argv, timeout=3600):
    process = subprocess.Popen(argv, cwd=ROOT, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, start_new_session=(os.name == "posix"))
    try:
        return process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        if os.name == "posix":
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        else:
            process.kill()
        process.wait()
        raise


def run(manifest_path, output_path):
    manifest = validate_manifest(json.loads(Path(manifest_path).read_text()))
    endpoint = isolated_automation_file(manifest["automationFile"])
    binary = Path(manifest["binary"]).resolve(strict=True)
    actual_hash = hashlib.sha256(binary.read_bytes()).hexdigest()
    if actual_hash != manifest["binarySha256"]:
        raise ValueError("binary_hash_mismatch")
    output = Path(output_path).resolve()
    output.mkdir(parents=True, exist_ok=False)
    case_rows = []
    for case in manifest["cases"]:
        case_dir = output / case["id"]
        case_dir.mkdir()
        case_report = case_dir / "qualification.json"
        argv = ["node", str(QUALIFIER), str(endpoint), case["trackId"], str(case_report),
                str(binary), str(case["expectedDurationSeconds"]), case["expectedProvider"],
                case["expectedRecordingId"]]
        try:
            return_code = run_qualification(argv)
            if case_report.is_file():
                try:
                    report = json.loads(case_report.read_text())
                    row = summarize_case(case, report)
                    row.update({"status": "complete" if return_code == 0 else "qualification_failed",
                                "exitCode": return_code, "report": str(case_report)})
                except (OSError, ValueError, TypeError):
                    row = {"caseId": case["id"], "status": "invalid_report",
                           "failureCount": 1, "exitCode": return_code,
                           "report": str(case_report)}
            else:
                row = {"caseId": case["id"], "status": "qualification_no_report",
                       "failureCount": 1, "exitCode": return_code,
                       "report": str(case_report)}
        except subprocess.TimeoutExpired:
            row = {"caseId": case["id"], "status": "timeout", "failureCount": 1,
                   "report": str(case_report)}
        case_rows.append(row)
        (output / "matrix.json").write_text(json.dumps({
            "schemaVersion": 1, "version": manifest["version"],
            "binarySha256": actual_hash,
            "versionEvidence": "manifest label plus hash of supplied binary; running GUI binary is not independently inspected",
            "cases": case_rows,
            "failureCount": sum(row.get("failureCount", 1) > 0 or row.get("status") != "complete" for row in case_rows),
            "acousticContentVerified": False,
        }, indent=2) + "\n")
    return 1 if any(row.get("status") != "complete" or row.get("failureCount", 1) for row in case_rows) else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--output", type=Path, required=True, help="new result directory")
    args = parser.parse_args()
    try:
        result = run(args.manifest, args.output)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        parser.error(str(error))
    raise SystemExit(result)


if __name__ == "__main__":
    main()
