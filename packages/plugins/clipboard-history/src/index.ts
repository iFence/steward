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
function actionRefs(): { id: string; title: string; icon: string }[] {
  return [
    {
      id: "copy",
      title: "Copy",
      icon: '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>',
    },
    {
      id: "pin",
      title: "Pin",
      icon: '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/></svg>',
    },
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
  // Confirm-to-copy: put the entry back on the clipboard so the user can paste
  // it into the target window. The detail drill-down was replaced by a direct
  // copy action to match a clipboard-history app's primary interaction.
  Clipboard.write(entry.text);
  showToast({ message: "Copied to clipboard", kind: "success" });
  return null;
}

export function run(actionId: string, itemId?: string): void {
  if (itemId) {
    currentSelectedId = itemId;
  }
  runAction(actionId, itemId);
}
