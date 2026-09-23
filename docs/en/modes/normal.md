# Normal mode

<script setup>
import ModeVideo from '../../.vitepress/components/ModeVideo'
import KeyLayout from '../../.vitepress/components/KeyLayout'
</script>

`Normal` is the most commonly used and easiest mode: move the pointer, scroll, click, drag, and enter the three targeting modes. Enter it from Idle with `Primary+E`.

By default, it intercepts only configured bindings. Unbound input still reaches the active application or tools such as AHK and Quicker. For example, if only `h = "move_left"` is configured, bare `h` moves the pointer while unconfigured `Alt+H` passes through intact.

```toml
[normal]
passthrough_unbound_keys = true # Set false to restore keyboard exclusivity.
```

<ModeVideo file="normal.mp4" title="Normal mode demonstration" description="Keyboard pointer movement, speed modifiers, scrolling, and common click operations." />

## Default keys

<KeyLayout
  layout="q w e r t y u i o p/Caps a s d f g h j k l ; '/Shift z x c v b n m , . Slash RShift/Ctrl Primary Alt Space"
  move="h j k l"
  click="; ' RShift"
  speed="Caps Shift v b"
  scroll="m ,"
  state="n"
  navigation="t y u i"
  mode="e f g q Primary"
  label="Normal default key groups"
  hint="Colours identify action types; blank keys are unassigned."
/>

| Key | Action |
| --- | --- |
| `h` `j` `k` `l` | Move left, down, up, right |
| `Caps Lock` / `Left Shift` | Precision / slow movement |
| `v` or `b` | Fast movement |
| `m` / `,` | Scroll down / up |
| `;` / `'` / `Right Shift` | Left / right / middle click |
| `n` | Toggle held input for dragging |
| `t` / `y` / `i` / `u` | Send `Home` / `End` / `Page Up` / `Page Down` |
| `g` / `f` / `Primary+F` | `Grid` / `Recursive Grid` / `UI Hint` |
| `Primary+S` | Switch to the next display |
| `q` / `Esc` | Return to Idle |

## Pointer speed

The default `precision`, `slow`, and `fast` bindings are held modifiers. To use
point-and-toggle behaviour, bind a key to `precision_toggle`, `slow_toggle`, or
`fast_toggle`; press the same binding again to clear it. The active speed is
shown below the mode indicator.

Speed uses pixels per second and acceleration. Smooth acceleration softens changes both when beginning and approaching top speed. Set `smooth_acceleration` to `false` for linear acceleration.

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
```

## Dragging and holding

`n = "toggle"` can keep any number of companions pressed in either order. Each companion follows its Normal binding: the default `;`, `'`, and `right_shift` bindings hold the left, right, and middle mouse buttons rather than the physical keys. A standalone tap, returning to Normal, or entering Idle releases every held target.

Hold a key bound to a mouse button for `long_press_toggle_ms` to latch that mouse button down; tap `n = "toggle"` to release it.

Set `auto_release_ms` to a non-zero value to finish a modifier-assisted drag without another release key. It applies only to a direct click/double-click binding latched by long press. Once one or more physical Shift/Ctrl/Alt/Win or Command keys are held and passed through, the first real pointer movement starts the delay and every later movement restarts it. When the pointer remains still for that long, KeySteer releases that mouse button and immediately clears its held indicator without affecting the physically held modifiers. Latches created by explicit `press` or `toggle` actions do not participate.

```toml
[normal]
passthrough_unbound_keys = true
long_press_toggle_ms = 500 # Set 0 to disable.
auto_release_ms = 0        # Set 0 to keep manual release.
```

## Custom shortcuts

```toml
[normal.bindings]
"primary+space" = "left_click"
"primary+g" = "grid"
q = "idle"
```

::: warning
In a targeting Mode, Grid or UI Hint labels take priority over inherited Normal bindings. Hold `Primary` to use Normal temporarily.
:::

## Temporary text input

`\` enters Text Input from Normal. Ordinary typing passes through; all exits use standard bindings:

```toml
[normal.bindings]
'\' = "text_input"

[text_input]
inherits = []
temporary_mode = "normal"
temporary_mode_keys = ["primary"]
temporary_mode_passthrough_keys = []

[text_input.bindings]
'enter \ esc' = "normal"

# Optional: submit Enter before returning. Replace the grouped binding above:
# '\ esc' = "normal"
# enter = ["send enter", "normal"]
```

Default exit keys are consumed. Remove enter from the binding for multiline editing or IME candidate selection. Explicit binding tables replace defaults; add the entry to your existing Normal table.

Hold Primary to borrow Normal and release it to continue typing. Primary uses key_aliases like other modes. Direct `inherits = ["normal"]` is supported, with local overrides winning, but inherited bare letters intercept typing. Entry and temporary-layer release clean up gestures and latched inputs; Q does not start Quick Switch while typing.

Home-row mappings are commented examples, disabled by default: Primary+H/J/K/L sends arrows, Primary+U/I/O sends Backspace/Delete/Insert, and Primary+T/Y sends Home/End. Ctrl, Shift and Ctrl+Shift variants are also commented. Choose modifiers that do not duplicate Primary. Explicit full chords precede trigger-stripped temporary-layer bindings. Mappings retain existing Send modifier suspension/restoration and key repeat behavior.

Text Input hides its text badge by default through `[mode_indicator.modes.text_input] enabled = false`. Temporary Normal uses its own badge settings and receives plugin results such as `screen next`.
## Mode badge style and position

Native and web badges now share the same size and positioning formulas; the extra preview display scaling compensation has been removed. The origin is the system cursor hotspot (usually the arrow tip and the cursor ring's centre), not the centre of the arrow image. Windows uses desktop coordinate pixels; macOS uses logical points. Backends do not rescale badge layout; font rasterization can still differ slightly.

Usually, edit the shared `[mode_indicator.ui]` settings; the default file lists font, corner radius, padding, border, and placement together. In the web simulator, open **Global settings → Mode badge** for live editing. Each mode's **Appearance** tab supports optional overrides; reset a field to inherit the shared style again.

Each mode can override the shared `[mode_indicator.ui]` style. Merge these settings into the corresponding existing tables:

```toml
[mode_indicator.modes.normal]
enabled = true
text = "Normal"

[mode_indicator.ui]
indicator_offset = [-12, 18]
font_size = 13
background_color = "#0A1338FF"
text_color = "#E8EEFFFF"
border_radius = 6
padding_x = 8
padding_y = 4

[mode_indicator.modes.text_input]
enabled = false
```

`indicator_offset = [X, Y]` positions the badge's **top-right corner**. Positive X moves right; positive Y moves down. Both values are integers from -32768 to 32767. The default `[-12, 18]` puts that corner 12 left and 18 below the cursor. Drag the orange anchor in the editor, use arrow keys (Shift moves by 10), or enter the array. The whole badge is clamped to display edges.

Individual modes can override these fields in tables such as `[mode_indicator.modes.normal.ui]`. As of 0.10.22, `position`, `indicator_x_offset`, and `indicator_y_offset` are removed. Use `indicator_offset = [X, Y]`; omitted offsets use the default or inherit the global setting. To show the Text Input badge, set `enabled = true` and use the same style fields in `[mode_indicator.modes.text_input.ui]`.
