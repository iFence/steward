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
  const value = host.params as unknown as Partial<StewardLaunchParams>;
  return {
    pluginId: value.pluginId ?? "",
    command: value.command ?? "",
    input: value.input ?? "",
    locale: value.locale ?? "en",
    accent: value.accent ?? "#863bff",
    dark: value.dark ?? true,
  };
}

export function onCommand(
  listener: (command: { command: string; input: string }) => void,
): () => void {
  return host.on("steward.command", (value) => {
    listener(value as { command: string; input: string });
  });
}

export function onTheme(listener: (theme: { accent: string; dark: boolean }) => void): () => void {
  return host.on("steward.theme", (value) => {
    listener(value as { accent: string; dark: boolean });
  });
}

export async function clipboardHistory(): Promise<ClipboardEntry[]> {
  return host.call<ClipboardEntry[]>("steward.clipboard.history");
}

export function closeWindow(): void {
  host.post("steward.window.close", null);
}
