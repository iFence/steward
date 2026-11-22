import { host } from "nsh";
import {
  HOST_API_VERSION,
  HOST_ERROR_ENVELOPE_VERSION,
  HOST_ERROR_CODES,
  HOST_RETRYABLE_ERROR_CODES,
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
  HostApiFamily,
  HostFailureState,
  StewardErrorCode,
} from "./contract.generated";
export {
  HOST_API_VERSION,
  HOST_API_FAMILIES,
  HOST_CALL_FIXTURES,
  HOST_CALL_NAMES,
  HOST_CAPABILITIES,
  HOST_CONTRACT_VERSION,
  HOST_ERROR_CODES,
  HOST_ERROR_ENVELOPE_VERSION,
  HOST_EVENT_NAMES,
  HOST_FAILURE_STATES,
  HOST_FEATURE_NAMES,
  HOST_MINIMUM_SUPPORTED_API_VERSION,
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

  constructor(
    code: StewardErrorCode,
    message: string,
    retryable = (HOST_RETRYABLE_ERROR_CODES as readonly StewardErrorCode[]).includes(code),
  ) {
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
  if (utf8ByteLength(text) > 100_000) {
    throw new StewardError("resource_exhausted", "clipboard text exceeds the 100000 byte limit");
  }
  try {
    await callHost("steward.clipboard.write", { text });
  } catch (error) {
    throw hostCallError("steward.clipboard.write", error);
  }
}

export async function storageGet(key: string): Promise<string | null> {
  if (!key || utf8ByteLength(key) > 256) {
    throw new StewardError("invalid_input", "storage key must contain 1 to 256 UTF-8 bytes");
  }
  try {
    return await callHost<string | null>("steward.storage.get", { key });
  } catch (error) {
    throw hostCallError("steward.storage.get", error);
  }
}

export async function storageSet(key: string, value: string): Promise<void> {
  if (!key || utf8ByteLength(key) > 256) {
    throw new StewardError("invalid_input", "storage key must contain 1 to 256 UTF-8 bytes");
  }
  if (utf8ByteLength(value) > 1024 * 1024) {
    throw new StewardError("resource_exhausted", "storage value exceeds the 1 MiB limit");
  }
  try {
    await callHost("steward.storage.set", { key, value });
  } catch (error) {
    throw hostCallError("steward.storage.set", error);
  }
}

export async function getSecret(key: string): Promise<string | null> {
  validateSecretKey(key);
  try {
    return await callHost<string | null>("steward.secrets.get", { key });
  } catch (error) {
    throw hostCallError("steward.secrets.get", error);
  }
}

export async function setSecret(key: string, value: string): Promise<void> {
  validateSecretKey(key);
  if (utf8ByteLength(value) > 2560) {
    throw new StewardError("resource_exhausted", "secret value exceeds the 2560 byte limit");
  }
  try {
    await callHost("steward.secrets.set", { key, value });
  } catch (error) {
    throw hostCallError("steward.secrets.set", error);
  }
}

export async function deleteSecret(key: string): Promise<boolean> {
  validateSecretKey(key);
  try {
    const result = await callHost<{ deleted: boolean }>("steward.secrets.delete", { key });
    return result.deleted;
  } catch (error) {
    throw hostCallError("steward.secrets.delete", error);
  }
}

export async function listSecrets(): Promise<string[]> {
  try {
    return await callHost<string[]>("steward.secrets.list");
  } catch (error) {
    throw hostCallError("steward.secrets.list", error);
  }
}

export async function calendarHolidays(year: number): Promise<HolidaySchedule> {
  if (!Number.isInteger(year) || year < 0 || year > 65_535) {
    throw new StewardError("invalid_input", "calendarHolidays year must be between 0 and 65535");
  }
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
  if (!value || utf8ByteLength(value) > 128) {
    throw new StewardError("invalid_input", "page title must contain 1 to 128 UTF-8 bytes");
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
  if (utf8ByteLength(options.title) > 128 || utf8ByteLength(options.message) > 1024) {
    throw new StewardError("invalid_input", "toast content exceeds its documented limits");
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
  if (utf8ByteLength(options.title) > 128 || utf8ByteLength(options.message) > 4096) {
    throw new StewardError("invalid_input", "confirmation content exceeds its documented limits");
  }
  if (
    (options.confirmLabel !== undefined && utf8ByteLength(options.confirmLabel) > 64) ||
    (options.cancelLabel !== undefined && utf8ByteLength(options.cancelLabel) > 64)
  ) {
    throw new StewardError("invalid_input", "confirmation labels must not exceed 64 UTF-8 bytes");
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
  if (!update.id || utf8ByteLength(update.id) > 128 || !update.message) {
    throw new StewardError("invalid_input", "progress requires an id and message");
  }
  if (utf8ByteLength(update.message) > 1024) {
    throw new StewardError("invalid_input", "progress message must not exceed 1024 UTF-8 bytes");
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
  if (utf8ByteLength(options.root) > 32_768) {
    throw new StewardError("invalid_input", "walk root exceeds the 32768 byte path limit");
  }
  if (
    options.maxResults !== undefined &&
    (!Number.isSafeInteger(options.maxResults) ||
      options.maxResults < 1 ||
      options.maxResults > 20_000)
  ) {
    throw new StewardError("invalid_input", "walk maxResults must be between 1 and 20000");
  }
  if (
    options.maxDepth !== undefined &&
    (!Number.isSafeInteger(options.maxDepth) || options.maxDepth < 1 || options.maxDepth > 64)
  ) {
    throw new StewardError("invalid_input", "walk maxDepth must be between 1 and 64");
  }
  if (
    options.timeoutMs !== undefined &&
    (!Number.isSafeInteger(options.timeoutMs) ||
      options.timeoutMs < 1 ||
      options.timeoutMs > 30_000)
  ) {
    throw new StewardError("invalid_input", "walk timeoutMs must be between 1 and 30000");
  }
  const id = options.id ?? createWalkId();
  if (!validWalkId(id)) {
    throw new StewardError(
      "invalid_input",
      "walk id must contain 1 to 128 letters, digits, dots, underscores, or hyphens",
    );
  }
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
  if (!validWalkId(id)) {
    throw new StewardError(
      "invalid_input",
      "cancelWalk id must contain 1 to 128 letters, digits, dots, underscores, or hyphens",
    );
  }
  try {
    const result = await callHost<{ cancelled: boolean }>("steward.fs.cancelWalk", { id });
    return result.cancelled;
  } catch (error) {
    throw hostCallError("steward.fs.cancelWalk", error);
  }
}

export async function readText(path: string, maxBytes = 1024 * 1024): Promise<string> {
  if (!path || utf8ByteLength(path) > 32_768) {
    throw new StewardError("invalid_input", "readText requires a bounded path");
  }
  if (!Number.isSafeInteger(maxBytes) || maxBytes < 1 || maxBytes > 4 * 1024 * 1024) {
    throw new StewardError("invalid_input", "readText maxBytes must be between 1 and 4 MiB");
  }
  try {
    return await callHost<string>("steward.fs.readText", { path, maxBytes });
  } catch (error) {
    throw hostCallError("steward.fs.readText", error);
  }
}

export async function writeText(path: string, contents: string): Promise<string> {
  if (!path || utf8ByteLength(path) > 32_768) {
    throw new StewardError("invalid_input", "writeText requires a bounded path");
  }
  if (utf8ByteLength(contents) > 4 * 1024 * 1024) {
    throw new StewardError("resource_exhausted", "writeText contents exceed the 4 MiB limit");
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
  if (!path || utf8ByteLength(path) > 32_768) {
    throw new StewardError("invalid_input", "openPath requires a bounded path");
  }
  try {
    await callHost("steward.system.openPath", { path });
  } catch (error) {
    throw hostCallError("steward.system.openPath", error);
  }
}

export async function revealPath(path: string): Promise<void> {
  if (!path || utf8ByteLength(path) > 32_768) {
    throw new StewardError("invalid_input", "revealPath requires a bounded path");
  }
  try {
    await callHost("steward.system.revealPath", { path });
  } catch (error) {
    throw hostCallError("steward.system.revealPath", error);
  }
}

export async function openWith(applicationId: string, path: string): Promise<void> {
  if (
    !applicationId ||
    utf8ByteLength(applicationId) > 32_768 ||
    !path ||
    utf8ByteLength(path) > 32_768
  ) {
    throw new StewardError("invalid_input", "openWith requires bounded applicationId and path");
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
  if (utf8ByteLength(options.url) > 8_192) {
    throw new StewardError("invalid_input", "fetchHttp URL exceeds the 8192 byte limit");
  }
  if (Object.keys(options.headers ?? {}).length > 32) {
    throw new StewardError("invalid_input", "fetchHttp accepts at most 32 headers");
  }
  if (options.body !== undefined && utf8ByteLength(options.body) > 1024 * 1024) {
    throw new StewardError("resource_exhausted", "fetchHttp body exceeds the 1 MiB limit");
  }
  const timeoutMs = options.timeoutMs ?? 10_000;
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 100 || timeoutMs > 30_000) {
    throw new StewardError("invalid_input", "fetchHttp timeoutMs must be between 100 and 30000");
  }
  if (
    options.maxResponseBytes !== undefined &&
    (!Number.isSafeInteger(options.maxResponseBytes) ||
      options.maxResponseBytes < 1 ||
      options.maxResponseBytes > 4 * 1024 * 1024)
  ) {
    throw new StewardError(
      "invalid_input",
      "fetchHttp maxResponseBytes must be between 1 and 4 MiB",
    );
  }
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
  if (utf8ByteLength(options.executable) > 260) {
    throw new StewardError("invalid_input", "runProcess executable exceeds the 260 byte limit");
  }
  if (utf8ByteLength(options.workingDirectory) > 32_768) {
    throw new StewardError(
      "invalid_input",
      "runProcess workingDirectory exceeds the 32768 byte limit",
    );
  }
  const requestId = options.id ?? createProcessId();
  if (!validProcessId(requestId)) {
    throw new StewardError("invalid_input", "process id contains unsupported characters");
  }
  // The launcher broker correlates requests with a positive u64. Keep the
  // friendlier string ID at the public API boundary and use one stable mapping
  // for both run and cancel calls.
  const numericRequestId = processIdNumber(requestId);
  const timeoutMs = options.timeoutMs ?? 30_000;
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 120_000) {
    throw new StewardError("invalid_input", "runProcess timeoutMs must be between 1 and 120000");
  }
  if (
    options.maxOutputBytes !== undefined &&
    (!Number.isSafeInteger(options.maxOutputBytes) ||
      options.maxOutputBytes < 1 ||
      options.maxOutputBytes > 4 * 1024 * 1024)
  ) {
    throw new StewardError(
      "invalid_input",
      "runProcess maxOutputBytes must be between 1 and 4 MiB",
    );
  }
  const argumentsValue = options.arguments ?? [];
  if (argumentsValue.length > 256) {
    throw new StewardError("resource_exhausted", "runProcess accepts at most 256 arguments");
  }
  if (argumentsValue.reduce((total, item) => total + utf8ByteLength(item), 0) > 32 * 1024) {
    throw new StewardError("resource_exhausted", "runProcess arguments exceed the 32 KiB limit");
  }
  if (argumentsValue.some((item) => item.includes("\0"))) {
    throw new StewardError("invalid_input", "runProcess arguments cannot contain NUL");
  }
  try {
    const result = await callHost<Omit<ProcessResult, "requestId">>(
      "steward.exec.run",
      {
        requestId: numericRequestId,
        executable: options.executable,
        arguments: argumentsValue,
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
  if (!validProcessId(requestId)) {
    throw new StewardError("invalid_input", "process id contains unsupported characters");
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

function validWalkId(value: string): boolean {
  return /^[A-Za-z0-9._-]{1,128}$/.test(value);
}

function validProcessId(value: string): boolean {
  return /^[A-Za-z0-9._:-]{1,128}$/.test(value);
}

function validateSecretKey(key: string): void {
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(key)) {
    throw new StewardError(
      "invalid_input",
      "secret key must match [A-Za-z0-9][A-Za-z0-9._-]{0,127}",
    );
  }
}

function utf8ByteLength(value: string): number {
  let length = 0;
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x80) {
      length += 1;
    } else if (code < 0x800) {
      length += 2;
    } else if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        length += 4;
        index += 1;
      } else {
        length += 3;
      }
    } else {
      length += 3;
    }
  }
  return length;
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
