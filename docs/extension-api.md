# Steward Extension API v2

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
import { Clipboard } from "nsh/clipboard";
import {
  clipboardHistory,
  closeWindow,
  launchParams,
  onCommand,
  onTheme,
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
}
```

`input` is the query text after a built-in prefix trigger, or the full query
for a regex trigger. A plugin window is reused by plugin ID, so handle
`onCommand` rather than assuming each command creates a new window.

## Host calls

| Helper | Result |
|---|---|
| `clipboardHistory()` | Current clipboard entries from Steward |
| `closeWindow()` | Close the current plugin window |
| `onCommand(cb)` | Receive focus/command events while the window stays open |
| `onTheme(cb)` | Receive accent and dark-mode changes |

The current release grants clipboard history, `${dataDir}` filesystem access,
https/mailto opening, and `steward.*` host calls. Network, process execution,
webview, and image-picker capabilities are denied.

## Example

```tsx
import { mount, signal, Text, View } from "nsh";
import { launchParams, onCommand } from "@steward/extension-api";

function App() {
  const input = signal(launchParams().input);
  onCommand((event) => input.set(event.input));

  return (
    <View style={{ flex: 1, padding: 24, backgroundColor: "#202024" }}>
      <Text style={{ color: "#ffffff" }}>Query: {input()}</Text>
    </View>
  );
}

mount(App);
```

Old `permissions`, `entry`, `runtime`, detached panel, and dynamic trigger
manifest fields are rejected by schema v2.
