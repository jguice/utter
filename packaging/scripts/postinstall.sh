#!/bin/sh
set -e

# Reload udev so the new 90-utter.rules takes effect on currently-plugged
# keyboards. Without this, the `uaccess` tag only applies to devices that
# get added after install.
if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules || true
    # `change` action so logind re-evaluates ACLs on already-plugged devices
    # rather than only firing for new device adds.
    udevadm trigger --subsystem-match=input --action=change || true
fi

# Belt-and-suspenders: the udev/uaccess rule above grants ACL access to
# the active-seat user without requiring `input` group membership, which
# is great when it works — but on some session configurations (no local
# seat assigned, headless, certain Wayland compositors) logind never
# applies the ACL. Add the installing user to `input` as a fallback so
# utter-watcher works after a re-login even when uaccess silently fails.
INSTALL_USER="${SUDO_USER:-}"
if [ -z "$INSTALL_USER" ] && [ -n "${PKEXEC_UID:-}" ]; then
    INSTALL_USER="$(getent passwd "$PKEXEC_UID" | cut -d: -f1 || true)"
fi
if [ -n "$INSTALL_USER" ] && [ "$INSTALL_USER" != "root" ] \
    && getent group input >/dev/null 2>&1 \
    && ! id -nG "$INSTALL_USER" 2>/dev/null | tr ' ' '\n' | grep -qx input
then
    if usermod -aG input "$INSTALL_USER" 2>/dev/null; then
        ADDED_INPUT_GROUP=1
    fi
fi

# Install & start ydotool (the system service that drives /dev/uinput).
# Our drop-in at /etc/systemd/system/ydotool.service.d/owner.conf sets
# a world-writable socket so utter running as any user can use it.
if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload || true
    systemctl enable ydotool.service || true
    # Use restart (not just enable --now) so the new drop-in takes effect on
    # upgrade installs where ydotool was already running under the old config.
    # restart on a stopped service just starts it, so this handles fresh
    # installs too.
    systemctl restart ydotool.service || true

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

To change the PTT key (default: rightmeta), run:

    utter set-key

…then press and hold the key you want and release. The watcher restarts
automatically.

EOM

if [ "${ADDED_INPUT_GROUP:-0}" = "1" ]; then
    cat <<EOM
Added user '$INSTALL_USER' to the 'input' group as a permission fallback.
Most desktops grant the watcher access immediately via udev/uaccess —
if utter-watcher fails with 'no input device advertises key …', log out
and back in to pick up the new group membership.

EOM
fi

exit 0
