import { effect, keymap, locale as systemLocale, mount, signal, Storage, Text, View } from "nsh";
import { calendarHolidays, launchParams, onCommand, onTheme } from "@steward/extension-api";
import { calendarCopyFor, monthTitleFor, relativeDaysLabel } from "./calendar-i18n";
import { holidayScheduleFromApi, officialDayKindFor } from "./calendar-meta";
import { calendarAnnotation, solarToLunar, type SolarDate } from "./lunar";

const MIN_YEAR = 1900;
const MAX_YEAR = 2100;
const GRID_CELLS = 42;
const CONTROL_HEIGHT = 30;
const CONTROL_RADIUS = 8;

interface Palette {
  background: string;
  card: string;
  border: string;
  foreground: string;
  muted: string;
  selection: string;
  accent: string;
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

function parseIsoDate(value: string): SolarDate {
  const [year, month, day] = value.split("-").map(Number);
  return { year, month, day };
}

function dayNumber(date: SolarDate): number {
  return Math.floor(Date.UTC(date.year, date.month - 1, date.day) / 86_400_000);
}

function shiftDate(date: SolarDate, days: number): SolarDate {
  const value = new Date(Date.UTC(date.year, date.month - 1, date.day + days));
  return {
    year: value.getUTCFullYear(),
    month: value.getUTCMonth() + 1,
    day: value.getUTCDate(),
  };
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

interface CalendarCell {
  date: SolarDate;
  currentMonth: boolean;
}

function monthCells(year: number, month: number): CalendarCell[] {
  const cells: CalendarCell[] = [];
  const leading = firstWeekdayMonday(year, month);
  const previousYear = month === 1 ? year - 1 : year;
  const previousMonth = month === 1 ? 12 : month - 1;
  const previousMonthDays = daysInMonth(previousYear, previousMonth);

  for (let index = leading - 1; index >= 0; index -= 1) {
    cells.push({
      date: {
        year: previousYear,
        month: previousMonth,
        day: previousMonthDays - index,
      },
      currentMonth: false,
    });
  }
  for (let day = 1; day <= daysInMonth(year, month); day += 1) {
    cells.push({ date: { year, month, day }, currentMonth: true });
  }

  const nextYear = month === 12 ? year + 1 : year;
  const nextMonth = month === 12 ? 1 : month + 1;
  let nextDay = 1;
  while (cells.length < GRID_CELLS) {
    cells.push({
      date: { year: nextYear, month: nextMonth, day: nextDay },
      currentMonth: false,
    });
    nextDay += 1;
  }
  return cells;
}

function isoWeek(date: SolarDate): number {
  const value = new Date(Date.UTC(date.year, date.month - 1, date.day));
  const weekday = value.getUTCDay() || 7;
  value.setUTCDate(value.getUTCDate() + 4 - weekday);
  const yearStart = Date.UTC(value.getUTCFullYear(), 0, 1);
  return Math.ceil(((value.getTime() - yearStart) / 86_400_000 + 1) / 7);
}

interface CalendarWeek {
  week: number;
  days: CalendarCell[];
}

function monthWeeks(year: number, month: number): CalendarWeek[] {
  const cells = monthCells(year, month);
  const leading = firstWeekdayMonday(year, month);
  return Array.from({ length: GRID_CELLS / 7 }, (_, index) => {
    const monday = new Date(Date.UTC(year, month - 1, 1 - leading + index * 7));
    return {
      week: isoWeek({
        year: monday.getUTCFullYear(),
        month: monday.getUTCMonth() + 1,
        day: monday.getUTCDate(),
      }),
      days: cells.slice(index * 7, (index + 1) * 7),
    };
  });
}

function visibleHolidayYears(year: number, month: number): number[] {
  const years = new Set<number>();
  for (const week of monthWeeks(year, month)) {
    for (const cell of week.days) years.add(cell.date.year);
  }
  return [...years].sort((left, right) => left - right);
}

interface CalendarDisplayCell {
  cell: CalendarCell;
  officialDay: "holiday" | "workday" | null;
}

interface CalendarDisplayWeek {
  week: number;
  days: CalendarDisplayCell[];
}

function monthDisplayWeeks(
  year: number,
  month: number,
  schedule: Record<string, string>,
): CalendarDisplayWeek[] {
  return monthWeeks(year, month).map((week) => ({
    ...week,
    days: week.days.map((cell) => ({
      cell,
      officialDay: officialDayKindFor(cell.date, schedule),
    })),
  }));
}

function holidayCacheKey(year: number): string {
  return `calendar.holiday.v1.${year}`;
}

function cachedHolidaySchedule(value: string | null): Record<string, string> | null {
  if (!value) return null;
  try {
    const parsed = JSON.parse(value) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
    return Object.fromEntries(
      Object.entries(parsed).filter(
        ([date, kind]) =>
          /^\d{4}-\d{2}-\d{2}$/.test(date) && (kind === "holiday" || kind === "workday"),
      ),
    );
  } catch {
    return null;
  }
}

async function fetchHolidaySchedule(year: number): Promise<Record<string, string>> {
  const response = await fetch(`https://timor.tech/api/holiday/year/${year}`, {
    headers: {
      Accept: "application/json",
      "User-Agent": "Steward (+https://github.com/iFence/steward)",
    },
  });
  if (!response.ok) {
    throw new Error(`Timor holiday API returned HTTP ${response.status}`);
  }
  return holidayScheduleFromApi(await response.json());
}

async function loadHolidaySchedule(
  year: number,
  viaSteward: boolean,
): Promise<Record<string, string>> {
  const cacheKey = holidayCacheKey(year);
  try {
    const cached = cachedHolidaySchedule(await Storage.getItem(cacheKey));
    if (cached) return cached;
  } catch {
    // Persistent storage is optional in development hosts.
  }

  try {
    const schedule = viaSteward ? await calendarHolidays(year) : await fetchHolidaySchedule(year);
    void Storage.setItem(cacheKey, JSON.stringify(schedule)).catch(() => {});
    return schedule;
  } catch {
    return {};
  }
}

async function holidaySchedules(
  source: string,
  viaSteward: boolean,
): Promise<Record<string, string>> {
  const schedules = await Promise.all(
    source.split(",").map((value) => loadHolidaySchedule(Number(value), viaSteward)),
  );
  return Object.assign({}, ...schedules);
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
  const accent = signal(params.accent);
  const dark = signal(params.dark);
  const holidaySchedule = signal<Record<string, string>>({});
  const holidayRequests = new Map<string, Promise<Record<string, string>>>();
  effect(() => {
    const source = visibleHolidayYears(year(), month()).join(",");
    let active = true;
    let request = holidayRequests.get(source);
    if (!request) {
      request = holidaySchedules(source, Boolean(params.pluginId));
      holidayRequests.set(source, request);
    }
    void request.then((schedule) => {
      if (active) holidaySchedule.set(schedule);
    });
    return () => {
      active = false;
    };
  });
  onCommand(({ input }) => {
    const next = resolveMonth(input.replace(/^calendar\s*/i, "").trim());
    year.set(next.year);
    month.set(next.month);
    selected.set(initialSelection(next, today()));
  });

  onTheme((theme) => {
    accent.set(theme.accent);
    dark.set(theme.dark);
  });

  // Steward supplies its configured locale. The standalone NSH development
  // host has no launch params, so follow the operating system there.
  const copy = calendarCopyFor(params.pluginId ? params.locale : systemLocale());
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
  };

  const moveMonth = (delta: number) => {
    selectMonth(year(), month() + delta);
  };

  const moveYear = (delta: number) => {
    selectMonth(year() + delta, month());
  };

  const goToday = () => {
    const current = today();
    year.set(current.year);
    month.set(current.month);
    selected.set(isoDate(current));
  };

  const selectDate = (date: SolarDate, currentMonth: boolean) => {
    if (!currentMonth) selectMonth(date.year, date.month);
    selected.set(isoDate(date));
  };

  const moveSelectedDate = (days: number) => {
    const next = shiftDate(parseIsoDate(selected()), days);
    if (next.year < MIN_YEAR || next.year > MAX_YEAR) return;
    selectDate(next, next.year === year() && next.month === month());
  };

  keymap.bind(
    {
      left: () => moveSelectedDate(-1),
      right: () => moveSelectedDate(1),
      up: () => moveSelectedDate(-7),
      down: () => moveSelectedDate(7),
    },
    { repeat: true },
  );

  const current = today();
  const relativeDay = () =>
    relativeDaysLabel(dayNumber(parseIsoDate(selected())) - dayNumber(current), copy);

  return (
    <View
      style={{
        backgroundColor: colors.background,
        flex: 1,
        minHeight: 0,
        minWidth: 0,
        overflow: "hidden",
      }}
      testID="calendar-root"
    >
      <View style={{ flex: 1, gap: 16, minHeight: 0, minWidth: 0, padding: 24 }}>
        <View
          style={{
            alignItems: "center",
            flexDirection: "row",
            flexShrink: 0,
            gap: 12,
            minWidth: 0,
          }}
        >
          <View
            style={{
              alignItems: "flex-end",
              flex: 1,
              flexDirection: "row",
              gap: 8,
              minWidth: 0,
            }}
          >
            <Text
              style={{
                color: colors.foreground,
                flexShrink: 1,
                fontSize: 28,
                fontWeight: "bold",
                minWidth: 0,
                numberOfLines: 1,
                textOverflow: "ellipsis",
              }}
              testID="calendar-month-title"
            >
              {monthTitleFor(year(), month(), copy)}
            </Text>
            {relativeDay() ? (
              <Text
                style={{
                  color: colors.muted,
                  flexShrink: 0,
                  fontSize: 12,
                  lineHeight: 16,
                  paddingBottom: 3,
                }}
                testID="calendar-relative-day"
              >
                {relativeDay()}
              </Text>
            ) : null}
          </View>
          <View style={{ alignItems: "center", flexDirection: "row", flexShrink: 0, gap: 8 }}>
            <View
              accessibilityLabel={copy.previousYear}
              accessible
              disabled={year() === MIN_YEAR}
              onPress={() => moveYear(-1)}
              role="button"
              style={{
                alignItems: "center",
                backgroundColor: colors.card,
                borderColor: colors.border,
                borderRadius: CONTROL_RADIUS,
                borderWidth: 1,
                height: CONTROL_HEIGHT,
                justifyContent: "center",
                width: 32,
              }}
              testID="calendar-prev-year"
            >
              <Text
                style={{
                  color: colors.foreground,
                  fontSize: 20,
                  lineHeight: 20,
                  textAlign: "center",
                  textAlignVertical: "center",
                }}
              >
                «
              </Text>
            </View>
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
                borderRadius: CONTROL_RADIUS,
                borderWidth: 1,
                height: CONTROL_HEIGHT,
                justifyContent: "center",
                width: 32,
              }}
              testID="calendar-prev-month"
            >
              <Text
                style={{
                  color: colors.foreground,
                  fontSize: 20,
                  lineHeight: 20,
                  textAlign: "center",
                  textAlignVertical: "center",
                }}
              >
                ‹
              </Text>
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
                borderRadius: CONTROL_RADIUS,
                borderWidth: 1,
                height: CONTROL_HEIGHT,
                justifyContent: "center",
                paddingHorizontal: 14,
              }}
              testID="calendar-today"
            >
              <Text
                style={{
                  color: colors.foreground,
                  fontSize: 13,
                  lineHeight: 16,
                  textAlign: "center",
                  textAlignVertical: "center",
                }}
              >
                {copy.today}
              </Text>
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
                borderRadius: CONTROL_RADIUS,
                borderWidth: 1,
                height: CONTROL_HEIGHT,
                justifyContent: "center",
                width: 32,
              }}
              testID="calendar-next-month"
            >
              <Text
                style={{
                  color: colors.foreground,
                  fontSize: 20,
                  lineHeight: 20,
                  textAlign: "center",
                  textAlignVertical: "center",
                }}
              >
                ›
              </Text>
            </View>
            <View
              accessibilityLabel={copy.nextYear}
              accessible
              disabled={year() === MAX_YEAR}
              onPress={() => moveYear(1)}
              role="button"
              style={{
                alignItems: "center",
                backgroundColor: colors.card,
                borderColor: colors.border,
                borderRadius: CONTROL_RADIUS,
                borderWidth: 1,
                height: CONTROL_HEIGHT,
                justifyContent: "center",
                width: 32,
              }}
              testID="calendar-next-year"
            >
              <Text
                style={{
                  color: colors.foreground,
                  fontSize: 20,
                  lineHeight: 20,
                  textAlign: "center",
                  textAlignVertical: "center",
                }}
              >
                »
              </Text>
            </View>
          </View>
        </View>

        <View
          style={{
            backgroundColor: colors.card,
            borderColor: colors.border,
            borderRadius: 16,
            borderWidth: 1,
            flex: 1,
            minHeight: 0,
            overflow: "hidden",
            padding: 8,
          }}
        >
          <View
            style={{
              flexDirection: "row",
              flexShrink: 0,
              paddingBottom: 6,
              paddingTop: 2,
            }}
          >
            <View
              style={{ alignItems: "center", padding: 5, width: 40 }}
              testID="calendar-week-header"
            >
              <Text style={{ color: colors.muted, fontSize: 11, fontWeight: "bold" }}>
                {copy.week}
              </Text>
            </View>
            <View style={{ flex: 1, flexDirection: "row" }}>
              <Each of={copy.weekdays} key="index">
                {(weekday) => (
                  <View style={{ alignItems: "center", flex: 1, padding: 5 }}>
                    <Text style={{ color: colors.muted, fontSize: 12, fontWeight: "bold" }}>
                      {weekday()}
                    </Text>
                  </View>
                )}
              </Each>
            </View>
          </View>
          <Each
            of={monthDisplayWeeks(year(), month(), holidaySchedule()).map((week) => ({
              ...week,
              selectedDate: selected(),
            }))}
            key={(week) =>
              `${isoDate(week.days[0].cell.date)}:${week.selectedDate}:${week.days
                .map((day) => day.officialDay ?? "-")
                .join("")}`
            }
          >
            {(week) => (
              <View
                style={{
                  alignItems: "stretch",
                  flex: 1,
                  flexDirection: "row",
                  minHeight: 0,
                }}
              >
                <View
                  accessibilityLabel={copy.weekLabel(week().week)}
                  accessible
                  style={{ alignItems: "center", justifyContent: "center", width: 40 }}
                  testID={`calendar-week-${pad(week().week)}`}
                >
                  <Text style={{ color: colors.muted, fontSize: 11, fontWeight: "bold" }}>
                    {pad(week().week)}
                  </Text>
                </View>
                <Each
                  of={week().days}
                  key={(cell) => `${isoDate(cell.cell.date)}:${cell.officialDay ?? "-"}`}
                >
                  {(cell) => {
                    const value = cell().cell.date;
                    const valueIso = isoDate(value);
                    const lunar = solarToLunar(value.year, value.month, value.day);
                    const annotation = lunar
                      ? calendarAnnotation(lunar, value, copy.locale)
                      : { label: copy.unavailable, emphasized: false };
                    const officialDay = cell().officialDay;
                    const officialBadge =
                      officialDay === "holiday"
                        ? copy.holidayBadge
                        : officialDay === "workday"
                          ? copy.workdayBadge
                          : null;
                    const isSelected = valueIso === week().selectedDate;
                    const isToday =
                      cell().cell.currentMonth &&
                      value.year === current.year &&
                      value.month === current.month &&
                      value.day === current.day;
                    return (
                      <View
                        style={{
                          flex: 1,
                          minHeight: 0,
                          minWidth: 0,
                          overflow: "hidden",
                          padding: 2,
                        }}
                      >
                        <View
                          accessibilityLabel={`${valueIso}${copy.dateSeparator}${annotation.label}${
                            officialBadge ? `${copy.dateSeparator}${officialBadge}` : ""
                          }`}
                          accessible
                          onPress={() => selectDate(value, cell().cell.currentMonth)}
                          role="button"
                          style={{
                            alignItems: "center",
                            backgroundColor: isSelected ? colors.selection : "transparent",
                            borderColor: isSelected ? colors.accent : "transparent",
                            borderRadius: 10,
                            borderWidth: isSelected ? 2 : 1,
                            flex: 1,
                            justifyContent: "center",
                            minHeight: 0,
                            padding: 3,
                            position: "relative",
                            width: "100%",
                          }}
                          testID={`calendar-day-${valueIso}`}
                        >
                          {isSelected ? (
                            <View
                              style={{
                                backgroundColor: colors.accent,
                                borderRadius: 3,
                                height: 6,
                                position: "absolute",
                                right: 5,
                                top: 5,
                                width: 6,
                              }}
                              testID={`calendar-selected-${valueIso}`}
                            />
                          ) : null}
                          {officialDay ? (
                            <View
                              style={{
                                alignItems: "center",
                                backgroundColor:
                                  cell().cell.currentMonth && officialDay === "holiday"
                                    ? colors.accent
                                    : colors.muted,
                                borderBottomRightRadius: 7,
                                height: 14,
                                justifyContent: "center",
                                left: 0,
                                minWidth: 18,
                                paddingHorizontal: 2,
                                position: "absolute",
                                top: 0,
                              }}
                              testID={`calendar-badge-${valueIso}`}
                            >
                              <Text
                                style={{
                                  color: "#ffffff",
                                  fontSize: 8,
                                  fontWeight: "bold",
                                  lineHeight: 10,
                                  numberOfLines: 1,
                                  textAlign: "center",
                                  textAlignVertical: "center",
                                }}
                              >
                                {officialBadge}
                              </Text>
                            </View>
                          ) : null}
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
                                color: isToday
                                  ? "#ffffff"
                                  : isSelected
                                    ? colors.accent
                                    : cell().cell.currentMonth
                                      ? colors.foreground
                                      : colors.muted,
                                fontSize: 16,
                                fontWeight: isToday || isSelected ? "bold" : "normal",
                              }}
                            >
                              {value.day}
                            </Text>
                          </View>
                          <Text
                            style={{
                              color:
                                cell().cell.currentMonth && annotation.emphasized
                                  ? colors.accent
                                  : colors.muted,
                              fontSize: 10,
                              marginTop: 2,
                              numberOfLines: 1,
                              textAlign: "center",
                              textOverflow: "clip",
                            }}
                          >
                            {annotation.label}
                          </Text>
                        </View>
                      </View>
                    );
                  }}
                </Each>
              </View>
            )}
          </Each>
        </View>
      </View>
    </View>
  );
}

mount(App);
