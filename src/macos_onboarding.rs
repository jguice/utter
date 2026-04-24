// Onboarding window for macOS permissions (Microphone, Input Monitoring,
// Accessibility). Shown before the status bar app takes over, whenever at
// least one of the three TCC grants is pending. A 1-second NSTimer polls
// status and updates row labels + Grant button visibility in place; the
// Start button enables once all three are granted.
//
// FB7381305 avoidance: we never touch any AX API until Input Monitoring
// has been granted. Apple's bug causes IM's prompt to be suppressed if
// AXIsProcessTrustedWithOptions ran earlier in the same process. The AX
// row stays in "After Input Monitoring" mode until polling sees IM granted.

use crate::macos;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{declare_class, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBox, NSBoxType, NSButton, NSColor, NSFont,
    NSLayoutAttribute, NSStackView, NSStackViewDistribution, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowStyleMask,
};
// Used for pinning the name-label widths in make_row so the three status
// columns align. No-op import otherwise.
#[allow(unused_imports)]
use objc2_app_kit::NSLayoutConstraint;
use objc2_foundation::{
    CGFloat, MainThreadMarker, NSEdgeInsets, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSSize, NSString, NSTimer,
};
use std::sync::{Mutex, OnceLock};

// Global state so target/action selectors (which can't capture Rust locals)
// can reach the retained UI handles + the `on_start` closure. Populated in
// `show` before the window is made key; cleared in `start:` after running
// the closure. The UI handles inside are MainThreadOnly objects, so access
// happens only on the main thread — the Mutex is a belt-and-suspenders
// guard for the Option replacement, not a real cross-thread synchronizer.
static STATE: OnceLock<Mutex<Option<OnboardingUiState>>> = OnceLock::new();

// `Retained` of MainThreadOnly classes is !Send by default; the UI handles
// never leave the main thread (NSTimer callbacks all fire there, same for
// button target/action), but the Mutex and OnceLock machinery require Send.
// We wrap in a dedicated struct and assert Send manually — safe because
// every method that reads these fields runs on the main thread.
struct OnboardingUiState {
    window: Retained<NSWindow>,
    mic_status: Retained<NSTextField>,
    im_status: Retained<NSTextField>,
    ax_status: Retained<NSTextField>,
    mic_button: Retained<NSButton>,
    im_button: Retained<NSButton>,
    ax_button: Retained<NSButton>,
    footer_status: Retained<NSTextField>,
    start_button: Retained<NSButton>,
    timer: Retained<NSTimer>,
    on_start: Option<Box<dyn FnOnce() + Send>>,
}

unsafe impl Send for OnboardingUiState {}

fn state_cell() -> &'static Mutex<Option<OnboardingUiState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

declare_class!(
    struct UtterOnboardingTarget;

    unsafe impl ClassType for UtterOnboardingTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "UtterOnboardingTarget";
    }

    impl DeclaredClass for UtterOnboardingTarget {
        type Ivars = ();
    }

    unsafe impl NSObjectProtocol for UtterOnboardingTarget {}

    unsafe impl UtterOnboardingTarget {
        #[method(grantMic:)]
        fn grant_mic(&self, _sender: Option<&AnyObject>) {
            // Once mic is Denied, AVCaptureDevice.requestAccess won't
            // re-prompt — it returns false immediately. Deep-link to
            // the Mic pane in System Settings so the user can toggle
            // utter back on. For NotDetermined, fire the request as
            // normal; the completion block runs on a private dispatch
            // queue, so spawn + forget to avoid freezing the UI while
            // the prompt is up.
            if macos::mic_status() == macos::AvAuthStatus::Denied {
                open_settings_pane("Privacy_Microphone");
                return;
            }
            std::thread::spawn(|| {
                let _ = macos::request_mic_access_blocking();
            });
        }

        #[method(grantInputMonitoring:)]
        fn grant_input_monitoring(&self, _sender: Option<&AnyObject>) {
            // IOHIDRequestAccess re-fires the prompt for NotDetermined
            // but no-ops on Denied. If denied (or still pending after
            // the request), deep-link to Settings so the user has a
            // way out.
            if !macos::request_input_monitoring() {
                open_settings_pane("Privacy_ListenEvent");
            }
        }

        #[method(grantAccessibility:)]
        fn grant_accessibility(&self, _sender: Option<&AnyObject>) {
            // prompt_accessibility shows the native AX dialog with its
            // own "Open System Settings" link when not trusted; if it
            // returns true the grant is already in place. Also deep-link
            // for the denied/stuck case.
            if !macos::prompt_accessibility() {
                open_settings_pane("Privacy_Accessibility");
            }
        }

        #[method(start:)]
        fn start(&self, _sender: Option<&AnyObject>) {
            let (on_start, window, timer) = {
                let mut guard = match state_cell().lock() {
                    Ok(g) => g,
                    Err(e) => {
                        log::warn!("onboarding: state lock poisoned: {e}");
                        return;
                    }
                };
                let Some(state) = guard.as_mut() else { return };
                let on_start = state.on_start.take();
                let window = state.window.clone();
                let timer = state.timer.clone();
                (on_start, window, timer)
            };
            unsafe {
                timer.invalidate();
                window.close();
            }
            // Drop the whole state after close so the Retained handles
            // release on the main thread (they're MainThreadOnly).
            let _ = state_cell().lock().map(|mut g| *g = None);
            if let Some(cb) = on_start {
                cb();
            }
        }

        #[method(pollStatus:)]
        fn poll_status(&self, _sender: Option<&AnyObject>) {
            refresh_status();
        }
    }
);

impl UtterOnboardingTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Display the onboarding window. Must be called on the main thread,
/// before (or while) `NSApplication::run` is spinning — the window joins
/// the live runloop, so it only paints once the app is running. `on_start`
/// runs exactly once, on the main thread, after the user clicks Start
/// (and after the window has been ordered out).
pub fn show(mtm: MainThreadMarker, on_start: Box<dyn FnOnce() + Send>) {
    unsafe {
        let target = UtterOnboardingTarget::new(mtm);
        let target_ref: &AnyObject = &*target;

        let window = build_window(mtm);
        let content = build_content(mtm, target_ref);
        window.setContentView(Some(&content.root));
        window.center();

        // Install the 1 Hz poll timer. Target/selector keeps the tap
        // logic local to the class — no block2 machinery needed, and the
        // timer retains the target for us.
        let timer: Retained<NSTimer> = NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            1.0,
            target_ref,
            sel!(pollStatus:),
            None,
            true,
        );

        // Publish state before the first paint so pollStatus has something
        // to look at. refresh_status is also called synchronously right
        // below to fill in initial values without waiting for the first
        // timer tick.
        {
            let mut guard = state_cell().lock().expect("onboarding state lock");
            *guard = Some(OnboardingUiState {
                window: window.clone(),
                mic_status: content.mic_status,
                im_status: content.im_status,
                ax_status: content.ax_status,
                mic_button: content.mic_button,
                im_button: content.im_button,
                ax_button: content.ax_button,
                footer_status: content.footer_status,
                start_button: content.start_button,
                timer,
                on_start: Some(on_start),
            });
        }

        refresh_status();

        // Bring the app forward so the window is key. In menu-bar
        // (Accessory) activation policy, windows still show, but the
        // user needs to see this one immediately — activateIgnoringOtherApps
        // makes sure the prompts it triggers aren't hidden behind another
        // app's stack.
        let app = NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        window.makeKeyAndOrderFront(None);

        // Leak the target instance for the process lifetime. NSTimer
        // retains it, but once the timer is invalidated the retain drops
        // and the selector lookup for button clicks would dangle. The
        // same pattern (std::mem::forget) is used in macos_ui.rs.
        std::mem::forget(target);
    }
}

fn build_window(mtm: MainThreadMarker) -> Retained<NSWindow> {
    unsafe {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(580.0, 380.0));
        // Titled + Closable only — no resize, no minimize. This is a
        // modal-feeling pre-flight, not a document window.
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
        let alloc = mtm.alloc::<NSWindow>();
        let window: Retained<NSWindow> = NSWindow::initWithContentRect_styleMask_backing_defer(
            alloc,
            frame,
            style,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        );
        window.setTitle(&NSString::from_str("Set up utter"));
        window.setReleasedWhenClosed(false);
        window
    }
}

struct ContentViews {
    root: Retained<NSView>,
    mic_status: Retained<NSTextField>,
    im_status: Retained<NSTextField>,
    ax_status: Retained<NSTextField>,
    mic_button: Retained<NSButton>,
    im_button: Retained<NSButton>,
    ax_button: Retained<NSButton>,
    footer_status: Retained<NSTextField>,
    start_button: Retained<NSButton>,
}

fn build_content(mtm: MainThreadMarker, target: &AnyObject) -> ContentViews {
    unsafe {
        // Outer vertical stack fills the window's content view. Insets
        // match the spec (~24pt edge); spacing 16 between sections.
        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        stack.setSpacing(16.0 as CGFloat);
        stack.setEdgeInsets(NSEdgeInsets {
            top: 24.0,
            left: 24.0,
            bottom: 24.0,
            right: 24.0,
        });
        stack.setDistribution(NSStackViewDistribution::Fill);

        let title = make_label(
            mtm,
            "utter needs three macOS permissions",
            16.0,
            true,
            NSColor::labelColor(),
        );
        stack.addArrangedSubview(&*title_as_view(&title));

        let subtitle = make_label(
            mtm,
            "Grant each permission below. Input Monitoring and \
             Accessibility require a restart after toggling — macOS will \
             ask to 'Quit & Reopen' — click it.",
            11.0,
            false,
            NSColor::secondaryLabelColor(),
        );
        subtitle.setMaximumNumberOfLines(0);
        subtitle.setUsesSingleLineMode(false);
        subtitle.setPreferredMaxLayoutWidth(580.0 - 48.0);
        stack.addArrangedSubview(&*title_as_view(&subtitle));

        stack.addArrangedSubview(&*make_separator(mtm));

        // Shared minimum width for the bold name column so the three status
        // labels ("Pending" / "After Input Monitoring") line up at the same
        // x position regardless of which name ("Microphone" vs "Input
        // Monitoring" vs "Accessibility") is widest.
        let name_col_width: CGFloat = 140.0;

        let mic_row = make_row(
            mtm,
            "Microphone",
            "Pending",
            target,
            sel!(grantMic:),
            name_col_width,
        );
        stack.addArrangedSubview(&*mic_row.view);

        let im_row = make_row(
            mtm,
            "Input Monitoring",
            "Pending",
            target,
            sel!(grantInputMonitoring:),
            name_col_width,
        );
        stack.addArrangedSubview(&*im_row.view);

        let ax_row = make_row(
            mtm,
            "Accessibility",
            "After Input Monitoring",
            target,
            sel!(grantAccessibility:),
            name_col_width,
        );
        // AX button is visible from the start; refresh_status() drives its
        // title ("Waiting…") and enabled state until IM is granted.
        stack.addArrangedSubview(&*ax_row.view);

        stack.addArrangedSubview(&*make_separator(mtm));

        // Footer: status text on the left, Start button on the right.
        let footer = NSStackView::new(mtm);
        footer.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        footer.setAlignment(NSLayoutAttribute::CenterY);
        footer.setSpacing(12.0 as CGFloat);
        footer.setDistribution(NSStackViewDistribution::Fill);

        let footer_status = make_label(
            mtm,
            "Grant all three to continue",
            12.0,
            false,
            NSColor::secondaryLabelColor(),
        );
        footer.addArrangedSubview(&*title_as_view(&footer_status));

        let spacer = make_spacer(mtm);
        footer.addArrangedSubview(&spacer);

        let start_button = NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Start using utter"),
            Some(target),
            Some(sel!(start:)),
            mtm,
        );
        start_button.setEnabled(false);
        footer.addArrangedSubview(&*button_as_view(&start_button));

        stack.addArrangedSubview(&footer);

        // The content view's size is determined by the window's frame;
        // wrap the stack in an NSView so autoresizing pins it to all
        // edges. Using the stack directly as contentView works too, but
        // this way the background gets painted by the wrapper and the
        // stack doesn't fight with the window's content rect.
        let root_frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(520.0, 380.0),
        );
        let root_alloc = mtm.alloc::<NSView>();
        let root: Retained<NSView> = NSView::initWithFrame(root_alloc, root_frame);
        root.addSubview(&stack);

        // Pin the stack to the root view's edges via autoresizing mask —
        // simpler than full NSLayoutConstraint setup for a fixed-size
        // window.
        stack.setTranslatesAutoresizingMaskIntoConstraints(true);
        use objc2_app_kit::NSAutoresizingMaskOptions;
        stack.setAutoresizingMask(
            NSAutoresizingMaskOptions::NSViewWidthSizable
                | NSAutoresizingMaskOptions::NSViewHeightSizable,
        );
        stack.setFrame(root_frame);

        ContentViews {
            root,
            mic_status: mic_row.status,
            im_status: im_row.status,
            ax_status: ax_row.status,
            mic_button: mic_row.button,
            im_button: im_row.button,
            ax_button: ax_row.button,
            footer_status,
            start_button,
        }
    }
}

struct Row {
    view: Retained<NSStackView>,
    status: Retained<NSTextField>,
    button: Retained<NSButton>,
}

fn make_row(
    mtm: MainThreadMarker,
    name: &str,
    initial_status: &str,
    target: &AnyObject,
    action: objc2::runtime::Sel,
    name_col_width: CGFloat,
) -> Row {
    unsafe {
        let row = NSStackView::new(mtm);
        row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        row.setAlignment(NSLayoutAttribute::CenterY);
        row.setSpacing(12.0 as CGFloat);
        row.setDistribution(NSStackViewDistribution::Fill);

        let name_label = make_label(mtm, name, 13.0, true, NSColor::labelColor());
        // Pin each row's name column to the same width so the status
        // column starts at the same x for all three rows. Equal (not
        // greater-than-or-equal) keeps short names from stretching to
        // their natural width.
        name_label.setTranslatesAutoresizingMaskIntoConstraints(false);
        name_label
            .widthAnchor()
            .constraintEqualToConstant(name_col_width)
            .setActive(true);
        row.addArrangedSubview(&*title_as_view(&name_label));

        let status_label = make_label(
            mtm,
            initial_status,
            13.0,
            false,
            NSColor::secondaryLabelColor(),
        );
        row.addArrangedSubview(&*title_as_view(&status_label));

        let spacer = make_spacer(mtm);
        row.addArrangedSubview(&spacer);

        let button = NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Grant"),
            Some(target),
            Some(action),
            mtm,
        );
        row.addArrangedSubview(&*button_as_view(&button));

        Row {
            view: row,
            status: status_label,
            button,
        }
    }
}

fn make_label(
    mtm: MainThreadMarker,
    text: &str,
    size: CGFloat,
    bold: bool,
    color: Retained<NSColor>,
) -> Retained<NSTextField> {
    unsafe {
        let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
        let font = if bold {
            NSFont::boldSystemFontOfSize(size)
        } else {
            NSFont::systemFontOfSize(size)
        };
        label.setFont(Some(&font));
        label.setTextColor(Some(&color));
        label.setBordered(false);
        label.setDrawsBackground(false);
        label.setEditable(false);
        label.setSelectable(false);
        label
    }
}

fn make_separator(mtm: MainThreadMarker) -> Retained<NSView> {
    unsafe {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(472.0, 1.0));
        let alloc = mtm.alloc::<NSBox>();
        let boxed: Retained<NSBox> = objc2::msg_send_id![alloc, initWithFrame: frame];
        boxed.setBoxType(NSBoxType::NSBoxSeparator);
        // NSBox is-a NSView — hand the NSStackView a plain NSView
        // reference. Retain-count stays with the NSBox so it lives as
        // long as the window hierarchy.
        Retained::cast::<NSView>(boxed)
    }
}

fn make_spacer(mtm: MainThreadMarker) -> Retained<NSView> {
    unsafe {
        let alloc = mtm.alloc::<NSView>();
        let view: Retained<NSView> = NSView::initWithFrame(
            alloc,
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)),
        );
        // Low hugging priority so the stack view expands this to fill
        // the free horizontal space, pushing the Grant button to the
        // trailing edge. 1 is "willing to stretch indefinitely".
        use objc2_app_kit::NSLayoutConstraintOrientation;
        view.setContentHuggingPriority_forOrientation(
            1.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
        view
    }
}

// NSStackView.addArrangedSubview wants &NSView, but the objc2 types for
// NSTextField / NSButton / NSStackView don't auto-deref to NSView —
// their inheritance is expressed via ClassType::Super chains rather than
// Deref. Upcast explicitly through msg_send of self (a no-op cast) to
// satisfy the signature. Retain a new reference so the caller's original
// Retained isn't prematurely dropped.
/// Open System Settings > Privacy & Security to the given pane. The
/// `anchor` is one of: `Privacy_Microphone`, `Privacy_ListenEvent`
/// (Input Monitoring), `Privacy_Accessibility`.
fn open_settings_pane(anchor: &str) {
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
    let _ = std::process::Command::new("open").arg(&url).status();
}

fn title_as_view(label: &NSTextField) -> Retained<NSView> {
    let ptr: *const NSTextField = label;
    unsafe { Retained::retain(ptr as *mut NSView).expect("upcast NSTextField to NSView") }
}

fn button_as_view(button: &NSButton) -> Retained<NSView> {
    let ptr: *const NSButton = button;
    unsafe { Retained::retain(ptr as *mut NSView).expect("upcast NSButton to NSView") }
}

/// Snapshot current TCC status and update labels + buttons to match.
/// Called synchronously once at startup, then every 1s on the NSTimer.
/// All UI writes happen on the main thread (timer fires there).
fn refresh_status() {
    let guard = match state_cell().lock() {
        Ok(g) => g,
        Err(e) => {
            log::warn!("onboarding: state lock poisoned during refresh: {e}");
            return;
        }
    };
    let Some(state) = guard.as_ref() else { return };

    let mic = macos::mic_status();
    let im = macos::input_monitoring_granted();
    // AX is only safe to query once IM is granted (FB7381305). Before
    // that, we show "After Input Monitoring" and treat AX as not granted.
    let ax = if im {
        macos::is_accessibility_trusted()
    } else {
        false
    };

    unsafe {
        // Every row always shows a button (never setHidden), so row
        // heights stay uniform and the vertical layout is consistent.
        // The button's title + enabled state reflect the permission
        // state — "Grant" for actionable, "Open Settings" for denied
        // (requestAccess won't re-prompt once denied), "Granted" for
        // done (disabled), "Waiting…" for AX while IM is pending.

        // Mic
        let (mic_status_label, mic_btn_title, mic_btn_enabled) = match mic {
            macos::AvAuthStatus::Authorized => ("Granted \u{2713}", "Granted", false),
            macos::AvAuthStatus::Denied => ("Denied \u{2717}", "Open Settings", true),
            macos::AvAuthStatus::Restricted => ("Restricted \u{2717}", "Open Settings", true),
            macos::AvAuthStatus::NotDetermined => ("Pending", "Grant", true),
        };
        state
            .mic_status
            .setStringValue(&NSString::from_str(mic_status_label));
        state
            .mic_button
            .setTitle(&NSString::from_str(mic_btn_title));
        state.mic_button.setEnabled(mic_btn_enabled);

        // IM
        let (im_status_label, im_btn_title, im_btn_enabled) = if im {
            ("Granted \u{2713}", "Granted", false)
        } else {
            ("Pending", "Grant", true)
        };
        state
            .im_status
            .setStringValue(&NSString::from_str(im_status_label));
        state
            .im_button
            .setTitle(&NSString::from_str(im_btn_title));
        state.im_button.setEnabled(im_btn_enabled);

        // AX (locked behind IM to avoid FB7381305)
        let (ax_status_label, ax_btn_title, ax_btn_enabled) = if !im {
            ("After Input Monitoring", "Waiting\u{2026}", false)
        } else if ax {
            ("Granted \u{2713}", "Granted", false)
        } else {
            ("Pending", "Grant", true)
        };
        state
            .ax_status
            .setStringValue(&NSString::from_str(ax_status_label));
        state
            .ax_button
            .setTitle(&NSString::from_str(ax_btn_title));
        state.ax_button.setEnabled(ax_btn_enabled);

        let all_granted =
            mic == macos::AvAuthStatus::Authorized && im && ax;
        state.start_button.setEnabled(all_granted);
        let footer_text = if all_granted {
            "All set. Click Start."
        } else {
            "Grant all three to continue"
        };
        state
            .footer_status
            .setStringValue(&NSString::from_str(footer_text));
    }
}
