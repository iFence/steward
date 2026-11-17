import type { CalendarLocale } from "./calendar-i18n";
import type { SolarDate } from "./lunar";

export type OfficialDayKind = "holiday" | "workday";

interface TimorHolidayInfo {
  holiday: boolean;
  date: string;
}

interface TimorYearResponse {
  code: number;
  holiday?: Record<string, TimorHolidayInfo | null>;
}

const SOLAR_TERM_NAMES: Record<CalendarLocale, string[]> = {
  en: [
    "Minor Cold",
    "Major Cold",
    "Start of Spring",
    "Rain Water",
    "Awakening of Insects",
    "Spring Equinox",
    "Qingming",
    "Grain Rain",
    "Start of Summer",
    "Grain Buds",
    "Grain in Ear",
    "Summer Solstice",
    "Minor Heat",
    "Major Heat",
    "Start of Autumn",
    "End of Heat",
    "White Dew",
    "Autumn Equinox",
    "Cold Dew",
    "Frost’s Descent",
    "Start of Winter",
    "Minor Snow",
    "Major Snow",
    "Winter Solstice",
  ],
  zh: [
    "小寒",
    "大寒",
    "立春",
    "雨水",
    "惊蛰",
    "春分",
    "清明",
    "谷雨",
    "立夏",
    "小满",
    "芒种",
    "夏至",
    "小暑",
    "大暑",
    "立秋",
    "处暑",
    "白露",
    "秋分",
    "寒露",
    "霜降",
    "立冬",
    "小雪",
    "大雪",
    "冬至",
  ],
  ja: [
    "小寒",
    "大寒",
    "立春",
    "雨水",
    "啓蟄",
    "春分",
    "清明",
    "穀雨",
    "立夏",
    "小満",
    "芒種",
    "夏至",
    "小暑",
    "大暑",
    "立秋",
    "処暑",
    "白露",
    "秋分",
    "寒露",
    "霜降",
    "立冬",
    "小雪",
    "大雪",
    "冬至",
  ],
  ko: [
    "소한",
    "대한",
    "입춘",
    "우수",
    "경칩",
    "춘분",
    "청명",
    "곡우",
    "입하",
    "소만",
    "망종",
    "하지",
    "소서",
    "대서",
    "입추",
    "처서",
    "백로",
    "추분",
    "한로",
    "상강",
    "입동",
    "소설",
    "대설",
    "동지",
  ],
  de: [
    "Kleine Kälte",
    "Große Kälte",
    "Frühlingsanfang",
    "Regenwasser",
    "Insekten erwachen",
    "Frühlingsäquinoktium",
    "Qingming",
    "Kornregen",
    "Sommeranfang",
    "Kleine Fülle",
    "Ährenzeit",
    "Sommersonnenwende",
    "Kleine Hitze",
    "Große Hitze",
    "Herbstanfang",
    "Ende der Hitze",
    "Weißer Tau",
    "Herbstäquinoktium",
    "Kalter Tau",
    "Frostfall",
    "Winteranfang",
    "Kleiner Schnee",
    "Großer Schnee",
    "Wintersonnenwende",
  ],
  fr: [
    "Petit froid",
    "Grand froid",
    "Début du printemps",
    "Eau de pluie",
    "Éveil des insectes",
    "Équinoxe de printemps",
    "Qingming",
    "Pluie de céréales",
    "Début de l’été",
    "Petite plénitude",
    "Grain en épi",
    "Solstice d’été",
    "Petite chaleur",
    "Grande chaleur",
    "Début de l’automne",
    "Fin de la chaleur",
    "Rosée blanche",
    "Équinoxe d’automne",
    "Rosée froide",
    "Descente du givre",
    "Début de l’hiver",
    "Petite neige",
    "Grande neige",
    "Solstice d’hiver",
  ],
  ru: [
    "Малые холода",
    "Большие холода",
    "Начало весны",
    "Дождевая вода",
    "Пробуждение насекомых",
    "Весеннее равноденствие",
    "Цинмин",
    "Хлебные дожди",
    "Начало лета",
    "Малое изобилие",
    "Колошение хлебов",
    "Летнее солнцестояние",
    "Малая жара",
    "Большая жара",
    "Начало осени",
    "Конец жары",
    "Белая роса",
    "Осеннее равноденствие",
    "Холодная роса",
    "Выпадение инея",
    "Начало зимы",
    "Малый снег",
    "Большой снег",
    "Зимнее солнцестояние",
  ],
};

// Solar-term date table adapted from lion1ou/calendar (MIT).
// See THIRD_PARTY_NOTICES.md for the full notice.
const SOLAR_TERM_TABLE = `
9778397bd097c36b0b6fc9274c91aa 97b6b97bd19801ec9210c965cc920e 97bcf97c3598082c95f8c965cc920f 97bd0b06bdb0722c965ce1cfcc920f b027097bd097c36b0b6fc9274c91aa 97b6b97bd19801ec9210c965cc920e
97bcf97c359801ec95f8c965cc920f 97bd0b06bdb0722c965ce1cfcc920f b027097bd097c36b0b6fc9274c91aa 97b6b97bd19801ec9210c965cc920e 97bcf97c359801ec95f8c965cc920f 97bd0b06bdb0722c965ce1cfcc920f
b027097bd097c36b0b6fc9274c91aa 9778397bd19801ec9210c965cc920e 97b6b97bd19801ec95f8c965cc920f 97bd09801d98082c95f8e1cfcc920f 97bd097bd097c36b0b6fc9210c8dc2 9778397bd197c36c9210c9274c91aa
97b6b97bd19801ec95f8c965cc920e 97bd09801d98082c95f8e1cfcc920f 97bd097bd097c36b0b6fc9210c8dc2 9778397bd097c36c9210c9274c91aa 97b6b97bd19801ec95f8c965cc920e 97bcf97c3598082c95f8e1cfcc920f
97bd097bd097c36b0b6fc9210c8dc2 9778397bd097c36c9210c9274c91aa 97b6b97bd19801ec9210c965cc920e 97bcf97c3598082c95f8c965cc920f 97bd097bd097c35b0b6fc920fb0722 9778397bd097c36b0b6fc9274c91aa
97b6b97bd19801ec9210c965cc920e 97bcf97c3598082c95f8c965cc920f 97bd097bd097c35b0b6fc920fb0722 9778397bd097c36b0b6fc9274c91aa 97b6b97bd19801ec9210c965cc920e 97bcf97c359801ec95f8c965cc920f
97bd097bd097c35b0b6fc920fb0722 9778397bd097c36b0b6fc9274c91aa 97b6b97bd19801ec9210c965cc920e 97bcf97c359801ec95f8c965cc920f 97bd097bd097c35b0b6fc920fb0722 9778397bd097c36b0b6fc9274c91aa
97b6b97bd19801ec9210c965cc920e 97bcf97c359801ec95f8c965cc920f 97bd097bd07f595b0b6fc920fb0722 9778397bd097c36b0b6fc9210c8dc2 9778397bd19801ec9210c9274c920e 97b6b97bd19801ec95f8c965cc920f
97bd07f5307f595b0b0bc920fb0722 7f0e397bd097c36b0b6fc9210c8dc2 9778397bd097c36c9210c9274c920e 97b6b97bd19801ec95f8c965cc920f 97bd07f5307f595b0b0bc920fb0722 7f0e397bd097c36b0b6fc9210c8dc2
9778397bd097c36c9210c9274c91aa 97b6b97bd19801ec9210c965cc920e 97bd07f1487f595b0b0bc920fb0722 7f0e397bd097c36b0b6fc9210c8dc2 9778397bd097c36b0b6fc9274c91aa 97b6b97bd19801ec9210c965cc920e
97bcf7f1487f595b0b0bb0b6fb0722 7f0e397bd097c35b0b6fc920fb0722 9778397bd097c36b0b6fc9274c91aa 97b6b97bd19801ec9210c965cc920e 97bcf7f1487f595b0b0bb0b6fb0722 7f0e397bd097c35b0b6fc920fb0722
9778397bd097c36b0b6fc9274c91aa 97b6b97bd19801ec9210c965cc920e 97bcf7f1487f531b0b0bb0b6fb0722 7f0e397bd097c35b0b6fc920fb0722 9778397bd097c36b0b6fc9274c91aa 97b6b97bd19801ec9210c965cc920e
97bcf7f1487f531b0b0bb0b6fb0722 7f0e397bd07f595b0b6fc920fb0722 9778397bd097c36b0b6fc9274c91aa 97b6b97bd19801ec9210c9274c920e 97bcf7f0e47f531b0b0bb0b6fb0722 7f0e397bd07f595b0b0bc920fb0722
9778397bd097c36b0b6fc9210c91aa 97b6b97bd197c36c9210c9274c920e 97bcf7f0e47f531b0b0bb0b6fb0722 7f0e397bd07f595b0b0bc920fb0722 9778397bd097c36b0b6fc9210c8dc2 9778397bd097c36c9210c9274c920e
97b6b7f0e47f531b0723b0b6fb0722 7f0e37f5307f595b0b0bc920fb0722 7f0e397bd097c36b0b6fc9210c8dc2 9778397bd097c36b0b70c9274c91aa 97b6b7f0e47f531b0723b0b6fb0721 7f0e37f1487f595b0b0bb0b6fb0722
7f0e397bd097c35b0b6fc9210c8dc2 9778397bd097c36b0b6fc9274c91aa 97b6b7f0e47f531b0723b0b6fb0721 7f0e27f1487f595b0b0bb0b6fb0722 7f0e397bd097c35b0b6fc920fb0722 9778397bd097c36b0b6fc9274c91aa
97b6b7f0e47f531b0723b0b6fb0721 7f0e27f1487f531b0b0bb0b6fb0722 7f0e397bd097c35b0b6fc920fb0722 9778397bd097c36b0b6fc9274c91aa 97b6b7f0e47f531b0723b0b6fb0721 7f0e27f1487f531b0b0bb0b6fb0722
7f0e397bd097c35b0b6fc920fb0722 9778397bd097c36b0b6fc9274c91aa 97b6b7f0e47f531b0723b0787b0721 7f0e27f0e47f531b0b0bb0b6fb0722 7f0e397bd07f595b0b0bc920fb0722 9778397bd097c36b0b6fc9210c91aa
97b6b7f0e47f149b0723b0787b0721 7f0e27f0e47f531b0723b0b6fb0722 7f0e397bd07f595b0b0bc920fb0722 9778397bd097c36b0b6fc9210c8dc2 977837f0e37f149b0723b0787b0721 7f07e7f0e47f531b0723b0b6fb0722
7f0e37f5307f595b0b0bc920fb0722 7f0e397bd097c35b0b6fc9210c8dc2 977837f0e37f14998082b0787b0721 7f07e7f0e47f531b0723b0b6fb0721 7f0e37f1487f595b0b0bb0b6fb0722 7f0e397bd097c35b0b6fc9210c8dc2
977837f0e37f14998082b0787b06bd 7f07e7f0e47f531b0723b0b6fb0721 7f0e27f1487f531b0b0bb0b6fb0722 7f0e397bd097c35b0b6fc920fb0722 977837f0e37f14998082b0787b06bd 7f07e7f0e47f531b0723b0b6fb0721
7f0e27f1487f531b0b0bb0b6fb0722 7f0e397bd097c35b0b6fc920fb0722 977837f0e37f14998082b0787b06bd 7f07e7f0e47f531b0723b0b6fb0721 7f0e27f1487f531b0b0bb0b6fb0722 7f0e397bd07f595b0b0bc920fb0722
977837f0e37f14998082b0787b06bd 7f07e7f0e47f531b0723b0b6fb0721 7f0e27f1487f531b0b0bb0b6fb0722 7f0e397bd07f595b0b0bc920fb0722 977837f0e37f14998082b0787b06bd 7f07e7f0e47f149b0723b0787b0721
7f0e27f0e47f531b0b0bb0b6fb0722 7f0e397bd07f595b0b0bc920fb0722 977837f0e37f14998082b0723b06bd 7f07e7f0e37f149b0723b0787b0721 7f0e27f0e47f531b0723b0b6fb0722 7f0e397bd07f595b0b0bc920fb0722
977837f0e37f14898082b0723b02d5 7ec967f0e37f14998082b0787b0721 7f07e7f0e47f531b0723b0b6fb0722 7f0e37f1487f595b0b0bb0b6fb0722 7f0e37f0e37f14898082b0723b02d5 7ec967f0e37f14998082b0787b0721
7f07e7f0e47f531b0723b0b6fb0722 7f0e37f1487f531b0b0bb0b6fb0722 7f0e37f0e37f14898082b0723b02d5 7ec967f0e37f14998082b0787b06bd 7f07e7f0e47f531b0723b0b6fb0721 7f0e37f1487f531b0b0bb0b6fb0722
7f0e37f0e37f14898082b072297c35 7ec967f0e37f14998082b0787b06bd 7f07e7f0e47f531b0723b0b6fb0721 7f0e27f1487f531b0b0bb0b6fb0722 7f0e37f0e37f14898082b072297c35 7ec967f0e37f14998082b0787b06bd
7f07e7f0e47f531b0723b0b6fb0721 7f0e27f1487f531b0b0bb0b6fb0722 7f0e37f0e366aa89801eb072297c35 7ec967f0e37f14998082b0787b06bd 7f07e7f0e47f149b0723b0787b0721 7f0e27f1487f531b0b0bb0b6fb0722
7f0e37f0e366aa89801eb072297c35 7ec967f0e37f14998082b0723b06bd 7f07e7f0e47f149b0723b0787b0721 7f0e27f0e47f531b0723b0b6fb0722 7f0e37f0e366aa89801eb072297c35 7ec967f0e37f14998082b0723b06bd
7f07e7f0e37f14998083b0787b0721 7f0e27f0e47f531b0723b0b6fb0722 7f0e37f0e366aa89801eb072297c35 7ec967f0e37f14898082b0723b02d5 7f07e7f0e37f14998082b0787b0721 7f07e7f0e47f531b0723b0b6fb0722
7f0e36665b66aa89801e9808297c35 665f67f0e37f14898082b0723b02d5 7ec967f0e37f14998082b0787b0721 7f07e7f0e47f531b0723b0b6fb0722 7f0e36665b66a449801e9808297c35 665f67f0e37f14898082b0723b02d5
7ec967f0e37f14998082b0787b06bd 7f07e7f0e47f531b0723b0b6fb0721 7f0e36665b66a449801e9808297c35 665f67f0e37f14898082b072297c35 7ec967f0e37f14998082b0787b06bd 7f07e7f0e47f531b0723b0b6fb0721
7f0e26665b66a449801e9808297c35 665f67f0e37f1489801eb072297c35 7ec967f0e37f14998082b0787b06bd 7f07e7f0e47f531b0723b0b6fb0721 7f0e27f1487f531b0b0bb0b6fb0722
`
  .trim()
  .split(/\s+/);

const solarTermDaysCache = new Map<number, number[]>();

function solarTermDays(year: number): number[] | null {
  if (year < 1900 || year > 2100) return null;
  const cached = solarTermDaysCache.get(year);
  if (cached) return cached;

  const table = SOLAR_TERM_TABLE[year - 1900];
  if (!table) return null;
  const packed = [0, 5, 10, 15, 20, 25].map((start) =>
    Number.parseInt(table.slice(start, start + 5), 16).toString(),
  );
  const days: number[] = [];
  for (const value of packed) {
    days.push(
      Number(value[0]),
      Number(value.slice(1, 3)),
      Number(value[3]),
      Number(value.slice(4, 6)),
    );
  }
  solarTermDaysCache.set(year, days);
  return days;
}

export function solarTermFor(date: SolarDate, locale: CalendarLocale): string | null {
  const days = solarTermDays(date.year);
  if (!days) return null;
  const firstTerm = (date.month - 1) * 2;
  if (days[firstTerm] === date.day) return SOLAR_TERM_NAMES[locale][firstTerm];
  if (days[firstTerm + 1] === date.day) return SOLAR_TERM_NAMES[locale][firstTerm + 1];
  return null;
}

function dateKey(date: SolarDate): string {
  return `${date.year}-${String(date.month).padStart(2, "0")}-${String(date.day).padStart(2, "0")}`;
}

export function holidayScheduleFromApi(payload: unknown): Record<string, OfficialDayKind> {
  const response = payload as TimorYearResponse;
  if (response.code !== 0) throw new Error(`Timor holiday API returned code ${response.code}`);

  const schedule: Record<string, OfficialDayKind> = {};
  for (const info of Object.values(response.holiday ?? {})) {
    if (!info || typeof info.date !== "string") continue;
    schedule[info.date] = info.holiday ? "holiday" : "workday";
  }
  return schedule;
}

export function officialDayKindFor(
  date: SolarDate,
  schedule: Record<string, string>,
): OfficialDayKind | null {
  const kind = schedule[dateKey(date)];
  return kind === "holiday" || kind === "workday" ? kind : null;
}
