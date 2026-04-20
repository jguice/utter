# Backlog

Deferred improvements. Not urgent, but worth capturing.

## UX

- **Warn on very long recordings.** Parakeet is optimized for 15-60s
  utterances and degrades beyond ~5 min. Fire a `notify-send` warning
  when a recording crosses ~3 min, and hard-stop + warn at ~10 min
  (model accuracy drops, inference memory balloons).
- **Visible recording indicator** (optional). Plasma shows a mic icon
  in the tray while audio is captured, but a small OSD overlay (like
  Wispr Flow's) would be nicer. Likely needs a small Qt/GTK app or a
  KDE widget.
- **Transcription history / undo.** Keep the last N transcriptions so
  the user can paste an earlier one without re-dictating.

## Portability

- **Detect & handle more distros in install.sh.** Currently: Fedora,
  Debian/Ubuntu, Arch. Add openSUSE (`zypper`), NixOS (flake),
  Alpine (`apk`) if requests come in.
- **Per-compositor input injection.** We default to ydotool (works
  everywhere via uinput). On wlroots/GNOME, `wtype` avoids needing a
  privileged daemon. Detect the compositor and prefer the lighter
  tool where available.
- **X11 fallback.** We assume Wayland for `wl-copy` and `ydotool`. An
  X11 path would use `xclip`/`xsel` and `xdotool`. Not a priority.

## Packaging

v0.1.0 ships prebuilt `.deb` and `.rpm` (both arm64 and amd64) as
GitHub release assets via nfpm + GitHub Actions, plus a one-liner
installer (`scripts/install-release.sh`). The remaining wins are
about discoverability and auto-updates:

- **COPR repo (Fedora).** `sudo dnf copr enable jguice/utter &&
  sudo dnf install utter` adds auto-updates via `dnf`. The spec file
  can mostly reuse the nfpm config.
- **Self-hosted apt repo** on GitHub Pages (via `aptly` or `reprepro`)
  for `sudo apt install utter` + auto-updates on Debian/Ubuntu.
- **Launchpad PPA** (Ubuntu-only alternative to the apt repo).
- **AUR package (Arch).**
- **Flatpak.** Probably painful because of /dev/uinput and /dev/input
  access requirements — Flatpak sandboxes heavily restrict both.
- **Bundle model as a GitHub release asset.** ~640 MB fits under the
  2 GB-per-asset limit. Acts as a mirror if the HF repo ever disappears.
- **In-app update check.** Utter daemon could compare its running
  version to the latest GitHub release and `notify-send` when a new
  one is available. Pairs well with the COPR/apt repo once those land.

## Reliability

- **Hotplug support in the watcher.** Currently enumerates
  `/dev/input/event*` once at startup. If you plug in a new keyboard
  later, the watcher won't pick it up without a restart. Subscribe to
  udev events instead.
- **Graceful daemon shutdown.** On SIGTERM, finish any in-flight
  transcription before exiting. Right now a restart mid-transcription
  drops the audio.
- **CI.** GitHub Actions using `ubuntu-24.04-arm` runners to verify
  `cargo build` doesn't break on pushes.

## Model / quality

- **Local-LLM cleanup (tier 2).** Tier 1 (regex) ships today — a
  token-scan drops fillers (uh/um/er/ah/erm/hmm), collapses 3+
  same-word repetitions, and folds short-token stutters into the full
  word that follows ("wh wh wh what" → "what"). Handles the obvious
  cases with zero deps. Gated behind `UTTER_CLEANUP=1` (default on).
  Tier 2: match OpenWhispr's quality by bundling a small instruct
  model (Qwen 2.5 3B or Llama 3.2 3B, q5_k_m) via `llama-cpp-rs` or a
  llama.cpp sidecar; apply OpenWhispr's cleanup prompt. Gate behind
  `UTTER_LLM_CLEANUP=1` + model path. Adds ~2-4 GB + ~500 ms
  latency on a 3B model. Opt-in for users who want full LLM polish.
- **Initial prompt support.** transcribe-rs doesn't expose Parakeet's
  initial-prompt feature yet; would help with domain vocabulary
  (code identifiers, proper nouns).
- **Voice activity detection.** Trim leading/trailing silence before
  feeding the model — small accuracy + speed win.
- **Streaming mode.** Parakeet TDT supports streaming; transcribe-rs
  doesn't wire it up yet. Would unlock real-time incremental output
  for long dictations.

## Configuration

- **Config file** (`~/.config/utter/config.toml`) to replace the
  remaining `UTTER_*` env vars (cleanup, notify) plus the watcher key
  (currently written as a systemd drop-in by `utter set-key`).
  Migration path: on first run, if `config.toml` doesn't exist, read
  any `UTTER_*` env vars that *are* set and persist them; env vars
  still override the file (standard UNIX precedence). Ship the service
  units without `Environment=UTTER_*` lines in the same release — new
  installs start clean, existing installs keep working via the
  override ordering.
- **Opt-in clipboard write.** The default paste path only writes the
  primary selection, leaving the regular clipboard untouched. Users
  with clipboard managers may want dictations to land in their
  clipboard history — expose this as a config-file toggle once the
  config file lands.
- **Re-introduce paste-method selection IF a real app breaks
  Shift+Insert.** The current default (Shift+Insert, primary
  selection) covers every terminal + GTK/Qt input we've tested. The
  `ctrl-v` / `ctrl-shift-v` branches were removed because they were
  speculative — no app in our test set needed them. If a user reports
  a specific app where Shift+Insert fails, add a per-app or
  config-file override **with that app documented as the motivation**.
- **Per-app paste method.** Different apps want different keystrokes
  (Claude Code wants Shift+Insert, Konsole wants Ctrl+Shift+V, most
  GUIs want Ctrl+V). Look up the focused window class via KWin
  D-Bus / wlroots toplevel-management and dispatch accordingly. Only
  worth building if the previous item turns up real-world breakage.
