# Modes and actions reference

Use this page to look up action names and arguments when editing a binding. For your first remapping, start with [Configuration](/en/reference/configuration#minimal-configuration).

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
| `bisect` | Keep a directional half of the region, without letter labels. |
| `text_select` | OCR text selection: regions → words → characters. |
| `ui_hint` | Shows labels for interactive elements. |
| `window` | Locks the window under the pointer for movement, centred resizing, layouts, and tiling. |
| `window_quick` | Quick ratio layout. |
| `window_editor` | Automatic arrangement and region editing. |
| `window_restore` | Restore a layout or tab template. |
| `window_tab` | Combine windows into switchable tab groups. |
| `plugin:<id>` | A plugin Mode, for example `plugin:screen-selector`. |

```toml
[normal.bindings]
g = "grid"
f = "ui_hint"
"primary+g" = "recursive_grid"
"primary+f" = "bisect"
"primary+s" = "screen next"

# Optional: send directional keys to the focused application.
# "primary+h" = "left"
# "primary+j" = "down"
# "primary+k" = "up"
# "primary+l" = "right"
```

## Window mode

For a first exercise, see the [Window guide](/en/modes/window). This section covers actions and configurable behavior.

| Mode | Default entry | Default Q destination |
| --- | --- | --- |
| `window` | Global `Alt+W` (Option+W on macOS) | `idle` |
| `window_quick` | Window's `A` | `window` |
| `window_editor` | Window's `E` | `window` |
| `window_restore` | Window's `R` | `window` |
| `window_tab` | Window's `T` | `window` |

Each mode has its own keys, inheritance, app exceptions, and style. Q follows its binding regardless of entry path: entering Editor directly still makes Q return to Window. Triggering the current mode's entry again returns to Idle.

### Choose windows and scope

```toml
[window]
screens = "all"           # "current" or "all" (default)
include_minimized = false
```

| Setting | Window default | Other four modes |
| --- | --- | --- |
| `screens` | `"all"` displays | `"current"` display |
| `include_minimized` | false | false |

Enable `include_minimized` to include minimized windows. Automatic tiling and grouping still happen separately on each display. Numbers stay stable within a session and are rebuilt from current candidates on re-entry. Cards avoid other numbers and the help panel.

Alt+W defaults to the window under the pointer; moving the pointer afterward does not change the target. A complete number selects a window and centers the pointer. Only a possible multi-digit prefix waits, for 250 ms by default.

| Default key | Action | Purpose |
| --- | --- | --- |
| `H/J/K/L` | `window_left/down/up/right` | Move; S changes to resizing around the center |
| `S` | `window_size` | Toggle movement / resizing |
| `D` | `window_screen_next` | Move to the next display |
| `F` / `Shift+F` | `window_maximize` / `window_minimize` | Maximize / restore; minimize / restore |
| `C` | `window_center` | Center |
| `Tab` / `Shift+Tab` | `window_select` / `window_select_previous` | Next / previous; prefer tabs in the current group |
| `X` | `window_close` | Request close; the app may ask you to save |
| `Ctrl` / `Ctrl+X` | `window_multi_select` / `window_multi_clear` | Start / clear multi-selection |

Confirm multi-selection with Ctrl or Enter, or move/resize directly. `E` arranges only the selection; `T` combines it into a group. See [Select multiple windows](/en/window-management/window#select-multiple-windows). X closes only the current tab member; a one-member group dissolves automatically. Editor's X still removes a region.

### Quick, Editor, and presets

| Mode | Main operations |
| --- | --- |
| Quick | H/J/K/L adjusts proportions: repeat a direction to shrink, reverse it to expand; axes are independent. Start nearest half-screen, with full-screen included automatically. Tab changes windows and resets proportions |
| Editor | Tiles on entry; directions select, Shift+direction splits, Ctrl+H/L changes width, Ctrl+K/J changes height; neighbors adjust together |
| Editor selection | Two window numbers swap positions. Backtick plus a region number selects a region or moves into an empty one. New regions reuse free numbers without renumbering others |
| Save | Ctrl+S in Editor/Tabs, optional note, Enter; up to 80 characters, or Layout N / Tabs N when empty |
| Restore | Type a preset number; PageUp/PageDown changes pages. Successful layouts enter Editor by default; failures stay in the list with a reason |
| Delete | X in Restore toggles deletion; select a number, check its name, Enter to confirm. X returns to restoration |

Changes apply immediately and remain on exit. Layouts store regions, window counts, and notes. Restoration fills them with current windows in recent activity order; extra windows stay put and missing windows leave empty regions. Apps are not launched and documents are not reopened.

Layouts and tab templates share `workspace.ksw` in the data directory. Exchange files with the [simulator](/en/simulator), replace the file in the data directory, then re-enter Restore. Deleting a preset does not change existing windows or groups. Failed saves preserve the original file; externally changed records must be selected again.

### Undo, redo, and reset

| Default key | Action | Effect |
| --- | --- | --- |
| `Z` | `window_undo` | Undo one step; continuous movement/resizing counts as one group |
| `Shift+Z` | `window_redo` | Redo; a new change clears redo history |
| `Shift+C` | `window_reset_initial` | Restore adjusted windows' positions, sizes, and states from session start |

All three are available by default in Window, Quick, and Editor. Switching between window modes preserves the starting point. Leaving the group for Idle or another mode and re-entering creates a new one.

Ordinary adjustments retain the last 32 history groups. Editor supports individual undo while open and counts the whole editing round as one group afterward. Initial states are not limited by the 32 groups, and resetting can itself be undone. Undo does not reopen closed windows or restore application content.

### Grid targeting

Window's grid entries are independent of Normal:

```toml
[window.bindings]
g = "grid"
"primary+f" = "recursive_grid"
```

G opens Grid; bare F maximizes/restores. Window explicitly binds Primary+F to Recursive Grid, and this binding remains available while the temporary layer is active. Remap them or set `none` to disable; existing custom tables need the entries added. Other window modes do not have these two default entries. Holding Primary also follows temporary-layer rules.

### T: persistent tab groups

Press T from Window: a multi-selection becomes one group, including entire existing groups when their members are selected. Without multi-selection, compatible ungrouped windows are grouped by app on each display. Existing groups remain; Z undoes the arrangement.

For manual grouping, enter one window number, then another to form a group immediately. Keep adding numbers; T ends that group and starts another. Space ends a number, and `~` selects a whole group.

| Input | Result |
| --- | --- |
| `12t34t` | Windows 1+2 and 3+4 form two groups; only when numbers have no multi-digit ambiguity |
| `123t` | Windows 1, 2, and 3 form one group |
| `~1` | Select group 1 |
| `~1 4t` | Add window 4 to group 1 |
| `~1 ~2t` | Merge group 2 into group 1, retaining number 1 |
| `1 Space 2 t` / `12 Space t` | Explicitly choose windows 1+2 / window 12 |

A window number selects one member; a group number selects the whole group. Selecting an existing member only switches tabs. Invalid numbers show a reason and keep the current target. New groups use the lowest free number, starting again at ~1 after all dissolve. T finishes valid pending number input without adding an undo step.

| Default key | Action / effect |
| --- | --- |
| `T` | `window_tab_end`: finish the group; clear selection if fewer than two windows |
| `~` / Space | `window_tab_group` / `window_number_end`: group prefix / number separator |
| `D` / `X` | `window_tab_remove` / `window_tab_dissolve`: remove the current member / dissolve the group |
| `Tab` / `Shift+Tab`, or `J` / `K` | Next / previous tab |
| `H` / `L` | Reorder the tab left / right |
| `Z` / `Shift+Z` | Undo / redo grouping, removal, dissolution, and ordering |
| `Ctrl+S` | `window_save_layout`: save a template and note |
| `Q` / `Esc` | Return to Window / exit to Idle, keeping completed groups |

Edit these in `[window_tab.bindings]`. After exit, apps receive Tab and digits normally, while the tab bar remains clickable. On Windows, drag tabs to reorder or transfer members, or drag `~group number` to merge a group. An insertion line marks the destination. Release outside the bar or right-click to cancel; scroll long tab bars.

Only the active member is visible. Quick/Editor treat a group as one target. Switching aligns the new member with the group's position; minimizing the active member minimizes the group. Closing the tab bar dissolves the group, while closing an app removes only that member. Dissolving or quitting KeySteer normally restores windows hidden by KeySteer.

::: info Save a template to reuse a group
Groups last until dissolved or KeySteer exits. Templates store member count, order, active position, area, and note, without app identities. Restore by choosing windows in order; changes apply after all are selected. Q/Esc cancels before then.
:::

Ordinary resizable windows are supported. Native fullscreen and incompatible windows show a reason. Windows hides inactive members; macOS minimizes/restores individual windows, so system animations may appear. The tab bar uses space at the top of the group without covering the app's bottom edge.

### Target application audio

In Window, Quick, and Editor, hold V and use J/K to lower/raise app volume by 1% per step. V+M toggles mute. Hold a direction to keep adjusting; release V to stop. Raising volume does not unmute. Several windows from the same app may change together.

For existing custom binding tables, add these lines to each relevant mode:

```toml
"v+j" = "window_volume_down"
"v+k" = "window_volume_up"
"v+m" = "window_volume_mute"
```

### Application output and system audio

| Default key | Action |
| --- | --- |
| `V+H` / `V+L` | `window_audio_previous` / `window_audio_next`: app output device |
| `Shift+V+J` / `Shift+V+K` | `window_system_volume_down` / `window_system_volume_up`: system volume |
| `Shift+V+M` | `window_system_volume_mute`: system mute |
| `Shift+V+H` / `Shift+V+L` | `window_system_audio_previous` / `window_system_audio_next`: system output device |

Devices cycle by name, one per press; the panel shows the target name. App output does not change the system default. Some apps adopt a device only after playback restarts. Holding a mute key does not keep toggling it.

::: info Audio availability
Windows apps need an audio session; start playback if none exists. System audio works on macOS 14+. Independent app audio needs macOS 14.2+, the packaged KeySteer.app, and System Audio Recording permission.
:::

macOS app audio is processed in memory without recording or uploading, but adds playback latency. Settings remain after leaving the mode; quitting KeySteer restores the original path. Devices without software volume support show a reason. See [Apple Core Audio Tap](https://developer.apple.com/documentation/CoreAudio/capturing-system-audio-with-core-audio-taps) for technical background.

## Movement, scrolling, and speed

| Action | Description |
| --- | --- |
| `move_left`, `move_down`, `move_up`, `move_right` | Move continuously while held; a tap also moves a short distance. |
| `scroll_left`, `scroll_right`, `scroll_up`, `scroll_down` | Scroll by `[scroll].scroll_step`. |
| `scroll_half_*` | Scroll by `scroll_step_half`. |
| `scroll_full_*` | Scroll by `scroll_step_full`. |
| `precision`, `slow`, `fast` | Change pointer speed while held. |
| `precision_toggle`, `slow_toggle`, `fast_toggle` | Toggle a speed with a tap; tap the same binding again to clear it. The active speed appears below the mode indicator. |
| `follow` | Toggle pointer following in Grid, Recursive Grid or Bisect. |
| `bisect_left` / `bisect_down` / `bisect_up` / `bisect_right` | Keep the left/bottom/top/right half in Bisect; default h/j/k/l. |

`wheel_*` remains a compatibility alias for `scroll_*`. Speed actions are usually paired with a movement key:

```toml
[normal.bindings]
h = "move_left"
"v b" = "fast"
```

`"v b"` binds two independent keys to the same action, not a sequence. Use `+` for a chord.

## Pointer buttons and dragging

| Action | Purpose |
| --- | --- |
| `mouse_left` / `mouse_right` / `mouse_middle` / `mouse_x1` / `mouse_x2` | Click the button; the first three also accept `left_click` / `right_click` / `middle_click`; long presses keep it held by default |
| `double_click` | Short press double-clicks; long press holds only the left button |
| `left_press` / `right_press` | Hold left / right mouse button |
| `left_release` / `right_release` | Release left / right mouse button |
| `toggle_left` / `toggle_right` | Toggle left / right held state |
| `toggle` without arguments | Lock partner keys or mouse buttons according to their Normal bindings |
| `press <target...>` / `release <target...>` / `toggle <target...>` | Hold / release / toggle one or more explicit targets |

Targets can be key names or `mouse_left`, `mouse_right`, `mouse_middle`, `mouse_x1`, and `mouse_x2`:

```toml
[normal.bindings]
n = "toggle"
x = ["press shift", "left_click", "release shift"]
```

The default no-argument toggle is N. Pair N with `;` to hold the left mouse button, in either press order. Several partners can be locked. Tap N alone, return to Normal, or enter Idle to release all. Holding N alone locks N itself.

`long_press_toggle_ms` defaults to 500 ms. While enabled, pressing a click key presses the mouse button, and release lets go; reaching the threshold keeps it held. With 0, pressing immediately completes a click instead. A short double-click press completes on release. See [Dragging and holding](/en/modes/normal#dragging-and-holding) for steps and automatic-release conditions.

## Send a key

A bare key name sends that key to the focused application. You can write a chord directly with `+` or use the explicit `send` form:

```toml
[normal.bindings]
t = "home"
"primary+shift+s" = "send primary+shift+s"
```

`send` must be followed by a valid key or chord. It injects input into the current application and does not switch KeySteer modes.

## Run an external command: `exec`

Use `exec` to launch scripts, apps, or desktop tools. These examples are for macOS:

```toml
[normal.bindings]
"primary+shift+t" = "exec open -a Terminal"
"primary+shift+b" = ["exec say build-started", "wait 500", "exec open ."]
```

Syntax:

```text
exec <program> [arg1] [arg2] ...
```

The first word is the program; later words are separate arguments. It starts in the background. KeySteer does not wait for completion or show command output. Launch failures are logged.

::: info When you need shell syntax
`exec` runs a program directly. It does not expand `~`, variables, pipes, redirection, or `&&`. Put complex logic in a script or call a shell explicitly.
:::

```toml
# Execute a script or program whose path contains no spaces.
x = "exec /usr/local/bin/keysteer-script"

# Windows: arguments are split on spaces.
y = "exec cmd /C start notepad"
```

Arguments split on spaces without quote escaping. For paths containing spaces or complex arguments, use a wrapper script at a path without spaces.

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

## Switch the active window

`window_activate_next` / `window_activate_previous` switches windows directly, without entering Window or showing numbers.

By default, it starts with the pointer's window, falling back to the active window. The pointer moves to the selected window's center. All ordinary non-minimized windows participate. Tabs members stay adjacent in tab order; windows on the same display and in the same app are kept together where possible. Cycling continues past the group, for example A1 → A2 → B1 → B2.

No default shortcut is assigned. Add your own:

```toml
[normal.bindings]
x = "window_activate_next"
c = "window_activate_previous"
```

## Cycle overlapping windows

`window_overlap_next` / `window_overlap_previous` cycles only within a group of overlapping windows, without entering Window.

If A overlaps B and B overlaps C, all three belong to one group even if A does not touch C. Touching edges alone does not count. Each press uses the latest positions; minimized windows are excluded.

By default, the active window is preferred, falling back to the pointer's window. Tabs are checked in tab order, but only within the overlap group. A lone window only centers the pointer; no valid target means no action.

```toml
[normal.bindings]
x = "window_overlap_next"
c = "window_overlap_previous"
```

## Move a window between displays

`move_window next` selects the next display; `previous` (or `prev`) selects the previous one. `move_window 2` selects display 2, using the same numbering as the `screen` plugin.

By default, `Primary+D` moves the pointer's window. The pointer follows, keeping its relative position where possible; focus and current mode stay unchanged. `Primary+S` only switches the pointer's display. One display or no movable window means no action.

| Situation | Behavior |
| --- | --- |
| Equal display sizes | Preserve window size and offset |
| Different display sizes | Scale position within the available space and fit the work area where possible |
| Maximized window on Windows | Keep maximized and migrate its restore position |
| Native fullscreen on macOS | Leave fullscreen, move, then restore fullscreen, with system animations |

The pointer stays within the target display. Apps may restrict movement, fullscreen transitions, or sizing at different DPI settings.

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
| `area_hint` | Show UI Hint within the current Grid, Recursive Grid, or Bisect region (default `u`); `Tab`, `Backspace`, or `Esc` returns to it. |
| `area_text` | Start Text Select within the current grid region (default `i`); `Esc` returns to it. |
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

Show or hide mode controls without clearing selection or filters.

Normal defaults to `"?" = "key_help"`; targeting modes can inherit it. Keep this entry in custom tables. See [Key help](/en/reference/configuration#key-help) for default visibility and styling.

## UI Hint scan scope

```toml
[ui_hint]
scan_scope = "all" # window / active / screen / all
```

| `scan_scope` | Area |
| --- | --- |
| `window` | Pointer's window, then active window if unavailable, then pointer's display |
| `active` | Active window, then pointer's window, then pointer's display |
| `screen` | Pointer's whole display, without combining other displays |
| `all` (default) | Visible windows and UI on all displays; spanning windows are traversed once |

All strategies use these rules on both platforms. The `window` scope attempts to activate the pointer's window. Text Select uses the same four choices through its independent `text_select.scan_scope`, also defaulting to `all`. Once selected, empty results, source failure, or activation failure do not change the scope.

`screen` / `all` use the global `strategy`, without applying the foreground application's `app_configs` strategy to other applications. Visibility checks exclude fully covered windows and retain visible portions of controls; visual sources read the composited desktop. Results grow dynamically with no default 10,000-target ceiling. For visual-only scans, `rectangle_max_candidates = 0` means unlimited; explicit positive values still apply, and Hybrid ignores that option. Timeouts, cancellation and image-size protections remain active.

From a grid, `area_hint` scans the selected region across visible applications using the global `strategy`. Results appear incrementally. In the label view, `previous_level` (and the default `Tab` action) returns directly to the grid's previous depth, path and region, even while the scan is running; rebind it in `[ui_hint.bindings]` if needed. While the search input is open, `Tab` and `Backspace` retain their search preview and editing actions. Press `Esc` to close search before using a return key to go back to the grid.

Existing configurations retain `u` or `i` when used as grid labels or custom inherited actions. Bind the scan actions to another key, or explicitly bind them locally. Region Text Select follows its `after_copy` setting: the default enters Normal, while `return` restores the grid region.

Changing displays or targets clears old labels and scans again; moving between displays within `all` preserves labels. Ordinary pointer movement does not keep rescanning. Press `Primary+R` to refresh manually.

## Window target source

Choose the active window or the window under the pointer with each window mode's `target` setting:

```toml
[window]
target = "active"
[window_quick]
target = "mouse"
[window_editor]
target = "active"
[window_restore]
target = "mouse"
[window_tab]
target = "active"
```

| Value | Priority |
| --- | --- |
| `active` | Active window → pointer's window |
| `mouse` | Pointer's window → active window |
| Omitted | Pointer's window for a new session; retain selection between window submodes |

An explicit setting chooses again on each mode entry. A suffix on an individual action overrides the mode's `target`:

```toml
[normal.bindings]
"alt+j" = "window_overlap_next active"
"alt+k" = "window_overlap_next mouse"
"alt+n" = "window_activate_next active"
"alt+w" = "window mouse"
"alt+d" = "move_window next active"
[window.bindings]
"x" = "window_close active"
"shift+x" = "window_close mouse"
"h" = "window_left mouse"
```

Within a mode, actions without a suffix keep using the selected window. A suffix chooses when each action starts; held movement/resizing stays with that target. A valid preferred window prevents fallback. A rejected operation does not switch to another window.

| Accept a suffix | Do not accept a suffix |
| --- | --- |
| Window cycling, mode entry, moving between displays | Undo, redo, reset to initial state |
| Moving, resizing, state, closing, app audio | Preset save/delete/confirm, number input |
| Layout direction, splitting, ratios, region removal, tab-member actions | System audio |

Layout actions can only choose windows already in the layout. Tab actions affect the selected window's group without merging groups. Overlap cycling with no companion only centers the pointer on the preferred window. If neither source has a valid window, nothing happens.

## UIHint search and information copying

Start with the [UI Hint exercise](/en/modes/ui-hint#search-and-result-previews). This section covers search syntax, copying, and style settings.

### Search and confirm

Press `/` to search, even before the first labels arrive. Search labels, OCR text, accessibility names, or control types, such as `Copy`, `复制` / `fz`, or `按钮` / `an` / `button`. Chinese supports initials or pasted text; system input-method candidate windows are not supported yet.

| Operation | Default key / result |
| --- | --- |
| Preview next result | `Tab`; changes preview without moving the pointer |
| Accept preview | `Enter`, `/`, `Primary+Q`; move, clear the query, stay in UI Hint |
| Cancel search | `Esc`; restore labels without moving |
| Labels only | `@la` or `la@`; `@l` narrows by label prefix |

`@` alone keeps candidate labels visible; it does not select all. An ordinary label filters rather than jumping immediately. Label codes stay stable during search. Confirming with no valid result does not move the pointer.

### Multiple selections and order

Separate selections with spaces, such as `@la @ka` or `fz button ab`. Results merge in input order, keeping the first occurrence of duplicates. Repeated spaces add no empty items; a trailing space shows candidates for the next item. Editing any item updates results.

Complete labels select directly. Text/initials count as selected points only on a unique match. Several points initially show their average center, `0/N`. N counts confirmed, unique labels only; unfinished or ambiguous items do not count.

### Copy the four fields

| Default key (both platforms) | Content |
| --- | --- |
| `Ctrl+1` | OCR text |
| `Ctrl+2` | Accessibility name and control type |
| `Ctrl+3` | Current point coordinates |
| `Ctrl+4` | Current point color |

Successful information copying confirms the point, moves the pointer, and closes search, keeping UI Hint labels. Empty fields or failure keep search open and preserve the clipboard. Visually truncated text is copied in full.

`search_copy_keys` requires four distinct, non-conflicting chords. An explicit `primary` follows platform aliases; the defaults use Ctrl, including on macOS.

### Adjust points and pick colors

Tap Ctrl to switch between input and adjustment. While adjusting, use Normal directions and speed keys. A green input background and right-side dot mark adjustment. Click actions first confirm the point, then perform the click.

`Ctrl+Shift+4` enters adjustment and cycles HEX → RGB → HSL. `Shift+4` still types `$`.

```toml
[ui_hint.search_bindings]
ctrl = "point_toggle"
tab = "point_next" # Any valid key or chord
"ctrl+shift+4" = "color_next"
```

`point_toggle` must be a single tapped key. `point_next` and `color_next` accept chords. A custom `search_bindings` replaces the whole table; an empty table disables these actions.

With multiple points, Tab moves from the average center to the first label, then cycles in selection order, showing `1/N…N/N`. Initial points match direct label jumps, regardless of label display offsets.

`[ui_hint.search_point]` supports `marker_color`, `marker_radius`, and `marker_width`. `color_formats` specifies a non-repeating order, with the first as default, such as `["rgb", "hex", "hsl"]`.

Colors come from the screen, excluding KeySteer's overlays. macOS needs screen-recording permission. Copying waits for current samples; failures keep an empty field instead of copying an old color.

### Displaying multiple points

The four `field_modes` entries correspond to OCR, accessibility, coordinates, and color:

```toml
[ui_hint.search_point]
field_modes = ["concat", "concat", "switch", "switch"] # Built-in defaults; optional
```

| Value | Display and copy |
| --- | --- |
| `concat` | Join initial-point information in selection order, separated by newlines |
| `switch` | Follow the current inspected point as Tab or directions change it |

Text and accessibility concatenate by default; coordinates and color follow the current point. Single-point text comes from the element under that point. Empty space retains coordinates and color only. Edit these options under **UI Hint → Behavior** on the website.

### Search input editing keys

`[ui_hint.search_edit_keys]` uses key = action, for example `"enter / primary+q" = "accept"`, `"primary+v" = "paste"`, `"primary+c" = "copy"`, and `"primary+a" = "select_all"`.

Spaces on the left separate alternative keys. Configuring an action replaces all its default keys; omitted actions retain defaults. Legacy action = key tables are still readable; exports use the new form. Copying an input selection copies query text without closing search.

### Panels and color styles

This example sets copying keys and the color swatch. Single-point colors show a swatch beside the title; concatenated multi-point colors do not show one swatch.

```toml
[ui_hint]
search_copy_keys = ["ctrl+1", "ctrl+2", "ctrl+3", "ctrl+4"]

[ui_hint.search_point.color_preview]
enabled = true
width = 16
height = 16
x_offset = 4
y_offset = 0
border_width = 1
```

Point adjustment can have its own border and background:

```toml
[ui_hint.search_point]
input_border_color = { light = "#16856BFF", dark = "#68D9B1FF" }
```

The default background is green: light `#E8F6F0FF` / dark `#284D44FF`. An unmodified border follows `search_input_ui.border_color`. Edit, preview, or reset these under **UI Hint → Appearance**.

The input and information panels support the same fonts, colors, borders, corners, and padding. The input defaults to width 280 and vertical padding 6. This example customizes the appearance:

```toml
[ui_hint.search_input_ui]
position_mode = "screen"
position = ["95%", "50%", "5%", "50%"]
width = 420
font_family = "Microsoft YaHei UI"
font_size = 16
padding_x = 12
padding_y = 6
border_radius = 8
border_width = 1
background_color = "#0A1338F2"
text_color = "#E8EEFFFF"
border_color = "#6E82D6FF"

[ui_hint.search_info_ui]
position_mode = "search_input"
position = ["44%", "50%", "56%", "50%"]
width = 520
font_size = 14
```

Position uses a reference and four percentage insets; see [Search panel positioning](/en/reference/configuration#search-panel-positioning). Preview both panels together in the web editor. Default styles do not need to be repeated in your file.

## Text Select

Press `c` in Normal to enter `text_select`. The input and original-position hints appear first; the compact text preview appears after input and stays visible during refinement. The mode uses its independent `text_select.scan_scope` (default `all`) and shares UIHint's OCR options and appearance, requesting OCR text only.

Type a region label to select its text and move the pointer `[12, 12]` beyond its bottom-right corner without clicking. Press `Alt+C` (Option+C on macOS) to refine into system-tokenized words, then Unicode grapheme characters. Ctrl+C on Windows/Linux and Command+C on macOS always copy the checked OCR text, regardless of query content or input highlighting; ordinary `c` remains input. Missing language rules fall back to characters without a bundled dictionary.

The first level uses UIHint search: type text, pinyin initials, or space-separated labels such as `a d f`; use `@a @d` for explicit labels. Word and character levels start fully checked and use fixed decimal numbers starting at 1, unchanged by filtering or paging. Letters, text and pinyin initials search every item in the current level, including other pages, without changing existing checks. A single number toggles its item; `1-20` selects the inclusive range, and `1-3 4-5` selects two ranges in input order without duplicates. Descending ranges follow their written direction. Numbers resolve immediately when no longer valid ID shares their prefix. With 20 items, `8`, `9` and `12` need no separator; `1` and `2` wait for more digits or confirmation. Ambiguous numbers and range endpoints accept a space, copy or refine action to confirm. After an automatic single-number toggle, a following hyphen still extends it into a range such as `8-9`. Ambiguity uses only the IDs actually displayed on the current page; other pages and filtered-out items cannot delay a visible number. Words, characters, input edits, page changes and arriving search results use the same matching rule. When both `1` and `12` are displayed, `1` still waits for more input or confirmation. Committed numbers leave the input while search text stays. Incomplete, invalid and out-of-bounds ranges do not execute; deleting them does not undo confirmed checks.

Backspace edits nonempty input; with empty input it clears all checked items on the current level. Ctrl+A selects the input characters when nonempty, or checks all items across pages when empty. Alt+Backspace restores the parent level. Tab cycles the word/character pages, wrapping from the last to the first; it does not change checked items. The footer shows the current/total pages and the cycle key. Ctrl+Backspace resets to regions. Enter follows the same copy rule. Successful copying triggers `text_select.lifecycle.after_copy`, returning to Normal by default. Ctrl+X clears the entire input without changing the clipboard or level; confirmed word/character checks stay unchanged. `Primary+Q` returns to Normal without copying. Esc exits; `?` shows effective keys. All controls are configurable.

Regions copy in source order; words and characters combine in selection order for both preview and copying. With empty input, press Backspace to clear the checks, then type numbers or ranges in the desired order. Unchecking removes an item; checking it again appends it to the end. Ctrl+A preserves the existing order and appends missing items in panel order. Paging, clearing input and returning to a parent preserve selection order. OCR rows in the same paragraph are joined and passed through the same whitespace cleanup as UIHint search OCR copying, preserving English word boundaries and paragraph separators. The system font layout wraps the preview to the actual panel width, so wider panels show more text per line; preview and copying use the same cleaned text. Disjoint English words get a separator and disjoint letters concatenate. Only the first level draws hints and selected-region outlines over the original text. Words and characters appear exclusively as checked choices in the panel; refinement neither calculates their screen coordinates nor runs extra character detection. The wider panel (default `text_select.ui.panel_width = 800`) previews up to four lines with paragraph breaks, separating the selection count from compact cards with spacing and a divider; copying includes all checked text. The standard Text Select indicator shows the level. Initial hints and input paint before cooperative first-level search indexing; refinement does not capture the screen again.

Hold Shift on the first level to raise the next overlapping hint layer; release to restore the default, and hold again to cycle further. This reuses `ui_hint.overlap_cycle_key` and accepts either side. Word and character levels do not cycle overlap layers. Word cards wrap by their individual lengths. Set `text_select.ui.word_min_width` / `char_min_width` (defaults 48 / 40, range 24–240) in **Text Select → Appearance** to adjust each grid’s density.

Configure the region outline and pointer landing independently. `padding = [x, y]` expands both horizontal and vertical sides by 0–100 units. `border_width` accepts 0–20; zero hides the outline. Omitting the color follows the theme accent. The pointer offset is relative to the original region's bottom-right corner: positive values move right/down, independently of outline padding.

Backspace and Ctrl+A switch between editing input and changing checked items. Copy always targets checked panel text; Ctrl+X always clears the entire input. Remapping retains these rules. Copy uses `text_copy`, with `mod+c` resolving to Ctrl+C or Command+C independently of the default Primary-to-Alt override and grouped with Enter. Add the group to an existing complete binding table, assign another key, or use `none` to disable it. With no checked panel text, copying leaves the clipboard untouched. Only successful copying of checked OCR text follows `lifecycle.after_copy`. Parent navigation uses a separate Alt+Backspace binding:

```toml
[text_select.bindings]
backspace = "text_clear_selection"
"ctrl+a" = "text_select_all"
"alt+backspace" = "previous_level"
tab = "text_cycle_page"
"alt+c" = "text_refine"
"ctrl+backspace" = "first_level"
"mod+c enter" = "text_copy"
"ctrl+x" = "text_clear_input"
esc = "cancel_selection"
"primary+q" = "normal"
"?" = "key_help"
```

```toml
[text_select.ui]
panel_width = 800
pointer_offset = [12, 12]
border_color = { light = "#16856BFF", dark = "#68D9B1FF" }
border_width = 1.5
padding = [2, 2]
word_min_width = 48
char_min_width = 40
```

## Configurable selection controls

`first_level` clears the selection and returns to the first level; `previous_level` goes back one level. `confirm_selection` confirms and `cancel_selection` cancels the selection and returns to the source mode. `search_selection` opens UIHint search. In ordinary UIHint, `reopen_selection` only reopens a completed selection, retaining its original Tab behavior. Global `escape` still enters Idle. A group such as `"tab backspace"` means either key, not a chord.

Existing defaults are unchanged. Sparse/older configurations inherit the formerly fixed controls; explicit `none` disables a key. For example:

```toml
[grid.bindings]
space = "none"
"ctrl+backspace" = "first_level"
```

Recursive Grid and Bisect have independent binding tables. Normal blind targeting also accepts reset/parent actions. UIHint and the screen selector plugin expose their controls in their binding tables. Window Editor's backtick uses `window_area_number`; Restore uses `window_previous_page` / `window_next_page`; Window selection input uses `window_multi_cancel` and configurable editing mappings. Defaults remain backtick, PageUp/PageDown, Esc and the existing editing keys. Help follows effective bindings, including disabled keys.
