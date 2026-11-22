# Steward Roadmap

Steward is a Windows-first launcher and plugin platform. The current platform
is built on GPUI Fast and NSH. The migration's exact status, decisions, and
release gates live in [nsh-migration.md](nsh-migration.md).

## Current Status

| Milestone                               | Status                                                   |
| --------------------------------------- | -------------------------------------------------------- |
| M0 core foundation                      | complete                                                 |
| M1 application launcher MVP             | complete                                                 |
| NSH migration (NSH-0 to NSH-3)          | complete                                                 |
| NSH release hardening (NSH-4)           | blocked: clean VM and production artifacts               |
| Plugin process isolation (Phase 3)      | blocked: clean VM acceptance                             |
| Windows support polish (M4)             | blocked: clean-VM and interactive IME/display acceptance |
| Plugin ecosystem v1 (M5)                | complete                                                 |
| Plugin capability layer (PC-0 to PC-4)  | complete                                                 |
| Public third-party admission (PC-5)     | implementation complete; blocked on clean-VM/production   |
| AI capability and MCP layers (M6-M8)    | planned                                                  |
| External runtime compatibility (M9-M10) | later                                                    |

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
  +-- NSH plugin host processes (one per open window)
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

Required before the NSH release is considered complete:

- [x] complete the production Ed25519 root-key ceremony;
- [x] provision the CI release key and certificate;
- [x] replace the empty development trust-root file;
- [x] add unified launcher/action, hotkey, settings, and schema-migration
      foundations;
- [x] add capability grant, revocation, and audit storage;
- [x] add automated trust, signed-package, performance, and MSI lifecycle
      acceptance tooling;
- [x] measure release cold start, RSS, summon latency, plugin first frame, plugin
      focus, and hidden VM count;
- [ ] blocked: install the MSI on a clean Windows machine;
- [ ] blocked: install and launch all official plugins through the production
      trust root.

Local Release evidence is recorded in [benchmarks.md](benchmarks.md). The final
local summon run measured 60 ms p95, above the 50 ms gate, so measurement is
complete but the performance gate remains open. Implementation is complete;
acceptance is blocked because the current environment has no clean Windows VM,
elevated installer lifecycle, or production-signed plugin artifacts.

## Plugin Process Isolation (Phase 3)

- [x] run one `--plugin-host-process` child per open plugin window;
- [x] keep launcher and settings windows free of plugin VMs;
- [x] verify signed packages independently in each child;
- [x] route command, focus, theme, and close actions across the process
      boundary;
- [x] surface exit and protocol failures without taking down the launcher;
- [x] enforce Windows Job Object memory, CPU, process-count, and handle limits;
- [x] implement per-plugin restart/backoff policy;
- [x] measure first frame, focus, host count, aggregate RSS, crash containment,
      memory-limit enforcement, and route scaling on a Release build;
- [ ] blocked: run signed-package install/upgrade/crash acceptance on a clean
      Windows VM;
- [ ] blocked: complete production-root plugin installation acceptance.

The implementation and local Release measurements are complete. Phase 3 is
blocked only on clean-VM and production-artifact acceptance.

## M4 - Windows Support Polish

Implementation is complete. Acceptance is tracked in
[windows-acceptance.md](windows-acceptance.md) and requires a clean Windows 11
VM with the release MSI and at least one mixed-DPI or multi-monitor display
configuration.

- [x] clean install, upgrade, repair, and uninstall paths are automated by
      `scripts/test-msi-lifecycle.ps1`;
- [x] mixed-DPI and multi-monitor summon geometry is implemented and covered by
      native geometry tests plus `scripts/test-windows-support.ps1`;
- [x] focused Unicode keyboard input, navigation events, IME context
      attachment, and foreground focus are covered by the Windows support suite;
- [x] tray/hotkey creation and destruction, single-instance activation, and
      autostart repair/removal regression coverage is automated;
- [x] `steward-app.exe --write-diagnostics <path>` emits a support report with
      display DPI, autostart state, trust-root identity, plugin package hashes,
      database schema, and recent error lines;
- [ ] blocked: run the support suite and real Chinese IME composition on the
      acceptance VM;
- [ ] blocked: finish the clean-VM MSI install/upgrade/repair/uninstall matrix.

## M5 - Plugin Ecosystem v1

- [x] plugin scaffolding CLI;
- [x] package inspection and signature status UI;
- [x] install, update, remove, and rollback flows;
- [x] a local marketplace index;
- [x] publisher identity and revocation operations;
- [x] automated package policy checks.

Implementation and operating guidance are recorded in
[plugin-ecosystem.md](plugin-ecosystem.md). M5 is local-first: a public
third-party marketplace and admission service are not part of this milestone.
Rollback remains subject to NSH's anti-downgrade rule after a newer build is
accepted as healthy.

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
2. Finish plugin process isolation, resource containment, and restart policy.
3. Complete PC-3 network, controlled process execution, and cross-domain
   validation. (Done)
4. Complete PC-4 embedded presentation and navigation. (Done)
5. Complete PC-5/PDX-4 clean-VM and production independent-publisher
   acceptance. Automated admission, publisher operations, quarantine, root
   revocation responses, and cross-domain fixtures are implemented.
6. Improve plugin authoring and package management. Encrypted plugin-private
   secrets are implemented as PDX-5.1; other deferred capabilities still need
   reviewed workloads.
7. Add AI-callable capabilities with explicit consent and diagnostics.
8. Add MCP and external ecosystem adapters only where they preserve the
   launcher's latency and memory goals.
