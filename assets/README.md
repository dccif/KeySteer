# KeySteer project assets

Canonical visual assets used by the README, documentation, releases, and
website live here.

- `brand/`: logos, wordmarks, color references, and original source files.
- `icons/`: processed application icons consumed by builds and packaging.
- `images/`: general illustrations and diagrams.
- `screenshots/`: product screenshots and demonstrations.

Prefer lossless source files here. Optimized website copies may be generated
under `../website/assets/images/`.

The README uses `brand/keysteer-wordmark.webp`; its lossless PNG counterpart
is kept beside it. `icons/keysteer-icon.png` is a lightweight 256 px transparent
master. `keysteer.ico` embeds 16-64 px representations into Windows portable
executables. The master PNG is compiled into the macOS status item, and the
macOS packager also generates 16-256 px Retina application-icon
representations from it.
