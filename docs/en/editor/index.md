# Configuration & Simulator (Beta)

Configuration & Simulator is for trying a change before saving it: edit bindings in the browser, preview pointer actions, and adjust Grid, Recursive Grid, and UI Hint styles. Data stays in your browser and is never uploaded. For complex actions, external commands, and advanced fields, treat the TOML documentation as authoritative.

With KeySteer 0.8.11 or later, choose **Configuration & Simulator...** in the tray or menu-bar menu to open the active configuration directly. The configuration is passed to the browser in a URL fragment, which GitHub Pages requests never receive; the page clears it immediately after reading it. A browser extension could theoretically read the page address during the handoff, so do not keep passwords or tokens in configuration commands.

<p class="ks-open-simulator"><a href="../simulator" target="_blank" rel="noopener">Open Configuration & Simulator ↗</a></p>

## Recommended workflow

1. Open the simulator, import an existing TOML file, or start with the default configuration.
2. Change bindings and mode styles while watching the preview.
3. Download the generated `keysteer.<name>.toml`.
4. Place it in KeySteer's data directory and choose Reload Configuration from the status menu.
5. When configuration errors occur, run `keysteer --check -c <file>`; the Rust program's validation is authoritative.

The simulator is helpful for rapid experimentation and previewing, but it is not a complete configuration validator. Complex fields, platform permissions, external commands, and final validation remain the responsibility of KeySteer. See [Configuration](/en/reference/configuration) and [Modes and actions](/en/reference/modes-and-actions) for the full syntax.

The simulator opens in a new page to keep its wide keyboard layout from covering the documentation sidebar. It also works well on a second display.
