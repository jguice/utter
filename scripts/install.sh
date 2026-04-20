#!/usr/bin/env bash
# utter installer.
#
# What it does:
#   1. Install system packages (openssl-dev, clang, cmake, alsa-utils,
#      wl-clipboard, ydotool, libnotify)
#   2. Install rustup if `cargo` isn't on PATH
#   3. `cargo install --path . --locked`
#   4. Download the Parakeet model
#   5. Drop in ydotool socket-owner config + enable the system service
#   6. Add the user to the `input` group so the watcher can read evdev
#   7. Install the utter-daemon and -watcher user services
#
# What it does NOT do:
#   - Start the services (the `input` group change takes effect at next
#     login, so starting before logout/reboot would fail)
#   - Reboot / log you out
#
# After running this, LOG OUT AND BACK IN, then:
#   systemctl --user start utter-daemon utter-watcher

set -euo pipefail

# --- helpers -----------------------------------------------------------------

BOLD="$(tput bold 2>/dev/null || true)"
DIM="$(tput dim 2>/dev/null || true)"
RED="$(tput setaf 1 2>/dev/null || true)"
GREEN="$(tput setaf 2 2>/dev/null || true)"
YELLOW="$(tput setaf 3 2>/dev/null || true)"
RESET="$(tput sgr0 2>/dev/null || true)"

step() { echo; echo "${BOLD}==>${RESET} $*"; }
info() { echo "    $*"; }
warn() { echo "${YELLOW}${BOLD}warn:${RESET} $*"; }
fail() { echo "${RED}${BOLD}error:${RESET} $*" >&2; exit 1; }

# --- prereq checks -----------------------------------------------------------

[[ "$(uname -s)" == "Linux" ]] || fail "Linux only."
[[ -n "${XDG_RUNTIME_DIR-}" ]] || \
  fail "XDG_RUNTIME_DIR not set. Run from within a systemd user session (a regular desktop login)."

# Warn (but don't hard-fail) outside Wayland — clipboard/paste use wl-copy
# and ydotool, which work on X11 too but our defaults assume Wayland.
if [[ "${XDG_SESSION_TYPE-}" != "wayland" ]]; then
  warn "XDG_SESSION_TYPE is '${XDG_SESSION_TYPE-unset}', not 'wayland'. Continuing, but auto-paste may misbehave."
fi

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_DIR"

# --- distro dispatch ---------------------------------------------------------

if [[ ! -r /etc/os-release ]]; then
  fail "/etc/os-release missing — can't detect distro."
fi
. /etc/os-release

step "Detected distro: ${ID:-unknown} ${VERSION_ID:-}"

case "${ID:-}" in
  fedora|fedora-asahi-remix)
    INSTALL_CMD=(sudo dnf install -y)
    PKGS=(openssl-devel pkgconf-pkg-config clang cmake gcc-c++ alsa-utils wl-clipboard libnotify ydotool)
    ;;
  debian|ubuntu|pop|linuxmint)
    INSTALL_CMD=(sudo apt-get install -y)
    sudo apt-get update
    PKGS=(libssl-dev pkg-config clang cmake g++ alsa-utils wl-clipboard libnotify-bin ydotool)
    ;;
  arch|manjaro|endeavouros|cachyos)
    INSTALL_CMD=(sudo pacman -S --needed --noconfirm)
    PKGS=(openssl pkgconf clang cmake gcc alsa-utils wl-clipboard libnotify ydotool)
    ;;
  *)
    warn "Unsupported distro '${ID:-?}'. Install these manually, then re-run:"
    warn "  openssl-dev, pkg-config, clang, cmake, g++, alsa-utils, wl-clipboard, libnotify, ydotool"
    read -r -p "Continue anyway? [y/N] " cont
    [[ "$cont" == "y" || "$cont" == "Y" ]] || exit 1
    PKGS=()
    INSTALL_CMD=()
    ;;
esac

# --- 1. system packages ------------------------------------------------------

if [[ ${#PKGS[@]} -gt 0 ]]; then
  step "Installing system packages (sudo)"
  info "${PKGS[*]}"
  "${INSTALL_CMD[@]}" "${PKGS[@]}"
fi

# --- 2. rust toolchain -------------------------------------------------------

if ! command -v cargo >/dev/null 2>&1; then
  step "Installing Rust toolchain via rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain stable --profile minimal
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
else
  step "Rust found: $(cargo --version)"
fi

# --- 3. build + install binary ----------------------------------------------

# Stop running utter services before cargo install so it doesn't hit ETXTBSY
# on a repeat-run upgrade. Remember whether they were running so we can
# restart them after the install step finishes.
RESTART_DAEMON=0
RESTART_WATCHER=0
if systemctl --user is-active --quiet utter-daemon 2>/dev/null; then
  RESTART_DAEMON=1
fi
if systemctl --user is-active --quiet utter-watcher 2>/dev/null; then
  RESTART_WATCHER=1
fi
if [[ $RESTART_DAEMON -eq 1 || $RESTART_WATCHER -eq 1 ]]; then
  step "Stopping utter services for upgrade"
  systemctl --user stop utter-watcher utter-daemon 2>/dev/null || true
fi

step "Building + installing utter (this takes a few minutes on first run)"
cargo install --path . --locked --force

if [[ $RESTART_DAEMON -eq 1 ]]; then
  systemctl --user start utter-daemon || true
  sleep 1
fi
if [[ $RESTART_WATCHER -eq 1 ]]; then
  systemctl --user start utter-watcher || true
fi

# --- 4. download the model ---------------------------------------------------

step "Downloading Parakeet model (~640 MB, skipped if already present)"
"$REPO_DIR/scripts/download-model.sh"

# --- 5. ydotool system service + drop-in ------------------------------------

UID_NUM="$(id -u)"
GID_NUM="$(id -g)"

step "Configuring ydotool system service (sudo)"
sudo mkdir -p /etc/systemd/system/ydotool.service.d
# Regenerate the drop-in with the actual UID:GID instead of shipping a
# hard-coded 1000:1000 — different distros assign different first-user UIDs.
sudo tee /etc/systemd/system/ydotool.service.d/owner.conf >/dev/null <<EOF
[Service]
ExecStart=
ExecStart=/usr/bin/ydotoold --socket-own=${UID_NUM}:${GID_NUM}
EOF
sudo systemctl daemon-reload
sudo systemctl enable --now ydotool

# --- 6. input group membership ----------------------------------------------

if id -nG "$USER" | tr ' ' '\n' | grep -qx input; then
  info "already in 'input' group"
else
  step "Adding $USER to the 'input' group (needed for evdev key watching)"
  sudo usermod -aG input "$USER"
  GROUP_NEEDS_RELOGIN=1
fi

# --- 7. user systemd services -----------------------------------------------

step "Installing utter user services"
mkdir -p "$HOME/.config/systemd/user"
cp examples/utter-daemon.service "$HOME/.config/systemd/user/"
cp examples/utter-watcher.service "$HOME/.config/systemd/user/"
systemctl --user daemon-reload
systemctl --user enable utter-daemon utter-watcher

# --- done --------------------------------------------------------------------

echo
echo "${GREEN}${BOLD}Install complete.${RESET}"
echo

if [[ -n "${GROUP_NEEDS_RELOGIN-}" ]]; then
  cat <<EOF
${YELLOW}${BOLD}Important:${RESET} you were just added to the 'input' group. The new
membership takes effect at your next login, so:

  1. Log out of your desktop session (not just lock — fully log out)
  2. Log back in
  3. The services will start automatically (they're enabled).

Verify with:
  id | grep -o input
  systemctl --user status utter-daemon utter-watcher

Then hold Right Cmd (or your configured key) and speak into a text field.
EOF
else
  cat <<EOF
Start the services now:

  systemctl --user start utter-daemon utter-watcher

Then hold Right Cmd (or your configured key) and speak into a text field.
EOF
fi

echo
echo "To change the push-to-talk key, edit ExecStart in"
echo "  ~/.config/systemd/user/utter-watcher.service"
echo "and restart the watcher."
