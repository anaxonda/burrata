#!/usr/bin/env bash
set -euo pipefail
script_dir="$(cd "$(dirname "$0")/.." && pwd)"
source "$script_dir/automation-ready.sh"
root="$(mktemp -d "${TMPDIR:-/tmp}/burrata-automation-ready-test.XXXXXX")"
trap 'rm -rf -- "$root"' EXIT
endpoint="$root/automation.json"

# An unchanged endpoint from a prior process must never satisfy startup.
printf 'stale endpoint fixture\n' > "$endpoint"
stale_identity="$(automation_file_identity "$endpoint")"
if automation_registration_changed "$endpoint" "$stale_identity"; then
  echo "unchanged stale endpoint was treated as fresh" >&2
  exit 1
fi

# Same-directory atomic replacement must provide a new filesystem identity.
printf 'new endpoint fixture\n' > "$root/automation.new"
mv -f -- "$root/automation.new" "$endpoint"
if ! automation_registration_changed "$endpoint" "$stale_identity"; then
  echo "atomic endpoint replacement was not detected" >&2
  exit 1
fi

# A previously absent endpoint becomes ready on first creation.
rm -- "$endpoint"
absent_identity=""
if automation_registration_changed "$endpoint" "$absent_identity"; then
  echo "absent endpoint was treated as ready" >&2
  exit 1
fi
printf 'first endpoint fixture\n' > "$endpoint"
if ! automation_registration_changed "$endpoint" "$absent_identity"; then
  echo "newly created endpoint was not detected" >&2
  exit 1
fi
printf 'automation readiness fixtures passed\n'
