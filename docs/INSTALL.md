# Install

The common paths (macOS DMG + Linux one-liner) are covered in the [README quickstart](../README.md#quickstart). This page is for requirements, manual package install, verifying the install, and building from source.

## Requirements

**macOS** (tested on Apple Silicon, macOS 26)
- macOS 13 or later
- A working microphone (System Settings → Sound → Input)

**Linux** (tested on Fedora Asahi Remix + KDE Plasma 6)
- Wayland and systemd user sessions
- A working microphone (`wpctl status` should show it)
- Member of the `input` group (`sudo usermod -aG input $USER`, then log out/in). The package installs a udev rule that may handle this via `uaccess` ACLs, but group membership is the reliable fallback.

**Both**
- ~1 GB RAM for the loaded model, ~650 MB disk for the model files
- English-only — Parakeet is English-only. For multilingual, swap in Whisper via `transcribe-rs` features.

## Linux — manual package install

If you'd rather skip the one-line installer, grab the asset that matches your system from [the latest release](https://github.com/jguice/utter/releases/latest):

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

The package pulls in its runtime deps, drops a udev rule for keyboard access via `uaccess`, and registers systemd user services. It does **not** start the services or fetch the model — do that now:

```bash
curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/download-model.sh | bash
systemctl --user daemon-reload
systemctl --user enable --now utter-daemon utter-watcher
```

If the watcher fails to start (check with `journalctl --user -u utter-watcher -n 20`), the `uaccess` ACL may not be working on your system. Add yourself to the `input` group as a fallback:

```bash
sudo usermod -aG input $USER
# Log out and back in for the group change to take effect.
```

## Verify

```bash
utter --version                                        # prints version
systemctl --user is-active utter-daemon utter-watcher  # both "active" (Linux)
```

If any of those fail, jump to [Troubleshooting](TROUBLESHOOTING.md).

## From source (advanced)

Most users won't need this — skip unless no prebuilt package matches your system, or you want to hack on utter.

System libraries: you'll need the ALSA development headers (`alsa-devel` on Fedora/openSUSE, `libasound2-dev` on Debian/Ubuntu). Runtime libraries are already present on any system with audio output.

```bash
git clone https://github.com/jguice/utter.git
cd utter
./scripts/install.sh
```

First build takes ~5–10 minutes (downloads ONNX Runtime, compiles ~300 crates). The script handles system deps across Fedora / Debian / Ubuntu / Arch. Requires a Rust toolchain (`rustup` / `cargo`).
