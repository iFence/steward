/**
 * Official example plugin: Calendar.
 *
 * M2 end-to-end link (TS -> esbuild -> QuickJS -> launcher UI). Typing
 * `calendar` (optionally with an offset like `calendar +3`) lists the next
 * seven days; selecting a row copies its ISO date and shows a toast. The
 * `clipboard.write` permission is declared in `plugin.json`; without it the
 * host bridge rejects the call with JSON-RPC -32000.
 */

import {
  Clipboard,
  List,
  selectItem,
  showToast,
  type ListItem,
  type View,
} from "@steward/extension-api";

export const id = "com.example.calendar";
export const name = "Calendar";
export const version = "0.1.0";

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] as const;

function isoDate(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function relativeLabel(date: Date, today: Date): string {
  const diff = Math.round((date.getTime() - today.getTime()) / (24 * 60 * 60 * 1000));
  if (diff === 0) {
    return "Today";
  }
  if (diff === 1) {
    return "Tomorrow";
  }
  if (diff === -1) {
    return "Yesterday";
  }
  return `${WEEKDAYS[date.getDay()]}`;
}

function parseOffset(input: string): number {
  const match = input.match(/[+-]?\d+/);
  return match ? Number(match[0]) : 0;
}

function nextDays(offset: number): ListItem[] {
  const today = new Date();
  const items: ListItem[] = [];
  for (let index = 0; index < 7; index += 1) {
    const date = new Date(today.getFullYear(), today.getMonth(), today.getDate() + offset + index);
    items.push({
      id: isoDate(date),
      title: isoDate(date),
      subtitle: relativeLabel(date, today),
      keywords: [`${WEEKDAYS[date.getDay()]}`, isoDate(date)],
    });
  }
  return items;
}

export function command(name: string, input: string): View {
  const offset = parseOffset(input.trim());
  const items = nextDays(offset);
  List({
    items,
    onSelect: (item) => {
      Clipboard.write(item.id);
      showToast({ message: `Copied ${item.id}`, kind: "success" });
    },
  });
  return { type: "list", items };
}

export function select(itemId: string): void {
  selectItem(itemId);
}
