# Steward NSH Migration Plan

> Status: NSH-0 through NSH-3 complete; NSH-4 implementation complete, blocked
> on clean-VM and production-artifact acceptance. M4 implementation is complete
> and blocked on the clean-VM, mixed-DPI, and real-IME acceptance run. M5 plugin
> ecosystem v1 is complete. Plugin capability milestones PC-0 through PC-4 are
> complete; public third-party admission is the next target.
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

| Area          | Advantages                                                                                         | Costs and risks                                                                    |
| ------------- | -------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| UI capability | Full native TSX UI instead of a fixed view vocabulary                                              | Larger runtime/API surface to learn                                                |
| Maintenance   | Reuses NSH components, bundler, package format, and tests                                          | NSH is young; exact revisions and compatibility checks matter                      |
| Memory        | Launcher starts no JavaScript VM; only open plugin windows allocate VMs                            | Each plugin host process still costs a 64 MiB VM allowance plus application assets |
| Isolation     | One process and VM per open plugin, per-VM quotas, Job Object resource limits, and signed packages | Runs as the current Windows user; not yet an OS filesystem/network sandbox         |
| Distribution  | Standard signed package with certificate, revocation, and rollback checks                          | Production key ceremony and secret provisioning are required                       |
| Migration     | Direct cutover removes two protocols and the old runtime                                           | Existing plugins and plugin storage are not compatible                             |
| Performance   | Route discovery is metadata-only and does not start a VM                                           | Plugin first-frame performance depends on NSH startup and is still a release gate  |

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
4. Each plugin command opens one isolated plugin-host presentation. Schema-v3
   commands select `windowed` or `embedded`; embedded uses a managed frameless
   hosted surface without moving the VM into the shell. A second command for
   the same plugin ID and mode focuses the existing presentation and emits
   `steward.command`.
5. `plugin.json` schema v2/v3 is routing metadata only. Schema v2 remains
   readable and defaults to `windowed`; schema v3 may select `embedded`.
   Executable code and capability policy live in the signed `plugin.nshpkg`.
6. The cutover is intentionally breaking. JSON Views, Virtual UI Trees,
   dynamic triggers, inline plugin results, detached panels, dock workspaces,
   the old runtime, IPC crate, and esbuild plugin build are removed.
7. Release loads only verified packages. The release trust root is read from
   `assets/nsh/trust-roots.json`; an empty file fails closed.
8. Windows is the acceptance platform. Other `cfg` branches remain buildable
   but are not part of functional acceptance.
9. The release capability policy is deny-by-default. It grants clipboard,
   `${dataDir}` filesystem access, https/mailto URL opening, scoped HTTPS
   fetch, and exact non-shell `.exe` execution. Named Steward host calls and
   resource scopes are intersected with active user grants at launch;
   `steward.*` remains only for legacy official migration. Raw sockets,
   unrestricted network access, shell/interpreter execution, webview, and
   picker capabilities remain denied.

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
              +-- host bridge: storage, preferences, feedback, filesystem,
              |   system-open, scoped HTTPS
              +-- NDJSON execution request -> launcher-owned broker
```

## Public Contracts

### Plugin layout

```text
plugins/<plugin-id>/
  plugin.json       Steward routing and hosted preference metadata
  plugin.nshpkg     signed NSH application package
```

Schema v3 manifest:

```json
{
  "schemaVersion": 3,
  "id": "it.aspirin.calendar",
  "name": "Calendar",
  "version": "0.1.4",
  "icon": "<svg ...>",
  "package": "plugin.nshpkg",
  "preferences": {
    "schemaVersion": 1,
    "fields": [
      {
        "key": "showHints",
        "title": "Show hints",
        "type": "boolean",
        "default": true
      }
    ]
  },
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

| Limit             |     Value |
| ----------------- | --------: |
| JS heap           |    64 MiB |
| Interpreter stack |     1 MiB |
| Bundle load       | 2 seconds |
| One JS call       |    250 ms |
| UI nodes          |    20,000 |

### Host bridge

Launch parameters:

```json
{
  "pluginId": "it.aspirin.calendar",
  "command": "calendar",
  "input": "calendar",
  "locale": "en",
  "accent": "#863bff",
  "dark": true,
  "presentation": "windowed",
  "hostApiVersion": "0.5.0",
  "features": ["host_info", "runtime_feature_discovery"],
  "preferences": {
    "showHints": true
  }
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

| Milestone | Status   | Scope                                                                                                           |
| --------- | -------- | --------------------------------------------------------------------------------------------------------------- |
| NSH-0     | complete | Decisions, contracts, and migration ledger                                                                      |
| NSH-1     | complete | GPUI Fast/NSH dependency and embedding proof                                                                    |
| NSH-2     | complete | NSH Rust launcher and search; GPUI Kit settings                                                                 |
| NSH-3     | complete | Registry, host, SDK, and official plugin cutover                                                                |
| NSH-4     | blocked  | Implementation and local Release measurements complete; clean-VM and production-artifact acceptance unavailable |

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

- Manifest v2/v3 discovery and routing do not start a VM.
- A verified package opens the signed command's windowed or embedded presentation.
- A repeat command in the same presentation mode focuses the existing surface
  and emits `steward.command`.
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

### M4: Windows Support Polish (2026-11-21, implementation complete)

Files:

- `crates/app/src/autostart.rs`
- `crates/app/src/diagnostics.rs`
- `crates/app/src/instance.rs`
- `crates/app/src/main.rs`
- `crates/app/src/platform.rs`
- `crates/app/src/shell.rs`
- `crates/storage/src/lib.rs`
- `scripts/test-windows-support.ps1`
- `docs/windows-acceptance.md`
- `docs/architecture.md`
- `docs/roadmap.md`
- `Changelog.md`
- `Changelog-zh-CN.md`

Acceptance implementation:

- The existing MSI lifecycle script remains the clean install, major upgrade,
  repair, launch, and uninstall gate.
- PerMonitorV2 summon geometry continues to use the monitor under the cursor,
  fits the launcher to that monitor's work area, and now emits target-DPI,
  cross-monitor, and physical-size metrics. Native geometry tests cover clamped
  and negative-coordinate work areas.
- `scripts/test-windows-support.ps1` checks tray/hotkey lifetime, isolated
  single-instance activation, autostart repair and removal, launch geometry on
  every monitor, foreground keyboard focus, Unicode input, navigation events,
  IME context attachment, and structured invalid-package failure.
- The script uses isolated data, mutex/event, and summon-hotkey names. It
  restores the real `HKCU\...\Run\Steward` value in `finally`; production names
  and the persisted summon hotkey remain unchanged when the test environment
  overrides are absent.
- `steward-app.exe --write-diagnostics <path>` produces a read-only support
  report. It includes executable/package hashes, monitor DPI/work areas, DB
  schema, autostart truth, trust-root identity, and recent error lines without
  applying migrations or reading clipboard/plugin-private content.
- The support report is generated before logger and GUI initialization so it
  remains available when the shell cannot start.

Local verification (2026-11-21):

- `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` passed. The workspace run reported 146 passed and 2
  intentionally ignored tests.
- `pnpm lint`, `pnpm format:check`, `pnpm typecheck`, and `pnpm build` passed.
- `nsh test` passed Calendar (13), Clipboard History (1), and UI Showcase (1).
- `scripts/test-windows-support.ps1` passed tray/hotkey creation and
  destruction, isolated single-instance activation, support-report generation,
  plugin-cache inspection, and invalid-package `trust_failure` containment in
  the restricted local desktop session.
- The support suite is documented with `-AllowNonInteractive` for restricted
  sessions; that mode skips cursor control, foreground keyboard input, and IME
  instead of claiming acceptance. Normative P0-A09 runs must omit the switch.
- The regression suite exposed and now covers a failure-reporting
  `RefCell already borrowed` panic: package launch errors are stored before
  borrowing `AppState` again to record metrics and publish the notice.

Remaining blocked:

- Run P0-A09 and real Chinese IME composition on the Windows acceptance VM.
- Record evidence for a mixed-DPI or multi-monitor configuration; a single-DPI
  local run only validates the geometry path, not the acceptance matrix.
- Complete the clean-VM P0-A07/P0-A08 MSI install, upgrade, repair, and uninstall
  lifecycle run.

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

### M5: Plugin Ecosystem v1 (2026-11-21, implementation complete)

Files:

- `Cargo.toml`
- `crates/plugin-store/*`
- `crates/plugin-cli/*`
- `crates/app/src/plugin_management.rs`
- `crates/app/src/settings.rs`
- `crates/app/src/services.rs`
- `crates/app/src/plugin_runtime.rs`
- `crates/app/src/main.rs`
- `crates/app/i18n/*/main.ftl`
- `scripts/check-plugin-policies.ps1`
- `.github/workflows/release.yml`
- `docs/plugin-ecosystem.md`
- `docs/architecture.md`
- `docs/roadmap.md`
- `Changelog.md`
- `Changelog-zh-CN.md`

Acceptance:

- [x] `steward-plugin new` scaffolds an NSH TSX plugin with manifest, package,
      capability, TypeScript, and build metadata.
- [x] package inspection reports signature validity, trusted-root status,
      manifest/package identity, build, minimum runtime, package hash, publisher
      release-key ID, and release-policy review items.
- [x] Settings > Plugins shows installed signature/trust state and exposes
      install/update/remove/rollback operations through a background worker.
- [x] installs and updates require the two-file distribution contract and a
      newer semantic version/build; activation uses staged writes and preserves up
      to five verified rollback versions.
- [x] rollback re-verifies the retained package and remains fail-closed under
      NSH's accepted-build anti-downgrade rule.
- [x] local marketplace archives are verified before indexing; artifact and
      package hashes plus publisher key ID are checked again at installation.
- [x] publisher identity is the certified NSH release-key ID, and root-signed
      revocation lists can be created and applied to the shared persistent trust
      store.
- [x] package policy checks run in Rust tests and against every
      production-signed archive in the release workflow.

Implementation is local-first. A hosted public marketplace, publisher
registration service, and automatic public-marketplace admission policy remain
later platform work.

Local verification (2026-11-21):

- `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` passed. The workspace run reported 152 passed and 2
  intentionally ignored tests.
- The new plugin-store integration suite passed 6/6: scaffold generation,
  signature/publisher/policy inspection, install/update/rollback history,
  local marketplace installation, revocation enforcement, and rejection of
  extra archive files.
- `pnpm lint`, `pnpm format:check`, `pnpm typecheck`, and `pnpm build` passed.
- `nsh test` passed Calendar (13), Clipboard History (1), and UI Showcase (1).
- `steward-plugin new` produced the complete scaffold, and `nsh check App.tsx`
  passed against that generated output.
- `scripts/check-plugin-policies.ps1` passed PowerShell parser validation. Its
  production-archive run remains part of the release workflow because the
  current checkout has no production-signed plugin archives.

### PC-1: Preferences and User Feedback (2026-11-21, complete)

Files:

- `crates/plugin-registry/src/{lib.rs,manifest.rs}`
- `crates/storage/src/{lib.rs,preferences.rs,schema.rs}`
- `crates/plugin-contract/src/{lib.rs,capabilities.rs}`
- `crates/plugin-host/src/{authorization.rs,launcher.rs,lib.rs}`
- `crates/plugin-store/src/package.rs`
- `crates/app/src/{handlers.rs,launcher_model.rs,plugin_management.rs,plugin_window.rs,settings.rs,state.rs}`
- `crates/app/i18n/*/main.ftl`
- `packages/extension-api/src/host.ts`
- `packages/plugins/ui-showcase/{App.tsx,App.test.tsx,nsh.json,plugin.json}`
- `docs/{architecture.md,extension-api.md,nsh-plugin-api.md,plugin-capability-roadmap.md,plugin-development-zh-CN.md,plugin-host-api-compatibility.md,plugin-manifest-spec.md}`
- `Changelog.md`
- `Changelog-zh-CN.md`

Acceptance:

- [x] `plugin.json` v2 carries a bounded preference schema with text, boolean,
      select, folder, and application fields, defaults, validation, and migrations.
- [x] Preferences are keyed by plugin ID in SQLite, isolated across plugins, and
      preserved across package updates.
- [x] Settings renders and edits declared preferences in the shell process
      without loading QuickJS.
- [x] Invalid values fail before persistence or host injection.
- [x] Folder and application values use absolute-local-path validation; PC-2
      remains responsible for persistent path grants and open APIs.
- [x] Toast and confirmation cross the isolated child process protocol; the
      launcher owns confirmation resolution.
- [x] Progress calls are bounded and delivered back as `steward.progress`
      events.
- [x] UI Showcase declares all five preference kinds and exercises toast,
      confirmation denial, and progress in headless NSH tests.
- [x] The public host contract advances to `0.2.0` with stable invalid-input,
      cancellation, confirmation-denial, and I/O error codes.

Verification:

- `cargo fmt --all --check` passed.
- `cargo clippy --workspace --all-targets -- -D warnings` passed.
- `cargo test --workspace`: 173 passed, 2 intentionally ignored.
- `cargo test -p steward-plugin-registry`: 8 passed.
- `cargo test -p steward-storage`: 14 passed.
- `cargo test -p steward-plugin-contract`: 8 passed.
- `cargo test -p steward-plugin-host --lib`: 13 passed, 1 ignored benchmark.
- `@steward/extension-api` TypeScript check, ESLint, and Prettier checks passed.
- `nsh check App.tsx` and release builds passed for all three official plugins.
- `nsh test .`: Calendar 13, Clipboard History 1, UI Showcase 3; all passed.

### PC-2: Scoped Filesystem and System Resources (2026-11-21, complete)

Files:

- `crates/plugin-contract/src/{lib.rs,capabilities.rs}`
- `crates/plugin-host/src/authorization.rs`
- `crates/plugin-store/src/package.rs`
- `crates/app/src/{main.rs,launch.rs,plugin_filesystem.rs,plugin_management.rs,plugin_system.rs,plugin_window.rs}`
- `packages/extension-api/{package.json,src/host.ts}`
- `packages/plugins/local-files/*`
- `packages/plugins/git-repos/*`
- `docs/plugin-capability-roadmap.md`

Acceptance:

- [x] Native folder and file pickers create persistent grants for the exact
      canonical selection. A read-write folder grants both read and write access.
- [x] Every filesystem read, write, walk, open, reveal, and open-with call
      reloads active grants and revalidates the canonical path at execution time.
- [x] UNC paths and roots outside active grants are rejected. Walks never
      descend through symbolic links, junctions, or other Windows reparse points.
- [x] Walks are bounded by result count, depth, deadline, and caller-controlled
      cancellation, and run outside the QuickJS UI thread.
- [x] Dynamic path grants appear in Settings and can be revoked while the
      plugin process stays open.
- [x] Path open and reveal require read access. Open-with accepts only one
      installed-application ID plus one selected path, never a shell command line.
- [x] Installed-application discovery reuses Steward's existing Start
      Menu/AppsFolder catalog.
- [x] The public host contract advances to `0.3.0`.
- [x] `@steward/plugin-local-files` and `@steward/plugin-git-repos` consume the
      same APIs from unrelated workloads and pass headless host-mocked tests.

Remaining PC-2 boundary:

- Filesystem operations run as the current Windows user. Canonicalization and
  reparse-point checks prevent grant escapes and traversal, but do not turn the
  plugin host into an OS-level sandbox against a same-user attacker.
- Destructive recycle-bin operations remain later filesystem work.

Local verification (2026-11-21):

- `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` passed. The workspace run reported 188 passed and 2
  intentionally ignored tests.
- `pnpm lint`, `pnpm format:check`, `pnpm typecheck`, and `pnpm build` passed
  for all five official plugins.
- `nsh test` passed Calendar (13), Clipboard History (1), Local Files (1), Git
  Repositories (1), and UI Showcase (3).
- Filesystem tests cover active-grant reads, live revocation, traversal and UNC
  rejection, write grants, a directory symlink escape, bounded walks,
  cancellation, deadlines, Unicode and extended-length paths, case-insensitive
  duplicate roots, and shared use by two plugin identities.

### PC-3: Network and Controlled Execution (2026-11-21, complete)

Files:

- `crates/plugin-contract/src/{lib.rs,capabilities.rs}`
- `crates/plugin-host/src/{authorization.rs,policy.rs}`
- `crates/plugin-store/src/package.rs`
- `crates/app/src/{plugin_network.rs,plugin_execution.rs,plugin_window.rs}`
- `packages/extension-api/src/host.ts`
- `packages/plugins/{network-check,git-repos}/`

Delivered:

- [x] Host API `0.4.0` adds `steward.net.fetch`, `steward.exec.run`, and
      `steward.exec.cancel`.
- [x] HTTPS grants are scoped by host, method, optional exact path or path
      prefix, and optional port. Only HTTPS 443 is admitted by the release
      profile.
- [x] Every initial request and redirect reloads active grants. Redirects are
      capped at five; origin changes remove authorization and cookie headers.
- [x] Request bodies, response bodies, headers, timeout, concurrency, queue
      depth, and redirects are bounded. IP literals, localhost/`.local`/
      `.internal`, embedded credentials, raw sockets, and reserved headers are
      rejected before I/O.
- [x] Package policy, launch authorization, and runtime broker accept only
      exact `.exe` targets and reject shells and common script runtimes.
- [x] Controlled processes cross the isolated process boundary to a
      launcher-owned broker, preserving the plugin host's one-active-process
      limit.
- [x] The broker rechecks active executable and write-root grants, resolves
      `PATH` targets itself, clears inherited environment variables, passes
      arguments as an array without shell interpolation, and enforces argument
      count/bytes, timeout, and combined output limits.
- [x] A kill-on-close Windows Job Object owns each process tree. Timeout,
      cancellation, output exhaustion, plugin exit, and broker shutdown
      terminate descendants.
- [x] Network and process handlers append payload-free capability audit
      records with allowed, denied, failed, or cancelled outcomes.
- [x] `@steward/plugin-network-check` validates scoped HTTPS.
      `@steward/plugin-git-repos` validates `git status`, `git remote`,
      upstream lookup, unpushed-commit counting, and clone through the same
      generic API. Neither adds host-side domain logic.
- [x] Headless NSH tests mock and assert both capabilities. Rust tests cover
      grant intersection, host/method/path matching, local-network and header
      rejection, request limits, shell/interpreter rejection, and Windows
      process-tree termination. A Windows broker integration test creates a
      repository and bare remote, then runs init, status, remote, push,
      upstream-count, and clone through the same argument-array execution path.

Local verification (2026-11-21):

- `cargo check -p steward-app` passed.
- `cargo test -p steward-plugin-contract`, `cargo test -p steward-plugin-host
--lib`, `cargo test -p steward-plugin-store --lib`, and `cargo test -p
steward-app --bin steward-app` passed.
- `nsh check App.tsx` passed for Network Check and Git Repositories.
- `nsh test .` passed Network Check (1) and Git Repositories (2).

Remaining PC-3 boundary:

- Controlled processes still run as the current Windows user. The broker limits
  what it starts and owns the resulting tree, but it is not an OS security
  sandbox against a same-user attacker or an approved tool's own subprocess
  behavior.
- Host-owned Recycle Bin removal remains outside the PC-3 implementation.

### PC-4: Embedded Presentation and Navigation (2026-11-21, complete)

Files:

- `crates/plugin-contract/src/{lib.rs,capabilities.rs}`
- `crates/plugin-registry/src/{lib.rs,manifest.rs}`
- `crates/plugin-host/src/{lib.rs,route.rs}`
- `crates/app/src/{plugin_window.rs,handlers.rs,state.rs}`
- `packages/extension-api/src/host.ts`
- `packages/plugins/ui-showcase/`
- `docs/windows-acceptance.md`

Delivered:

- [x] Schema v3 adds optional per-command `windowed` or `embedded`
      presentation while keeping schema v2 readable as `windowed`.
- [x] The verified manifest mode flows through the route index and launch
      request. A mode mismatch is rejected before package verification.
- [x] Host API `0.5.0` and protocol v2 add back, close, title, and visibility
      lifecycle events. Navigation permissions are named, automatic, and
      audited.
- [x] Embedded commands open a launcher-positioned frameless surface in the
      existing isolated plugin-host child process.
- [x] Back, Esc, host-window close, crash, and process shutdown remove the
      presentation. Back/Esc/window close restore and focus the launcher.
- [x] `steward.visibility` reports activation changes to plugin code without
      exposing native window handles.
- [x] UI Showcase `1.1.0` is an embedded consumer and passes headless
      navigation, title, and visibility tests.
- [x] The Windows acceptance matrix covers keyboard/IME, DPI, monitor changes,
      activation, teardown, and process-isolation checks.

Design decision:

- This milestone uses a closely managed hosted window, not an in-shell VM or a
  cross-process render bridge. It preserves process isolation and reports the
  zero-hidden-VM invariant. A true single-window renderer remains a separate
  architecture change.

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
nsh test packages/plugins/local-files
nsh test packages/plugins/git-repos
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
./scripts/test-windows-support.ps1 -Exe target/release/steward-app.exe
cargo test -p steward-plugin-host --release synthetic_plugin_route_scaling -- --ignored --nocapture
```

Pass `-PackagePath` and `-TrustRootFile` to the plugin scripts when measuring
production-signed artifacts from outside the development workspace.

The repository-level `.cargo/config.toml` selects the `rsproxy` registry so
plain Cargo commands can resolve dependency versions that the local global
mirror has not synchronized yet. Pass Cargo's `--config` options to override
the mirror when needed.

## Performance Gates

| Metric                          |                              Gate |
| ------------------------------- | --------------------------------: |
| Cached cold start               | previous release + 500 ms or less |
| Summon p95                      |                     50 ms or less |
| Idle RSS increase               |                    32 MiB or less |
| Official plugin first frame p95 |                    500 ms or less |
| Existing plugin focus p95       |                     50 ms or less |
| Hidden plugin VMs               |                              zero |

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
  `STEWARD_INSTANCE_SUFFIX` and `STEWARD_SUMMON_HOTKEY` are developer-only
  acceptance overrides for running the Windows support suite beside a normal
  Steward install. Production launches do not require or persist them.
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
