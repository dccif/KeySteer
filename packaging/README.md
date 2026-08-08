# KeySteer packaging

Do not distribute the bare files from `target/release`. Use the platform script
so users receive the application identity, icon and portable archive expected
by the operating system.

## macOS

KeySteer targets macOS 14 or later so the native ScreenCaptureKit screenshot
API can be used directly without compatibility shims.

Run on a Mac; the host architecture is selected automatically:

```bash
bash packaging/macos/package.sh
```

Pass `aarch64-apple-darwin` or `x86_64-apple-darwin` to build a specific target.
The script builds an optimized binary, creates and signs `KeySteer.app`, embeds
the icon, and produces a ZIP plus SHA-256 file under `dist/<target>/`.

Local builds use an ad-hoc signature. A public release should provide a stable
Developer ID identity so Accessibility and Screen Recording permissions remain
attached to KeySteer across upgrades:

```bash
KEYSTEER_CODESIGN_IDENTITY="Developer ID Application: Example (TEAMID)" \
APPLE_ID="developer@example.com" \
APPLE_TEAM_ID="TEAMID" \
APPLE_APP_PASSWORD="xxxx-xxxx-xxxx-xxxx" \
bash packaging/macos/package.sh
```

`KEYSTEER_NOTARY_PROFILE` may be used instead of the three Apple account
variables when a `notarytool` keychain profile is already configured.

## Windows

Run in PowerShell; the host architecture is selected automatically:

```powershell
.\packaging\windows\package.ps1
```

Pass `x86_64-pc-windows-msvc` or `aarch64-pc-windows-msvc` for a specific
target. The output is a portable ZIP containing the icon-bearing, GUI-subsystem
`KeySteer.exe`, plus its SHA-256 file under `dist/<target>/`.

### Benchmark the unpacked Windows package

After packaging, benchmark the actual distributed executable (not
`target/release`) with the comparison runner wrapper:

```powershell
.\tools\benchmark-windows-dist.ps1 x86_64-pc-windows-msvc
```

The script defaults to the validated `Alt+E`, then `;` left-click scenario and
writes the runner CSV plus a JSON sidecar under `dist/<target>/benchmarks/`.
Pass `-SetupKey`, `-Key`, or `-Observe` to match another configuration;
`-Executable` supports an archive extracted elsewhere, and `-KeepRunning`
leaves the launched KeySteer instance open. Its recorded `process_create_ms`
measures `CreateProcess` completion only; the fixed `-StartupMs` delay is the
application-settle window, not a readiness metric.

## GitHub releases

CI calls the same scripts. Pushing a tag such as `v0.1.0` builds all four
archives and publishes them to a GitHub Release. Optional macOS signing secrets
are documented by the variable names in `.github/workflows/release.yml`.
