# KeySteer

<p align="center">
  <img src="assets/brand/keysteer-wordmark.webp" alt="KeySteer" width="760">
</p>

<p align="center"><strong>Put your mouse in the hands of your keyboard: lightweight, native, and configurable.</strong></p>

<p align="center">
  <sub>Language / 语言 · <a href="README.md">简体中文</a> · <strong>English</strong></sub>
</p>

KeySteer is a keyboard-driven mouse-control tool for Windows and macOS.

[Documentation](https://dccif.github.io/KeySteer/en/) · [中文文档](https://dccif.github.io/KeySteer/)

## Features

- **Normal**: move the pointer with Vim-style `hjkl` keys.
- **Hold and drag**: hold or toggle the left, middle, or right mouse button for dragging.
- **Grid**: quickly target a region with a two-key combination.
- **Recursive Grid**: keep subdividing the current region for precise targeting.
- **UI Hint**: type labels shown on buttons, links, menus, and inputs. macOS supports Accessibility Tree, Vision, and Hybrid strategies; Windows supports UI Automation, dual OCR visual recognition, and Hybrid.
- **Multiple displays**: use `Primary+S` to switch to the next display.
- **Appearance and configuration**: customise Grid/Hint labels, indicators, and more with TOML.

`Primary` is a cross-platform name: it is `Command` on macOS and `Alt` on Windows by default. Change it to a physical key you prefer in `[key_aliases]`.

## Video demonstrations

### Normal

Keyboard movement, speed modifiers, scrolling, and clicking.

https://github.com/user-attachments/assets/10c990c4-903c-49fb-b8d7-5441430d3496

### Grid

Large first-level labels, second-level previews, and rapid targeting with two keys.

https://github.com/user-attachments/assets/4ecb749e-d770-43c1-907a-e55a4144a9ca

### Recursive Grid

Progressive subdivision, backtracking, and precise targeting.

https://github.com/user-attachments/assets/cb399755-5cde-40a0-ba64-d00c7e581cc6

### UI Hint

Scan interface elements, filter labels, and target controls.

https://github.com/user-attachments/assets/71efcae3-eb11-46d0-aba4-0a5df5e9c80c

## Default keys

Press `Primary+E` to enter `Normal`. In the shipped configuration, `Primary` is `Command` on macOS and left `Alt` on Windows; you can change it in `[key_aliases]`.

| Key | Action |
| --- | --- |
| `h j k l` | Move left, down, up, right |
| `Caps Lock` / `Left Shift` / `v` or `b` | Precision / slow / fast movement |
| `m` / `,` | Scroll down / up |
| `;` / `'` / `Right Shift` | Left / right / middle click |
| `n` | Toggle a held mouse button for dragging |
| `g` / `f` / `Primary+F` | `Grid` / `Recursive Grid` / `UI Hint` |
| `Primary+S` | Switch to the next display |
| `q` or `Esc` | Return to Idle |

## Configuration

No configuration file is required: the built-in `Config::default()` matches the shipped [`keysteer.default.toml`](keysteer.default.toml).

The application first selects a user configuration named `keysteer.<name>.toml` in its data directory (excluding `keysteer.default.toml`). If no user configuration exists, it loads the default TOML; if that is also absent, it uses the built-in defaults. An explicit `--config`/`-c` always takes precedence.

```bash
# Validate the repository's default example; ./ denotes this exact path.
cargo run -- --check -c ./keysteer.default.toml

# Print the complete effective configuration.
cargo run -- --dump-config

# Check permissions, displays, the input backend, and the foreground app.
cargo run -- --doctor
```

See the [configuration reference](docs/reference/configuration.md) and [modes and actions](docs/reference/modes-and-actions.md). You can also edit bindings and styles in the [Configuration & Simulator](https://dccif.github.io/KeySteer/en/editor/).

## Installation

Download the ZIP matching your operating system and CPU architecture from [GitHub Releases](https://github.com/dccif/KeySteer/releases/latest). On Windows, extract it and run `KeySteer.exe`. On macOS, extract it and move `KeySteer.app` to `/Applications`.

If Gatekeeper prevents a manually installed macOS application from opening, first confirm that it came from the official release page above, then run:

```bash
sudo xattr -cr /Applications/KeySteer.app
```

Grant Accessibility and Screen Recording permissions before opening the app.

## Development and packaging

The development Rust version is specified by `rust-toolchain.toml`. The documentation site requires Node 24+ and the pnpm version pinned in `package.json`.

```bash
cargo run
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings

pnpm install
pnpm docs:build
```

## Supported platforms

- Windows 10/11: x64 and ARM64
- macOS 14+: Apple Silicon and Intel

## License and copyright

Copyright © 2026 dccif. KeySteer is released under the **GNU General Public License v3.0 or later (GPL-3.0-or-later)**; see [LICENSE](LICENSE) for the full text.

You may use, study, modify, and redistribute this project. Any modified version or derivative containing this project that you distribute must provide corresponding source under the GPL. The GPL does not restrict private modifications that are not distributed outside yourself or your organisation.
