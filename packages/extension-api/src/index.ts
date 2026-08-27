/**
 * Steward plugin extension API — M2 runtime polyfill.
 *
 * Plugin source imports these helpers; esbuild inlines them into the plugin's
 * single-file IIFE bundle. At runtime the plugin host installs the
 * `globalThis.steward` bridge (clipboard + toasts) before evaluating the
 * bundle, and every API below routes through that bridge. There is no Node
 * runtime and no other globals: a plugin that needs more than this cannot
 * work in M2.
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

/**
 * A serializable view returned by `command`. M2 supports:
 * - `list`: rows rendered in the launcher drop-down;
 * - `calendar`: a month grid (year/month/today, Monday or Sunday first).
 */
export type View =
  | { type: "list"; items: ListItem[] }
  | {
      type: "calendar";
      year: number;
      month: number;
      today: string;
      startOfWeek?: 0 | 1;
      selected?: string;
    }
  | null;

/** The module shape the host reads from `globalThis.__stewardPlugin`. */
export interface PluginModule {
  command(name: string, input: string): View;
  select?(itemId: string): void;
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
  };
  showToast(options: ToastOptions): void;
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

/** Clipboard access, gated by the manifest's `clipboard.read/write` grants. */
export const Clipboard = {
  read(): string {
    return hostBridge().clipboard.read();
  },
  write(text: string): void {
    hostBridge().clipboard.write(text);
  },
};

/** Show a transient toast in the Steward UI. */
export function showToast(options: ToastOptions): void {
  hostBridge().showToast(options);
}

export interface Action {
  id: string;
  title: string;
  onRun: () => void | Promise<void>;
}

export interface ActionPanelOptions {
  actions: Action[];
}

/**
 * Action panels are a M3 API. Calling this in M2 fails loudly instead of
 * silently rendering nothing.
 */
export function ActionPanel(options: ActionPanelOptions): never {
  void options;
  throw new Error("@steward/extension-api: ActionPanel is not supported in M2");
}
