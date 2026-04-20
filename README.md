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

Detects your distro + arch, downloads the matching `.rpm` or `.deb` from the latest GitHub release, installs via `dnf`/`apt`, fetches the Parakeet model into your XDG data dir, and starts the services. Run as your regular user (the script invokes `sudo` internally); takes 2–5 minutes over a decent connection. Nothing else to do afterward — hold the key and speak.

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

Unlike the one-liner above, the manual path doesn't download the model or start the services — do those yourself now:

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

- **Default key:** Right Cmd on Apple keyboards, Right Super on others (`rightmeta` in kernel-event terms). To change it, run `utter set-key` and press the key you'd rather use.
- **Visual cue:** while you hold the key, your desktop's standard **microphone-in-use icon** lights up in the tray / status bar. That's the intentional "utter is listening" indicator — no custom overlay.
- **Output:** text is copied to both the clipboard and the primary selection, then auto-pasted via Shift+Insert into the focused window.

## Configuration

utter runs as two systemd user services, each handling different concerns:

- **`utter-daemon`** — loads the model, records audio, transcribes, pastes. Owns all the output-side environment variables in the table below.
- **`utter-watcher`** — watches `/dev/input` for key events. Its only knob is which key triggers recording (`--key <name>` in its `ExecStart`); see [Change the push-to-talk key](#change-the-push-to-talk-key).

To change any of the env vars below, run `systemctl --user edit utter-daemon` and add an `[Service]` block with the overrides.

| Env var                  | Values                                         | Default          | Purpose                                                                 |
|--------------------------|------------------------------------------------|------------------|-------------------------------------------------------------------------|
| `UTTER_AUTOTYPE`      | `0` / `1`                                      | `1`              | When 0, only copies to clipboard (Ctrl+V to paste manually).            |
| `UTTER_PASTE_METHOD`  | `shift-insert`, `ctrl-v`, `ctrl-shift-v`, `type` | `shift-insert` | Which keystroke to send after copying. `type` = character-at-a-time.    |
| `UTTER_CLEANUP`       | `0` / `1`                                      | `1`              | Drop fillers (uh/um/er/ah/erm/hmm), collapse stutters (`wh wh what`→`what`, `I I I think`→`I think`). Set 0 for raw Parakeet output. |
| `UTTER_NOTIFY`        | `0` / `1`                                      | `0`              | When 1, fires a short `notify-send` toast on recording start / error.   |
| `YDOTOOL_SOCKET`         | path                                           | `/tmp/.ydotool_socket` | Socket path for the ydotool daemon (only change if you relocated it). |
| `RUST_LOG`               | `info`, `debug`, ...                           | `info`           | Log verbosity (visible via `journalctl --user -u utter-daemon`). |

### Change the push-to-talk key

Easiest: let utter detect the key for you.

```bash
utter set-key
```

Press and hold the key you want, then release. utter reports what it detected (e.g. `Detected: rightmeta (code 126)`), confirms the press **and** release both fired cleanly (so you know hold-to-talk will actually work on that key), and writes the override to `~/.config/systemd/user/utter-watcher.service.d/override.conf`. Pass `--dry-run` to just detect without saving.

That covers both "what evdev name does this key have?" and "can utter actually read this key's events on my system?" in one step — useful if you remapped a key in your keyboard firmware (QMK, VIA, Karabiner-on-macOS-sibling-tool) or via a desktop utility, and you want to confirm it shows up as something usable.

### Recording indicator

While you're holding the key, your desktop's standard **microphone-in-use icon** (the small mic that appears in KDE's system tray, GNOME's top bar, etc.) is the intentional visual cue that utter is listening. When you release the key, recording stops and the icon disappears. utter doesn't ship a custom overlay widget on purpose — the system indicator is already there, already correct, and doesn't draw anything ugly over your screen.

For a noisier feedback mode (a toast on start / errors), set `UTTER_NOTIFY=1` (see the table above).

### Manual override (if you'd rather)

`utter set-key` is a wrapper around editing the systemd unit. To do it by hand:

```bash
systemctl --user edit utter-watcher
```

In the editor that opens, add:

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/utter watch --key capslock
```

The `--key` argument accepts either a named alias or a raw evdev keycode (as digits). Any key that exists in `/usr/include/linux/input-event-codes.h` works — the named list below is just so the common cases are readable:

- **Modifiers:** `leftmeta rightmeta leftctrl rightctrl leftalt rightalt leftshift rightshift`
- **Lock keys:** `capslock scrolllock numlock`
- **Navigation + utility:** `home end pageup pagedown insert menu printscreen pause`
- **Function keys:** `f1`..`f24`
- **Apple aliases:** `rightcmd leftcmd rightoption leftoption` (map to `rightmeta` etc.)
- **Anything else:** pass the raw evdev code as digits, e.g. `--key 70` for scroll lock, `--key 194` for F21. `utter set-key` detects whichever key you press and picks the nicer form automatically.

Save, then `systemctl --user restart utter-watcher`. For from-source installs, the `ExecStart` binary path is `%h/.cargo/bin/utter` instead of `/usr/bin/utter`.

## Architecture

```
               ┌─── utter watch ────┐
evdev ──► key event (press/release)
               └──► Unix socket ───────────┐
                                           │
                                           ▼
                               ┌─── utter daemon ───┐
                               │ ┌──────────┐  ┌──────────┐ │
                               │ │ Parakeet │  │ arecord  │ │
                               │ │   ONNX   │  │ subproc  │ │
                               │ └──────────┘  └──────────┘ │
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
