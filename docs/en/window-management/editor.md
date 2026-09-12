# Editor: tile and edit regions

<script setup>
import ModeVideo from '../../.vitepress/components/ModeVideo'
</script>

Enter with E to tile immediately; swap numbered windows and adjust regions.

<ModeVideo file="editor.mp4" title="Editor: tile and edit regions" description="Enter with E to tile immediately; swap numbered windows and adjust regions." />

## How to use it

Press `Alt+W`, then `E` for Editor. **Entering immediately attempts to arrange candidate windows.** Layouts respect application minimum sizes; there is no Enter-to-apply step.

| Key | Action in Editor |
| --- | --- |
| `H / J / K / L` | Select the region to the left / below / above / right |
| `Shift+H/J/K/L` | Split in the chosen direction |
| `Ctrl+H / Ctrl+L` | Decrease / increase the selected region's width |
| `Ctrl+K / Ctrl+J` | Decrease / increase the selected region's height |
| Two window numbers | Select two windows in sequence to swap their positions |
| Backtick (&#96;), then a region number | Select a region; with a source window selected, move it into an empty region |
| `X` | Remove a region without closing the application window |
| `Z / Shift+Z` | Undo / redo edits |
| `Ctrl+S` | Save the layout |

For example, select your editor's region with the direction keys, then press `Ctrl+L` to widen it. Neighbouring regions adjust together to keep the layout tiled. If an application's minimum size prevents an adjustment, read the on-screen message and try another region or fewer splits. `Z` steps back through edits, including the automatic arrangement on entry.

For saving instructions and a demo, see [Save an arrangement](/en/window-management/restore#save).

[Window management overview](/en/window-management/) · [Full configuration reference](/en/reference/configuration)
