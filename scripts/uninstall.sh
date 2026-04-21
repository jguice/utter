#!/usr/bin/env bash
# utter uninstaller.
#
# Usage (from anywhere):
#   curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/uninstall.sh | bash
#
# Or with flags:
#   ./scripts/uninstall.sh [--keep-model] [-y|--yes]
#
# Works for both packaged (.rpm/.deb) and from-source installs. Detects which
# one you have and removes the right pieces. Run as your regular user, not
# root — the script invokes `sudo` internally where needed.
#
# What it removes:
#   - utter binary (package uninstall OR ~/.cargo/bin/utter)
#   - utter-daemon.service + utter-watcher.service (package-owned OR
#     ~/.config/systemd/user/)
#   - user-created overrides under ~/.config/systemd/user/utter-*.service.d/
#   - udev rule installed by the package (done by dnf/apt)
#   - ydotool socket-owner drop-in we installed (if present)
#   - the Parakeet model at ~/.local/share/utter (unless --keep-model)
#
# What it leaves alone:
#   - ydotool itself (may be used by other tools on your system)
#   - `input` group membership (you can remove manually with
#     `sudo gpasswd -d "$USER" input` if you want)

set -euo pipefail

BOLD="$(tput bold 2>/dev/null || true)"
GREEN="$(tput setaf 2 2>/dev/null || true)"
YELLOW="$(tput setaf 3 2>/dev/null || true)"
RED="$(tput setaf 1 2>/dev/null || true)"
RESET="$(tput sgr0 2>/dev/null || true)"

step() { echo; echo "${BOLD}==>${RESET} $*"; }
info() { echo "    $*"; }
warn() { echo "${YELLOW}${BOLD}warn:${RESET} $*"; }
fail() { echo "${RED}${BOLD}error:${RESET} $*" >&2; exit 1; }

KEEP_MODEL=0
ASSUME_YES=0
for arg in "$@"; do
  case "$arg" in
    --keep-model) KEEP_MODEL=1 ;;
    -y|--yes) ASSUME_YES=1 ;;
    -h|--help)
      sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) fail "unknown option: $arg" ;;
  esac
done

[[ $EUID -ne 0 ]] || fail "Run as your regular user, not root. The script uses \`sudo\` where needed."

# --- detect install type -----------------------------------------------------

PACKAGED=false
PKG_TOOL=""
if command -v rpm >/dev/null 2>&1 && rpm -q utter >/dev/null 2>&1; then
  PACKAGED=true
  PKG_TOOL="dnf"
elif command -v dpkg-query >/dev/null 2>&1 && dpkg-query -s utter >/dev/null 2>&1; then
  PACKAGED=true
  PKG_TOOL="apt-get"
fi

if $PACKAGED; then
  step "Detected packaged install (${PKG_TOOL})"
elif [[ -x "$HOME/.cargo/bin/utter" ]] || \
     [[ -f "$HOME/.config/systemd/user/utter-daemon.service" ]]; then
  step "Detected from-source install"
else
  step "No utter install detected — cleaning up anything stray."
fi

# --- confirm -----------------------------------------------------------------

if [[ $ASSUME_YES -eq 0 ]]; then
  echo
  echo "This will remove utter from your system."
  if [[ $KEEP_MODEL -eq 0 ]]; then
    echo "The Parakeet model at ~/.local/share/utter will also be deleted"
    echo "(~640 MB). Pass --keep-model to keep it."
  fi
  echo
  # Read from the controlling terminal so this works under `curl ... | bash`,
  # where stdin is the script body. Fall back to /dev/null if no TTY (CI etc).
  if [[ -r /dev/tty ]]; then
    read -r -p "Continue? [y/N] " reply < /dev/tty
  else
    warn "no TTY for prompt — pass -y to confirm non-interactively."
    exit 1
  fi
  [[ "$reply" == "y" || "$reply" == "Y" ]] || { echo "cancelled."; exit 0; }
fi

# --- stop user services ------------------------------------------------------

step "Stopping utter user services"
systemctl --user disable --now utter-daemon utter-watcher 2>/dev/null | sed 's/^/    /' || true

# --- remove binary + service files ------------------------------------------

if $PACKAGED; then
  step "Removing the utter package (sudo)"
  case "$PKG_TOOL" in
    dnf)     sudo dnf remove -y utter ;;
    apt-get) sudo apt-get remove -y utter ;;
  esac

  # Package preremove tries to `systemctl --user stop` from the sudo context,
  # which often can't reach the user session — the binary stays running after
  # the unit file is deleted. Reap any orphans now.
  orphan_pids=$(pgrep -u "$UID" -x utter 2>/dev/null || true)
  if [[ -n "$orphan_pids" ]]; then
    step "Killing orphan utter processes"
    echo "    $orphan_pids"
    kill $orphan_pids 2>/dev/null || true
    sleep 1
    # Anything still lingering?
    orphan_pids=$(pgrep -u "$UID" -x utter 2>/dev/null || true)
    [[ -n "$orphan_pids" ]] && kill -9 $orphan_pids 2>/dev/null || true
  fi
else
  step "Removing from-source utter files"
  rm -f "$HOME/.config/systemd/user/utter-daemon.service"
  rm -f "$HOME/.config/systemd/user/utter-watcher.service"
  rm -f "$HOME/.cargo/bin/utter"

  # Only remove the ydotool drop-in if it was not the one shipped by a
  # package. Since we just established this is a from-source install, any
  # drop-in here is ours to clean up.
  if [[ -f /etc/systemd/system/ydotool.service.d/owner.conf ]]; then
    step "Removing ydotool socket-owner drop-in (sudo)"
    sudo rm -f /etc/systemd/system/ydotool.service.d/owner.conf
    sudo rmdir /etc/systemd/system/ydotool.service.d 2>/dev/null || true
    sudo systemctl daemon-reload || true
  fi
fi

# --- remove user-created service overrides ----------------------------------

if [[ -d "$HOME/.config/systemd/user/utter-daemon.service.d" ]] || \
   [[ -d "$HOME/.config/systemd/user/utter-watcher.service.d" ]]; then
  step "Removing user-created service overrides"
  rm -rf "$HOME/.config/systemd/user/utter-daemon.service.d"
  rm -rf "$HOME/.config/systemd/user/utter-watcher.service.d"
fi

systemctl --user daemon-reload 2>/dev/null || true
# Clear "not-found inactive dead" entries left in the systemd user graph after
# unit files are removed. Cosmetic — they'd clear on next login — but tidier.
systemctl --user reset-failed 2>/dev/null || true

# Remove stale sockets so a subsequent install starts clean.
if [[ -n "${XDG_RUNTIME_DIR-}" ]]; then
  rm -f "$XDG_RUNTIME_DIR/utter.sock"
fi

# --- model -------------------------------------------------------------------

if [[ $KEEP_MODEL -eq 0 ]]; then
  if [[ -d "$HOME/.local/share/utter" ]]; then
    step "Removing Parakeet model"
    rm -rf "$HOME/.local/share/utter"
  fi
else
  info "Keeping ~/.local/share/utter (--keep-model)."
fi

# --- done --------------------------------------------------------------------

echo
echo "${GREEN}${BOLD}Uninstall complete.${RESET}"
echo
cat <<'EOM'
Notes:
- ydotool was not removed — it may be used by other tools on your system.
  To remove it anyway: sudo dnf remove -y ydotool    (or apt remove)
- Your `input` group membership (if any) was left alone. To drop it:
      sudo gpasswd -d "$USER" input
  …then log out / log in for the change to take effect.
EOM
