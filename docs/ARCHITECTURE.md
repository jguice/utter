# Architecture

## Linux

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

## macOS

Single-process `LSUIElement` menu-bar app (no systemd, no ydotool). Audio via cpal + CoreAudio; PTT via CGEventTap; paste via NSPasteboard + synthesized Cmd+V through CGEventPost; permissions flow through a first-run onboarding window that drives the native TCC prompts.
