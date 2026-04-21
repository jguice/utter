# macOS port — design notes

Starting brief for a future session that will pick this up cold. Written after shipping v0.2.0 on Linux; nothing here is code yet.

## What utter is (and what the port must preserve)

Push-to-talk voice dictation. Hold a key, speak, release — the transcription pastes into the focused text field. Local Parakeet ONNX model, ~100-200 ms per utterance on Apple Silicon. Two long-running processes (a daemon that owns the model + audio; a watcher that listens for the PTT key).

The value proposition is **speed and lightness**. Wispr Flow and similar cloud-based tools run ~300-500 ms latency including network. utter on-device is 2-5x faster and has no data leaving the machine. The port must preserve this — if it ends up slower than Wispr Flow, the whole point is gone.

## What already works on macOS

- **`transcribe-rs` + `ort` (ONNX Runtime)**: both target macOS (Apple Silicon + Intel). The `cpuid_info` warning we see on Asahi probably won't appear — onnxruntime recognizes Apple Silicon natively on macOS.
- **`clap`, `tokio`, `anyhow`, `dirs`**: cross-platform. `dirs::data_dir()` returns `~/Library/Application Support/` on macOS, which is where the model should live.
- **Pure logic**: `cleanup_transcription`, `parse_key_name`/`canonical_name_for` (keycode semantics are different on macOS, so these need revisiting), `selections_to_write`, `write_watcher_override_at`.

## What has to be replaced

Each of these is a platform boundary — something that talks to the OS. Before any macOS code is written, `src/main.rs` should be split so these are behind traits and the Linux implementations move to `src/platform/linux/*.rs`. The macOS implementations then land in `src/platform/macos/*.rs` and compile-gate with `#[cfg(target_os = "macos")]`.

### 1. Key watcher

**Linux today:** `evdev` crate reads `/dev/input/event*`. Enumerate keyboards, watch for press/release of the configured key.

**macOS:** `CGEventTap` at `kCGHeadInsertEventTap` + `kCGSessionEventTap`, listening for `kCGEventKeyDown` and `kCGEventKeyUp`. The tap runs on a `CFRunLoop` thread. Crates to use: `core-graphics` + `core-foundation` (both well-maintained).

**Permission:** macOS prompts for **Input Monitoring** (System Settings → Privacy & Security → Input Monitoring) the first time the tap is installed. One-time grant. Accepted cost — Wispr Flow and every other PTT tool has the same prompt.

**Keycodes:** macOS uses its own virtual-keycode space (kVK_* constants from `Carbon.h`), not evdev codes. The set-key interactive picker needs a macOS-native keycode map. Right Command = `kVK_RightCommand` = 0x36, etc. The canonical-name abstraction stays the same; the number underneath changes.

### 2. Keystroke injection (paste trigger)

**Linux today:** `ydotool` spawned as subprocess, synthesizes Cmd+V or Shift+Insert via uinput.

**macOS:** `CGEventCreateKeyboardEvent` from the `core-graphics` crate — synthesize Cmd+V directly (macOS uses Cmd+V universally, no primary-selection Shift+Insert dance). Four events: modifier down, key down, key up, modifier up. Post via `CGEventPost(kCGHIDEventTap, ...)`.

**Permission:** **Accessibility** (System Settings → Privacy & Security → Accessibility). One-time grant, same as Wispr Flow.

**No subprocess needed** — we call CoreGraphics directly. Drops the ydotool dependency entirely on macOS.

### 3. Clipboard

**Linux today:** `wl-copy` writes primary selection by default; `UTTER_CLIPBOARD=1` opts in to the regular clipboard.

**macOS:** no primary selection exists — there's only `NSPasteboard.general`. The "don't pollute the clipboard" feature loses its Linux meaning.

**The trick for macOS** (user's insight): write to NSPasteboard with a **custom pasteboard type** — e.g. `com.utter.dictation` — alongside (or instead of) the standard `NSPasteboardTypeString`. Well-behaved clipboard managers (Paste, Maccy, Alfred) can be configured to ignore that type, so dictations don't show up in clipboard history. Users who want dictations in history simply don't filter.

Implementation: use the `objc` or `objc2` crate to call `[NSPasteboard generalPasteboard]` → `declareTypes:@[NSPasteboardTypeString, "com.utter.dictation"] owner:nil` → `setString:forType:` twice. Cmd+V reads `NSPasteboardTypeString` regardless of the presence of other types.

Document the custom type and recommended clipboard-manager filter in the macOS README.

### 4. Audio capture

**Linux today:** subprocess `arecord` writes a WAV to `/tmp/utter-*.wav`; we read the WAV after stop.

**macOS:** use the `cpal` crate for CoreAudio capture. Opens the default input device, streams PCM samples into a ring buffer. No subprocess, no WAV file, no disk I/O — just push samples directly to Parakeet when the user releases the key. Latency win.

Probably worth converting Linux to cpal too as a follow-up; unifies the codepath. But not a prerequisite — the port can keep Linux on arecord and start macOS on cpal.

### 5. Notifications

**Linux today:** `notify-send` subprocess (libnotify).

**macOS:** `osascript -e 'display notification "..." with title "utter"'` is the zero-dependency path. Or native `UNUserNotificationCenter` via objc2 — nicer but adds a bind for a feature that's already opt-in via `show_notifications = true`. Go with osascript first; upgrade later if it feels janky.

### 6. Service management

**Linux today:** systemd user services (`utter-daemon.service`, `utter-watcher.service`) at `~/.config/systemd/user/`, enabled via `systemctl --user enable --now`.

**macOS:** launchd agents (`.plist` files) at `~/Library/LaunchAgents/com.utter.daemon.plist` and `~/Library/LaunchAgents/com.utter.watcher.plist`, enabled via `launchctl load`. Plist structure: `Label`, `ProgramArguments`, `RunAtLoad`, `KeepAlive`. StandardOutPath + StandardErrorPath for logs (there's no journalctl equivalent — logs go to files or `log stream --predicate ...`).

Example minimal plist:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.utter.daemon</string>
    <key>ProgramArguments</key>
    <array><string>/usr/local/bin/utter</string><string>daemon</string></array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>EnvironmentVariables</key>
    <dict><key>RUST_LOG</key><string>info</string></dict>
</dict>
</plist>
```

### 7. Socket path

**Linux:** `$XDG_RUNTIME_DIR/utter.sock`.

**macOS:** there's no XDG_RUNTIME_DIR. Use `$TMPDIR/utter.sock` (which is a user-specific temp dir on macOS, e.g. `/var/folders/.../T/utter.sock`). Unix domain sockets work identically.

### 8. set-key semantics

The Linux set-key feature writes a systemd drop-in. On macOS, it writes a new plist with the chosen keycode in `ProgramArguments` and reloads via `launchctl bootout` + `launchctl bootstrap`. Same external CLI (`utter set-key`), different internal mechanics. Feature-gate the implementation.

## Architecture refactor (must happen first)

`src/main.rs` is 1100+ lines. Before adding a second platform, split:

```
src/
  main.rs                    # CLI dispatch only
  daemon.rs                  # socket server, state machine
  watcher.rs                 # reads key events from `platform`
  set_key.rs                 # interactive picker
  cleanup.rs                 # text post-processing (already pure)
  platform/
    mod.rs                   # traits: AudioSource, KeyWatcher, TextEmitter, ServiceManager
    linux/
      mod.rs
      evdev_watcher.rs
      arecord_audio.rs
      ydotool_emitter.rs
      systemd_service.rs
    macos/
      mod.rs
      cgevent_watcher.rs     # CGEventTap
      coreaudio_audio.rs     # cpal wrapper
      cgevent_emitter.rs     # CGEventCreateKeyboardEvent
      launchd_service.rs
```

Traits roughly:

```rust
trait AudioSource {
    async fn start(&mut self) -> Result<()>;
    async fn stop(&mut self) -> Result<Vec<i16>>;  // returns 16kHz mono PCM
}

trait TextEmitter {
    async fn emit(&self, text: &str) -> Result<()>;  // writes clipboard + synthesizes paste
}

trait KeyWatcher {
    async fn watch(&self, target: KeyCode, tx: mpsc::Sender<KeyEvent>) -> Result<()>;
}
```

Time budget for the refactor: ~1 day. Do it on Linux first (no behavior change), land it, then start the macOS impl as a second platform.

## Packaging

- **Homebrew formula** (public tap or `brew tap jguice/utter`). Builds from a release tarball or a signed binary. Users get `brew install utter` + `brew services start utter`.
- **`.pkg` installer** for non-brew users. Can be built with `pkgbuild` + `productbuild`. Signing: Apple Developer ID required to avoid Gatekeeper warnings ($99/year). Notarization required for Gatekeeper clearance.
- **Universal binary** (arm64 + x86_64 in one file): `cargo build --release --target aarch64-apple-darwin && cargo build --release --target x86_64-apple-darwin && lipo -create ...`. Or ship separate binaries and let the installer pick.

For a v0.1 macOS release, homebrew-only is probably enough. `.pkg` + signing comes later.

## CI

GitHub Actions offers `macos-14` (arm64) and `macos-13` (x86_64) runners. Add matrix entries to `.github/workflows/release.yml` alongside the existing Linux ones. Tests + clippy on macOS runners in `ci.yml` too.

## Order of operations

1. **Refactor main.rs into platform modules on Linux** (~1 day, no behavior change, land standalone).
2. **macOS audio via cpal** (~1 day). Easiest piece, validates the toolchain works.
3. **macOS text emitter (CGEvent paste)** (~1 day). Includes Accessibility permission flow + user-facing first-run guidance.
4. **macOS key watcher (CGEventTap)** (~2-3 days). Hardest piece: event tap teardown, thread interaction with tokio, Input Monitoring permission.
5. **macOS pasteboard with custom type** (~0.5 day).
6. **launchd services + set-key macOS variant** (~1 day).
7. **Homebrew formula + CI matrix** (~1 day).
8. **Docs + first-run UX** (~0.5-1 day).

**Total: ~2-3 weeks of focused work** for a shippable v0.3.0 or v0.4.0 macOS release.

## Risks & unknowns

- **CGEventTap thread model** interacting with tokio: the event tap runs on a CFRunLoop thread; getting events to a tokio task cleanly (without blocking) needs care. `tokio::sync::mpsc` + `spawn_blocking` for the runloop is the likely shape, but unproven.
- **TCC prompt UX on first run:** we can't prompt ahead of time. The first call to `CGEventTapCreate` triggers the Input Monitoring prompt; the first `CGEventPost` triggers Accessibility. Needs clear first-run instructions and maybe a `utter preflight` subcommand that triggers both prompts explicitly with guidance.
- **Apple Silicon vs Intel:** only Apple Silicon is likely to get the full 100-200 ms performance target. Intel Macs will be slower via ORT, might be acceptable, needs measurement.
- **System Integrity Protection (SIP)** doesn't block our tools, but macOS in general may add surprises (rate-limiting on pasteboard changes, focus races with CGEventPost, etc.). Expect a few whack-a-mole bugs after initial testing.

## Out of scope for the first macOS release

- Per-app paste method (analog of the Linux backlog item). macOS is Cmd+V everywhere — simpler.
- Voice activity detection / streaming mode (same as Linux: future).
- iOS port (entirely different platform — iOS doesn't allow global hotkeys at all).
- Supporting macOS 12 or older — target 13+ (when Input Monitoring API stabilized).

## One-line kickoff for the next session

> Read `docs/MACOS_PORT.md` end-to-end, then start with step 1 (refactor `src/main.rs` into platform modules on Linux, no behavior change). Land that as its own PR before touching any macOS code.
