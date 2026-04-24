# Configuration

## Usage

Hold the push-to-talk key, speak, release. The transcription pastes into whichever field has focus.

- **Default key:** Right Cmd on Apple keyboards, Right Super on others (`rightmeta` in kernel-event terms). To change it, run `utter set-key` and press the key you'd rather use.
- **Visual cue:** while you hold the key, your desktop's standard **microphone-in-use icon** lights up in the tray / status bar. That's the intentional "utter is listening" indicator — no custom overlay.
- **Output:** text is written to the primary selection and auto-pasted via Shift+Insert into the focused window. The regular clipboard is deliberately left untouched so whatever you last copied is preserved.

## Services (Linux)

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

## Env var overrides

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

## Change the push-to-talk key

Easiest: let utter detect the key for you.

```bash
utter set-key
```

Press and hold the key you want, then release. utter reports what it detected (e.g. `Detected: rightmeta (code 126)`), confirms the press **and** release both fired cleanly (so you know hold-to-talk will actually work on that key), and writes the new key to `~/.config/utter/config.toml`. Pass `--dry-run` to just detect without saving.

That covers both "what evdev name does this key have?" and "can utter actually read this key's events on my system?" in one step — useful if you remapped a key in your keyboard firmware (QMK, VIA, Karabiner-on-macOS-sibling-tool) or via a desktop utility, and you want to confirm it shows up as something usable.

## Recording indicator

While you're holding the key, your desktop's standard **microphone-in-use icon** (the small mic that appears in KDE's system tray, GNOME's top bar, the macOS menu bar, etc.) is the intentional visual cue that utter is listening. When you release the key, recording stops and the icon disappears. utter doesn't ship a custom overlay widget on purpose — the system indicator is already there, already correct, and doesn't draw anything ugly over your screen.

## Manual override (if you'd rather)

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
