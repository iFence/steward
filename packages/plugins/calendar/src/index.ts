/**
 * Official example plugin: Calendar.
 *
 * M2 end-to-end link (TS -> esbuild -> QuickJS -> launcher UI). Typing
 * `calendar` opens a real month calendar view; optional arguments select the
 * month: `calendar +3` / `calendar -1` (month offset) or `calendar 2026-09`
 * (absolute). Selecting a day copies its ISO date and shows a toast. The
 * `clipboard.write` permission is declared in `plugin.json`; without it the
 * host bridge rejects the call with JSON-RPC -32000.
 */

import { Clipboard, showToast, type View } from "@steward/extension-api";

export const id = "com.example.calendar";
export const name = "Calendar";
export const version = "0.1.0";

function isoDate(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

/**
 * Resolve the requested month from the query input: `YYYY` / `YYYY-MM` is
 * absolute, `+N` / `-N` / a bare number is an offset from the current month,
 * anything else defaults to the current month.
 */
function resolveMonth(input: string): { year: number; month: number; today: string } {
  const now = new Date();
  let year = now.getFullYear();
  let month = now.getMonth() + 1;

  const absolute = input.match(/^(\d{4})(?:-(\d{1,2}))?$/);
  if (absolute) {
    year = Number(absolute[1]);
    if (absolute[2]) {
      month = Number(absolute[2]);
    }
  } else {
    const offset = input.match(/^[+-]?\d+$/);
    if (offset) {
      const delta = Number(offset[0]);
      month += delta;
      year += Math.floor((month - 1) / 12);
      month = ((((month - 1) % 12) + 12) % 12) + 1;
    }
  }
  return { year, month: Math.max(1, Math.min(12, month)), today: isoDate(now) };
}

export function command(name: string, input: string): View {
  const { year, month, today } = resolveMonth(input.trim());
  return {
    type: "calendar",
    year,
    month,
    today,
    startOfWeek: 1,
    selected: today,
  };
}

export function select(itemId: string): void {
  Clipboard.write(itemId);
  showToast({ message: `Copied ${itemId}`, kind: "success" });
}
