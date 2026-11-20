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
}
```

## Steward host bridge

| Interface                | Purpose                                                  |
| ------------------------ | -------------------------------------------------------- |
| `clipboardHistory()`     | Read Steward's clipboard-history snapshot.               |
| `copyText(text)`         | Write text through Steward's clipboard capability.       |
| `storageGet(key)`        | Read a plugin-private string value.                      |
| `storageSet(key, value)` | Persist a plugin-private string value.                   |
| `calendarHolidays(year)` | Read Steward's cached mainland China holiday schedule.   |
| `closeWindow()`          | Ask Steward to close this plugin window.                 |
| `onCommand(cb)`          | Receive another command while the plugin window is open. |
| `onTheme(cb)`            | Receive accent/dark-mode changes.                        |

These helpers live in `@steward/extension-api`. UI and device capabilities use
the modules exported by the pinned NSH 0.6.1 runtime.

`calendarHolidays(year)` returns a map keyed by ISO date with values `holiday`
or `workday`. Steward fetches a year from
[Timor's free holiday API](https://timor.tech/api/holiday/) only when the year is
not cached, stores the normalized result under its data directory, and returns
an empty schedule when no cached or live data is available. The plugin does not
receive general network capability through this host call.

The embedding host allows clipboard, filesystem under `${dataDir}`,
https/mailto URL opening, and Steward host names. General plugin network,
process execution, webview, and image picker are denied in Steward v1.

Each open plugin gets 64 MiB heap, 1 MiB stack, a 2 second load budget, a
250 ms per-call budget, and at most 20,000 UI nodes.

For end-to-end local development, host-mocked tests, development signing, and
isolated Steward integration, see
[Plugin development and local testing (Chinese)](plugin-development-zh-CN.md).
