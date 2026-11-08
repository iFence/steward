# AGENTS.md

Guidelines for contributing to Steward, a Windows-first launcher and plugin
platform. Repository docs may be bilingual; code and commit messages are
English.

## Architecture

Steward is a monorepo combining a Rust workspace with a pnpm TypeScript
workspace for official NSH plugin applications.

```text
Steward app process
  |
  +-- GPUI Fast window
        |
        +-- NSH Rust launcher/settings root (no JavaScript VM)
        |
        +-- NSH plugin window (one QuickJS VM per open plugin)
              |
              +-- signed plugin.nshpkg
```

- `steward-app` owns the tray, global hotkey, windows, search services, and
  plugin-window lifecycle.
- The launcher and settings are NSH Rust surfaces. They do not start QuickJS.
- Official plugins are TSX applications built and signed by the pinned NSH
  CLI. `plugin.json` is route metadata and points at `plugin.nshpkg`.
- Route discovery reads metadata only and never starts a plugin VM.
- `docs/nsh-migration.md` is the canonical migration plan and status record.
- `docs/architecture.md` is the architecture reference and decision log.

## Module breakdown

- Workspace root `Cargo.toml` (resolver 2, edition 2021, version 0.3.0)
  declares shared versions in `[workspace.dependencies]`.
- Rust crates live under `crates/`:

  | crate | role |
  |---|---|
  | `app` | GPUI Fast entry, NSH shell, tray, hotkeys, and plugin windows |
  | `core-engine` | application search, fuzzy matching, and ranking |
  | `plugin-host` | signed-package verification, catalog, and route-only launch |
  | `plugin-registry` | plugin routing metadata cache (SQLite) |
  | `storage` | SQLite wrapper and settings |
  | `ui-components` | NSH Rust launcher components |

- TypeScript packages use the `@steward/*` scope. `@steward/extension-api`
  contains typed Steward host calls; official plugins live under
  `packages/plugins/`.
- Rust crates use the `steward-*` package prefix.

## Performance and isolation

- Track cold start, RSS, summon latency, and plugin first-frame latency in
  `docs/benchmarks.md`.
- Prefer synchronous operations when they are fast enough. Do not add an async
  ORM around SQLite.
- Cold start and route search may scale with installed plugin metadata, but
  must not start with the number of installed VMs. Hidden plugin VMs must be
  zero.
- Each open plugin gets the limits in `docs/nsh-migration.md`: 64 MiB heap,
  1 MiB stack, 2 s load budget, 250 ms call budget, and 20,000 UI nodes.
- Plugin capabilities are deny-by-default. Release capability policy is
  defined in `crates/plugin-host/src/lib.rs`.

## Dependency policy

- Prefer established dependencies already listed in
  `[workspace.dependencies]`.
- Use `nucleo` for fuzzy matching, `rusqlite` for SQLite, `serde` for JSON,
  and NSH crates for UI/runtime behavior.
- Pin `gpui-fast` and `gpui-fast-platform` to `=0.1.9`.
- Pin NSH to the reviewed git revision until a published release includes
  `HostChannel` and `EngineContext`. Update `docs/nsh-migration.md` before
  moving the revision.
- Keep the runtime footprint small. Do not add a second matcher, database
  abstraction, JSON implementation, or JavaScript runtime.

## Plugin packaging and signing

- A distributable plugin zip contains `<plugin-id>/plugin.json` and
  `<plugin-id>/plugin.nshpkg` only.
- Release builds load verified packages only. Production trust roots live in
  `assets/nsh/trust-roots.json`; that file intentionally ships empty until the
  production key ceremony is complete, so release verification fails closed.
- Never commit private keys or certificate secrets. CI reads
  `STEWARD_NSH_RELEASE_KEY` and `STEWARD_NSH_RELEASE_CERT`.
- The manifest ID/version and package ID/version must match.

## Release notes

- `Changelog.md` and `Changelog-zh-CN.md` contain user-facing notes. Each
  released version uses a `## vX.Y.Z` heading.
- Add user-facing changes to both files in the same commit.
- `scripts/extract-release-notes.sh <tag> [changelog]` extracts the English
  release body and fails when the section is absent.

## Git and validation

- Use Conventional Commits in English, for example
  `feat(plugin-host): verify NSH packages before launch`.
- Keep commits coherent and avoid work-in-progress commits.
- Rust checks: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace`.
- TypeScript checks: `pnpm lint`, `pnpm format:check`, `pnpm typecheck`, and
  `pnpm build`.
- Plugin checks: run `nsh test` for each official plugin and verify every
  generated `.nshpkg` before packaging.
- TypeScript style: double quotes, semicolons, 100-column lines, and trailing
  commas.
