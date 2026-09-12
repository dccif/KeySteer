# KeySteer

<p align="center">
  <img src="assets/brand/keysteer-wordmark.webp" alt="KeySteer" width="760">
</p>

<p align="center"><strong>From clicks to split screens, put your workspace on your keyboard.</strong></p>

<p align="center">
  <sub>Language / 语言 · <a href="README.md">简体中文</a> · <strong>English</strong></sub>
</p>

KeySteer brings native keyboard control to Windows and macOS. Move and click with `hjkl`, type labels to target controls, then move, tile, and group windows with Window mode. Keep your hands on the keys and your attention on the task.

[Download](https://github.com/dccif/KeySteer/releases/latest) · [Get started](https://dccif.github.io/KeySteer/en/guide/getting-started) · [Window guide](https://dccif.github.io/KeySteer/en/modes/window) · [Try the simulator](https://dccif.github.io/KeySteer/en/editor/)

## Features

- **Window**: move, resize, centre, and move windows between displays with `Alt+W`.
- **Quick & Editor**: place one window on half a screen or arrange several, then fine-tune their space.
- **Tabs & workspaces**: group windows into persistent tabs; save layouts and tab templates for reuse.
- **Audio controls**: adjust application or system volume and output devices from your layout. See the [Window guide](docs/en/modes/window.md) for platform requirements.

- **Normal**: move the pointer with Vim-style `hjkl` keys.
- **Hold and drag**: hold or toggle the left, middle, or right mouse button for dragging.
- **Grid**: quickly target a region with a two-key combination.
- **Recursive Grid**: keep subdividing the current region for precise targeting.
- **UI Hint**: type labels shown on buttons, links, menus, and inputs. macOS supports Accessibility Tree, Vision, and Hybrid strategies; Windows supports UI Automation, dual OCR visual recognition, and Hybrid.
- **Multiple displays**: use `Primary+S` to switch to the next display.
- **Appearance and configuration**: customise Grid/Hint labels, indicators, and more with TOML.

`Primary` is a cross-platform name: it is `Command` on macOS and `Alt` on Windows by default. Change it to a physical key you prefer in `[key_aliases]`.

## Nanosecond-scale core response

**151 ns on the core key-processing path** — developer-machine test for **0.9.21**. That is **0.151 μs / 0.000151 ms**. This is a core-path measurement, not total keyboard-to-screen latency; native input injection, rendering and display refresh add their own time.

## Two ways to start

| Try this | Default sequence |
| --- | --- |
| Move and click | `Primary+E` → `H/J/K/L` → `;` → `Esc` |
| Move a window | Pointer over the window → `Alt+W` → release entry keys → `H/J/K/L` → `Q` |
| Arrange several windows | `Alt+W` → `E` (applies immediately) → `Z` to undo |

On macOS, the Window entry is **Option+W**, not Command+W. From Window, use `A` for Quick, `T` for Tabs, and `R` for saved presets. Start with the [step-by-step Window guide](docs/en/modes/window.md).

## Video demonstrations

Seven short, silent simulator recordings: **keys and the current action centred together at the bottom**, with Chinese and English captions. Play the videos below directly on GitHub. All bindings shown are configurable.

### Window: control your workspace

Move with `H/J/K/L`, switch to resize with `S`, centre with `C`, and cycle maximize/minimize/restore with `F`. Use `V` chords for audio and `X` to close the selected window.

https://github.com/user-attachments/assets/a1f38691-f6cf-45bf-b264-03a1a70cd104

### Quick: press A for split layouts

Press `A` from Window, then use direction keys to place the window. Repeat a direction to change its ratio; customise `split_ratios`, or press `Q` to return to Window.

https://github.com/user-attachments/assets/e9a278cd-a05a-4931-80cc-d2f832142296

### Editor: press E to tile and fine-tune

`E` tiles immediately. Type `1`, then `2` to swap windows; use `Shift+direction` to split regions, `Ctrl+direction` to move dividers, and direction keys to adjust the layout.

https://github.com/user-attachments/assets/e21b68e3-f970-4e47-b5a2-fec80720b43b

### Tabs: group windows by application

On first entry with `T` from Window, windows are grouped by application. Use `Tab` / `Shift+Tab` to cycle through members and direction keys to move the group.

https://github.com/user-attachments/assets/2c1b894d-7402-4b57-9fd1-da03f5bd81a9

### Save: keep layouts and tab groups

Press `Ctrl+S` in Editor or Tabs, optionally enter a note, then press `Enter`. Leave the note empty for an automatic name. Saved presets describe layout and grouping arrangements, not a list of applications to launch.

https://github.com/user-attachments/assets/9965dbb6-ed80-4071-b636-563563b83b7d

### key_help: keep available actions in view

The panel lists available keys and actions. Add `"?" = "key_help"` to the relevant mode’s bindings to toggle it with the question mark, or choose another key. A binding is required. This clip invokes the same action with the simulator’s preview button.

https://github.com/user-attachments/assets/9c28c826-d465-4d47-b671-7a275c282a76

### Configuration & Simulator: edit on a full keyboard

Tray context menu → **Configuration & Simulator...** opens your browser (network access required). Click a key on the full keyboard and choose an action, e.g. `W → move_up` and `A → move_left`. The menu entry is illustrated; the binding editor is the actual project simulator. Native application behavior is authoritative.

https://github.com/user-attachments/assets/fbb5f750-fcda-4d3f-92da-1f7667387927

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

See the [configuration reference](docs/en/reference/configuration.md) and [modes and actions](docs/en/reference/modes-and-actions.md). You can also edit bindings and styles in the [Configuration & Simulator](https://dccif.github.io/KeySteer/en/editor/).

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
