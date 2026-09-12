# Window: move and control windows

<script setup>
import ModeVideo from '../../.vitepress/components/ModeVideo'
</script>

Move, resize, centre, cycle window states and control audio.

<ModeVideo file="window.mp4" title="Window: move and control windows" description="Move, resize, centre, cycle window states and control audio." />

## How to use it

Entering Window activates and locks the window under the pointer. Moving the pointer elsewhere does not automatically change the target. Type a displayed window number to activate that window and move the pointer to its centre.

| Key | Action in Window |
| --- | --- |
| `H / J / K / L` | Move left / down / up / right; hold for continuous adjustment |
| `S` | Toggle movement and resizing around the centre |
| `C` | Centre the window |
| `D` | Move to the next display |
| `F` | Cycle maximize → minimize → restore |
| Number | Select a window by number |
| `Tab / Shift+Tab` | Next / previous window; prefer members of the current tab group |
| `X` | Request that the window close; the app may ask you to save |
| `Z / Shift+Z` | Undo / redo window adjustments |
| `Shift+C` | Restore positions, sizes, and window states from the start of this window session |

By default, candidates are **non-minimized windows on the current display**. Existing numbers stay stable where possible: closing a window does not renumber the others, although re-entering may reclaim gaps. Only a prefix that could form a longer number waits, for 250 ms by default.

Need to click something? Hold `Primary` to temporarily use Normal's pointer movement, scrolling, and click bindings. Release it to resume the same target. Each window mode has independent direction bindings; remapping Normal does not automatically remap these modes.

## Audio controls

These default combinations work in Window, Quick, and Editor. Hold `V` and press the partner key; add `Shift` for system controls.

| Scope | Volume down / up | Toggle mute | Previous / next output device |
| --- | --- | --- | --- |
| Application owning the target window | `V+J / V+K` | `V+M` | `V+H / V+L` |
| System | `Shift+V+J / Shift+V+K` | `Shift+V+M` | `Shift+V+H / Shift+V+L` |

Volume changes by 1% per step. Raising application volume does not automatically unmute it. Multiple windows from the same app may share an audio session.

- **Windows:** application volume needs an available audio session; start playback if none is found. Whether a device change moves an existing stream immediately depends on the application.
- **macOS:** system audio works on macOS 14+; independent application audio needs macOS 14.2+ and System Audio Recording permission. Local processing adds audio latency. Some output devices do not support software volume control.

[Window management overview](/en/window-management/) · [Full configuration reference](/en/reference/configuration)
