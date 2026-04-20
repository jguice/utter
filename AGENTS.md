# AGENTS.md

**Read [`README.md`](README.md) first.** It has everything an agent or human needs to install, configure, and troubleshoot utter — no duplicate content here. This file captures only the few things specific to running utter operations via an agent.

## Agent-specific notes

- **Don't run `install-release.sh` with `sudo`.** The script invokes `sudo` internally for the package install step. Running the whole thing as root breaks the per-user pieces (model download into the user's XDG dir, `systemctl --user enable`). It will refuse to run as root.

- **Verify after any install or config change.** Don't assume success just because a command exited 0. Run:
  ```bash
  utter --version
  systemctl --user is-active utter-daemon utter-watcher
  systemctl is-active ydotool
  ```
  Expect the version string and three `active` lines. If any differ, check `journalctl --user -u utter-daemon -n 30` and `journalctl --user -u utter-watcher -n 30`.

- **When editing service config, always use `systemctl --user edit <unit>`** rather than editing files under `/usr/lib/systemd/user/`. The latter are package-owned and will be overwritten on upgrade; the former writes to `~/.config/systemd/user/<unit>.service.d/override.conf`, which survives upgrades.

- **After editing any service, `systemctl --user daemon-reload` and then `restart` the affected unit.** Neither happens automatically.

- **Don't guess env var names or values.** They're listed in README's Configuration table. Valid values for `UTTER_PASTE_METHOD` are exactly `shift-insert`, `ctrl-v`, `ctrl-shift-v`, or `type` — any other string makes the daemon warn on every paste.

- **If a command requires `sudo`, tell the user to run it rather than trying to pipe a password.** Hook-based command execution typically can't forward a TTY for password prompts.
