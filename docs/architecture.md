# Steward Architecture

> Current architecture as of v0.3.2. The migration plan, evidence, and release
> gates are in [nsh-migration.md](nsh-migration.md).
>
> The tracked plugin-platform responsibilities and hardening work are in
> [plugin-platform-refactor.md](plugin-platform-refactor.md).

## Product Shape

Steward is a Windows-first launcher and plugin platform. It starts silently in
the system tray, summons a focused launcher with a global hotkey, searches
installed applications, Windows Settings pages, built-in actions, and installed
plugin routes, and opens each plugin as a full NSH application window. On
Windows, it also searches files indexed by the system Windows Search service.
Route metadata may select an embedded presentation, which uses a
launcher-positioned frameless hosted surface while retaining the same isolated
plugin-host process.

The product's performance target is:

- fast summon and query response independent of installed plugin count;
- no plugin VM until a plugin window opens;
- one plugin host process and VM per open plugin, destroyed with its window;
- explicit memory, stack, call, load, and UI-node limits.

## Launcher Surface

The launcher uses a dark, blurred Acrylic/vibrancy surface. Its scrim is at
least 75% opaque so panel text stays legible while some backdrop blur remains,
and it adapts up to 90% opacity when the sampled backdrop is bright. The
settings window remains fully opaque.

Every launcher result has an icon. Applications, files, and Windows Settings
use their native Windows shell icons when available. Plugins use the inline SVG
from `plugin.json`, with a generic plugin fallback for older manifests. Built-in
calculator and URL actions use Steward-owned SVG icons.

## Windows Support

The process declares PerMonitorV2 DPI awareness before creating GPUI surfaces.
Each summon measures the monitor under the cursor, converts the launcher's
logical client size at that monitor's effective DPI, fits it to the work area,
and applies exact geometry after the cross-monitor DPI transition. Startup also
detects stale Alt/IME modifier state so the first keystroke after activation
reaches the focused NSH input instead of the system menu.

The tray, global hotkey, single-instance mutex, activation event, and autostart
`Run` value have explicit process-lifetime ownership. Autostart repair only
rewrites a dangling path, upgrades rewrite it to the installed executable, and
uninstall removes it. `scripts/test-windows-support.ps1` covers these paths,
mixed-DPI summoning, Unicode input, IME context attachment, invalid-package
containment, and support-report generation.

Support can run the shipped executable directly:

```text
steward-app.exe --write-diagnostics <output.json>
```

The report is generated without applying database migrations or initializing
the shell. It records executable and plugin-package hashes, monitor DPI and work
areas, database schema state, autostart registry truth, trust-root identity,
and recent error lines. It deliberately excludes clipboard contents and plugin
private storage.

## Process and Runtime Model

```text
steward-app.exe
  |
  +-- GPUI Fast application
        |
        +-- Steward launcher window
        |     +-- NSH Rust root
        |     +-- application search services
        |     +-- Windows Search SystemIndex client
        |     +-- Windows Settings page catalog
        |     +-- plugin route catalog
        |
        +-- Steward settings window
        |     +-- GPUI Kit Root + Settings
        |     +-- language, accent, autostart, hotkeys, update status
        |
        +-- plugin host process (one per open plugin presentation)
              +-- steward-app --plugin-host-process
              +-- nsh-platform-gpui AppView
              +-- windowed or managed embedded surface
              +-- one QuickJS VM
              +-- signed plugin.nshpkg
              +-- HostChannel bridge
              |
              +-- launcher-owned execution broker
```

The shell is Rust and contains no JavaScript VM. A plugin VM is created only
inside an isolated plugin host process started by
`crates/app/src/plugin_window.rs`. The shell and child communicate through
versioned, session-scoped NDJSON control and lifecycle messages. The child
independently verifies the package before creating its NSH window. The shell
accepts the receipt only after the child reports a real first render and the
fatal-error grace period completes.

This boundary isolates renderer and VM crashes from the launcher, but it is not
yet an OS security sandbox: children run as the current Windows user. Signed
package checks, the release capability ceiling, and NSH quotas remain separate
application-level controls. Each child is also assigned to a Windows Job Object
with a 256 MiB process/job memory limit, a 50% hard CPU-rate cap, one active
process, and kill-on-close lifetime ownership. The shell polls the child's
handle count and terminates the job above 1,024 handles. Unexpected exits never
enter an automatic restart loop; a later user launch is gated by per-plugin
2/4/8/16/30-second exponential backoff until a healthy first frame resets it.
This is still not an OS security sandbox and does not restrict filesystem,
network, registry, or token access.

Controlled execution does not spawn from the plugin host. A plugin host sends a
structured exact-executable request over the session-scoped NDJSON protocol;
the launcher rechecks active grants, executes the approved target in its own
kill-on-close Job Object, and returns bounded output. This avoids weakening the
plugin host's one-active-process limit. The broker has no shell or command-line
string, clears the inherited environment, confines the working directory to a
live write grant, and terminates the full process tree on timeout,
cancellation, output exhaustion, plugin exit, or broker shutdown.

GUI startup is guarded by a session-scoped Windows named mutex before storage,
search workers, tray, or windows are initialized. A secondary launch signals a
named activation event, causing the existing process to reveal or focus its
launcher, and then exits. Installer and headless utility modes run outside this
GUI ownership gate.

## Application Updates

Steward checks `iFence/steward` GitHub Releases on startup by default. The
preference is persisted in SQLite and can be changed under **Settings >
General**; **Settings > About** always offers a manual check and displays the
latest release notes and download progress.

Only a non-draft, non-prerelease tag newer than `CARGO_PKG_VERSION` is eligible.
The selected asset must be the `steward-app-<version>-<arch>.msi` matching the
running Windows architecture and a URL under that exact GitHub Release. Before
launching the installer through ShellExecute, Steward downloads and verifies it
against the release's `SHA256SUMS-msi.txt`. The updater then quits so the MSI's
existing major-upgrade flow can replace the running executable and preserve
autostart state. This host-only network path does not grant network capability
to plugins.

## Rust Modules

| Crate                     | Responsibility                                                                                      |
| ------------------------- | --------------------------------------------------------------------------------------------------- |
| `steward-app`             | GPUI Fast shell, tray, hotkey, search, and isolated plugin host process entry                       |
| `steward-core-engine`     | application matching, calculator, URL detection                                                     |
| `steward-search`          | shared pinyin haystacks and fuzzy-search vocabulary                                                 |
| `steward-plugin-contract` | versioned launch, process protocol, notice, error, and session DTOs                                 |
| `steward-plugin-registry` | strict `plugin.json` v2/v3 parser and SQLite route cache                                            |
| `steward-plugin-host`     | catalog/route facade, package trust, capability policy, asynchronous launch preparation             |
| `steward-plugin-store`    | package inspection, local lifecycle/history, marketplace index, publisher and revocation operations |
| `steward-plugin-cli`      | `steward-plugin` scaffolding, inspection, policy, and lifecycle CLI                                 |
| `steward-storage`         | SQLite settings, usage, app cache, and clipboard rows                                               |
| `steward-ui-components`   | NSH Rust launcher rows and palette                                                                  |

Steward's SQLite database records an explicit `PRAGMA user_version` and applies
migrations incrementally. Schema v2 adds publisher/version-scoped capability
grants and structured capability audit records; schema v3 adds plugin-scoped
host preference values. A database newer than the
application's supported schema is rejected instead of being rewritten.

The application splits launcher search into built-in, application, indexed
file, Windows Settings, and plugin sources. A narrow launcher service exposes
only the query, command, settings, and notice ports needed by the NSH shell.
Settings remain in the shell process; each plugin window has its own process
and host-channel composition root.

There is no separate plugin-runtime executable. The shipped `steward-app.exe`
starts in plugin-host child mode, and `steward-plugin-contract` owns the
versioned parent/child message types.

## Search and Launch Flow

Steward does not build a full-disk file index. Windows file search queries the
system `SystemIndex` through `ISearchQueryHelper` and OLE DB across every
location included in the Windows Search index. The query runs on a dedicated
worker thread and coalesces pending keystrokes to the latest text. If Windows
Search is unavailable, the launcher offers a Windows Search settings repair
action and does not fall back to directory walking. The storage migration
removes private file-index snapshots left by earlier releases.

1. At startup, the route catalog is loaded from the SQLite plugin metadata
   cache. A background rescan updates the cache without starting a VM.
2. Launcher searches fan out to the cached application index, the Windows
   Search worker, the in-memory Settings and plugin route indexes, and built-in
   commands.
3. Local results return immediately. A matching Windows Search result advances
   the search revision so the NSH shell re-queries and adds indexed files; stale
   file replies are discarded.
4. The Settings index reuses the shared `nucleo` matcher and pinyin haystacks;
   selecting a page launches its `ms-settings:` URI through ShellExecute.
5. The root results contain applications, indexed files, Windows Settings
   pages, calculators, URL actions, and plugin routes.
6. Selecting a plugin route sends a launch request to the app event loop.
7. `PluginHost::prepare_launch` reads the package, verifies its signature,
   certificate, revocation, and rollback state, checks package
   ID/version against `plugin.json`, applies the release capability policy, and
   returns an NSH `LoadedPackage`.
8. The app opens or focuses the single window for that plugin ID. A repeat
   command focuses the window and emits `steward.command`.

## Plugin Contract

The installed layout is:

```text
%APPDATA%\Steward\plugins\<plugin-id>\
  plugin.json
  plugin.nshpkg
```

`plugin.json` contains routing metadata and the optional host-rendered
preference schema:

```json
{
  "schemaVersion": 3,
  "id": "it.aspirin.calendar",
  "name": "Calendar",
  "version": "0.1.4",
  "package": "plugin.nshpkg",
  "preferences": {
    "schemaVersion": 1,
    "fields": [
      {
        "key": "showHints",
        "title": "Show hints",
        "type": "boolean",
        "default": true
      }
    ]
  },
  "commands": [
    {
      "name": "calendar",
      "title": "Calendar",
      "keywords": ["calendar"],
      "trigger": { "type": "command" },
      "presentation": "embedded"
    }
  ]
}
```

Supported triggers are command name, longest matching prefix, and regex.
Protected command metadata selects `windowed` or `embedded` presentation.
Schema v2 remains readable and defaults to `windowed`; schema v3 is required
for `embedded`.
Preference fields support text, boolean, select, folder, and application values.
Dynamic triggers and inline plugin results no longer exist.

The NSH package contains the TSX application and UI assets. Its package ID and
version must equal the route manifest. The exact public surface is documented
in [nsh-plugin-api.md](nsh-plugin-api.md) and
[plugin-manifest-spec.md](plugin-manifest-spec.md).

## Capability and Resource Policy

The release policy ceiling still contains:

- clipboard history;
- read/write access scoped to `${dataDir}`;
- user-selected file and folder roots with read or read-write grants;
- bounded filesystem metadata walks, text read/write, and cancellation;
- default-path open, Explorer reveal, and catalog-only open-with;
- installed-application discovery;
- opening https and mailto URLs;
- scoped HTTPS requests with exact host, method, and optional path grants;
- exact non-shell `.exe` execution through the launcher-owned broker;
- named Steward host calls, with `steward.*` retained only for official
  migration packages.

Raw sockets, non-HTTPS network access, shell/interpreter execution,
unrestricted network hosts, webview, generic NSH image-picker, and unrestricted
filesystem roots are denied.
The policy is applied again when a package is prepared for launch; package
metadata cannot grant itself extra capabilities.

The current host bridge exposes clipboard history and writes, plugin-private
string storage, command/theme events, plugin-window close, resolved
preferences, toast, confirmation, progress, user-selected filesystem roots,
installed applications, system-open operations, scoped HTTPS, controlled
execution, and presentation navigation. Clipboard, storage, network, process,
and resource calls are
capability-gated by the signed package plus the release policy; plugin-private
storage is isolated under the plugin's data directory.

PC-0 adds a semantic host API version, `minHostVersion`, feature discovery,
stable errors, and launch-time intersection with active SQLite grants. New
third-party packages cannot declare `steward.*`. Every launch records allowed
and denied declarations, and Steward host-call handlers append payload-free
allow audit events.

PC-1 adds schema-bounded preferences and feedback APIs. Preference rows are
keyed by plugin ID so they survive package updates, and Settings renders them
without starting a VM. Toast and confirmation requests cross the
session-scoped child protocol; the launcher owns confirmation resolution.
Progress calls are delivered back to the plugin as `steward.progress` events.
Folder and application preference values are path-validated but remain values:
they do not create filesystem grants or launch targets.

PC-2 adds native file/folder selection. A selection is canonicalized and stored
as a resource-scoped grant in the existing capability table. Filesystem and
system-open handlers query matching grants on every call, reject UNC and
out-of-root paths, and treat symlinks, junctions, and other reparse points as
non-traversable during walks. Path work runs on bounded worker queues; walks
have depth, result, and deadline limits plus caller-driven cancellation.
System opening accepts a path plus either the default handler or an exact
installed-application ID, never a shell command line.

PC-3 adds scoped HTTPS and controlled execution. Packages declare
`steward.net.fetch` with `network.http` host/method/path grants, and
`steward.exec.run` with exact `.exe` targets under `process.execute`. Runtime
handlers reload active grants, reject non-HTTPS/local-network requests and
shell or interpreter executables, and write capability audit records without
storing request or response payloads. Network redirects are checked
individually against the same grant. Execution arguments remain a string array
through `Command`, and both network and process operations have bounded
concurrency, timeout, and output or response limits.

PC-4 adds schema-v3 per-command presentation. `embedded` starts the same
isolated plugin-host child as `windowed`, but renders a launcher-positioned
frameless, non-resizable surface. Back, Esc, close, and plugin-requested
navigation flush a lifecycle event, destroy the child process, and restore
launcher focus. The design intentionally does not move the QuickJS VM into the
shell or claim a cross-process render bridge.

PDX-3 freezes the command-oriented bridge as host API `1.0.0`. The contract
defines the ten public API families, their quotas, and the stable mapping for
disabled, revoked, expired, malformed, timed-out, cancelled, and
resource-exhausted failures. The API reference is generated from that contract,
and compatibility fixtures replay the supported `0.3.x` through `0.5.x` lines
through current schemas and host DTOs. The same-user process boundary remains
explicit: isolation limits crash propagation and host-call reach, but it is not
an operating-system security boundary between processes running as one user.

VM limits:

| Limit       |  Value |
| ----------- | -----: |
| Heap        | 64 MiB |
| Stack       |  1 MiB |
| Load        |    2 s |
| One JS call | 250 ms |
| UI nodes    | 20,000 |

## Trust and Packaging

Production roots are configured in `assets/nsh/trust-roots.json`. The
production Ed25519 root public key is committed there, while its secret is
kept offline. CI supplies the certified release signing key through
`STEWARD_NSH_RELEASE_KEY` and its certificate through
`STEWARD_NSH_RELEASE_CERT`. Release package launch still fails closed if no
trusted root accepts the package certificate.

`scripts/package-plugins.ps1` creates one zip per official plugin. Each zip
contains only the top-level plugin directory, `plugin.json`, and the verified
`plugin.nshpkg`. `scripts/package-msi.ps1` packages the app; no plugin runtime
or file-index helper executable is installed.

## Plugin Management

The Settings > Plugins page and `steward-plugin` CLI share
`steward-plugin-store`. The store enforces the two-file archive contract,
re-verifies packages through NSH, checks manifest/package identity and build,
records publisher key identity, applies Steward's release-policy review, and
performs staged install/update/remove/rollback operations. Managed history and
store state live beside the installed plugins; the route cache is refreshed
after each mutation.

The local marketplace is an index over operator-provided archives under
`<data-dir>/marketplace/packages`. An index entry records artifact and package
hashes, version/build, and publisher key ID, but package trust is always checked
again when it is installed. Root-signed NSH revocation lists can be applied to
the same persistent trust store used by plugin launch.

PC-5 adds a versioned public-admission review over the same inspection result.
It requires a trusted publisher, compatible host API, successful release policy,
and named capability declarations with no manual-review warnings.
`marketplace sync --public` uses that gate before writing an index; local sync
continues to surface review-required entries for development. The public
marketplace and publisher registration service are not enabled yet. See
[plugin-ecosystem.md](plugin-ecosystem.md) and
[plugin-public-admission.md](plugin-public-admission.md).

## Verification and Operations

CI runs Rust formatting, clippy, workspace tests, TypeScript lint/format/type
checks/builds, and headless NSH plugin tests. Release performance and clean-MSI
acceptance are tracked in [benchmarks.md](benchmarks.md) and
[nsh-migration.md](nsh-migration.md). Windows support acceptance uses
`scripts/test-windows-support.ps1`; clean-VM MSI lifecycle evidence remains a
separate gate.

The pre-migration architecture and decision log are preserved in
[architecture-history.md](architecture-history.md) for context. New decisions
must be recorded in this file and linked from the migration ledger.
