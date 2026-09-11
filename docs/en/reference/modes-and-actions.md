# Modes and actions reference

Every binding in a configuration has this form:

```toml
key = "action"
```

It may also be an action array:

```toml
key = ["action 1", "action 2", "action 3"]
```

Array entries run in written order. Empty arrays are invalid. Arrays do not block the input thread; `wait` pauses only the current sequence.

## Mode names

| Value | Meaning |
| --- | --- |
| `idle` | Waits for `[hotkeys]` and does not intercept normal input. |
| `normal` | Move, scroll, click, and enter targeting modes. |
| `grid` | Full-screen coordinate grid. |
| `recursive_grid` | Repeatedly subdivides the current region. |
| `ui_hint` | Shows labels for interactive elements. |
| `window` | Locks the window under the pointer for movement, centred resizing, layouts, and tiling. |
| `window_quick` | Quick ratio layout. |
| `window_editor` | Automatic arrangement and region editing. |
| `window_restore` | Restore saved layouts. |
| `plugin:<id>` | A plugin Mode, for example `plugin:screen-selector`. |

```toml
[normal.bindings]
g = "grid"
f = "recursive_grid"
"primary+f" = "ui_hint"
"primary+s" = "screen next"

# Optional: send directional keys to the focused application.
# "primary+h" = "left"
# "primary+j" = "down"
# "primary+k" = "up"
# "primary+l" = "right"
```

## Window mode

Window, Quick, Editor, Restore and Tabs each have two independent settings:

```toml
[window]
screens = "current"       # "current" or "all"
include_minimized = false
```

Use the same keys in `[window_quick]`, `[window_editor]`, `[window_restore]` and `[window_tab]`. Defaults select non-minimized windows on the current display. With `screens = "all"`, numbering includes all displays; automatic layouts and groups stay on each window’s own display. Enabling `include_minimized` also makes minimized windows eligible for selection and layout. Re-entering a fresh Window session numbers the current candidates from 1, releasing old numbers held by closed or excluded windows; refreshes within a session keep surviving numbers stable. Identity cards avoid each other and the bottom help panel.

Window operations use five independently registered modes. Each has its own bindings, inheritance, application overrides, temporary mode and style. H/J/K/L are explicit defaults and do not follow Normal remapping.

| Mode | Default entry | Default Q destination |
| --- | --- | --- |
| `window` | Global Alt+W | `idle` |
| `window_quick` | A in Window | `window` |
| `window_editor` | E in Window | `window` |
| `window_restore` | R in Window | `window` |
| `window_tab` | T in Window | `window` |

Q is an ordinary mode binding and behaves identically regardless of the entry path. Launching Editor directly from Idle still makes its default Q enter Window. Rebinding Q selects another destination; activating the current mode follows the normal toggle-to-Idle rule.

Alt+W locks the window under the pointer. By default, non-minimized windows on the current display receive stable numbers; closing one does not renumber the others. A complete number focuses and locks its window and centers the pointer; only ambiguous prefixes wait, for 250ms by default. Tab / Shift+Tab cycles within the current tab group, or through ordinary windows when outside a group; number selection can leave a group, S switches movement/centered resizing, H/J/K/L adjusts, D changes display, F cycles maximize → minimize → restore, C centers, Z undoes one step, Shift+Z redoes, and Shift+C restores the initial state of the current window session.

Quick adjusts each axis independently with H/J/K/L. The first direction chooses the configured ratio nearest one half; the same direction shrinks and the opposite expands. `window_quick.split_ratios` defaults to 1/4, 1/3, 1/2, 2/3 and 3/4, with full size appended automatically. Tab changes target and resets ratios; Z undoes.

Editor automatically arranges windows on entry. H/J/K/L navigates regions, Shift+direction splits, the default Ctrl+H/L shrinks/grows the selected region's width and Ctrl+K/J shrinks/grows its height using Editor's own `resize_step`/`resize_speed`. Neighboring regions adjust to preserve tiling. All keys are configurable. X removes a region and Z undoes. Select two window numbers to swap them, or use backtick followed by a region number to select or fill an empty region. Changes apply immediately. Pending transactions finish before handoff; window modes preserve the target and stable numbers. New splits reuse the lowest available region number without renumbering survivors.

Ctrl+S in Editor opens the bottom note field and saves a layout. Notes are optional and limited to 80 characters. In Restore, type a layout number; PageUp/PageDown changes pages. Only successful application triggers `window_restore.lifecycle.after_finish`, which defaults to Editor. Failure stays in Restore with an error. Layouts store geometry, window count and notes without application or native window identity. Restoration fills regions in recent-activity order, leaves extra windows untouched and retains empty regions when there are fewer windows.

X toggles restore/delete inside Restore, preserving the page and cancelling any pending choice. In delete state, type a number to see the selected name, then Enter to delete. X returns to restore; Q follows Restore’s binding back to Window. Deletion stays in delete state, refreshes pages and preserves all other IDs. Deleting the final item writes a valid empty library. The record is reread before submission; external changes require selection again, and failed writes preserve the original file.

Layouts and Tabs templates share `workspace.ksw` in the same application-data directory as logs. Default names are `Layout N` and `Tabs N`; a nonempty note becomes the name. Both types share saving, the Restore list and deletion. It interoperates with the [web simulator](/en/simulator). Browser edits affect demo windows and browser storage; download and replace the program’s workspace file, then enter Restore to read it.

### T: persistent tab groups

Initial numbering keeps windows from the same application together where possible; later refreshes preserve existing numbers. Minimizing the active application window minimizes the whole group. Restoring a member shows that member while the others remain tucked away.

The strip reserves space at the top of the group's layout rectangle, including maximized and tiled layouts. Alt+W lists every member's number, application and title, with the active member marked. On Windows, vertical or horizontal scrolling browses overflowing tabs; selecting a member scrolls it into view.

On Windows, drag tabs even after leaving Window mode: reorder within a strip or move a window onto another group's strip. Drag the `~group number` to merge the whole group. An insertion line shows the destination; release outside a strip or right-click to cancel. Strips follow their application's stacking order. Switching preserves window content where supported to reduce redisplay flicker; applications that already use layered drawing retain the compatible show/hide path.

Windows and macOS share the same grouping workflow. Alt+W then T groups compatible ungrouped windows from the same application within each selected display, preserving existing groups. Each group shows its active member with an independent clickable tab strip. Groups persist after leaving the mode until dissolved or KeySteer exits.

New groups use the smallest available group number without renumbering existing groups; after all groups dissolve, numbering starts at `~1` again. Tab strips remain visible when another application gains focus. On Windows, native window-position events move the strip directly, without a fixed polling interval or display-frame schedule.

Digits always identify individual windows. Groups use `~1`, `~2`, and so on. The first item establishes the starting window or group; the second immediately groups, and later items append. T ends this batch and starts the next; no Enter is required and T creates no undo record.

| Input | Result |
| --- | --- |
| `12t34t` | Group windows 1–2, then 3–4, when those numbers are unambiguous |
| `123t` | Group windows 1, 2 and 3 |
| `~1` | Select group 1 |
| `~1 4t` | Append window 4 to group 1 |
| `~1 ~2t` | Merge all of group 2 into group 1, retaining group 1's number |
| `1 Space 2 t` / `12 Space t` | Explicitly choose windows 1 and 2 / window 12 |

The configurable `~` action immediately highlights group numbers; a complete group number returns to window input. Existing ambiguous multi-digit parsing is preserved; Space explicitly ends a number, and T processes a pending valid number before ending the batch. Invalid numbers explain the failure without redirecting the target. A window number transfers only that member from its previous group; a group number transfers the entire group. Selecting an existing target member only activates it.

| Key | Operation |
| --- | --- |
| T | End this batch; an empty batch or single independent window only clears selection |
| D / X | Remove the active member, retaining the old group target / dissolve and end the batch |
| Tab / Shift+Tab | Next / previous tab |
| H / L (`move_left` / `move_right`) | Reorder the active tab |
| K / J (`move_up` / `move_down`) | Previous / next tab |
| Z / Shift+Z | Undo / redo membership and order changes |
| Ctrl+S | Save a Tab template and optional note |
| Q / Esc | Return to Window / exit, preserving completed groups |

All bindings live in `[window_tab.bindings]`, including `window_tab_group` for the prefix and `window_number_end` for the separator. Grouping edits apply inside T. Window mode retains number selection, Tab for next and Shift+Tab for previous across ordinary windows and every group member. Configure these as `window_select` / `window_select_previous` in `[window.bindings]`. Outside Window mode, application Tab and number input remains untouched; mouse tab switching is still available.

Applications remain independent windows. Only the active member is shown; dragging and resizing move that window alone. Selecting another tab first aligns the new member to the current group position, then shows it. Window/Quick/Editor layouts treat a group as one target. Closing the tab strip dissolves the group and reveals its members. Closing an application removes that member and selects an adjacent one; a single survivor becomes an ordinary window. Window history restores group geometry, while T has separate membership/order history.

Tab templates appear in the existing Restore/Delete list with a Tabs label. They store member count, selection order, active position, normalized area and note, without native window identities. Restoring enters T to collect windows in order and applies automatically when the count is reached. Incomplete selection changes no windows and Q/Esc cancels it. Deleting a template leaves running groups intact.

Supported members are ordinary resizable windows on the current desktop; native fullscreen and incompatible windows are rejected. Windows hides inactive members without changing their parent or embedding styles. macOS uses per-window minimization through Accessibility, so system minimize/restore animations may still appear. Dissolving or normal shutdown reveals the windows hidden by KeySteer. Tab templates are available on both platforms.

## Movement, scrolling, and speed

| Action | Description |
| --- | --- |
| `move_left`, `move_down`, `move_up`, `move_right` | Move continuously while held; a tap also moves a short distance. |
| `scroll_left`, `scroll_right`, `scroll_up`, `scroll_down` | Scroll by `[scroll].scroll_step`. |
| `scroll_half_*` | Scroll by `scroll_step_half`. |
| `scroll_full_*` | Scroll by `scroll_step_full`. |
| `precision`, `slow`, `fast` | Change pointer speed while held. |
| `precision_toggle`, `slow_toggle`, `fast_toggle` | Toggle a speed with a tap; tap the same binding again to clear it. The active speed appears below the mode indicator. |
| `follow` | Toggle pointer following in Grid or Recursive Grid. |

`wheel_*` remains a compatibility alias for `scroll_*`. Speed actions are usually paired with a movement key:

```toml
[normal.bindings]
h = "move_left"
"v b" = "fast"
```

`"v b"` binds two independent keys to the same action, not a sequence. Use `+` for a chord.

## Pointer buttons and dragging

| Action | Description |
| --- | --- |
| `left_click`, `right_click`, `middle_click`, `mouse_x1`, `mouse_x2` | With long-press detection enabled, send MouseDown immediately and MouseUp on a short release; at the threshold, only latch the existing press. With it disabled, inject an atomic click on key-down. |
| `double_click` | Immediately starts the first left-button press, completes the double-click on a short release, or latches the left button on a long press. |
| `left_press`, `right_press` | Hold a pointer button. |
| `left_release`, `right_release` | Release a held pointer button. |
| `toggle_left`, `toggle_right` | Toggle the held state of that pointer button. |
| `toggle` | Without parameters, latch each companion's effective Normal keyboard or mouse target regardless of which key was pressed first. A short standalone tap, returning to Normal, or entering Idle releases all latches; a long standalone hold latches its activating key. |
| `press <target...>` | Hold one or more keys or pointer buttons. |
| `release <target...>` | Release targets that were previously held. |
| `toggle <target...>` | Toggle the state of targets. |

Targets are key names or `mouse_left`, `mouse_right`, and `mouse_middle`, `mouse_x1`, `mouse_x2`:

```toml
[normal.bindings]
n = "toggle"
x = ["press shift", "left_click", "release shift"]
```

## Send a key

A bare key name sends that key to the focused application. You can write a chord directly with `+` or use the explicit `send` form:

```toml
[normal.bindings]
t = "home"
"primary+shift+s" = "send primary+shift+s"
```

`send` must be followed by a valid key or chord. It injects input into the current application and does not switch KeySteer modes.

## Run an external command: `exec`

Use `exec` to connect KeySteer to scripts, launchers, or other desktop tools. It starts a program but does not wait for it to complete or show its output in KeySteer.

```toml
[normal.bindings]
"primary+shift+t" = "exec open -a Terminal"
"primary+shift+b" = ["exec say build-started", "wait 500", "exec open ."]
```

Syntax:

```text
exec <program> [arg1] [arg2] ...
```

The first word is the program and every subsequent word is a separate argument. KeySteer invokes Rust's process API directly, not a shell: it does not expand `~`, environment variables, pipes, redirections, or `&&`.

For shell syntax, put the logic in a script and execute that script directly. On Windows you can explicitly use `cmd`:

```toml
# Execute a script or program whose path contains no spaces.
x = "exec /usr/local/bin/keysteer-script"

# Windows: arguments are split on spaces.
x = "exec cmd /C start notepad"
```

Configuration values are split on spaces and do not offer quote-escaping. Use a script or a wrapper program without spaces for paths or complex arguments. Commands are detached; KeySteer does not wait for an exit status or display stdout/stderr. A failed launch is logged.

## Plugin verbs and arguments

Plugins can register verbs in their manifest. Write arguments directly after the verb:

```toml
[normal.bindings]
"primary+s" = "screen next"
"primary+1" = "screen 1"
"primary+shift+s" = "call screen"
```

- `screen next` calls the `screen` plugin verb with `next`.
- `screen 1` calls the same verb with `1`.
- `call screen` explicitly calls a parameterless verb.

Explicit `call` is useful for a no-argument invocation or to avoid ambiguity. An unknown lowercase verb with parameters is treated as a plugin call. A misspelled built-in action fails when the configuration loads rather than silently sending a key.

## Move a window between displays

The bundled Window Mover plugin exports `move_window next`, `move_window previous` (or `prev`), and numbered destinations such as `move_window 2`.
It targets the application window under the physical pointer when invoked. The pointer follows the window, keeping its position within it; focus and the active mode remain unchanged. If a maximized window changes size, the pointer keeps its proportional position. The pointer stays within the destination display for oversized or partly off-screen windows.
Display numbers follow the `screen` plugin. One display or no movable window is a no-op.

Equal display dimensions preserve the offset and window size, even with different taskbar layouts. Different dimensions map the fraction of available travel within work areas and keep the window accessible where possible.
Windows maximized windows move directly while retaining their state, without a restore/maximize cycle, and receive a migrated restore position. macOS native-fullscreen windows automatically exit full screen, move, and return to full screen using the system animations. The pointer follows when the transition completes. Applications may restrict movement or fullscreen changes.
Applications can impose their own position or DPI size constraints. The default binding is `Primary+S+D → move_window next`; releasing `Primary+S` still only switches the pointer display. Both bindings are configurable in any binding table.

## Other actions

| Action | Parameters and effect |
| --- | --- |
| `move_mouse <x> <y>` | Move to absolute desktop coordinates; requires two integers. |
| `wait` or `wait 0` | Wait the default `100ms`. |
| `wait <max_ms>` | Wait a random time between 0 and the limit. |
| `wait <min_ms> <max_ms>` | Wait a random time in that range; maximum is `86400000ms`. |
| `finish` | Finish the current targeting session. |
| `restart_mode` | Clear and restart the current targeting session. |
| `rescan` | Rescan UI Hint. |
| `escape` | Leave the current mode and return to Idle. |
| `reload_config` | Reload configuration from disk. |
| `set_config <path> <TOML value>` | Edit and persist a dotted path, for example `set_config pointer.max_speed 800`. |
| `quit` | Exit KeySteer. |
| `none` | Disable the binding, usually to block an inherited key. |

`set_config` values must be valid TOML. Quote strings; arrays and tables can be passed directly. It edits only the currently loaded configuration file. If KeySteer is using built-in defaults, specify a file with `--config` first:

```toml
[normal.bindings]
"primary+1" = "set_config pointer.max_speed 800"
"primary+2" = "set_config general.excluded_apps [\"com.example.App\"]"
```

The new value is parsed and validated before being written. On failure, the last valid configuration remains active.

## Binding parsing order

The right-hand side is parsed in this order:

1. `none` / `__disabled__`.
2. Explicit actions: `call`, `send`, `exec`, `move_mouse`, `set_config`, `press`, `release`, `toggle`, `wait`.
3. Built-in actions such as `move_left`, `left_click`, `fast`, and `finish`.
4. Plugin verbs with parameters.
5. A `+` chord or known bare key, sent to the focused application.
6. A built-in Mode name or namespaced plugin Mode name.

See the [default configuration](/generated/keysteer.default.toml) for the complete shipped example.

## `key_help`

Toggle the available-key panel without restarting the active mode or clearing its selection. Enable with `"?" = "key_help"` in Normal, inherited by targeting modes. Omit or comment out the entry to disable it. Configure its appearance in `[key_help]`.

Window cards emphasize application names above longer window titles and use thicker leader lines when displaced. Editing uses a shaded grid with region numbers centered in their regions. The help panel keeps the configured font size and falls back to the screen when the target is too small. Move / Resize / Quick / Edit appear as separate keycap badges; the application and window title each get their own line. Backtick plus a region number also opens the tree editor from ordinary Window or Quick, selects that region, and activates its window; empty regions move the pointer to their center.

The default is `f = "size_cycle"`. After minimizing, press F to restore the same window to its original position and size, then press F again to repeat.

Window, Quick, and Editor bind Z to `window_undo`, Shift+Z to `window_redo`, and Shift+C to `window_reset_initial`. C remains `window_center` in Window. Rebind these actions in each mode’s existing bindings table while keeping other bindings. A new adjustment clears redo. Continuous movement/resizing is one undo group; edits undo step by step while editing, then become one group on leaving the editor.

Reset restores positions, sizes, and maximized/minimized states of windows modified in the current window-mode session. Its baseline survives switches among the six window modes and is renewed after leaving the group and entering again. Reset itself is undoable and redoable. It does not reopen closed windows, restore application content, or modify untouched windows. The initial baseline is retained independently of the 32-group history limit.

## UI Hint scan scope

```toml
[ui_hint]
scan_scope = "screen" # window: pointer window (default); screen: complete pointer display
```

Works with Hybrid, Accessibility Tree, and Vision. Screen scope covers visible content on the display
containing the pointer. Moving to another display, including a custom `Alt+S` binding to `screen next`,
clears old hints and starts a new scan automatically. `Primary+R` manually refreshes the scan.

Entering ordinary Window mode (Alt+W by default) activates and raises the window under the pointer without moving the pointer. X invokes `window_close`, requesting normal closure of the selected window; the application handles save/cancel dialogs. Rebind it in `[window.bindings]`, for example `"f9" = "window_close"`, and remove or disable the original X binding. X in the layout editor still deletes a region.

For a tab group in ordinary Window mode, X closes only its active member. Once closure is confirmed, a group with one remaining member dissolves automatically and reveals that window. A/E/T and other modes retain their own X bindings.
