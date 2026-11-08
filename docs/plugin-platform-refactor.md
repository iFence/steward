# Steward Plugin Platform Refactor

> Status: implementation tracked here. This document covers phases 1 and 2 of
> the plugin-platform architecture work. Process isolation, marketplace
> admission, host API negotiation, and user-managed capability grants remain
> phase 3 and phase 4 work.

## Invariants

- Release builds load verified signed packages only.
- Route discovery never starts a plugin VM.
- No plugin VM exists without an open plugin window.
- Search continues to use `nucleo`; this work does not add a second matcher.
- The current `plugin.json` schema, launch parameters, and TypeScript helper
  surface remain compatible.
- Phases 1 and 2 do not make third-party plugins safe to admit. Plugin VMs
  remain in the Steward process until phase 3.

## Status

| Area | Status |
|---|---|
| Phase 1: launch and diagnostics hardening | Complete |
| Phase 2: responsibility and dependency split | Complete |
| Phase 3: one process per plugin window | Not started |
| Phase 4: third-party marketplace admission | Not started |

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

- `cargo test --workspace`: 73 tests passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- Launch work is owned by `plugin-host::launcher` and started through
  `PluginHost::start_launch`; the UI thread receives a bounded worker channel.
- Receipt acceptance is emitted by the plugin window after `run_package` and a
  250 ms no-fatal-error grace period.
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
- Shared search and contract crates are workspace members with unit tests.
- Route regexes are compiled during indexing and query results are capped.
- `events.rs` schedules handlers; domain commands live in `handlers.rs`.
- `PluginWindowManager` owns window lookup, insertion, removal, and close
  cleanup.
- Storage delegates settings, application cache, clipboard, and schema logic
  to separate internal modules without changing its public API.

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
- 2026-11-08: no schema or plugin API migration is part of this work.
- 2026-11-08: the application keeps one SQLite file during the storage split;
  repository ownership is separated inside the crate.
