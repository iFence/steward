# NSH Plugin API

Plugins are TSX applications compiled and signed with NSH. UI comes from
`nsh`, `nsh/base`, and standard NSH modules; Steward adds only a small typed
host bridge.

## Launch parameters

```ts
interface StewardLaunchParams {
  pluginId: string;
  command: string;
  input: string;
  locale: string;
  accent: string;
  dark: boolean;
  hostApiVersion: string;
  features: string[];
  preferences: Record<string, unknown>;
}
```

## Steward host bridge

| Interface                 | Purpose                                                  |
| ------------------------- | -------------------------------------------------------- |
| `clipboardHistory()`      | Read Steward's clipboard-history snapshot.               |
| `copyText(text)`          | Write text through Steward's clipboard capability.       |
| `storageGet(key)`         | Read a plugin-private string value.                      |
| `storageSet(key, value)`  | Persist a plugin-private string value.                   |
| `calendarHolidays(year)`  | Read Steward's cached mainland China holiday schedule.   |
| `closeWindow()`           | Ask Steward to close this plugin window.                 |
| `preferences()`           | Read all host-resolved preferences.                      |
| `preference(key, value)`  | Read one preference with a typed fallback.               |
| `toast(options)`          | Show a transient launcher notice.                        |
| `confirm(options)`        | Ask the launcher for explicit confirmation.              |
| `reportProgress(update)`  | Report bounded progress for a long operation.            |
| `pickFolder(write?)`      | Select and persistently grant a file/folder root.        |
| `filesystemGrants()`      | List active selected roots and read/write modes.         |
| `walk(options)`           | Run a bounded, cancellable metadata walk.                |
| `cancelWalk(id)`          | Cancel a walk by ID.                                     |
| `readText(path)`          | Read UTF-8 text under an active read grant.              |
| `writeText(path, text)`   | Write UTF-8 text under an active write grant.            |
| `installedApplications()` | List installed open-with targets.                        |
| `openPath(path)`          | Open a selected path with its default handler.           |
| `revealPath(path)`        | Reveal a selected path in Explorer.                      |
| `openWith(appId, path)`   | Open a selected path with a catalog application.         |
| `fetchHttp(options)`      | Make a scoped HTTPS request through Steward.             |
| `runProcess(options)`     | Run an exact approved executable through the broker.     |
| `cancelProcess(id)`       | Cancel a controlled process tree.                        |
| `onProgress(cb)`          | Receive `steward.progress` events.                       |
| `onCommand(cb)`           | Receive another command while the plugin window is open. |
| `onTheme(cb)`             | Receive accent/dark-mode changes.                        |
| `hostInfo()`              | Read the host API version and available features.        |
| `supportsFeature(name)`   | Check an optional feature before using it.               |
| `requireFeature(name)`    | Throw a stable `feature_unavailable` error when absent.  |

These helpers live in `@steward/extension-api`. UI and device capabilities use
the modules exported by the pinned NSH 0.6.1 runtime.

`calendarHolidays(year)` returns a map keyed by ISO date with values `holiday`
or `workday`. Steward fetches a year from
[Timor's free holiday API](https://timor.tech/api/holiday/) only when the year is
not cached, stores the normalized result under its data directory, and returns
an empty schedule when no cached or live data is available. The plugin does not
receive general network capability through this host call.

The embedding host allows clipboard, filesystem under `${dataDir}`,
user-selected scoped roots, bounded filesystem operations, installed-app
discovery, system open/reveal/open-with, https/mailto URL opening, scoped
HTTPS fetch, and exact non-shell executable execution. Raw sockets,
local-network requests, webview, unrestricted filesystem roots, and the generic
NSH image picker are denied.

Each open plugin gets 64 MiB heap, 1 MiB stack, a 2 second load budget, a
250 ms per-call budget, and at most 20,000 UI nodes.

Third-party packages negotiate `minHostVersion`, use named permissions, and
receive an effective grant narrowed by active SQLite user grants. PC-1 adds
hosted preferences and feedback. PC-2 adds selected-path grants, bounded
filesystem operations, application discovery, and system-open calls. PC-3 adds
scoped HTTPS and launcher-brokered exact-executable execution with process-tree
cleanup; unrestricted roots and raw sockets remain denied. Official migration
packages retain the legacy ceiling. See
[plugin-host-api-compatibility.md](plugin-host-api-compatibility.md).

For end-to-end local development, host-mocked tests, development signing, and
isolated Steward integration, see
[Plugin development and local testing (Chinese)](plugin-development-zh-CN.md).
