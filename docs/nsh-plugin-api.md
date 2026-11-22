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
  presentation: "windowed" | "embedded";
  hostApiVersion: string;
  features: string[];
  preferences: Record<string, unknown>;
}
```

## Steward host bridge

The canonical helper-visible bridge, including request and response examples,
quotas, and failure behavior for every API family, is the generated
[plugin API reference](plugin-api-reference.md). These helpers live in
`@steward/extension-api`. UI and device capabilities use the modules exported
by the pinned NSH 0.6.1 runtime.

`calendarHolidays(year)` returns a map keyed by ISO date with values `holiday`
or `workday`. Steward fetches a year from
[Timor's free holiday API](https://timor.tech/api/holiday/) only when the year is
not cached, stores the normalized result under its data directory, and returns
an empty schedule when no cached or live data is available. The plugin does not
receive general network capability through this host call.

The embedding host allows clipboard, filesystem under `${dataDir}`,
user-selected scoped roots, bounded filesystem operations, installed-app
discovery, system open/reveal/open-with, https/mailto URL opening, scoped
HTTPS fetch, exact non-shell executable execution, and isolated presentation
navigation. Raw sockets, local-network requests, webview, unrestricted
filesystem roots, and the generic NSH image picker are denied.

Plugins and Steward run as the same Windows user. Process isolation limits VM
crash containment, host-call reach, and controlled-process lifetime; it is not
an operating-system security boundary against another process already running
as that user. Unsupported capabilities include dynamic launcher search
providers, background services, cross-plugin IPC, arbitrary WebView/process
access, raw sockets, shared credential brokers, binary file I/O,
Recycle Bin operations, and file/clipboard change subscriptions.

Each open plugin gets 64 MiB heap, 1 MiB stack, a 2 second load budget, a
250 ms per-call budget, and at most 20,000 UI nodes.

Third-party packages negotiate `minHostVersion`, use named permissions, and
receive an effective grant narrowed by active SQLite user grants. PC-1 adds
hosted preferences and feedback. PC-2 adds selected-path grants, bounded
filesystem operations, application discovery, and system-open calls. PC-3 adds
scoped HTTPS and launcher-brokered exact-executable execution with process-tree
cleanup. PC-4 adds schema-v3 `windowed`/`embedded` command routing plus back,
close, title, visibility, focus restoration, and runtime teardown without
moving plugin code into the shell. Unrestricted roots and raw sockets remain
denied. Official migration packages retain the legacy ceiling. See
[plugin-host-api-compatibility.md](plugin-host-api-compatibility.md).

For end-to-end local development, host-mocked tests, development signing, and
isolated Steward integration, see
[Plugin development and local testing (Chinese)](plugin-development-zh-CN.md).
