import { host } from "nsh/host";

export interface StewardLaunchParams {
  pluginId: string;
  command: string;
  input: string;
  locale: string;
  accent: string;
  dark: boolean;
}

export interface ClipboardEntry {
  id: string;
  text: string;
  copied_at: number;
}

export function launchParams(): StewardLaunchParams {
  const value = (host.params ?? {}) as Partial<StewardLaunchParams>;
  return {
    pluginId: value.pluginId ?? "",
    command: value.command ?? "",
    input: value.input ?? "",
    locale: value.locale ?? "en",
    accent: value.accent ?? "#863bff",
    dark: value.dark ?? true,
  };
}

export function onCommand(listener: (command: { command: string; input: string }) => void): {
  remove(): void;
} {
  return host.on("steward.command", (value) => {
    listener(value as { command: string; input: string });
  });
}

export function onTheme(listener: (theme: { accent: string; dark: boolean }) => void): {
  remove(): void;
} {
  return host.on("steward.theme", (value) => {
    listener(value as { accent: string; dark: boolean });
  });
}

export async function clipboardHistory(): Promise<ClipboardEntry[]> {
  return (await host.call("steward.clipboard.history")) as ClipboardEntry[];
}

export function closeWindow(): void {
  host.post("steward.window.close", null);
}
