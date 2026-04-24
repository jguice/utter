# Uninstall

## macOS

Drag `utter.app` to the Trash. The Parakeet model lives at `~/Library/Application Support/utter/` — `rm -rf ~/Library/Application\ Support/utter/` clears it plus the config file. Revoke the TCC grants in System Settings → Privacy & Security if you want a fully clean state.

## Linux — one command

```bash
curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/uninstall.sh | bash
```

Detects whether you installed from a package or from source, stops the user services, removes the binary + units + ydotool drop-in + user overrides, and (unless you pass `--keep-model`) deletes the Parakeet model under `~/.local/share/utter`. Run as your regular user; it `sudo`s internally where needed.

Flags: `--keep-model` to retain the model, `-y` to skip the confirmation prompt.

Leaves `ydotool` and your `input` group membership alone — both may be used by other tools. The uninstall output tells you how to remove them manually if you want.

## Manual

Prefer to do it by hand? The script is <100 lines, inspect it in `scripts/uninstall.sh`. The short version:

- **Package install:** `systemctl --user disable --now utter-{daemon,watcher}` then `sudo dnf remove -y utter` (or `sudo apt remove -y utter`).
- **Source install:** as above, plus `cargo uninstall utter` and `sudo rm -f /etc/systemd/system/ydotool.service.d/owner.conf`.
- **Model:** `rm -rf ~/.local/share/utter` (not package-owned).
