# Steward

<p align="center">
  <img src="assets/steward.png" alt="Steward" width="300">
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/github/license/iFence/steward?style=flat-square&color=orange" alt="License"></a>
  <img src="https://img.shields.io/badge/MSRV-1.95-red?style=flat-square" alt="MSRV">
  <img src="https://img.shields.io/badge/platform-Windows-lightgrey?style=flat-square" alt="Platform">
  <a href="https://github.com/iFence/steward"><img src="https://img.shields.io/github/stars/iFence/steward?style=social" alt="Stars"></a>
</p>

<p align="center">
  <a href="README_CN.md">中文版</a>
</p>

Steward is a small, polished launcher and plugin platform for Windows, built with Rust, GPUI, and `gpui-component`.

The product goal is a launcher that summons instantly, stays low-memory, and remains stable over long sessions, serving both everyday app-launching and an extensible plugin space for developers and power users. The core app owns the desktop shell, launcher state, fast navigation, and plugin host. Heavier capabilities are isolated behind process plugins so they can evolve without slowing down or destabilizing the core.

<p align="center">
  <img src="assets/1.png" alt="Steward screenshot" width="760">
</p>


## Repository Structure

```
steward/
├── Cargo.toml                 # Rust workspace root
├── crates/
│   ├── app/                   # Binary entry: GPUI window / hotkey / lifecycle
│   ├── core-engine/           # Search, indexing, fuzzy matching, ranking (no UI deps)
│   ├── plugin-host/           # Plugin lifecycle, routing, permissions, IPC gateway
│   ├── plugin-registry/       # Plugin metadata cache (SQLite)
│   ├── storage/               # SQLite wrapper, config read/write
│   ├── ui-components/         # Business UI components on gpui-component
│   └── ipc-protocol/          # Shared message protocol between main & plugin runtime
├── plugin-runtime/            # Standalone plugin host process (embedded QuickJS)
├── packages/                  # TS side (pnpm workspace)
│   ├── extension-api/         # TS type declarations + runtime polyfill for plugin authors
│   └── plugins/               # Official sample plugins
├── docs/                      # Architecture / API / manifest / benchmark docs
├── scripts/                   # Build / packaging / benchmark scripts
└── .github/workflows/         # CI / release
```

## Getting Started

Prerequisites: Rust stable, Node.js ≥ 20, pnpm.

```bash
# Native app (M0/M1)
cargo run -p steward-app

# TS side
pnpm install
pnpm build
```

On launch the app stays silent in the system tray rather than popping up a window; quit via the tray menu's "Exit Steward".

Hotkeys / interactions:

- `Ctrl+Alt+Space`: summon / hide the launcher bar
- `Esc`: hide the launcher bar
- Tray left-click: summon / hide the launcher bar
- Tray right-click menu: show / hide, exit
- Drag the launcher bar by holding anywhere on it (the input area is draggable too)

## Documentation

- [Architecture](docs/architecture.md)
- [Plugin API draft](docs/extension-api.md)
- [Plugin manifest spec draft](docs/plugin-manifest-spec.md)
- [Performance benchmarks](docs/benchmarks.md)

## Milestones

- [x] M0: Skeleton runs (GPUI launcher bar + global hotkey + system tray)
- [x] M1: Application-launcher MVP (app scanning + nucleo fuzzy matching + SQLite index)
- [ ] M2: Plugin system v1 (JSON-RPC IPC + QuickJS runtime + scaling strategies)
- [ ] M3: Plugin API coverage + Node polyfill + UI polish
- [ ] M4: Windows support polish
- [ ] M5: Plugin ecosystem infrastructure (scaffolding, signature verification, plugin marketplace)

## License

[MIT](LICENSE)
