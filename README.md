# Steward

<p align="center">
  <img src="assets/steward.png" alt="Steward" width="300">
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/github/license/iFence/steward?style=flat-square&color=orange" alt="License"></a>
  <a href="https://github.com/iFence/steward/releases"><img src="https://img.shields.io/github/downloads/iFence/steward/total?style=flat-square&color=blue" alt="Downloads"></a>
  <a href="https://github.com/iFence/steward/releases/latest"><img src="https://img.shields.io/github/v/release/iFence/steward?style=flat-square&color=green" alt="Version"></a>
  <img src="https://img.shields.io/badge/MSRV-1.97-red?style=flat-square" alt="MSRV">
  <img src="https://img.shields.io/badge/platform-Windows-lightgrey?style=flat-square" alt="Platform">
  <a href="https://github.com/iFence/steward"><img src="https://img.shields.io/github/stars/iFence/steward?style=social" alt="Stars"></a>
</p>

<p align="center">
  <a href="README_CN.md">中文版</a>
</p>

Steward is a Windows launcher and plugin platform built with Rust, GPUI Fast,
and Native Shell (NSH).

Search installed applications and common Windows Settings pages from the same
launcher, then open a matching setting directly instead of navigating through
the Settings app.

The launcher is an NSH Rust surface and the settings window uses GPUI Kit
components, so neither starts a JavaScript VM. Plugin commands open signed TSX
applications in full NSH windows with a 64 MiB VM, time/node quotas, and
least-privilege capabilities.

<p align="center">
  <img src="assets/steward-screenshot.png" alt="Steward screenshot" width="760">
</p>

## Repository structure

```text
steward/
├── Cargo.toml
├── crates/
│   ├── app/                 GPUI Fast shell and isolated plugin host processes
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

Install the matching NSH CLI:

```powershell
cargo install --locked --version 0.7.0 nsh-cli
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

If startup, display scaling, autostart, or plugin verification fails, generate
a support report without starting the GUI:

```powershell
& .\target\release\steward-app.exe --write-diagnostics `
  .\target\steward-diagnostics.json
```

The report includes application and package hashes, monitor DPI and work areas,
the database schema, autostart state, and recent error lines. It does not read
clipboard contents or plugin-private storage.

## Installing existing plugins

Download the required `steward-plugin-<name>-<version>.zip` asset from
[GitHub Releases](https://github.com/iFence/steward/releases). Each archive
contains the plugin directory, `plugin.json`, and the signed `plugin.nshpkg`:

```text
calendar/
  plugin.json
  plugin.nshpkg
```

Either install the archive from Settings > Plugins, or use the CLI:

```powershell
cargo run -p steward-plugin-cli --bin steward-plugin -- `
  install .\steward-plugin-calendar-0.2.0.zip
```

The resulting layout should be:

```text
%APPDATA%\Steward\plugins\
  it.aspirin.calendar\
    plugin.json
    plugin.nshpkg
```

Steward scans this directory at startup and every 30 seconds while running.
Open the launcher and type one of the plugin commands:

- Calendar: `calendar`
- Clipboard History: `clipboard-history`
- UI Showcase: `uishowcase`

Settings > Plugins shows signature/trust status and provides install, update,
remove, and rollback actions. For a local marketplace, place verified zips in
`%APPDATA%\Steward\marketplace\packages` and choose **Sync Local Marketplace**.
Do not copy only `plugin.nshpkg` or flatten plugin files into the `plugins`
root; only a trusted package whose manifest and package ID/version match can be
installed or opened. See [plugin ecosystem](docs/plugin-ecosystem.md) for CLI,
policy, publisher, and revocation workflows.

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

## Releases

Pushing a version tag such as `v0.4.0` starts the Windows build-and-release
workflow. The tag must match `[workspace.package].version` in `Cargo.toml`, and
`Changelog.md` must contain a matching `## vX.Y.Z` section. The workflow builds
the MSI, signs and packages the official plugins, generates SHA-256 checksums,
and publishes the files on the GitHub Release.

Configure `STEWARD_NSH_RELEASE_KEY` and `STEWARD_NSH_RELEASE_CERT` as repository
secrets containing the signing key and certificate file contents:

```powershell
git tag v0.4.0
git push origin v0.4.0
```

## Documentation

- [Architecture](docs/architecture.md)
- [NSH migration plan and status](docs/nsh-migration.md)
- [NSH plugin API](docs/nsh-plugin-api.md)
- [Plugin development and local testing (Chinese)](docs/plugin-development-zh-CN.md)
- [Plugin ecosystem, CLI, marketplace, and policy](docs/plugin-ecosystem.md)
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
- [ ] NSH-4: implementation and local release measurements complete; blocked on
  clean-VM and production-artifact acceptance
- [ ] Plugin process isolation (Phase 3): implementation complete; blocked on
  clean-VM acceptance
- [ ] M4 Windows support polish: implementation complete; blocked on clean-VM,
  mixed-DPI, and interactive IME acceptance
- [x] M5 plugin ecosystem v1: scaffolding, inspection UI, local marketplace,
  publisher/revocation flows, lifecycle policies, and automated checks

## Acknowledgements

Thank you to [Timor's free holiday API](https://timor.tech/api/holiday/) for
providing the Calendar plugin's mainland China holiday and adjusted-workday
data. The service is free and does not require registration; please respect its
documented limit of 10,000 requests per IP address per 24 hours. Timor's site
content is published under
[CC BY-NC-SA 4.0](https://creativecommons.org/licenses/by-nc-sa/4.0/).

## License

[MIT](LICENSE). NSH-derived notices are listed in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
