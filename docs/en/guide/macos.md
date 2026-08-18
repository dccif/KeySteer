# macOS

KeySteer supports macOS 14 and later. Move `KeySteer.app` to your Applications folder.

## Install from a GitHub Release

Download the ZIP for your architecture from the [official GitHub Releases](https://github.com/dccif/KeySteer/releases/latest), extract it, and move `KeySteer.app` to `/Applications`. If Gatekeeper prevents it from opening, first confirm the download source, then run:

```bash
sudo xattr -cr /Applications/KeySteer.app
```

## First-time permissions

1. Before opening `KeySteer.app`, go to **System Settings → Privacy & Security → Accessibility**.
2. Allow KeySteer to control your computer.
3. Restart KeySteer and test it with `Primary+E`.

Vision detection in `UI Hint` also needs **Screen Recording** permission. Grid and Recursive Grid do not inspect screen contents, but keyboard capture still needs Accessibility permission. Grant both permissions up front to avoid later interruptions.

## File locations

The packaged app uses:

```text
~/Library/Application Support/KeySteer/
```

It contains:

- `keysteer.<name>.toml`: configuration files.
- `keysteer.log`: runtime log.
- `keysteer.log.1`, `.2`, `.3`: rotated logs.

## Scroll direction

When you use natural scrolling, vertical inversion is enabled by default. Adjust it in the configuration if needed:

```toml
[platform.macos.scroll]
invert_horizontal = false
invert_vertical = true
```

## Permission problems

If the menu-bar icon appears but keys do nothing:

1. Check that the Accessibility list authorises `KeySteer.app`.
2. Quit and reopen KeySteer.
3. Run `keysteer --doctor` to check whether the keyboard is available.
4. If it persists, inspect `keysteer.log` in the data directory.

Because future builds may not have a stable Developer ID signature, upgrades can require you to grant permission again.
