# Configuration

KeySteer does not require a configuration file. When none is present it uses built-in defaults, which match the shipped `keysteer.default.toml`.

Configurations use TOML. You can [download the complete shipped example](/generated/keysteer.default.toml). This guide starts with a working setup, then customisation and troubleshooting.

::: tip
To change only shortcuts, see [Getting started](/en/guide/getting-started).

For action parameters, arrays, `exec`, and plugin verbs, see the [Modes and actions reference](/en/reference/modes-and-actions).
:::

## Configuration file location

KeySteer looks for `keysteer.<name>.toml` in its data directory. If no user configuration exists, it tries `keysteer.default.toml`; if neither exists, it uses built-in defaults.

- Windows: the directory that contains the executable.
- Packaged macOS app: `~/Library/Application Support/KeySteer/`.

The name must be `keysteer.<name>.toml`, such as `keysteer.user.toml`. You can also specify it explicitly:

```bash
keysteer --config keysteer.user.toml
keysteer --config ./profiles/keysteer.work.toml
keysteer --check --config keysteer.user.toml
```

| Command | Purpose |
| --- | --- |
| `keysteer --check -c keysteer.user.toml` | Parse and validate a configuration without starting the runtime. |
| `keysteer --dump-config` | Print the complete effective configuration. |
| `keysteer --doctor` | Check the backend, keyboard, displays, permissions, and entry keys. |
| `keysteer --help` | Show CLI options and default shortcuts. |

## Minimal configuration

```toml
[normal.bindings]
# Use Space to return to Idle from Normal.
space = "idle"
```

Fields you do not specify retain their defaults. Starting with the shipped default configuration is recommended.

## Configuration structure

| Section | Purpose |
| --- | --- |
| `[general]` | Excluded applications. |
| `[key_aliases]` | Custom key names and cross-platform modifiers. |
| `[hotkeys]` | Mode entry points from Idle. |
| `[normal]`, `[grid]`, `[recursive_grid]`, `[ui_hint]` | Inheritance, bindings, and parameters for each mode. |
| `[pointer]`, `[scroll]` | Pointer speed and scroll distance. |
| `[theme]`, `[mode_indicator]` | Colours and mode indicators. |
| `[[app_configs]]` | Per-application binding overrides. |
| `[plugin_modes]` | Plugin settings and Mode bindings. |
| `[debug]` | Debug log categories. |

## Keys and aliases

### Key syntax

The left side of a binding accepts a single key, a chord, or multiple independent keys sharing one action:

```toml
[normal]
long_press_toggle_ms = 500

[normal.bindings]
h = "move_left"
"primary+shift+s" = "send primary+shift+s"
"v b" = "fast"
```

- `+` means one chord.
- A space means several independent keys bound to the same action, not a key sequence.
- In the shipped configuration, `primary` is `Command` on macOS and left `Alt` on Windows. It is an alias; change `[key_aliases.windows]` to use `Ctrl` on Windows.
- Generic modifiers such as `ctrl`, `alt`, and `shift` match both sides. `left_` and `right_` match one side only.

Common names include `a-z`, `0-9`, `space`, `enter`, `esc`, `tab`, `delete`, `backspace`, `up`, `down`, `left`, `right`, `home`, `end`, `page_up`, `page_down`, `f1-f20`, and `numpad_0-numpad_9`.

### Custom aliases

```toml
[key_aliases]
Hyper = "right_ctrl"

[key_aliases.windows]
Primary = "left_alt"

[key_aliases.macos]
Primary = "left_cmd"
```

Top-level aliases apply on all platforms. An alias value must be one key, never a chord, and aliases are case-insensitive. `Primary` is a parsable cross-platform alias, not a fixed physical key; `primary` and `Primary` refer to the same alias.

### Bindings, arrays, and inheritance

The value can be one string or an array of strings:

```toml
[normal.bindings]
h = "move_left"
x = ["press shift", "left_click", "release shift"]
"primary+shift+b" = ["exec say start", "wait 300", "exec say done"]
```

Array actions run from left to right. `wait` pauses only that sequence, never the whole event loop; an empty array is invalid.

The effective binding table for a Mode is merged in this order:

1. Its own `[<mode>.bindings]`.
2. Parent Modes listed in `inherits`, in written order.
3. Matching `app_configs` overrides.
4. Suggested plugin bindings, which fill only unoccupied keys.

The runtime only replaces bindings while merging.

```toml
[grid]
inherits = ["hotkeys", "normal"]

[grid.bindings]
q = "none" # Disable q inherited from Normal.
```

`none` and `__disabled__` both explicitly disable a binding. Keep at least one `[hotkeys]` entry; otherwise KeySteer runs but cannot enter a Mode from Idle.

## Normal and targeting modes

### Normal

```toml
[normal]
long_press_toggle_ms = 500

[normal.bindings]
h = "move_left"
j = "move_down"
k = "move_up"
l = "move_right"
";" = "left_click"
g = "grid"
f = "recursive_grid"
"primary+f" = "ui_hint"
"primary+s" = "screen next"
```

`passthrough_unbound_keys = true` is the default: Normal consumes only input matching a complete KeySteer binding. Unbound keys and unconfigured modifier chords preserve their original down/up lifecycle. With `false`, Normal returns to keyboard exclusivity and its old permissive chord matching. Grid, Recursive Grid, and UI Hint are always exclusive; Idle always passes through unmatched input and also uses complete chord matching.

`long_press_toggle_ms` applies to bindings for mouse buttons and to a standalone parameterless `toggle` key. At the threshold, a mouse button remains down; a standalone `toggle` holds its activating key down even after the physical key is released. A short standalone `toggle` releases all latches. Set `0` to disable this behaviour; the allowed range is `0..=60000` milliseconds.

### Grid

```toml
[grid]
grid_cols = 5
grid_rows = 4
keys = "12345qwertasdfgzxcvb"
max_depth = 3
cursor_follow_selection = true

[grid.lifecycle]
after_finish = "normal"
after_click = "finish"
```

`keys` must contain exactly `grid_cols × grid_rows` characters, assigned left-to-right and top-to-bottom. `max_depth` is the largest number of selection levels before confirmation. The first screen has large first-key labels and small second-key previews; `[grid.ui].matched_text_color`, `text_color`, and `matched_border_color` control these. The preview changes only drawing, not selection depth.

### Recursive Grid

```toml
[recursive_grid]
grid_cols = 3
grid_rows = 3
keys = "qweasdzxc"
max_depth = 10
min_size_width = 1
min_size_height = 1

[recursive_grid.lifecycle]
after_finish = "keep"
after_click = "keep"
```

`max_depth` must be in `1..=20`. `layers` can override the grid shape at a given depth; omitted fields inherit the base setting:

```toml
[recursive_grid]
layers = [
  { depth = 0, grid_cols = 2, grid_rows = 2, keys = "crtn" },
]
```

### UI Hint

```toml
[ui_hint]
strategy = "hybrid" # axtree, vision, or hybrid
hint_characters = "asdfghjkl"
scan_timeout_ms = 2500
scan_retry_count = 1
scan_retry_delay_ms = 200
visible_check_enabled = false
clickable_roles = ["button", "link", "checkbox", "text_field", "menu_item"]

[ui_hint.lifecycle]
after_finish = "normal"
after_click = "normal"
```

macOS supports Accessibility Tree, Vision, and Hybrid. Windows defaults to `hybrid`: UIA and the full visual pipeline run in parallel, stream their results, deduplicate, and merge them; this includes native minimise, maximise, and close buttons. `vision` uses available system OCR and automatically discovered WeChat OCR in parallel, then falls back to built-in pixel-region recognition. OCR needs no configuration and WeChat components are not distributed with KeySteer. `clickable_roles` are cross-platform semantic roles; use `ax:` or `uia:` for native roles.

## Pointer, scrolling, and theme

```toml
[pointer]
initial_speed = 1000.0
max_speed = 2200.0
acceleration = 3000.0
smooth_acceleration = true
tap_distance = 2.5
slow_multiplier = 0.35
precision_multiplier = 0.12
fast_multiplier = 2.0

[scroll]
scroll_step = 50
scroll_step_half = 500
scroll_step_full = 1000000

[platform.macos.scroll]
invert_horizontal = false
invert_vertical = true
```

Speed is pixels/second and acceleration is pixels/second², independent of display refresh rate. `smooth_acceleration = true` uses a gentler S curve; `false` uses linear acceleration.

Theme colours use `#RRGGBBAA` and can differ for light and dark appearance:

```toml
[theme.dark]
surface = "#0A1338FF"
accent = "#6E82D6FF"
accent_alt = "#8FA2F0FF"
on_accent_alt = "#081022FF"
text = "#E8EEFFFF"

[mode_indicator.cursor]
left_pressed_color = "#00FF00FF"
middle_pressed_color = "#FF00FFFF"
right_pressed_color = "#00FFFFFF"
```

When a pointer button is held with `press` or `toggle`, its translucent circular indicator uses the matching `*_pressed_color` at 20% opacity.

## Application overrides: `[[app_configs]]`

Use application overrides to disable or replace bindings in selected programs:

```toml
[[app_configs]]
bundle_id = "com.apple.Terminal"
bindings = { "primary+shift+e" = "none" }

[[normal.app_configs]]
bundle_id = "Figma"
bindings = { v = "none", "primary+f" = "grid" }
```

Root `[[app_configs]]` applies to every Mode; `[[normal.app_configs]]` applies only in Normal. A match can be a macOS bundle ID, a Windows executable name, or part of a window title.

## Plugin settings

Plugin Modes use a namespace:

```toml
[plugin_modes."plugin:screen-selector".settings]
preserve = true

[plugin_modes."plugin:screen-selector"]
inherits = ["hotkeys", "normal"]
```

For the built-in Screen Selector, `preserve = true` keeps the Grid or Recursive Grid selection path when switching displays. Set it to `false` to begin on the target display.

## Runtime changes and debugging

`set_config` edits a dotted path and atomically writes the configuration only after parsing and validation succeed:

```toml
[normal.bindings]
"primary+1" = "set_config pointer.max_speed 800"
"primary+2" = "set_config theme.dark.accent \"#FF8800FF\""
```

Invalid changes leave the active configuration untouched. The status menu's Reload Configuration reloads it from disk.

```toml
[debug]
enabled = true
keys = true
actions = true
modes = true
backend = true
pointer = false
motion = false
overlay = true
timers = true
```

Enable debug logs only while investigating an issue; they are written to `keysteer.log` in the data directory.
