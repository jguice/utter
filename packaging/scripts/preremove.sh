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

    # Don't touch ydotool — the user may want it for other tools.
fi

exit 0
