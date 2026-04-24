<img src="utter-icon.png" alt="utter" width="128" align="right">

# utter

[![CI](https://github.com/jguice/utter/actions/workflows/ci.yml/badge.svg)](https://github.com/jguice/utter/actions/workflows/ci.yml)
[![Release](https://github.com/jguice/utter/actions/workflows/release.yml/badge.svg)](https://github.com/jguice/utter/actions/workflows/release.yml)

Local, no-cloud push-to-talk dictation for **Linux and macOS**. Hold a key, speak, release — the transcription appears in whatever text field is focused.

Uses [NVIDIA Parakeet-TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) (INT8 ONNX) for speech recognition, runs entirely offline, ~50× faster than real-time on a modern laptop CPU (measured ~150 ms for a 4-second utterance on an M2 Max).

**What utter does:**

- **True hold-to-talk.** Press the key, speak, release — the transcription appears. Key press *and* release are read directly from the OS event stream (evdev on Linux, CGEventTap on macOS), so the interaction feels immediate regardless of which UI framework owns the focused window.
- **Fully local.** No cloud, no API keys, no telemetry, no server process. The Parakeet model runs entirely on your device.
- **Fast.** ~150 ms to transcribe 4 seconds of audio on a modern CPU — roughly 50× faster than real-time.
- **Per-platform native integration.** Linux: paste through ydotool's `uinput` layer, works on any Wayland compositor. macOS: menu-bar `LSUIElement` app, signed with Developer ID, first-run onboarding window walks you through the three TCC prompts (Microphone, Input Monitoring, Accessibility).
- **Cross-architecture.** Prebuilt `.deb` / `.rpm` for `aarch64` and `x86_64` Linux; signed + notarized `.dmg` for Apple Silicon macOS.

## Requirements

**Linux** (tested on Fedora Asahi Remix + KDE Plasma 6)
- Wayland and systemd user sessions
- A working microphone (`wpctl status` should show it; `arecord -d 2 /tmp/test.wav && aplay /tmp/test.wav` to verify)

**macOS** (tested on Apple Silicon, macOS 26)
- macOS 13 or later
- A working microphone (System Settings → Sound → Input)

**Both**
- ~1 GB RAM for the loaded model, ~650 MB disk for the model files
- For the from-source path only: a Rust toolchain (`rustup` / `cargo`)

English-only — Parakeet is English-only. For multilingual, swap in Whisper via `transcribe-rs` features.

> **Using an agent?** Point it at [`AGENTS.md`](AGENTS.md) for task-structured install / verify / configure / troubleshoot instructions in a copy-paste-ready format.

## Install

### Linux — one command (recommended)

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

### macOS

Download `utter-VERSION-macos-arm64.dmg` from [the latest GitHub release](https://github.com/jguice/utter/releases/latest) (Apple Silicon only for now). The DMG is signed with a Developer ID certificate and notarized by Apple, so it opens without Gatekeeper warnings.

Open the DMG, drag `utter.app` to `/Applications` (or wherever you keep apps), then double-click to launch. On **first run** utter shows an onboarding window for the three macOS permissions it needs:

1. **Microphone** → click Grant → native prompt → Allow
2. **Input Monitoring** → click Grant → "Keystroke Receiving" prompt → Open System Settings → toggle utter ON → Quit & Reopen (macOS relaunches utter)
3. **Accessibility** → click Grant → Open System Settings → toggle utter ON → Quit & Reopen

After all three grants land, the menu bar icon appears and push-to-talk is live. The default key is **Right Option (⌥)**; change it from the menu bar's *Change PTT Key…* item.

You still need the Parakeet model (~650 MB, one-time):

```bash
curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/download-model.sh | bash
```

The model drops into `~/Library/Application Support/utter/models/`.

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
- **Output:** text is written to the primary selection and auto-pasted via Shift+Insert into the focused window. The regular clipboard is deliberately left untouched so whatever you last copied is preserved.

## Configuration

utter runs as two systemd user services:

- **`utter-daemon`** — loads the model, records audio, transcribes, emits text.
- **`utter-watcher`** — watches `/dev/input` for the PTT key and tells the daemon when to start and stop.

Both read their user-facing settings from a `config.toml`:

- Linux: `~/.config/utter/config.toml`
- macOS: `~/Library/Application Support/utter/config.toml`

The file is created on first daemon start (using defaults + any `UTTER_*` env vars you had set at the time) and re-read on every daemon restart. Edit it by hand, via `utter set-key` for the PTT key, or (on macOS) via the menu-bar toggles for the bool settings. Restart to pick up changes:

```bash
# Linux
systemctl --user restart utter-daemon utter-watcher

# macOS: Cmd+Q from the menu bar icon, then reopen utter.app
```

Default contents:

```toml
# utter configuration.

# PTT key: named alias or numeric evdev keycode (as a string).
key = "rightmeta"

# Synthesize Shift+Insert to paste. false = user pastes manually.
auto_paste = true

# Also write dictations to the regular clipboard (for clipboard-manager
# users). Default leaves the regular clipboard untouched.
write_clipboard = false

# Drop fillers (uh, um, er, ah, erm, hmm) and collapse stuttered
# repetitions (`I I I think` → `I think`).
filter_filler_words = true
```

### Env var overrides

Every field above is overridable at runtime via an environment variable with the same name, upper-cased and prefixed `UTTER_` — e.g. `UTTER_AUTO_PASTE=0` wins over `auto_paste = true` in the file. Useful for one-off runs (`UTTER_AUTO_PASTE=0 utter daemon`) or systemd-drop-in tweaks without editing the config file:

| Env var                     | Values    | Overrides field       | Purpose                                                                 |
|-----------------------------|-----------|-----------------------|-------------------------------------------------------------------------|
| `UTTER_KEY`                 | name/code | `key`                 | PTT key.                                                                |
| `UTTER_AUTO_PASTE`          | `0` / `1` | `auto_paste`          | Synthesize Shift+Insert paste.                                          |
| `UTTER_WRITE_CLIPBOARD`     | `0` / `1` | `write_clipboard`     | Also write the regular clipboard (not just primary selection).          |
| `UTTER_FILTER_FILLER_WORDS` | `0` / `1` | `filter_filler_words` | Drop fillers (uh/um/er/ah/erm/hmm), collapse stutters.                  |

These stay env-only (third-party tools, not utter's config):

| Env var          | Default                 | Purpose                                                                 |
|------------------|-------------------------|-------------------------------------------------------------------------|
| `YDOTOOL_SOCKET` | `/tmp/.ydotool_socket`  | Socket path for the ydotool daemon (only change if you relocated it).   |
| `RUST_LOG`       | `info`                  | Log verbosity (`journalctl --user -u utter-daemon`).                    |

Precedence: **CLI flag > env var > config file > default.**

### Change the push-to-talk key

Easiest: let utter detect the key for you.

```bash
utter set-key
```

Press and hold the key you want, then release. utter reports what it detected (e.g. `Detected: rightmeta (code 126)`), confirms the press **and** release both fired cleanly (so you know hold-to-talk will actually work on that key), and writes the new key to `~/.config/utter/config.toml`. Pass `--dry-run` to just detect without saving.

That covers both "what evdev name does this key have?" and "can utter actually read this key's events on my system?" in one step — useful if you remapped a key in your keyboard firmware (QMK, VIA, Karabiner-on-macOS-sibling-tool) or via a desktop utility, and you want to confirm it shows up as something usable.

### Recording indicator

While you're holding the key, your desktop's standard **microphone-in-use icon** (the small mic that appears in KDE's system tray, GNOME's top bar, the macOS menu bar, etc.) is the intentional visual cue that utter is listening. When you release the key, recording stops and the icon disappears. utter doesn't ship a custom overlay widget on purpose — the system indicator is already there, already correct, and doesn't draw anything ugly over your screen.

### Manual override (if you'd rather)

`utter set-key` is a wrapper that edits `~/.config/utter/config.toml`. To do it by hand, open the file in your editor and change the `key = "..."` line:

```toml
key = "capslock"
```

Then restart the watcher: `systemctl --user restart utter-watcher`.

The `key` field accepts either a named alias or a raw evdev keycode as a string. Any key that exists in `/usr/include/linux/input-event-codes.h` works — the named list below is just so the common cases are readable:

- **Modifiers:** `leftmeta rightmeta leftctrl rightctrl leftalt rightalt leftshift rightshift`
- **Lock keys:** `capslock scrolllock numlock`
- **Navigation + utility:** `home end pageup pagedown insert menu printscreen pause`
- **Function keys:** `f1`..`f24`
- **Apple aliases:** `rightcmd leftcmd rightoption leftoption` (map to `rightmeta` etc.)
- **Anything else:** pass the raw evdev code as digits (as a quoted string in TOML), e.g. `key = "70"` for scroll lock, `key = "194"` for F24. `utter set-key` detects whichever key you press and picks the nicer form automatically.

Save, then `systemctl --user restart utter-watcher`.

## Architecture (Linux)

> macOS runs as a single-process `LSUIElement` menu-bar app (no systemd, no ydotool). Audio via cpal + CoreAudio; PTT via CGEventTap; paste via NSPasteboard + synthesized Cmd+V through CGEventPost; permissions flow through a first-run onboarding window that drives the native TCC prompts. Everything below describes the Linux multi-service architecture.


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
                                     wl-copy --primary (primary selection)
                                     ydotool key (Shift+Insert)
                                     ─► focused window
```

Two systemd user services:

- **`utter-daemon`** — loads the model once (~630 ms on M2 Max), opens a Unix socket at `$XDG_RUNTIME_DIR/utter.sock`, accepts `start` / `stop` / `toggle` / `quit`. `start` forks `arecord` writing to `/tmp/utter-*.wav`. `stop` SIGINTs arecord, hands the WAV to Parakeet, writes the output to the primary selection, and synthesizes Shift+Insert via ydotool.
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

**Paste goes to the wrong place / nothing pastes.**
Shift+Insert pastes from the primary selection — which any mouse text-selection overwrites. If you highlighted something between releasing the key and the paste firing, the paste may use *that* text instead of your transcription. Release the PTT key in the window you want the text to land in and don't touch the mouse until it pastes.

If the cursor blinks but no text appears, check `journalctl --user -u utter-daemon -n 50` for `paste failed` or `wl-copy failed` warnings — those indicate ydotool or the compositor rejected something.

## Uninstall

### macOS

Drag `utter.app` to the Trash. The Parakeet model lives at `~/Library/Application Support/utter/` — `rm -rf ~/Library/Application\ Support/utter/` clears it plus the config file. Revoke the TCC grants in System Settings → Privacy & Security if you want a fully clean state.

### Linux — one command

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
