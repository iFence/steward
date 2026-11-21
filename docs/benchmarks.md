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

## v0.3.2 NSH-4 Local Release Candidate Evidence

Environment: local Windows development sandbox, `--release` build, 2026-11-21.
The same sandbox still does not expose WMI hardware details, so this closes the
measurement gate as development evidence but is not clean-VM acceptance. Plugin
packages were signed by an ephemeral local root; no development key or trust
root was committed.

| Metric | Result |
|---|---:|
| Release executable | 46,571,008 bytes |
| Release executable SHA-256 | `2359171b370d087ac40dc19628fa1791061dc27a110a3672a9d0480e26cad9dc` |
| MSI | 20,504,576 bytes |
| MSI SHA-256 | `2ad516064982ad1f94883f1360af69e0e0dd4e79ebf15a5a91997cd139625dec` |
| Cold start to tray-ready | 447 ms |
| Tray-only RSS | 95.6 MiB working set / 47.4 MiB private |
| Launcher-open RSS | 102.8 MiB working set / 57.8 MiB private |
| First summon | 44 ms |
| Summon p95, 20 samples | 60 ms |
| Calendar first-frame p95, 20 samples | 332.17 ms |
| Calendar existing-window focus p95 | 0.34 ms |
| Calendar host peak RSS p95 | 102.88 MiB |
| Calendar aggregate peak RSS p95 | 207.75 MiB |
| Clipboard History first-frame p95, 20 samples | 328.14 ms |
| Clipboard History existing-window focus p95 | 0.36 ms |
| Clipboard History host peak RSS p95 | 99.55 MiB |
| Clipboard History aggregate peak RSS p95 | 201.85 MiB |
| UI Showcase first-frame p95, 20 samples | 340.63 ms |
| UI Showcase existing-window focus p95 | 0.64 ms |
| UI Showcase host peak RSS p95 | 98.93 MiB |
| UI Showcase aggregate peak RSS p95 | 202.57 MiB |
| Hidden plugin VMs at tray-ready | 0 |
| Hidden plugin host processes at tray-ready | 0 |
| Calendar host peak handles | 438 of 1,024 |

All three official plugins opened together in one shell with exactly three host
processes. Shell peak RSS was 98.04 MiB, host peak RSS was 289.90 MiB, aggregate
peak RSS was 387.95 MiB, and orphaned host processes after shell termination
were zero. First-frame times in the three-window run were 358.56 ms (Calendar),
367.99 ms (Clipboard History), and 383.34 ms (UI Showcase).

The forced-crash release run terminated the Calendar host after its first
frame. The shell remained alive, host count returned to zero, the failure was
reported as `runtime_error`, and the repeated launch was rejected with 695 ms
remaining in the 2-second first backoff. The Job Object query reported 256 MiB
memory, 50% CPU, one active process, and a 1,024-handle polling ceiling.

A second Release run lowered the test-only process/job memory limit to 32 MiB.
The host was terminated, the shell stayed alive, host count returned to zero,
and the failure was classified as `resource_limit`.

The final local 20-sample summon run measured a 60 ms p95, which exceeds the
50 ms gate even though an earlier build on the same sandbox measured 46 ms.
This gate remains open and must be resolved on the clean acceptance VM.

Release route scaling used one command per synthetic plugin, round-robin across
command, prefix, and regex triggers. Query figures are p95 over 200 samples
after 20 warm-up queries:

| Installed plugins | First scan | Cached metadata read | Route build | Exact p95 | Prefix p95 | Regex p95 |
|---:|---:|---:|---:|---:|---:|---:|
| 100 | 104.86 ms | 2.45 ms | 2.07 ms | 56.80 us | 5.50 us | 88.50 us |
| 500 | 450.68 ms | 3.74 ms | 6.59 ms | 59.40 us | 15.50 us | 63.70 us |
| 1000 | 885.23 ms | 4.62 ms | 13.81 ms | 75.00 us | 28.10 us | 162.80 us |

The generated MSI compiled and linked with WiX 3.14.1 and decompiled into one
application executable, licenses, icon, and WiX runtime binaries. It did not
contain the retired plugin runtime or file-index helper. Administrative
extraction failed with Windows Installer error 2503 in this sandbox, so clean
install/upgrade/repair/uninstall remains an acceptance-VM gate.

## v0.3.1 P0 Release Candidate Evidence

Environment: local Windows development sandbox (Intel UHD Graphics 630),
optimized release build, 2026-11-17. The sandbox denied WMI/CIM hardware
details and interactive desktop access, so this is development evidence rather
than clean-VM release acceptance.

| Metric | Result |
|---|---:|
| Cached cold start to tray-ready metrics | 472 ms |
| Tray-only working set | 107.5 MiB |
| Launcher open working set | 108.3 MiB |
| First summon | 39 ms |
| Summon p95, 20 close/summon samples | 46 ms |
| Calendar first-frame p95, 20 samples | 349.1 ms |
| Calendar existing-window focus p95 | 5.4 ms |
| Clipboard History first-frame p95, 2 samples | 327.9 ms |
| UI Showcase first-frame p95, 2 samples | 336.2 ms |
| Hidden plugin VMs after tray startup | 0 |
| v0.3.1 MSI size | 26,505,216 bytes |
| v0.3.1 MSI SHA-256 | `38784d16424a8e7f9175301483cf4dd94d81121327d15bae73d04a56aa03325b` |

The plugin measurements used packages signed by an ephemeral local root through
`STEWARD_NSH_TRUST_ROOT`; no development key or trust root is committed.
Production-signed artifacts for these source changes were subsequently
regenerated by Build and Release run `44029877843`.

## v0.3.1 Production-signed Artifacts

Environment: GitHub Actions Windows runner, manual release workflow on
2026-11-17, commit `f91f19436c6cead116b0eb3a1fd22ed5ddb64a7e`.

| Artifact | Size | SHA-256 |
|---|---:|---|
| `steward-app-0.3.1-x86_64.msi` | 26,529,792 bytes | `be7b71799a5c9a44673140a42d4c03d42ff7517f1af39771dba972f4cd53d165` |
| Calendar `0.2.0` zip | 29,710 bytes | `01045e968adbac41e4fb9ec23cd797a9224c1cb8d51860c1e58fba0e265b9d20` |
| Clipboard History `0.1.1` zip | 7,914 bytes | `374d9b233a5fc621cd2f12345982aced4742ebf6841a6407d9061fcb8d21c23a` |
| UI Showcase `1.0.0` zip | 5,158 bytes | `3e3155671c71c8322e305f24d10df0ccd7316876dc3f6d4f297059918522546a` |

The workflow verified every zip against the committed production root. After
download, the same archives were checked against their published checksum
manifests and verified again locally with the production root. The clean-VM
install, upgrade, repair, and uninstall gates remain open.

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
| Hidden plugin host processes | zero |

Also record:

- executable and MSI size;
- shell RSS and aggregate RSS with zero, one, and three open plugin windows;
- plugin host process count while launcher-only, after opening each window, and
  after closing all windows;
- route-search latency at 100, 500, and 1000 synthetic installed plugins.

## Method

Use `scripts/bench.sh` and `scripts/bench-resident.ps1` where applicable. For
NSH plugin measurements:

1. Start a release build with an empty plugin root.
2. Record process start, tray readiness, and first launcher summon.
3. Install signed packages and record first-frame time for each plugin.
4. Invoke the same plugin command again and record focus-plus-command latency.
5. Close all plugin windows and verify no hidden plugin VM or plugin host
   process remains.

Store raw output and machine details alongside any future table updates.

`scripts/bench-plugins.ps1` reads the `plugin_host_containment` and
`plugin_host_process_count` metrics produced by the release build. Its
`summary.json` records the plugin-host PID, configured memory/CPU/handle limits,
peak child RSS, aggregate shell plus child RSS, peak handle count, first-frame
p95, focus p95, and whether the child process disappeared after the shell was
stopped. This replaces functional smoke output as the Phase 3 measurement
input, but the production gate still requires execution on the clean acceptance
VM.

`scripts/bench-plugin-set.ps1` opens all three official plugins in one shell.
Its summary records the three child PIDs, plugin-host process count, per-plugin
first-frame/focus timings, shell RSS, host RSS, and aggregate RSS. Use it to
fill the zero/one/three-host sections of this file; use
`bench-plugins.ps1` for the per-plugin cold-window p95 gates.

For crash containment and restart policy, run:

```text
pwsh scripts/test-plugin-isolation.ps1 -Exe <release exe> `
  -PackagePath <signed plugin.nshpkg> -TrustRootFile <production root>
```

The script force-terminates the plugin host after its first frame, then verifies
that the shell remains alive, the host count returns to zero, and the repeated
command is rejected during the 2-second first backoff window.

Add `-MemoryLimitMiB 32` to verify that a reduced process/job cap produces a
structured `resource_limit` failure without terminating the shell.

## Synthetic Plugin Scaling

`scripts/gen-test-plugins.sh <count> <dir>` generates route-only plugin
fixtures. It is intended for cold-start metadata scans and route-query latency
at 100, 500, and 1000 installed plugins. Synthetic fixtures are unsigned and
must not be opened; package verification is a separate acceptance path.

The checked-in Release benchmark generates and removes its own fixtures:

```text
cargo test -p steward-plugin-host --release \
  synthetic_plugin_route_scaling -- --ignored --nocapture
```

It covers first scan, cold cached metadata read, route-index build, and exact,
prefix, and regex query p95 without starting a plugin VM.

## Historical Baselines

Pre-NSH M0-M3 measurements and file-index experiments are preserved in
[benchmarks-history.md](benchmarks-history.md). Those baselines describe the
retired runtime and must not be treated as current plugin performance data.
