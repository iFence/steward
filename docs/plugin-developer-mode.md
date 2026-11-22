# Steward Plugin Developer Mode

> Status: PDX-2 implementation complete and CI-gated. Developer mode is a
> local development workflow; it does not change production trust or capability
> policy.

## Isolated Profile

Developer mode resolves its profile before Steward opens the database, claims
the single-instance mutex, starts search, or creates the plugin store.

| Item | Installed app | Developer mode |
| --- | --- | --- |
| Data directory | `%APPDATA%\Steward` | `%APPDATA%\Steward-Developer` |
| Plugin directory | `<data>\plugins` | `<developer-data>\plugins` |
| Instance lock | Production profile | `developer` suffix |
| Trust roots | Embedded production roots | Embedded roots plus explicit developer roots |

Start a development shell alongside the installed app:

```powershell
cargo run -p steward-app -- --developer-mode
```

Use explicit isolated locations when needed:

```powershell
cargo run -p steward-app -- `
  --developer-mode `
  --developer-data-dir D:\steward-dev\data `
  --developer-plugins-dir D:\steward-dev\plugins `
  --developer-trust-root D:\steward-dev\signing\root.pub
```

`--developer-trust-root` accepts a public key file, one base64 key, or a JSON
array of keys. The option may be repeated. `STEWARD_DEVELOPER_TRUST_ROOT` and
`STEWARD_DEVELOPER_DATA_DIR` provide environment equivalents. Regular
`STEWARD_DATA_DIR` and `STEWARD_PLUGINS_DIR` are ignored after developer mode
is selected, so a stale shell environment cannot redirect development writes
into the installed profile.

## One-Command Build Loop

The CLI uses the same isolated profile convention. Run the app in developer
mode in one terminal, then use this command after editing the plugin:

```powershell
steward-plugin --developer-mode dev D:\plugins\my-plugin `
  --signing-key C:\secure\release.secret `
  --signing-cert C:\secure\release.cert
```

The command performs this sequence:

1. runs `dev:build` or `build` from `package.json` when present, otherwise
   runs `nsh build App.tsx --release -o dist`;
2. signs `App.tsx` with `nsh publish` when signing paths are supplied;
3. creates the exact `<plugin-id>/plugin.json` and
   `<plugin-id>/plugin.nshpkg` archive;
4. verifies signature, trust root, host API, capability, manifest/package
   identity, and policy checks through the normal plugin store;
5. replaces the installed development package, including an unchanged
   semantic version and build number; and
6. queues a static reload for the running developer shell.

`STEWARD_NSH_RELEASE_KEY` and `STEWARD_NSH_RELEASE_CERT` can replace the signing
arguments. A custom build command must either produce `dist/plugin.nshpkg` or
run with signing credentials. The CLI refuses a package older than the edited
source instead of silently reloading a stale build.

Useful variants:

```powershell
# Reinstall an existing signed package without rebuilding it.
steward-plugin --developer-mode dev . --no-build

# Replace and validate without opening a new window.
steward-plugin --developer-mode dev . --no-open

# Explicitly select the static-bundle fallback.
steward-plugin --developer-mode dev . --reload static
```

Custom isolated paths accepted by the CLI are
`--developer-data-dir` and `--developer-plugins-dir`. In developer mode the
regular `--data-dir` and `--plugins-dir` options are rejected.

## Static Reload

PDX-2 does not depend on NSH GPUI hot reload. A reload request contains only the
plugin ID, route, title, and whether to open the plugin. The running shell
consumes that request, refreshes the route cache, closes the old host process,
and starts a new static package in a fresh QuickJS VM.

The same action is available from Settings > Plugins as **Reload**. A crashed
or closed plugin can be opened again by the queued request; `--no-open` skips
that behavior.

## Settings Installation

Settings > Plugins provides:

- **Install ZIP...** for a normal two-file plugin archive;
- **Install Folder...** for a directory containing exactly `plugin.json` and
  `plugin.nshpkg`.

Both paths call the same `PluginStore` inspection and installation code as the
CLI. In developer mode, installing a new local package uses the same
same-version replacement rule as `steward-plugin dev`.

## Developer Diagnostics

Settings > Plugins shows a Developer Diagnostics section when the app is in
developer mode. Each installed plugin reports:

- first-frame latency;
- host-call outcomes and failures;
- denied capability attempts;
- process exits and recent crashes;
- recent runtime events; and
- a bounded excerpt of log lines containing the plugin ID and an error signal.

Capability tracing records the call name and outcome only. Request payloads,
arguments, clipboard contents, file contents, signing secrets, and other
sensitive values are intentionally omitted by default.

## Launch Doctor

Run the canonical launch diagnosis against an installed ID, ZIP, or two-file
directory:

```powershell
steward-plugin --developer-mode doctor com.example.my-plugin
steward-plugin --developer-mode doctor .\plugin.zip --json
```

Doctor checks source/layout, manifest parsing, package parsing, ID and version
agreement, signature, configured and accepted trust roots, host API
compatibility, declared capabilities, and release policy. It exits non-zero and
reports the first failing stage, making it suitable for CI and editor tasks.

## Verification

`pnpm test:pdx2` creates an isolated signed plugin, rebuilds it twice at the
same version, verifies package replacement and queued static reloads, checks
doctor success and layout failure, and confirms that the app ignores a
production `STEWARD_DATA_DIR` while in developer mode.

## Safety Invariants

- Developer mode never weakens signature verification, trust roots, host API
  checks, capability policy, process isolation, or user grants.
- Same-version replacement is accepted only in developer mode.
- The installed profile's database, plugin directory, and trust configuration
  are untouched.
- Host-call tracing and reload requests do not persist request payloads or
  secrets.
