import { host } from "nsh";

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

export type HolidayDayKind = "holiday" | "workday";
export type HolidaySchedule = Record<string, HolidayDayKind>;

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

export async function copyText(text: string): Promise<void> {
  await host.call("steward.clipboard.write", { text });
}

export async function storageGet(key: string): Promise<string | null> {
  return host.call<string | null>("steward.storage.get", { key });
}

export async function storageSet(key: string, value: string): Promise<void> {
  await host.call("steward.storage.set", { key, value });
}

export async function calendarHolidays(year: number): Promise<HolidaySchedule> {
  if (!Number.isInteger(year)) throw new Error("calendarHolidays requires an integer year");
  return host.call<HolidaySchedule>("steward.calendar.holidays", { year }, { timeout: 20_000 });
}

export function closeWindow(): void {
  host.post("steward.window.close", null);
}
