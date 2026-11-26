#!/bin/sh
set -e

if command -v systemctl >/dev/null 2>&1; then
    # Disable the user services globally so they don't start for any user
    # after uninstall.
    systemctl --global disable utter-daemon.service utter-watcher.service || true

    # Stop any running instances in active user sessions. This is best-effort —
    # we may not have session visibility from the package scriptlet context.
    for uid in $(loginctl list-sessions --no-legend 2>/dev/null | awk '{print $2}' | sort -u); do
        if [ -n "$uid" ]; then
            su -s /bin/sh "$(getent passwd "$uid" | cut -d: -f1)" -c \
                'systemctl --user stop utter-watcher utter-daemon' 2>/dev/null || true
        fi
    done

    # Don't touch ydotool — the user may want it for other tools. Only undo
    # the user-unit disable that postinstall did on ydotool 1.0.x systems.
    # deb passes "remove"/"purge" on uninstall; rpm passes 0. Skip on upgrades.
    case "${1:-}" in remove|purge|0) full_remove=1 ;; *) full_remove=0 ;; esac
    if [ "$full_remove" = 1 ] && [ -f /var/lib/utter/ydotool-user-unit-disabled ]; then
        if [ -f /usr/lib/systemd/user/ydotool.service ]; then
            systemctl --global enable ydotool.service || true
        fi
        rm -f /var/lib/utter/ydotool-user-unit-disabled
        rmdir /var/lib/utter 2>/dev/null || true
    fi
fi

exit 0
