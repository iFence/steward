# Steward

<p align="center">
  <img src="assets/steward.png" alt="Steward" width="300">
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/github/license/iFence/steward?style=flat-square&color=orange" alt="License"></a>
  <img src="https://img.shields.io/badge/MSRV-1.97-red?style=flat-square" alt="MSRV">
  <img src="https://img.shields.io/badge/platform-Windows-lightgrey?style=flat-square" alt="Platform">
  <a href="https://github.com/iFence/steward"><img src="https://img.shields.io/github/stars/iFence/steward?style=social" alt="Stars"></a>
</p>

<p align="center">
  <a href="README_CN.md">中文版</a>
</p>

Steward is a Windows launcher and plugin platform built with Rust, GPUI Fast,
and Native Shell (NSH).

The launcher is an NSH Rust surface and the settings window uses GPUI Kit
components, so neither starts a JavaScript VM. Plugin commands open signed TSX
applications in full NSH windows with a 64 MiB VM, time/node quotas, and
least-privilege capabilities.

<p align="center">
  <img src="assets/1.png" alt="Steward screenshot" width="760">
</p>

## Repository structure

```text
steward/
├── Cargo.toml
├── crates/
│   ├── app/                 GPUI Fast shell, tray, hotkeys, plugin windows
│   ├── core-engine/         application search and matching
│   ├── plugin-host/         signed package verification and route catalog
│   ├── plugin-registry/     plugin.json v2 metadata cache
│   ├── storage/             SQLite and settings
│   └── ui-components/       NSH Rust launcher components
├── packages/
│   ├── extension-api/       typed Steward host calls for TSX plugins
│   └── plugins/             Calendar, Clipboard History, UI Showcase
├── assets/nsh/              release trust roots
├── docs/                    architecture, migration, API, release records
└── scripts/                 plugin packaging, MSI, and benchmarks
```

## Getting started

Prerequisites:

- Rust 1.97.1 with the MSVC target
- Node.js 20 or newer
- pnpm 11
- The pinned NSH CLI for plugin development

Install the reviewed NSH CLI revision:

```powershell
cargo install --locked --git https://github.com/longbridge/nsh `
  --rev 5359c07ff7b5728e67957c24322373309c90666b nsh-cli
```

Build and run:

```powershell
pnpm install
pnpm typecheck
pnpm build
cargo run -p steward-app
```

The app starts silently in the system tray. Summon it with `Ctrl+Alt+Space`.
Set `STEWARD_SHOW_ON_START=1` for development if the launcher should be visible
immediately.

## Plugin development

Each official plugin has:

```text
App.tsx          NSH TSX application
nsh.json         NSH package metadata
plugin.json      Steward v2 routing metadata
App.test.tsx     headless NSH test
```

Run `nsh check App.tsx`, `nsh test .`, and `nsh build App.tsx --release -o dist`
from a plugin directory. See [docs/nsh-plugin-api.md](docs/nsh-plugin-api.md)
for Steward's host bridge and [docs/plugin-manifest-spec.md](docs/plugin-manifest-spec.md)
for the route manifest.

Release packages are signed with the NSH release key and certificate:

```powershell
$env:STEWARD_NSH_RELEASE_KEY = "<path-or-secret>"
$env:STEWARD_NSH_RELEASE_CERT = "<path-or-secret>"
.\scripts\package-plugins.ps1
```

The resulting zip contains only `plugin.json` and `plugin.nshpkg`. Extract its
top-level plugin directory under `%APPDATA%\Steward\plugins`.

## Documentation

- [Architecture](docs/architecture.md)
- [NSH migration plan and status](docs/nsh-migration.md)
- [NSH plugin API](docs/nsh-plugin-api.md)
- [Plugin manifest v2](docs/plugin-manifest-spec.md)
- [Performance benchmarks](docs/benchmarks.md)
- [Roadmap](docs/roadmap.md)
- [Changelog](Changelog.md)

## Milestones

- [x] M0: GPUI launcher shell, tray, and global hotkey
- [x] M1: application launcher MVP
- [x] NSH-0: migration decisions and contracts
- [x] NSH-1: GPUI Fast and NSH embedding
- [x] NSH-2: NSH Rust launcher and GPUI Kit settings
- [x] NSH-3: NSH plugin windows and official plugin migration
- [ ] NSH-4: production keys, release benchmarks, and final acceptance

## License

[MIT](LICENSE). NSH-derived notices are listed in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
