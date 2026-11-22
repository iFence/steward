import { host } from "nsh";
import {
  HOST_API_VERSION,
  HOST_ERROR_ENVELOPE_VERSION,
  HOST_ERROR_CODES,
  type HostCallName,
  type HostErrorPayload,
  type HostEventName,
  type StewardErrorCode,
} from "./contract.generated";

export type {
  HostCallFixture,
  HostCallName,
  HostErrorPayload,
  HostEventName,
  StewardErrorCode,
} from "./contract.generated";
export {
  HOST_API_VERSION,
  HOST_CALL_FIXTURES,
  HOST_CALL_NAMES,
  HOST_CAPABILITIES,
  HOST_CONTRACT_VERSION,
  HOST_ERROR_CODES,
  HOST_ERROR_ENVELOPE_VERSION,
  HOST_EVENT_NAMES,
  HOST_FEATURE_NAMES,
  HOST_RETRYABLE_ERROR_CODES,
} from "./contract.generated";

export type PresentationMode = "windowed" | "embedded";

export interface StewardLaunchParams {
  pluginId: string;
  command: string;
  input: string;
  locale: string;
  accent: string;
  dark: boolean;
  presentation: PresentationMode;
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

export interface HttpFetchOptions {
  url: string;
  method?: "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE";
  headers?: Record<string, string>;
  body?: string;
  timeoutMs?: number;
  maxResponseBytes?: number;
}

export interface HttpResponse {
  status: number;
  url: string;
  headers: Array<{ name: string; value: string }>;
  body: string;
  redirects: number;
  elapsedMs: number;
}

export interface ProcessRunOptions {
  id?: string;
  executable: string;
  arguments?: string[];
  workingDirectory: string;
  timeoutMs?: number;
  maxOutputBytes?: number;
}

export interface ProcessResult {
  requestId: string;
  exitCode: number | null;
  stdout: string;
  stderr: string;
  durationMs: number;
  timedOut: boolean;
  cancelled: boolean;
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
    presentation: value.presentation === "embedded" ? "embedded" : "windowed",
    hostApiVersion: value.hostApiVersion ?? HOST_API_VERSION,
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
  return onHost("steward.command", (value) => {
    listener(value as { command: string; input: string });
  });
}

export function onTheme(listener: (theme: { accent: string; dark: boolean }) => void): () => void {
  return onHost("steward.theme", (value) => {
    listener(value as { accent: string; dark: boolean });
  });
}

export async function clipboardHistory(): Promise<ClipboardEntry[]> {
  try {
    return await callHost<ClipboardEntry[]>("steward.clipboard.history");
  } catch (error) {
    throw hostCallError("steward.clipboard.history", error);
  }
}

export async function copyText(text: string): Promise<void> {
  try {
    await callHost("steward.clipboard.write", { text });
  } catch (error) {
    throw hostCallError("steward.clipboard.write", error);
  }
}

export async function storageGet(key: string): Promise<string | null> {
  try {
    return await callHost<string | null>("steward.storage.get", { key });
  } catch (error) {
    throw hostCallError("steward.storage.get", error);
  }
}

export async function storageSet(key: string, value: string): Promise<void> {
  try {
    await callHost("steward.storage.set", { key, value });
  } catch (error) {
    throw hostCallError("steward.storage.set", error);
  }
}

export async function calendarHolidays(year: number): Promise<HolidaySchedule> {
  if (!Number.isInteger(year)) throw new Error("calendarHolidays requires an integer year");
  try {
    return await callHost<HolidaySchedule>(
      "steward.calendar.holidays",
      { year },
      { timeout: 20_000 },
    );
  } catch (error) {
    throw hostCallError("steward.calendar.holidays", error);
  }
}

export function closeWindow(): void {
  postHost("steward.window.close", null);
}

export async function backToLauncher(): Promise<void> {
  try {
    await callHost("steward.navigation.back");
  } catch (error) {
    throw hostCallError("steward.navigation.back", error);
  }
}

export async function closePage(): Promise<void> {
  try {
    await callHost("steward.navigation.close");
  } catch (error) {
    throw hostCallError("steward.navigation.close", error);
  }
}

export async function setPageTitle(title: string): Promise<void> {
  const value = title.trim();
  if (!value || value.length > 128) {
    throw new StewardError("invalid_input", "page title must contain 1 to 128 characters");
  }
  try {
    await callHost("steward.navigation.setTitle", { title: value });
  } catch (error) {
    throw hostCallError("steward.navigation.setTitle", error);
  }
}

export function onVisibility(listener: (visible: boolean) => void): () => void {
  return onHost("steward.visibility", (value) => {
    const event = value as { visible?: unknown };
    listener(event.visible === true);
  });
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
    await callHost("steward.feedback.toast", options);
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
    const result = await callHost<{ confirmed: boolean }>("steward.feedback.confirm", options, {
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
    await callHost("steward.feedback.progress", update);
  } catch (error) {
    throw hostCallError("steward.feedback.progress", error);
  }
}

export function onProgress(listener: (update: ProgressUpdate) => void): () => void {
  return onHost("steward.progress", (value) => {
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
    const result = await callHost<{ path: string }>(
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
    return await callHost<FilesystemGrant[]>("steward.fs.grants");
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
    return await callHost<WalkResult>(
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
    const result = await callHost<{ cancelled: boolean }>("steward.fs.cancelWalk", { id });
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
    return await callHost<string>("steward.fs.readText", { path, maxBytes });
  } catch (error) {
    throw hostCallError("steward.fs.readText", error);
  }
}

export async function writeText(path: string, contents: string): Promise<string> {
  if (!path) {
    throw new StewardError("invalid_input", "writeText requires a path");
  }
  try {
    const result = await callHost<{ path: string }>("steward.fs.writeText", { path, contents });
    return result.path;
  } catch (error) {
    throw hostCallError("steward.fs.writeText", error);
  }
}

export async function installedApplications(): Promise<InstalledApplication[]> {
  try {
    return await callHost<InstalledApplication[]>("steward.system.applications", undefined, {
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
    await callHost("steward.system.openPath", { path });
  } catch (error) {
    throw hostCallError("steward.system.openPath", error);
  }
}

export async function revealPath(path: string): Promise<void> {
  if (!path) {
    throw new StewardError("invalid_input", "revealPath requires a path");
  }
  try {
    await callHost("steward.system.revealPath", { path });
  } catch (error) {
    throw hostCallError("steward.system.revealPath", error);
  }
}

export async function openWith(applicationId: string, path: string): Promise<void> {
  if (!applicationId || !path) {
    throw new StewardError("invalid_input", "openWith requires an applicationId and path");
  }
  try {
    await callHost("steward.system.openWith", { applicationId, path });
  } catch (error) {
    throw hostCallError("steward.system.openWith", error);
  }
}

export async function fetchHttp(options: HttpFetchOptions): Promise<HttpResponse> {
  if (!options.url) {
    throw new StewardError("invalid_input", "fetchHttp requires a URL");
  }
  const timeoutMs = options.timeoutMs ?? 10_000;
  try {
    return await callHost<HttpResponse>(
      "steward.net.fetch",
      {
        url: options.url,
        method: options.method ?? "GET",
        headers: options.headers,
        body: options.body,
        timeoutMs,
        maxResponseBytes: options.maxResponseBytes,
      },
      { timeout: timeoutMs + 2_000 },
    );
  } catch (error) {
    throw hostCallError("steward.net.fetch", error);
  }
}

export async function runProcess(options: ProcessRunOptions): Promise<ProcessResult> {
  if (!options.executable || !options.workingDirectory) {
    throw new StewardError(
      "invalid_input",
      "runProcess requires an executable and workingDirectory",
    );
  }
  const requestId = options.id ?? createProcessId();
  if (!/^[A-Za-z0-9._:-]{1,128}$/.test(requestId)) {
    throw new StewardError("invalid_input", "process id contains unsupported characters");
  }
  // The launcher broker correlates requests with a positive u64. Keep the
  // friendlier string ID at the public API boundary and use one stable mapping
  // for both run and cancel calls.
  const numericRequestId = processIdNumber(requestId);
  const timeoutMs = options.timeoutMs ?? 30_000;
  try {
    const result = await callHost<Omit<ProcessResult, "requestId">>(
      "steward.exec.run",
      {
        requestId: numericRequestId,
        executable: options.executable,
        arguments: options.arguments ?? [],
        workingDirectory: options.workingDirectory,
        timeoutMs,
        maxOutputBytes: options.maxOutputBytes,
      },
      { timeout: timeoutMs + 2_000 },
    );
    return { ...result, requestId };
  } catch (error) {
    throw hostCallError("steward.exec.run", error);
  }
}

export async function cancelProcess(requestId: string): Promise<boolean> {
  if (!requestId) {
    throw new StewardError("invalid_input", "cancelProcess requires a request id");
  }
  const numericId = processIdNumber(requestId);
  try {
    const result = await callHost<{ cancelled: boolean }>("steward.exec.cancel", {
      requestId: numericId,
    });
    return result.cancelled;
  } catch (error) {
    throw hostCallError("steward.exec.cancel", error);
  }
}

function createWalkId(): string {
  const random = Math.floor(Math.random() * 0x100000000).toString(36);
  return `walk-${Date.now().toString(36)}-${random}`;
}

function createProcessId(): string {
  const random = Math.floor(Math.random() * 0x100000000).toString(36);
  return `process-${Date.now().toString(36)}-${random}`;
}

function processIdNumber(value: string): number {
  let high = 0x811c9dc5;
  let low = 0x9e3779b9;
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    high ^= code;
    high = Math.imul(high, 0x01000193) >>> 0;
    low ^= code + index;
    low = Math.imul(low, 0x85ebca6b) >>> 0;
  }
  return (high & 0x1fffff) * 0x100000000 + low || 1;
}

function callHost<T = unknown>(
  name: HostCallName,
  data?: unknown,
  options?: { timeout?: number },
): Promise<T> {
  return host.call<T>(name, data, options);
}

function postHost(name: HostCallName, data: unknown): void {
  host.post(name, data);
}

function onHost(event: HostEventName, listener: (value: unknown) => void): () => void {
  return host.on(event, listener);
}

function hostCallError(capability: string, cause: unknown): StewardError {
  const message = cause instanceof Error ? cause.message : String(cause);
  const payload = parseHostErrorPayload(message);
  if (payload) {
    return new StewardError(payload.code, payload.message, payload.retryable);
  }
  return new StewardError("host_failure", `Steward call '${capability}' failed: ${message}`, true);
}

function parseHostErrorPayload(message: string): HostErrorPayload | null {
  let value: unknown;
  try {
    value = JSON.parse(message);
  } catch {
    return null;
  }
  if (!value || typeof value !== "object") return null;
  const candidate = value as Partial<HostErrorPayload>;
  if (
    candidate.version !== HOST_ERROR_ENVELOPE_VERSION ||
    typeof candidate.code !== "string" ||
    !isStewardErrorCode(candidate.code) ||
    typeof candidate.message !== "string" ||
    typeof candidate.retryable !== "boolean"
  ) {
    return null;
  }
  return {
    version: candidate.version,
    code: candidate.code,
    message: candidate.message,
    retryable: candidate.retryable,
  };
}

function isStewardErrorCode(value: string): value is StewardErrorCode {
  return (HOST_ERROR_CODES as readonly string[]).includes(value);
}
