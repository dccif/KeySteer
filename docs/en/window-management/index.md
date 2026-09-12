# Window management overview

<script setup>
import ModeVideo from '../../.vitepress/components/ModeVideo'
</script>

Start with one window, then build a reusable workspace. Window and its four default submodes have independent, configurable bindings.

## Try it once

Pointer over a window → `Alt+W` → release the entry keys → `H/J/K/L` → `Q`. On macOS use Option+W, not Command+W.

| Task | Mode | From Window |
| --- | --- | --- |
| Move, resize, centre, cycle window states and control audio. | [Window](/en/window-management/window) | — |
| Choose placement and ratios with direction keys. | [Quick](/en/window-management/quick) | `A` |
| Tile immediately; swap numbered windows and adjust regions. | [Editor](/en/window-management/editor) | `E` |
| Group compatible windows by application. | [Tabs](/en/window-management/tabs) | `T` |
| Save an arrangement, then fill it with the windows you need today. | [Restore](/en/window-management/restore) | `R` |

<ModeVideo file="window.mp4" title="Window demonstration" description="Keys and the current action appear below the simulator." />

## Shared controls and troubleshooting

| What you see | What to try |
| --- | --- |
| A window has no number | Check its display, minimized state, and compatibility; candidate scope is configurable independently for each mode |
| Leaving does not restore the previous layout | Leaving keeps changes; use `Z`, or `Shift+C` in Window/Quick/Editor to restore this session's initial state |
| `X` does something unexpected | Window closes a window, Editor removes a region, Tabs dissolves a group, and Restore toggles deletion; check the current mode hint |
| New shortcuts do not work after upgrading | An explicit `[mode.bindings]` replaces that mode's default table; add new bindings from the latest defaults |

Continue with [Window configuration](/en/reference/configuration#window-configuration), the [action reference](/en/reference/modes-and-actions#window-mode), or [macOS installation and permissions](/en/guide/macos).
