# Install

## Requirements

**macOS** (tested on Apple Silicon, macOS 26)
- macOS 13 or later
- A working microphone (System Settings → Sound → Input)

**Linux** (tested on Fedora Asahi Remix + KDE Plasma 6)
- Wayland and systemd user sessions
- A working microphone (`wpctl status` should show it; `arecord -d 2 /tmp/test.wav && aplay /tmp/test.wav` to verify)

**Both**
- ~1 GB RAM for the loaded model, ~650 MB disk for the model files
- English-only — Parakeet is English-only. For multilingual, swap in Whisper via `transcribe-rs` features.

## 1. Fetch the Parakeet model (all platforms)

One-time, ~650 MB. Every install path needs it.

```bash
curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/download-model.sh | bash
```

Drops the model into:
- macOS: `~/Library/Application Support/utter/models/`
- Linux: `~/.local/share/utter/models/`

> **Linux one-liner users:** skip this — the `install-release.sh` one-command installer below runs the same fetch for you.

## 2. Install utter

### macOS (Apple Silicon)

Download `utter-VERSION-macos-arm64.dmg` from [the latest GitHub release](https://github.com/jguice/utter/releases/latest) (Apple Silicon only for now). The DMG is signed with a Developer ID certificate and notarized by Apple, so it opens without Gatekeeper warnings.

Open the DMG, drag `utter.app` to `/Applications` (or wherever you keep apps), then double-click to launch. On **first run** utter shows an onboarding window for the three macOS permissions it needs:

1. **Microphone** → click Grant → native prompt → Allow
2. **Input Monitoring** → click Grant → "Keystroke Receiving" prompt → Open System Settings → toggle utter ON → Quit & Reopen (macOS relaunches utter)
3. **Accessibility** → click Grant → Open System Settings → toggle utter ON → Quit & Reopen

After all three grants land, the menu bar icon appears and push-to-talk is live. The default key is **Right Cmd (⌘)**.

### Linux — one command (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/install-release.sh | bash
```

Detects your distro + arch, downloads the matching `.rpm` or `.deb` from the latest GitHub release, installs via `dnf`/`apt`, fetches the Parakeet model, and starts the services. Run as your regular user (the script invokes `sudo` internally); takes 2–5 minutes over a decent connection. Nothing else to do afterward — hold the key and speak.

Supported out of the box: **Fedora** (incl. Asahi Remix), **RHEL** / **CentOS** / **Rocky** / **Alma** / **Nobara**, **Debian**, **Ubuntu** (incl. Pop, Mint, KDE Neon). Both `x86_64` and `aarch64`.

Prefer to inspect first?

```bash
curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/install-release.sh -o install-release.sh
less install-release.sh
bash install-release.sh
```

### Linux — manual package install

Grab the asset that matches your system from [the latest GitHub release](https://github.com/jguice/utter/releases/latest):

| Distro family    | arm64 / aarch64                       | x86_64 / amd64                      |
|------------------|----------------------------------------|-------------------------------------|
| Fedora / RHEL    | `utter-VERSION-1.aarch64.rpm`        | `utter-VERSION-1.x86_64.rpm`      |
| Debian / Ubuntu  | `utter_VERSION_arm64.deb`            | `utter_VERSION_amd64.deb`         |
| Anything else    | `utter-VERSION-linux-arm64.tar.gz`   | `utter-VERSION-linux-amd64.tar.gz`|

```bash
# Fedora / RHEL / openSUSE
sudo dnf install ./utter-*.rpm

# Debian / Ubuntu
sudo apt install ./utter_*.deb
```

The package pulls in its runtime deps (`ydotool`, `alsa-utils`, `wl-clipboard`, `libnotify`), drops a udev rule for keyboard access via `uaccess` (no `usermod` needed), registers systemd user services, and configures the ydotool socket. It does **not** start the services or fetch the model — do that now:

```bash
systemctl --user daemon-reload
systemctl --user enable --now utter-daemon utter-watcher
```

(The model fetch from step 1 above still applies.)

## Verify

```bash
utter --version                                        # prints version
systemctl --user is-active utter-daemon utter-watcher  # both "active" (Linux)
systemctl is-active ydotool                            # "active" (Linux)
```

If any of those fail, jump to [Troubleshooting](TROUBLESHOOTING.md).

## From source (advanced)

Most users won't need this — skip unless no prebuilt package matches your system, or you want to hack on utter.

```bash
git clone https://github.com/jguice/utter.git
cd utter
./scripts/install.sh
```

First build takes ~5–10 minutes (downloads ONNX Runtime, compiles ~300 crates). The script handles system deps across Fedora / Debian / Ubuntu / Arch. Requires a Rust toolchain (`rustup` / `cargo`).
