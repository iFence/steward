# Steward Extension API v5

> Host API version: `0.5.0`. See
> [plugin-host-api-compatibility.md](plugin-host-api-compatibility.md) for
> version negotiation, feature discovery, and stable error codes.

NSH is the plugin UI and runtime API. Steward no longer exposes a serializable
JSON View or Virtual UI Tree API. Plugins import UI primitives from `nsh`,
components from `nsh/base`, platform capabilities from standard NSH modules,
and only Steward-specific calls from `@steward/extension-api`.

See [nsh-plugin-api.md](nsh-plugin-api.md) for the complete contract. The
canonical TypeScript surface is
[`packages/extension-api/src/host.ts`](../packages/extension-api/src/host.ts).

## Imports

```tsx
import { Input, mount, signal, Text, View } from "nsh";
import { Switch } from "nsh/base";
import {
  calendarHolidays,
  backToLauncher,
  clipboardHistory,
  closePage,
  closeWindow,
  confirm,
  filesystemGrants,
  fetchHttp,
  hostInfo,
  installedApplications,
  copyText,
  launchParams,
  onCommand,
  onProgress,
  onTheme,
  onVisibility,
  openPath,
  openWith,
  pickFile,
  pickFolder,
  preference,
  preferences,
  reportProgress,
  setPageTitle,
  readText,
  revealPath,
  runProcess,
  cancelProcess,
  storageGet,
  storageSet,
  supportsFeature,
  requireFeature,
  toast,
  walk,
  writeText,
} from "@steward/extension-api";
```

## Launch data

`launchParams()` returns:

```ts
interface StewardLaunchParams {
  pluginId: string;
  command: string;
  input: string;
  locale: string;
  accent: string;
  dark: boolean;
  presentation: "windowed" | "embedded";
  hostApiVersion: string;
  features: string[];
  preferences: Record<string, unknown>;
}
```

`preferences()` returns the resolved host-owned preference map.
`preference(key, fallback)` is the typed convenience form:

```ts
const density = preference<"compact" | "comfortable">("density", "comfortable");
```

`input` is the query text after a built-in prefix trigger, or the full query
for a regex trigger. A plugin window is reused by plugin ID, so handle
`onCommand` rather than assuming each command creates a new window.

## Host calls

| Helper                           | Result                                                                 |
| -------------------------------- | ---------------------------------------------------------------------- |
| `clipboardHistory()`             | Current clipboard entries from Steward                                 |
| `copyText(text)`                 | Writes text through Steward's clipboard capability                     |
| `storageGet(key)`                | Reads plugin-private string storage                                    |
| `storageSet(key, value)`         | Persists plugin-private string storage                                 |
| `calendarHolidays(year)`         | Reads Steward's cached holiday schedule for a year                     |
| `closeWindow()`                  | Close the current plugin window                                        |
| `backToLauncher()`               | Leave the plugin page and restore launcher focus                       |
| `closePage()`                    | Close the current plugin page                                          |
| `setPageTitle(title)`            | Set a bounded plugin-page title                                        |
| `preferences()` / `preference()` | Read host-resolved plugin preferences                                  |
| `toast(options)`                 | Show a transient launcher notice                                       |
| `confirm(options)`               | Ask the launcher to confirm an action; resolves to `false` when denied |
| `reportProgress(update)`         | Report bounded progress for a long operation                           |
| `pickFolder(write?)`             | Ask the user for a folder and persist a read or read-write grant       |
| `pickFile()`                     | Ask the user for a file and persist a read grant                       |
| `filesystemGrants()`             | List the plugin's active selected roots and access modes               |
| `walk(options)`                  | Bounded, cancellable recursive metadata walk under a selected root     |
| `cancelWalk(id)`                 | Cancel a walk by its caller-supplied ID                                |
| `readText(path, maxBytes?)`      | Read bounded UTF-8 text under an active read grant                     |
| `writeText(path, contents)`      | Write bounded text under an active write grant                         |
| `installedApplications()`        | List launch targets in Steward's installed-app catalog                 |
| `openPath(path)`                 | Open a selected path with its registered default handler               |
| `revealPath(path)`               | Show a selected path in File Explorer                                  |
| `openWith(applicationId, path)`  | Open a selected path with an app from the installed-app catalog        |
| `fetchHttp(options)`             | Make a scoped HTTPS request with redirect and size enforcement         |
| `runProcess(options)`            | Run an exact approved executable through the launcher broker           |
| `cancelProcess(requestId)`       | Cancel a controlled process and its tree by request ID                 |
| `onProgress(cb)`                 | Receive host progress events                                           |
| `onCommand(cb)`                  | Receive focus/command events while the window stays open               |
| `onTheme(cb)`                    | Receive accent and dark-mode changes                                   |
| `onVisibility(cb)`               | Receive page visibility changes                                        |
| `hostInfo()`                     | Read the host API version and available feature names                  |
| `supportsFeature(name)`          | Test one available feature before calling it                           |
| `requireFeature(name)`           | Throw `StewardError("feature_unavailable")` when absent                |

The current release ceiling allows clipboard history, `${dataDir}` filesystem
access, selected-path host calls, system open/reveal with a selected root,
installed-app discovery, https/mailto opening, scoped HTTPS fetch, and exact
approved executable execution. Official packages use named host calls; older
installed packages retain `steward.*` compatibility until they are updated.
New third-party packages must use named host calls and active grants. Raw
sockets, local-network requests, shell/interpreter execution, webview,
arbitrary filesystem roots, and the generic NSH image picker remain denied.

New third-party packages must declare `minHostVersion` and use named
host permissions. Permission declarations are requests, not grants. Sensitive
calls require a matching active user grant and are audited at launch and when
the Steward host handler executes. Settings > Plugins lets users allow or
revoke each declared sensitive capability while Steward is running.

Preferences are declared in `plugin.json`, so they can be edited in Settings
without opening the plugin process. Toast, confirmation, and progress calls are
automatic named capabilities:

```json
{
  "host": ["steward.feedback.toast", "steward.feedback.confirm", "steward.feedback.progress"]
}
```

Confirmation is host-owned and must be used before destructive or externally
visible actions.

## Presentation and navigation

The command manifest declares `presentation: "windowed"` or `"embedded"`.
`launchParams().presentation` exposes the verified value. Embedded commands
still run in the isolated plugin-host child process and use a
launcher-positioned frameless hosted surface.

```json
{
  "capabilities": {
    "host": ["steward.navigation.back", "steward.navigation.setTitle"]
  }
}
```

```tsx
import { backToLauncher, onVisibility, setPageTitle } from "@steward/extension-api";

const visible = signal(true);
onVisibility((next) => visible.set(next));

async function leavePage() {
  await setPageTitle("Repositories");
  await backToLauncher();
}
```

Back, Esc, or `closePage()` destroys the plugin host process. The launcher is
shown and focused again only for embedded presentations. `onVisibility`
reports activation changes without exposing window handles.

## Filesystem and system resources

Filesystem declarations are requests for the host API, not broad directory
access. Calling `pickFolder()` or `pickFile()` shows Steward's native dialog and
stores the user's exact canonical selection in the capability grant table.
Every later operation reloads those grants, canonicalizes the requested path,
and rejects paths outside them. Directory walks also have result, depth,
deadline, and cancellation limits and never descend through symlinks,
junctions, or other Windows reparse points.

```json
{
  "capabilities": {
    "host": [
      "steward.fs.pick",
      "steward.fs.walk",
      "steward.fs.readText",
      "steward.system.openPath",
      "steward.system.revealPath"
    ]
  }
}
```

`steward.system.openWith` accepts only an ID returned by
`installedApplications()` and one selected path. It never accepts a command
line. `steward.system.openPath` and `steward.system.revealPath` require read
access to the path. Revoking a selected root in Settings blocks the next call
without requiring a plugin restart.

`@steward/plugin-local-files` and `@steward/plugin-git-repos` exercise the same
APIs from unrelated document-inspection and repository-discovery workloads.

## Network and controlled execution

Network declarations combine the named host call with exact resource scopes:

```json
{
  "capabilities": {
    "host": ["steward.net.fetch"],
    "network": {
      "http": [
        {
          "host": "api.example.com",
          "schemes": ["https"],
          "methods": ["GET"],
          "pathPrefixes": ["/v1"]
        }
      ]
    }
  }
}
```

`fetchHttp()` supports `GET`, `HEAD`, `POST`, `PUT`, `PATCH`, and `DELETE`.
The initial URL and every redirect are checked against an active matching
grant. Only HTTPS and public DNS names are accepted; IP literals, localhost,
`.local`, `.internal`, embedded URL credentials, raw sockets, and arbitrary
ports are denied. Request bodies are capped at 1 MiB, responses at 4 MiB,
timeouts at 30 seconds, and redirects at five. Reserved headers such as `Host`,
`Content-Length`, and `Transfer-Encoding` cannot be supplied by the plugin.

Execution requires both the named call and an exact executable grant:

```json
{
  "capabilities": {
    "host": ["steward.exec.run", "steward.exec.cancel"],
    "process": { "execute": ["git.exe"] }
  }
}
```

```ts
const result = await runProcess({
  id: "git-status-1",
  executable: "git.exe",
  arguments: ["status", "--porcelain=v1", "--branch"],
  workingDirectory: selectedRepositoryRoot,
  timeoutMs: 30_000,
  maxOutputBytes: 1024 * 1024,
});
```

`workingDirectory` must be inside a live write grant. Arguments are passed as
an array, never through a shell or command-line string. Shells and common
script runtimes are rejected even if declared. The launcher clears inherited
environment variables, enforces argument/runtime/output limits, and owns the
process tree's kill-on-close Job Object. `cancelProcess(requestId)` requests
termination of the same tree.

The Network Check plugin validates the HTTPS path while Git Repositories uses
the same execution API for status, remote inspection, unpushed-commit counts,
and clone.

## Example

```tsx
import { mount, signal, Text, View } from "nsh";
import { onCommand, preference } from "@steward/extension-api";

function App() {
  const input = signal("");
  const title = preference("title", "Plugin");
  onCommand((event) => input.set(event.input));

  return (
    <View style={{ flex: 1, padding: 24, backgroundColor: "#202024" }}>
      <Text style={{ color: "#ffffff" }}>
        {title}: {input()}
      </Text>
    </View>
  );
}

mount(App);
```

Old `permissions`, `entry`, `runtime`, detached panel, and dynamic trigger
manifest fields are rejected. Presentation requires schema v3; schema-v2
manifests remain readable as `windowed`.
