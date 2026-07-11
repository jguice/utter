#!/bin/sh
set -e

# Reload udev so the new 90-utter.rules takes effect on currently-plugged
# keyboards. Without this, the `uaccess` tag only applies to devices that
# get added after install.
if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules || true
    udevadm trigger --subsystem-match=input || true
fi

if command -v systemctl >/dev/null 2>&1; then
    # --global enables the user services for every user on their next login.
    # Already-logged-in sessions need `systemctl --user daemon-reload` + start.
    systemctl --global enable utter-daemon.service utter-watcher.service || true
fi

cat <<'EOM'

utter is installed.

Before first use, download the Parakeet model (~640 MB) as your user:

    /usr/share/utter/download-model.sh

(Don't run with sudo — the model belongs in your XDG data dir.)

To start the services in your current session without logging out:

    systemctl --user daemon-reload
    systemctl --user enable --now utter-daemon utter-watcher

Then hold Right Cmd (or the key you configured) and speak.

EOM

exit 0
