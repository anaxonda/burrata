#!/usr/bin/env python3
"""Bind Fedora RPM smoke evidence to the exact upstream installer and source."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import math

FEDORA_IMAGE = "registry.fedoraproject.org/fedora@sha256:7a04a2f9d11bba3ff8016ae79cc01ba49965578746c930702e141a3be1f8d4de"


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(65536), b""):
            h.update(block)
    return h.hexdigest()


def downloaded_rpm(root, commit, tag, version):
    if not re.fullmatch(r"[a-f0-9]{40}", commit) or not re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?", version):
        raise ValueError("Invalid requested source/version")
    inventory = json.loads((root / "artifact-inventory.json").read_text())
    if any(inventory.get(k) != v for k, v in {"commit": commit, "tag": tag, "label": "linux-x64", "target": "x86_64-unknown-linux-gnu"}.items()):
        raise ValueError("RPM inventory source/target mismatch")
    records = [r for r in inventory["artifacts"] if str(r.get("file", "")).endswith(".rpm")]
    if len(records) != 1:
        raise ValueError("Exactly one RPM inventory record required")
    record = records[0]
    if not re.fullmatch(r"[A-Za-z0-9._+-]+\.rpm", record["file"]) or not re.fullmatch(r"[a-f0-9]{64}", record.get("sha256", "")):
        raise ValueError("Unsafe RPM inventory filename/digest")
    rpm = root / "installers" / record["file"]
    actual = list((root / "installers").glob("*.rpm"))
    if actual != [rpm] or not rpm.is_file() or rpm.is_symlink() or rpm.resolve() != rpm.absolute():
        raise ValueError("Unexpected, missing or linked RPM")
    if type(record.get("bytes")) is not int or rpm.stat().st_size != record["bytes"] or digest(rpm) != record["sha256"]:
        raise ValueError("RPM bytes do not match upstream inventory")
    return {"installer": str(rpm.absolute()), "installerSha256": record["sha256"],
            "sourceCommit": commit, "sourceVersion": version, "tag": tag}


def validate_evidence(evidence, artifact):
    def at_least(value, minimum):
        return type(value) in (int, float) and math.isfinite(value) and value >= minimum
    if evidence.get("status") != "passed" or any(evidence.get(k) != artifact[k] for k in ("sourceCommit", "sourceVersion", "installerSha256")):
        raise ValueError("RPM smoke failed or is for different source/installer")
    checks = evidence.get("checks", {})
    rpm = checks.get("rpm", {})
    if (any(rpm.get(k) is not True for k in ("postRuntimeVerify", "uninstalled", "packageAndRuntimePathsAbsent"))
            or type(rpm.get("runtimeUid")) is not int or rpm["runtimeUid"] <= 0
            or rpm.get("installedVersion") != artifact["sourceVersion"]):
        raise ValueError("Real unprivileged RPM verification/removal proof missing")
    cleanup = checks.get("rpmRuntimeCleanup")
    if (not isinstance(cleanup, dict) or cleanup.get("noLiveOwnedProcessesProven") is not True
            or type(cleanup.get("errors")) is not list or cleanup["errors"]):
        raise ValueError("Identity-scoped owned runtime cleanup proof missing or failed")
    native = checks.get("nativeAudio", {})
    output = native.get("osOutput", {})
    control = native.get("privateMonitorControl", {})
    def mcp_passed(item):
        return (isinstance(item, dict)
                and all(item.get(k) is True for k in ("initialized", "statusRunning", "frontendMounted"))
                and type(item.get("toolCount")) is int and item["toolCount"] > 0)
    entrypoints = checks.get("mcpEntrypoints")
    consoles = entrypoints.get("shippedConsole") if isinstance(entrypoints, dict) else None
    if (not mcp_passed(checks.get("mcp")) or not isinstance(entrypoints, dict)
            or not mcp_passed(entrypoints.get("applicationMcp"))
            or not isinstance(consoles, list) or not consoles or not all(mcp_passed(item) for item in consoles)):
        raise ValueError("Actual frontend evidence missing for a required public MCP entrypoint")
    if (native.get("status") != "sampled"
            or native.get("engine") != "native" or native.get("channels") != 2
            or not at_least(native.get("capturedSeconds"), 2.9) or output.get("postPlayerGain") is not True
            or output.get("physicalHardware") is not False or not at_least(output.get("verifiedToneSeconds"), 3)
            or not at_least(output.get("remuteQuietSeconds"), .5) or control.get("remuteWhilePlaying") is not True
            or not at_least(control.get("remuteClockAdvanceSeconds"), .5)):
        raise ValueError("Actual frontend/Native/private OS output evidence missing")
    provenance = evidence.get("rpmArtifactProvenance", {})
    if (provenance.get("verifiedReleaseWorkflowRun") is not True
            or provenance.get("repository") != "anaxonda/burrata"
            or provenance.get("artifactSourceCommit") != artifact["sourceCommit"]
            or not re.fullmatch(r"[a-f0-9]{40}", provenance.get("headSha", ""))
            or provenance.get("artifactWorkflowHeadSha") != provenance.get("headSha")
            or str(provenance.get("runId")) != evidence.get("sourceRunId")
            or provenance.get("harnessCommit") != evidence.get("harnessCommit")
            or provenance.get("image") != FEDORA_IMAGE
            or not re.fullmatch(r"[a-f0-9]{40}", evidence.get("harnessCommit", ""))):
        raise ValueError("RPM upstream workflow/image/harness provenance missing")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--artifact-root", type=Path)
    p.add_argument("--commit", required=True)
    p.add_argument("--tag", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--output", type=Path)
    p.add_argument("--raw-evidence", type=Path)
    p.add_argument("--provenance", type=Path)
    p.add_argument("--incoming", type=Path)
    args = p.parse_args()
    if args.incoming:
        roots = list(args.incoming.glob(f"burrata-linux-x64-{args.commit}"))
        evidence_paths = list(args.incoming.glob(f"burrata-rpm-smoke-{args.commit}/rpm-smoke-evidence.json"))
        if len(roots) != 1 or len(evidence_paths) != 1:
            raise ValueError("RPM platform artifact or installed acceptance evidence missing")
        artifact = downloaded_rpm(roots[0], args.commit, args.tag, args.version)
        validate_evidence(json.loads(evidence_paths[0].read_text()), artifact)
        return
    artifact = downloaded_rpm(args.artifact_root, args.commit, args.tag, args.version)
    if args.raw_evidence:
        evidence = json.loads(args.raw_evidence.read_text())
        evidence["rpmArtifactProvenance"] = json.loads(args.provenance.read_text())
        validate_evidence(evidence, artifact)
        artifact = evidence
    args.output.write_text(json.dumps(artifact, indent=2) + "\n")


if __name__ == "__main__":
    main()
