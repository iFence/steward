# Steward Benchmarks

## v0.3.0 NSH Functional Evidence

Environment: Windows development machine, debug build. Recorded 2026-10-27.

| Check | Result |
|---|---|
| Silent startup | Process alive; tray and global-hotkey windows created |
| Launcher surface | NSH Rust root rendered in a GPUI Fast window |
| Settings surface | NSH Rust settings root opened without a process crash |
| Calendar | Signed NSH package opened a full plugin window |
| Clipboard History | Signed NSH package opened a full plugin window |
| UI Showcase | Signed NSH package opened a full plugin window |
| Headless plugin tests | 3 plugins, 3 tests passed |
| Package verification | All three `.nshpkg` files verified with the development root |
| Package layout | Each zip contained only `plugin.json` and `plugin.nshpkg` |
| Release smoke | Optimized release opened the development-signed Calendar window |
| MSI build | v0.3.0 MSI linked and decompiled; no plugin runtime file present |

These are functional smoke results, not release performance acceptance.

## Release Gates

The NSH release cannot be marked complete until the following are measured on a
release build:

| Metric | Gate |
|---|---:|
| Cached cold start | Previous release + 500 ms or less |
| Summon p95 | 50 ms or less |
| Idle RSS increase | 32 MiB or less |
| Official plugin first frame p95 | 500 ms or less |
| Existing plugin focus p95 | 50 ms or less |
| Hidden plugin VMs | zero |

Also record:

- executable and MSI size;
- idle RSS with zero, one, and three open plugin windows;
- route-search latency at 100, 500, and 1000 synthetic installed plugins.

## Method

Use `scripts/bench.sh` and `scripts/bench-resident.ps1` where applicable. For
NSH plugin measurements:

1. Start a release build with an empty plugin root.
2. Record process start, tray readiness, and first launcher summon.
3. Install signed packages and record first-frame time for each plugin.
4. Invoke the same plugin command again and record focus-plus-command latency.
5. Close all plugin windows and verify no hidden plugin VM remains.

Store raw output and machine details alongside any future table updates.

## Synthetic Plugin Scaling

`scripts/gen-test-plugins.sh <count> <dir>` generates route-only plugin
fixtures. It is intended for cold-start metadata scans and route-query latency
at 100, 500, and 1000 installed plugins. Synthetic fixtures are unsigned and
must not be opened; package verification is a separate acceptance path.

## Historical Baselines

Pre-NSH M0-M3 measurements and file-index experiments are preserved in
[benchmarks-history.md](benchmarks-history.md). Those baselines describe the
retired runtime and must not be treated as current plugin performance data.
