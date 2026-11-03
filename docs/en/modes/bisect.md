# Bisect

<script setup>
import ModeVideo from '../../.vitepress/components/ModeVideo'
</script>

Bisect is recursive positioning without letter labels. Press `Primary+F` from Normal, then use four direction keys to keep half of the current region. The pointer follows the selected region's center by default.

<ModeVideo
  file="bisect-en.mp4"
  title="Bisect mode demonstration"
  description="Halve the target area with H/J/K/L, then press Enter to position the pointer without clicking."
/>

| Key | Action |
| --- | --- |
| `H` / `L` | Keep the left/right half, halving width |
| `K` / `J` | Keep the top/bottom half, halving height |
| `U` | Show UI Hint within the current region; `Tab`, `Backspace`, or `Esc` returns to it |
| `I` | Start Text Select within the current region; `Esc` returns to that region |
| `Tab` / `Backspace` | Undo one cut; return to the previous mode at the root |
| `Space` | Reset to the whole screen |
| `Enter` | Move to the region's center and finish selection; no click by default |
| `` ` `` | Toggle cursor following |
| `Primary` | Temporarily borrow Normal movement, scrolling and clicks (enabled in the shipped config) |
| `Esc` | Return to the previous mode |
| `Primary+Q` | Return to Normal; restore the window session when borrowed from Window |

Two center guides mark the horizontal and vertical halves. Each key chooses only one half: `L → J → H` keeps the right half of the screen, its bottom half, then that region's left half. There is no label alphabet, configurable row/column count, or quadrant action. Direction keys can be rebound independently.

Region scans use UI Hint or Text Select settings. Use `Alt+C` in Text Select to refine and copy part of the text. Rebind `area_hint` and `area_text` in `[bisect.bindings]`; existing inherited user actions are preserved.

## Shared Recursive Grid behavior

Region fill, outline and terminal background use Recursive Grid styling, configured independently in `[bisect.ui]`. Clicking keeps the current region by default so you can continue cutting. After confirming or reaching a limit, undo a level or reset to select again. With `preserve` enabled, the screen selector replays the directional path on the new screen.

Defaults allow 20 cuts and a minimum width/height of 2 coordinate units. When width cannot shrink further, vertical cuts remain available, and vice versa. Reaching the depth limit or exhausting both axes invokes `after_finish` (`keep` by default).

## Independent configuration

```toml
[normal.bindings]
"primary+f" = "bisect"

[bisect]
max_depth = 20
min_size_width = 2
min_size_height = 2
cursor_follow_selection = true
inherits = ["hotkeys", "normal"]
temporary_mode = "normal"
temporary_mode_keys = ["primary"]

[bisect.bindings]
h = "bisect_left"
j = "bisect_down"
k = "bisect_up"
l = "bisect_right"

[bisect.lifecycle]
after_finish = "keep"
after_click = "keep"

[bisect.ui]
line_width = 1
```

To use arrow keys, set `h = "none"` and `arrow_left = "bisect_left"`, then repeat for the other directions. A temporary Normal layer consumes its activation keys. To enter Bisect directly with `Primary+F` from another mode, explicitly add that chord to the source mode's `bindings`.

See the [configuration reference](/en/reference/configuration#bisect) for more options.
