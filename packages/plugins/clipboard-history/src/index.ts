/**
 * Official example plugin: Clipboard History.
 *
 * M3 milestone validation plugin: exercises `List` + `Detail` + `ActionPanel` +
 * `LocalStorage` + `Clipboard.history()` / `Clipboard.write`. Typing
 * `clipboard-history` shows the recent clipboard entries (pinned ones first);
 * selecting an entry drills into a `Detail` view of the full text; the action
 * bar copies the entry back to the clipboard or pins/unpins it.
 */

import {
  ActionPanel,
  Clipboard,
  List,
  LocalStorage,
  runAction,
  showToast,
  type Action,
  type ClipboardEntry,
  type ListItem,
  type View,
} from "@steward/extension-api";

export const id = "com.example.clipboard-history";
export const name = "Clipboard History";
export const version = "0.1.0";

const PIN_KEY = "pinned";
const MAX_TEXT_PREVIEW = 60;

let currentEntries: ClipboardEntry[] = [];
let currentSelectedId: string | null = null;

function readPins(): Set<string> {
  try {
    const raw = LocalStorage.get(PIN_KEY) ?? "[]";
    return new Set(JSON.parse(raw) as string[]);
  } catch {
    return new Set();
  }
}

function writePins(pins: Set<string>): void {
  LocalStorage.set(PIN_KEY, JSON.stringify([...pins]));
}

function formatTime(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleString();
}

/** Pinned entries first, then newest-first. */
function orderedEntries(): ClipboardEntry[] {
  const pins = readPins();
  return [...currentEntries].sort((a, b) => {
    const aPinned = pins.has(a.id) ? 1 : 0;
    const bPinned = pins.has(b.id) ? 1 : 0;
    if (aPinned !== bPinned) return bPinned - aPinned;
    return b.copied_at - a.copied_at;
  });
}

/** The serializable action refs rendered by the host action bar. */
function actionRefs(): { id: string; title: string }[] {
  return [
    { id: "copy", title: "Copy" },
    { id: "pin", title: "Pin" },
  ];
}

const copyAction: Action = {
  id: "copy",
  title: "Copy",
  onRun: () => {
    const entry = currentEntries.find((candidate) => candidate.id === currentSelectedId);
    if (entry) {
      Clipboard.write(entry.text);
      showToast({ message: "Copied to clipboard", kind: "success" });
    }
  },
};

const pinAction: Action = {
  id: "pin",
  title: "Pin",
  onRun: () => {
    if (!currentSelectedId) {
      return;
    }
    const pins = readPins();
    if (pins.has(currentSelectedId)) {
      pins.delete(currentSelectedId);
    } else {
      pins.add(currentSelectedId);
    }
    writePins(pins);
    showToast({
      message: pins.has(currentSelectedId) ? "Pinned" : "Unpinned",
      kind: "info",
    });
  },
};

export function command(_name: string, _input: string): View {
  currentEntries = Clipboard.history();
  currentSelectedId = null;
  const entries = orderedEntries();
  const items: ListItem[] = entries.map((entry) => ({
    id: entry.id,
    title: entry.text.slice(0, MAX_TEXT_PREVIEW),
    subtitle: formatTime(entry.copied_at),
    keywords: [entry.text],
  }));
  List({
    items,
    onSelect: (item) => {
      currentSelectedId = item.id;
    },
  });
  ActionPanel({ actions: [copyAction, pinAction] });
  return {
    type: "list",
    items,
    actionPanel: { actions: actionRefs() },
  };
}

export function select(itemId: string): View {
  currentSelectedId = itemId;
  const entry = currentEntries.find((candidate) => candidate.id === itemId);
  if (!entry) {
    return null;
  }
  return {
    type: "detail",
    title: entry.text.slice(0, MAX_TEXT_PREVIEW),
    subtitle: formatTime(entry.copied_at),
    content: [{ type: "text", value: entry.text }],
    actionPanel: { actions: actionRefs() },
  };
}

export function run(actionId: string, itemId?: string): void {
  if (itemId) {
    currentSelectedId = itemId;
  }
  runAction(actionId, itemId);
}
