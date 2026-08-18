# Grid mode

<script setup>
import ModeVideo from '../../.vitepress/components/ModeVideo'
import KeyLayout from '../../.vitepress/components/KeyLayout'
</script>

`Grid` divides the active display into labelled cells.

<ModeVideo file="grid.mp4" title="Grid mode demonstration" description="Large first-level labels, inner second-level previews, and rapid targeting with consecutive key presses." />

Enter it from Normal with `g`. The default layout is 5 columns × 4 rows:

<KeyLayout layout="12345/qwert/asdfg/zxcvb" target="1 2 3 4 5 q w e r t a s d f g z x c v b" label="Grid default keys" />

The initial screen shows a prominent first key at the centre of every top-level cell and a muted, smaller second-key grid within it. The default maximum selection depth is `max_depth = 3`.

Normal bindings do not conflict with Grid labels. Hold `Primary` to temporarily enter Normal, so you can move, scroll, or click without leaving Grid.

## Controls

| Key | Action |
| --- | --- |
| Label key | Select a cell and enter the next level |
| `` ` `` (the key above Tab) | Toggle whether selection follows the pointer |
| `Primary+Q` / `Esc` | Return to Normal |

Grid labels take priority over identically named inherited Normal bindings. For example, `q` selects a Grid cell rather than leaving the mode.

## Configuration

```toml
[grid]
grid_cols = 5
grid_rows = 4
keys = "12345qwertasdfgzxcvb"
max_depth = 3
cursor_follow_selection = true

[grid.ui]
font_size = 20
border_width = 1
# Optional: colours for large initial labels and their inner borders.
# Small labels use text_color with reduced opacity.
# matched_text_color = "#8FA2F0FF"
# matched_border_color = "#8FA2F0FF"

[grid.lifecycle]
after_finish = "normal"
after_click = "finish"
```

`keys` must contain exactly `grid_cols × grid_rows` characters, ordered left-to-right and top-to-bottom. For continuous subdivision of one area, use [Recursive Grid](/en/modes/recursive-grid).
