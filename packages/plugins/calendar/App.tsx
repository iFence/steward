import { Clipboard } from "nsh/clipboard";
import { mount, Scroll, signal, Text, View } from "nsh";
import { launchParams, onCommand, onTheme } from "@steward/extension-api";
import {
  festivalFor,
  lunarCellLabel,
  lunarDateLabel,
  lunarZodiacYear,
  solarToLunar,
  type SolarDate,
} from "./lunar";

const MIN_YEAR = 1900;
const MAX_YEAR = 2100;
const GRID_CELLS = 42;
const WEEKDAYS_ZH = ["一", "二", "三", "四", "五", "六", "日"];
const WEEKDAYS_EN = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const LONG_WEEKDAYS_ZH = ["日", "一", "二", "三", "四", "五", "六"];

interface CalendarCopy {
  subtitle: string;
  today: string;
  previous: string;
  next: string;
  selected: string;
  copyHint: string;
  copied: string;
  unavailable: string;
  festival: string;
  weekdays: string[];
}

interface Palette {
  background: string;
  card: string;
  border: string;
  foreground: string;
  muted: string;
  selection: string;
  accent: string;
}

function isChinese(locale: string): boolean {
  return locale.toLowerCase().startsWith("zh");
}

function copyFor(locale: string): CalendarCopy {
  if (isChinese(locale)) {
    return {
      subtitle: "公历与农历",
      today: "今天",
      previous: "上个月",
      next: "下个月",
      selected: "选中日期",
      copyHint: "点击日期可复制公历",
      copied: "已复制",
      unavailable: "农历数据不可用",
      festival: "节日",
      weekdays: WEEKDAYS_ZH,
    };
  }
  return {
    subtitle: "Gregorian and Chinese lunar calendar",
    today: "Today",
    previous: "Previous month",
    next: "Next month",
    selected: "Selected date",
    copyHint: "Select a date to copy its ISO value",
    copied: "Copied",
    unavailable: "Lunar data unavailable",
    festival: "Festival",
    weekdays: WEEKDAYS_EN,
  };
}

function paletteFor(dark: boolean, accent: string): Palette {
  return dark
    ? {
        background: "#19191d",
        card: "#242429",
        border: "#38383f",
        foreground: "#f5f5f7",
        muted: "#94949c",
        selection: "#302e3b",
        accent,
      }
    : {
        background: "#f5f5f7",
        card: "#ffffff",
        border: "#dedee3",
        foreground: "#202024",
        muted: "#73737b",
        selection: "#f0ecff",
        accent,
      };
}

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

function isoDate(date: SolarDate): string {
  return `${date.year}-${pad(date.month)}-${pad(date.day)}`;
}

function parseIsoDate(value: string): SolarDate | null {
  const match = value.match(/^(\d{4})-(\d{2})-(\d{2})$/);
  if (!match) return null;
  return { year: Number(match[1]), month: Number(match[2]), day: Number(match[3]) };
}

function today(): SolarDate {
  const now = new Date();
  return { year: now.getFullYear(), month: now.getMonth() + 1, day: now.getDate() };
}

function resolveMonth(input: string): { year: number; month: number } {
  const now = today();
  let year = now.year;
  let month = now.month;
  const absolute = input.match(/^(\d{4})(?:-(\d{1,2}))?$/);
  const offset = input.match(/^[+-]?\d+$/);

  if (absolute) {
    year = Number(absolute[1]);
    month = absolute[2] ? Number(absolute[2]) : month;
  } else if (offset) {
    month += Number(offset[0]);
    year += Math.floor((month - 1) / 12);
    month = ((((month - 1) % 12) + 12) % 12) + 1;
  }

  year = Math.max(MIN_YEAR, Math.min(MAX_YEAR, year));
  month = Math.max(1, Math.min(12, month));
  return { year, month };
}

function daysInMonth(year: number, month: number): number {
  return new Date(Date.UTC(year, month, 0)).getUTCDate();
}

function firstWeekdayMonday(year: number, month: number): number {
  return (new Date(Date.UTC(year, month - 1, 1)).getUTCDay() + 6) % 7;
}

function monthCells(year: number, month: number): Array<number | null> {
  const cells: Array<number | null> = Array(firstWeekdayMonday(year, month)).fill(null);
  for (let day = 1; day <= daysInMonth(year, month); day += 1) cells.push(day);
  while (cells.length < GRID_CELLS) cells.push(null);
  return cells;
}

function monthTitle(year: number, month: number, locale: string): string {
  if (isChinese(locale)) return `${year}年${month}月`;
  return new Date(year, month - 1, 1).toLocaleString(locale, {
    month: "long",
    year: "numeric",
  });
}

function formatSolarDate(date: SolarDate, locale: string): string {
  if (isChinese(locale)) {
    const weekday =
      LONG_WEEKDAYS_ZH[new Date(Date.UTC(date.year, date.month - 1, date.day)).getUTCDay()];
    return `${date.year}年${date.month}月${date.day}日 星期${weekday}`;
  }
  return new Date(date.year, date.month - 1, date.day).toLocaleDateString(locale, {
    day: "numeric",
    month: "long",
    weekday: "long",
    year: "numeric",
  });
}

function initialSelection(month: { year: number; month: number }, current: SolarDate): string {
  const selected =
    current.year === month.year && current.month === month.month
      ? current
      : { year: month.year, month: month.month, day: 1 };
  return isoDate(selected);
}

function App() {
  const params = launchParams();
  const initial = resolveMonth(params.input.replace(/^calendar\s*/i, "").trim());
  const year = signal(initial.year);
  const month = signal(initial.month);
  const selected = signal(initialSelection(initial, today()));
  const status = signal("");
  const accent = signal(params.accent);
  const dark = signal(params.dark);

  onCommand(({ input }) => {
    const next = resolveMonth(input.replace(/^calendar\s*/i, "").trim());
    year.set(next.year);
    month.set(next.month);
    selected.set(initialSelection(next, today()));
    status.set("");
  });

  onTheme((theme) => {
    accent.set(theme.accent);
    dark.set(theme.dark);
  });

  const copy = copyFor(params.locale);
  const currentPalette = () => paletteFor(dark(), accent());
  const colors: Palette = {
    get accent() {
      return currentPalette().accent;
    },
    get background() {
      return currentPalette().background;
    },
    get border() {
      return currentPalette().border;
    },
    get card() {
      return currentPalette().card;
    },
    get foreground() {
      return currentPalette().foreground;
    },
    get muted() {
      return currentPalette().muted;
    },
    get selection() {
      return currentPalette().selection;
    },
  };

  const selectMonth = (nextYear: number, nextMonth: number) => {
    const monthIndex = Math.max(
      MIN_YEAR * 12,
      Math.min(MAX_YEAR * 12 + 11, nextYear * 12 + nextMonth - 1),
    );
    const boundedYear = Math.floor(monthIndex / 12);
    const boundedMonth = (monthIndex % 12) + 1;
    year.set(boundedYear);
    month.set(boundedMonth);
    selected.set(isoDate({ year: boundedYear, month: boundedMonth, day: 1 }));
    status.set("");
  };

  const moveMonth = (delta: number) => {
    selectMonth(year(), month() + delta);
  };

  const goToday = () => {
    const current = today();
    year.set(current.year);
    month.set(current.month);
    selected.set(isoDate(current));
    status.set("");
  };

  const selectDay = (day: number) => {
    const value = isoDate({ year: year(), month: month(), day });
    selected.set(value);
    status.set("");
    void Clipboard.writeText(value)
      .then(() => status.set(copy.copied))
      .catch(() => status.set(""));
  };

  const selectedSolar = () => parseIsoDate(selected());
  const selectedLunar = () => {
    const solar = selectedSolar();
    return solar ? solarToLunar(solar.year, solar.month, solar.day) : null;
  };
  const selectedFestival = () => {
    const solar = selectedSolar();
    const lunar = selectedLunar();
    return solar && lunar ? festivalFor(lunar, solar) : null;
  };
  const current = today();

  return (
    <Scroll style={{ flex: 1, backgroundColor: colors.background }}>
      <View style={{ padding: 24, gap: 16 }}>
        <View style={{ flexDirection: "row", alignItems: "center", gap: 12 }}>
          <View style={{ flex: 1 }}>
            <Text
              style={{ color: colors.foreground, fontSize: 28, fontWeight: "bold" }}
              testID="calendar-month-title"
            >
              {monthTitle(year(), month(), params.locale)}
            </Text>
            <Text style={{ color: colors.muted, fontSize: 13, marginTop: 3 }}>{copy.subtitle}</Text>
          </View>
          <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
            <View
              accessibilityLabel={copy.previous}
              accessible
              disabled={year() === MIN_YEAR && month() === 1}
              onPress={() => moveMonth(-1)}
              role="button"
              style={{
                alignItems: "center",
                backgroundColor: colors.card,
                borderColor: colors.border,
                borderRadius: 9,
                borderWidth: 1,
                height: 36,
                justifyContent: "center",
                width: 36,
              }}
              testID="calendar-prev-month"
            >
              <Text style={{ color: colors.foreground, fontSize: 22 }}>‹</Text>
            </View>
            <View
              accessibilityLabel={copy.today}
              accessible
              onPress={goToday}
              role="button"
              style={{
                alignItems: "center",
                backgroundColor: colors.card,
                borderColor: colors.border,
                borderRadius: 9,
                borderWidth: 1,
                height: 36,
                justifyContent: "center",
                paddingHorizontal: 14,
              }}
              testID="calendar-today"
            >
              <Text style={{ color: colors.foreground, fontSize: 13 }}>{copy.today}</Text>
            </View>
            <View
              accessibilityLabel={copy.next}
              accessible
              disabled={year() === MAX_YEAR && month() === 12}
              onPress={() => moveMonth(1)}
              role="button"
              style={{
                alignItems: "center",
                backgroundColor: colors.card,
                borderColor: colors.border,
                borderRadius: 9,
                borderWidth: 1,
                height: 36,
                justifyContent: "center",
                width: 36,
              }}
              testID="calendar-next-month"
            >
              <Text style={{ color: colors.foreground, fontSize: 22 }}>›</Text>
            </View>
          </View>
        </View>

        <View
          style={{
            backgroundColor: colors.card,
            borderColor: colors.border,
            borderRadius: 16,
            borderWidth: 1,
            padding: 8,
          }}
        >
          <View style={{ flexDirection: "row", paddingBottom: 6, paddingTop: 2 }}>
            <Each of={copy.weekdays} key="index">
              {(weekday) => (
                <View style={{ alignItems: "center", padding: 5, width: "14.2857%" }}>
                  <Text style={{ color: colors.muted, fontSize: 12, fontWeight: "bold" }}>
                    {weekday()}
                  </Text>
                </View>
              )}
            </Each>
          </View>
          <View style={{ flexDirection: "row", flexWrap: "wrap" }}>
            <Each of={monthCells(year(), month())} key="index">
              {(day) => (
                <View style={{ padding: 2, width: "14.2857%" }}>
                  {day() === null ? (
                    <View style={{ height: 68 }} />
                  ) : (
                    (() => {
                      const value = { year: year(), month: month(), day: day()! };
                      const valueIso = isoDate(value);
                      const lunar = solarToLunar(value.year, value.month, value.day);
                      const label = lunar ? lunarCellLabel(lunar, value) : copy.unavailable;
                      const festival = lunar ? festivalFor(lunar, value) : null;
                      const isSelected = valueIso === selected();
                      const isToday =
                        value.year === current.year &&
                        value.month === current.month &&
                        value.day === current.day;
                      return (
                        <View
                          accessibilityLabel={`${valueIso}，${label}`}
                          accessible
                          onPress={() => selectDay(value.day)}
                          role="button"
                          style={{
                            alignItems: "center",
                            backgroundColor: isSelected ? colors.selection : "transparent",
                            borderColor: isSelected || isToday ? colors.accent : "transparent",
                            borderRadius: 10,
                            borderWidth: isSelected ? 2 : 1,
                            height: 68,
                            justifyContent: "center",
                            padding: 3,
                          }}
                          testID={`calendar-day-${valueIso}`}
                        >
                          <View
                            style={{
                              alignItems: "center",
                              backgroundColor: isToday ? colors.accent : "transparent",
                              borderRadius: 15,
                              height: 30,
                              justifyContent: "center",
                              width: 30,
                            }}
                          >
                            <Text
                              style={{
                                color: isToday ? "#ffffff" : colors.foreground,
                                fontSize: 16,
                                fontWeight: isToday || isSelected ? "bold" : "normal",
                              }}
                            >
                              {day()}
                            </Text>
                          </View>
                          <Text
                            numberOfLines={1}
                            style={{
                              color: festival ? colors.accent : colors.muted,
                              fontSize: 10,
                              marginTop: 2,
                              textAlign: "center",
                            }}
                          >
                            {label}
                          </Text>
                        </View>
                      );
                    })()
                  )}
                </View>
              )}
            </Each>
          </View>
        </View>

        <View
          style={{
            alignItems: "center",
            backgroundColor: colors.card,
            borderColor: colors.border,
            borderRadius: 14,
            borderWidth: 1,
            flexDirection: "row",
            flexWrap: "wrap",
            gap: 16,
            justifyContent: "space-between",
            padding: 16,
          }}
        >
          <View style={{ flex: 1, gap: 4, minWidth: 240 }}>
            <Text style={{ color: colors.muted, fontSize: 11, fontWeight: "bold" }}>
              {copy.selected}
            </Text>
            <Text style={{ color: colors.foreground, fontSize: 17, fontWeight: "bold" }}>
              {selectedSolar()
                ? formatSolarDate(selectedSolar()!, params.locale)
                : copy.unavailable}
            </Text>
            <Text style={{ color: colors.foreground, fontSize: 14 }}>
              {selectedLunar()
                ? `农历 ${lunarZodiacYear(selectedLunar()!)} ${lunarDateLabel(selectedLunar()!)}`
                : copy.unavailable}
            </Text>
            {selectedFestival() ? (
              <Text style={{ color: colors.accent, fontSize: 13, fontWeight: "bold" }}>
                {copy.festival} · {selectedFestival()}
              </Text>
            ) : null}
          </View>
          <Text style={{ color: status() ? colors.accent : colors.muted, fontSize: 12 }}>
            {status() || copy.copyHint}
          </Text>
        </View>

        <Text style={{ color: colors.muted, fontSize: 11, textAlign: "right" }}>1900–2100</Text>
      </View>
    </Scroll>
  );
}

mount(App);
