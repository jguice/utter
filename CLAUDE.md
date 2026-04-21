# CLAUDE.md — notes for Claude Code working on this repo

Concise conventions for Claude Code when developing utter. **User-facing install/configure/troubleshoot docs live in [`AGENTS.md`](AGENTS.md).** This file is only for working *on* the codebase.

## Project layout

- `src/main.rs` — everything: CLI dispatch, Unix-socket daemon, evdev watcher, arecord subprocess, clipboard/paste, disfluency cleanup. Single file on purpose while the surface area is small. Tests live in a `#[cfg(test)] mod tests` at the bottom.
- `Cargo.toml` / `Cargo.lock` — deps pinned; `rust-toolchain.toml` pins to stable 1.95. `Cargo.lock` **must** be committed — CI uses `--locked`, so version bumps to `Cargo.toml` without a corresponding lockfile update break the release build (see commit `0f39c01`).
- `examples/utter-{daemon,watcher}.service` — systemd user service files for **source installs** (`ExecStart=%h/.cargo/bin/utter …`). Used by `scripts/install.sh`.
- `packaging/` — artifacts consumed by the RPM/DEB builder (nfpm):
    - `packaging/nfpm.yaml` — declarative package spec (both formats).
    - `packaging/systemd/` — system-path unit files (`ExecStart=/usr/bin/utter …`). Separate from `examples/` because the paths differ.
    - `packaging/udev/90-utter.rules` — tags keyboard evdev devices with `uaccess` so users don't need `input` group membership.
    - `packaging/scripts/{post,pre}*.sh` — RPM/DEB maintainer scriptlets.
- `scripts/download-model.sh` — HF-pinned model fetcher. Pin lives in `MODEL_REV=` at the top.
- `scripts/install.sh` — from-source installer.
- `scripts/install-release.sh` — one-shot release installer (downloads .rpm/.deb from GitHub release).
- `.github/workflows/release.yml` — CI matrix build on tag push.

## Build, test, redeploy locally

```bash
cargo build --release --locked
cargo test --release            # runs the cleanup_transcription unit tests

# Redeploy a local build over a running install:
systemctl --user stop utter-watcher utter-daemon
cp --remove-destination target/release/utter ~/.cargo/bin/       # or sudo cp --remove-destination ... /usr/bin/ if package-installed
systemctl --user start utter-daemon
sleep 1                                        # daemon needs to bind the socket before the watcher starts
systemctl --user start utter-watcher
```

**Use `cp --remove-destination`, not plain `cp`.** `systemctl stop` returns as soon as SIGTERM is sent, but the kernel's exec mapping for the binary lingers briefly — plain `cp` then fails with `Text file busy` (`ETXTBSY`). `--remove-destination` unlinks the old file first and writes a new inode, which the kernel always allows regardless of whether the old inode is still mapped. If the shell continues past the failure (it will unless `set -e`), the subsequent `start` commands revive the *old* binary and the redeploy silently fails.

## Conventions

- No narrative comments. A function-level line is fine if the *why* isn't obvious; delete anything that just restates what the code does.
- No emojis in code or commit messages (user preference).
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `feat!:` or `BREAKING CHANGE:` footer for major, `docs:`, `refactor:`, `perf:`, `build:`, `chore:`, `ci:`, `test:`. release-please reads these to auto-generate `CHANGELOG.md` and bump `Cargo.toml` on merge.
- Version bumps: update `Cargo.toml`, then `cargo build` to sync `Cargo.lock`, then commit both together. Always tag on main.
- Breaking config changes (renamed env var, new required step, etc.) = minor version bump pre-1.0.

## Release process

Fully automated — just land conventional commits on main.

1. Push commits using conventional-commit prefixes (see above).
2. `.github/workflows/release-please.yml` watches main and opens a "chore: release vX.Y.Z" PR that bumps `Cargo.toml`, updates `CHANGELOG.md`, and bumps the manifest.
3. Review and merge the release PR.
4. release-please then tags `vX.Y.Z` and creates the GitHub Release with changelog-derived notes.
5. The tag push triggers `.github/workflows/release.yml`, which builds aarch64 + x86_64 `.deb`, `.rpm`, and tarballs and attaches them to the release.
6. Verify with `gh release view vX.Y.Z --json assets`.

Don't manually edit `Cargo.toml` version, `CHANGELOG.md`, or `.release-please-manifest.json` — release-please owns them.

## Common gotchas

- **`cargo install` "Text file busy"** — the running daemon has the binary mmap'd; stop it first.
- **`--locked` CI failure after a version bump** — commit the regenerated `Cargo.lock`.
- **Security hook blocks workflow writes** — GitHub Actions files get scanned for injection risks. Route any `${{ github.* }}` or `${{ inputs.* }}` values through an `env:` block before using them in shell.
- **`notify-send` / `ydotool` / `wl-copy` called from the daemon** — they inherit the daemon's environment. `UTTER_*` and `YDOTOOL_SOCKET` must be set in the service file, not assumed from the user's shell.
- **Socket mismatch between ydotool CLI and ydotoold daemon** — CLI looks at `$XDG_RUNTIME_DIR/.ydotool_socket` first, daemon (as system service) writes to `/tmp/.ydotool_socket`. Service env var `YDOTOOL_SOCKET=/tmp/.ydotool_socket` bridges them.

## When in doubt

Before recommending a syntax or flag for an external tool (ydotool, keyd, evdev, systemd, udev), read its man page or source. This repo has prior incidents from guessing — see `feedback_verify_before_recommending` in the persistent memory.
