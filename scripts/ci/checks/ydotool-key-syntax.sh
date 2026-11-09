#!/usr/bin/env bash
# utter picks `ydotool key` syntax from its help text (classify_ydotool_help
# in src/main.rs; keep the marker strings in sync). Check the real package
# prints the help utter expects for its generation.
set -euo pipefail
sock=/tmp/.ydotool_socket
kind=dgram; [[ $UTTER_YDOTOOL_GEN == 0.1 ]] && kind=stream
python3 "$UTTER_REPO/scripts/ci/fake-ydotoold.py" "$kind" "$sock" > /tmp/fake-ydotoold.log &
fake=$!
trap 'kill $fake 2>/dev/null || true' EXIT
for _ in $(seq 50); do grep -q ready /tmp/fake-ydotoold.log && break; sleep 0.1; done

help="$(env -u XDG_RUNTIME_DIR YDOTOOL_SOCKET="$sock" ydotool key --help 2>&1 || true)"
echo "$help"
if grep -q "separated by plus" <<<"$help"; then got=0.1
elif grep -q "Usage: key" <<<"$help"; then got=1
else got=unknown; fi
echo "detected ydotool key syntax: $got (expected $UTTER_YDOTOOL_GEN)"
[[ $got == "$UTTER_YDOTOOL_GEN" ]]
