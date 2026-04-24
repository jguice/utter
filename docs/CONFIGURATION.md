# Configuration

The basics — hold-to-talk, default key, changing the key — are in the [README quickstart](../README.md#quickstart). This page is the full reference: config file format, env var overrides, the PTT-key alias table, the recording indicator, and how services are laid out on Linux.

## Config file

utter's settings live in a TOML file, created on first daemon start (using defaults + any `UTTER_*` env vars set at the time) and re-read whenever utter restarts.

- Linux: `~/.config/utter/config.toml`
- macOS: `~/Library/Application Support/utter/config.toml`

Edit it by hand, via `utter set-key` for the PTT key, or (on macOS) via the menu-bar toggles for the bool flags. Restart to pick up changes:

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

# Synthesize the paste shortcut (Shift+Insert on Linux, Cmd+V on macOS).
# false = user pastes manually.
auto_paste = true

# Also write dictations to the regular clipboard. Linux-only; macOS has
# a single pasteboard and the auto-paste flow already uses it.
write_clipboard = false

# Drop fillers (uh, um, er, ah, erm, hmm) and collapse stuttered
# repetitions (`I I I think` → `I think`).
filter_filler_words = true
```

## Env var overrides

Every field above is overridable at runtime via an environment variable with the same name, upper-cased and prefixed `UTTER_` — e.g. `UTTER_AUTO_PASTE=0` wins over `auto_paste = true` in the file. Useful for one-off runs (`UTTER_AUTO_PASTE=0 utter daemon`) or systemd-drop-in tweaks without editing the file:

| Env var                     | Values    | Overrides field       | Purpose                                                                 |
|-----------------------------|-----------|-----------------------|-------------------------------------------------------------------------|
| `UTTER_KEY`                 | name/code | `key`                 | PTT key.                                                                |
| `UTTER_AUTO_PASTE`          | `0` / `1` | `auto_paste`          | Synthesize the paste shortcut.                                          |
| `UTTER_WRITE_CLIPBOARD`     | `0` / `1` | `write_clipboard`     | Also write the regular clipboard (Linux only).                          |
| `UTTER_FILTER_FILLER_WORDS` | `0` / `1` | `filter_filler_words` | Drop fillers (uh/um/er/ah/erm/hmm), collapse stutters.                  |

These stay env-only (third-party tools, not utter's config):

| Env var          | Default                 | Purpose                                                                 |
|------------------|-------------------------|-------------------------------------------------------------------------|
| `YDOTOOL_SOCKET` | `/tmp/.ydotool_socket`  | Socket path for the ydotool daemon (Linux; only change if you relocated it). |
| `RUST_LOG`       | `info`                  | Log verbosity (`journalctl --user -u utter-daemon` on Linux; `log stream --process utter` on macOS). |

Precedence: **CLI flag > env var > config file > default.**

## PTT key aliases

`utter set-key` picks the nicer form automatically. If you're editing `config.toml` by hand, these names are accepted:

- **Modifiers:** `leftmeta rightmeta leftctrl rightctrl leftalt rightalt leftshift rightshift`
- **Lock keys:** `capslock scrolllock numlock`
- **Navigation + utility:** `home end pageup pagedown insert menu printscreen pause`
- **Function keys:** `f1`..`f24`
- **Apple aliases:** `rightcmd leftcmd rightoption leftoption` (map to `rightmeta` etc.)
- **Anything else:** pass the raw evdev code as digits (as a quoted string in TOML), e.g. `key = "70"` for scroll lock, `key = "194"` for F24.

On macOS, `rightmeta` / `rightcmd` → Right Cmd, `rightoption` / `rightalt` → Right Option, `capslock` → Caps Lock, function keys → F1..F24.

After hand-editing the config, restart: `systemctl --user restart utter-watcher` (Linux) or Cmd+Q + relaunch (macOS).

`utter set-key --dry-run` detects the key and reports the canonical name without writing the config — useful for figuring out how a remapped key (QMK / VIA / Karabiner) shows up on your system.

## Recording indicator

While you're holding the key, your desktop's standard **microphone-in-use icon** (the small mic that appears in KDE's system tray, GNOME's top bar, the macOS menu bar Control Center, etc.) is the intentional visual cue that utter is listening. When you release the key, recording stops and the icon disappears. utter doesn't ship a custom overlay widget on purpose — the system indicator is already there, already correct, and doesn't draw anything ugly over your screen.

## Services (Linux only)

utter runs as two systemd user services on Linux:

- **`utter-daemon`** — loads the model, records audio, transcribes, emits text.
- **`utter-watcher`** — watches `/dev/input` for the PTT key and tells the daemon when to start and stop.

macOS runs everything in one `LSUIElement` menu-bar process — no systemd, no separate watcher. See [Architecture](ARCHITECTURE.md).
