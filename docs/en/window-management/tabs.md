# Tabs: group windows

<script setup>
import ModeVideo from '../../.vitepress/components/ModeVideo'
</script>

Enter with T to group compatible windows by application.

<ModeVideo file="tabs.mp4" title="Tabs: group windows" description="Enter with T to group compatible windows by application." />

## How to use it

Press `T` from Window to group compatible, ungrouped windows from the same application separately on each display, preserving existing groups. Press `Z` to undo this automatic grouping.

You can also combine windows from different applications:

1. Choose the first displayed window number, for example `1`, then press Space to end the number.
2. Type the second window number, for example `2`, then Space. They form a group immediately.
3. Add more numbers to append members. Press `T` to finish this group and prepare another.
4. Press `Q` to return to Window. Groups remain after leaving the mode; click a tab to switch members.

The numbers `1` and `2` are examples: use the numbers on your screen. Space makes the boundary explicit so windows 1 and 2 are not interpreted as window 12.

| Key | Action in Tabs |
| --- | --- |
| `Tab / Shift+Tab` or `J / K` | Next / previous tab |
| `H / L` | Reorder the active tab left / right |
| `~`, then a group number | Select a whole group, such as `~1` |
| `D` | Remove the active member from its group |
| `X` | Dissolve the group without closing applications |
| `Z / Shift+Z` | Undo / redo grouping and ordering |
| `Ctrl+S` | Save a tab template |

Only the active member is shown. Quick and Editor treat a group as one layout target. Minimizing the active member minimizes the group; dissolving it or quitting KeySteer normally restores members hidden by KeySteer. Groups last until dissolved or KeySteer exits; save a template to reuse them later.

On Windows, you can also drag tabs after leaving the mode: reorder them, move a window to another strip, or drag `~group number` to merge a whole group. Scroll the strip when there are many tabs. macOS uses system minimize/restore operations to switch members, so system animations may appear. Native fullscreen and incompatible windows report that grouping is unsupported.

For saving instructions and a demo, see [Save an arrangement](/en/window-management/restore#save).

[Window management overview](/en/window-management/) · [Full configuration reference](/en/reference/configuration)
