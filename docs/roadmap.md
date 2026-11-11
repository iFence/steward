# Steward Roadmap

Steward is a Windows-first launcher and plugin platform. The current platform
is built on GPUI Fast and NSH. The migration's exact status, decisions, and
release gates live in [nsh-migration.md](nsh-migration.md).

## Current Status

| Milestone | Status |
|---|---|
| M0 core foundation | complete |
| M1 application launcher MVP | complete |
| NSH migration (NSH-0 to NSH-3) | complete |
| NSH release hardening (NSH-4) | in progress |
| Windows support polish (M4) | next |
| Plugin ecosystem v1 (M5) | planned |
| AI capability and MCP layers (M6-M8) | planned |
| External runtime compatibility (M9-M10) | later |

Legacy M2/M3 records for QuickJS, JSON-RPC, JSON View, Virtual UI Tree, and
dock workspaces are historical only. They are preserved in
[architecture-history.md](architecture-history.md) and are not implementation
targets.

## Platform Architecture

```text
Steward
  |
  +-- NSH Rust shell (no plugin VM)
  |     +-- application search
  |     +-- settings and hotkeys
  |     +-- plugin route catalog
  |
  +-- NSH plugin windows
  |     +-- signed plugin.nshpkg
  |     +-- TSX application and QuickJS VM
  |     +-- typed Steward host bridge
  |
  +-- Future capability layer
        +-- AI-callable plugin capabilities
        +-- MCP tools
        +-- automation workflows
```

## NSH-4 Release Hardening

Required before v0.3.0 is considered release-complete:

- [x] complete the production Ed25519 root-key ceremony;
- [x] provision the CI release key and certificate;
- [x] replace the empty development trust-root file;
- [ ] measure release cold start, RSS, summon latency, plugin first frame, plugin
  focus, and hidden VM count;
- [ ] install the MSI on a clean Windows machine;
- [ ] install and launch all official plugins through the production trust root.

## M4 - Windows Support Polish

- clean install, upgrade, repair, and uninstall paths;
- mixed-DPI and multi-monitor summon behavior;
- IME and keyboard accessibility acceptance;
- tray lifecycle and autostart regression coverage;
- crash and package-verification diagnostics suitable for support.

## M5 - Plugin Ecosystem v1

- plugin scaffolding CLI;
- package inspection and signature status UI;
- install, update, remove, and rollback flows;
- a local marketplace index;
- publisher identity and revocation operations;
- automated package policy checks.

## M6 - AI Capability System

Plugins will be able to expose typed capabilities in addition to UI commands.
The initial model should cover:

- capability discovery and JSON schema descriptions;
- explicit user grants for data sources and side effects;
- audit logs and per-call budgets;
- examples spanning UI actions and AI-callable functions.

## M7 - MCP Compatibility

- expose selected Steward capabilities as MCP tools;
- consume external MCP servers through an explicit permission boundary;
- preserve cancellation, timeout, and audit semantics across the bridge;
- avoid making MCP a mandatory dependency of the launcher shell.

## M8 - AI Search and Research

- source-aware search over local and remote providers;
- citation and provenance metadata;
- streaming UI in NSH plugin windows;
- privacy controls for indexing and model requests.

## M9 - External Ecosystem Compatibility

Raycast, Vicinae, and Node compatibility should be implemented as optional
adapters or sidecars. They must not add a permanent Node runtime to the shell.

## M10 - Additional Runtimes and Marketplace

- evaluate WASM for compute-heavy plugins;
- add signed runtime-specific package types;
- publish a marketplace with package verification and rollback;
- keep route discovery and shell startup independent of runtime count.

## Priorities

1. Finish NSH-4 and make Windows installation trustworthy.
2. Improve plugin authoring and package management.
3. Add AI-callable capabilities with explicit consent and diagnostics.
4. Add MCP and external ecosystem adapters only where they preserve the
   launcher's latency and memory goals.
