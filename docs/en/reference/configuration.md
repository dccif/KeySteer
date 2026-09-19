# Configuration

KeySteer does not require a configuration file. When none is present it uses built-in defaults, which match the shipped `keysteer.default.toml`.

Configurations use TOML. You can [download the complete shipped example](/generated/keysteer.default.toml). This guide starts with a working setup, then customisation and troubleshooting.

::: tip
To change only shortcuts, see [Getting started](/en/guide/getting-started).

For action parameters, arrays, `exec`, and plugin verbs, see the [Modes and actions reference](/en/reference/modes-and-actions).
:::

## Mode usage and quick switching

```toml
[mode_usage]
save_after_entries = 100

[quick_switch]
enabled = true
key = "q"
hold_ms = 350
blacklist = ["idle"]
position = "mouse" # screen / window / mouse

[quick_switch.ui]
font_size = 28
border_width = 1
border_radius = 6
padding_x = 6
padding_y = 8
# background_color = "#EEF2FFFF"
# text_color = "#193781FF"
# border_color = "#506DD0FF"
```

Each actual mode change increments its entry count; repeats and same-mode keep/restart do not. Counts stay in memory until the configured positive number of new entries triggers a background checkpoint to `workspace.ksw` (100 by default). Workspace edits and orderly exit also save pending counts. There is no periodic save timer. Windows/macOS system termination notifications request a checkpoint; power loss, forced termination or an expired system shutdown deadline can still lose unsaved counts.

In an operation mode, press Q alone and hold it for 350 ms to show the panel, or use Q+1…9 immediately. Releasing Q closes the panel; a short tap executes its original binding on release. Idle, paused operation, excluded applications and native note entry do not intercept Q. The panel shows up to nine registered modes, sorted by entry count and then mode id. Numbers stay fixed while it is open. Selecting the current mode preserves its state. The blacklist only filters the panel, leaving normal shortcuts available.

Positions are the current pointer screen's center (`screen`), the foreground window's center with a screen fallback (`window`), or below the pointer's mode indicator (`mouse`), clamped to the current screen. Styles also support `font_family` and per-appearance colors, resolved when configuration loads. The web editor displays imported workspace statistics and edits the threshold and panel options, preserving counts on export. Version 1 files remain readable; files containing statistics use workspace version 2.

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

Omitted fields retain their defaults, but an explicit bindings table such as `[normal.bindings]` replaces the corresponding default table. Removed or remapped plugin shortcuts are not restored from internal defaults. Start by copying the full default configuration. A user profile takes precedence over the default file; the two files are not merged.

After saving, select Reload Configuration in the status menu. The program returns to Idle; use your launcher to enter Normal again. `left_shift = "slow"` slows movement while held. Use `shift = "slow_toggle"` to toggle slow movement with either Shift; a retained `right_shift = "middle_click"` binding still takes precedence for right Shift.

You can also add number keys to your existing `[normal.bindings]` table:

```toml
"1" = "precision_toggle"
"2" = "slow_toggle"
"3" = "fast_toggle"
```

Tap to select precision, slow, or fast speed. Press the same key again to return to normal speed, or another key to switch levels. Configure the ratios with `precision_multiplier`, `slow_multiplier`, and `fast_multiplier` in `[pointer]`. Label modes such as Grid still give label input priority over these number bindings.

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

### Chords with a shared prefix

The engine compiles chord relationships when configuration is loaded. Built-in actions and plugins share this rule. Window movement defaults to `primary+d` (Alt+D on Windows, Command+D on macOS), independently of `primary+s` for switching the pointer display. You can also configure a shared prefix:

```toml
[normal.bindings]
"primary+s" = "screen next"
"primary+s+d" = "move_window next"
```

In this custom configuration, hold `Primary+S` and press D to move the window, or release the short chord to switch the pointer display. Use `move_window previous` or `move_window 2` for another destination. The final non-modifier key completes the chord. No timeout or plugin changes are needed. Inheritance, per-app overrides, `none`, and modifier sides are respected.

Mode changes and successful configuration reloads cancel pending actions. Standalone modifiers remain immediate.
A continuous action used as an ambiguous prefix receives one Down/Up tap on release; use independent keys for movement, long-press clicks, and toggle gestures.

### Key syntax

The left side of a binding accepts a single key, a chord, or multiple independent keys sharing one action:

```toml
[normal]
long_press_toggle_ms = 500
auto_release_ms = 0

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
auto_release_ms = 0

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

`long_press_toggle_ms` applies to bindings for mouse buttons and to a standalone parameterless `toggle` key. At the threshold, a mouse button remains down; a standalone `toggle` holds its activating key down even after the physical key is released. A parameterless `toggle` may use any activation key and latch any number of companions at once; companions and the activation key work in either order without waiting for the threshold. Each companion follows its Normal binding, so the default `;`, `'`, and `right_shift` bindings hold the left, right, and middle mouse buttons rather than those physical keys. A short standalone `toggle`, returning to Normal, or entering Idle releases all latches. Set `0` to disable this behaviour; the allowed range is `0..=60000` milliseconds.

`auto_release_ms` defaults to `0`, which keeps manual release. It applies only to a direct click/double-click binding latched by long press. Once one or more physical Shift/Ctrl/Alt/Win or Command keys are held and passed through, the first real pointer movement starts the delay and every later movement restarts it. When the pointer remains still for that long, KeySteer releases that mouse button and immediately clears its held indicator without affecting the physical modifiers. Latches created by explicit `press` or `toggle` actions do not participate. The allowed range is `0..=60000` milliseconds.

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

In `[recursive_grid.ui]`, `font_size = 0` (default) sizes letters at 40% of the shorter cell edge, then shrinks them to fit, using regular weight by default. Set a positive value such as `20` to specify a maximum font size.

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

## Window configuration

New to window control? Start with the [Window guide](/en/modes/window) for practical recipes and default keys.

Use the five top-level sections `[window]`, `[window_quick]`, `[window_editor]`, `[window_restore]` and `[window_tab]`. Each supports its own `bindings`, `inherits`, `app_configs`, `temporary_mode`, `temporary_mode_keys`, `number_timeout_ms`, `border_width`, `ui` and `lifecycle`. Defaults inherit hotkeys and temporarily use Normal while Primary is held.

| Parameter | Section | Default |
| --- | --- | --- |
| `move_step` / `move_speed` | Window | 20 / 600 |
| `resize_step` / `resize_speed` | Window and Editor, independently | 20 / 500 |
| `split_ratios` | Quick | `["1/4", "1/3", "1/2", "2/3", "3/4"]` |
| `gap` | Quick, Editor and Restore, independently | 0 |
| `number_timeout_ms` | All five | 250, range 100–2000 |
| `border_width` | All five | 3 |
| `lifecycle.after_finish` | Restore | `"window_editor"` |

Steps and gaps use logical pixels, speeds use logical pixels/second, and Windows applies the target display's DPI. Numbers and borders use each mode's `ui`; the shared panel uses `[key_help]` fonts and colors.

Merge these launchers into the existing hotkeys table:

```toml
[hotkeys]
"alt+w" = "window"
"alt+a" = "window_quick"
"alt+e" = "window_editor"
"alt+r" = "window_restore"

[window_quick]
split_ratios = ["1/5", "2/5", "3/5", "4/5"]
gap = 12.0

[window_editor]
resize_step = 10.0
gap = 12.0

[window_restore.lifecycle]
after_finish = "window_editor"
```

`window_delete` is an action that toggles restore/delete inside Restore (X by default), not a mode. Remove the old `[window_delete]` section and configure `[window_restore]` instead.

Setting parameters alone preserves default bindings. An explicit `[mode.bindings]` replaces that mode's whole default map. Copy the keys you need before changing one; Editor's `q = "grid"`, for example, always enters Grid regardless of entry path. Use `[[window_editor.app_configs]]` for app overrides and `inherits` to share bindings explicitly.

To migrate, replace `window_layout`, `window_edit` and `window_saved_layouts` with `window_quick`, `window_editor` and `window_restore`. Replace `window_cancel`/`window_exit` with the destination mode, such as `window` or `idle`. Remove `window.exit_mode`, `layout_keys` and `double_tap_ms`; move `window.split_ratios` to Quick and assign `window.gap` to Quick, Editor or Restore as needed. Removed fields and actions produce migration errors; user files are never rewritten automatically.

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

## Key help

The `key_help` verb toggles the available-key panel. Normal binds `"?" = "key_help"` by default, and targeting modes inherit it. Existing custom binding tables must add this binding explicitly. `?` matches the character produced by the operating system. Idle closes the panel.

`mouse_key_help` controls initial visibility in Normal, Grid, Recursive Grid, and UI Hint (default `false`). Set it to `true` to show the panel on entry. `?` always toggles it regardless of the default. Manual visibility persists between these modes; returning from Idle restores the default. The former `[key_help].enabled` setting is now named `mouse_key_help`.

`[key_help]` supports `font_family`, `font_size`, `background_color`, `text_color`, `border_color`, `border_width`, `border_radius`, `padding_x`, and `padding_y`. Empty font and omitted background/text colors follow the mode indicator and theme. Colors accept `#RRGGBBAA` or `{ light = "#RRGGBBAA", dark = "#RRGGBBAA" }`. Title sizing, columns and centering adapt automatically.

Window and its A/E/R/T submodes show the lower help panel by default. To start with it hidden:

```toml
[key_help]
mouse_key_help = false
window_key_help = false
```

These modes bind `"?" = "key_help"` by default, so `?` can still show or hide the panel. The manual choice persists between window submodes; leaving and reentering restores the configured default. Window borders and numbers remain visible. This setting is independent of `mouse_key_help`. Existing custom window binding tables must add `"?" = "key_help"` explicitly.

Use **Edit key help style** in the [simulator](/en/simulator) to preview changes and export TOML.

### Mouse side buttons

Windows and macOS accept `mouse_x1` (usually Back) and `mouse_x2` (usually Forward)
as binding triggers. Aliases are `xbutton1`/`mouse4` and `xbutton2`/`mouse5`.
Add the lines to the corresponding existing tables:

```toml
[hotkeys]
mouse_x1 = "normal"

[normal.bindings]
mouse_x1 = "key_help"
mouse_x2 = "grid"
"ctrl+mouse_x2" = "ui_hint"
```

Chords, inheritance, app overrides and held actions work as usual. For example,
`mouse_x2 = "scroll_down"` scrolls while held and stops on release. Unbound or
`none` side buttons pass through even in keyboard-capturing modes. A matched
binding consumes both edges. Physical side buttons do not emit `Clicked`.
These names also work as actions that inject native mouse side-button clicks:

```toml
[normal.bindings]
t = "mouse_x1"
y = "mouse_x2"
```

`press mouse_x1`, `release mouse_x1`, and `toggle mouse_x2` are supported too.
Clicks follow the usual long-press latch behavior. The receiving application decides
whether they navigate Back or Forward. If mouse software remaps a physical side
button to a keyboard shortcut, bind the shortcut that the driver actually emits.


Window and region labels use `[window.ui].font_size`, defaulting to 28. Region labels appear below the window-centre number. Help stays near the inside bottom edge when space permits and omits the redundant window-number key list.

Quick rulers use the labels from split_ratios: mix "1/2", "1/3", 0.3 and 0.45. Use a quoted decimal such as "0.30" to preserve trailing zeros. A screen-proportioned preview below Actions labels the current width and height; other ratios appear as small ticks. The inner rectangle shows the current placement.

### Window card guide lines

Shared by Window and its related modes, including lines created by label collision avoidance. Hiding lines does not disable collision avoidance.

```toml
[window.card]
guide_line_enabled = true
guide_line_width = 3.0 # 0–32; 0 also hides lines
# Inherits the card border color. The last two digits control alpha.
# Light/dark themed colors are also supported.
# guide_line_color = "#6E82D680"
```


## Temporary input layers

While a temporary trigger is held, resolve full chords in the temporary mode first, then the current mode. Only if neither matches, remove the trigger keys and try the temporary mode followed by the current mode again. Each layer includes its inherited bindings. Full matching requires all held keys. This also means Normal's explicit Primary+F opens UI Hint before its bare F can open Recursive Grid.

Set `temporary_mode_passthrough_keys = ["q"]` on Grid, Recursive Grid, UI Hint, a Window mode, or a plugin mode to skip the temporary layer for Q. The list defaults to `[]`, accepts aliases and chords, and matches either the full or trigger-stripped input. It routes to the current mode, without injecting keys into other applications.

The Quick Switch trigger now runs its original action immediately on key-down. Repeats and key-up continue through normal input routing. Holding the same press to `hold_ms` opens the switcher, even when its immediate action changed mode or entered Idle; release closes it without replaying the action. A press that starts in Idle does not arm the switcher.
