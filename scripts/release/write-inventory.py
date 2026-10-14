import hashlib
import json
import os
import pathlib
import platform
import subprocess
import sys

output = pathlib.Path(sys.argv[1])
files = sorted(pathlib.Path("release-artifacts/installers").iterdir())
inventory = {
    "tag": os.environ["RELEASE_TAG"],
    "commit": os.environ["RELEASE_COMMIT"],
    "target": os.environ["TARGET"],
    "label": os.environ["LABEL"],
    "runner": platform.platform(),
    "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
    "node": subprocess.check_output(["node", "--version"], text=True).strip(),
    "artifacts": [
        {"file": f.name, "sha256": hashlib.sha256(f.read_bytes()).hexdigest(), "bytes": f.stat().st_size}
        for f in files
    ],
}
output.write_text(json.dumps(inventory, indent=2) + "\n", encoding="utf-8")
