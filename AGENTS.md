# KeySteer AI maintenance entry point

Before changing this project, read `docs/ai/README.md`, then read only the topic
files it routes to for the task at hand.

Source-of-truth order:

1. Current Rust/TypeScript/build code and tests.
2. `keysteer.default.toml` for shipped behavior.
3. `docs/ai/` for architectural intent and navigation.

Architectural rules that must remain true:

- Modes and plugins use `api::ModeEvent`, `api::Command`, and `HostContext`; they
  do not call native APIs.
- Native code stays under `src/platform/<os>/` and implements `api::Backend`.
- A successful synthetic click emits one semantic `Clicked` event; physical
  clicks and press/release/toggle do not.
- Targeting `keep` preserves the existing mode instance and selection state;
  only `restart` sends `Restarted`.
- UI scanning is asynchronous, cancellable by scan id, and streams partial
  results. Do not block the engine loop waiting for a full scan.
- Overlay hot paths reuse native windows, buffers, images, and fonts. Do not
  reintroduce per-frame full-screen allocation without measurements.
- Packaged macOS application data belongs in
  `~/Library/Application Support/KeySteer/`; portable binaries use their own
  directory, never the process working directory.

When a change moves a boundary, changes a lifecycle, or adds/removes a major
module, update the relevant `docs/ai/` topic and its index entry in the same
change.
