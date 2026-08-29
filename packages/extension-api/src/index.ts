/**
 * Steward plugin extension API — M3 runtime polyfill.
 *
 * Plugin source imports these helpers; esbuild inlines them into the plugin's
 * single-file IIFE bundle. At runtime the plugin host installs the
 * `globalThis.steward` bridge (clipboard + toasts + storage + injected
 * clipboard history) before evaluating the bundle, and every API below routes
 * through that bridge. There is no Node runtime and no other globals: a plugin
 * that needs more than this cannot work in M3 without a Node polyfill (the
 * second M3 wave).
 */

export const version = "0.1.0";

/** One row rendered by the launcher's list view. */
export interface ListItem {
  /** Stable id; `item.invoke` uses it to reach the row's `onSelect`. */
  id: string;
  title: string;
  subtitle?: string;
  /** Extra keywords for future fuzzy matching inside the view. */
  keywords?: string[];
  icon?: string;
}

export interface ListOptions {
  items: ListItem[];
  onSelect?: (item: ListItem) => void;
}

/** A clipboard-history entry injected by the host into `command.invoke`. */
export interface ClipboardEntry {
  id: string;
  text: string;
  /** UNIX timestamp (seconds) when the text was copied. */
  copied_at: number;
}

/** A serializable action reference declared in a view's `actionPanel`. */
export interface ActionRef {
  id: string;
  title: string;
  icon?: string;
}

/** A view's action panel: a list of action references the host renders as a bar. */
export interface ActionPanelSpec {
  actions: ActionRef[];
}

/** One block in a `detail` view's content column. */
export interface DetailBlock {
  type: "text" | "code" | "separator";
  value?: string;
  language?: string;
}

/** One field in a `form` view. */
export interface FormField {
  id: string;
  label: string;
  type: "text" | "multiline" | "password" | "toggle" | "select";
  placeholder?: string;
  options?: { id: string; label: string }[];
  value?: string | boolean;
  required?: boolean;
}

/** One cell in a `grid` view. */
export interface GridItem {
  id: string;
  title: string;
  subtitle?: string;
  /** Optional inline SVG icon document, rendered inside the cell. */
  icon?: string;
  /** A small label shown in the cell's corner (e.g. a count or shortcut). */
  badge?: string;
}

/**
 * A serializable view returned by `command`. M3 supports:
 * - `list`: rows rendered in the launcher drop-down;
 * - `calendar`: a month grid (year/month/today, Monday or Sunday first);
 * - `detail`: a title + content blocks (with an optional action bar);
 * - `form`: a field stack with a Submit button.
 */
export type View =
  | { type: "list"; items: ListItem[]; actionPanel?: ActionPanelSpec }
  | {
      type: "calendar";
      year: number;
      month: number;
      today: string;
      startOfWeek?: 0 | 1;
      selected?: string;
      actionPanel?: ActionPanelSpec;
    }
  | {
      type: "detail";
      title: string;
      subtitle?: string;
      content: DetailBlock[];
      actionPanel?: ActionPanelSpec;
    }
  | { type: "form"; title?: string; fields: FormField[]; submitLabel?: string }
  | {
      type: "grid";
      columns: number;
      items: GridItem[];
      selectedId?: string;
      actionPanel?: ActionPanelSpec;
    }
  | {
      type: "search";
      placeholder?: string;
      actionPanel?: ActionPanelSpec;
    }
  | null;

/**
 * The module shape the host reads from `globalThis.__stewardPlugin`.
 *
 * Every handler may be `async`: the host drives the returned Promise to
 * settlement by draining the QuickJS micro-task queue until it resolves or the
 * command deadline fires. Micro-task-only async works (awaiting host functions
 * like `Clipboard`/`LocalStorage` resolves synchronously); true cross-process
 * await (sockets, disk) lands with the fs/network permissions in a later
 * milestone.
 */
export interface PluginModule {
  command(name: string, input: string): View | Promise<View>;
  /**
   * Handle a list item's confirm. May return a new view (e.g. a `detail`
   * drill-down), which the host replaces the command's view slot with.
   */
  select?(itemId: string): View | Promise<View>;
  /** Handle an `actionPanel` action invocation. */
  run?(actionId: string, itemId?: string): void | Promise<void>;
  /** Handle a `form` submit. */
  submit?(values: Record<string, string | boolean>): void | Promise<void>;
  /**
   * Stream a `search` view's results as the user types. The host invokes it
   * with the current query text and renders the returned `View` (usually a
   * `list` or `grid`) in the search view's results area.
   */
  search?(query: string): View | Promise<View>;
}

export interface ToastOptions {
  message: string;
  kind?: "info" | "success" | "error";
  durationMs?: number;
}

/**
 * Host bridge installed by the runtime (`globalThis.steward`). Plugins never
 * touch this object directly; they use the typed helpers below.
 */
interface HostBridge {
  clipboard: {
    read(): string;
    write(text: string): void;
    /** The clipboard-history snapshot the host injected for this invocation. */
    history(): ClipboardEntry[];
  };
  showToast(options: ToastOptions): void;
  /** Open a URL in the user's default browser (granted by `open.url`). */
  openUrl(url: string): void;
  /** Open a file / folder / shell target with the OS default handler (`open.path`). */
  openPath(path: string): void;
  /**
   * Read a file on the host (granted by `fs.read`, sandboxed to the plugin's
   * `fs_roots`). Returns the text for `"utf8"` (default) or a base64-decoded
   * `Uint8Array` for `"base64"`; resolves only after a cross-process round-trip.
   */
  fs: {
    readFile(path: string, encoding?: "utf8" | "base64"): Promise<string | Uint8Array>;
    writeFile(path: string, data: string | Uint8Array, encoding?: "utf8" | "base64"): Promise<void>;
  };
  net: {
    request(options: NetRequestOptions): Promise<NetResponse>;
  };
  /** Per-plugin local key-value storage (file-backed, sandboxed). */
  storage: {
    get(key: string): string | null;
    set(key: string, value: string): void;
    remove(key: string): void;
    clear(): void;
  };
}

function hostBridge(): HostBridge {
  const bridge = (globalThis as { steward?: HostBridge }).steward;
  if (!bridge) {
    throw new Error(
      "@steward/extension-api requires the Steward runtime host bridge (globalThis.steward)",
    );
  }
  return bridge;
}

/** The list registered by the latest `List` call; `selectItem` dispatches on it. */
let currentList: { items: ListItem[]; onSelect?: (item: ListItem) => void } = {
  items: [],
};

/**
 * Register the list view for the current command invocation. The plugin's
 * `command` both returns the view (for the host to render) and calls `List` to
 * attach the `onSelect` handler used by `item.invoke`.
 */
export function List(options: ListOptions): void {
  currentList = options;
}

/**
 * Dispatch an `item.invoke` to the `onSelect` handler registered by the latest
 * `List` call. Plugins export `select(itemId)` and delegate to this helper.
 */
export function selectItem(id: string): void {
  const item = currentList.items.find((candidate) => candidate.id === id);
  if (item && currentList.onSelect) {
    currentList.onSelect(item);
  }
}

/** Clipboard access, gated by the manifest's `clipboard.read/write/history` grants. */
export const Clipboard = {
  read(): string {
    return hostBridge().clipboard.read();
  },
  write(text: string): void {
    hostBridge().clipboard.write(text);
  },
  /** The history snapshot injected by the host for this invocation. */
  history(): ClipboardEntry[] {
    return hostBridge().clipboard.history();
  },
};

/** Show a transient toast in the Steward UI. */
export function showToast(options: ToastOptions): void {
  hostBridge().showToast(options);
}

/** Open `url` in the user's default browser (requires the `open.url` permission). */
export function openUrl(url: string): void {
  hostBridge().openUrl(url);
}

/** Open a file / folder / shell target with the OS default handler (requires `open.path`). */
export function openPath(path: string): void {
  hostBridge().openPath(path);
}

/**
 * Read a file on the host. Requires the `fs.read` permission and a path that
 * falls under one of the plugin's declared `fs_roots`. `encoding` may be
 * `"utf8"` (default, returns a string) or `"base64"` (returns a `Uint8Array`).
 */
export const fs = {
  readFile(path: string, encoding: "utf8" | "base64" = "utf8"): Promise<string | Uint8Array> {
    return hostBridge().fs.readFile(path, encoding);
  },
  writeFile(
    path: string,
    data: string | Uint8Array,
    encoding: "utf8" | "base64" = "utf8",
  ): Promise<void> {
    return hostBridge().fs.writeFile(path, data, encoding);
  },
};

export interface NetRequestOptions {
  method?: "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD";
  url: string;
  headers?: Record<string, string>;
  body?: string;
  timeoutMs?: number;
  maxBytes?: number;
}

export interface NetResponse {
  status: number;
  headers: Record<string, string>;
  body: string;
}

/** Make an HTTP(S) request on the host (requires the `network` permission). */
export const net = {
  request(options: NetRequestOptions): Promise<NetResponse> {
    return hostBridge().net.request(options);
  },
};

export interface Action {
  id: string;
  title: string;
  onRun: (itemId?: string) => void | Promise<void>;
}

export interface ActionPanelOptions {
  actions: Action[];
}

/** Actions registered by the latest `ActionPanel` call; `runAction` dispatches. */
let currentActions: { actions: Action[] } = { actions: [] };

/**
 * Register a view's action panel. The plugin's `command` both returns a view
 * with an `actionPanel` (for the host to render) and calls `ActionPanel` to
 * attach the `onRun` handlers invoked by `action.invoke`.
 */
export function ActionPanel(options: ActionPanelOptions): void {
  currentActions = options;
}

/** Dispatch an action by id; the plugin's `run(actionId, itemId?)` forwards here. */
export function runAction(actionId: string, itemId?: string): void {
  const action = currentActions.actions.find((candidate) => candidate.id === actionId);
  if (action) {
    action.onRun(itemId);
  }
}

export interface FormOptions {
  fields: FormField[];
  onSubmit: (values: Record<string, string | boolean>) => void | Promise<void>;
}

/** Form registered by the latest `Form` call; `submitForm` dispatches. */
let currentForm: FormOptions | null = null;

/**
 * Register a form view for the current command invocation. The plugin's
 * `command` returns `{ type: "form", fields }` and calls `Form` to attach the
 * `onSubmit` handler invoked by `form.submit`.
 */
export function Form(options: FormOptions): void {
  currentForm = options;
}

/** Dispatch a form submit; the plugin's `submit(values)` forwards here. */
export function submitForm(values: Record<string, string | boolean>): void {
  if (currentForm) {
    currentForm.onSubmit(values);
  }
}

/** Per-plugin local key-value storage (file-backed, sandboxed, no permission). */
export const LocalStorage = {
  get(key: string): string | null {
    return hostBridge().storage.get(key);
  },
  set(key: string, value: string): void {
    hostBridge().storage.set(key, value);
  },
  remove(key: string): void {
    hostBridge().storage.remove(key);
  },
  clear(): void {
    hostBridge().storage.clear();
  },
};
