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

`n = "toggle"` controls whether paired input stays pressed. It works with any key or modifier, including `Shift`, `alt`, and `ctrl`.

Hold a key bound to a mouse button for `long_press_toggle_ms` to latch that mouse button down; tap `n = "toggle"` to release it.

```toml
[normal]
passthrough_unbound_keys = true
long_press_toggle_ms = 500 # Set 0 to disable.
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
