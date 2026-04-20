# utter

Local, no-cloud push-to-talk dictation for Linux. Hold a key, speak, release — the transcription appears in whatever text field is focused.

Uses [NVIDIA Parakeet-TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) (INT8 ONNX) for speech recognition, runs entirely offline, ~50× faster than real-time on a modern laptop CPU (measured ~150 ms for a 4-second utterance on an M2 Max).

**What utter does:**

- **True hold-to-talk.** Press the key, speak, release — the transcription appears. Key press *and* release are both read directly from the kernel via evdev, so the interaction feels immediate and doesn't depend on the compositor cooperating.
- **Fully local.** No cloud, no API keys, no telemetry, no server process. The Parakeet model runs entirely on your device.
- **Fast.** ~150 ms to transcribe 4 seconds of audio on a modern CPU — roughly 50× faster than real-time.
- **Compositor-agnostic.** Paste goes through ydotool's `uinput` layer, so text lands in the focused window regardless of whether you're on KDE, GNOME, Sway, or any other Wayland desktop.
- **Cross-architecture.** Prebuilt `.deb` and `.rpm` packages for both `aarch64` (Apple Silicon / Snapdragon / Ampere / Pi) and `x86_64`.

## Requirements

- Linux with Wayland and systemd user sessions (tested on Fedora Asahi Remix, KDE Plasma 6)
- A working microphone (`wpctl status` should show it; try `arecord -d 2 /tmp/test.wav && aplay /tmp/test.wav`)
- ~1 GB RAM for the loaded model, ~650 MB disk for the model files
- For the from-source path only: a Rust toolchain (`rustup` / `cargo`)

English-only — Parakeet is English-only. For multilingual, swap in Whisper via `transcribe-rs` features.

> **Using an agent?** Point it at [`AGENTS.md`](AGENTS.md) for task-structured install / verify / configure / troubleshoot instructions in a copy-paste-ready format.

## Install

### One command (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/install-release.sh | bash
```

Detects your distro + arch, downloads the matching `.rpm` or `.deb` from the latest GitHub release, installs via `dnf`/`apt`, fetches the Parakeet model into your XDG data dir, and starts the services. Run as your regular user (the script invokes `sudo` internally); takes 2–5 minutes over a decent connection.

Supported out of the box: **Fedora** (incl. Asahi Remix), **RHEL** / **CentOS** / **Rocky** / **Alma** / **Nobara**, **Debian**, **Ubuntu** (incl. Pop, Mint, KDE Neon). Both `x86_64` and `aarch64`.

Prefer to inspect first?

```bash
curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/install-release.sh -o install-release.sh
less install-release.sh
bash install-release.sh
```

### Manual package install

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

The package pulls in its runtime deps (`ydotool`, `alsa-utils`, `wl-clipboard`, `libnotify`), drops a udev rule for keyboard access via `uaccess` (no `usermod` needed), registers systemd user services, and configures the ydotool socket.

After install, download the model and start the services (one time):

```bash
/usr/share/utter/download-model.sh
systemctl --user daemon-reload
systemctl --user enable --now utter-daemon utter-watcher
```

### From source

If no prebuilt package matches your system, or you want to hack on it:

```bash
git clone https://github.com/jguice/utter.git
cd utter
./scripts/install.sh
```

First build takes ~5–10 minutes (downloads ONNX Runtime, compiles ~300 crates). The script handles system deps across Fedora / Debian / Ubuntu / Arch.

### Verify

```bash
utter --version                                        # prints version
systemctl --user is-active utter-daemon utter-watcher # both "active"
systemctl is-active ydotool                               # "active"
```

If any of those fail, jump to [Troubleshooting](#troubleshooting).

## Usage

Hold the push-to-talk key, speak, release. The transcription pastes into whichever field has focus.

- **Default key:** Right Cmd on Apple keyboards, Right Super on others (`rightmeta` in kernel-event terms).
- **Output:** text is copied to both the clipboard and the primary selection, then auto-pasted via Shift+Insert.

## Configuration

All configuration is via systemd service environment variables. To change any of them, run `systemctl --user edit utter-daemon` (or `utter-watcher`) and add an `[Service]` block with the overrides.

| Env var                  | Values                                         | Default          | Purpose                                                                 |
|--------------------------|------------------------------------------------|------------------|-------------------------------------------------------------------------|
| `UTTER_AUTOTYPE`      | `0` / `1`                                      | `1`              | When 0, only copies to clipboard (Ctrl+V to paste manually).            |
| `UTTER_PASTE_METHOD`  | `shift-insert`, `ctrl-v`, `ctrl-shift-v`, `type` | `shift-insert` | Which keystroke to send after copying. `type` = character-at-a-time.    |
| `UTTER_CLEANUP`       | `0` / `1`                                      | `1`              | Drop fillers (uh/um/er/ah/erm/hmm), collapse stutters (`wh wh what`→`what`, `I I I think`→`I think`). Set 0 for raw Parakeet output. |
| `UTTER_NOTIFY`        | `0` / `1`                                      | `0`              | When 1, fires a short `notify-send` toast on recording start / error.   |
| `YDOTOOL_SOCKET`         | path                                           | `/tmp/.ydotool_socket` | Socket path for the ydotool daemon (only change if you relocated it). |
| `RUST_LOG`               | `info`, `debug`, ...                           | `info`           | Log verbosity (visible via `journalctl --user -u utter-daemon`). |

### Change the push-to-talk key

Override the service unit (this works for both packaged and from-source installs and survives upgrades):

```bash
systemctl --user edit utter-watcher
```

In the editor that opens, add:

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/utter watch --key capslock
```

Replace `capslock` with any of:

```
leftmeta rightmeta leftctrl rightctrl leftalt rightalt leftshift rightshift
capslock f1..f20
```

Apple-friendly aliases also work: `rightcmd`, `leftcmd`, `rightoption`, `leftoption`.

For from-source installs the binary path is `%h/.cargo/bin/utter` instead of `/usr/bin/utter`. Save and exit; the watcher restarts automatically.

## Architecture

```
               ┌─── utter watch ────┐
evdev ──► key event (press/release)
               └──► Unix socket ───────────┐
                                           │
                                           ▼
                               ┌─── utter daemon ───┐
                               │ ┌──────┐  ┌────────────┐ │
                               │ │ Pkr  │  │  arecord   │ │
                               │ │ ONNX │  │  subproc   │ │
                               │ └──────┘  └────────────┘ │
                               └─────────────┬─────────────┘
                                             │
                                             ▼
                                     wl-copy (clipboard + primary)
                                     ydotool key (paste keystroke)
                                     ─► focused window
```

Two systemd user services:

- **`utter-daemon`** — loads the model once (~630 ms on M2 Max), opens a Unix socket at `$XDG_RUNTIME_DIR/utter.sock`, accepts `start` / `stop` / `toggle` / `quit`. `start` forks `arecord` writing to `/tmp/utter-*.wav`. `stop` SIGINTs arecord, hands the WAV to Parakeet, and runs the output through clipboard + paste keystroke.
- **`utter-watcher`** — async evdev loop. Enumerates keyboards at startup, watches for the configured key on each, sends `start` on press and `stop` on release to the daemon. Ignores autorepeat (value=2).

Plus one system service for `ydotoold` (the only privileged component — needs `/dev/uinput`).

## Troubleshooting

**Nothing pastes after I release the key.**
```bash
journalctl --user -u utter-daemon -n 30
journalctl --user -u utter-watcher -n 30
```
Look for "recording to ..." / "transcribed in ..." in the daemon log and "key down → start" / "key up → stop" in the watcher log. If neither shows, the watcher isn't picking up the key. Check read access to `/dev/input/event*`:

```bash
ls -l /dev/input/event* | head
```
- **Packaged install:** the udev rule should grant ACL read access on login; run `sudo udevadm control --reload-rules && sudo udevadm trigger --subsystem-match=input` and log out+in if needed.
- **From-source install:** confirm you're in the `input` group (`id | grep input`). If not, `sudo usermod -aG input "$USER"` and log out+in.

**"no audio captured" error.**
Your mic isn't producing samples. Test with `arecord -d 3 /tmp/x.wav && aplay /tmp/x.wav`. If that fails, check `wpctl status` (default source is right and volume is non-zero) and `journalctl --user | grep spa.alsa`.

**Auto-paste is slow / characters appear one-by-one.**
Your configured paste method failed (the focused app didn't accept it) and fell back to `ydotool type`. Try a different paste method via `UTTER_PASTE_METHOD` — some apps want `ctrl-v`, terminals want `ctrl-shift-v`, Claude Code and most others want `shift-insert`.

**Text pastes something else entirely (stale text).**
Usually means only one of the two selections (clipboard / primary) is being written. The installed version writes both; if you're on an older build, pull and reinstall.

## Uninstall

### One command

```bash
curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/uninstall.sh | bash
```

Detects whether you installed from a package or from source, stops the user services, removes the binary + units + ydotool drop-in + user overrides, and (unless you pass `--keep-model`) deletes the Parakeet model under `~/.local/share/utter`. Run as your regular user; it `sudo`s internally where needed.

Flags: `--keep-model` to retain the model, `-y` to skip the confirmation prompt.

Leaves `ydotool` and your `input` group membership alone — both may be used by other tools. The uninstall output tells you how to remove them manually if you want.

### Manual

Prefer to do it by hand? The script is <100 lines, inspect it in `scripts/uninstall.sh`. The short version:

- **Package install:** `systemctl --user disable --now utter-{daemon,watcher}` then `sudo dnf remove -y utter` (or `sudo apt remove -y utter`).
- **Source install:** as above, plus `cargo uninstall utter` and `sudo rm -f /etc/systemd/system/ydotool.service.d/owner.conf`.
- **Model:** `rm -rf ~/.local/share/utter` (not package-owned).

## Credits & license

- **Software:** MIT, see [`LICENSE`](LICENSE).
- **Parakeet-TDT 0.6B v3 model:** NVIDIA — CC-BY-4.0. Original model card: <https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3>. ONNX conversion by Ilya Stupakov: <https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx>.
- **Inference stack:** [`transcribe-rs`](https://github.com/cjpais/transcribe-rs) by cjpais (MIT).
- See [`NOTICE`](NOTICE) for full attribution and third-party licenses.

Contributions welcome — especially per-compositor input injection backends, hotplug-aware evdev watching, and config-file support. See [`BACKLOG.md`](BACKLOG.md) for starter ideas.
