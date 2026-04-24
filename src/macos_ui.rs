// macOS menu bar UI for utter. Only used when the bundle launches in
// combined mode (`open utter.app` with no --args). Runs on the main
// thread; the tokio runtime that drives the daemon + watcher lives on a
// worker thread (see run_combined_with_menu_bar in main.rs).
//
// Menu (v1): version label, current PTT key, config file path, quit.
// All label items are disabled (non-clickable). Quit uses the built-in
// NSApp `terminate:` selector — no custom target class needed.

use crate::Config;
use anyhow::{anyhow, Result};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{declare_class, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSImage, NSMenu, NSMenuItem, NSStatusBar,
    NSVariableStatusItemLength,
};
use objc2_foundation::{CGFloat, MainThreadMarker, NSObject, NSObjectProtocol, NSSize, NSString};

// Obj-C class exposed to NSMenuItem's target/action pattern. Needs to be
// a real class so the selector lookup at click time resolves. Ivars are
// empty — the actions shell out to tccutil with a hardcoded bundle id.
declare_class!(
    struct UtterMenuTarget;

    unsafe impl ClassType for UtterMenuTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "UtterMenuTarget";
    }

    impl DeclaredClass for UtterMenuTarget {
        type Ivars = ();
    }

    unsafe impl NSObjectProtocol for UtterMenuTarget {}

    unsafe impl UtterMenuTarget {
        #[method(resetTccGrants:)]
        fn reset_tcc_grants(&self, _sender: Option<&AnyObject>) {
            // ListenEvent (Input Monitoring) and Accessibility live in the
            // SYSTEM TCC db (/Library/...), which requires root to modify.
            // `tccutil reset` without elevation silently no-ops on those —
            // it prints "Successfully reset" but the system-level grant
            // survives. Mic is user-level and would clear without auth, but
            // we batch all three under one auth prompt via "tccutil reset
            // All", which clears every entry tccutil knows about for the
            // bundle. Single osascript admin prompt, native auth dialog.
            // `with prompt` replaces the default "osascript wants to make
            // changes" body text with something the user can act on. The
            // title still shows "osascript" (it's the calling process and
            // macOS doesn't let us change that for the auth dialog).
            let script = r#"do shell script "/usr/bin/tccutil reset All com.utter.app" with prompt "Reset utter's macOS permission grants (Microphone, Input Monitoring, Accessibility) so the next launch re-prompts for them. Admin required because Input Monitoring + Accessibility live in the system TCC database." with administrator privileges"#;
            let status = std::process::Command::new("osascript")
                .arg("-e")
                .arg(script)
                .status();
            match status {
                Ok(s) if s.success() => log::info!("tccutil reset All com.utter.app: ok (admin)"),
                Ok(s) => log::warn!("tccutil reset All com.utter.app: {s} (auth cancelled or tccutil failed)"),
                Err(e) => log::warn!("tccutil reset spawn: {e:#}"),
            }
            log::info!("TCC grants reset — quit and relaunch utter.app to trigger fresh prompts");
        }

        #[method(toggleAutoPaste:)]
        fn toggle_auto_paste(&self, sender: Option<&AnyObject>) {
            flip_config_flag("auto_paste", |c| c.auto_paste, |c, v| c.with_auto_paste(v), sender);
        }

        #[method(toggleFilterFillerWords:)]
        fn toggle_filter_filler_words(&self, sender: Option<&AnyObject>) {
            flip_config_flag("filter_filler_words", |c| c.filter_filler_words, |c, v| c.with_filter_filler_words(v), sender);
        }
    }
);

impl UtterMenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

pub fn run_status_bar_app(
    cfg: &Config,
    config_path: &std::path::Path,
    start_daemon: std::sync::mpsc::Sender<()>,
) -> Result<()> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| anyhow!("status bar app must run on the main thread"))?;

    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        // Accessory keeps the app off the Dock + out of the Cmd-Tab
        // switcher while still allowing a menu bar item. (Prohibited
        // would block the NSStatusItem entirely.)
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        // Check permissions and decide whether to show onboarding. Only
        // check AX if IM is already granted (avoids FB7381305 poisoning
        // a later IOHIDRequestAccess call). If all three are already
        // granted, signal the runtime to start the daemon immediately;
        // otherwise put up the onboarding window, which will signal
        // when the user completes it.
        let mic_ok = crate::macos::mic_status() == crate::macos::AvAuthStatus::Authorized;
        let im_ok = crate::macos::input_monitoring_granted();
        let all_granted = mic_ok && im_ok && crate::macos::is_accessibility_trusted();
        if all_granted {
            log::info!("all permissions granted — skipping onboarding, starting daemon");
            let _ = start_daemon.send(());
        } else {
            log::info!(
                "onboarding needed: mic_ok={mic_ok} im_ok={im_ok} (AX deferred while IM pending)"
            );
            let start_daemon_for_onboard = start_daemon.clone();
            crate::macos_onboarding::show(
                mtm,
                Box::new(move || {
                    let _ = start_daemon_for_onboard.send(());
                }),
            );
        }

        let status_bar = NSStatusBar::systemStatusBar();
        let item = status_bar.statusItemWithLength(NSVariableStatusItemLength);

        if let Some(button) = item.button(mtm) {
            // Use an SF Symbol for the menu bar so the icon is a proper
            // template image: monochrome, auto-tinted by macOS to match
            // the current menu bar style (light/dark/translucent), and
            // visually consistent with the other system menu bar icons.
            // "waveform.circle.fill" echoes utter's full-color app icon
            // (waveform inside a circle) without the color baggage.
            // Falls back to utter.icns if the symbol isn't available
            // (SF Symbols require macOS 11+; utter already requires 13).
            let symbol_name = NSString::from_str("waveform.circle.fill");
            let a11y = NSString::from_str("utter");
            let icon: Option<Retained<NSImage>> = unsafe {
                msg_send_id![
                    objc2::class!(NSImage),
                    imageWithSystemSymbolName: &*symbol_name,
                    accessibilityDescription: &*a11y,
                ]
            };
            let icon = icon.or_else(|| NSImage::imageNamed(&NSString::from_str("utter")));
            if let Some(icon) = icon {
                // Bump the SF Symbol's intrinsic point size so it fills
                // the menu bar button properly — default NSImage size
                // from imageWithSystemSymbolName is small (~16pt) and
                // leaves a lot of empty space in the button's pill.
                // 20pt + Regular weight visually matches adjacent system
                // menu bar icons. setSize alone isn't enough; SF Symbols
                // need a point-size configuration to actually render larger.
                let config: Option<Retained<objc2::runtime::AnyObject>> = unsafe {
                    msg_send_id![
                        objc2::class!(NSImageSymbolConfiguration),
                        configurationWithPointSize: 18.0 as CGFloat,
                        weight: 5_isize, // NSFontWeightRegular
                    ]
                };
                if let Some(config) = config {
                    let sized: Option<Retained<NSImage>> = unsafe {
                        msg_send_id![&*icon, imageWithSymbolConfiguration: &*config]
                    };
                    if let Some(sized) = sized {
                        button.setImage(Some(&sized));
                    } else {
                        button.setImage(Some(&icon));
                    }
                } else {
                    icon.setSize(NSSize::new(20.0 as CGFloat, 20.0 as CGFloat));
                    button.setImage(Some(&icon));
                }
            } else {
                log::warn!("menu bar icon could not be loaded");
            }
        }

        let menu = {
            let alloc = mtm.alloc::<NSMenu>();
            NSMenu::initWithTitle(alloc, &NSString::from_str("utter"))
        };
        // We manage enabled state explicitly — auto-enable walks the
        // responder chain to find a target for each item and would
        // enable labels we want disabled.
        menu.setAutoenablesItems(false);

        // One Obj-C target instance shared across every clickable item
        // (toggles, reset, change-key). NSMenuItem.setTarget is unretained,
        // so we leak this single Retained at the end so the target outlives
        // the menu items that point at it.
        let target = UtterMenuTarget::new(mtm);
        let target_ref: &AnyObject = &*target;

        // Capture env once to compute env-override greyout per toggle.
        let env = crate::config::utter_env_snapshot();

        add_label(
            &menu,
            mtm,
            &format!("utter v{}", env!("CARGO_PKG_VERSION")),
        );
        add_label(&menu, mtm, &format!("PTT: {}", cfg.key));
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        add_toggle(
            &menu, mtm, target_ref,
            "Auto-paste after dictation",
            sel!(toggleAutoPaste:),
            cfg.auto_paste,
            env.contains_key("UTTER_AUTO_PASTE"),
        );
        // write_clipboard is intentionally not exposed on macOS yet: macOS
        // has only one pasteboard, and the auto-paste flow needs text on
        // it to do Cmd+V. A future PR will implement true "leave the
        // clipboard untouched" via snapshot-and-restore around the paste,
        // and re-add this toggle.
        add_toggle(
            &menu, mtm, target_ref,
            "Filter filler words",
            sel!(toggleFilterFillerWords:),
            cfg.filter_filler_words,
            env.contains_key("UTTER_FILTER_FILLER_WORDS"),
        );

        menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_label(&menu, mtm, &format!("Config: {}", config_path.display()));
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Dev-only: "Reset TCC grants". Shells out to tccutil reset on the
        // bundle id for Microphone, Input Monitoring, and Accessibility.
        // After clicking, user must Quit + relaunch — macOS doesn't let a
        // running process pick up the fresh grant.
        let reset_item = {
            let alloc = mtm.alloc::<NSMenuItem>();
            NSMenuItem::initWithTitle_action_keyEquivalent(
                alloc,
                &NSString::from_str("Reset TCC Grants (dev)"),
                Some(sel!(resetTccGrants:)),
                &NSString::from_str(""),
            )
        };
        reset_item.setTarget(Some(target_ref));
        menu.addItem(&reset_item);

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Quit uses the AppKit built-in terminate: selector; target=nil
        // routes it through the responder chain to NSApp. Cmd+Q works
        // because NSStatusItem menus do accept key equivalents.
        let quit = {
            let alloc = mtm.alloc::<NSMenuItem>();
            NSMenuItem::initWithTitle_action_keyEquivalent(
                alloc,
                &NSString::from_str("Quit utter"),
                Some(sel!(terminate:)),
                &NSString::from_str("q"),
            )
        };
        menu.addItem(&quit);

        item.setMenu(Some(&menu));
        // Hold the target for the process lifetime — see comment at creation.
        std::mem::forget(target);
        log::info!("status bar item installed; NSApplication.run() taking over");
        app.run();
    }
    Ok(())
}

unsafe fn add_label(menu: &NSMenu, mtm: MainThreadMarker, text: &str) {
    let alloc = mtm.alloc::<NSMenuItem>();
    let item = NSMenuItem::initWithTitle_action_keyEquivalent(
        alloc,
        &NSString::from_str(text),
        None,
        &NSString::from_str(""),
    );
    item.setEnabled(false);
    menu.addItem(&item);
}

unsafe fn add_toggle(
    menu: &NSMenu,
    mtm: MainThreadMarker,
    target: &AnyObject,
    title: &str,
    selector: objc2::runtime::Sel,
    is_on: bool,
    env_overridden: bool,
) {
    use objc2_app_kit::{NSControlStateValueOff, NSControlStateValueOn};
    let display_title = if env_overridden {
        format!("{title} (env override)")
    } else {
        title.to_string()
    };
    let alloc = mtm.alloc::<NSMenuItem>();
    let item = NSMenuItem::initWithTitle_action_keyEquivalent(
        alloc,
        &NSString::from_str(&display_title),
        Some(selector),
        &NSString::from_str(""),
    );
    item.setTarget(Some(target));
    item.setState(if is_on { NSControlStateValueOn } else { NSControlStateValueOff });
    if env_overridden {
        item.setEnabled(false);
    }
    menu.addItem(&item);
}

/// Load current config from disk (file-only, ignoring env so the toggle
/// reflects what the user actually set), flip one bool via the supplied
/// closures, save, update the clicked menu item's checkmark, and post a
/// notification reminding the user to restart.
fn flip_config_flag<R, W>(
    flag_name: &str,
    read: R,
    write: W,
    sender: Option<&AnyObject>,
)
where
    R: FnOnce(&Config) -> bool,
    W: FnOnce(Config, bool) -> Config,
{
    let path = match Config::default_path() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("toggle {flag_name}: default_path: {e:#}");
            return;
        }
    };
    // Read the raw file (no env overlay) so the toggle is the inverse of
    // what's actually persisted, not the inverse of the env-shadowed value.
    let current = match std::fs::read_to_string(&path)
        .map_err(anyhow::Error::from)
        .and_then(|t| Config::from_toml(&t))
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("toggle {flag_name}: read {}: {e:#}", path.display());
            return;
        }
    };
    let new_value = !read(&current);
    let updated = write(current, new_value);
    if let Err(e) = updated.save_to(&path) {
        log::warn!("toggle {flag_name}: save_to: {e:#}");
        return;
    }
    log::info!("toggle {flag_name} -> {new_value}; quit + relaunch to apply");

    // Update the clicked item's checkmark live so the menu doesn't lie
    // until next launch. SAFETY: target/action always passes the
    // NSMenuItem that was clicked.
    if let Some(s) = sender {
        use objc2_app_kit::{NSControlStateValueOff, NSControlStateValueOn};
        unsafe {
            let item: &NSMenuItem = &*(s as *const AnyObject as *const NSMenuItem);
            item.setState(if new_value { NSControlStateValueOn } else { NSControlStateValueOff });
        }
    }

    notify_restart(flag_name, new_value);
}

/// Fire-and-forget osascript notification describing what the toggle
/// means in user terms (not just the field name + bool). The daemon
/// doesn't hot-reload, so every body ends with "Restart utter to apply."
fn notify_restart(flag_name: &str, new_value: bool) {
    let (title, body) = match (flag_name, new_value) {
        ("auto_paste", true) => (
            "utter: auto-paste enabled".to_string(),
            "Dictations will paste themselves into the focused window. Restart utter to apply.".to_string(),
        ),
        ("auto_paste", false) => (
            "utter: auto-paste disabled".to_string(),
            "Dictations are written to the clipboard; paste them yourself with Cmd+V. Restart utter to apply.".to_string(),
        ),
        ("filter_filler_words", true) => (
            "utter: filler-word filtering enabled".to_string(),
            "Words like 'uh', 'um', and stuttered repeats will be removed before pasting. Restart utter to apply.".to_string(),
        ),
        ("filter_filler_words", false) => (
            "utter: filler-word filtering disabled".to_string(),
            "Dictations will be pasted exactly as transcribed. Restart utter to apply.".to_string(),
        ),
        (name, v) => (
            format!("utter: {name} = {v}"),
            "Saved. Restart utter to apply.".to_string(),
        ),
    };
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
