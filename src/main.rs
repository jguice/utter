use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::io::Read;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams};
use transcribe_rs::onnx::Quantization;

mod config;
use config::Config;

mod audio;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
mod macos_ui;

#[cfg(target_os = "macos")]
mod macos_onboarding;

#[derive(Parser)]
#[command(
    name = "utter",
    version = concat!(env!("CARGO_PKG_VERSION"), "+", env!("UTTER_GIT_SHA")),
    about = "Local push-to-talk voice dictation"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
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
    /// Stop recording, transcribe, write to the primary selection, and
    /// auto-paste via Shift+Insert (unless `auto_paste = false` in config).
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
        /// Key to watch. Either a named alias (rightmeta, leftmeta, rightctrl,
        /// capslock, f1..f20, Apple aliases rightcmd/leftcmd/rightoption/etc.)
        /// or a raw evdev keycode as digits (e.g. `--key 70` for scroll lock,
        /// `--key 194` for f24). If omitted, the watcher reads `key` from
        /// `~/.config/utter/config.toml` (default `rightmeta`). `utter
        /// set-key` writes the config for you.
        #[arg(long)]
        key: Option<String>,
    },
    /// Interactively pick the push-to-talk key. Stops the watcher, listens
    /// for a press + release on any keyboard, then (unless --dry-run)
    /// writes the chosen key to ~/.config/utter/config.toml and
    /// restarts the watcher.
    SetKey {
        /// Just report the detected key — don't save or restart the watcher.
        #[arg(long)]
        dry_run: bool,
        /// Seconds to wait for a key press before giving up.
        #[arg(long, default_value = "20")]
        timeout: u64,
    },
}

#[cfg(target_os = "linux")]
fn socket_path() -> Result<PathBuf> {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR not set (need an active systemd user session)")?;
    Ok(PathBuf::from(runtime).join("utter.sock"))
}

#[cfg(target_os = "macos")]
fn socket_path() -> Result<PathBuf> {
    macos::socket_path()
}

fn default_model_dir() -> Result<PathBuf> {
    let data = dirs::data_dir().ok_or_else(|| anyhow!("no data dir"))?;
    Ok(data.join("utter/models/parakeet-tdt-0.6b-v3-int8"))
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();

    // On macOS, combined mode (no subcommand, used by `open utter.app`)
    // runs the tokio runtime on a worker thread so NSApplication can own
    // the main thread for the menu bar item. All other paths use a
    // standard tokio runtime on the calling thread.
    #[cfg(target_os = "macos")]
    if cli.cmd.is_none() {
        return run_combined_with_menu_bar();
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(dispatch(cli))
}

async fn dispatch(cli: Cli) -> Result<()> {
    match cli.cmd {
        None => run_combined().await,
        Some(Cmd::Daemon { model }) => run_daemon(model).await,
        Some(Cmd::Start) => send_command("start").await,
        Some(Cmd::Stop) => send_command("stop").await,
        Some(Cmd::Toggle) => send_command("toggle").await,
        Some(Cmd::Quit) => send_command("quit").await,
        Some(Cmd::SocketPath) => {
            println!("{}", socket_path()?.display());
            Ok(())
        }
        Some(Cmd::Watch { key }) => run_watcher(key.as_deref()).await,
        Some(Cmd::SetKey { dry_run, timeout }) => run_set_key(dry_run, timeout).await,
    }
}

#[cfg(target_os = "macos")]
fn run_combined_with_menu_bar() -> Result<()> {
    // Read the current PTT key + config path on the main thread before
    // handing off to the runtime — the menu bar item is built once and
    // the initial labels need to be populated before NSApplication::run
    // takes over.
    let config_path = Config::default_path()?;
    let cfg = Config::load_or_migrate(&config_path, &config::utter_env_snapshot())?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // The runtime thread waits on this channel before spawning the daemon.
    // The main thread sends on it either immediately (all perms already
    // granted) or after the user finishes onboarding.
    let (start_tx, start_rx) = std::sync::mpsc::channel::<()>();

    // Park the runtime on its own thread so it stays alive for the
    // duration of NSApplication::run(). Dropping the runtime would stop
    // the daemon mid-dictation.
    std::thread::Builder::new()
        .name("utter-runtime".into())
        .spawn(move || {
            if start_rx.recv().is_err() {
                log::warn!("runtime thread: start signal dropped before send");
                return;
            }
            rt.block_on(async {
                if let Err(e) = run_combined().await {
                    log::error!("combined: {e:#}");
                }
            });
        })?;

    // Owns the main thread forever. `Quit utter` from the menu sends
    // `terminate:` to NSApp, which exits the process; the runtime thread
    // is torn down by the OS, and the daemon socket gets cleaned up by
    // the next process start (daemon unlinks stale sockets on bind).
    macos_ui::run_status_bar_app(&cfg, &config_path, start_tx)
}

/// Default no-subcommand entry point: spin up the daemon and the watcher
/// as two tokio tasks in a single process. Used by `utter.app` so the
/// user double-clicks once and dictation is live. The daemon's Unix-socket
/// contract is preserved — the watcher still talks to it via
/// `send_command_quiet` — so nothing about the cross-process IPC layer
/// changes.
async fn run_combined() -> Result<()> {
    let daemon = tokio::spawn(async {
        if let Err(e) = run_daemon(None).await {
            log::error!("daemon: {e:#}");
        }
    });

    // Give the daemon a head-start so the watcher's first keypress (if it
    // comes early) finds a bound socket. Model load on first run is 5-30s;
    // subsequent runs are sub-second. The watcher survives a transient
    // "daemon not running" either way — send_command_quiet just logs it —
    // so this is belt-and-suspenders, not correctness.
    wait_for_socket(std::time::Duration::from_secs(60)).await;

    let watcher = tokio::spawn(async {
        if let Err(e) = run_watcher(None).await {
            log::error!("watcher: {e:#}");
        }
    });

    let _ = tokio::join!(daemon, watcher);
    Ok(())
}

/// Poll until the daemon's socket exists or the deadline passes. Cheap
/// stat per iteration; the daemon creates the socket synchronously during
/// startup, so as soon as it's bound we stop waiting.
async fn wait_for_socket(timeout: std::time::Duration) {
    let start = std::time::Instant::now();
    let poll = std::time::Duration::from_millis(100);
    loop {
        if let Ok(path) = socket_path() {
            if path.exists() {
                return;
            }
        }
        if start.elapsed() >= timeout {
            log::warn!("daemon socket didn't appear within {timeout:?}; starting watcher anyway");
            return;
        }
        tokio::time::sleep(poll).await;
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
    Recording { capture: audio::AudioCapture },
}

/// Cached EI (Emulated Input) state reused across dictations to avoid
/// creating a new XDG RemoteDesktop portal session per dictation.
#[cfg(target_os = "linux")]
struct EiState {
    context: reis::ei::Context,
    device: reis::ei::Device,
    keyboard: reis::ei::Keyboard,
    keymap: *mut xkbcommon_dl::xkb_keymap,
    xkb_state: *mut xkbcommon_dl::xkb_state,
}

// Safety: The raw XKB pointers are only accessed from the async runtime
// (single-threaded logical access via Mutex).
#[cfg(target_os = "linux")]
unsafe impl Send for EiState {}

/// Wrapper to make raw XKB pointers Send-safe for use across `.await` points.
/// The pointers are only dereferenced in unsafe blocks within the async runtime.
#[cfg(target_os = "linux")]
struct SendPtr<T>(*mut T);
#[cfg(target_os = "linux")]
unsafe impl<T> Send for SendPtr<T> {}

#[cfg(target_os = "linux")]
impl Drop for EiState {
    fn drop(&mut self) {
        let xkb = xkbcommon_dl::xkbcommon_handle();
        unsafe {
            if !self.xkb_state.is_null() {
                (xkb.xkb_state_unref)(self.xkb_state);
            }
            if !self.keymap.is_null() {
                (xkb.xkb_keymap_unref)(self.keymap);
            }
        }
    }
}

/// Parse an XKB keymap from a file descriptor received from the compositor
/// and create an xkb_state from it.
#[cfg(target_os = "linux")]
unsafe fn parse_keymap_from_fd(
    fd: std::os::unix::io::OwnedFd,
) -> Result<(*mut xkbcommon_dl::xkb_keymap, *mut xkbcommon_dl::xkb_state)> {
    let xkb = xkbcommon_dl::xkbcommon_handle();

    let mut file = std::fs::File::from(fd);
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .context("read keymap fd")?;

    let ctx = (xkb.xkb_context_new)(xkbcommon_dl::xkb_context_flags::XKB_CONTEXT_NO_FLAGS);
    if ctx.is_null() {
        return Err(anyhow!("xkb_context_new failed"));
    }

    let keymap = (xkb.xkb_keymap_new_from_buffer)(
        ctx,
        buf.as_ptr() as *const _,
        buf.len(),
        xkbcommon_dl::xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1,
        xkbcommon_dl::xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
    );
    (xkb.xkb_context_unref)(ctx);

    if keymap.is_null() {
        return Err(anyhow!("xkb_keymap_new_from_buffer failed"));
    }

    let state = (xkb.xkb_state_new)(keymap);
    if state.is_null() {
        (xkb.xkb_keymap_unref)(keymap);
        return Err(anyhow!("xkb_state_new failed"));
    }

    log::info!("ei: parsed XKB keymap from compositor");
    Ok((keymap, state))
}

/// Map an ASCII character to an evdev keycode and whether Shift is needed.
/// Uses US QWERTY layout. For non-ASCII chars, falls back to the XKB keymap
/// using `find_key_for_char_xkb`.
#[cfg(target_os = "linux")]
fn char_to_evdev(ch: char) -> Option<(u32, bool)> {
    // evdev keycodes for a-z from linux/input-event-codes.h (non-sequential).
    const EVDEV_A_TO_Z: [u32; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, // a-m
        49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44, // n-z
    ];
    Some(match ch {
        'a'..='z' => (EVDEV_A_TO_Z[(ch as u8 - b'a') as usize], false),
        'A'..='Z' => (EVDEV_A_TO_Z[(ch as u8 - b'A') as usize], true),
        '0' => (11, false), '1' => (2, false), '2' => (3, false),
        '3' => (4, false), '4' => (5, false), '5' => (6, false),
        '6' => (7, false), '7' => (8, false), '8' => (9, false), '9' => (10, false),
        ' ' => (57, false),
        '-' => (12, false), '=' => (13, false),
        '[' => (26, false), ']' => (27, false),
        ';' => (39, false), '\'' => (40, false), '`' => (41, false),
        '\\' => (43, false), ',' => (51, false), '.' => (52, false), '/' => (53, false),
        '_' => (12, true), '+' => (13, true),
        '{' => (26, true), '}' => (27, true),
        ':' => (39, true), '"' => (40, true), '~' => (41, true),
        '|' => (43, true), '<' => (51, true), '>' => (52, true), '?' => (53, true),
        '!' => (2, true), '@' => (3, true), '#' => (4, true), '$' => (5, true),
        '%' => (6, true), '^' => (7, true), '&' => (8, true), '*' => (9, true),
        '(' => (10, true), ')' => (11, true),
        '\n' => (28, false), '\t' => (15, false),
        _ => return None,
    })
}

/// Fallback: find the XKB keycode for a non-ASCII character using the XKB
/// keymap. Returns `(evdev_keycode, shift_needed)` by subtracting the
/// standard XKB offset (8) from the XKB keycode.
#[cfg(target_os = "linux")]
unsafe fn find_key_for_char_xkb(
    keymap: *mut xkbcommon_dl::xkb_keymap,
    ch: char,
) -> Option<(u32, bool)> {
    let xkb = xkbcommon_dl::xkbcommon_handle();
    let target_cp = ch as u32;
    let min = (xkb.xkb_keymap_min_keycode)(keymap);
    let max = (xkb.xkb_keymap_max_keycode)(keymap);
    let shift_idx = (xkb.xkb_keymap_mod_get_index)(
        keymap,
        xkbcommon_dl::XKB_MOD_NAME_SHIFT.as_ptr() as *const _,
    );

    for code in min..=max {
        let num_layouts = (xkb.xkb_keymap_num_layouts_for_key)(keymap, code);
        for layout in 0..num_layouts {
            let num_levels = (xkb.xkb_keymap_num_levels_for_key)(keymap, code, layout);
            for level in 0..num_levels {
                let mut syms_ptr: *const xkbcommon_dl::xkb_keysym_t = std::ptr::null();
                let count = (xkb.xkb_keymap_key_get_syms_by_level)(
                    keymap, code, layout, level, &mut syms_ptr,
                );
                if count > 0 {
                    let sym = *syms_ptr;
                    let cp = (xkb.xkb_keysym_to_utf32)(sym);
                    if cp == target_cp {
                        // XKB keycode = evdev keycode + 8 (standard offset)
                        let evdev_code = code.saturating_sub(8).max(1);
                        let shift = level == 1 && shift_idx < 32;
                        return Some((evdev_code, shift));
                    }
                }
            }
        }
    }
    None
}

struct Daemon {
    model: Arc<Mutex<ParakeetModel>>,
    state: Mutex<State>,
    config: Config,
    #[cfg(target_os = "linux")]
    ei_state: Mutex<Option<EiState>>,
}

async fn run_daemon(model_override: Option<PathBuf>) -> Result<()> {
    let config_path = Config::default_path()?;
    let env = config::utter_env_snapshot();
    let cfg = Config::load_or_migrate(&config_path, &env)?;
    log::info!(
        "config loaded from {} (auto_paste={}, write_clipboard={}, filter_filler_words={})",
        config_path.display(),
        cfg.auto_paste,
        cfg.write_clipboard,
        cfg.filter_filler_words,
    );

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
        config: cfg,
        #[cfg(target_os = "linux")]
        ei_state: Mutex::new(None),
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
    let capture = audio::start_audio().await.context("start cpal input stream")?;
    *state = State::Recording { capture };
    Ok(())
}

async fn stop_and_transcribe(daemon: &Daemon) -> Result<String> {
    let prev_state = {
        let mut state = daemon.state.lock().await;
        std::mem::replace(&mut *state, State::Idle)
    };

    let samples = match obtain_samples(prev_state).await {
        Ok(s) => s,
        Err(e) => return Err(e),
    };

    if samples.is_empty() {
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

    let cleaned = if daemon.config.filter_filler_words {
        cleanup_transcription(text.trim())
    } else {
        text.trim().to_string()
    };

    // Append a trailing space so consecutive dictations don't smash together
    // (Parakeet ends sentences with "." but no whitespace).
    let out = if cleaned.is_empty() {
        String::new()
    } else {
        format!("{cleaned} ")
    };
    if !out.is_empty() {
        emit_text(&out, daemon).await;
    }
    Ok(out)
}

async fn obtain_samples(state: State) -> Result<Vec<f32>> {
    match state {
        State::Recording { capture } => audio::stop_audio(capture).await,
        State::Idle => Err(anyhow!("not recording")),
    }
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

#[cfg(target_os = "linux")]
async fn emit_text(text: &str, daemon: &Daemon) {
    let t0 = Instant::now();
    log::info!(
        "emit: start (len={}, write_clipboard={}, auto_paste={})",
        text.len(),
        daemon.config.write_clipboard,
        daemon.config.auto_paste
    );

    // Best-effort write to Wayland primary selection (and optionally the
    // regular clipboard) for clipboard-manager users. Ignore errors —
    // the real paste path is EI typing below.
    if let Err(e) = wl_copy(text, daemon.config.write_clipboard) {
        log::warn!("wl-copy (best-effort) failed: {e:#}");
    }
    log::info!("emit: wl_copy returned at +{:?}", t0.elapsed());

    if daemon.config.auto_paste {
        if let Err(e) = ei_type_text(text, daemon).await {
            log::warn!("ei type failed: {e:#}");
        }
        log::info!("emit: ei_type returned at +{:?}", t0.elapsed());
    } else {
        log::info!("emit: auto_paste off, not typing");
    }
}

#[cfg(target_os = "macos")]
async fn emit_text(text: &str, cfg: &Config) {
    let t0 = Instant::now();
    log::info!("emit: start (len={}, auto_paste={})", text.len(), cfg.auto_paste);
    if let Err(e) = macos::emit_text(text, cfg).await {
        log::warn!("emit: {e:#}");
    }
    log::info!("emit: returned at +{:?}", t0.elapsed());
}

/// Write text to the Wayland primary selection (and optionally the regular
/// clipboard) via the `wlr-data-control` protocol. Blocking — keeps the
/// Wayland connection open until the receiving app has consumed the data,
/// so the selection stays valid for the subsequent Shift+Insert paste.
#[cfg(target_os = "linux")]
fn wl_copy(text: &str, also_clipboard: bool) -> Result<()> {
    use wl_clipboard_rs::copy::{ClipboardType, MimeType, Options, Source};

    let mut opts = Options::new();
    opts.clipboard(ClipboardType::Primary);
    let bytes = text.as_bytes().to_vec();
    opts.copy(Source::Bytes(bytes.into()), MimeType::Text)
        .context("wl-clipboard-rs: copy to primary selection")?;

    if also_clipboard {
        let mut opts = Options::new();
        opts.clipboard(ClipboardType::Regular);
        let bytes = text.as_bytes().to_vec();
        opts.copy(Source::Bytes(bytes.into()), MimeType::Text)
            .context("wl-clipboard-rs: copy to clipboard")?;
    }
    Ok(())
}

/// Paste via the Wayland EI (Emulated Input) protocol using the `reis` crate.
/// Connects to the compositor's EIS server, creates a virtual keyboard, and
/// sends Shift+Insert. Reuses a cached EI context across dictations to avoid
/// creating a new XDG RemoteDesktop portal session each time.
#[cfg(target_os = "linux")]

/// Type text character-by-character via the EI virtual keyboard, using the
/// compositor's XKB keymap to resolve each character to the correct keycode
/// and modifier state. This avoids the clipboard entirely.
#[cfg(target_os = "linux")]
async fn ei_type_text(text: &str, daemon: &Daemon) -> Result<()> {
    use reis::ei;
    use std::collections::HashMap;

    const KEY_LEFTSHIFT: u32 = 42;
    const KEY_INSERT: u32 = 110;

    // Check if we have a usable cached EI context. Reuse it regardless of
    // whether a keymap is available — if there's no keymap we fall back to
    // Shift+Insert paste. This avoids creating a new XDG RemoteDesktop portal
    // session on every PTT event.
    {
        let ei_state = daemon.ei_state.lock().await;
        if let Some(ref state) = *ei_state {
            if state.keyboard.is_alive() {
                let last_serial: u32 = u32::MAX;
                state.device.start_emulating(0, last_serial);

                if state.keymap.is_null() {
                    // Fall back to Shift+Insert paste (no keymap available).
                    log::info!("ei: reusing cached context, Shift+Insert fallback");
                    state.keyboard.key(KEY_LEFTSHIFT, ei::keyboard::KeyState::Press);
                    state.keyboard.key(KEY_INSERT, ei::keyboard::KeyState::Press);
                    state.device.frame(last_serial, 0);
                    state.keyboard.key(KEY_INSERT, ei::keyboard::KeyState::Released);
                    state.keyboard.key(KEY_LEFTSHIFT, ei::keyboard::KeyState::Released);
                    state.device.frame(last_serial, 0);
                } else {
                    log::info!("ei: reusing cached context for typing ({} chars)", text.len());
                    for ch in text.chars() {
                        let (code, shift) = match char_to_evdev(ch) {
                            Some(x) => x,
                            None => {
                                if !state.keymap.is_null() {
                                    match unsafe { find_key_for_char_xkb(state.keymap, ch) } {
                                        Some(x) => x,
                                        None => {
                                            log::warn!("ei: cannot type U+{:04X}, skipping", ch as u32);
                                            continue;
                                        }
                                    }
                                } else {
                                    log::warn!("ei: U+{:04X} not in ASCII table and no keymap, skipping", ch as u32);
                                    continue;
                                }
                            }
                        };

                        if shift {
                            state.keyboard.key(KEY_LEFTSHIFT, ei::keyboard::KeyState::Press);
                        }
                        state.keyboard.key(code, ei::keyboard::KeyState::Press);
                        state.device.frame(last_serial, 0);

                        state.keyboard.key(code, ei::keyboard::KeyState::Released);
                        if shift {
                            state.keyboard.key(KEY_LEFTSHIFT, ei::keyboard::KeyState::Released);
                        }
                        state.device.frame(last_serial, 0);
                    }
                }

                state.device.stop_emulating(last_serial);
                if let Err(e) = state.context.flush() {
                    log::warn!("ei: flush failed on cached context, will recreate: {e}");
                    drop(ei_state);
                    *daemon.ei_state.lock().await = None;
                    return Err(anyhow!("ei flush: {e}"));
                }
                log::info!("ei: done with cached context");
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                return Ok(());
            }
            log::info!("ei: cached context no longer alive, recreating");
        }
    }

    // Create a new EI context — try env socket first, then XDG RemoteDesktop portal.
    let context = if let Some(ctx) = ei::Context::connect_to_env().ok().flatten() {
        log::info!("ei: connected via LIBEI_SOCKET");
        ctx
    } else {
        log::info!("ei: no LIBEI_SOCKET, requesting portal access");
        connect_eis_via_portal(&daemon.config.permission_lifetime).await?
    };

    // Blocking handshake on a dedicated thread.
    let context =
        tokio::task::spawn_blocking(move || -> Result<ei::Context> {
            let _handshake = context.handshake();
            context.flush().map_err(|e| anyhow!("ei flush handshake: {e}"))?;

            loop {
                context.read().map_err(|e| anyhow!("ei read handshake: {e}"))?;
                while let Some(result) = context.pending_event() {
                    if let reis::PendingRequestResult::Request(ei::Event::Handshake(hs, ev)) =
                        result
                    {
                        match ev {
                            ei::handshake::Event::HandshakeVersion { .. } => {
                                hs.handshake_version(1);
                                hs.name("utter");
                                hs.context_type(ei::handshake::ContextType::Sender);
                                for &(iface, ver) in &[
                                    ("ei_callback", 1),
                                    ("ei_connection", 1),
                                    ("ei_seat", 1),
                                    ("ei_device", 1),
                                    ("ei_pingpong", 1),
                                    ("ei_keyboard", 1),
                                ] {
                                    hs.interface_version(iface, ver);
                                }
                                hs.finish();
                                context.flush().map_err(|e| anyhow!("ei flush hs finish: {e}"))?;
                            }
                            ei::handshake::Event::Connection { .. } => {
                                return Ok(context);
                            }
                            _ => {}
                        }
                    }
                }
            }
        })
        .await
        .map_err(|e| anyhow!("ei handshake task: {e}"))??;

    // Discover devices — seat → bind → device → keyboard.
    #[derive(Default)]
    struct SeatData {
        caps: HashMap<String, u64>,
    }
    #[derive(Default)]
    struct DevData {
        keyboard: Option<ei::Keyboard>,
        resumed: bool,
    }

    let mut seats: HashMap<ei::Seat, SeatData> = HashMap::new();
    let mut devices: HashMap<ei::Device, DevData> = HashMap::new();
    let last_serial: u32 = u32::MAX;
    let mut found_keyboard = false;
    let mut keymap_fd: Option<std::os::unix::io::OwnedFd> = None;

    for _ in 0..200 {
        context.read().map_err(|e| anyhow!("ei read devices: {e}"))?;

        while let Some(result) = context.pending_event() {
            let request = match result {
                reis::PendingRequestResult::Request(r) => r,
                _ => continue,
            };
            match request {
                ei::Event::Connection(_, ei::connection::Event::Ping { ping }) => {
                    ping.done(0);
                }
                ei::Event::Seat(seat, ev) => {
                    let data = seats.entry(seat.clone()).or_default();
                    match ev {
                        ei::seat::Event::Capability { mask, interface } => {
                            data.caps.insert(interface.to_owned(), mask);
                        }
                        ei::seat::Event::Done => {
                            if let Some(&mask) = data.caps.get("ei_keyboard") {
                                log::info!("ei: binding seat to keyboard (mask={mask})");
                                seat.bind(mask);
                                context.flush().map_err(|e| anyhow!("ei flush bind: {e}"))?;
                            }
                        }
                        ei::seat::Event::Device { device } => {
                            devices.insert(device, DevData::default());
                        }
                        _ => {}
                    }
                }
                ei::Event::Device(device, ev) => {
                    let data = devices.entry(device).or_default();
                    match ev {
                        ei::device::Event::Interface { object } => {
                            if object.interface() == "ei_keyboard" {
                                if let Some(kb) = object.downcast::<ei::Keyboard>() {
                                    data.keyboard = Some(kb);
                                }
                            }
                        }
                        ei::device::Event::Resumed { .. } => {
                            data.resumed = true;
                            if data.keyboard.is_some() {
                                found_keyboard = true;
                            }
                        }
                        _ => {}
                    }
                }
                ei::Event::Keyboard(_kb, ev) => {
                    if let ei::keyboard::Event::Keymap { keymap: fd, .. } = ev {
                        log::info!("ei: received keymap from compositor");
                        keymap_fd = Some(fd);
                    }
                }
                _ => {}
            }
        }
        context.flush().map_err(|e| anyhow!("ei flush devices: {e}"))?;
        if found_keyboard {
            break;
        }
    }

    if !found_keyboard {
        return Err(anyhow!("ei: no keyboard device found after scanning events"));
    }

    let mut found_device = None;
    let mut found_keyboard_obj = None;
    for (device, data) in &devices {
        if let (Some(keyboard), true) = (&data.keyboard, data.resumed) {
            found_device = Some(device.clone());
            found_keyboard_obj = Some(keyboard.clone());
            break;
        }
    }

    let (device, keyboard) = match (found_device, found_keyboard_obj) {
        (Some(d), Some(k)) => (d, k),
        _ => return Err(anyhow!("ei: keyboard found but no events sent")),
    };

    // Parse the keymap from the compositor.
    let (xkb_keymap, xkb_state) = match keymap_fd {
        Some(fd) => match unsafe { parse_keymap_from_fd(fd) } {
            Ok((km, st)) => (SendPtr(km), SendPtr(st)),
            Err(e) => {
                log::warn!("ei: keymap parse failed, falling back to no keymap: {e:#}");
                (SendPtr(std::ptr::null_mut()), SendPtr(std::ptr::null_mut()))
            }
        },
        None => {
            log::warn!("ei: no keymap received from compositor");
            (SendPtr(std::ptr::null_mut()), SendPtr(std::ptr::null_mut()))
        }
    };

    // Cache the context for reuse.
    *daemon.ei_state.lock().await = Some(EiState {
        context: context.clone(),
        device: device.clone(),
        keyboard: keyboard.clone(),
        keymap: xkb_keymap.0,
        xkb_state: xkb_state.0,
    });

    if xkb_keymap.0.is_null() {
        // No keymap — fall back to Shift+Insert paste.
        log::warn!("ei: no keymap available, falling back to Shift+Insert");
        device.start_emulating(0, last_serial);

        keyboard.key(KEY_LEFTSHIFT, ei::keyboard::KeyState::Press);
        keyboard.key(110, ei::keyboard::KeyState::Press); // KEY_INSERT
        device.frame(last_serial, 0);

        keyboard.key(110, ei::keyboard::KeyState::Released);
        keyboard.key(KEY_LEFTSHIFT, ei::keyboard::KeyState::Released);
        device.frame(last_serial, 0);

        device.stop_emulating(last_serial);
        context.flush().map_err(|e| anyhow!("ei flush key events: {e}"))?;
        log::info!("ei: sent Shift+Insert via virtual keyboard (no keymap fallback)");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        return Ok(());
    }

    // Type each character using the ASCII→evdev lookup table, falling back
    // to the XKB keymap for non-ASCII characters.
    device.start_emulating(0, last_serial);

    for ch in text.chars() {
        // First try the fast direct evdev table.
        let (code, shift) = match char_to_evdev(ch) {
            Some(x) => x,
            None => {
                // Fallback: use XKB keymap for non-ASCII chars.
                if !xkb_keymap.0.is_null() {
                    match unsafe { find_key_for_char_xkb(xkb_keymap.0, ch) } {
                        Some(x) => x,
                        None => {
                            log::warn!("ei: cannot type U+{:04X}, skipping", ch as u32);
                            continue;
                        }
                    }
                } else {
                    log::warn!("ei: U+{:04X} not in ASCII table and no keymap, skipping", ch as u32);
                    continue;
                }
            }
        };

        if shift {
            keyboard.key(KEY_LEFTSHIFT, ei::keyboard::KeyState::Press);
        }
        keyboard.key(code, ei::keyboard::KeyState::Press);
        device.frame(last_serial, 0);

        keyboard.key(code, ei::keyboard::KeyState::Released);
        if shift {
            keyboard.key(KEY_LEFTSHIFT, ei::keyboard::KeyState::Released);
        }
        device.frame(last_serial, 0);
    }

    device.stop_emulating(last_serial);
    context.flush().map_err(|e| anyhow!("ei flush key events: {e}"))?;
    log::info!("ei: typed {} chars via virtual keyboard", text.len());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    Ok(())
}

/// Connect to the EI server via the XDG RemoteDesktop portal.
/// Persists the restore token to `~/.config/utter/ei_token` so the
/// permissions dialog only appears once.
#[cfg(target_os = "linux")]
async fn connect_eis_via_portal(permission_lifetime: &str) -> Result<reis::ei::Context> {
    use ashpd::desktop::remote_desktop::{
        ConnectToEISOptions, DeviceType, RemoteDesktop, SelectDevicesOptions, StartOptions,
    };
    use ashpd::desktop::{CreateSessionOptions, PersistMode};
    use enumflags2::BitFlags;
    use reis::ei;
    use std::os::unix::net::UnixStream;

    let token_path = dirs::config_dir()
        .context("no config dir")?
        .join("utter/ei_token");

    let saved_token = std::fs::read_to_string(&token_path).ok();

    let remote_desktop = RemoteDesktop::new()
        .await
        .context("ei: create RemoteDesktop proxy")?;

    let session = remote_desktop
        .create_session(CreateSessionOptions::default())
        .await
        .context("ei: create session")?;

    let persist_mode = match permission_lifetime {
        "session" => PersistMode::Application,
        _ => PersistMode::ExplicitlyRevoked,
    };

    let mut options = SelectDevicesOptions::default()
        .set_devices(BitFlags::from(DeviceType::Keyboard))
        .set_persist_mode(persist_mode);

    if let Some(ref token) = saved_token {
        options = options.set_restore_token(token.as_str());
        log::info!("ei: using saved restore token");
    }

    remote_desktop
        .select_devices(&session, options)
        .await
        .context("ei: select devices")?;

    let selected = remote_desktop
        .start(&session, None, StartOptions::default())
        .await
        .context("ei: start session")?
        .response()
        .context("ei: start response")?;

    // Persist the new restore token for next launch.
    if let Some(token) = selected.restore_token() {
        if let Some(parent) = token_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&token_path, token) {
            log::warn!("ei: failed to save restore token: {e:#}");
        } else {
            log::info!("ei: saved restore token to {}", token_path.display());
        }
    }

    let fd = remote_desktop
        .connect_to_eis(&session, ConnectToEISOptions::default())
        .await
        .context("ei: connect to EIS")?;

    let stream = UnixStream::from(fd);
    ei::Context::new(stream).context("ei: create context from portal fd")
}

/// Canonical short name for an evdev keycode. Used for pretty display in
/// the config file and CLI output. Anything not listed here still works —
/// it just gets stored as the raw numeric keycode instead. The set below
/// deliberately covers keys that are plausible push-to-talk choices (they
/// don't fire during normal typing); it's not a completeness statement.
#[cfg(target_os = "linux")]
fn canonical_name_for(code: evdev::KeyCode) -> Option<&'static str> {
    Some(match code {
        // Modifiers
        evdev::KeyCode::KEY_RIGHTMETA => "rightmeta",
        evdev::KeyCode::KEY_LEFTMETA => "leftmeta",
        evdev::KeyCode::KEY_RIGHTCTRL => "rightctrl",
        evdev::KeyCode::KEY_LEFTCTRL => "leftctrl",
        evdev::KeyCode::KEY_RIGHTALT => "rightalt",
        evdev::KeyCode::KEY_LEFTALT => "leftalt",
        evdev::KeyCode::KEY_RIGHTSHIFT => "rightshift",
        evdev::KeyCode::KEY_LEFTSHIFT => "leftshift",
        // Lock keys — common PTT picks because they're otherwise unused on most setups
        evdev::KeyCode::KEY_CAPSLOCK => "capslock",
        evdev::KeyCode::KEY_SCROLLLOCK => "scrolllock",
        evdev::KeyCode::KEY_NUMLOCK => "numlock",
        // Top-row utility keys
        evdev::KeyCode::KEY_PAUSE => "pause",
        evdev::KeyCode::KEY_SYSRQ => "printscreen",
        evdev::KeyCode::KEY_INSERT => "insert",
        evdev::KeyCode::KEY_COMPOSE => "menu",
        // Navigation cluster
        evdev::KeyCode::KEY_HOME => "home",
        evdev::KeyCode::KEY_END => "end",
        evdev::KeyCode::KEY_PAGEUP => "pageup",
        evdev::KeyCode::KEY_PAGEDOWN => "pagedown",
        // Function row — common QMK/keyd target for remapped layer keys
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
        evdev::KeyCode::KEY_F21 => "f21",
        evdev::KeyCode::KEY_F22 => "f22",
        evdev::KeyCode::KEY_F23 => "f23",
        evdev::KeyCode::KEY_F24 => "f24",
        _ => return None,
    })
}

#[cfg(target_os = "linux")]
fn parse_key_name(name: &str) -> Result<evdev::KeyCode> {
    let n = name.to_ascii_lowercase();
    let n = n.strip_prefix("key_").unwrap_or(&n);

    // Named aliases for common PTT-worthy keys. Everything not listed here
    // can still be used — see the numeric-code fallback below.
    let named = match n {
        "rightmeta" | "rightsuper" | "rightcmd" | "rightcommand" => Some(evdev::KeyCode::KEY_RIGHTMETA),
        "leftmeta" | "leftsuper" | "leftcmd" | "leftcommand" => Some(evdev::KeyCode::KEY_LEFTMETA),
        "rightctrl" | "rightcontrol" => Some(evdev::KeyCode::KEY_RIGHTCTRL),
        "leftctrl" | "leftcontrol" => Some(evdev::KeyCode::KEY_LEFTCTRL),
        "rightalt" | "rightoption" => Some(evdev::KeyCode::KEY_RIGHTALT),
        "leftalt" | "leftoption" => Some(evdev::KeyCode::KEY_LEFTALT),
        "rightshift" => Some(evdev::KeyCode::KEY_RIGHTSHIFT),
        "leftshift" => Some(evdev::KeyCode::KEY_LEFTSHIFT),
        "capslock" | "caps" => Some(evdev::KeyCode::KEY_CAPSLOCK),
        "scrolllock" | "scroll_lock" | "scroll" => Some(evdev::KeyCode::KEY_SCROLLLOCK),
        "numlock" | "num_lock" => Some(evdev::KeyCode::KEY_NUMLOCK),
        "pause" | "break" => Some(evdev::KeyCode::KEY_PAUSE),
        "printscreen" | "prtsc" | "prtscn" | "sysrq" => Some(evdev::KeyCode::KEY_SYSRQ),
        "insert" | "ins" => Some(evdev::KeyCode::KEY_INSERT),
        "menu" | "compose" | "contextmenu" | "context_menu" => Some(evdev::KeyCode::KEY_COMPOSE),
        "home" => Some(evdev::KeyCode::KEY_HOME),
        "end" => Some(evdev::KeyCode::KEY_END),
        "pageup" | "pgup" | "page_up" => Some(evdev::KeyCode::KEY_PAGEUP),
        "pagedown" | "pgdn" | "page_down" => Some(evdev::KeyCode::KEY_PAGEDOWN),
        "f1" => Some(evdev::KeyCode::KEY_F1),
        "f2" => Some(evdev::KeyCode::KEY_F2),
        "f3" => Some(evdev::KeyCode::KEY_F3),
        "f4" => Some(evdev::KeyCode::KEY_F4),
        "f5" => Some(evdev::KeyCode::KEY_F5),
        "f6" => Some(evdev::KeyCode::KEY_F6),
        "f7" => Some(evdev::KeyCode::KEY_F7),
        "f8" => Some(evdev::KeyCode::KEY_F8),
        "f9" => Some(evdev::KeyCode::KEY_F9),
        "f10" => Some(evdev::KeyCode::KEY_F10),
        "f11" => Some(evdev::KeyCode::KEY_F11),
        "f12" => Some(evdev::KeyCode::KEY_F12),
        "f13" => Some(evdev::KeyCode::KEY_F13),
        "f14" => Some(evdev::KeyCode::KEY_F14),
        "f15" => Some(evdev::KeyCode::KEY_F15),
        "f16" => Some(evdev::KeyCode::KEY_F16),
        "f17" => Some(evdev::KeyCode::KEY_F17),
        "f18" => Some(evdev::KeyCode::KEY_F18),
        "f19" => Some(evdev::KeyCode::KEY_F19),
        "f20" => Some(evdev::KeyCode::KEY_F20),
        "f21" => Some(evdev::KeyCode::KEY_F21),
        "f22" => Some(evdev::KeyCode::KEY_F22),
        "f23" => Some(evdev::KeyCode::KEY_F23),
        "f24" => Some(evdev::KeyCode::KEY_F24),
        _ => None,
    };
    if let Some(code) = named {
        return Ok(code);
    }

    // Numeric fallback — accept any raw evdev keycode as digits. Lets people
    // use keys utter doesn't have a short name for (scroll lock, pause,
    // F21-F24, media keys, remapped firmware layers, etc.) without us
    // needing to enumerate them.
    if let Ok(code) = n.parse::<u16>() {
        return Ok(evdev::KeyCode::new(code));
    }

    Err(anyhow!(
        "unknown key name: `{name}` (not a named alias and not a numeric evdev code)"
    ))
}

#[cfg(target_os = "macos")]
async fn run_watcher(key_arg: Option<&str>) -> Result<()> {
    macos::run_watcher(key_arg).await
}

#[cfg(target_os = "linux")]
async fn run_watcher(key_arg: Option<&str>) -> Result<()> {
    // --key flag wins; otherwise fall back to config.key.
    let cfg = Config::load_or_migrate(&Config::default_path()?, &config::utter_env_snapshot())?;
    let key_name = key_arg.unwrap_or(&cfg.key).to_string();
    let target = parse_key_name(&key_name)?;
    log::info!("watching for key {key_name} (code {})", target.code());

    let matching: Vec<(std::path::PathBuf, evdev::Device)> = evdev::enumerate()
        .filter(|(_, d)| {
            d.supported_keys()
                .is_some_and(|k| k.contains(target))
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

#[cfg(target_os = "linux")]
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
pub(crate) async fn send_command_quiet(cmd: &str) -> Result<()> {
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

#[cfg(target_os = "macos")]
async fn run_set_key(dry_run: bool, timeout_secs: u64) -> Result<()> {
    macos::run_set_key(dry_run, timeout_secs).await
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
async fn pick_key_and_maybe_save(dry_run: bool, timeout_secs: u64) -> Result<()> {
    // Enumerate anything that looks like a keyboard. KEY_A is present on
    // every real keyboard and absent on mice/touchpads/joysticks, so it's
    // a good discriminator that doesn't lock us to one modifier class.
    let devices: Vec<(std::path::PathBuf, evdev::Device)> = evdev::enumerate()
        .filter(|(_, d)| {
            d.supported_keys()
                .is_some_and(|k| k.contains(evdev::KeyCode::KEY_A))
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

    // Prefer the canonical short name when we have one (rightmeta, capslock,
    // f13, ...). Fall back to the raw numeric code for keys we don't have
    // an alias for — everything works at the evdev level regardless of
    // whether utter knows a short name for it.
    let name: String = canonical_name_for(code)
        .map(String::from)
        .unwrap_or_else(|| code.code().to_string());

    println!(
        "Detected: {name} (code {}). Press + release both captured — hold-to-talk will work.",
        code.code()
    );

    if dry_run {
        eprintln!("(--dry-run: not saving.)");
        return Ok(());
    }

    save_key_to_config(&name)?;
    println!("Saved key `{name}` to ~/.config/utter/config.toml.");
    Ok(())
}

/// Persist the chosen PTT key into the user's config file, preserving
/// whatever other values the file currently holds. Also removes a stale
/// pre-config-file `utter-watcher.service.d/override.conf` if present —
/// that file would otherwise shadow the new config-file key via
/// systemd's `ExecStart=` override.
#[cfg(target_os = "linux")]
fn save_key_to_config(key_name: &str) -> Result<()> {
    let path = Config::default_path()?;
    let env = config::utter_env_snapshot();
    let cfg = Config::load_or_migrate(&path, &env)?.with_key(key_name);
    cfg.save_to(&path)?;
    remove_stale_watcher_override();
    Ok(())
}

#[cfg(target_os = "linux")]
fn remove_stale_watcher_override() {
    let Some(config_dir) = dirs::config_dir() else { return; };
    let override_path = config_dir.join("systemd/user/utter-watcher.service.d/override.conf");
    if !override_path.exists() {
        return;
    }
    if let Err(e) = std::fs::remove_file(&override_path) {
        log::warn!("couldn't remove stale {}: {e:#}", override_path.display());
        return;
    }
    // Best-effort rmdir of the now-empty drop-in dir.
    if let Some(parent) = override_path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    log::info!("removed stale systemd drop-in at {}", override_path.display());
}

#[cfg(target_os = "linux")]
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
        // First key-down wins; release of the held key completes the test.
        // value 2 = autorepeat, ignored.
        match ev.value() {
            1 if pressed.is_none() => pressed = Some(code),
            0 if pressed == Some(code) => {
                let _ = tx.send(code).await;
                return Ok(());
            }
            _ => {}
        }
    }
}

#[cfg(target_os = "linux")]
fn watcher_is_active() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "utter-watcher.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn run_systemctl_user(args: &[&str]) -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{
        canonical_name_for, cleanup_transcription, parse_key_name,
    };

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

    #[test]
    fn parse_key_name_resolves_named_aliases() {
        assert_eq!(parse_key_name("rightmeta").unwrap(), evdev::KeyCode::KEY_RIGHTMETA);
        assert_eq!(parse_key_name("leftmeta").unwrap(), evdev::KeyCode::KEY_LEFTMETA);
        assert_eq!(parse_key_name("capslock").unwrap(), evdev::KeyCode::KEY_CAPSLOCK);
        assert_eq!(parse_key_name("scrolllock").unwrap(), evdev::KeyCode::KEY_SCROLLLOCK);
        assert_eq!(parse_key_name("f13").unwrap(), evdev::KeyCode::KEY_F13);
        assert_eq!(parse_key_name("f24").unwrap(), evdev::KeyCode::KEY_F24);
        assert_eq!(parse_key_name("printscreen").unwrap(), evdev::KeyCode::KEY_SYSRQ);
        assert_eq!(parse_key_name("menu").unwrap(), evdev::KeyCode::KEY_COMPOSE);
    }

    #[test]
    fn parse_key_name_accepts_apple_and_synonym_aliases() {
        assert_eq!(parse_key_name("rightcmd").unwrap(), evdev::KeyCode::KEY_RIGHTMETA);
        assert_eq!(parse_key_name("rightcommand").unwrap(), evdev::KeyCode::KEY_RIGHTMETA);
        assert_eq!(parse_key_name("rightsuper").unwrap(), evdev::KeyCode::KEY_RIGHTMETA);
        assert_eq!(parse_key_name("leftoption").unwrap(), evdev::KeyCode::KEY_LEFTALT);
        assert_eq!(parse_key_name("rightcontrol").unwrap(), evdev::KeyCode::KEY_RIGHTCTRL);
        assert_eq!(parse_key_name("caps").unwrap(), evdev::KeyCode::KEY_CAPSLOCK);
        assert_eq!(parse_key_name("sysrq").unwrap(), evdev::KeyCode::KEY_SYSRQ);
        assert_eq!(parse_key_name("pgup").unwrap(), evdev::KeyCode::KEY_PAGEUP);
        assert_eq!(parse_key_name("page_down").unwrap(), evdev::KeyCode::KEY_PAGEDOWN);
    }

    #[test]
    fn parse_key_name_is_case_insensitive() {
        assert_eq!(parse_key_name("RIGHTMETA").unwrap(), evdev::KeyCode::KEY_RIGHTMETA);
        assert_eq!(parse_key_name("RightMeta").unwrap(), evdev::KeyCode::KEY_RIGHTMETA);
        assert_eq!(parse_key_name("F13").unwrap(), evdev::KeyCode::KEY_F13);
    }

    #[test]
    fn parse_key_name_strips_key_prefix() {
        assert_eq!(parse_key_name("KEY_RIGHTMETA").unwrap(), evdev::KeyCode::KEY_RIGHTMETA);
        assert_eq!(parse_key_name("key_capslock").unwrap(), evdev::KeyCode::KEY_CAPSLOCK);
        assert_eq!(parse_key_name("KEY_F24").unwrap(), evdev::KeyCode::KEY_F24);
    }

    #[test]
    fn parse_key_name_numeric_fallback() {
        // 125 = KEY_LEFTMETA in linux/input-event-codes.h
        assert_eq!(parse_key_name("125").unwrap().code(), 125);
        // 194 = KEY_F24
        assert_eq!(parse_key_name("194").unwrap(), evdev::KeyCode::KEY_F24);
        // Arbitrary code with no short name — still accepted.
        assert_eq!(parse_key_name("240").unwrap().code(), 240);
    }

    #[test]
    fn parse_key_name_rejects_unknown_input() {
        assert!(parse_key_name("").is_err());
        assert!(parse_key_name("not_a_real_key").is_err());
        assert!(parse_key_name("f99").is_err());
        // Out-of-range for u16 falls through to the error path.
        assert!(parse_key_name("99999999").is_err());
    }

    #[test]
    fn canonical_name_roundtrips_through_parse_key_name() {
        // Every code with a canonical name should parse back to the same code.
        let codes = [
            evdev::KeyCode::KEY_RIGHTMETA,
            evdev::KeyCode::KEY_LEFTMETA,
            evdev::KeyCode::KEY_RIGHTCTRL,
            evdev::KeyCode::KEY_LEFTCTRL,
            evdev::KeyCode::KEY_RIGHTALT,
            evdev::KeyCode::KEY_LEFTALT,
            evdev::KeyCode::KEY_RIGHTSHIFT,
            evdev::KeyCode::KEY_LEFTSHIFT,
            evdev::KeyCode::KEY_CAPSLOCK,
            evdev::KeyCode::KEY_SCROLLLOCK,
            evdev::KeyCode::KEY_NUMLOCK,
            evdev::KeyCode::KEY_PAUSE,
            evdev::KeyCode::KEY_SYSRQ,
            evdev::KeyCode::KEY_INSERT,
            evdev::KeyCode::KEY_COMPOSE,
            evdev::KeyCode::KEY_HOME,
            evdev::KeyCode::KEY_END,
            evdev::KeyCode::KEY_PAGEUP,
            evdev::KeyCode::KEY_PAGEDOWN,
            evdev::KeyCode::KEY_F1,
            evdev::KeyCode::KEY_F12,
            evdev::KeyCode::KEY_F20,
            evdev::KeyCode::KEY_F24,
        ];
        for code in codes {
            let name = canonical_name_for(code)
                .unwrap_or_else(|| panic!("no canonical name for {code:?}"));
            let parsed = parse_key_name(name)
                .unwrap_or_else(|e| panic!("roundtrip failed for {name}: {e}"));
            assert_eq!(parsed, code, "roundtrip mismatch for {name}");
        }
    }

    #[test]
    fn canonical_name_absent_for_plain_letters() {
        // Letter keys aren't plausible PTT picks; we deliberately don't
        // assign canonical names and the caller falls back to the numeric
        // code instead.
        assert!(canonical_name_for(evdev::KeyCode::KEY_A).is_none());
        assert!(canonical_name_for(evdev::KeyCode::KEY_SPACE).is_none());
    }

}
