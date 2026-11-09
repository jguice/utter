#!/usr/bin/env bash
# Runs every scripts/ci/checks/*.sh against the distro tools installed in a
# CI container. Each check tests one assumption utter makes about a
# distro-packaged tool (ydotool, systemd, ...), so a PR that relies on a new
# assumption adds its own check file.
#
# Usage (as root, inside the container): run-distro-checks.sh <ydotool-gen>
#   ydotool-gen: "0.1" (Debian/Ubuntu) or "1" (Fedora/Asahi)
set -euo pipefail
export UTTER_YDOTOOL_GEN="${1:?usage: $0 <ydotool-gen>}"
export UTTER_REPO="$(cd "$(dirname "$0")/../.." && pwd)"

fail=0
for check in "$UTTER_REPO"/scripts/ci/checks/*.sh; do
  echo "::group::$(basename "$check")"
  if bash "$check"; then
    echo "ok: $(basename "$check")"
  else
    echo "::error::$(basename "$check") failed"
    fail=1
  fi
  echo "::endgroup::"
done
exit "$fail"
