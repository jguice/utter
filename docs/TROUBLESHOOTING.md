# Troubleshooting

**Nothing pastes after I release the key.**
```bash
journalctl --user -u utter-daemon -n 30
journalctl --user -u utter-watcher -n 30
```
Look for "recording to ..." / "transcribed in ..." in the daemon log and "key down → start" / "key up → stop" in the watcher log. If neither shows, the watcher isn't picking up the key. Check read access to `/dev/input/event*`:

```bash
ls -l /dev/input/event* | head
```
- **Packaged install:** the udev rule should grant ACL read access on login; run `sudo udevadm control --reload-rules && sudo udevadm trigger --subsystem-match=input` and log out+in if needed.
- **From-source install:** confirm you're in the `input` group (`id | grep input`). If not, `sudo usermod -aG input "$USER"` and log out+in.

**"no audio captured" error.**
Your mic isn't producing samples. Test with `arecord -d 3 /tmp/x.wav && aplay /tmp/x.wav`. If that fails, check `wpctl status` (default source is right and volume is non-zero) and `journalctl --user | grep spa.alsa`.

**Paste goes to the wrong place / nothing pastes.**
Shift+Insert pastes from the primary selection — which any mouse text-selection overwrites. If you highlighted something between releasing the key and the paste firing, the paste may use *that* text instead of your transcription. Release the PTT key in the window you want the text to land in and don't touch the mouse until it pastes.

If the cursor blinks but no text appears, check `journalctl --user -u utter-daemon -n 50` for `paste failed` or `wl-copy failed` warnings — those indicate ydotool or the compositor rejected something.
