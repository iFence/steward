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
| `closeWindow()`          | Ask Steward to close this plugin window.                 |
| `onCommand(cb)`          | Receive another command while the plugin window is open. |
| `onTheme(cb)`            | Receive accent/dark-mode changes.                        |

These helpers live in `@steward/extension-api`. UI and device capabilities use
the modules exported by the pinned NSH 0.5.1 runtime.

The embedding host allows clipboard, filesystem under `${dataDir}`,
https/mailto URL opening, and Steward host names. Network, process execution,
webview, and image picker are denied in v1.

Each open plugin gets 64 MiB heap, 1 MiB stack, a 2 second load budget, a
250 ms per-call budget, and at most 20,000 UI nodes.

For end-to-end local development, host-mocked tests, development signing, and
isolated Steward integration, see
[Plugin development and local testing (Chinese)](plugin-development-zh-CN.md).
