# AGENTS.md

Guidelines for contributing to **Steward** — a launcher / plugin platform skeleton: an English-language developer guide (repo docs/README are Chinese; code and commit messages are English).

## Architecture

Steward is a monorepo combining a **Rust workspace** (native host) and a **pnpm TypeScript workspace** (plugin SDK + official plugins).

```
┌───────────────────────────────────────────────────────────┐
│  Native main process (Rust)        │  Plugin system       │
│  app (GPUI window/tray/hotkey)     │  plugin-runtime      │
│  ├─ gpui (git, from Zed)           │  (embedded QuickJS)  │
│  ├─ core-engine (search)           │   ↑ bundled TS→JS    │
│  └─ plugin-host / registry /       │   (esbuild)         │
│     storage (SQLite)               │                      │
│        └── JSON-RPC over UDS /     │                      │
│            Windows named pipe      │                      │
└───────────────────────────────────────────────────────────┘
```

- **Native main process**: `steward-app` binary — GPUI window, tray, global hotkey, lifecycle. Currently at **M0** (silent tray startup + centered borderless launcher bar).
- **Plugin system** (M2, placeholder): a separate `plugin-runtime` process embeds **QuickJS**; TS plugins are bundled to single-file JS via **esbuild**, then loaded in QuickJS. IPC is JSON-RPC over Unix socket / Windows named pipe, protocol defined in `ipc-protocol`.
- **Ground truth**: `docs/architecture.md` is the canonical architecture doc and includes a **decision-record** section — append new decisions there rather than inline.

## Module breakdown

- **Workspace root** `Cargo.toml` (resolver 2, edition 2021, v0.1.0): shared `[workspace.package]` + `[workspace.dependencies]` as the "standard library" of external deps — declare versions **once**, not per-crate.
- Rust crates (under `crates/`, all new code must live here):

  | crate | role |
  |---|---|
  | `app` | binary entry: GPUI window / tray / hotkey / lifecycle (only fully wired crate today) |
  | `core-engine` | search, indexing, fuzzy match, ranking — **no UI deps**, unit-testable |
  | `plugin-host` | plugin lifecycle, routing, permissions, IPC gateway |
  | `plugin-registry` | plugin metadata cache (SQLite) |
  | `storage` | SQLite wrapper, config read/write |
  | `ui-components` | business UI components on gpui-component |
  | `ipc-protocol` | shared message protocol between main & plugin runtime |
  | `plugin-runtime` | standalone plugin host process (embedded QuickJS) |

  > Most crates are scaffolding placeholders (empty deps); their planned deps live in `[workspace.dependencies]` until wired in for M1/M2.

- **TS side**: pnpm workspace, npm scope `@steward/*` — `@steward/extension-api`, `@steward/plugins/*`, bundled with **esbuild**.
- **Naming**: Rust crates prefixed `steward-*`; npm packages scoped `@steward/*`.

## Performance

Performance (low memory + fast response) is the product's core positioning.

- **Track metrics** in `docs/benchmarks.md`: cold start (ms), RSS (MB), summon latency (ms), measured via `scripts/bench.sh` per milestone.
- **Prefer sync where fast enough** — e.g. SQLite via `rusqlite` (bundled); avoid async ORMs.
- **Scaling (M2, mandatory)**: metadata cache (cold start reads cache, not full file I/O), trigger routing with a **100 ms timeout circuit-break**, graded isolation (shared-pool with heap limit + exec timeout vs. dedicated subprocess for privileged/heavy plugins), least-privilege permissions.
- **Watch linear degradation**: cold start and search latency must not grow linearly with plugin count, only with active plugin count (see `scripts/gen-test-plugins.sh`).

## Don't reinvent the wheel (dependency policy)

Prefer battle-tested dependencies over hand-rolled code:

- **Use the workspace library**: `nucleo` (fuzzy match), `rusqlite` (SQLite), `rquickjs` (QuickJS), `serde`/`serde_json` (JSON), `tokio`, `tracing`, `anyhow`, `gpui`/`gpui-component`, `global-hotkey`, `tray-icon`.
- **Never duplicate an existing dependency's job** — no second fuzzy matcher, no second DB/ORM, no handwritten JSON when `serde` exists.
- **Before adding a dependency**: check `[workspace.dependencies]` (root `Cargo.toml`), `Cargo.lock`, and `node_modules` first.
- **Prefer crates.io published versions**; use git deps only when necessary (gpui is pulled from Zed at a pinned revision — `rust-toolchain.toml` is pinned to **1.95.0** accordingly).
- **Keep the footprint small** — fewer/lighter deps directly serve the low-memory, fast-build goals.

## Git — Angular-style (Conventional Commits)

Commit messages follow **Conventional Commits** (established by repo history; best practice even though no tool enforces it).

- Format: `type(<optional scope>): subject`
- **Scope is optional**; a crate/package name can be used when helpful (e.g. `feat(core-engine): ...`).
- Recommended types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`, `style`, `build`, `ci`, `revert`.
- **Message in English**.
- Group logically — one coherent unit per commit; avoid amorphous "wip" commits; reference issue/PR numbers when relevant.

## Conventions

- **Rust**: `rustfmt` + `clippy -- -D warnings` and `cargo test --workspace` — enforced in CI.
- **TypeScript**: eslint + prettier — `semi: true`, `singleQuote: false` (**double quotes**), `printWidth: 100`, `trailingComma: all`; run `pnpm lint`, `pnpm typecheck`, `pnpm build`.
- **Language**: code and commit messages in English; comments/docs may be bilingual.
- **Docs**: architecture/spec/benchmark docs live in `docs/`.
