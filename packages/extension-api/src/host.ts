import { host } from "nsh";

export interface StewardLaunchParams {
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

export interface ClipboardEntry {
  id: string;
  text: string;
  copied_at: number;
}

export type HolidayDayKind = "holiday" | "workday";
export type HolidaySchedule = Record<string, HolidayDayKind>;
export type StewardErrorCode =
  | "permission_denied"
  | "feature_unavailable"
  | "timeout"
  | "resource_exhausted"
  | "host_failure"
  | "invalid_input"
  | "cancelled"
  | "confirmation_denied"
  | "io_failure";

export type PathPickMode = "folderRead" | "folderWrite" | "fileRead";

export interface FilesystemGrant {
  path: string;
  read: boolean;
  write: boolean;
}

export interface WalkOptions {
  root: string;
  id?: string;
  maxResults?: number;
  maxDepth?: number;
  timeoutMs?: number;
}

export interface WalkEntry {
  path: string;
  relativePath: string;
  directory: boolean;
  reparsePoint: boolean;
  size: number;
}

export interface WalkResult {
  root: string;
  entries: WalkEntry[];
  truncated: boolean;
  elapsedMs: number;
}

export interface InstalledApplication {
  id: string;
  name: string;
}

export interface StewardErrorObject {
  code: StewardErrorCode;
  message: string;
  retryable: boolean;
}

export interface HostInfo {
  apiVersion: string;
  features: string[];
}

export class StewardError extends Error {
  readonly code: StewardErrorCode;
  readonly retryable: boolean;

  constructor(code: StewardErrorCode, message: string, retryable = false) {
    super(message);
    this.name = "StewardError";
    this.code = code;
    this.retryable = retryable;
  }
}

function rawLaunchParams(): Partial<StewardLaunchParams> {
  return host.params as unknown as Partial<StewardLaunchParams>;
}

export function launchParams(): StewardLaunchParams {
  const value = rawLaunchParams();
  return {
    pluginId: value.pluginId ?? "",
    command: value.command ?? "",
    input: value.input ?? "",
    locale: value.locale ?? "en",
    accent: value.accent ?? "#863bff",
    dark: value.dark ?? true,
    hostApiVersion: value.hostApiVersion ?? "0.0.0",
    features: value.features ?? [],
    preferences: value.preferences ?? {},
  };
}

export function hostInfo(): HostInfo {
  const params = launchParams();
  return {
    apiVersion: params.hostApiVersion,
    features: [...params.features],
  };
}

export function supportsFeature(feature: string): boolean {
  return hostInfo().features.includes(feature);
}

export function requireFeature(feature: string, message?: string): void {
  if (!supportsFeature(feature)) {
    throw new StewardError(
      "feature_unavailable",
      message ?? `Steward feature '${feature}' is unavailable`,
    );
  }
}

export function preferences<T extends Record<string, unknown> = Record<string, unknown>>(): T {
  return launchParams().preferences as T;
}

export function preference<T>(key: string, fallback: T): T {
  const value = preferences()[key];
  return value === undefined ? fallback : (value as T);
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
  try {
    return await host.call<ClipboardEntry[]>("steward.clipboard.history");
  } catch (error) {
    throw hostCallError("steward.clipboard.history", error);
  }
}

export async function copyText(text: string): Promise<void> {
  try {
    await host.call("steward.clipboard.write", { text });
  } catch (error) {
    throw hostCallError("steward.clipboard.write", error);
  }
}

export async function storageGet(key: string): Promise<string | null> {
  try {
    return await host.call<string | null>("steward.storage.get", { key });
  } catch (error) {
    throw hostCallError("steward.storage.get", error);
  }
}

export async function storageSet(key: string, value: string): Promise<void> {
  try {
    await host.call("steward.storage.set", { key, value });
  } catch (error) {
    throw hostCallError("steward.storage.set", error);
  }
}

export async function calendarHolidays(year: number): Promise<HolidaySchedule> {
  if (!Number.isInteger(year)) throw new Error("calendarHolidays requires an integer year");
  try {
    return await host.call<HolidaySchedule>(
      "steward.calendar.holidays",
      { year },
      { timeout: 20_000 },
    );
  } catch (error) {
    throw hostCallError("steward.calendar.holidays", error);
  }
}

export function closeWindow(): void {
  host.post("steward.window.close", null);
}

export interface ToastOptions {
  title: string;
  message: string;
}

export async function toast(options: ToastOptions): Promise<void> {
  if (!options.title.trim() || !options.message.trim()) {
    throw new StewardError("invalid_input", "toast requires a title and message");
  }
  try {
    await host.call("steward.feedback.toast", options);
  } catch (error) {
    throw hostCallError("steward.feedback.toast", error);
  }
}

export interface ConfirmOptions {
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  destructive?: boolean;
}

export async function confirm(options: ConfirmOptions): Promise<boolean> {
  if (!options.title.trim() || !options.message.trim()) {
    throw new StewardError("invalid_input", "confirm requires a title and message");
  }
  try {
    const result = await host.call<{ confirmed: boolean }>("steward.feedback.confirm", options, {
      timeout: 300_000,
    });
    return result.confirmed;
  } catch (error) {
    const mapped = hostCallError("steward.feedback.confirm", error);
    if (mapped.code === "confirmation_denied" || mapped.code === "cancelled") {
      return false;
    }
    throw mapped;
  }
}

export interface ProgressUpdate {
  id: string;
  message: string;
  current?: number;
  total?: number;
}

export async function reportProgress(update: ProgressUpdate): Promise<void> {
  if (!update.id || !update.message) {
    throw new StewardError("invalid_input", "progress requires an id and message");
  }
  if (
    update.current !== undefined &&
    (!Number.isSafeInteger(update.current) || update.current < 0)
  ) {
    throw new StewardError("invalid_input", "progress current must be a non-negative integer");
  }
  if (update.total !== undefined && (!Number.isSafeInteger(update.total) || update.total < 0)) {
    throw new StewardError("invalid_input", "progress total must be a non-negative integer");
  }
  if (update.current !== undefined && update.total !== undefined && update.current > update.total) {
    throw new StewardError("invalid_input", "progress current must not exceed total");
  }
  try {
    await host.call("steward.feedback.progress", update);
  } catch (error) {
    throw hostCallError("steward.feedback.progress", error);
  }
}

export function onProgress(listener: (update: ProgressUpdate) => void): () => void {
  return host.on("steward.progress", (value) => {
    listener(value as unknown as ProgressUpdate);
  });
}

export async function pickFolder(write = false): Promise<string | null> {
  return pickPath(write ? "folderWrite" : "folderRead");
}

export async function pickFile(): Promise<string | null> {
  return pickPath("fileRead");
}

async function pickPath(mode: PathPickMode): Promise<string | null> {
  try {
    const result = await host.call<{ path: string }>(
      "steward.fs.pick",
      { mode },
      {
        timeout: 300_000,
      },
    );
    return result.path;
  } catch (error) {
    const mapped = hostCallError("steward.fs.pick", error);
    if (mapped.code === "cancelled") return null;
    throw mapped;
  }
}

export async function filesystemGrants(): Promise<FilesystemGrant[]> {
  try {
    return await host.call<FilesystemGrant[]>("steward.fs.grants");
  } catch (error) {
    throw hostCallError("steward.fs.grants", error);
  }
}

export async function walk(options: WalkOptions): Promise<WalkResult> {
  if (!options.root) {
    throw new StewardError("invalid_input", "walk requires a root path");
  }
  const id = options.id ?? createWalkId();
  try {
    return await host.call<WalkResult>(
      "steward.fs.walk",
      {
        id,
        root: options.root,
        maxResults: options.maxResults,
        maxDepth: options.maxDepth,
        timeoutMs: options.timeoutMs,
      },
      { timeout: (options.timeoutMs ?? 10_000) + 2_000 },
    );
  } catch (error) {
    throw hostCallError("steward.fs.walk", error);
  }
}

export async function cancelWalk(id: string): Promise<boolean> {
  if (!id) {
    throw new StewardError("invalid_input", "cancelWalk requires a walk id");
  }
  try {
    const result = await host.call<{ cancelled: boolean }>("steward.fs.cancelWalk", { id });
    return result.cancelled;
  } catch (error) {
    throw hostCallError("steward.fs.cancelWalk", error);
  }
}

export async function readText(path: string, maxBytes = 1024 * 1024): Promise<string> {
  if (!path) {
    throw new StewardError("invalid_input", "readText requires a path");
  }
  try {
    return await host.call<string>("steward.fs.readText", { path, maxBytes });
  } catch (error) {
    throw hostCallError("steward.fs.readText", error);
  }
}

export async function writeText(path: string, contents: string): Promise<string> {
  if (!path) {
    throw new StewardError("invalid_input", "writeText requires a path");
  }
  try {
    const result = await host.call<{ path: string }>("steward.fs.writeText", { path, contents });
    return result.path;
  } catch (error) {
    throw hostCallError("steward.fs.writeText", error);
  }
}

export async function installedApplications(): Promise<InstalledApplication[]> {
  try {
    return await host.call<InstalledApplication[]>("steward.system.applications", undefined, {
      timeout: 20_000,
    });
  } catch (error) {
    throw hostCallError("steward.system.applications", error);
  }
}

export async function openPath(path: string): Promise<void> {
  if (!path) {
    throw new StewardError("invalid_input", "openPath requires a path");
  }
  try {
    await host.call("steward.system.openPath", { path });
  } catch (error) {
    throw hostCallError("steward.system.openPath", error);
  }
}

export async function revealPath(path: string): Promise<void> {
  if (!path) {
    throw new StewardError("invalid_input", "revealPath requires a path");
  }
  try {
    await host.call("steward.system.revealPath", { path });
  } catch (error) {
    throw hostCallError("steward.system.revealPath", error);
  }
}

export async function openWith(applicationId: string, path: string): Promise<void> {
  if (!applicationId || !path) {
    throw new StewardError("invalid_input", "openWith requires an applicationId and path");
  }
  try {
    await host.call("steward.system.openWith", { applicationId, path });
  } catch (error) {
    throw hostCallError("steward.system.openWith", error);
  }
}

function createWalkId(): string {
  const random = Math.floor(Math.random() * 0x100000000).toString(36);
  return `walk-${Date.now().toString(36)}-${random}`;
}

function hostCallError(capability: string, cause: unknown): StewardError {
  const message = cause instanceof Error ? cause.message : String(cause);
  const coded = /\[([a-z_]+)\]\s*(.*)/.exec(message);
  if (coded && isStewardErrorCode(coded[1])) {
    return new StewardError(
      coded[1],
      coded[2] || `Steward call '${capability}' failed`,
      isRetryable(coded[1]),
    );
  }
  if (/confirmation denied/i.test(message)) {
    return new StewardError("confirmation_denied", `Steward confirmation was denied`, false);
  }
  if (/cancel/i.test(message)) {
    return new StewardError("cancelled", `Steward call '${capability}' was cancelled`, true);
  }
  if (/invalid|required|must |unknown option|out of range/i.test(message)) {
    return new StewardError("invalid_input", message, false);
  }
  if (/permission|not granted|denied/i.test(message)) {
    return new StewardError("permission_denied", `Steward denied the '${capability}' capability`);
  }
  if (/timeout|timed out/i.test(message)) {
    return new StewardError("timeout", `Steward call '${capability}' timed out`, true);
  }
  if (/resource|exhaust|limit/i.test(message)) {
    return new StewardError(
      "resource_exhausted",
      `Steward call '${capability}' exceeded a resource limit`,
    );
  }
  return new StewardError("host_failure", `Steward call '${capability}' failed: ${message}`, true);
}

function isStewardErrorCode(value: string): value is StewardErrorCode {
  return [
    "permission_denied",
    "feature_unavailable",
    "timeout",
    "resource_exhausted",
    "host_failure",
    "invalid_input",
    "cancelled",
    "confirmation_denied",
    "io_failure",
  ].includes(value);
}

function isRetryable(code: StewardErrorCode): boolean {
  return code === "timeout" || code === "resource_exhausted" || code === "host_failure";
}
