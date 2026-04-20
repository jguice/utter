use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams};
use transcribe_rs::onnx::Quantization;

#[derive(Parser)]
#[command(name = "utter", version, about = "Local push-to-talk voice dictation")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the long-lived daemon (loads Parakeet, binds socket).
    Daemon {
        /// Override model directory (default: $XDG_DATA_HOME/utter/models/parakeet-tdt-0.6b-v3-int8)
        #[arg(long)]
        model: Option<PathBuf>,
    },
    /// Begin recording.
    Start,
    /// Stop recording, transcribe, emit text to clipboard (auto-paste if UTTER_AUTOTYPE=1).
    Stop,
    /// Start if idle, stop if recording.
    Toggle,
    /// Shut the daemon down.
    Quit,
    /// Print the socket path.
    SocketPath,
    /// Watch /dev/input for hold-to-talk: start on key down, stop on key up.
    /// Requires the running user to be in the `input` group.
    Watch {
        /// Key name (e.g. rightmeta, leftmeta, rightctrl, capslock, f13).
        #[arg(long, default_value = "rightmeta")]
        key: String,
    },
    /// Interactively pick the push-to-talk key. Stops the watcher, listens
    /// for a press + release on any keyboard, then (unless --dry-run)
    /// writes a systemd user override so the watcher uses that key.
    SetKey {
        /// Just report the detected key — don't save or restart the watcher.
        #[arg(long)]
        dry_run: bool,
        /// Seconds to wait for a key press before giving up.
        #[arg(long, default_value = "20")]
        timeout: u64,
    },
}

fn socket_path() -> Result<PathBuf> {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR not set (need an active systemd user session)")?;
    Ok(PathBuf::from(runtime).join("utter.sock"))
}

fn default_model_dir() -> Result<PathBuf> {
    let data = dirs::data_dir().ok_or_else(|| anyhow!("no XDG data dir"))?;
    Ok(data.join("utter/models/parakeet-tdt-0.6b-v3-int8"))
}

fn fresh_wav_path() -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("utter-{ts}.wav"))
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Daemon { model } => run_daemon(model).await,
        Cmd::Start => send_command("start").await,
        Cmd::Stop => send_command("stop").await,
        Cmd::Toggle => send_command("toggle").await,
        Cmd::Quit => send_command("quit").await,
        Cmd::SocketPath => {
            println!("{}", socket_path()?.display());
            Ok(())
        }
        Cmd::Watch { key } => run_watcher(&key).await,
        Cmd::SetKey { dry_run, timeout } => run_set_key(dry_run, timeout).await,
    }
}

async fn send_command(cmd: &str) -> Result<()> {
    let path = socket_path()?;
    let mut stream = UnixStream::connect(&path)
        .await
        .with_context(|| format!("daemon not running at {}", path.display()))?;
    stream.write_all(format!("{cmd}\n").as_bytes()).await?;
    stream.shutdown().await.ok();

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    while reader.read_line(&mut line).await? > 0 {
        print!("{line}");
        line.clear();
    }
    Ok(())
}

enum State {
    Idle,
    Recording { child: Child, wav_path: PathBuf },
}

struct Daemon {
    model: Arc<Mutex<ParakeetModel>>,
    state: Mutex<State>,
}

async fn run_daemon(model_override: Option<PathBuf>) -> Result<()> {
    let model_dir = model_override.map(Ok).unwrap_or_else(default_model_dir)?;
    if !model_dir.exists() {
        return Err(anyhow!(
            "model directory does not exist: {}\nRun scripts/download-model.sh first.",
            model_dir.display()
        ));
    }
    log::info!("loading Parakeet from {}", model_dir.display());
    let load_start = Instant::now();
    let model = tokio::task::spawn_blocking({
        let dir = model_dir.clone();
        move || ParakeetModel::load(&dir, &Quantization::Int8)
    })
    .await??;
    log::info!("model loaded in {:?}", load_start.elapsed());

    let socket = socket_path()?;
    if socket.exists() {
        let _ = std::fs::remove_file(&socket);
    }
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("bind {}", socket.display()))?;
    log::info!("listening on {}", socket.display());

    let daemon = Arc::new(Daemon {
        model: Arc::new(Mutex::new(model)),
        state: Mutex::new(State::Idle),
    });

    let sock_cleanup = socket.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = std::fs::remove_file(&sock_cleanup);
        std::process::exit(0);
    });

    loop {
        let (stream, _) = listener.accept().await?;
        let d = daemon.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(d, stream).await {
                log::warn!("client error: {e:#}");
            }
        });
    }
}

async fn handle_client(daemon: Arc<Daemon>, stream: UnixStream) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let cmd = line.trim();
    log::debug!("cmd: {cmd}");

    match cmd {
        "start" => match start_recording(&daemon).await {
            Ok(()) => reply(&mut write, "recording\n").await,
            Err(e) => reply(&mut write, &format!("error: {e:#}\n")).await,
        },
        "stop" => match stop_and_transcribe(&daemon).await {
            Ok(text) => reply(&mut write, &format!("{text}\n")).await,
            Err(e) => reply(&mut write, &format!("error: {e:#}\n")).await,
        },
        "toggle" => {
            let is_recording = matches!(*daemon.state.lock().await, State::Recording { .. });
            if is_recording {
                match stop_and_transcribe(&daemon).await {
                    Ok(text) => reply(&mut write, &format!("{text}\n")).await,
                    Err(e) => reply(&mut write, &format!("error: {e:#}\n")).await,
                }
            } else {
                match start_recording(&daemon).await {
                    Ok(()) => reply(&mut write, "recording\n").await,
                    Err(e) => reply(&mut write, &format!("error: {e:#}\n")).await,
                }
            }
        }
        "quit" => {
            log::info!("shutting down");
            reply(&mut write, "bye\n").await;
            let _ = std::fs::remove_file(socket_path()?);
            std::process::exit(0);
        }
        other => {
            reply(&mut write, &format!("unknown command: {other}\n")).await;
        }
    }
    Ok(())
}

/// Send a reply to a client that may have already disconnected. The watcher
/// uses `send_command_quiet` which closes the socket immediately after
/// writing the command, so every press/release produces an EPIPE here.
/// Swallow write errors at debug level rather than logging a warn.
async fn reply<W: tokio::io::AsyncWriteExt + Unpin>(write: &mut W, msg: &str) {
    if let Err(e) = write.write_all(msg.as_bytes()).await {
        log::debug!("client disconnected before reply: {e}");
    }
}

async fn start_recording(daemon: &Daemon) -> Result<()> {
    let mut state = daemon.state.lock().await;
    if matches!(*state, State::Recording { .. }) {
        return Err(anyhow!("already recording"));
    }
    let wav_path = fresh_wav_path();
    log::info!("recording to {}", wav_path.display());
    notify("\u{1f3a4} Recording…", "", 1500).await;
    let child = Command::new("arecord")
        .args(["-q", "-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "wav"])
        .arg(&wav_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn arecord (is alsa-utils installed?)")?;
    *state = State::Recording { child, wav_path };
    Ok(())
}

async fn stop_and_transcribe(daemon: &Daemon) -> Result<String> {
    let (mut child, wav_path) = {
        let mut state = daemon.state.lock().await;
        match std::mem::replace(&mut *state, State::Idle) {
            State::Recording { child, wav_path } => (child, wav_path),
            State::Idle => return Err(anyhow!("not recording")),
        }
    };

    if let Some(pid) = child.id() {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        let _ = kill(Pid::from_raw(pid as i32), Signal::SIGINT);
    }
    let _ = child.wait().await;

    // Empty WAVs mean arecord opened the device but never got audio frames —
    // usually a PipeWire/ALSA "Cannot allocate memory" error on the source.
    // Check size before handing to hound so we give a useful error instead
    // of "Failed to read enough bytes".
    let wav_meta = tokio::fs::metadata(&wav_path).await.ok();
    let wav_size = wav_meta.map(|m| m.len()).unwrap_or(0);
    if wav_size <= 128 {
        let _ = tokio::fs::remove_file(&wav_path).await;
        notify(
            "Utter: no audio",
            "Hold the key longer, or check your mic",
            3000,
        )
        .await;
        return Err(anyhow!(
            "no audio captured ({wav_size}B WAV). Check your mic with \
             `wpctl status` and `journalctl --user -n 30 | grep spa.alsa`. \
             On Asahi, the built-in mic may fail with `set_hw_params: \
             Cannot allocate memory` — plug in a USB or 3.5mm headset mic."
        ));
    }

    let samples = tokio::task::spawn_blocking({
        let p = wav_path.clone();
        move || transcribe_rs::audio::read_wav_samples(&p)
    })
    .await??;

    if samples.is_empty() {
        let _ = tokio::fs::remove_file(&wav_path).await;
        return Err(anyhow!("no audio captured"));
    }

    let started = Instant::now();
    let model = daemon.model.clone();
    let text = tokio::task::spawn_blocking(move || -> Result<String> {
        let mut m = model.blocking_lock();
        let result = m
            .transcribe_with(&samples, &ParakeetParams::default())
            .map_err(|e| anyhow!("transcribe: {e}"))?;
        Ok(result.text)
    })
    .await??;
    log::info!("transcribed in {:?}: {:?}", started.elapsed(), text);

    let _ = tokio::fs::remove_file(&wav_path).await;

    // Post-process: drop filler words, collapse stuttered repetitions.
    // Default on; disable with UTTER_CLEANUP=0.
    let cleaned = if std::env::var("UTTER_CLEANUP").ok().as_deref() == Some("0") {
        text.trim().to_string()
    } else {
        cleanup_transcription(text.trim())
    };

    // Append a trailing space so consecutive dictations don't smash together
    // (Parakeet ends sentences with "." but no whitespace).
    let out = if cleaned.is_empty() {
        String::new()
    } else {
        format!("{cleaned} ")
    };
    if !out.is_empty() {
        emit_text(&out).await;
    }
    Ok(out)
}

/// Lightweight post-processing of Parakeet output.
///
/// Parakeet is trained on professionally-transcribed data and emits a very
/// verbatim transcript — every "uh", restarted word, and repeated token.
/// OpenWhispr pipes its Parakeet output through a local LLM (Qwen 2.5 7B)
/// to clean this up. We do a cheaper offline pass: drop obvious fillers,
/// collapse repeated tokens, and fold short-token stutters into the full
/// word that follows them.
///
/// Deliberately conservative — if the text doesn't clearly match a stutter
/// pattern, leave it alone. An LLM-based cleanup tier is on the roadmap for
/// users who want the OpenWhispr experience exactly.
fn cleanup_transcription(text: &str) -> String {
    const FILLERS: &[&str] = &["uh", "um", "er", "ah", "erm", "hmm", "mm", "uhh", "umm"];

    fn clean_word(s: &str) -> &str {
        s.trim_end_matches(|c: char| c.is_ascii_punctuation() && c != '\'')
    }

    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut i = 0;

    while i < tokens.len() {
        let tok = tokens[i];
        let clean = clean_word(tok);
        let clean_lower = clean.to_ascii_lowercase();

        // 1. Drop filler words (treat "uh," the same as "uh").
        if FILLERS.contains(&clean_lower.as_str()) {
            i += 1;
            continue;
        }

        // 2. Count consecutive tokens that equal this one (case-insensitive).
        let mut j = i + 1;
        while j < tokens.len()
            && clean_word(tokens[j]).to_ascii_lowercase() == clean_lower
        {
            j += 1;
        }
        let rep_count = j - i;

        // 3. Partial-word stutter: a short token (≤4 chars) that repeats 2+
        //    times, followed by a longer word beginning with those same
        //    letters. "wh wh wh what" → "what".
        if clean.len() <= 4 && !clean.is_empty() && rep_count >= 2 && j < tokens.len() {
            let next = tokens[j];
            let next_lower = clean_word(next).to_ascii_lowercase();
            if next_lower.starts_with(&clean_lower) && next_lower.len() > clean_lower.len() {
                out.push(next.to_string());
                i = j + 1;
                continue;
            }
        }

        // 4. Full-word repetition of 3+ tokens: "I I I think" → "I think".
        //    Keep the last occurrence so any trailing punctuation survives.
        if rep_count >= 3 {
            out.push(tokens[j - 1].to_string());
            i = j;
            continue;
        }

        out.push(tok.to_string());
        i += 1;
    }

    // Normalize punctuation spacing that may have opened up when we dropped
    // fillers or collapsed repeats.
    let mut joined = out.join(" ");
    for pair in [" ,", " .", " ?", " !", " ;", " :"] {
        joined = joined.replace(pair, &pair[1..]);
    }
    while joined.contains(",,") {
        joined = joined.replace(",,", ",");
    }
    while joined.contains("  ") {
        joined = joined.replace("  ", " ");
    }
    joined.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::cleanup_transcription;

    #[test]
    fn drops_fillers() {
        assert_eq!(cleanup_transcription("I uh went to the store"), "I went to the store");
        assert_eq!(cleanup_transcription("uh um er ah"), "");
        assert_eq!(cleanup_transcription("Hello, um, world"), "Hello, world");
    }

    #[test]
    fn collapses_partial_stutter_before_full_word() {
        assert_eq!(cleanup_transcription("wh wh wh what"), "what");
        assert_eq!(
            cleanup_transcription("I want to go to the wh wh wh whatever store"),
            "I want to go to the whatever store"
        );
        assert_eq!(cleanup_transcription("fri fri fri fri frictionless"), "frictionless");
    }

    #[test]
    fn collapses_triple_plus_word_repetition() {
        assert_eq!(cleanup_transcription("I I I think so"), "I think so");
        assert_eq!(cleanup_transcription("no no no no"), "no");
    }

    #[test]
    fn leaves_light_repetition_alone() {
        assert_eq!(cleanup_transcription("very very good"), "very very good");
        assert_eq!(cleanup_transcription("the the cat"), "the the cat");
    }

    #[test]
    fn preserves_contractions_and_case() {
        assert_eq!(cleanup_transcription("I don't know"), "I don't know");
        assert_eq!(cleanup_transcription("Hello, World."), "Hello, World.");
    }

    #[test]
    fn fixes_punctuation_spacing_around_dropped_fillers() {
        assert_eq!(cleanup_transcription("okay , uh , so"), "okay, so");
    }
}

async fn emit_text(text: &str) {
    // Always put the text on the clipboard — a safety net (user can paste
    // manually if auto-paste fails) and the source we read for the paste
    // keystroke.
    if let Err(e) = wl_copy(text).await {
        log::warn!("wl-copy failed: {e:#}");
    }
    if std::env::var("UTTER_AUTOTYPE").ok().as_deref() != Some("1") {
        return;
    }
    // UTTER_PASTE_METHOD picks the keystroke that triggers paste in the
    // focused app. Default is shift-insert because it's the most universal
    // Linux paste binding (terminals, Qt/GTK apps all accept it) and it
    // avoids Ctrl+V clashes (e.g. Claude Code binds Ctrl+V to image paste).
    let method = std::env::var("UTTER_PASTE_METHOD")
        .unwrap_or_else(|_| "shift-insert".to_string());
    let paste_result = match method.as_str() {
        // Kernel keycodes from /usr/include/linux/input-event-codes.h.
        // Sequence is press-press-release-release.
        // LEFTSHIFT=42, INSERT=110
        "shift-insert" | "shift+insert" => {
            ydotool_keys(&["42:1", "110:1", "110:0", "42:0"]).await
        }
        // LEFTCTRL=29, V=47
        "ctrl-v" | "ctrl+v" => ydotool_keys(&["29:1", "47:1", "47:0", "29:0"]).await,
        // LEFTCTRL=29, LEFTSHIFT=42, V=47 — Konsole's default paste combo.
        "ctrl-shift-v" | "ctrl+shift+v" => {
            ydotool_keys(&["29:1", "42:1", "47:1", "47:0", "42:0", "29:0"]).await
        }
        // Skip paste entirely, type character-by-character. Slow for long
        // text but works in apps that don't support clipboard paste.
        "type" => ydotool_type(text).await,
        other => Err(anyhow!(
            "unknown UTTER_PASTE_METHOD '{other}' (expected: shift-insert, ctrl-v, ctrl-shift-v, type)"
        )),
    };
    if let Err(paste_err) = paste_result {
        log::warn!("paste failed ({method}): {paste_err:#}; falling back to typing");
        if let Err(type_err) = ydotool_type(text).await {
            log::warn!("type fallback also failed: {type_err:#}");
        }
    }
}

async fn wl_copy(text: &str) -> Result<()> {
    // Write to BOTH the regular clipboard and the primary selection. Wayland
    // (inherited from X11) has two independent paste buffers:
    //   - clipboard (Ctrl+C / Ctrl+V, Ctrl+Shift+V in terminals)
    //   - primary selection (Shift+Insert, middle-click) — set implicitly
    //     by mouse text selection
    // If we only populate the clipboard, Shift+Insert keeps pasting whatever
    // the user last mouse-selected (often stale). Writing both keeps every
    // common paste shortcut in sync with the dictation output.
    for primary in [false, true] {
        let mut cmd = Command::new("wl-copy");
        if primary {
            cmd.arg("--primary");
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn wl-copy")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes()).await?;
        }
        child.wait().await?;
    }
    Ok(())
}

async fn ydotool_type(text: &str) -> Result<()> {
    // --key-delay 0 --key-hold 1 minimizes the inter-key delay; default
    // is 20ms/20ms = ~40ms per character, which adds up to multi-second
    // delays for normal-length transcriptions.
    let output = Command::new("ydotool")
        .args(["type", "--key-delay", "0", "--key-hold", "1", "--"])
        .arg(text)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ydotool type: {}", stderr.trim()));
    }
    Ok(())
}

async fn ydotool_keys(codes: &[&str]) -> Result<()> {
    let output = Command::new("ydotool")
        .args(["key", "--key-delay", "0"])
        .args(codes)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ydotool key: {}", stderr.trim()));
    }
    Ok(())
}

async fn notify(summary: &str, body: &str, expire_ms: u64) {
    // Opt-in. Most users are fine with just the system mic icon Plasma
    // puts in the tray while audio is captured, and don't want an extra
    // toast on every hotkey press.
    if std::env::var("UTTER_NOTIFY").ok().as_deref() != Some("1") {
        return;
    }
    let _ = Command::new("notify-send")
        .args([
            "--app-name=Utter",
            "--icon=audio-input-microphone",
            "--expire-time",
            &expire_ms.to_string(),
            summary,
            body,
        ])
        .status()
        .await;
}

/// Canonical short name for an evdev keycode. Inverse of `parse_key_name`
/// for the subset of keys we can reliably use for push-to-talk (modifiers,
/// capslock, F1–F20). Returns None for letter keys, number keys, etc. —
/// those work technically but are terrible PTT choices because every
/// keypress during normal typing would start/stop a recording.
fn canonical_name_for(code: evdev::KeyCode) -> Option<&'static str> {
    Some(match code {
        evdev::KeyCode::KEY_RIGHTMETA => "rightmeta",
        evdev::KeyCode::KEY_LEFTMETA => "leftmeta",
        evdev::KeyCode::KEY_RIGHTCTRL => "rightctrl",
        evdev::KeyCode::KEY_LEFTCTRL => "leftctrl",
        evdev::KeyCode::KEY_RIGHTALT => "rightalt",
        evdev::KeyCode::KEY_LEFTALT => "leftalt",
        evdev::KeyCode::KEY_RIGHTSHIFT => "rightshift",
        evdev::KeyCode::KEY_LEFTSHIFT => "leftshift",
        evdev::KeyCode::KEY_CAPSLOCK => "capslock",
        evdev::KeyCode::KEY_F1 => "f1",
        evdev::KeyCode::KEY_F2 => "f2",
        evdev::KeyCode::KEY_F3 => "f3",
        evdev::KeyCode::KEY_F4 => "f4",
        evdev::KeyCode::KEY_F5 => "f5",
        evdev::KeyCode::KEY_F6 => "f6",
        evdev::KeyCode::KEY_F7 => "f7",
        evdev::KeyCode::KEY_F8 => "f8",
        evdev::KeyCode::KEY_F9 => "f9",
        evdev::KeyCode::KEY_F10 => "f10",
        evdev::KeyCode::KEY_F11 => "f11",
        evdev::KeyCode::KEY_F12 => "f12",
        evdev::KeyCode::KEY_F13 => "f13",
        evdev::KeyCode::KEY_F14 => "f14",
        evdev::KeyCode::KEY_F15 => "f15",
        evdev::KeyCode::KEY_F16 => "f16",
        evdev::KeyCode::KEY_F17 => "f17",
        evdev::KeyCode::KEY_F18 => "f18",
        evdev::KeyCode::KEY_F19 => "f19",
        evdev::KeyCode::KEY_F20 => "f20",
        _ => return None,
    })
}

fn parse_key_name(name: &str) -> Result<evdev::KeyCode> {
    let n = name.to_ascii_lowercase();
    let n = n.strip_prefix("key_").unwrap_or(&n);
    let code = match n {
        "rightmeta" | "rightsuper" | "rightcmd" | "rightcommand" => evdev::KeyCode::KEY_RIGHTMETA,
        "leftmeta" | "leftsuper" | "leftcmd" | "leftcommand" => evdev::KeyCode::KEY_LEFTMETA,
        "rightctrl" | "rightcontrol" => evdev::KeyCode::KEY_RIGHTCTRL,
        "leftctrl" | "leftcontrol" => evdev::KeyCode::KEY_LEFTCTRL,
        "rightalt" | "rightoption" => evdev::KeyCode::KEY_RIGHTALT,
        "leftalt" | "leftoption" => evdev::KeyCode::KEY_LEFTALT,
        "rightshift" => evdev::KeyCode::KEY_RIGHTSHIFT,
        "leftshift" => evdev::KeyCode::KEY_LEFTSHIFT,
        "capslock" => evdev::KeyCode::KEY_CAPSLOCK,
        "f1" => evdev::KeyCode::KEY_F1,
        "f2" => evdev::KeyCode::KEY_F2,
        "f3" => evdev::KeyCode::KEY_F3,
        "f4" => evdev::KeyCode::KEY_F4,
        "f5" => evdev::KeyCode::KEY_F5,
        "f6" => evdev::KeyCode::KEY_F6,
        "f7" => evdev::KeyCode::KEY_F7,
        "f8" => evdev::KeyCode::KEY_F8,
        "f9" => evdev::KeyCode::KEY_F9,
        "f10" => evdev::KeyCode::KEY_F10,
        "f11" => evdev::KeyCode::KEY_F11,
        "f12" => evdev::KeyCode::KEY_F12,
        "f13" => evdev::KeyCode::KEY_F13,
        "f14" => evdev::KeyCode::KEY_F14,
        "f15" => evdev::KeyCode::KEY_F15,
        "f16" => evdev::KeyCode::KEY_F16,
        "f17" => evdev::KeyCode::KEY_F17,
        "f18" => evdev::KeyCode::KEY_F18,
        "f19" => evdev::KeyCode::KEY_F19,
        "f20" => evdev::KeyCode::KEY_F20,
        other => return Err(anyhow!("unknown key name: {other}")),
    };
    Ok(code)
}

async fn run_watcher(key_name: &str) -> Result<()> {
    let target = parse_key_name(key_name)?;
    log::info!("watching for key {key_name} (code {})", target.code());

    let matching: Vec<(std::path::PathBuf, evdev::Device)> = evdev::enumerate()
        .filter(|(_, d)| {
            d.supported_keys()
                .map_or(false, |k| k.contains(target))
        })
        .collect();

    if matching.is_empty() {
        return Err(anyhow!(
            "no input device advertises key {key_name}. Is the user in the `input` group?"
        ));
    }
    for (path, device) in &matching {
        log::info!(
            "watching {} ({})",
            path.display(),
            device.name().unwrap_or("?")
        );
    }

    let mut handles = Vec::new();
    for (path, device) in matching {
        let target = target;
        handles.push(tokio::spawn(async move {
            if let Err(e) = watch_device(device, target).await {
                log::warn!("watch {}: {e:#}", path.display());
            }
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    Ok(())
}

async fn watch_device(device: evdev::Device, target: evdev::KeyCode) -> Result<()> {
    let mut stream = device.into_event_stream()?;
    loop {
        let ev = stream.next_event().await?;
        if ev.event_type() != evdev::EventType::KEY {
            continue;
        }
        if ev.code() != target.code() {
            continue;
        }
        match ev.value() {
            1 => {
                log::debug!("key down -> start");
                if let Err(e) = send_command_quiet("start").await {
                    log::warn!("start: {e:#}");
                }
            }
            0 => {
                log::debug!("key up -> stop");
                if let Err(e) = send_command_quiet("stop").await {
                    log::warn!("stop: {e:#}");
                }
            }
            _ => {} // value 2 = autorepeat, ignore
        }
    }
}

// send_command variant that doesn't print the server reply — the watcher fires
// many times per second and we don't want stdout spam.
async fn send_command_quiet(cmd: &str) -> Result<()> {
    let path = socket_path()?;
    let mut stream = UnixStream::connect(&path)
        .await
        .with_context(|| format!("daemon not running at {}", path.display()))?;
    stream.write_all(format!("{cmd}\n").as_bytes()).await?;
    stream.shutdown().await.ok();
    Ok(())
}

// ---------------------------------------------------------------------------
// set-key: interactively pick the PTT key and persist it as a systemd
// override. Handles two common friction points:
//   1. "what IS my right Cmd called to evdev?" — we report the canonical
//      name and the numeric code.
//   2. "does this key actually work end-to-end?" — we confirm both press
//      AND release events landed, so the user knows hold-to-talk will work.
// ---------------------------------------------------------------------------

async fn run_set_key(dry_run: bool, timeout_secs: u64) -> Result<()> {
    // Stop the running watcher so it doesn't intercept the test key-press.
    // Remember if it was running so we can restore state on error / dry-run.
    let watcher_was_active = watcher_is_active();
    if watcher_was_active {
        let _ = run_systemctl_user(&["stop", "utter-watcher.service"]);
    }

    let result = pick_key_and_maybe_save(dry_run, timeout_secs).await;

    // Reload unit files so any override.conf we just wrote is picked up.
    let _ = run_systemctl_user(&["daemon-reload"]);

    // Decide the watcher's final state:
    //   - dry-run: restore whatever we found it in.
    //   - real run + save succeeded: always activate. The user's explicit
    //     intent with `utter set-key` is "use this key from now on," so
    //     leaving the watcher stopped would be surprising — even if the
    //     user had stopped it manually before running set-key.
    //   - real run + detection failed: restore previous state.
    let should_activate = match (dry_run, &result) {
        (true, _) => watcher_was_active,
        (false, Ok(_)) => true,
        (false, Err(_)) => watcher_was_active,
    };
    let restarted = if should_activate {
        // restart (= stop-if-running then start) covers both "watcher is
        // stopped" and "already running under the old config" in one call.
        run_systemctl_user(&["restart", "utter-watcher.service"])
            .map(|s| s.success())
            .unwrap_or(false)
    } else {
        false
    };

    // Only report on non-dry-run saves — dry-run prints its own message
    // inside pick_key_and_maybe_save, and detection errors propagate via
    // `result` to the caller.
    if !dry_run {
        if let Ok(()) = &result {
            if restarted {
                println!("Watcher restarted with the new key — hold it to dictate.");
            } else if should_activate {
                println!(
                    "Saved, but `systemctl --user restart utter-watcher` failed — \
                     start it manually once you've sorted out whatever's wrong."
                );
            } else {
                // Can happen on dry-run + was-active=false, but dry-run is handled above.
                // This branch is only reached if we chose not to activate for some other reason.
                println!("Saved. Start the watcher with `systemctl --user start utter-watcher` to use the new key.");
            }
        }
    }

    result
}

async fn pick_key_and_maybe_save(dry_run: bool, timeout_secs: u64) -> Result<()> {
    // Enumerate anything that looks like a keyboard. KEY_A is present on
    // every real keyboard and absent on mice/touchpads/joysticks, so it's
    // a good discriminator that doesn't lock us to one modifier class.
    let devices: Vec<(std::path::PathBuf, evdev::Device)> = evdev::enumerate()
        .filter(|(_, d)| {
            d.supported_keys()
                .map_or(false, |k| k.contains(evdev::KeyCode::KEY_A))
        })
        .collect();

    if devices.is_empty() {
        return Err(anyhow!(
            "no readable keyboard devices found.\n\
             \n\
             Package install: the udev uaccess rule grants read access on \
             login — if you just installed, log out + back in, or run:\n\
               sudo udevadm control --reload-rules && sudo udevadm trigger --subsystem-match=input\n\
             \n\
             From-source install: check you're in the `input` group:\n\
               id | grep input\n\
             If not: sudo usermod -aG input \"$USER\" and log out + back in."
        ));
    }

    eprintln!(
        "Listening on {} keyboard device(s).\n\
         Press and hold the key you want to use for push-to-talk, then release it.\n\
         (Ctrl+C to cancel; timeout in {}s.)",
        devices.len(),
        timeout_secs
    );

    // One task per device; first press+release pair wins.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<evdev::KeyCode>(1);
    let mut handles = Vec::new();
    for (_path, device) in devices {
        let tx = tx.clone();
        handles.push(tokio::spawn(async move {
            let _ = watch_first_press_release(device, tx).await;
        }));
    }
    drop(tx);

    let recv_result =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), rx.recv()).await;
    for h in handles {
        h.abort();
    }

    let code = recv_result
        .map_err(|_| anyhow!("timed out — no key press + release captured"))?
        .ok_or_else(|| anyhow!("no key detected"))?;

    let name = canonical_name_for(code).ok_or_else(|| {
        anyhow!(
            "detected key with code {} but utter doesn't have a short name for it. \
             Modifier keys (ctrl/alt/meta/shift), capslock, and F1-F20 are the \
             supported set — pick one of those for push-to-talk.",
            code.code()
        )
    })?;

    println!("Detected: {name} (code {}). Press + release both captured — hold-to-talk will work.", code.code());

    if dry_run {
        eprintln!("(--dry-run: not saving.)");
        return Ok(());
    }

    write_watcher_override(name)?;
    println!("Saved override for key `{name}`.");
    Ok(())
}

async fn watch_first_press_release(
    device: evdev::Device,
    tx: tokio::sync::mpsc::Sender<evdev::KeyCode>,
) -> Result<()> {
    let mut stream = device.into_event_stream()?;
    let mut pressed: Option<evdev::KeyCode> = None;
    loop {
        let ev = stream.next_event().await?;
        if ev.event_type() != evdev::EventType::KEY {
            continue;
        }
        let code = evdev::KeyCode::new(ev.code());
        match ev.value() {
            1 => {
                // First key-down wins; ignore other keys pressed during the hold.
                if pressed.is_none() {
                    pressed = Some(code);
                }
            }
            0 => {
                // Release of the held key completes the test.
                if pressed == Some(code) {
                    let _ = tx.send(code).await;
                    return Ok(());
                }
            }
            _ => {} // 2 = autorepeat, ignore
        }
    }
}

fn write_watcher_override(key_name: &str) -> Result<()> {
    let override_dir = dirs::config_dir()
        .ok_or_else(|| anyhow!("no XDG config dir"))?
        .join("systemd/user/utter-watcher.service.d");
    std::fs::create_dir_all(&override_dir)?;
    let override_path = override_dir.join("override.conf");

    // Resolve the binary path via /proc/self/exe — works whether utter is
    // installed at /usr/bin/utter (package) or ~/.cargo/bin/utter (source).
    let exe = std::env::current_exe()
        .context("reading /proc/self/exe to resolve utter's binary path")?;

    let content = format!(
        "# Written by `utter set-key` — edit at your own risk, or re-run the command.\n\
         [Service]\n\
         ExecStart=\n\
         ExecStart={} watch --key {}\n",
        exe.display(),
        key_name,
    );
    std::fs::write(&override_path, content)
        .with_context(|| format!("writing {}", override_path.display()))?;
    Ok(())
}

fn watcher_is_active() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "utter-watcher.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_systemctl_user(args: &[&str]) -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
}
