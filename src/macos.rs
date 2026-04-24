// macOS platform implementation. All items here are only compiled when
// target_os = "macos" (main.rs cfg-gates the `mod macos;` line). Counterparts
// of the Linux-only helpers in main.rs: audio capture (cpal), text emission
// (NSPasteboard + CGEventPost), key watcher (CGEventTap), set-key picker,
// notification (osascript), and the kVK_* keycode mapping.
//
// Stubs today; implementations land task-by-task.

#![allow(unused_variables, dead_code)]

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

use crate::Config;

// Socket path on macOS: $TMPDIR/utter.sock. sun_path on BSD-derived systems
// is 104 bytes; $TMPDIR is typically /var/folders/ab/xxxxxx/T/ (~50 chars),
// so this fits with headroom. Assert at bind time to fail loudly rather than
// silently truncate.
pub fn socket_path() -> Result<PathBuf> {
    let tmp = std::env::var("TMPDIR")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/tmp".to_string());
    let path = PathBuf::from(tmp).join("utter.sock");
    let bytes = path.to_string_lossy().len();
    if bytes >= 104 {
        return Err(anyhow!(
            "$TMPDIR-derived socket path is {} bytes (limit 104): {}",
            bytes,
            path.display()
        ));
    }
    Ok(path)
}

// --- Permissions -----------------------------------------------------------
//
// macOS gates Microphone, Input Monitoring, and Accessibility behind TCC.
// Each subsystem normally fires its own prompt the first time the relevant
// API is used: the user gets hit with the mic prompt mid-dictation (slow,
// jarring), the accessibility prompt only on the first paste (silently
// fails to paste until granted), and Input Monitoring at watcher start.
// `ensure_permissions` replaces that with proactive checks at startup so
// all three prompts fire upfront in sequence.
//
// Threading: must run on the main thread before NSApplication.run().
// AVCaptureDevice.requestAccess's completion handler dispatches back to
// the calling thread's runloop; we spin the main runloop manually
// (CFRunLoopRunInMode) to wait for it. The other two prompts (IOHID, AX)
// are fire-and-forget — no completion API exists for them.

// AVAuthorizationStatus from AVFoundation/AVCaptureDevice.h
#[repr(i64)]
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum AvAuthStatus {
    NotDetermined = 0,
    Restricted = 1,
    Denied = 2,
    Authorized = 3,
}

impl AvAuthStatus {
    fn from_raw(v: i64) -> Self {
        match v {
            1 => Self::Restricted,
            2 => Self::Denied,
            3 => Self::Authorized,
            // Unknown / 0: treat as NotDetermined so we trigger the prompt.
            _ => Self::NotDetermined,
        }
    }
}

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVMediaTypeAudio: *const objc2_foundation::NSString;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(
        options: core_foundation::dictionary::CFDictionaryRef,
    ) -> bool;
    static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
}

// Input Monitoring uses the existing CGPreflight/Request bindings further
// down this file (see input_monitoring_granted / request_input_monitoring).

pub fn mic_status() -> AvAuthStatus {
    unsafe {
        let cls = objc2::class!(AVCaptureDevice);
        let raw: i64 = objc2::msg_send![cls, authorizationStatusForMediaType: AVMediaTypeAudio];
        AvAuthStatus::from_raw(raw)
    }
}

/// Trigger the Microphone TCC prompt and block until the user responds
/// or the timeout elapses. Returns true only if granted.
///
/// Must NOT be called on the main thread — AVCaptureDevice fires the
/// completion block on macOS's private dispatch queue, and the channel
/// recv blocks the calling thread. Blocking the main thread starves
/// AppKit and LaunchServices kills the process.
pub fn request_mic_access_blocking() -> bool {
    use block2::RcBlock;
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel::<bool>();

    // `BOOL` in the Obj-C ABI is `objc2::runtime::Bool`, not Rust's `bool`.
    let block = RcBlock::new(move |granted: objc2::runtime::Bool| {
        let _ = tx.send(granted.as_bool());
    });

    unsafe {
        let cls = objc2::class!(AVCaptureDevice);
        let _: () = objc2::msg_send![
            cls,
            requestAccessForMediaType: AVMediaTypeAudio,
            completionHandler: &*block,
        ];
    }

    rx.recv_timeout(std::time::Duration::from_secs(120))
        .unwrap_or_else(|_| {
            log::warn!("mic access prompt: timed out after 120s");
            false
        })
}

pub fn is_accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) }
}

/// Calling AXIsProcessTrustedWithOptions with kAXTrustedCheckOptionPrompt=true
/// makes the system display the Accessibility prompt if the app is not
/// already trusted. Returns the current trusted status (false while the
/// prompt is shown); the prompt itself is fire-and-forget.
pub fn prompt_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    unsafe {
        let key: CFString =
            TCFType::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let value = CFBoolean::true_value();
        let dict = CFDictionary::from_CFType_pairs(&[(key, value.as_CFType())]);
        AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef())
    }
}

/// Modal explanatory dialog telling the user exactly which buttons to
/// click on each of the three upcoming prompts. Written to match macOS's
/// real button labels — the wrong click (Deny, or Quit & Reopen before
/// toggling) wedges the flow.
///
/// Uses osascript `display dialog` (NSAlert under the hood — instant,
/// unlike `display notification` which queues through Notification
/// Center). Blocks until dismissed.
fn show_permissions_intro_dialog() {
    let body = "utter needs three macOS permissions. Three prompts will follow, in order:\n\n\
                1. Microphone — click Allow.\n\
                2. Keystroke Receiving (Input Monitoring) — click Open System Settings, then toggle utter ON.\n\
                3. Accessibility — click Open System Settings, then toggle utter ON.\n\n\
                After each toggle, macOS will ask to \"Quit & Reopen\" utter. Click Quit & Reopen — that's required for the grant to take effect.\n\n\
                If for any reason the restart doesn't happen automatically, reopen utter from your Applications folder (or wherever you keep it) and we'll pick up where we left off.";
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        r#"display dialog "{}" with title "utter needs three macOS permissions" buttons {{"Continue"}} default button "Continue""#,
        escape(body),
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .status();
}

/// Pre-flight all three macOS permissions at startup.
///
/// **Must NOT run on the main thread.** Internally blocks (channel recv
/// for mic, sleeps for IM/AX polling, subprocess spawns for dialogs).
/// Blocking the main thread starves AppKit and LaunchServices kills
/// the bundle. Call from a worker thread before spawning the daemon.
///
/// Idempotent: returns immediately if all three are already granted;
/// only shows the explanatory dialog + prompts if any are pending.
pub fn ensure_permissions() {
    // Mic and IM statuses are safe to query here. AX status check
    // (AXIsProcessTrustedWithOptions) must NOT run before
    // IOHIDRequestAccess — Apple bug FB7381305 silently suppresses the
    // IM prompt if AX was touched earlier in the process. So we defer
    // every AX-related call until after IM has had its chance.
    let mic = mic_status();
    let im_granted = input_monitoring_granted();

    let mic_pending = mic == AvAuthStatus::NotDetermined;
    let im_pending = !im_granted;

    // Fast path: if mic and IM are already granted, it's safe to check
    // AX (no IOHIDRequestAccess will fire in this process). If AX is
    // also granted, everything is in place — no prompts, no dialogs.
    if !mic_pending && !im_pending && is_accessibility_trusted() {
        log::info!("all permissions already granted — skipping prompts");
        return;
    }

    log::info!(
        "permission status: mic={mic:?} input_monitoring_granted={im_granted} (AX deferred)"
    );
    show_permissions_intro_dialog();

    // Fire the prompts. Mic blocks on its completion handler (so the
    // user dismisses its prompt before IM's fires). IM and AX are
    // fire-and-forget — their prompts queue in macOS.
    if mic_pending {
        let mic_granted = request_mic_access_blocking();
        log::info!("mic prompt resolved: granted={mic_granted}");
    }
    if im_pending {
        let _ = request_input_monitoring();
        log::info!("input monitoring prompt fired");
    }
    // AX is safe to touch now that IM's request has been issued.
    let ax_trusted = is_accessibility_trusted();
    if !ax_trusted {
        let _ = prompt_accessibility();
        log::info!("accessibility prompt fired");
    }

    // Apple's own DTS position: IM and AX grants don't take effect
    // for the running process — only for a new process. Even if our
    // polling shows "granted" mid-session, the watcher's CGEventTap
    // and any CGEventPost calls won't actually work until we relaunch.
    // So we always tell the user to quit + reopen after granting.
    //
    // We do a short wait first so the dialog doesn't fire while the
    // user is still clicking through the OS prompts (confusing UX).
    wait_for_prompts_to_settle(60);

    show_restart_required_dialog();
}

/// Wait up to `timeout_secs` seconds, returning as soon as IM and AX
/// both report granted OR both report denied. We use this just to avoid
/// showing the "quit and reopen" dialog while the user is still in the
/// middle of clicking through OS prompts; it's not load-bearing for
/// actually detecting grants (that only takes effect on relaunch).
fn wait_for_prompts_to_settle(timeout_secs: u64) {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let im_decided =
            input_monitoring_granted() || input_monitoring_denied();
        let ax_decided = is_accessibility_trusted();
        // For AX we only have "trusted" vs "not trusted" — no way to
        // distinguish denied from pending. Best we can do is poll for
        // trusted=true; give up when the timeout hits.
        if im_decided && ax_decided {
            return;
        }
    }
}

/// Final dialog telling the user they need to quit + reopen utter for
/// Input Monitoring and Accessibility grants to actually activate in
/// the running process. Per Apple DTS: permission changes only take
/// effect on relaunch.
fn show_restart_required_dialog() {
    let body = "Permissions set.\n\n\
        Input Monitoring and Accessibility grants only activate when utter restarts. \
        Quit utter now (\u{2318}Q in the menu bar) and reopen it — dictation will work \
        on the next launch.";
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        r#"display dialog "{}" with title "One more step: restart utter" buttons {{"OK"}} default button "OK""#,
        escape(body),
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .status();
}


// --- Audio capture ---------------------------------------------------------
//
// cpal streams aren't Send — we confine the stream to its own std::thread and
// exchange samples via a shared Arc<Mutex<Vec<f32>>>. stop_audio signals via a
// oneshot channel, joins the thread, and downsamples to 16 kHz mono f32 for
// Parakeet.

const TARGET_SAMPLE_RATE: u32 = 16_000;

pub struct AudioCapture {
    shared: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
    stop_tx: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<Result<u32>>,  // returns source sample rate
    channels: u16,
}

fn build_input_stream(
    target: &std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
    channels_out: &mut u16,
    rate_out: &mut u32,
) -> Result<cpal::Stream> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device"))?;
    let supported = device
        .default_input_config()
        .context("query default input config")?;
    let sample_format = supported.sample_format();
    let channels = supported.channels();
    let rate = supported.sample_rate().0;
    *channels_out = channels;
    *rate_out = rate;
    let config: cpal::StreamConfig = supported.clone().into();

    log::info!(
        "cpal input: device={:?} rate={} channels={} format={:?}",
        device.name().ok(),
        rate,
        channels,
        sample_format
    );

    let buf = target.clone();
    let err_fn = |e| log::warn!("cpal stream error: {e}");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| {
                if let Ok(mut v) = buf.lock() {
                    v.extend_from_slice(data);
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| {
                if let Ok(mut v) = buf.lock() {
                    v.extend(data.iter().map(|&s| s as f32 / 32768.0));
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _| {
                if let Ok(mut v) = buf.lock() {
                    v.extend(data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0));
                }
            },
            err_fn,
            None,
        ),
        other => return Err(anyhow!("unsupported cpal sample format: {other:?}")),
    }
    .context("cpal build_input_stream")?;
    Ok(stream)
}

pub async fn start_audio() -> Result<AudioCapture> {
    let shared: std::sync::Arc<std::sync::Mutex<Vec<f32>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(16_000 * 10)));
    let shared_for_thread = shared.clone();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(u16, u32)>>();

    let thread = std::thread::spawn(move || -> Result<u32> {
        let mut channels = 0u16;
        let mut rate = 0u32;
        let stream = match build_input_stream(&shared_for_thread, &mut channels, &mut rate) {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(anyhow!("{e:#}")));
                return Err(e);
            }
        };
        use cpal::traits::StreamTrait;
        if let Err(e) = stream.play() {
            let _ = ready_tx.send(Err(anyhow!("stream.play: {e}")));
            return Err(anyhow!("stream.play: {e}"));
        }
        let _ = ready_tx.send(Ok((channels, rate)));

        // Blocking recv from the std mpsc stop channel. cpal callbacks run
        // on its own CoreAudio thread; this thread just parks until told
        // to stop, then drops the stream which triggers teardown.
        let _ = stop_rx.recv();
        drop(stream);
        Ok(rate)
    });

    let (channels, _rate) = ready_rx
        .recv()
        .map_err(|_| anyhow!("cpal thread died before sending ready"))??;

    Ok(AudioCapture {
        shared,
        stop_tx,
        thread,
        channels,
    })
}

pub async fn stop_audio(capture: AudioCapture) -> Result<Vec<f32>> {
    let AudioCapture { shared, stop_tx, thread, channels } = capture;
    let _ = stop_tx.send(());
    let rate = tokio::task::spawn_blocking(move || thread.join())
        .await
        .map_err(|e| anyhow!("join cpal thread: {e:#}"))?
        .map_err(|_| anyhow!("cpal thread panicked"))??;
    let mut samples = {
        let locked = shared.lock().map_err(|e| anyhow!("lock samples: {e}"))?;
        locked.clone()
    };
    // Downmix to mono if the device captured stereo+.
    if channels > 1 {
        let c = channels as usize;
        let mut mono = Vec::with_capacity(samples.len() / c);
        for chunk in samples.chunks_exact(c) {
            let sum: f32 = chunk.iter().sum();
            mono.push(sum / c as f32);
        }
        samples = mono;
    }
    // Naive downsample to 16 kHz — pick every Nth sample. Sufficient for a
    // short-utterance PoC; upgrade to rubato or similar if quality matters.
    if rate != TARGET_SAMPLE_RATE && rate > 0 {
        let ratio = rate as f64 / TARGET_SAMPLE_RATE as f64;
        let out_len = (samples.len() as f64 / ratio) as usize;
        let mut out = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let src = (i as f64 * ratio) as usize;
            if src < samples.len() {
                out.push(samples[src]);
            }
        }
        samples = out;
    }
    Ok(samples)
}

// --- Text emission ---------------------------------------------------------
//
// Pasteboard -> synthesize Cmd+V. No ydotool, no wl-copy, no subprocess.
// Custom pasteboard type "com.utter.dictation" rides alongside plain text so
// users who run Paste / Maccy / Alfred can configure their clipboard manager
// to filter dictations out of history.

const UTTER_PASTEBOARD_TYPE: &str = "com.utter.dictation";

fn write_pasteboard(text: &str) -> Result<()> {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::{NSArray, NSString};

    // Every call into AppKit here is Obj-C. The unsafety is real (we're
    // crossing an FFI boundary and trusting AppKit's contracts); none of
    // it is memory-unsafe from our side as long as we pass valid refs.
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        let custom_type = NSString::from_str(UTTER_PASTEBOARD_TYPE);

        // NSString has ImmutableWithMutableSubclass mutability, which means
        // &NSString doesn't implement IsRetainable — so NSArray::from_slice
        // rejects it. Go through raw-pointer retain to get a Retained<NSString>
        // we can put in a Vec for NSArray::from_vec.
        let plain_type: Retained<NSString> = Retained::retain(
            NSPasteboardTypeString as *const NSString as *mut NSString,
        )
        .ok_or_else(|| anyhow!("NSPasteboardTypeString was null"))?;
        let types = NSArray::from_vec(vec![plain_type, custom_type.clone()]);
        pb.declareTypes_owner(&types, None);

        let ns_text = NSString::from_str(text);
        pb.setString_forType(&ns_text, NSPasteboardTypeString);
        pb.setString_forType(&ns_text, &custom_type);
    }
    Ok(())
}

fn synthesize_cmd_v() -> Result<()> {
    use core_graphics::event::{
        CGEvent, CGEventFlags, CGEventTapLocation, KeyCode as CGKeyCodes,
    };
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // ANSI 'V' virtual keycode. core-graphics 0.24 doesn't re-export ANSI
    // letter codes as named constants.
    const ANSI_V: u16 = 0x09;

    let src = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("create CGEventSource"))?;

    // Cmd down -> V down (with Cmd flag) -> V up (with Cmd flag) -> Cmd up.
    // Setting the Cmd flag on the V events is the bit that actually makes
    // the app see "Cmd+V" as a keyboard shortcut; without it, some apps
    // treat the V as a plain letter and type 'v' instead of pasting.
    let cmd_down = CGEvent::new_keyboard_event(src.clone(), CGKeyCodes::COMMAND, true)
        .map_err(|_| anyhow!("create Cmd down event"))?;
    let v_down = CGEvent::new_keyboard_event(src.clone(), ANSI_V, true)
        .map_err(|_| anyhow!("create V down event"))?;
    v_down.set_flags(CGEventFlags::CGEventFlagCommand);
    let v_up = CGEvent::new_keyboard_event(src.clone(), ANSI_V, false)
        .map_err(|_| anyhow!("create V up event"))?;
    v_up.set_flags(CGEventFlags::CGEventFlagCommand);
    let cmd_up = CGEvent::new_keyboard_event(src, CGKeyCodes::COMMAND, false)
        .map_err(|_| anyhow!("create Cmd up event"))?;

    // Post into the current session so the keystroke lands in whatever app
    // currently has keyboard focus, after any session-level event taps.
    cmd_down.post(CGEventTapLocation::AnnotatedSession);
    v_down.post(CGEventTapLocation::AnnotatedSession);
    v_up.post(CGEventTapLocation::AnnotatedSession);
    cmd_up.post(CGEventTapLocation::AnnotatedSession);
    Ok(())
}

pub async fn emit_text(text: &str, cfg: &Config) -> Result<()> {
    // Pasteboard write + key synthesis are both synchronous AppKit /
    // CoreGraphics calls; wrap the whole thing in spawn_blocking so tokio's
    // reactor stays free. Each chunk is microseconds on Apple Silicon.
    let text = text.to_string();
    let auto_paste = cfg.auto_paste;
    tokio::task::spawn_blocking(move || -> Result<()> {
        write_pasteboard(&text)?;
        if auto_paste {
            synthesize_cmd_v()?;
        }
        Ok(())
    })
    .await?
}

// --- Key parsing / naming --------------------------------------------------
//
// macOS virtual keycodes (kVK_*) come from Carbon's HIToolbox/Events.h. They
// aren't re-exported by core-graphics, so we define the ones we care about
// locally rather than pulling in a Carbon binding. Numbers match
// /System/Library/Frameworks/Carbon.framework/Headers/Events.h.

pub const KVK_RETURN: u16 = 0x24;
pub const KVK_TAB: u16 = 0x30;
pub const KVK_SPACE: u16 = 0x31;
pub const KVK_DELETE: u16 = 0x33;
pub const KVK_ESCAPE: u16 = 0x35;
pub const KVK_COMMAND: u16 = 0x37;      // left
pub const KVK_SHIFT: u16 = 0x38;        // left
pub const KVK_CAPSLOCK: u16 = 0x39;
pub const KVK_OPTION: u16 = 0x3A;       // left
pub const KVK_CONTROL: u16 = 0x3B;      // left
pub const KVK_RIGHT_SHIFT: u16 = 0x3C;
pub const KVK_RIGHT_OPTION: u16 = 0x3D;
pub const KVK_RIGHT_CONTROL: u16 = 0x3E;
pub const KVK_FUNCTION: u16 = 0x3F;
pub const KVK_RIGHT_COMMAND: u16 = 0x36;

pub const KVK_F1: u16 = 0x7A;
pub const KVK_F2: u16 = 0x78;
pub const KVK_F3: u16 = 0x63;
pub const KVK_F4: u16 = 0x76;
pub const KVK_F5: u16 = 0x60;
pub const KVK_F6: u16 = 0x61;
pub const KVK_F7: u16 = 0x62;
pub const KVK_F8: u16 = 0x64;
pub const KVK_F9: u16 = 0x65;
pub const KVK_F10: u16 = 0x6D;
pub const KVK_F11: u16 = 0x67;
pub const KVK_F12: u16 = 0x6F;
pub const KVK_F13: u16 = 0x69;
pub const KVK_F14: u16 = 0x6B;
pub const KVK_F15: u16 = 0x71;
pub const KVK_F16: u16 = 0x6A;
pub const KVK_F17: u16 = 0x40;
pub const KVK_F18: u16 = 0x4F;
pub const KVK_F19: u16 = 0x50;
pub const KVK_F20: u16 = 0x5A;

pub const KVK_HOME: u16 = 0x73;
pub const KVK_PAGE_UP: u16 = 0x74;
pub const KVK_FORWARD_DELETE: u16 = 0x75;
pub const KVK_END: u16 = 0x77;
pub const KVK_PAGE_DOWN: u16 = 0x79;

pub const KVK_ANSI_V: u16 = 0x09;

/// Maps a modifier keycode to the specific bit in a CGEventFlags mask that
/// reflects "this exact key is currently held." On FlagsChanged, the event
/// carries the keycode of the modifier whose state changed and a CGEventFlags
/// snapshot of the new state — checking whether that keycode's bit is set in
/// the new mask tells you press (set) vs release (cleared). The low bits come
/// from NX_* device-specific masks; caps lock and fn only have class bits.
pub fn modifier_flag_bit(code: u16) -> Option<u64> {
    Some(match code {
        KVK_CONTROL         => 1 << 0,
        KVK_SHIFT           => 1 << 1,
        KVK_RIGHT_SHIFT     => 1 << 2,
        KVK_COMMAND         => 1 << 3,
        KVK_RIGHT_COMMAND   => 1 << 4,
        KVK_OPTION          => 1 << 5,
        KVK_RIGHT_OPTION    => 1 << 6,
        KVK_RIGHT_CONTROL   => 1 << 13,
        KVK_CAPSLOCK        => 1 << 16,   // class bit only (no separate side bit)
        KVK_FUNCTION        => 1 << 23,
        _ => return None,
    })
}

pub fn is_modifier(code: u16) -> bool {
    modifier_flag_bit(code).is_some()
}

pub fn parse_key_name(name: &str) -> Result<u16> {
    let n = name.to_ascii_lowercase();
    let n = n.strip_prefix("key_").unwrap_or(&n);

    let code = match n {
        // Cross-platform aliases — keep the same names the Linux parser uses
        // so config.toml is portable.
        "rightmeta" | "rightsuper" | "rightcmd" | "rightcommand" => KVK_RIGHT_COMMAND,
        "leftmeta" | "leftsuper" | "leftcmd" | "leftcommand" | "meta" | "cmd" | "command" => KVK_COMMAND,
        "rightctrl" | "rightcontrol" => KVK_RIGHT_CONTROL,
        "leftctrl" | "leftcontrol" | "ctrl" | "control" => KVK_CONTROL,
        "rightalt" | "rightoption" | "rightopt" => KVK_RIGHT_OPTION,
        "leftalt" | "leftoption" | "leftopt" | "alt" | "option" | "opt" => KVK_OPTION,
        "rightshift" => KVK_RIGHT_SHIFT,
        "leftshift" | "shift" => KVK_SHIFT,
        "capslock" | "caps" => KVK_CAPSLOCK,
        "fn" | "function" => KVK_FUNCTION,
        "return" | "enter" => KVK_RETURN,
        "tab" => KVK_TAB,
        "space" => KVK_SPACE,
        "delete" | "backspace" => KVK_DELETE,
        "escape" | "esc" => KVK_ESCAPE,
        "home" => KVK_HOME,
        "end" => KVK_END,
        "pageup" | "pgup" | "page_up" => KVK_PAGE_UP,
        "pagedown" | "pgdn" | "page_down" => KVK_PAGE_DOWN,
        "forwarddelete" | "fwddelete" => KVK_FORWARD_DELETE,
        "f1" => KVK_F1,
        "f2" => KVK_F2,
        "f3" => KVK_F3,
        "f4" => KVK_F4,
        "f5" => KVK_F5,
        "f6" => KVK_F6,
        "f7" => KVK_F7,
        "f8" => KVK_F8,
        "f9" => KVK_F9,
        "f10" => KVK_F10,
        "f11" => KVK_F11,
        "f12" => KVK_F12,
        "f13" => KVK_F13,
        "f14" => KVK_F14,
        "f15" => KVK_F15,
        "f16" => KVK_F16,
        "f17" => KVK_F17,
        "f18" => KVK_F18,
        "f19" => KVK_F19,
        "f20" => KVK_F20,
        _ => {
            // Numeric fallback: decimal or 0x-prefixed hex.
            if let Some(hex) = n.strip_prefix("0x") {
                if let Ok(v) = u16::from_str_radix(hex, 16) {
                    return Ok(v);
                }
            }
            if let Ok(v) = n.parse::<u16>() {
                return Ok(v);
            }
            return Err(anyhow!(
                "unknown key name: `{name}` (not a named alias and not a numeric kVK code)"
            ));
        }
    };
    Ok(code)
}

pub fn canonical_name_for(code: u16) -> Option<&'static str> {
    Some(match code {
        KVK_RIGHT_COMMAND => "rightmeta",
        KVK_COMMAND => "leftmeta",
        KVK_RIGHT_CONTROL => "rightctrl",
        KVK_CONTROL => "leftctrl",
        KVK_RIGHT_OPTION => "rightalt",
        KVK_OPTION => "leftalt",
        KVK_RIGHT_SHIFT => "rightshift",
        KVK_SHIFT => "leftshift",
        KVK_CAPSLOCK => "capslock",
        KVK_FUNCTION => "fn",
        KVK_RETURN => "return",
        KVK_TAB => "tab",
        KVK_SPACE => "space",
        KVK_DELETE => "delete",
        KVK_ESCAPE => "escape",
        KVK_HOME => "home",
        KVK_END => "end",
        KVK_PAGE_UP => "pageup",
        KVK_PAGE_DOWN => "pagedown",
        KVK_FORWARD_DELETE => "forwarddelete",
        KVK_F1 => "f1",
        KVK_F2 => "f2",
        KVK_F3 => "f3",
        KVK_F4 => "f4",
        KVK_F5 => "f5",
        KVK_F6 => "f6",
        KVK_F7 => "f7",
        KVK_F8 => "f8",
        KVK_F9 => "f9",
        KVK_F10 => "f10",
        KVK_F11 => "f11",
        KVK_F12 => "f12",
        KVK_F13 => "f13",
        KVK_F14 => "f14",
        KVK_F15 => "f15",
        KVK_F16 => "f16",
        KVK_F17 => "f17",
        KVK_F18 => "f18",
        KVK_F19 => "f19",
        KVK_F20 => "f20",
        _ => return None,
    })
}

// --- Input Monitoring preflight ------------------------------------------
//
// core-graphics 0.24 doesn't expose the TCC preflight/request APIs that
// appeared in macOS 10.15 and stabilized in 11+. Declare them ourselves.
// Both return `true` when Input Monitoring is currently granted. Request
// also triggers the system prompt if not yet granted; it's what actually
// makes utter show up in System Settings > Privacy & Security > Input
// Monitoring so the user has something to toggle.

// IOHID equivalents of the older CG calls. Apple deprecated the CG path
// (CGPreflightListenEventAccess / CGRequestListenEventAccess) in favor of
// IOKit's IOHIDCheckAccess / IOHIDRequestAccess, which behave more
// reliably for ad-hoc-signed bundles: the CG variants have been observed
// to return `true` (granted) without ever surfacing the prompt or
// registering the app in System Settings > Privacy > Input Monitoring,
// so the user has nothing to toggle and key events never arrive.
//
// kIOHIDRequestType from IOKit/hid/IOHIDLib.h
const KIO_HID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;
// kIOHIDAccessType
const KIO_HID_ACCESS_TYPE_GRANTED: u32 = 0;
const KIO_HID_ACCESS_TYPE_DENIED: u32 = 1;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDCheckAccess(request: u32) -> u32;
    fn IOHIDRequestAccess(request: u32) -> bool;
}

pub fn input_monitoring_granted() -> bool {
    unsafe { IOHIDCheckAccess(KIO_HID_REQUEST_TYPE_LISTEN_EVENT) == KIO_HID_ACCESS_TYPE_GRANTED }
}

fn input_monitoring_denied() -> bool {
    unsafe { IOHIDCheckAccess(KIO_HID_REQUEST_TYPE_LISTEN_EVENT) == KIO_HID_ACCESS_TYPE_DENIED }
}

/// Trigger the Input Monitoring prompt and register the bundle in the
/// system's Input Monitoring list. Returns whether access was granted
/// synchronously — false typically just means the prompt is showing
/// asynchronously, not that the user denied it.
pub fn request_input_monitoring() -> bool {
    unsafe { IOHIDRequestAccess(KIO_HID_REQUEST_TYPE_LISTEN_EVENT) }
}

/// Poke the TCC subsystem so utter lands in the Input Monitoring list and
/// the user gets a prompt on first-run. Does NOT gate on the preflight
/// result — the in-process preflight cache has been observed to return
/// false even after the user explicitly toggles the grant on, so a hard
/// gate produces false negatives. The tap setup will still "succeed"
/// without permission but deliver no events; the caller is responsible
/// for timing out if nothing arrives.
pub fn ensure_input_monitoring(_subcommand: &str) -> Result<()> {
    let pre = input_monitoring_granted();
    if pre {
        return Ok(());
    }
    let post = request_input_monitoring();
    log::info!(
        "Input Monitoring: preflight={pre} post_request={post}. Note: macOS \
         does NOT show a prompt for unsigned CLI binaries. If keys don't \
         register, open System Settings > Privacy & Security > Input \
         Monitoring, click `+`, add target/release/utter, and re-run. \
         Rebuilding invalidates the grant (CDHash changes), so re-add after \
         each `cargo build`."
    );
    Ok(())
}

// --- Watcher ---------------------------------------------------------------
//
// The CGEventTap callback runs on a CFRunLoop thread, not a tokio executor.
// Shape: spawn a plain std::thread that owns the runloop + tap, forwards
// press/release events to tokio through an UnboundedSender. Modifier keys
// come through FlagsChanged (no KeyDown/KeyUp); non-modifiers come through
// KeyDown/KeyUp directly. On tap-disabled events we re-enable so the OS
// doesn't silently kill the tap after an unrelated hang.

#[derive(Copy, Clone, Debug)]
enum KeyEvent {
    Press,
    Release,
}

fn spawn_event_tap(
    target: u16,
    tx: tokio::sync::mpsc::UnboundedSender<KeyEvent>,
    options: core_graphics::event::CGEventTapOptions,
    ready_tx: std::sync::mpsc::Sender<Result<()>>,
) -> std::thread::JoinHandle<()> {
    use core_foundation::base::TCFType;
    use core_foundation::mach_port::CFMachPortInvalidate;
    use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
    use core_graphics::event::{
        CGEvent, CGEventTap, CGEventTapLocation, CGEventTapPlacement, CGEventType,
    };

    let target_is_modifier = is_modifier(target);
    let target_bit = modifier_flag_bit(target);
    let previous_pressed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    std::thread::spawn(move || {
        let prev = previous_pressed.clone();
        let tx = tx.clone();
        let tap_result = CGEventTap::new(
            CGEventTapLocation::Session,
            CGEventTapPlacement::HeadInsertEventTap,
            options,
            vec![
                CGEventType::KeyDown,
                CGEventType::KeyUp,
                CGEventType::FlagsChanged,
                CGEventType::TapDisabledByTimeout,
                CGEventType::TapDisabledByUserInput,
            ],
            move |_proxy, etype, event: &CGEvent| -> Option<CGEvent> {
                match etype {
                    CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                        // The tap-disabled events are not delivered with a
                        // keycode payload; we re-enable via the tap handle
                        // outside the closure (see below). For listen-only
                        // mode the OS uses a shorter timeout, but re-arming
                        // is still the fix in both modes.
                        log::warn!("CGEventTap disabled ({etype:?}) — re-enabling");
                    }
                    CGEventType::KeyDown | CGEventType::KeyUp if !target_is_modifier => {
                        let keycode =
                            event.get_integer_value_field(core_graphics::event::EventField::KEYBOARD_EVENT_KEYCODE)
                                as u16;
                        if keycode == target {
                            let is_down = matches!(etype, CGEventType::KeyDown);
                            let was = prev.swap(is_down, std::sync::atomic::Ordering::SeqCst);
                            if was != is_down {
                                let _ = tx.send(if is_down {
                                    KeyEvent::Press
                                } else {
                                    KeyEvent::Release
                                });
                            }
                        }
                    }
                    CGEventType::FlagsChanged if target_is_modifier => {
                        let keycode =
                            event.get_integer_value_field(core_graphics::event::EventField::KEYBOARD_EVENT_KEYCODE)
                                as u16;
                        if keycode == target {
                            if let Some(bit) = target_bit {
                                let flags = event.get_flags().bits();
                                let is_down = (flags & bit) != 0;
                                let was = prev.swap(is_down, std::sync::atomic::Ordering::SeqCst);
                                if was != is_down {
                                    let _ = tx.send(if is_down {
                                        KeyEvent::Press
                                    } else {
                                        KeyEvent::Release
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
                None
            },
        );

        let tap = match tap_result {
            Ok(t) => t,
            Err(()) => {
                let _ = ready_tx.send(Err(anyhow!(
                    "CGEventTapCreate returned NULL. Grant Input Monitoring \
                     to utter in System Settings > Privacy & Security > Input \
                     Monitoring, then rerun."
                )));
                return;
            }
        };

        // Wire the tap into this thread's runloop.
        unsafe {
            let src = match tap.mach_port.create_runloop_source(0) {
                Ok(s) => s,
                Err(()) => {
                    let _ = ready_tx.send(Err(anyhow!("create_runloop_source failed")));
                    return;
                }
            };
            CFRunLoop::get_current().add_source(&src, kCFRunLoopCommonModes);
            tap.enable();
            let _ = ready_tx.send(Ok(()));
            CFRunLoop::run_current();
            // Invalidate the port on exit so the runloop source is released.
            CFMachPortInvalidate(tap.mach_port.as_concrete_TypeRef());
        }
    })
}

pub async fn run_watcher(key_arg: Option<&str>) -> Result<()> {
    use core_graphics::event::CGEventTapOptions;

    let cfg = Config::load_or_migrate(
        &Config::default_path()?,
        &crate::config::utter_env_snapshot(),
    )?;
    let key_name = key_arg.unwrap_or(&cfg.key).to_string();
    let target = parse_key_name(&key_name)?;
    log::info!("watching for key {key_name} (kVK 0x{:02X})", target);

    ensure_input_monitoring("watcher")?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<KeyEvent>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    // ListenOnly is enough — we observe press/release, never modify or drop
    // events. Default (intercept) mode would require Accessibility on top of
    // Input Monitoring.
    let _thread = spawn_event_tap(target, tx, CGEventTapOptions::ListenOnly, ready_tx);
    ready_rx
        .recv()
        .map_err(|_| anyhow!("event tap thread died before signalling ready"))??;
    log::info!("event tap installed; holding key triggers dictation");

    while let Some(ev) = rx.recv().await {
        match ev {
            KeyEvent::Press => {
                log::debug!("press -> start");
                if let Err(e) = crate::send_command_quiet("start").await {
                    log::warn!("start: {e:#}");
                }
            }
            KeyEvent::Release => {
                log::debug!("release -> stop");
                if let Err(e) = crate::send_command_quiet("stop").await {
                    log::warn!("stop: {e:#}");
                }
            }
        }
    }
    Ok(())
}

// --- set-key picker --------------------------------------------------------
//
// Listen-only CGEventTap captures the first key-down event (any keycode),
// then waits for the matching release. Then it stops the runloop, returning
// the captured keycode to the tokio side for pretty-printing + config save.

#[derive(Copy, Clone, Debug)]
enum PickEvent {
    Pressed(u16),
    Released(u16),
}

fn spawn_picker_tap(
    tx: tokio::sync::mpsc::UnboundedSender<PickEvent>,
    ready_tx: std::sync::mpsc::Sender<Result<()>>,
) -> std::thread::JoinHandle<()> {
    use core_foundation::base::TCFType;
    use core_foundation::mach_port::CFMachPortInvalidate;
    use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
    use core_graphics::event::{
        CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventType,
    };
    use std::sync::atomic::{AtomicU32, Ordering};

    std::thread::spawn(move || {
        // Shared between callback invocations. u32::MAX = "not yet captured".
        let captured: std::sync::Arc<AtomicU32> =
            std::sync::Arc::new(AtomicU32::new(u32::MAX));
        let captured_cb = captured.clone();
        let tx_cb = tx.clone();

        let tap_result = CGEventTap::new(
            CGEventTapLocation::Session,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            vec![
                CGEventType::KeyDown,
                CGEventType::KeyUp,
                CGEventType::FlagsChanged,
                CGEventType::TapDisabledByTimeout,
                CGEventType::TapDisabledByUserInput,
            ],
            move |_proxy, etype, event: &CGEvent| -> Option<CGEvent> {
                match etype {
                    CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                        log::warn!("picker tap disabled ({etype:?}); re-enable on next event");
                        return None;
                    }
                    CGEventType::KeyDown => {
                        let code = event.get_integer_value_field(
                            core_graphics::event::EventField::KEYBOARD_EVENT_KEYCODE,
                        ) as u16;
                        if captured_cb
                            .compare_exchange(
                                u32::MAX,
                                code as u32,
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                            )
                            .is_ok()
                        {
                            let _ = tx_cb.send(PickEvent::Pressed(code));
                        }
                    }
                    CGEventType::KeyUp => {
                        let code = event.get_integer_value_field(
                            core_graphics::event::EventField::KEYBOARD_EVENT_KEYCODE,
                        ) as u16;
                        if captured_cb.load(Ordering::SeqCst) == code as u32 {
                            let _ = tx_cb.send(PickEvent::Released(code));
                            CFRunLoop::get_current().stop();
                        }
                    }
                    CGEventType::FlagsChanged => {
                        let code = event.get_integer_value_field(
                            core_graphics::event::EventField::KEYBOARD_EVENT_KEYCODE,
                        ) as u16;
                        if let Some(bit) = modifier_flag_bit(code) {
                            let is_down = (event.get_flags().bits() & bit) != 0;
                            let current = captured_cb.load(Ordering::SeqCst);
                            if is_down && current == u32::MAX {
                                if captured_cb
                                    .compare_exchange(
                                        u32::MAX,
                                        code as u32,
                                        Ordering::SeqCst,
                                        Ordering::SeqCst,
                                    )
                                    .is_ok()
                                {
                                    let _ = tx_cb.send(PickEvent::Pressed(code));
                                }
                            } else if !is_down && current == code as u32 {
                                let _ = tx_cb.send(PickEvent::Released(code));
                                CFRunLoop::get_current().stop();
                            }
                        }
                    }
                    _ => {}
                }
                None
            },
        );

        let tap = match tap_result {
            Ok(t) => t,
            Err(()) => {
                let _ = ready_tx.send(Err(anyhow!(
                    "CGEventTapCreate returned NULL. Grant Input Monitoring \
                     to utter in System Settings > Privacy & Security > Input \
                     Monitoring, then re-run `utter set-key`."
                )));
                return;
            }
        };

        unsafe {
            let src = match tap.mach_port.create_runloop_source(0) {
                Ok(s) => s,
                Err(()) => {
                    let _ = ready_tx.send(Err(anyhow!("create_runloop_source failed")));
                    return;
                }
            };
            CFRunLoop::get_current().add_source(&src, kCFRunLoopCommonModes);
            tap.enable();
            let _ = ready_tx.send(Ok(()));
            CFRunLoop::run_current();
            CFMachPortInvalidate(tap.mach_port.as_concrete_TypeRef());
        }
    })
}

pub async fn run_set_key(dry_run: bool, timeout_secs: u64) -> Result<()> {
    ensure_input_monitoring("set-key")?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PickEvent>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    let thread = spawn_picker_tap(tx, ready_tx);
    ready_rx
        .recv()
        .map_err(|_| anyhow!("picker thread died before signalling ready"))??;

    eprintln!(
        "Press and hold the key you want to use for push-to-talk, then release it.\n\
         (Ctrl+C to cancel; timeout in {timeout_secs}s.)"
    );

    let deadline = std::time::Duration::from_secs(timeout_secs);
    let mut pressed: Option<u16> = None;
    let code: u16 = loop {
        let ev = tokio::time::timeout(deadline, rx.recv())
            .await
            .map_err(|_| anyhow!("timed out — no press + release captured"))?
            .ok_or_else(|| anyhow!("picker channel closed"))?;
        match ev {
            PickEvent::Pressed(c) => pressed = Some(c),
            PickEvent::Released(c) if pressed == Some(c) => break c,
            PickEvent::Released(_) => {}
        }
    };
    // Wait for the runloop thread to exit cleanly.
    let _ = tokio::task::spawn_blocking(move || thread.join()).await;

    let name: String = canonical_name_for(code)
        .map(String::from)
        .unwrap_or_else(|| format!("0x{:02X}", code));

    println!(
        "Detected: {name} (kVK 0x{:02X}). Press + release both captured — hold-to-talk will work.",
        code
    );

    if dry_run {
        eprintln!("(--dry-run: not saving.)");
        return Ok(());
    }

    let path = Config::default_path()?;
    let env = crate::config::utter_env_snapshot();
    let cfg = Config::load_or_migrate(&path, &env)?.with_key(&name);
    cfg.save_to(&path)?;
    println!(
        "Saved key `{name}` to {}. Restart the watcher to pick it up.",
        path.display()
    );
    Ok(())
}

