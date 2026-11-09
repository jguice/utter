#!/usr/bin/env bash
# ydotool.service plus utter's drop-in must parse cleanly on this distro.
# Debian/Ubuntu get the unit from utter's package; Fedora ships its own.
set -euo pipefail
cd "$UTTER_REPO"
unit=/usr/lib/systemd/system/ydotool.service
[[ -f $unit ]] || install -Dm644 packaging/systemd/ydotool.service "$unit"
install -Dm644 packaging/systemd/ydotool-owner.conf \
  /etc/systemd/system/ydotool.service.d/owner.conf
systemctl cat ydotool.service 2>/dev/null || cat "$unit" /etc/systemd/system/ydotool.service.d/owner.conf
systemd-analyze verify ydotool.service
