# Release notes

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

Open the current configuration safely in the web simulator from the tray or menu bar, and fix macOS update-check crashes and worker cleanup on exit.

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
