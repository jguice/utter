#!/usr/bin/env bash
# utter one-shot release installer.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/install-release.sh | bash
#
# Or clone the repo and run ./scripts/install-release.sh. Run as your regular
# user (not root) — the script calls `sudo` internally only where needed, and
# a root-owned install will break the per-user systemd + model-download steps.
#
# What it does:
#   1. Detect distro (Fedora / Debian / Ubuntu family) and CPU arch.
#   2. Fetch the latest utter release from GitHub.
#   3. Download the right .rpm or .deb for your distro+arch.
#   4. Install it via dnf / apt (pulls in ydotool, alsa-utils, wl-clipboard,
#      libnotify as deps; drops udev + systemd files; enables ydotool).
#   5. Download the Parakeet model (~640 MB) as your user.
#   6. Start utter-daemon and utter-watcher in your current session.

set -euo pipefail

BOLD="$(tput bold 2>/dev/null || true)"
GREEN="$(tput setaf 2 2>/dev/null || true)"
YELLOW="$(tput setaf 3 2>/dev/null || true)"
RED="$(tput setaf 1 2>/dev/null || true)"
RESET="$(tput sgr0 2>/dev/null || true)"

step() { echo; echo "${BOLD}==>${RESET} $*"; }
warn() { echo "${YELLOW}${BOLD}warn:${RESET} $*"; }
fail() { echo "${RED}${BOLD}error:${RESET} $*" >&2; exit 1; }

[[ "$(uname -s)" == "Linux" ]] || fail "Linux only."
[[ $EUID -ne 0 ]] || fail "Run as your regular user, not root. The script uses \`sudo\` where needed."
[[ -n "${XDG_RUNTIME_DIR-}" ]] || \
  fail "XDG_RUNTIME_DIR not set. Run from inside a systemd user session (i.e. a normal desktop login)."

# --- distro / arch detection -------------------------------------------------

[[ -r /etc/os-release ]] || fail "/etc/os-release missing — can't detect distro."
. /etc/os-release

case "${ID:-}:${ID_LIKE:-}" in
  *fedora*|*rhel*|*centos*|*rocky*|*almalinux*|*nobara*)
    PKG_FORMAT=rpm
    INSTALL_CMD=(sudo dnf install -y)
    ;;
  *debian*|*ubuntu*|*linuxmint*|*pop*|*neon*)
    PKG_FORMAT=deb
    INSTALL_CMD=(sudo apt-get install -y)
    # apt -y on a local file needs an extra flag
    ;;
  *)
    fail "unsupported distro '${ID:-?}'. See the README for manual install."
    ;;
esac

case "$(uname -m)" in
  x86_64|amd64)   DEB_ARCH=amd64; RPM_ARCH=x86_64 ;;
  aarch64|arm64)  DEB_ARCH=arm64; RPM_ARCH=aarch64 ;;
  *) fail "unsupported architecture: $(uname -m)" ;;
esac

step "Detected: ${ID} on $(uname -m) → ${PKG_FORMAT}"

# --- fetch latest release ----------------------------------------------------

step "Finding latest utter release"
LATEST="$(curl -sSfL https://api.github.com/repos/jguice/utter/releases/latest \
           | python3 -c 'import json,sys; print(json.load(sys.stdin)["tag_name"])')"
VERSION="${LATEST#v}"
echo "    $LATEST"

if [[ "$PKG_FORMAT" == "rpm" ]]; then
  ASSET="utter-${VERSION}-1.${RPM_ARCH}.rpm"
else
  ASSET="utter_${VERSION}_${DEB_ARCH}.deb"
fi
URL="https://github.com/jguice/utter/releases/download/${LATEST}/${ASSET}"

# --- download + install ------------------------------------------------------

step "Downloading ${ASSET}"
TMPFILE="$(mktemp --suffix=".${PKG_FORMAT}")"
trap 'rm -f "$TMPFILE"' EXIT
curl -fsSL --progress-bar -o "$TMPFILE" "$URL" || \
  fail "download failed. Check that ${URL} exists."

step "Installing (sudo)"
if [[ "$PKG_FORMAT" == "deb" ]]; then
  sudo apt-get update -qq
  sudo apt-get install -y "$TMPFILE"
else
  "${INSTALL_CMD[@]}" "$TMPFILE"
fi

# --- model + services --------------------------------------------------------

step "Downloading Parakeet model (~640 MB, skipped if already present)"
/usr/share/utter/download-model.sh

step "Starting utter services"
systemctl --user daemon-reload
systemctl --user enable --now utter-daemon utter-watcher

echo
echo "${GREEN}${BOLD}Install complete.${RESET}"
echo
cat <<'EOM'
Hold Right Cmd (or your configured key) and speak into any text field.

Verify anytime with:
    systemctl --user status utter-daemon utter-watcher
    journalctl --user -u utter-daemon -f

To change the key (default: rightmeta), edit:
    systemctl --user edit utter-watcher

…and override ExecStart with e.g. --key capslock, --key f13, etc.
See /usr/share/doc/utter/README.md for the full list.
EOM
