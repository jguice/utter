#!/usr/bin/env bash
# Run the drop-in's ExecStartPost against a root-only socket (what
# ydotoold 0.1.x creates) and check it ends up usable by other users.
set -euo pipefail
conf="$UTTER_REPO/packaging/systemd/ydotool-owner.conf"
cmd="$(sed -n 's/^ExecStartPost=-//p' "$conf")"
[[ -n $cmd ]] || { echo "no ExecStartPost in $conf"; exit 1; }
sock=/tmp/.ydotool_socket
kind=dgram; [[ $UTTER_YDOTOOL_GEN == 0.1 ]] && kind=stream
python3 "$UTTER_REPO/scripts/ci/fake-ydotoold.py" "$kind" "$sock" > /tmp/fake-ydotoold-perm.log &
fake=$!
trap 'kill $fake 2>/dev/null || true' EXIT
for _ in $(seq 50); do grep -q ready /tmp/fake-ydotoold-perm.log && break; sleep 0.1; done
chmod 0600 "$sock"
eval "$cmd"
mode="$(stat -c %a "$sock")"
echo "socket mode after ExecStartPost: $mode"
[[ $mode == 666 ]]
