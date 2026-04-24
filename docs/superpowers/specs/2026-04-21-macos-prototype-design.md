# macOS prototype — design

Scope for the first working macOS build. Authoritative long-term design lives in [`../../MACOS_PORT.md`](../../MACOS_PORT.md); this file only captures the prototype delta and the intentional shortcuts.

## Goal

End-to-end happy path on Apple Silicon: run `utter daemon` and `utter watcher` in two terminals, hold the configured PTT key, speak, release, and have the transcription pasted into the focused text field. Plus a working `utter set-key` that captures whatever physical key the user presses and persists it.

Success = we can dictate a sentence into a text field on macOS. Latency and polish are follow-ups.

## In scope

1. **Audio capture** via `cpal` (CoreAudio). Stream PCM into a ring buffer; flush to Parakeet on key release. No subprocess, no WAV file.
   - cpal's default input on macOS is typically 48 kHz `f32`. Parakeet wants 16 kHz `i16`. Prototype approach: prefer a 16 kHz-capable input config if available, else downsample in-process via `rubato` (or naive linear decimation for a first pass — quality to be measured).
2. **Key watcher** via `CGEventTap` at `kCGSessionEventTap` + `kCGHeadInsertEventTap`, listening for `kCGEventKeyDown`, `kCGEventKeyUp`, and `kCGEventFlagsChanged`. Modifier keys (Cmd/Option/Ctrl/Shift, left and right) don't fire KeyDown/KeyUp — they come through `FlagsChanged`. To derive press vs release from a `FlagsChanged` event: read `kCGKeyboardEventKeycode` (the specific kVK_* code that changed) and diff the new flags mask against the previously-seen mask — if the bit corresponding to that keycode is now set, it's a press; if cleared, a release. Left vs right is distinguishable (e.g. `kVK_RightCommand = 0x36` vs `kVK_Command = 0x37`).
   - The callback must also handle `kCGEventTapDisabledByTimeout` and `kCGEventTapDisabledByUserInput` by calling `CGEventTapEnable(tap, true)` to re-arm. This is the single most common CGEventTap footgun.
   - Tap lifecycle: `CGEventTapCreate` → wrap returned `CFMachPort` in a `CFRunLoopSource` via `CFMachPortCreateRunLoopSource` → `CFRunLoopAddSource` on the runloop thread → `CFRunLoopRun()`. Teardown: invalidate the port and remove the source on drop.
   - Event tap runs on a dedicated `std::thread` owning a `CFRunLoop`; events forwarded to tokio via `tokio::sync::mpsc::UnboundedSender`. Keyboard event rate is trivial — no backpressure concerns.
3. **Text emitter** via `CGEventCreateKeyboardEvent` (Cmd down, V down, V up, Cmd up) + `CGEventPost`. Use `kCGSessionEventTapLocation` (aka `kCGAnnotatedSessionEventTap`) so the synthesized events are delivered into the current session after session-level taps, matching what a user keypress looks like. No ydotool.
4. **Clipboard** via `NSPasteboard.general` (`objc2`). Call `declareTypes:[NSPasteboardTypeString, "com.utter.dictation"] owner:nil` then `setString:forType:` for both types. Cmd+V reads `NSPasteboardTypeString` regardless; users who run Paste/Maccy/Alfred can add a filter rule for `com.utter.dictation` to keep dictations out of clipboard history. (Pulled back in from the deferred list — it's ~5 lines beyond the plain write.)
5. **`utter set-key` on macOS.** Installs a CGEventTap in listen-only mode (`CGEventTapOptions::ListenOnly`), captures the first key/modifier event, maps the keycode to a canonical name if known (hex numeric string fallback), and writes it via the existing `Config::load_or_migrate(...).with_key(...).save_to(Config::default_path()?)` path — the same helper Linux's `set-key` uses today (see `src/main.rs:961`). Watcher reads `key` from config and watches for that keycode.
   - **Listen-only does not exempt the tap from TCC.** `CGEventTapCreate` still requires Input Monitoring whether the tap is active or listen-only. First-run UX story: user runs `utter set-key`, macOS shows the Input Monitoring prompt, `CGEventTapCreate` returns NULL, `set-key` exits with an explanatory message. User grants Input Monitoring, re-runs `set-key`, picker works. This is the same first-prompt experience every PTT tool has.
   - No `launchctl bootout`/`bootstrap` reload — user restarts the watcher manually.
6. **Socket path** on macOS: `$TMPDIR/utter.sock`. Resolution lives in the existing `socket_path()` helper (`src/main.rs:74`), which both daemon and watcher call — cfg-gate the body so they stay in agreement automatically. `$TMPDIR` on macOS resolves to something like `/var/folders/ab/xxxxxx/T/` — roughly 50 characters plus `utter.sock` is well under the BSD `sun_path` limit of 104 bytes, but assert the length at bind time to fail loudly rather than silently truncate.
7. **Model path.** Teach `scripts/download-model.sh` about macOS with a `case "$OSTYPE" in darwin*) ... ;; *) ...` switch (a bare `[ "$OSTYPE" = ... ]` test is unreliable across shells; `case` matches the script's existing `set -euo pipefail` posture). Darwin branch sets `DEST="$HOME/Library/Application Support/utter/models/parakeet-tdt-0.6b-v3-int8"`; non-Darwin keeps the current `${XDG_DATA_HOME:-$HOME/.local/share}/utter/...` value. Every downstream use of `$DEST` must be double-quoted (`mkdir -p "$DEST"`, `cd "$DEST"`, etc.) because the macOS path contains a literal space in "Application Support". Match the existing script's behavior of overwriting `DEST` unconditionally (Linux honors `$XDG_DATA_HOME` only, not a pre-set `$DEST`); don't introduce a new `DEST="${DEST:-...}"` override path. The daemon side already matches: `default_model_dir()` at `src/main.rs:80` is `dirs::data_dir()?.join("utter/models/parakeet-tdt-0.6b-v3-int8")`, which on macOS joins `~/Library/Application Support/` with the same suffix the script writes. Rust side and shell side must stay in lockstep — the canonical literal is `utter/models/parakeet-tdt-0.6b-v3-int8`, appearing in both `scripts/download-model.sh` and `default_model_dir()`; change one, change the other. The `--model` CLI override remains as a fallback (`src/main.rs:33,98`).
8. **Notifications.** Keep the existing `notify()` abstraction in `src/main.rs:586` (gated by `config.show_notifications`). cfg-gate the body: on Linux, `notify-send` as today; on macOS, spawn `osascript` with `-e` and a single AppleScript string. The AppleScript reaching `osascript` must look like `display notification "<body>" with title "<summary>"`, where `<body>` and `<summary>` are AppleScript string literals — so the escape order is: replace `\` with `\\` first, then replace `"` with `\"`, applied to both fields. Reversing the order double-escapes the backslashes. Because we're invoking `osascript` via `tokio::process::Command::arg(...)`, the args are passed directly as `execve` argv without a shell in between — no additional shell quoting on top of the AppleScript escaping. Current call sites pass fixed literals, but escape anyway so future dynamic content doesn't break silently. Subprocess call, no extra deps.
9. **CLI surface is unchanged.** No new subcommands for macOS. `utter daemon`, `utter watcher`, and `utter set-key` behave identically to Linux from the user's point of view; only the internals differ.
10. **Eager Microphone prompt.** The daemon opens a cpal input stream at startup and immediately stops it, forcing macOS to prompt for Microphone before the user ever presses the PTT key. Without this, the prompt fires mid-dictation on first run and the first utterance is silently lost.
    - `build_input_stream(...)` alone does not trigger the TCC prompt on macOS 13+; CoreAudio only asks for permission once it actually starts pulling samples. So the sequence is: `build_input_stream` → `play()` → wait for one callback (or a short timeout, ~200 ms) → `pause()` then drop the stream (cpal has no public `stop()`; dropping is the canonical shutdown). A single callback is enough to make the TCC check fire.
    - Order this *before* model load, not after. Model load can take 5-30 seconds on first startup (ONNX graph optimization, INT8 weight mmaps); if the Microphone prompt is gated behind that delay, the daemon looks hung during the wait. Sequence: eager mic check → Microphone prompt (if needed) → model load → socket bind → serve.
    - Denial detection: cpal doesn't surface a distinct `PermissionDenied` variant — a denied grant shows up either as a `BuildStreamError`, as an error delivered through the stream's error callback, or simply as silence (no callback within the timeout). Treat any of those three as "no audio available"; log a clear message and continue to socket bind anyway. The daemon is still useful for `set-key` flow verification without audio, and `tccutil reset Microphone <binary path>` lets the user retry without a reinstall.
11. **Linux-only code paths are `#[cfg]`-gated, not mutated.** The existing `parse_key_name` / `canonical_name_for` / `notify()` bodies and the `evdev`-backed watcher stay on the Linux path, annotated `#[cfg(target_os = "linux")]`. The macOS counterparts are sibling items annotated `#[cfg(target_os = "macos")]`. No existing function is extended with runtime OS branching. This keeps Linux build behavior unchanged and makes the eventual trait refactor a mechanical move rather than an untangling. `kVK_*` constants (e.g. `kVK_RightCommand = 0x36`, `kVK_Command = 0x37`) aren't re-exported by `core-graphics`; define them as local `const`s in `src/macos.rs` rather than pulling in a Carbon binding.

## Out of scope (deferred to follow-ups)

- Full platform-trait refactor of `src/main.rs`. MACOS_PORT.md recommends doing this first ("Before any macOS code is written…"); the prototype deliberately defers it to compress the feedback loop. We'll add `src/macos.rs` with the platform pieces and cfg-gate at call sites. A proper `platform::{linux,macos}` split comes after we know the prototype works.
- `launchd` services / `.plist` files. Run manually from two terminals.
- `launchctl bootout`/`bootstrap` reload after `set-key`. User restarts the watcher manually.
- Native `UNUserNotificationCenter` integration. `osascript` is good enough for the prototype.
- Homebrew formula, `.pkg` installer, signing, notarization, universal binary.
- GitHub Actions macOS matrix entries.
- Intel Mac performance measurement.

## Architecture (prototype)

```
src/
  main.rs         # existing: CLI dispatch, daemon state machine, watcher loop
  config.rs       # existing, unchanged
  macos.rs        # NEW: CGEventTap watcher, cpal audio, CGEvent emitter, NSPasteboard write, set-key picker
```

`main.rs` gets `#[cfg(target_os = "linux")]` / `#[cfg(target_os = "macos")]` gates at the call sites that currently spawn `arecord`, call `ydotool`, read `evdev`, or write `wl-copy`. The macOS counterparts live in `macos.rs` behind a small internal module API — not a polished trait-based abstraction yet.

## Dependencies to add

- `cpal` — CoreAudio capture.
- `core-graphics` + `core-foundation` — CGEventTap, CGEventCreateKeyboardEvent, CFRunLoop.
- `objc2` + `objc2-app-kit` (or `objc2-foundation`) — NSPasteboard.

All gated behind `[target.'cfg(target_os = "macos")'.dependencies]` in `Cargo.toml` so Linux builds stay unchanged.

## `key` semantics across platforms

The `Config.key` field stays a `String`. Each platform owns its own alias/parse logic:
- Linux: existing `parse_key_name` / evdev-code path.
- macOS: new helper maps a small set of canonical names (`rightcommand`, `leftcommand`, `rightoption`, `leftoption`, `f13`–`f20`, `capslock`, letters, etc.) to macOS virtual keycodes (kVK_*). Unknown names that parse as hex or decimal numbers are treated as raw keycodes. Unknown non-numeric names are a config error at watcher startup.

`set-key` writes whichever form it can produce: canonical name if the captured code has one, else `0xNN` hex. Either round-trips through the parser.

## Risks, in priority order

1. **CGEventTap ↔ tokio threading.** The tap's callback runs on a `CFRunLoop` thread. Shape and caveats are in "In scope" #2. If this turns out to be fragile, that's where we spend time; everything else is comparatively mechanical.
2. **Three TCC prompts, not two.** On first run the user will see:
   - **Input Monitoring** — triggered by `CGEventTapCreate`. First hit by whichever of `utter set-key` or `utter watcher` the user runs first.
   - **Accessibility** — triggered by the first `CGEventPost`. Hit the first time the daemon tries to paste.
   - **Microphone** — triggered by cpal the first time the audio stream is started. Hit when the daemon loads. (Not previously called out; this is a real third prompt.)
   Each grant typically requires quitting and relaunching the specific process before the grant takes effect. In a dev loop, every `cargo build` that relinks the binary invalidates the prior TCC grant and re-prompts — TCC keys on the binary's signing identity, which for an unsigned `target/debug/utter` is effectively its on-disk hash. Accepted friction for the prototype; mitigated later by signing a proper bundle.
   Also: `cargo run -- daemon` invoked from a terminal inherits the terminal app's (iTerm/Terminal/Ghostty) grants for some checks — a known macOS dev-flow wart that occasionally surfaces as "the wrong app asked for permission."
3. **Modifier keys as PTT.** Covered in "In scope" #2 — FlagsChanged with mask-diff for press/release.

## How we'll know it's working

Manual test on the author's Apple Silicon Mac. Any TCC grant requires quitting the prompted process and re-running it:
1. `scripts/download-model.sh` — drops the model at `~/Library/Application Support/utter/models/parakeet-tdt-0.6b-v3-int8/`.
2. `cargo run -- daemon`. Microphone prompt appears before model load. The daemon pauses ~200 ms waiting for the first cpal callback (or the short denial timeout); that pause is expected, not a hang. Grant and re-run. Accessibility prompt fires on first paste attempt; grant and re-run.
3. `cargo run -- set-key`, press F13 (or the desired key). Grant Input Monitoring if prompted; re-run `set-key`. Confirm the file at `Config::default_path()` now has `key = "f13"` (or the hex fallback).
4. `cargo run -- watcher` in a third terminal. Input Monitoring should already be granted from step 3, so no new prompt. If for any reason the watcher re-prompts, grant and re-run the watcher.
5. Focus a text field, hold the configured key, say "hello world", release. Text appears.

**Linux regression guardrail.** As a hard acceptance criterion alongside the manual macOS path above, on the author's Linux box: `cargo build --release --locked` and `cargo test --release` must still pass, and a quick end-to-end dictation smoke test under systemd must still work. Any Linux regression from cfg-gating blocks the prototype.

No automated macOS tests in this iteration — CI matrix comes later.
