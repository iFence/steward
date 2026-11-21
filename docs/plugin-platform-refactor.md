# Steward Plugin Platform Refactor

> Status: implementation tracked here. Phases 1 and 2 are complete. Phase 3
> process isolation implementation is complete and blocked on clean-VM
> acceptance; marketplace admission, host API negotiation, and user-managed
> capability grants remain phase 4 work.

## Invariants

- Release builds load verified signed packages only.
- Route discovery never starts a plugin VM.
- No plugin VM exists without an open plugin window.
- Search continues to use `nucleo`; this work does not add a second matcher.
- The current `plugin.json` schema, launch parameters, and TypeScript helper
  surface remain compatible.
- The shell never owns a plugin VM.
- Third-party plugins remain inadmissible until process isolation, resource
  containment, capability review, and marketplace policy are complete.

## Status

| Area | Status |
|---|---|
| Phase 1: launch and diagnostics hardening | Complete |
| Phase 2: responsibility and dependency split | Complete |
| Phase 3: one process per plugin window | Blocked on clean-VM acceptance |
| Phase 4: third-party marketplace admission | Not started |

M5 local ecosystem infrastructure is complete: package scaffolding,
inspection UI, staged lifecycle operations, rollback history, a local
marketplace index, publisher identity, revocation application, and automated
policy checks. It does not yet admit public third-party publishers into a
hosted marketplace; that admission policy remains Phase 4 work.

## Phase 1: Launch and Diagnostics

### Plugin lifecycle

- [x] `P1-LAUNCH-1`: package reads, verification, unpacking, and launch-policy
  construction run on a worker thread.
- [x] `P1-LAUNCH-2`: plugin launch requests are coalesced by plugin ID while a
  launch is pending.
- [x] `P1-LAUNCH-3`: a package receipt is accepted only after the first frame
  and a 250 ms fatal-error grace period.
- [x] `P1-LAUNCH-4`: launch failures produce a dismissible launcher notice and
  a structured log event.
- [x] `P1-LAUNCH-5`: opening an already-running plugin focuses its window and
  forwards the newest command without creating another VM.

### Search and routing

- [x] `P1-SEARCH-1`: launcher input is debounced by 40 ms while generation
  checks discard stale results.
- [x] `P1-SEARCH-2`: regex triggers are compiled when the route index changes.
- [x] `P1-SEARCH-3`: each result source is capped at 32 candidates before UI
  truncation.

### Diagnostics

- [x] `P1-LOG-1`: daily rolling logs are written under the Steward data
  directory and retain seven days.
- [x] `P1-LOG-2`: application errors use `tracing` instead of `eprintln!`.
- [x] `P1-LOG-3`: panic location and payload are recorded before the release
  process aborts.

### Evidence

Evidence:

- `cargo test --workspace`: 137 tests passed, 1 ignored.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- Launch work is owned by `plugin-host::launcher` and started through
  `PluginHost::start_launch`; the UI thread receives a bounded worker channel.
- A wrapper around the NSH `AppView` observes its actual first render. Receipt
  acceptance is emitted only after that frame and a 250 ms no-fatal-error grace
  period; a two-second watchdog reports `first_frame_failed` when no frame is
  rendered. Session IDs prevent timers or errors from an old window affecting a
  reopened plugin window.
- Calendar host requests use one lazily-created, bounded worker per plugin
  window instead of one detached thread per call.
- Launcher application, file, settings, plugin, calculator, and URL results all
  resolve to an icon; missing native or manifest icons fall back to
  Steward-owned SVGs.
- Direct local equivalents `eslint` and `prettier --check` passed, and all
  three plugin `tsc -p` checks passed.
- `pnpm` script execution was blocked by the host PowerShell execution policy;
  `corepack pnpm` then attempted an online dependency repair, which the sandbox
  denied. The NSH CLI is not installed on this machine, so `nsh test` and
  `nsh build` were not rerun.
- The normal debug binary target was locked by a running Steward instance. The
  isolated verification build was started and then stopped at the user's
  request; `cargo check --workspace` and linked test binaries passed.

## Phase 2: Responsibility Split

### Shared crates

- [x] `P2-SEARCH-1`: create `steward-search` for shared haystack generation and
  matcher support.
- [x] `P2-SEARCH-2`: `plugin-host` no longer depends on `steward-core-engine`.
- [x] `P2-CONTRACT-1`: create `steward-plugin-contract` for launch, notice,
  error, session, and protocol-version types.

### Plugin host

- [x] `P2-HOST-1`: split catalog, route, trust, policy, and launcher concerns
  while preserving the `PluginHost` facade.
- [x] `P2-HOST-2`: route updates and launch preparation have focused tests.

### Application services and search

- [x] `P2-APP-1`: expose narrow launcher query, window-command, settings, and
  notice ports from the application-service facade.
- [x] `P2-APP-2`: split built-in, application, and plugin search sources behind
  a common contribution boundary.
- [x] `P2-APP-3`: move plugin window/session state behind a dedicated manager.

### Storage

- [x] `P2-STORAGE-1`: split settings, app usage/cache, clipboard, and schema
  responsibilities without changing the public `Storage` API or schema.

### Evidence

Evidence:

- `cargo tree -p steward-plugin-host` contains no `steward-core-engine`.
- `cargo tree -p steward-storage` contains no matcher, pinyin, or Windows
  application-scanner dependencies.
- Shared search and contract crates are workspace members with unit tests.
- Route regexes are compiled during indexing and query results are capped.
- `events.rs` schedules handlers; domain commands live in `handlers.rs`.
- `PluginWindowManager` owns window lookup, insertion, removal, and close
  cleanup.
- `PluginRuntimeState` owns plugin catalog scans, route updates, trust roots,
  pending launches, and launch polling; `SearchState` is limited to launcher
  search and app cache coordination.
- Search contributions and replies carry one `LauncherItem` per result, with its
  `RowAction` attached. Rows and actions are derived only at the UI boundary.
- Plugin manifest validation enforces path-safe reverse-domain IDs and semantic
  versions before route metadata reaches SQLite.
- Storage delegates settings, application cache, clipboard, and schema logic
  to separate internal modules without depending on `steward-core-engine`.

## Phase 3: One Process Per Plugin Window

### Process boundary

- [x] `P3-PROCESS-1`: the shell starts one `steward-app
  --plugin-host-process` child per open plugin and keeps no NSH `AppView`.
- [x] `P3-PROCESS-2`: parent/child messages are versioned, session-scoped
  NDJSON with separate control and lifecycle event directions.
- [x] `P3-PROCESS-3`: the child independently reloads the cached route
  metadata, verifies the signed package against the same trust roots, applies
  the release capability policy, and only then creates its NSH window.
- [x] `P3-PROCESS-4`: the parent accepts the package receipt only after the
  child reports an actual render and the existing 250 ms fatal-error grace
  period.
- [x] `P3-PROCESS-5`: repeated commands, focus changes, theme changes, and
  close requests cross the process boundary without creating another VM.
- [x] `P3-PROCESS-6`: child exit and protocol failures become structured
  launcher notices; closing a child is reaped by the shell.
- [x] `P3-PROCESS-7`: enforce process memory/CPU/handle limits with a Windows
  Job Object and define restart/backoff policy.
- [ ] `P3-PROCESS-8`: blocked; Release measurements are complete, but the
  signed-package crash/upgrade acceptance run requires a clean Windows VM.

### Isolation boundary

The child has its own GPUI application loop, NSH engine, QuickJS VM, host
channel, renderer, and process failure domain. It still runs as the same
Windows user and is not an OS security sandbox. Signed packages, release
capability policy, NSH engine quotas, and the parent/child protocol remain the
application-level controls. Every child is additionally assigned to a Windows
Job Object before its start message is written:

- 256 MiB process and job memory;
- 50% hard CPU-rate cap;
- one active process with kill-on-job-close;
- a 1,024-handle ceiling, checked every 500 ms and enforced by terminating the
  job.

Resource-limit exits use the `resource_limit` failure code. Unexpected exits do
not restart automatically, which prevents crash loops from consuming the full
desktop. The next user-initiated launch is delayed per plugin by 2, 4, 8, 16,
then 30 seconds; a healthy first frame resets the sequence, while a five-minute
failure-free window also clears stale failures. The remaining boundary is an
actual OS security sandbox: children still inherit the current user's
filesystem, network, registry, and token access.

### Evidence

- `cargo fmt --all --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` passed.
- `cargo test --workspace` passed with 139 tests and 1 intentionally ignored
  Windows Search integration test before the resource-limit tests; the current
  workspace run passes 144 tests with the Windows Search integration test and
  the release route benchmark intentionally ignored.
- A real isolated child loaded the development-signed Calendar package,
  emitted `rendered` and `healthy` with the expected camelCase protocol shape,
  accepted `close`, and exited with code 0.
- An isolated child with no matching package emitted a flushed `failed`
  (`not_found`) event and exited cleanly, covering startup-error propagation.
- The Job Object test assigns a real child process, verifies its handle query,
  and confirms that closing the job terminates the child.
- `scripts/test-plugin-isolation.ps1` records a forced child crash, verifies
  the shell survives, and verifies the repeated launch is rejected by backoff.
- `scripts/bench-plugins.ps1` now records the plugin-host PID, containment
  limits, peak host RSS, aggregate shell plus host RSS, and peak handle count.
- The optimized Release build passed 20-sample measurements for all official
  plugins: first-frame p95 was 332.17/328.14/340.63 ms, focus p95 stayed below
  0.7 ms, host peak handles stayed at or below 438, and hidden VMs/hosts were
  zero.
- Opening all three plugins in one Release shell produced exactly three host
  processes and 387.95 MiB aggregate peak RSS. Terminating the shell left zero
  orphaned hosts.
- The Release forced-crash run survived, emitted `runtime_error`, returned the
  host count to zero, and rejected the repeated launch with 695 ms remaining in
  the 2-second first backoff.
- A test-only 32 MiB memory cap terminated the plugin host, kept the shell
  alive, returned host count to zero, and emitted `resource_limit`.

`P3-PROCESS-8` remains open only for clean Windows VM installation/upgrade and
production-root acceptance; its development-machine Release measurements are
recorded in `benchmarks.md`.

## Verification

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm lint
pnpm format:check
pnpm typecheck
pnpm build
nsh test packages/plugins/calendar
nsh test packages/plugins/clipboard-history
nsh test packages/plugins/ui-showcase
```

## Decision Log

- 2026-11-08: phases 1 and 2 intentionally retain the in-process NSH runtime.
- 2026-11-20: phase 3 starts one `--plugin-host-process` child per open plugin;
  this supersedes the in-process runtime decision for plugin windows.
- 2026-11-08: no schema or plugin API migration is part of this work.
- 2026-11-08: the application keeps one SQLite file during the storage split;
  repository ownership is separated inside the crate.
