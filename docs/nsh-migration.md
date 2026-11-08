# Steward NSH Migration Plan

> Status: NSH-0 through NSH-3 complete; NSH-4 in progress.
>
> This is the canonical migration plan, decision record, and release ledger.
> Milestone changes must update this file with the changed files, verification
> commands, results, deviations, and any remaining release gates.

## Why NSH

The pre-v0.3.0 plugin stack was a purpose-built system: TypeScript was bundled
by esbuild into a serialized JSON View or Virtual UI Tree, sent over a custom
JSON-RPC channel, and rendered by Steward. It worked, but Steward owned both
the UI schema and the runtime bridge. Every richer interaction expanded that
private protocol.

NSH replaces that vertical stack with a maintained Rust UI engine and TSX
application runtime:

- Steward's launcher and settings use NSH Rust components, so the shell has no
  plugin VM.
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
| Memory | Launcher starts no JavaScript VM; only open plugin windows allocate VMs | In-process VMs still cost 64 MiB each plus application assets |
| Isolation | Per-VM heap/stack/time/node quotas, signed packages, capability grants | Not OS-process isolation; a host/runtime bug remains in-process |
| Distribution | Standard signed package with certificate, revocation, and rollback checks | Production key ceremony and secret provisioning are required |
| Migration | Direct cutover removes two protocols and the old runtime | Existing plugins and plugin storage are not compatible |
| Performance | Route discovery is metadata-only and does not start a VM | Plugin first-frame performance depends on NSH startup and is still a release gate |

## Decisions

1. Steward's launcher and settings are NSH Rust roots rendered by
   `nsh-platform-gpui`.
2. The product uses GPUI Fast directly and pins:
   - `gpui-fast = =0.1.9`
   - `gpui-fast-platform = =0.1.9`
3. NSH is pinned to git revision
   `5359c07ff7b5728e67957c24322373309c90666b` rather than crates.io `0.2.0`.
   The published `0.2.0` predates `HostChannel` and `EngineContext`, which the
   embedding requires.
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
        |     +-- settings
        |
        +-- NSH plugin window (one per plugin ID)
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
  "id": "com.example.calendar",
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
  "pluginId": "com.example.calendar",
  "command": "calendar",
  "input": "calendar",
  "locale": "en",
  "accent": "#863bff",
  "dark": true
}
```

Host calls:

- `steward.clipboard.history` -> clipboard entry array
- `steward.window.close` -> `{}`

Host events:

- `steward.command` -> `{ "command": "...", "input": "..." }`
- `steward.theme` -> `{ "accent": "#...", "dark": true }`

## Milestones

| Milestone | Status | Scope |
|---|---|---|
| NSH-0 | complete | Decisions, contracts, and migration ledger |
| NSH-1 | complete | GPUI Fast/NSH dependency and embedding proof |
| NSH-2 | complete | NSH Rust launcher, search pages, and settings |
| NSH-3 | complete | Registry, host, SDK, and official plugin cutover |
| NSH-4 | in progress | Packaging, CI, docs, production trust, and release measurements |

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

- Launcher and settings are NSH Rust roots.
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
- `STEWARD_SHOW_SETTINGS=1` opened the settings root without a process crash.

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

Pending release gates:

- Provision the production Ed25519 root public key and CI release
  key/certificate, then replace the intentionally empty trust-root file.
- Record release cold start, RSS, summon p95, plugin first-frame p95, existing
  plugin focus p95, and hidden-VM count.
- Install the generated release MSI on a clean Windows machine.
- Perform a signed package installation smoke test using the production trust
  root.

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

On this development machine the Cargo mirror must be selected explicitly when
offline:

```text
--offline --config "source.crates-io.replace-with='rsproxy'" --config "source.rsproxy.registry='sparse+https://rsproxy.cn/index/'"
```

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

- NSH git revision is used instead of crates.io `0.2.0` because `0.2.0` lacks
  `HostChannel` and `EngineContext`.
- Plugin VMs are in-process NSH engines, not separate OS processes. Signed
  packages, capability grants, and hard engine quotas mitigate the risk.
- The release trust-root file is empty until the production key ceremony is
  complete; release package launch fails closed in that state.
- `STEWARD_SHOW_ON_START`, `STEWARD_SHOW_SETTINGS`, `STEWARD_SMOKE_PLUGIN`,
  `STEWARD_SMOKE_COMMAND`, and `STEWARD_SMOKE_TITLE` are developer-only smoke
  hooks. `STEWARD_DATA_DIR` consistently redirects app data, plugin data, and
  clipboard storage for isolated tests.
- No migration commit or tag is created by this working-tree implementation.
  Milestone evidence is tied to the listed files and test results.

## Known Risks Before Release

- Package verification and unpacking currently happen synchronously on the
  UI/event thread in `PluginHost::prepare_launch`. Move this work to a worker
  before packages or disks become large enough to stall the launcher.
- The verified package receipt is accepted before the first NSH frame is
  confirmed healthy. The release candidate should accept the receipt only after
  the window reaches a healthy first frame.
- Automated coverage verifies routing, manifests, signing, headless plugin
  behavior, and process-level window opening. Interactive settings controls,
  IME, and input-driven hotkey replacement still need a Windows UI acceptance
  pass.
- Launcher result rows currently use a generic glyph instead of restoring
  extracted Windows application icons and rendering plugin SVG icons.

## Rollback

Before the first NSH release, revert the migration change set as a group. After
release, reinstall the previous MSI and previous plugin artifacts. Plugin
storage and window layout are intentionally not migrated across this breaking
change.
