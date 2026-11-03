# Recursive Grid mode

<script setup>
import ModeVideo from '../../.vitepress/components/ModeVideo'
import KeyLayout from '../../.vitepress/components/KeyLayout'
</script>

`Recursive Grid` repeatedly subdivides the current area, like zooming into a map. It is useful for small icons, small controls, canvas objects, and repeated work within one area.

<ModeVideo file="recursivegrid.mp4" title="Recursive Grid mode demonstration" description="Repeated subdivision, backtracking, and continued targeting of small targets." />

Enter it from Normal with `Primary+G`. The default grid is 3 × 3:

<KeyLayout layout="qwe/asd/zxc" target="q w e a s d z x c" label="Recursive Grid default keys" />

## How it differs from Grid

- Grid normally finishes after a fixed number of levels and is best for reaching an area quickly.
- Recursive Grid keeps its session after a click by default, so you can continue subdividing the selected area.

## Controls

| Key | Action |
| --- | --- |
| Label key | Subdivide the selected cell |
| `U` | Show UI Hint within the current region; `Tab`, `Backspace`, or `Esc` returns to it |
| `I` | Start Text Select within the current region; `Esc` returns to the grid |
| `Backspace` / `Tab` | Go back one level; exit when backing out from the root |
| `Space` | Return to the first level |
| `Enter` | Move to the centre of the current area without clicking |
| `` ` `` (the key above Tab) | Toggle pointer following at each level |
| `Primary` | Temporarily use Normal movement, scrolling, and clicking |
| `Primary+Q` / `Esc` | Return to Normal |

::: tip Take your time with small targets
Use `Tab` to back up a level, or Space to start over. `Enter` only centers the pointer; hold `Primary` to use Normal click keys when ready.
:::

The region scans use UI Hint or Text Select settings. Use `Alt+C` in Text Select to refine and copy part of the text. Rebind `area_hint` and `area_text` in `[recursive_grid.bindings]`; existing labels and inherited user actions are preserved.

## Configuration

```toml
[recursive_grid]
grid_cols = 3
grid_rows = 3
keys = "qweasdzxc"
max_depth = 10
min_size_width = 1
min_size_height = 1
cursor_follow_selection = true

[recursive_grid.lifecycle]
after_finish = "keep"
after_click = "keep"
```

For label-free, one-axis-at-a-time narrowing, use [Bisect](/en/modes/bisect) with `Primary+F` from Normal.
