# Steward NSH Migration Plan

> Status: NSH-0 through NSH-3 complete; NSH-4 implementation complete, blocked
> on clean-VM and production-artifact acceptance.
>
> This is the canonical migration plan, decision record, and release ledger.
> Milestone changes must update this file with the changed files, verification
> commands, results, deviations, and any remaining release gates.
>
> Phases 1 and 2 of the follow-up plugin-platform work are complete. Phase 3
> process isolation implementation is complete and blocked on clean-VM
> acceptance, tracked in
> [plugin-platform-refactor.md](plugin-platform-refactor.md).

## Why NSH

The pre-v0.3.0 plugin stack was a purpose-built system: TypeScript was bundled
by esbuild into a serialized JSON View or Virtual UI Tree, sent over a custom
JSON-RPC channel, and rendered by Steward. It worked, but Steward owned both
the UI schema and the runtime bridge. Every richer interaction expanded that
private protocol.

NSH replaces that vertical stack with a maintained Rust UI engine and TSX
application runtime:

- Steward's launcher uses NSH Rust components and its settings window uses
  GPUI Kit's `Settings` component, so the shell has no plugin VM.
- Plugins are first-class NSH applications with native components, signals,
  input handling, and window lifecycle.
- Steward keeps only the product-specific pieces: search, routing, clipboard
  history, package trust, and a small typed host bridge.
- Signed `.nshpkg` packages remove the old custom plugin runtime binary and
  make release verification part of the package format.

## Trade-offs

| Area | Advantages | Costs and risks |
|---|---|---|
| UI capability | Full native TSX UI instead of a fixed view vocabulary | Larger runtime/API surface to learn |
| Maintenance | Reuses NSH components, bundler, package format, and tests | NSH is young; exact revisions and compatibility checks matter |
| Memory | Launcher starts no JavaScript VM; only open plugin windows allocate VMs | Each plugin host process still costs a 64 MiB VM allowance plus application assets |
| Isolation | One process and VM per open plugin, per-VM quotas, Job Object resource limits, and signed packages | Runs as the current Windows user; not yet an OS filesystem/network sandbox |
| Distribution | Standard signed package with certificate, revocation, and rollback checks | Production key ceremony and secret provisioning are required |
| Migration | Direct cutover removes two protocols and the old runtime | Existing plugins and plugin storage are not compatible |
| Performance | Route discovery is metadata-only and does not start a VM | Plugin first-frame performance depends on NSH startup and is still a release gate |

## Decisions

1. Steward's launcher is an NSH Rust root rendered by `nsh-platform-gpui`;
   the settings window is a GPUI Kit view with `Root` and `Settings`.
2. The product uses GPUI Fast directly and pins:
   - `gpui-fast = =0.1.17`
   - `gpui-fast-platform = =0.1.17`
   - `gpui-kit = =0.10.0` with its `gpui-fast` feature, so NSH and the
     settings components share one GPUI backend.
3. NSH crates are pinned to published version `0.6.1`. The published
   `nsh-platform-gpui` crate works with GPUI Fast's blanket `IntoElement`
   implementation, so Steward does not vendor a compatibility patch.
4. Each plugin command opens one full NSH application window. A second command
   for the same plugin ID focuses the existing window and emits
   `steward.command`.
5. `plugin.json` schema v2 is routing metadata only. Executable code and
   capability policy live in the signed `plugin.nshpkg`.
6. The cutover is intentionally breaking. JSON Views, Virtual UI Trees,
   dynamic triggers, inline plugin results, detached panels, dock workspaces,
   the old runtime, IPC crate, and esbuild plugin build are removed.
7. Release loads only verified packages. The release trust root is read from
   `assets/nsh/trust-roots.json`; an empty file fails closed.
8. Windows is the acceptance platform. Other `cfg` branches remain buildable
   but are not part of functional acceptance.
9. The release capability policy is deny-by-default. It grants clipboard,
   `${dataDir}` filesystem access, https/mailto URL opening, and `steward.*`
   host calls. Network, process, webview, and picker capabilities are denied.

## Target Architecture

```text
Windows app process
  |
  +-- GPUI Fast
        |
        +-- NSH Rust launcher
        |     +-- application search and built-in commands
        |     +-- plugin.json route catalog
        |
        +-- GPUI Kit settings
        |     +-- Settings pages, groups, items, and fields
        |
        +-- NSH plugin host process (one per open plugin window)
              +-- steward-app --plugin-host-process
              +-- signed plugin.nshpkg
              +-- one QuickJS VM
              +-- host bridge: clipboard history, close, command, theme
```

## Public Contracts

### Plugin layout

```text
plugins/<plugin-id>/
  plugin.json       Steward routing metadata
  plugin.nshpkg     signed NSH application package
```

Schema v2 manifest:

```json
{
  "schemaVersion": 2,
  "id": "it.aspirin.calendar",
  "name": "Calendar",
  "version": "0.1.4",
  "icon": "<svg ...>",
  "package": "plugin.nshpkg",
  "commands": [
    {
      "name": "calendar",
      "title": "Calendar",
      "keywords": ["calendar"],
      "trigger": { "type": "command" }
    }
  ]
}
```

Package `nsh.json` ID and version must match `plugin.json`.

### VM limits

| Limit | Value |
|---|---:|
| JS heap | 64 MiB |
| Interpreter stack | 1 MiB |
| Bundle load | 2 seconds |
| One JS call | 250 ms |
| UI nodes | 20,000 |

### Host bridge

Launch parameters:

```json
{
  "pluginId": "it.aspirin.calendar",
  "command": "calendar",
  "input": "calendar",
  "locale": "en",
  "accent": "#863bff",
  "dark": true
}
```

Host calls:

- `steward.clipboard.history` -> clipboard entry array
- `steward.clipboard.write` -> `{}` after writing `{ "text": "..." }`
- `steward.storage.get` -> plugin-private string or `null`
- `steward.storage.set` -> `{}`
- `steward.window.close` -> `{}`

Host events:

- `steward.command` -> `{ "command": "...", "input": "..." }`
- `steward.theme` -> `{ "accent": "#...", "dark": true }`

## Milestones

| Milestone | Status | Scope |
|---|---|---|
| NSH-0 | complete | Decisions, contracts, and migration ledger |
| NSH-1 | complete | GPUI Fast/NSH dependency and embedding proof |
| NSH-2 | complete | NSH Rust launcher and search; GPUI Kit settings |
| NSH-3 | complete | Registry, host, SDK, and official plugin cutover |
| NSH-4 | blocked | Implementation and local Release measurements complete; clean-VM and production-artifact acceptance unavailable |

### NSH-0: Migration record

Files:

- `docs/nsh-migration.md`
- `docs/architecture.md`
- `docs/roadmap.md`
- `docs/plugin-manifest-spec.md`
- `docs/extension-api.md`

Acceptance:

- Decisions and public contracts are recorded.
- Legacy milestones are marked historical.
- The architecture decision record links to this file.

Evidence: this ledger records the exact dependency deviation, package contract,
capability policy, rollback rule, and release gates.

### NSH-1: GPUI Fast and NSH embedding

Files:

- `Cargo.toml`
- `Cargo.lock`
- `crates/app/Cargo.toml`
- `crates/app/src/nsh_surface.rs`
- `crates/app/examples/nsh_smoke.rs`
- `.github/workflows/ci.yml`

Acceptance:

- GPUI Fast and NSH are pinned in the workspace.
- A GPUI Fast window can contain an NSH `AppView`.
- The shipped app compiles on the pinned Rust toolchain.

Evidence:

- `cargo check --offline -p steward-app`: passed.
- The launcher and three signed plugins opened as NSH surfaces in local smoke
  runs on Windows.
- Dependency refresh (2026-11-16): GPUI Fast and its platform crate moved from
  0.1.9 to 0.1.16, GPUI Kit moved from 0.7.3 to 0.10.0, and NSH moved from the
  git revision to published 0.5.1. `cargo check -p steward-app`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` passed.
- Official plugins migrated to NSH 0.5.1's root module exports and typed
  change events. `nsh test` passed for all three plugins, and
  `pnpm lint`, `pnpm format:check`, `pnpm typecheck`, and `pnpm build` passed
  with the isolated NSH CLI 0.5.1.
- Dependency refresh (2026-11-20): NSH crates and CLI moved from 0.5.1 to
  0.6.1, GPUI Fast and its platform crate moved from 0.1.16 to 0.1.17, and the
  local `nsh-platform-gpui` compatibility patch was removed because upstream
  0.6.1 contains the required `IntoElement` fix. `KeyOptions` gained the
  required `for_focus` field; Steward's launcher keys are ordinary view
  bindings, so `bind_launcher_key` sets `for_focus: false`.
- The 0.6.1 validation passed with one copy each of `nsh-platform-gpui` and
  `gpui-fast`: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `pnpm lint`, `pnpm format:check`,
  `pnpm typecheck`, and `pnpm build`.
- NSH 0.6.1 `nsh check` passed for all three official plugins. `nsh test`
  passed Calendar (13 tests), Clipboard History (1 test), and UI Showcase
  (1 test). The matching 0.6.1 `nsh-gpui-host` built successfully.

### NSH-2: Steward shell

Files:

- `crates/app/src/main.rs`
- `crates/app/src/window.rs`
- `crates/app/src/platform.rs`
- `crates/app/src/shell.rs`
- `crates/app/src/settings.rs`
- `crates/app/src/services.rs`
- `crates/app/src/events.rs`
- `crates/app/src/hotkeys.rs`
- `crates/app/src/state.rs`
- `crates/ui-components/src/*`

Acceptance:

- The launcher is an NSH Rust root; settings use GPUI Kit `Root` and `Settings`.
- Application search, plugin routes, calculator, URL open, keyboard navigation,
  IME, resize, summon/hide, tray, hotkeys, language, accent, autostart, and
  summon-hotkey editing are wired.
- Old GPUI component/UI tree renderers are gone.

Evidence:

- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace`: passed.
- `cargo check -p steward-app` and workspace tests pass.
- Settings persist language, accent, autostart, and summon hotkey changes.
- `STEWARD_SHOW_SETTINGS=1` opens the GPUI Kit settings window.

### NSH-3: Plugin cutover

Files:

- `crates/plugin-registry/src/lib.rs`
- `crates/plugin-registry/src/manifest.rs`
- `crates/plugin-host/src/lib.rs`
- `crates/plugin-host/src/route.rs`
- `crates/app/src/plugin_window.rs`
- `packages/extension-api/src/host.ts`
- `packages/plugins/*`
- `scripts/gen-test-plugins.sh`

Acceptance:

- Manifest v2 discovery and routing do not start a VM.
- A verified package opens one full plugin window.
- A repeat command focuses the existing window and emits `steward.command`.
- Package ID/version mismatches, missing trust roots, and invalid packages are
  rejected before creating a window.
- Capability grants, OOM, call timeout, and node quota are enforced by the
  engine limits in `nsh_surface.rs`.
- The old runtime, IPC crate, esbuild bundler, and old SDK are removed.

Evidence:

- Registry tests cover scan, cache, incremental update, missing package, strict
  schema, old-field rejection, and unsafe package paths.
- Route tests cover command, prefix, regex, precedence, and longest-prefix
  matching.
- `nsh test` passed for Calendar, Clipboard History, and UI Showcase (3/3).
- Local Windows smoke tests opened all three signed plugin windows.
- The final smoke pass opened Calendar, Clipboard History, and UI Showcase
  windows from development-signed packages after the namespace cleanup.

### NSH-4: Release

Completed files:

- `crates/plugin-host/src/lib.rs`
- `assets/nsh/trust-roots.json`
- `scripts/package-plugins.ps1`
- `scripts/package-msi.ps1`
- `crates/app/wix/main.wxs`
- `package.json`
- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `docs/benchmarks.md`
- `docs/windows-acceptance.md`
- `docs/tinycast-parity-plan.md`
- `crates/app/src/launcher_model.rs`
- `crates/app/src/hotkey_registry.rs`
- `crates/app/src/settings_registry.rs`
- `crates/app/src/metrics.rs`
- `crates/storage/src/schema.rs`
- `crates/storage/src/capabilities.rs`
- `scripts/verify-trust-roots.ps1`
- `scripts/verify-release-plugins.ps1`
- `scripts/bench-plugins.ps1`
- `scripts/test-msi-lifecycle.ps1`
- `README.md`
- `README_CN.md`
- `AGENTS.md`
- `Changelog.md`
- `Changelog-zh-CN.md`
- `THIRD_PARTY_NOTICES.md`
- `assets/licenses/*`

Completed acceptance:

- Plugin zips contain exactly `plugin.json` and `plugin.nshpkg`.
- Package verification succeeds with a development root.
- The MSI definition no longer installs or stops `steward-plugin-runtime.exe`.
- CI runs Rust checks, TypeScript checks, NSH plugin tests, and builds.
- Apache-2.0 notices ship with the repository.
- MSI XML was parsed successfully.
- `cargo build -p steward-app` passed.
- A full optimized release build of `steward-app` passed, and the release
  binary opened a development-signed Calendar window.
- `pnpm lint`, `pnpm format:check`, `pnpm typecheck`, and `pnpm build` passed.
- `nsh verify` accepted all three packages with the development root.
- `scripts/package-msi.ps1 -SkipBuild` produced
  `target/wix/steward-app-0.3.0-x86_64.msi` (16.0 MB). The split notices
  components fixed WiX `LGHT0367`; decompilation showed the app and license
  files, with no plugin runtime or file-index helper executable.
- The production Ed25519 root and release certificate are configured, the
  release secrets are provisioned, and `assets/nsh/trust-roots.json` contains
  the production root public key.
- Manual `Build and Release` run 42953960819 completed successfully. Its MSI
  and plugin archives matched their published SHA-256 checksums, and all three
  packaged plugins verified against the production root.
- P0 foundation follow-up (2026-11-17): launcher contributions now produce a
  unified `LauncherItem`/`ActionId` model; hotkeys and settings have
  duplicate/conflict-aware registries; SQLite has explicit schema v2 migration;
  capability grants, revocation, and audit records are persisted.
- NSH 0.5.1 compatibility was restored for all official plugin sources by
  replacing removed `nsh/clipboard`, `nsh/storage`, and `nsh/host` imports with
  Steward host calls for clipboard write and plugin-private storage.
- Calendar completed 20 release samples at 349.1 ms first-frame p95 and 5.4 ms
  focus p95. Clipboard History and UI Showcase completed two-sample smoke
  measurements at 327.9 ms and 336.2 ms first-frame p95. Tray startup reported
  zero hidden plugin VMs.
- Resident release smoke recorded 472 ms to tray readiness and 46 ms summon p95
  over 20 close/summon samples. See `docs/benchmarks.md` for limitations.
- The Windows acceptance matrix and automated trust, package, lifecycle, and
  performance scripts are checked in.
- Release hardening follow-up (2026-11-21): CI and release packaging now use
  the pinned NSH CLI `0.6.1` instead of the stale `0.5.1`; CI checks
  `cargo fmt --all --check` and `pnpm format:check`; and cargo-wix is pinned to
  `0.3.9` for reproducible MSI builds.
- Build and Release run `44029877843` completed successfully on commit
  `f91f19436c6cead116b0eb3a1fd22ed5ddb64a7e`. The workflow used the published
  NSH CLI 0.5.1, rebuilt and production-signed all three official plugins,
  verified every archive against the production root, and built the v0.3.1 MSI.
  Downloading the artifacts and rechecking their SHA-256 manifests and
  production signatures also passed locally.

### Omission Audit (2026-11-18)

Files:

- `crates/app/src/plugin_window.rs`
- `crates/app/src/plugin_runtime.rs`
- `crates/app/src/services.rs`
- `crates/app/src/launcher_model.rs`
- `crates/app/src/search/mod.rs`
- `crates/plugin-registry/src/manifest.rs`
- `crates/storage/src/lib.rs`

Results:

- Plugin receipt acceptance requires an observed first NSH frame plus the
  fatal-error grace period, with session IDs isolating timers and errors across
  window reuse.
- Calendar host calls use a bounded worker instead of one detached thread per
  call.
- Storage no longer depends on `core-engine`, launcher results use one
  `LauncherItem` source model, plugin runtime coordination is separate from
  `SearchState`, and manifest IDs/versions are strictly validated.
- Every launcher result now has an icon: native Windows shell icons with
  product fallbacks for applications, files, and settings; manifest SVG with a
  generic fallback for plugins; and Steward-owned SVG icons for calculator and
  URL actions. UI Showcase now ships an inline SVG icon.
- `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` passed; the test run reported 137 passed and 1
  ignored.

Blocked release gates:

- Install the generated release MSI on a clean Windows machine.
- Perform a signed package installation smoke test using the production trust
  root.
- Execute the automated plus manual matrix in `docs/windows-acceptance.md` on a
  clean Windows 11 23H2+ x64 VM.

### Plugin Process Isolation (2026-11-20, implementation complete, blocked)

Files:

- `crates/app/src/main.rs`
- `crates/app/src/events.rs`
- `crates/app/src/handlers.rs`
- `crates/app/src/plugin_containment.rs`
- `crates/app/src/plugin_window.rs`
- `crates/app/src/state.rs`
- `crates/app/src/clipboard_history.rs`
- `crates/plugin-contract/src/lib.rs`
- `crates/plugin-host/src/lib.rs`
- `scripts/bench-plugins.ps1`
- `scripts/bench-plugin-set.ps1`
- `scripts/test-plugin-isolation.ps1`
- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `docs/plugin-platform-refactor.md`
- `docs/architecture.md`
- `docs/roadmap.md`

Results:

- The shell starts `steward-app --plugin-host-process` once per open plugin and
  no longer owns an NSH `AppView` or plugin VM.
- Parent/child control and lifecycle messages are versioned and session-scoped;
  the child independently verifies the signed package before creating a window.
- The receipt remains unaccepted until the child emits a real render followed
  by a successful fatal-error grace period.
- Command, focus, theme, and close actions are routed to the existing child.
  Unexpected pre-health or nonzero exits are surfaced as structured notices.
- Every child is assigned to a Windows Job Object before receiving its start
  message. The job enforces 256 MiB process/job memory, a 50% hard CPU-rate cap,
  one active process, and kill-on-close. The shell checks the child's handle
  count every 500 ms and terminates the job above 1,024 handles.
- Unexpected exits do not auto-restart. A subsequent user launch is gated by
  per-plugin 2/4/8/16/30-second exponential backoff, reset by a healthy first
  frame or after five minutes without a new failure.
- Container metrics include the child PID, configured limits, peak host RSS,
  aggregate shell plus host RSS, peak handle count, and orphan-process checks.
- `scripts/bench-plugin-set.ps1` opens all three official plugins in one shell
  so aggregate RSS and the zero/one/three-host process-count gate can be
  recorded without estimating three independent runs.
- `scripts/test-plugin-isolation.ps1` records forced-crash containment and the
  rejected immediate retry. The Release run passed locally with a temporary
  signed Calendar package: the shell survived, host count returned to zero, and
  the retry was rejected with 695 ms remaining in the first backoff.
- A 32 MiB test-only memory limit terminated the Calendar host while leaving
  the shell alive and host count at zero, with failure code `resource_limit`.
- Release measurements (2026-11-21, local development sandbox) recorded 447 ms
  cold start, Calendar/Clipboard/UI Showcase first-frame p95 of
  332.17/328.14/340.63 ms, sub-millisecond focus p95, zero hidden VMs/hosts,
  and 387.95 MiB aggregate peak RSS with all three plugin windows open. The
  final 20-sample summon run measured 60 ms p95, above the 50 ms gate, so this
  remains a clean-VM gate rather than an accepted local result.
- Release route scaling passed at 100/500/1000 synthetic plugins: cached
  metadata reads were 2.45/3.74/4.62 ms, route builds 2.07/6.59/13.81 ms, and
  all exact/prefix/regex query p95 values stayed below 0.17 ms.
- `cargo test -p steward-app --bin steward-app --no-fail-fast` passed with 79
  tests (78 passed, 1 intentionally ignored), including a real Windows Job
  Object child-lifetime test and restart-backoff unit tests.
- `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` passed; the workspace run reported 144 tests with
  the Windows Search integration test and release route benchmark intentionally
  ignored.
- `pnpm lint`, `pnpm format:check`, `pnpm typecheck`, and `pnpm build` passed
  with NSH CLI 0.6.1. The three plugin test suites were also run from clean
  temporary copies and passed: Calendar 13, Clipboard History 1, UI Showcase 1.
- `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` passed. An isolated Calendar child emitted
  `rendered` then `healthy` and exited 0 after `close`; a missing-package child
  flushed `failed/not_found`.

Remaining blocked:

- Blocked: run signed-package install/upgrade/crash acceptance on a clean
  Windows VM.

## Verification Commands

Rust:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

TypeScript and plugins:

```text
pnpm lint
pnpm format:check
pnpm typecheck
pnpm build
nsh test packages/plugins/calendar
nsh test packages/plugins/clipboard-history
nsh test packages/plugins/ui-showcase
```

Packaging:

```text
./scripts/package-plugins.ps1
./scripts/package-msi.ps1
```

Process isolation and release measurements:

```text
./scripts/bench-resident.ps1 -Samples 20
./scripts/bench-plugins.ps1 -Plugin calendar -Iterations 20
./scripts/bench-plugin-set.ps1
./scripts/test-plugin-isolation.ps1 -Plugin calendar
./scripts/test-plugin-isolation.ps1 -Plugin calendar -MemoryLimitMiB 32
cargo test -p steward-plugin-host --release synthetic_plugin_route_scaling -- --ignored --nocapture
```

Pass `-PackagePath` and `-TrustRootFile` to the plugin scripts when measuring
production-signed artifacts from outside the development workspace.

The repository-level `.cargo/config.toml` selects the `rsproxy` registry so
plain Cargo commands can resolve dependency versions that the local global
mirror has not synchronized yet. Pass Cargo's `--config` options to override
the mirror when needed.

## Performance Gates

| Metric | Gate |
|---|---:|
| Cached cold start | previous release + 500 ms or less |
| Summon p95 | 50 ms or less |
| Idle RSS increase | 32 MiB or less |
| Official plugin first frame p95 | 500 ms or less |
| Existing plugin focus p95 | 50 ms or less |
| Hidden plugin VMs | zero |

The current benchmark file contains functional smoke evidence, not production
release measurements. NSH-4 remains open until the gates above are measured.

## Accepted Deviations

- Plugin VMs and NSH windows run in one child process per open plugin. Child
  crashes no longer tear down the launcher; signed packages, the capability
  ceiling, and hard engine quotas remain mandatory. Children still run as the
  current Windows user until OS-level containment is added.
- `STEWARD_SHOW_ON_START`, `STEWARD_SHOW_SETTINGS`, `STEWARD_SMOKE_PLUGIN`,
  `STEWARD_SMOKE_COMMAND`, `STEWARD_SMOKE_TITLE`, and
  `STEWARD_SMOKE_PLUGINS_JSON` are developer-only smoke hooks.
  `STEWARD_PLUGIN_MEMORY_LIMIT_MIB`, `STEWARD_PLUGIN_CPU_RATE_PERCENT`, and
  `STEWARD_PLUGIN_HANDLE_LIMIT` are clamped developer-only containment test
  overrides; production defaults are 256 MiB, 50%, and 1,024 handles.
  `STEWARD_DATA_DIR` consistently redirects app data, plugin data, and clipboard
  storage for isolated tests.
- No migration commit or tag is created by this working-tree implementation.
  Milestone evidence is tied to the listed files and test results.

## Known Risks Before Release

- Package verification and receipt handling are now asynchronous: launch work
  runs on a worker and a receipt is accepted only after the first frame plus a
  250 ms fatal-error grace period. See
  [plugin-platform-refactor.md](plugin-platform-refactor.md).
- Automated coverage verifies routing, manifests, signing, headless plugin
  behavior, and process-level window opening. Interactive settings controls,
  IME, and input-driven hotkey replacement still need a Windows UI acceptance
  pass.

## Rollback

Before the first NSH release, revert the migration change set as a group. After
release, reinstall the previous MSI and previous plugin artifacts. Plugin
storage and window layout are intentionally not migrated across this breaking
change.
