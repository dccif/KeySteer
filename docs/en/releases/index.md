# Release notes

## 0.9.19

- Window Mover now defaults to `Primary+D`. Bindings remain configurable, and `Primary+S` independently switches the pointer display.
- The web configuration simulator now supports importing, editing, and exporting window-movement actions and `precision_toggle`, `slow_toggle`, and `fast_toggle`.

## 0.9.18

Fixed lost key routes after Reload Configuration, so saved settings take effect without quitting.

## 0.9.17

Added tap-to-toggle speed actions: `precision_toggle`, `slow_toggle`, and `fast_toggle` latch a speed on the first press and clear it on the next press. The active speed appears below the mode indicator. Parameterless `toggle` is decoupled from speed state, so both `n+Shift` and `Shift+n` preserve the existing combination-key behavior without re-running or clearing the speed action.

## 0.9.16

Improved Window Mover: maximized windows now move directly to another display while staying maximized, without a restore/maximize flicker.

## 0.9.15

Added the Window Mover plugin: point at an application window and press `Primary+S+D` to move both the window and pointer to the next display while preserving the pointer's relative position. `Primary+S` alone still switches the pointer display. Configure `move_window previous` or `move_window <display number>` for other destinations.

## 0.9.13

Reorganized the internal configuration, mode, and runtime boundaries and optimized large UI Hint label batches without changing matching, occlusion, or display-layer switching semantics. Signed Windows releases can now verify the same publisher, replace and restart automatically after downloading, and restore the previous version if startup fails.

## 0.9.12

Fixed UI Hint visual recognition failures, missing top status icons, and hints appearing for off-screen list items on macOS.

## 0.9.11

Improved input and UI Hint responsiveness, reduced OCR scan waits, and strengthened cancellation and resource cleanup on Windows and macOS for smoother, more reliable repeated use.

## 0.9.10

Fixed held-mouse text and pressed colors sometimes remaining after automatic drag release, while reducing temporary allocations and duplicate work in shared indicator and input-state handling.

## 0.9.9

Added optional automatic release for modifier-assisted dragging: after holding a mouse button, any modifier combination can pass through and the button is released when pointer movement stops; disabled by default.

## 0.9.8

Fixed the keyboard `toggle` state issue caused by the same kind of input transition as the earlier mouse long-press bug: multiple modifiers can now latch immediately in either order, remain held after their physical keys are released, and no longer pass the toggle activation chord to the focused window.

## 0.9.7

Fixed [Issue #1](https://github.com/dccif/KeySteer/issues/1), where a click could be triggered accidentally before a mouse button entered its held state, and upgraded the Rust toolchain to 1.98.

## 0.9.6

UI Hint now uses a reusable session workspace, an exact X-axis sweep, and a fast path for two-label overlap groups in the common 129–256-label range while preserving the inline path through 128 labels.

## 0.9.5

Windows UI Hint now scans the window group under the pointer at submission time. Window-context changes immediately clear stale hints and retarget without consuming retries, while Hybrid shares one scan plan and fully cancels recognition and releases generation resources when returning to Normal or Idle.

## 0.9.4

Cross-platform UI Hint now shares allocation-free text analysis and exact prefix highlighting, with corrected final visual-layer cycling.

## 0.9.3

UI Hint labels now use tighter spacing and improved cross-platform vertical centering. Exact typed-prefix colouring on macOS uses fewer native layers to prevent incomplete or overflowing highlights.

## 0.9.2

Windows UI Hint now defaults to Hybrid, merging UI Automation and visual results in parallel to cover window controls. Labels are more compact, readable, and slightly higher; tiled OCR shows its first results earlier, overlays respond faster, rescan position races are fixed, and cleanup and safety boundaries are tighter.

## 0.9.1

Windows system OCR now tiles by CPU and image size, streams completed regions, and skips unavailable OCR resources entirely.

## 0.9.0

Windows UI Hint adds on-demand dual OCR with lower capture latency, lower peak memory, and immediate cleanup after scanning.

## 0.8.14

Further reduce input and UI Hint tail latency and temporary allocations while tightening native thread and unsafe boundaries on Windows and macOS.

## 0.8.13

UI Hint now consumes scan results without cloning, chord injection avoids temporary allocations, and cross-platform unsafe boundaries are tighter.

## 0.8.12

Fix update checks on Windows and macOS, open the simulator in the Windows browser, and clean up update workers reliably.

## 0.8.11

Open the current configuration safely in the web simulator from the Windows tray or macOS top status icon, and fix macOS update-check crashes and worker cleanup on exit.

## 0.8.10

UI Hint now cancels stale scans immediately on exit, keeps repeated entry fast, and fixes a potential hang when macOS Accessibility permission is revoked.

## 0.8.9

UI Hint scanning and overlap switching are more reliable, startup and input are faster with lower memory use, and native resource and unsafe boundaries are tighter.

## 0.8.8

Input response and UI Hint scanning are faster with lower memory use and a smaller package.

## 0.8.7

Cursor and indicator movement is smoother on Windows and macOS, while key combinations and hold actions are faster and use less memory.

## 0.8.6

Fixed `n = "toggle"`: hold `n` alone to keep it pressed, use it with keyboard or mouse keys in either order to lock them correctly, and tap `n` to release everything.

## 0.8.5

Windows and macOS now feel faster and use less memory for movement, display, keyboard input, and UI search, with no changes to existing configuration or controls.
