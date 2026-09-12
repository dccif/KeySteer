# Window: arrange your workspace from the keyboard

For separate mode pages and videos, start with the [Window management overview](/en/window-management/).

Put your browser on the left and your editor on the right, collect scattered windows into tab groups, and save layouts you use every day. Start with moving one window, then build a workspace that suits your work.

::: tip Try moving a window first
Place the pointer over an ordinary window → press `Alt+W` → release the entry keys → move with `H/J/K/L` → press `Q` to finish.

This page uses shipped defaults. `Alt+W` means Alt on Windows and Option on macOS, **not Command+W**. `Primary` defaults to left Alt on Windows and Command on macOS. Capital letters make keys easier to read; only hold Shift where `Shift+` is written.
:::

## Choose what you want to do

| Your goal | From Window, press | Next step |
| --- | --- | --- |
| Move, resize, centre, or change displays | Stay in Window | Adjust with direction keys; `S` toggles resizing |
| Place one window on half the screen or in a corner | `A` → Quick | Choose a direction and proportion |
| Arrange several windows and fine-tune regions | `E` → Editor | Start with automatic arrangement, then edit |
| Collect windows into switchable tab groups | `T` → Tabs | Group windows from the same app automatically, or combine them yourself |
| Restore a saved layout or tab template | `R` → Restore | Type its number in the list |

In Quick, Editor, Tabs, or Restore, `Q` returns to Window; another `Q` returns to Idle. `Primary+Q` goes directly to Idle. Leaving a mode keeps applied changes; use undo to revert them.

## Move and select windows

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

## Recipe one: browser and editor side by side

1. Place the pointer over your browser, press `Alt+W`, release the entry keys, then press `A` for Quick.
2. Press `H` to place the browser on the left half.
3. Type your editor's window number, then press `L` to place it on the right half.
4. Press `Q` to return to Window, then `Q` again to start working.

Quick adjusts each axis independently. The first direction starts with the configured proportion nearest half a screen; repeating the direction shrinks it, while the opposite direction expands it. For example, press `K` after choosing the left half to place the window in the upper-left area. Default proportions are `1/4, 1/3, 1/2, 2/3, 3/4`, plus an automatically included full-screen proportion. Press `Z` to undo.

## Recipe two: arrange everything, then give your main window more room

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

## Recipe three: turn scattered windows into tab groups

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

## Save and restore a workspace

Press `Ctrl+S` in Editor or Tabs, type a note in the bottom field, and press Enter to save. Notes are optional and limited to 80 characters. Try names such as “Coding” or “Reading and notes”.

Press `R` from Window to open the shared preset list, then type a preset number. Use `PageUp / PageDown` to browse six entries per page.

| Template type | What restoring does |
| --- | --- |
| Layout | Fills saved regions with current windows; on success, enters Editor by default for further adjustment |
| Tabs | Enters Tabs so you can choose the required windows in order; applies when all are selected, and can be cancelled before then |

**Templates do not save application identities, launch apps, or reopen documents.** Layout restoration uses the current windows' recent activity order. Extra windows stay where they are; missing windows leave empty regions. For tab templates, you choose the members again.

To delete a preset, press `X` in Restore to switch to deletion, type its number, check the displayed name, and press `Enter`. Press `X` again to return to restoration. Deleting a template does not affect an existing tab group.

Layouts and tab templates share `workspace.ksw`: beside the executable on Windows portable builds, or in `~/Library/Application Support/KeySteer/` for the macOS app. Use the [Configuration & simulator](/en/editor/) to import, practise, and export. The website only changes sample windows and browser storage. Replace the file in the application's data directory with your export, then reopen Restore; back up the original file before replacing it if needed.

## Adjust audio without leaving your layout

These default combinations work in Window, Quick, and Editor. Hold `V` and press the partner key; add `Shift` for system controls.

| Scope | Volume down / up | Toggle mute | Previous / next output device |
| --- | --- | --- | --- |
| Application owning the target window | `V+J / V+K` | `V+M` | `V+H / V+L` |
| System | `Shift+V+J / Shift+V+K` | `Shift+V+M` | `Shift+V+H / Shift+V+L` |

Volume changes by 1% per step. Raising application volume does not automatically unmute it. Multiple windows from the same app may share an audio session.

- **Windows:** application volume needs an available audio session; start playback if none is found. Whether a device change moves an existing stream immediately depends on the application.
- **macOS:** system audio works on macOS 14+; independent application audio needs macOS 14.2+ and System Audio Recording permission. Local processing adds audio latency. Some output devices do not support software volume control.

## Troubleshooting

| What you see | What to try |
| --- | --- |
| A window has no number | Check its display, minimized state, and compatibility; candidate scope is configurable independently for each mode |
| Leaving does not restore the previous layout | Leaving keeps changes; use `Z`, or `Shift+C` in Window/Quick/Editor to restore this session's initial state |
| `X` does something unexpected | Window closes a window, Editor removes a region, Tabs dissolves a group, and Restore toggles deletion; check the current mode hint |
| New shortcuts do not work after upgrading | An explicit `[mode.bindings]` replaces that mode's default table; add new bindings from the latest defaults |

Continue with [Window configuration](/en/reference/configuration#window-configuration), the [action reference](/en/reference/modes-and-actions#window-mode), or [macOS installation and permissions](/en/guide/macos).
