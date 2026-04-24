# macOS Settings UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose PTT key change + all bool config settings through the macOS menu bar, wired to the existing `Config` TOML, with "quit and relaunch to apply" as the apply step. No hot-reload yet.

**Architecture:** All config-mutation logic lives in `src/config.rs` as pure `with_*` builder methods — shared, Linux benefits for free. AppKit menu items in `src/macos_ui.rs` invoke those builders + `Config::save_to()`. The "Change PTT Key…" item spawns `open utter.app --args set-key` as a subprocess (not an in-process picker) so the running watcher doesn't fight with the picker over the CGEventTap. Feedback is an `osascript` notification telling the user to Quit + reopen.

**Tech Stack:** Rust, `Config` / `toml` / `serde` (shared); `objc2` + `objc2-app-kit` (macOS UI only); `tokio::process::Command` for subprocess spawn.

---

## Revisions after review (2026-04-23)

These supersede the corresponding code in Tasks 3 + 4. Implement these versions, not the originals.

- **Task 3 / `flip_config_flag`:** use a closure-based signature `(flag_name, read_fn, write_fn)` instead of a string-keyed `current_flag` lookup table. Drop the `current_flag` function entirely. Compiler enforces the field-to-builder link, eliminating the silent "unknown name → false → flips to true" footgun.
- **Task 3 toggle handlers:** after a successful save, call `sender.setState(...)` on the clicked `NSMenuItem` so the checkmark updates live (no Cmd+Q required to see the new state). The "feature itself doesn't apply until restart" contract still holds for the underlying behavior, but the UI stops lying.
- **Task 3 menu construction:** for each toggle, check whether the corresponding `UTTER_*` env var is set (via `utter_env_snapshot`). If set: `setEnabled(false)` and append " (env override)" to the title. The checkmark still reflects the effective (env-overridden) value. Rationale: env > file precedence is intentional (matches `Config::with_env_overrides`), but a non-responsive toggle without explanation is confusing.
- **Task 4 spawn:** use `std::process::Command::new("open").args([bundle_path, "--args", "set-key", "--from-menu"])` instead of `std::env::current_exe()`. Direct `current_exe()` spawn risks losing LaunchServices-managed bundle attribution for TCC (per `memory/macos_tcc_unsigned_cli.md`); `open` keeps the child attributed to `com.utter.app` so Input Monitoring grants carry over. The bundle path comes from `objc2_foundation::NSBundle::mainBundle().bundlePath()` on the main thread, captured before `NSApplication::run()` and passed into `spawn_set_key_picker`.
- **Task 4 user-facing messaging:** drop the pre-spawn osascript notification ("Press and hold your new key…"). The child owns its messaging.
- **Task 4 (new): extend `SetKey` CLI with `--from-menu` flag.** When set on macOS, after a successful save the picker displays an NSAlert modal: title "PTT key changed", body "Saved as `<key>`. Quit utter (Cmd+Q in the menu bar) and reopen to apply." Single OK button. CLI-only invocations (no flag) keep the existing log-only behavior.
- **Risk register cleanups:** "Menu state drift" is resolved by the live `setState`. "Subprocess PTT picker attribution" is resolved by routing through `open` (LaunchServices).
- **Watcher-vs-picker keypress collision (new acknowledgement):** both the watcher and the picker use `ListenOnly` event taps, so they coexist without consuming each other's events. However, while the picker is active the running watcher *also* sees keypresses; if the user happens to press the current PTT key during the pick, an unsolicited dictation kicks off in the background. Accepted for v1 — rare and low-stakes. Followup (separate plan): add a cancel affordance (e.g. Escape) to the picker so users don't have to press the existing PTT key to abort.

---

## File Structure

- `src/config.rs` — add four `with_*` builder methods mirroring the existing `with_key`. Pure, shared.
- `src/macos_ui.rs` — new menu items (toggles + change-PTT-key + info note). Extends the existing `UtterMenuTarget` Obj-C class with new action selectors.
- `src/main.rs` — pass the full `Config` value to `run_status_bar_app` instead of just the PTT key string, so the menu reflects current bool state.
- Nothing new in `src/macos.rs`, `scripts/`, `Cargo.toml`, or `docs/MACOS_PORT.md`.

---

## Task 1: Config builders for bool flags (shared)

**Files:**
- Modify: `src/config.rs:133-137` (add four methods next to `with_key`)
- Modify: `src/config.rs` test module (add round-trip tests for each)

- [ ] **Step 1: Write the failing tests**

Append inside `#[cfg(test)] mod tests` in `src/config.rs`:

```rust
#[test]
fn with_flag_methods_replace_only_their_field() {
    let base = Config::default();

    let flipped = base
        .clone()
        .with_auto_paste(false)
        .with_write_clipboard(true)
        .with_filter_filler_words(false)
        .with_show_notifications(true);

    assert!(!flipped.auto_paste);
    assert!(flipped.write_clipboard);
    assert!(!flipped.filter_filler_words);
    assert!(flipped.show_notifications);
    // Unrelated fields preserved.
    assert_eq!(flipped.key, base.key);
}

#[test]
fn with_flag_methods_chain_with_with_key() {
    let c = Config::default()
        .with_key("f13")
        .with_auto_paste(false);
    assert_eq!(c.key, "f13");
    assert!(!c.auto_paste);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib config::tests::with_flag_methods 2>&1 | tail -20`
Expected: compilation error (`no method named with_auto_paste`, etc.) — this counts as the failing state since the method doesn't exist yet.

- [ ] **Step 3: Implement the four builders**

Insert immediately after the existing `with_key` method in `src/config.rs` (around line 137), inside `impl Config`:

```rust
/// Return a copy with `auto_paste` replaced. Pure.
pub fn with_auto_paste(mut self, v: bool) -> Self {
    self.auto_paste = v;
    self
}

/// Return a copy with `write_clipboard` replaced. Pure.
pub fn with_write_clipboard(mut self, v: bool) -> Self {
    self.write_clipboard = v;
    self
}

/// Return a copy with `filter_filler_words` replaced. Pure.
pub fn with_filter_filler_words(mut self, v: bool) -> Self {
    self.filter_filler_words = v;
    self
}

/// Return a copy with `show_notifications` replaced. Pure.
pub fn with_show_notifications(mut self, v: bool) -> Self {
    self.show_notifications = v;
    self
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib config::tests 2>&1 | tail -10`
Expected: all tests in the module (old + new two) pass.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add with_auto_paste / with_filter_filler_words / with_write_clipboard / with_show_notifications builders

Mirrors the existing with_key pattern. Pure functions, shared across
platforms. Groundwork for macOS menu bar toggles; Linux can reuse them
the moment anyone builds a Linux config UI.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Pass full Config into status bar (macOS)

**Files:**
- Modify: `src/macos_ui.rs:19-21` (signature) and throughout (label construction)
- Modify: `src/main.rs` (`run_combined_with_menu_bar`, the call site) — around line 150 in the current layout

- [ ] **Step 1: Update the `run_status_bar_app` signature**

In `src/macos_ui.rs`, change:

```rust
pub fn run_status_bar_app(ptt_key: &str, config_path: &std::path::Path) -> Result<()> {
```

to:

```rust
pub fn run_status_bar_app(cfg: &Config, config_path: &std::path::Path) -> Result<()> {
```

Add the import at the top of `src/macos_ui.rs`:

```rust
use crate::Config;
```

Replace the `ptt_key` usage on the label line (search for `"PTT: {ptt_key}"`) with:

```rust
add_label(&menu, mtm, &format!("PTT: {}", cfg.key));
```

- [ ] **Step 2: Update the call site in main.rs**

In `src/main.rs`, inside `run_combined_with_menu_bar`, change:

```rust
let ptt_key = cfg.key.clone();
```

to nothing (remove that line — we pass the whole `cfg` now).

Change the final line from:

```rust
macos_ui::run_status_bar_app(&ptt_key, &config_path)
```

to:

```rust
macos_ui::run_status_bar_app(&cfg, &config_path)
```

- [ ] **Step 3: Build to verify**

Run: `cargo check 2>&1 | tail -5`
Expected: `Finished` with no errors.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs src/macos_ui.rs
git commit -m "refactor(macos-ui): pass full Config into status bar

Prep for bool-toggle menu items that need to read the current state of
every config field when building the menu. Replaces the single ptt_key
string arg with a &Config reference.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Bool toggle menu items (macOS)

**Files:**
- Modify: `src/macos_ui.rs` — `UtterMenuTarget` class block + menu construction

This task is large-ish — it adds four toggle items and one small helper. Each toggle does the same thing: click → load config → flip one field → save → notify "Quit and relaunch to apply."

- [ ] **Step 1: Extend the Obj-C target class with four toggle selectors**

In `src/macos_ui.rs`, inside the `declare_class!` block, add these methods next to `reset_tcc_grants` (inside `unsafe impl UtterMenuTarget`):

```rust
#[method(toggleAutoPaste:)]
fn toggle_auto_paste(&self, _sender: Option<&AnyObject>) {
    flip_config_flag("auto_paste", |c, v| c.with_auto_paste(v));
}

#[method(toggleWriteClipboard:)]
fn toggle_write_clipboard(&self, _sender: Option<&AnyObject>) {
    flip_config_flag("write_clipboard", |c, v| c.with_write_clipboard(v));
}

#[method(toggleFilterFillerWords:)]
fn toggle_filter_filler_words(&self, _sender: Option<&AnyObject>) {
    flip_config_flag("filter_filler_words", |c, v| c.with_filter_filler_words(v));
}

#[method(toggleShowNotifications:)]
fn toggle_show_notifications(&self, _sender: Option<&AnyObject>) {
    flip_config_flag("show_notifications", |c, v| c.with_show_notifications(v));
}
```

- [ ] **Step 2: Add the `flip_config_flag` helper**

Below the `impl UtterMenuTarget { fn new ... }` block in `src/macos_ui.rs`, add:

```rust
/// Load current config, flip one field via the provided builder, save,
/// and post a notification telling the user to restart. Each toggle
/// menu item calls through here so the logic is identical across them.
///
/// `flag_name` is only used for the "which flag changed" side of
/// logging; the actual read/write is whatever the caller does inside
/// the `apply` closure.
fn flip_config_flag<F>(flag_name: &str, apply: F)
where
    F: FnOnce(crate::Config, bool) -> crate::Config,
{
    let path = match crate::Config::default_path() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("toggle {flag_name}: default_path: {e:#}");
            return;
        }
    };
    let env = crate::config::utter_env_snapshot();
    let current = match crate::Config::load_or_migrate(&path, &env) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("toggle {flag_name}: load_or_migrate: {e:#}");
            return;
        }
    };
    // Read the flag's current value by round-tripping through the
    // builder: applying `apply(_, !x)` twice should swap it, but that
    // needs us to know the starting value. Simpler: serialize, parse
    // the field manually? No — we have the struct in hand. Each
    // caller closes over the right field, so we flip by applying the
    // builder with the NEGATION of the correct field. To keep the
    // helper generic, pass the current bool in explicitly from the
    // caller's perspective by using the field match table below.
    let new_value = !current_flag(&current, flag_name);
    let updated = apply(current, new_value);
    if let Err(e) = updated.save_to(&path) {
        log::warn!("toggle {flag_name}: save_to: {e:#}");
        return;
    }
    log::info!("toggle {flag_name} -> {new_value}; quit + relaunch to apply");
    notify_restart(flag_name, new_value);
}

/// Lookup table for current flag values so `flip_config_flag` can
/// compute the new value without needing a parallel getter-builder.
fn current_flag(cfg: &crate::Config, flag_name: &str) -> bool {
    match flag_name {
        "auto_paste" => cfg.auto_paste,
        "write_clipboard" => cfg.write_clipboard,
        "filter_filler_words" => cfg.filter_filler_words,
        "show_notifications" => cfg.show_notifications,
        _ => {
            log::warn!("current_flag: unknown flag name `{flag_name}`");
            false
        }
    }
}

/// Fire-and-forget osascript notification to remind the user that the
/// daemon isn't hot-reloading — they must Cmd+Q and reopen for the
/// toggle to take effect. Notifications don't block the menu target.
fn notify_restart(flag_name: &str, new_value: bool) {
    let title = format!("utter: {flag_name} = {new_value}");
    let body = "Saved. Quit (Cmd+Q) and reopen utter.app to apply.";
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        body.replace('\\', "\\\\").replace('"', "\\\""),
        title.replace('\\', "\\\\").replace('"', "\\\""),
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .spawn();
}
```

- [ ] **Step 3: Build the toggle menu items in the menu construction block**

In `src/macos_ui.rs`, inside `run_status_bar_app` `unsafe { ... }`, find the section between the "Config: ..." label and the "Reset TCC Grants (dev)" item. Insert the four toggles there (before the reset item, after the existing config path separator):

```rust
// Bool toggles. Each item's checkmark reflects the value from cfg
// captured at process start; clicking flips the field, saves, and
// notifies. Menu is rebuilt on next launch, so the checkmark
// updates then. Hot-reload is explicitly out of scope.
add_toggle(
    &menu,
    mtm,
    &*target,
    "Auto-paste after dictation",
    sel!(toggleAutoPaste:),
    cfg.auto_paste,
);
add_toggle(
    &menu,
    mtm,
    &*target,
    "Write to clipboard (not just selection)",
    sel!(toggleWriteClipboard:),
    cfg.write_clipboard,
);
add_toggle(
    &menu,
    mtm,
    &*target,
    "Filter filler words (uh, um, I I I…)",
    sel!(toggleFilterFillerWords:),
    cfg.filter_filler_words,
);
add_toggle(
    &menu,
    mtm,
    &*target,
    "Show notifications",
    sel!(toggleShowNotifications:),
    cfg.show_notifications,
);

menu.addItem(&NSMenuItem::separatorItem(mtm));
```

Important: the existing code has `let target = UtterMenuTarget::new(mtm);` defined right before the "Reset TCC Grants" item. Move that `let target = ...;` line UP to just before the toggle-insertion point, so all five menu items (four toggles + reset) share the same target instance. Also, `std::mem::forget(target)` stays where it is at the end.

- [ ] **Step 4: Add the `add_toggle` helper**

Below the existing `add_label` function in `src/macos_ui.rs`, add:

```rust
unsafe fn add_toggle(
    menu: &NSMenu,
    mtm: MainThreadMarker,
    target: &AnyObject,
    title: &str,
    selector: objc2::runtime::Sel,
    is_on: bool,
) {
    use objc2_app_kit::{NSControlStateValueOff, NSControlStateValueOn};
    let alloc = mtm.alloc::<NSMenuItem>();
    let item = NSMenuItem::initWithTitle_action_keyEquivalent(
        alloc,
        &NSString::from_str(title),
        Some(selector),
        &NSString::from_str(""),
    );
    item.setTarget(Some(target));
    item.setState(if is_on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    menu.addItem(&item);
}
```

- [ ] **Step 5: Enable the NSButton feature for NSControlStateValue**

Open `Cargo.toml` and confirm `"NSCell"` is in the objc2-app-kit features list. `NSControlStateValue` and its constants live in `NSCell.rs`. If missing, add:

```toml
objc2-app-kit = { version = "0.2", features = [
    # ... existing entries ...
    "NSCell",
] }
```

It was added in an earlier commit, so this is a confirm-only step.

- [ ] **Step 6: Build to verify**

Run: `cargo check 2>&1 | tail -15`
Expected: `Finished` with no errors (warnings about unused imports are fine and can be cleaned up).

- [ ] **Step 7: Build the release binary and repackage**

Run: `cargo build --release && ./scripts/make-bundle.sh`
Expected: `built target/release/utter.app (CFBundleIdentifier=com.utter.app, ...)`.

- [ ] **Step 8: Manual smoke test**

1. `killall utter ; open target/release/utter.app` — launch.
2. Click the menu bar icon — the four toggles should appear with checkmarks matching the current `config.toml`.
3. Click one toggle. Expected: osascript notification fires ("utter: auto_paste = false — Saved. Quit and reopen to apply."). Menu closes.
4. `cat ~/Library/Application\ Support/utter/config.toml | grep auto_paste` — expected: value flipped on disk.
5. Click the menu again. Checkmark still shows the OLD value (menu was built at process start). That's acceptable for v1 — hot-reload is deferred.
6. Cmd+Q, reopen. Checkmark now reflects the new value. Dictate and confirm the change actually took effect (e.g. disabled auto_paste = no Cmd+V synthesis).

- [ ] **Step 9: Commit**

```bash
git add src/macos_ui.rs Cargo.toml
git commit -m "feat(macos): bool setting toggles in the menu bar

Adds four toggle items for auto_paste, write_clipboard,
filter_filler_words, show_notifications. Each flips the bit via
Config::with_*() + save_to(), then posts an osascript notification
reminding the user to Cmd+Q and reopen to apply. Menu checkmarks
reflect the state captured at process start; they update on next
launch since we don't hot-reload.

All four toggles share the existing UtterMenuTarget Obj-C class plus
a flip_config_flag helper that handles the load -> flip -> save ->
notify sequence uniformly.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: "Change PTT Key…" menu item (macOS)

Runs `open target/release/utter.app --args set-key` in a subprocess so the running watcher doesn't intercept the pick. After the subprocess exits, the user must Cmd+Q + reopen to pick up the new key — same pattern as the bool toggles.

**Files:**
- Modify: `src/macos_ui.rs` — add one selector to `UtterMenuTarget`, one helper, one menu item.

- [ ] **Step 1: Add the selector to UtterMenuTarget**

Inside the `declare_class!` block's `unsafe impl UtterMenuTarget` in `src/macos_ui.rs`, add:

```rust
#[method(changePttKey:)]
fn change_ptt_key(&self, _sender: Option<&AnyObject>) {
    spawn_set_key_picker();
}
```

- [ ] **Step 2: Add the `spawn_set_key_picker` helper**

Below `notify_restart` (from Task 3), add:

```rust
/// Launch `utter set-key` as a separate process so the picker's event
/// tap doesn't race the running watcher for keyboard events. We spawn
/// the current executable with the set-key subcommand rather than
/// shelling out to `open --args` — using the binary directly keeps
/// the TCC attribution aligned (the bundle's CFBundleIdentifier).
fn spawn_set_key_picker() {
    let Ok(exe) = std::env::current_exe() else {
        log::warn!("spawn_set_key_picker: can't find current_exe");
        return;
    };
    match std::process::Command::new(&exe)
        .arg("set-key")
        .spawn()
    {
        Ok(child) => log::info!("spawned set-key picker pid={}", child.id()),
        Err(e) => log::warn!("spawn set-key: {e:#}"),
    }
    let body = "Press and hold your new key, then release. After saving, Cmd+Q utter and reopen.";
    let script = format!(
        "display notification \"{}\" with title \"utter: change PTT key\"",
        body.replace('\\', "\\\\").replace('"', "\\\""),
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .spawn();
}
```

- [ ] **Step 3: Add the menu item**

In `src/macos_ui.rs` inside `run_status_bar_app`, below the "Reset TCC Grants (dev)" item (or anywhere between the toggles and Quit; convention is to group it near the PTT label), add:

```rust
let change_ptt_item = {
    let alloc = mtm.alloc::<NSMenuItem>();
    NSMenuItem::initWithTitle_action_keyEquivalent(
        alloc,
        &NSString::from_str("Change PTT Key…"),
        Some(sel!(changePttKey:)),
        &NSString::from_str(""),
    )
};
change_ptt_item.setTarget(Some(&*target));
menu.addItem(&change_ptt_item);
```

Place it *after* the "PTT: <key>" label + separator (between the PTT label and the Config: label makes the most UX sense). To do this cleanly, restructure the menu construction order to:

1. `utter vX.Y.Z` (disabled)
2. `PTT: <key>` (disabled)
3. `Change PTT Key…` (this new item)
4. separator
5. Four bool toggles
6. separator
7. `Config: <path>` (disabled)
8. separator
9. `Reset TCC Grants (dev)`
10. separator
11. `Quit utter` ⌘Q

Rearrange the existing `menu.addItem(...)` calls in `src/macos_ui.rs` to match this order.

- [ ] **Step 4: Build + manual smoke test**

Run: `cargo build --release && ./scripts/make-bundle.sh && killall utter 2>/dev/null; open target/release/utter.app`

1. Click the menu bar icon, select "Change PTT Key…"
2. Expected: osascript notification fires; a separate `utter` process prints "Press and hold the key..." in the macOS log (`log stream --process utter`).
3. Press + release a new key. Expected log: "Saved key \`<name>\` to /Users/.../config.toml."
4. `cat ~/Library/Application\ Support/utter/config.toml | grep key` — expected: new key name.
5. Cmd+Q, reopen. Menu "PTT:" label shows the new key. Dictation uses the new key.

- [ ] **Step 5: Commit**

```bash
git add src/macos_ui.rs
git commit -m "feat(macos): 'Change PTT Key…' menu item spawns set-key picker

Clicking the item spawns the current binary with the set-key
subcommand as a separate process so its event tap doesn't fight
with the already-running watcher's tap. User presses the new key,
set-key saves to config.toml, user Cmd+Qs and reopens to pick up
the change.

Menu reorganized so related items are grouped: PTT label + Change
PTT Key, bool toggles, config path + reset TCC, quit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Verify Linux build still green

**Files:**
- None modified. This is a check-only task.

- [ ] **Step 1: Cross-check Linux compilation locally**

The macOS dev box can't run `cargo build` targeting Linux without a cross-compilation toolchain, and we deliberately scoped that out. Manual check:

```bash
grep -n "with_auto_paste\|with_write_clipboard\|with_filter_filler_words\|with_show_notifications" src/config.rs
```

Expected: four method definitions + four test assertions — all in the shared (non-cfg-gated) sections of the file.

```bash
grep -n "#\[cfg(target_os" src/macos_ui.rs src/config.rs
```

Expected: `src/macos_ui.rs` is entirely gated at the module level from `main.rs` (no inline cfg needed); `src/config.rs` has no `cfg` gates.

- [ ] **Step 2: Check the tests module is still Linux-only-runnable**

Run: `cargo test --lib config::tests 2>&1 | tail -3`
Expected: passes on macOS (config tests don't depend on evdev). `src/main.rs`'s test mod is gated to Linux only, so on macOS only config tests run — confirming config changes didn't leak macOS-only types into shared code.

- [ ] **Step 3: No commit — this is a verification-only task**

---

## Self-review checklist

- [ ] All four config fields have both builders (Task 1) and menu items (Task 3).
- [ ] PTT key change has a menu item (Task 4).
- [ ] No placeholders remain (grep the plan for TBD / TODO / implement later).
- [ ] Type consistency: `with_*` method names in Task 1 match the `c.with_*(v)` calls in Task 3's `apply` closures.
- [ ] The menu order described in Task 4 step 3 uses every item from Tasks 2/3/4 plus the existing ones (nothing dropped).
- [ ] No hot-reload work — restart-to-apply is the explicit contract everywhere.
- [ ] Linux regression guardrail is Task 5.

---

## Out of scope (for this plan)

- Hot-reload: no `reload` socket command, no `Arc<RwLock<Config>>`, no watcher respawn-on-config-change. User Cmd+Qs and reopens.
- Linux UI (GTK preferences, menu bar on Linux, etc.). Config builders from Task 1 are available for it; the plumbing is a separate project.
- A "Preferences…" window with text fields. Menu items are enough for the current config surface.
- Status bar icon that reflects daemon state (idle / recording). Separate feature.
- Key-equivalents on the bool toggles (Cmd+Shift+1 etc.). Not needed.

## Risk register

- **Obj-C selector name collisions.** The new selector names (`toggleAutoPaste:`, `changePttKey:`, etc.) are scoped to `UtterMenuTarget`, so they won't collide with AppKit. Trivial.
- **Subprocess PTT picker attribution.** `std::env::current_exe()` returns the binary inside the `.app` bundle. The spawned child should inherit the bundle's CFBundleIdentifier for TCC purposes. If it doesn't, the child's CGEventTap will silently fail — the user sees the "Press and hold…" message but nothing happens. Mitigation: if the smoke test in Task 4 step 4 fails, fall back to `open "$(current_exe_bundle_root)" --args set-key`.
- **Hot-reload expectations.** Users might expect toggles to apply immediately. The notification explicitly says "Cmd+Q and reopen." If that UX is confusing in practice, add hot-reload later.
- **Menu state drift.** Toggle checkmarks are captured at process start; flipping a toggle doesn't update its own checkmark live. The notification covers this, but it's a small surprise. Could be fixed by manually updating the NSMenuItem state in the action handler — ~2 lines per toggle — but adds coupling between the action and the menu item's retained pointer. Revisit if annoying in practice.
