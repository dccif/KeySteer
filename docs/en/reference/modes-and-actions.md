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

## Movement, scrolling, and speed

| Action | Description |
| --- | --- |
| `move_left`, `move_down`, `move_up`, `move_right` | Move continuously while held; a tap also moves a short distance. |
| `scroll_left`, `scroll_right`, `scroll_up`, `scroll_down` | Scroll by `[scroll].scroll_step`. |
| `scroll_half_*` | Scroll by `scroll_step_half`. |
| `scroll_full_*` | Scroll by `scroll_step_full`. |
| `precision`, `slow`, `fast` | Change pointer speed while held. |
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
| `left_click`, `right_click`, `middle_click` | Inject one complete click immediately on the physical key-down edge. |
| `double_click` | Double-click the left button. |
| `left_press`, `right_press` | Hold a pointer button. |
| `left_release`, `right_release` | Release a held pointer button. |
| `toggle_left`, `toggle_right` | Toggle the held state of that pointer button. |
| `toggle` | Without parameters, toggle a paired input latch regardless of which key was pressed first. A short standalone tap releases all latches; a long standalone hold latches its activating key. |
| `press <target...>` | Hold one or more keys or pointer buttons. |
| `release <target...>` | Release targets that were previously held. |
| `toggle <target...>` | Toggle the state of targets. |

Targets are key names or `mouse_left`, `mouse_right`, and `mouse_middle`:

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
