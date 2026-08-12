# Windows comparison runner

`..\benchmark-windows-dist.ps1` performs repeatable startup and resident
resource sampling for a packaged build and writes JSON under
`target/benchmarks/`. Run the same scenario for baseline and candidate from
separate worktrees/target directories; keep only changes meeting the p99 and
memory gates documented in `docs/ai/08-build-docs-and-tests.md`.

Pass `-UsePerfProbe` only for a binary built with the `perf-probe` feature; this
records the internal `backend_started` marker. Without that switch, the startup
metric is intentionally named `config_check_process_ms`.

The runner deliberately keeps samples in memory until the process exits so
filesystem I/O does not contaminate the measured interval.
