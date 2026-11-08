# Steward Architecture

> Current architecture as of v0.3.0. The migration plan, evidence, and release
> gates are in [nsh-migration.md](nsh-migration.md).

## Product Shape

Steward is a Windows-first launcher and plugin platform. It starts silently in
the system tray, summons a focused launcher with a global hotkey, searches
installed applications, built-in actions, and installed plugin routes, and
opens each plugin as a full NSH application window.

The product's performance target is:

- fast summon and query response independent of installed plugin count;
- no plugin VM until a plugin window opens;
- one VM per open plugin, destroyed with its window;
- explicit memory, stack, call, load, and UI-node limits.

## Process and Runtime Model

```text
steward-app.exe
  |
  +-- GPUI Fast application
        |
        +-- Steward launcher window
        |     +-- NSH Rust root
        |     +-- application search services
        |     +-- plugin route catalog
        |
        +-- Steward settings window
        |     +-- NSH Rust root
        |     +-- language, accent, autostart, summon hotkey
        |
        +-- plugin window (one per plugin ID)
              +-- nsh-platform-gpui AppView
              +-- one QuickJS VM
              +-- signed plugin.nshpkg
              +-- HostChannel bridge
```

The shell is Rust and contains no JavaScript VM. A plugin VM is created only
inside `crates/app/src/plugin_window.rs` when a verified package opens.

## Rust Modules

| Crate | Responsibility |
|---|---|
| `steward-app` | GPUI Fast application, windows, tray, hotkey, search events, plugin windows |
| `steward-core-engine` | application matching, calculator, URL detection |
| `steward-plugin-registry` | strict `plugin.json` v2 parser and SQLite route cache |
| `steward-plugin-host` | package trust, signature/certificate verification, route index, launch preparation |
| `steward-storage` | SQLite settings, usage, app cache, and clipboard rows |
| `steward-ui-components` | NSH Rust launcher rows and palette |

There is no plugin-runtime executable and no custom IPC protocol crate.

## Search and Launch Flow

Steward does not build or query a full-disk file index. Search is limited to
installed applications, built-in actions, and plugin routes. The storage
migration removes file-index snapshots left by earlier releases.

1. At startup, the route catalog is loaded from the SQLite plugin metadata
   cache. A background rescan updates the cache without starting a VM.
2. Launcher searches fan out to the cached application index, built-in
   commands, and the in-memory route index.
3. The root results contain applications, calculators, URL actions, and plugin
   routes.
4. Selecting a plugin route sends a launch request to the app event loop.
5. `PluginHost::prepare_launch` reads the package, verifies its signature,
   certificate, revocation, and rollback state, checks package
   ID/version against `plugin.json`, applies the release capability policy, and
   returns an NSH `LoadedPackage`.
6. The app opens or focuses the single window for that plugin ID. A repeat
   command focuses the window and emits `steward.command`.

## Plugin Contract

The installed layout is:

```text
%APPDATA%\Steward\plugins\<plugin-id>\
  plugin.json
  plugin.nshpkg
```

`plugin.json` is routing metadata only:

```json
{
  "schemaVersion": 2,
  "id": "com.example.calendar",
  "name": "Calendar",
  "version": "0.1.4",
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

Supported triggers are command name, longest matching prefix, and regex.
Dynamic triggers and inline plugin results no longer exist.

The NSH package contains the TSX application and UI assets. Its package ID and
version must equal the route manifest. The exact public surface is documented
in [nsh-plugin-api.md](nsh-plugin-api.md) and
[plugin-manifest-spec.md](plugin-manifest-spec.md).

## Capability and Resource Policy

Release packages receive only:

- clipboard history;
- read/write access scoped to `${dataDir}`;
- opening https and mailto URLs;
- host calls in the `steward.*` namespace.

Network, process execution, webview, and image-picker capabilities are denied.
The policy is applied again when a package is prepared for launch; package
metadata cannot grant itself extra capabilities.

VM limits:

| Limit | Value |
|---|---:|
| Heap | 64 MiB |
| Stack | 1 MiB |
| Load | 2 s |
| One JS call | 250 ms |
| UI nodes | 20,000 |

## Trust and Packaging

Production roots are configured in `assets/nsh/trust-roots.json`. The file is
intentionally empty in the repository until the production root key ceremony
is complete, so release launches fail closed. CI supplies the release signing
key and certificate through `STEWARD_NSH_RELEASE_KEY` and
`STEWARD_NSH_RELEASE_CERT`.

`scripts/package-plugins.ps1` creates one zip per official plugin. Each zip
contains only the top-level plugin directory, `plugin.json`, and the verified
`plugin.nshpkg`. `scripts/package-msi.ps1` packages the app; no plugin runtime
or file-index helper executable is installed.

## Verification and Operations

CI runs Rust formatting, clippy, workspace tests, TypeScript lint/format/type
checks/builds, and headless NSH plugin tests. Release performance and clean-MSI
acceptance are tracked in [benchmarks.md](benchmarks.md) and
[nsh-migration.md](nsh-migration.md).

The pre-migration architecture and decision log are preserved in
[architecture-history.md](architecture-history.md) for context. New decisions
must be recorded in this file and linked from the migration ledger.
