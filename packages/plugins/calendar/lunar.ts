import type { CalendarLocale } from "./calendar-i18n";
import { solarTermFor } from "./calendar-meta";

const MIN_LUNAR_YEAR = 1900;
const MAX_LUNAR_YEAR = 2100;
const DAY_MS = 24 * 60 * 60 * 1000;
const LUNAR_EPOCH = Date.UTC(1900, 0, 31);

// Each value packs the twelve lunar month lengths for one year, the leap
// month number in the low nibble, and a 30-day flag for the leap month.
const LUNAR_INFO = [
  0x04bd8, 0x04ae0, 0x0a570, 0x054d5, 0x0d260, 0x0d950, 0x16554, 0x056a0, 0x09ad0, 0x055d2, 0x04ae0,
  0x0a5b6, 0x0a4d0, 0x0d250, 0x1d255, 0x0b540, 0x0d6a0, 0x0ada2, 0x095b0, 0x14977, 0x04970, 0x0a4b0,
  0x0b4b5, 0x06a50, 0x06d40, 0x1ab54, 0x02b60, 0x09570, 0x052f2, 0x04970, 0x06566, 0x0d4a0, 0x0ea50,
  0x06e95, 0x05ad0, 0x02b60, 0x186e3, 0x092e0, 0x1c8d7, 0x0c950, 0x0d4a0, 0x1d8a6, 0x0b550, 0x056a0,
  0x1a5b4, 0x025d0, 0x092d0, 0x0d2b2, 0x0a950, 0x0b557, 0x06ca0, 0x0b550, 0x15355, 0x04da0, 0x0a5b0,
  0x14573, 0x052b0, 0x0a9a8, 0x0e950, 0x06aa0, 0x0aea6, 0x0ab50, 0x04b60, 0x0aae4, 0x0a570, 0x05260,
  0x0f263, 0x0d950, 0x05b57, 0x056a0, 0x096d0, 0x04dd5, 0x04ad0, 0x0a4d0, 0x0d4d4, 0x0d250, 0x0d558,
  0x0b540, 0x0b6a0, 0x195a6, 0x095b0, 0x049b0, 0x0a974, 0x0a4b0, 0x0b27a, 0x06a50, 0x06d40, 0x0af46,
  0x0ab60, 0x09570, 0x04af5, 0x04970, 0x064b0, 0x074a3, 0x0ea50, 0x06b58, 0x055c0, 0x0ab60, 0x096d5,
  0x092e0, 0x0c960, 0x0d954, 0x0d4a0, 0x0da50, 0x07552, 0x056a0, 0x0abb7, 0x025d0, 0x092d0, 0x0cab5,
  0x0a950, 0x0b4a0, 0x0baa4, 0x0ad50, 0x055d9, 0x04ba0, 0x0a5b0, 0x15176, 0x052b0, 0x0a930, 0x07954,
  0x06aa0, 0x0ad50, 0x05b52, 0x04b60, 0x0a6e6, 0x0a4e0, 0x0d260, 0x0ea65, 0x0d530, 0x05aa0, 0x076a3,
  0x096d0, 0x04afb, 0x04ad0, 0x0a4d0, 0x1d0b6, 0x0d250, 0x0d520, 0x0dd45, 0x0b5a0, 0x056d0, 0x055b2,
  0x049b0, 0x0a577, 0x0a4b0, 0x0aa50, 0x1b255, 0x06d20, 0x0ada0, 0x14b63, 0x09370, 0x049f8, 0x04970,
  0x064b0, 0x168a6, 0x0ea50, 0x06b20, 0x1a6c4, 0x0aae0, 0x0a2e0, 0x0d2e3, 0x0c960, 0x0d557, 0x0d4a0,
  0x0da50, 0x05d55, 0x056a0, 0x0a6d0, 0x055d4, 0x052d0, 0x0a9b8, 0x0a950, 0x0b4a0, 0x0b6a6, 0x0ad50,
  0x055a0, 0x0aba4, 0x0a5b0, 0x052b0, 0x0b273, 0x06930, 0x07337, 0x06aa0, 0x0ad50, 0x14b55, 0x04b60,
  0x0a570, 0x054e4, 0x0d160, 0x0e968, 0x0d520, 0x0daa0, 0x16aa6, 0x056d0, 0x04ae0, 0x0a9d4, 0x0a2d0,
  0x0d150, 0x0f252, 0x0d520,
] as const;

const LUNAR_MONTHS = [
  "",
  "正月",
  "二月",
  "三月",
  "四月",
  "五月",
  "六月",
  "七月",
  "八月",
  "九月",
  "十月",
  "冬月",
  "腊月",
] as const;

const LUNAR_DAYS = [
  "",
  "初一",
  "初二",
  "初三",
  "初四",
  "初五",
  "初六",
  "初七",
  "初八",
  "初九",
  "初十",
  "十一",
  "十二",
  "十三",
  "十四",
  "十五",
  "十六",
  "十七",
  "十八",
  "十九",
  "二十",
  "廿一",
  "廿二",
  "廿三",
  "廿四",
  "廿五",
  "廿六",
  "廿七",
  "廿八",
  "廿九",
  "三十",
] as const;

const HEAVENLY_STEMS = ["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸"] as const;
const EARTHLY_BRANCHES = [
  "子",
  "丑",
  "寅",
  "卯",
  "辰",
  "巳",
  "午",
  "未",
  "申",
  "酉",
  "戌",
  "亥",
] as const;
const ZODIAC = ["鼠", "牛", "虎", "兔", "龙", "蛇", "马", "羊", "猴", "鸡", "狗", "猪"] as const;

const LUNAR_FESTIVALS: Record<string, string> = {
  "1-1": "春节",
  "1-15": "元宵节",
  "2-2": "龙抬头",
  "5-5": "端午节",
  "7-7": "七夕节",
  "7-15": "中元节",
  "8-15": "中秋节",
  "9-9": "重阳节",
  "12-8": "腊八节",
};

const SOLAR_FESTIVALS: Record<string, string> = {
  "1-1": "元旦",
  "2-14": "情人节",
  "3-7": "女生节",
  "3-8": "妇女节",
  "3-12": "植树节",
  "3-15": "消费者权益日",
  "4-1": "愚人节",
  "5-1": "劳动节",
  "5-4": "青年节",
  "5-12": "护士节",
  "5-31": "无烟日",
  "6-1": "儿童节",
  "7-1": "建党节",
  "8-1": "建军节",
  "8-6": "电影节",
  "9-10": "教师节",
  "10-1": "国庆节",
  "10-24": "程序员节",
  "10-31": "万圣夜",
  "11-1": "万圣节",
  "11-11": "光棍节",
  "12-13": "公祭日",
  "12-24": "平安夜",
  "12-25": "圣诞节",
};

export type LunarLocale = CalendarLocale;

const LOCALIZED_LUNAR_FESTIVALS: Record<LunarLocale, Record<string, string>> = {
  en: {
    "1-1": "New Year",
    "1-15": "Lantern Festival",
    "2-2": "Dragon Head",
    "5-5": "Dragon Boat",
    "7-7": "Qixi",
    "7-15": "Ghost Festival",
    "8-15": "Mid-Autumn",
    "9-9": "Double Ninth",
    "12-8": "Laba",
  },
  zh: LUNAR_FESTIVALS,
  ja: {
    "1-1": "春節",
    "1-15": "元宵節",
    "2-2": "龍頭節",
    "5-5": "端午節",
    "7-7": "七夕",
    "7-15": "中元節",
    "8-15": "中秋節",
    "9-9": "重陽節",
    "12-8": "臘八節",
  },
  ko: {
    "1-1": "설날",
    "1-15": "정월대보름",
    "2-2": "용두절",
    "5-5": "단오",
    "7-7": "칠석",
    "7-15": "백중",
    "8-15": "추석",
    "9-9": "중양절",
    "12-8": "납팔절",
  },
  de: {
    "1-1": "Neujahr",
    "1-15": "Laternenfest",
    "2-2": "Drachenkopf",
    "5-5": "Drachenboot",
    "7-7": "Qixi",
    "7-15": "Geisterfest",
    "8-15": "Mittherbst",
    "9-9": "Chongyang",
    "12-8": "Laba",
  },
  fr: {
    "1-1": "Nouvel An",
    "1-15": "Lanternes",
    "2-2": "Tête du dragon",
    "5-5": "Bateaux-dragons",
    "7-7": "Qixi",
    "7-15": "Fantômes",
    "8-15": "Mi-automne",
    "9-9": "Double neuf",
    "12-8": "Laba",
  },
  ru: {
    "1-1": "Новый год",
    "1-15": "Фонари",
    "2-2": "Голова дракона",
    "5-5": "Драконьи лодки",
    "7-7": "Циси",
    "7-15": "Призраки",
    "8-15": "Середина осени",
    "9-9": "Двойная девятка",
    "12-8": "Лаба",
  },
};

const LOCALIZED_SOLAR_FESTIVALS: Record<LunarLocale, Record<string, string>> = {
  en: {
    "1-1": "New Year’s Day",
    "2-14": "Valentine’s Day",
    "3-7": "Girls’ Day",
    "3-8": "Women’s Day",
    "3-12": "Arbor Day",
    "3-15": "Consumer Rights Day",
    "4-1": "April Fools’ Day",
    "5-1": "Labour Day",
    "5-4": "Youth Day",
    "5-12": "Nurses’ Day",
    "5-31": "No Tobacco Day",
    "6-1": "Children’s Day",
    "7-1": "Party Founding Day",
    "8-1": "Army Day",
    "8-6": "Film Festival",
    "9-10": "Teachers’ Day",
    "10-1": "National Day",
    "10-24": "Programmers’ Day",
    "10-31": "Halloween",
    "11-1": "All Saints’ Day",
    "11-11": "Singles’ Day",
    "12-13": "National Memorial Day",
    "12-24": "Christmas Eve",
    "12-25": "Christmas",
  },
  zh: SOLAR_FESTIVALS,
  ja: {
    "1-1": "元日",
    "2-14": "バレンタインデー",
    "3-7": "ガールズデー",
    "3-8": "女性の日",
    "3-12": "植樹祭",
    "3-15": "消費者権利デー",
    "4-1": "エイプリルフール",
    "5-1": "労働の日",
    "5-4": "青年の日",
    "5-12": "看護師の日",
    "5-31": "世界禁煙デー",
    "6-1": "こどもの日",
    "7-1": "中国共産党創立記念日",
    "8-1": "建軍記念日",
    "8-6": "映画祭",
    "9-10": "教師の日",
    "10-1": "国慶節",
    "10-24": "プログラマーの日",
    "10-31": "ハロウィン",
    "11-1": "諸聖人の日",
    "11-11": "独身の日",
    "12-13": "国家追悼の日",
    "12-24": "クリスマスイブ",
    "12-25": "クリスマス",
  },
  ko: {
    "1-1": "신정",
    "2-14": "밸런타인데이",
    "3-7": "걸스데이",
    "3-8": "세계 여성의 날",
    "3-12": "식목일",
    "3-15": "소비자 권리의 날",
    "4-1": "만우절",
    "5-1": "노동절",
    "5-4": "청년의 날",
    "5-12": "간호사의 날",
    "5-31": "세계 금연의 날",
    "6-1": "어린이날",
    "7-1": "건당 기념일",
    "8-1": "건군 기념일",
    "8-6": "영화제",
    "9-10": "스승의 날",
    "10-1": "국경절",
    "10-24": "프로그래머의 날",
    "10-31": "핼러윈",
    "11-1": "모든 성인의 날",
    "11-11": "솔로데이",
    "12-13": "국가추모일",
    "12-24": "크리스마스 이브",
    "12-25": "크리스마스",
  },
  de: {
    "1-1": "Neujahr",
    "2-14": "Valentinstag",
    "3-7": "Mädchentag",
    "3-8": "Frauentag",
    "3-12": "Tag des Baumes",
    "3-15": "Weltverbrauchertag",
    "4-1": "Aprilscherz",
    "5-1": "Tag der Arbeit",
    "5-4": "Jugendtag",
    "5-12": "Tag der Pflege",
    "5-31": "Weltnichtrauchertag",
    "6-1": "Kindertag",
    "7-1": "Parteigründungstag",
    "8-1": "Armeetag",
    "8-6": "Filmfestival",
    "9-10": "Lehrertag",
    "10-1": "Nationalfeiertag",
    "10-24": "Programmierertag",
    "10-31": "Halloween",
    "11-1": "Allerheiligen",
    "11-11": "Singles’ Day",
    "12-13": "Nationaler Gedenktag",
    "12-24": "Heiligabend",
    "12-25": "Weihnachten",
  },
  fr: {
    "1-1": "Jour de l’An",
    "2-14": "Saint-Valentin",
    "3-7": "Journée des filles",
    "3-8": "Journée des femmes",
    "3-12": "Fête de l’arbre",
    "3-15": "Journée des consommateurs",
    "4-1": "Poisson d’avril",
    "5-1": "Fête du Travail",
    "5-4": "Journée de la jeunesse",
    "5-12": "Journée des infirmiers",
    "5-31": "Journée sans tabac",
    "6-1": "Fête des enfants",
    "7-1": "Fondation du Parti",
    "8-1": "Fête de l’Armée",
    "8-6": "Festival du film",
    "9-10": "Fête des enseignants",
    "10-1": "Fête nationale",
    "10-24": "Journée des programmeurs",
    "10-31": "Halloween",
    "11-1": "Toussaint",
    "11-11": "Journée des célibataires",
    "12-13": "Journée du souvenir national",
    "12-24": "Réveillon de Noël",
    "12-25": "Noël",
  },
  ru: {
    "1-1": "Новый год",
    "2-14": "День Валентина",
    "3-7": "День девушек",
    "3-8": "8 Марта",
    "3-12": "День посадки деревьев",
    "3-15": "День прав потребителей",
    "4-1": "День смеха",
    "5-1": "Первое мая",
    "5-4": "День молодёжи",
    "5-12": "День медсестры",
    "5-31": "Всемирный день без табака",
    "6-1": "День детей",
    "7-1": "День основания партии",
    "8-1": "День армии",
    "8-6": "Кинофестиваль",
    "9-10": "День учителя",
    "10-1": "Национальный день",
    "10-24": "День программиста",
    "10-31": "Хэллоуин",
    "11-1": "День всех святых",
    "11-11": "День холостяков",
    "12-13": "День национального траура",
    "12-24": "Сочельник",
    "12-25": "Рождество",
  },
};

interface WeekFestival {
  month: number;
  occurrence: number;
  weekday: number;
  names: Record<LunarLocale, string>;
}

const WEEK_FESTIVALS: WeekFestival[] = [
  {
    month: 5,
    occurrence: 2,
    weekday: 0,
    names: {
      en: "Mother’s Day",
      zh: "母亲节",
      ja: "母の日",
      ko: "어머니날",
      de: "Muttertag",
      fr: "Fête des mères",
      ru: "День матери",
    },
  },
  {
    month: 6,
    occurrence: 3,
    weekday: 0,
    names: {
      en: "Father’s Day",
      zh: "父亲节",
      ja: "父の日",
      ko: "아버지날",
      de: "Vatertag",
      fr: "Fête des pères",
      ru: "День отца",
    },
  },
  {
    month: 11,
    occurrence: 4,
    weekday: 4,
    names: {
      en: "Thanksgiving",
      zh: "感恩节",
      ja: "感謝祭",
      ko: "추수감사절",
      de: "Thanksgiving",
      fr: "Action de grâce",
      ru: "День благодарения",
    },
  },
];

function normalizeLunarLocale(locale: string): LunarLocale {
  const language = locale.toLowerCase().replaceAll("_", "-").split("-")[0];
  switch (language) {
    case "zh":
      return "zh";
    case "ja":
      return "ja";
    case "ko":
      return "ko";
    case "de":
      return "de";
    case "fr":
      return "fr";
    case "ru":
      return "ru";
    default:
      return "en";
  }
}

export interface SolarDate {
  year: number;
  month: number;
  day: number;
}

export interface LunarDate {
  year: number;
  month: number;
  day: number;
  isLeapMonth: boolean;
}

function infoFor(year: number): number | null {
  return LUNAR_INFO[year - MIN_LUNAR_YEAR] ?? null;
}

function leapMonth(year: number): number {
  return (infoFor(year) ?? 0) & 0xf;
}

function leapDays(year: number): number {
  if (!leapMonth(year)) return 0;
  return ((infoFor(year) ?? 0) & 0x10000) !== 0 ? 30 : 29;
}

function monthDays(year: number, month: number): number {
  return ((infoFor(year) ?? 0) & (0x10000 >> month)) !== 0 ? 30 : 29;
}

function lunarYearDays(year: number): number {
  let days = 0;
  for (let month = 1; month <= 12; month += 1) {
    days += monthDays(year, month);
  }
  return days + leapDays(year);
}

function validSolarDate(date: SolarDate): boolean {
  if (
    !Number.isInteger(date.year) ||
    !Number.isInteger(date.month) ||
    !Number.isInteger(date.day) ||
    date.month < 1 ||
    date.month > 12 ||
    date.day < 1 ||
    date.day > 31
  ) {
    return false;
  }
  const utc = new Date(Date.UTC(date.year, date.month - 1, date.day));
  return (
    utc.getUTCFullYear() === date.year &&
    utc.getUTCMonth() === date.month - 1 &&
    utc.getUTCDate() === date.day
  );
}

/** Convert a Gregorian date to the Chinese lunar calendar (1900-2100). */
export function solarToLunar(year: number, month: number, day: number): LunarDate | null {
  const solar = { year, month, day };
  if (!validSolarDate(solar)) return null;

  let offset = Math.round((Date.UTC(year, month - 1, day) - LUNAR_EPOCH) / DAY_MS);
  if (offset < 0) return null;

  let lunarYear = MIN_LUNAR_YEAR;
  while (lunarYear <= MAX_LUNAR_YEAR) {
    const yearDays = lunarYearDays(lunarYear);
    if (offset < yearDays) break;
    offset -= yearDays;
    lunarYear += 1;
  }
  if (lunarYear > MAX_LUNAR_YEAR) return null;

  const leap = leapMonth(lunarYear);
  for (let lunarMonth = 1; lunarMonth <= 12; lunarMonth += 1) {
    const days = monthDays(lunarYear, lunarMonth);
    if (offset < days) {
      return { year: lunarYear, month: lunarMonth, day: offset + 1, isLeapMonth: false };
    }
    offset -= days;

    if (lunarMonth === leap) {
      const daysInLeapMonth = leapDays(lunarYear);
      if (offset < daysInLeapMonth) {
        return { year: lunarYear, month: lunarMonth, day: offset + 1, isLeapMonth: true };
      }
      offset -= daysInLeapMonth;
    }
  }

  return null;
}

export function lunarMonthName(lunar: LunarDate): string {
  const name = LUNAR_MONTHS[lunar.month] ?? "";
  return lunar.isLeapMonth ? `闰${name}` : name;
}

export function lunarDayName(lunar: LunarDate): string {
  return LUNAR_DAYS[lunar.day] ?? "";
}

export function lunarZodiacYear(lunar: LunarDate): string {
  const index = (((lunar.year - 4) % 60) + 60) % 60;
  return `${HEAVENLY_STEMS[index % 10]}${EARTHLY_BRANCHES[index % 12]}${ZODIAC[index % 12]}年`;
}

export function lunarDateLabel(lunar: LunarDate): string {
  return `${lunarMonthName(lunar)}${lunarDayName(lunar)}`;
}

export function lunarFestivalFor(lunar: LunarDate, locale = "zh"): string | null {
  if (lunar.isLeapMonth) return null;
  return (
    LOCALIZED_LUNAR_FESTIVALS[normalizeLunarLocale(locale)][`${lunar.month}-${lunar.day}`] ?? null
  );
}

function weekFestivalFor(solar: SolarDate, locale: LunarLocale): string | null {
  const weekday = new Date(Date.UTC(solar.year, solar.month - 1, solar.day)).getUTCDay();
  const occurrence = Math.ceil(solar.day / 7);
  return (
    WEEK_FESTIVALS.find(
      (festival) =>
        festival.month === solar.month &&
        festival.occurrence === occurrence &&
        festival.weekday === weekday,
    )?.names[locale] ?? null
  );
}

export function solarFestivalFor(solar: SolarDate, locale = "zh"): string | null {
  const normalized = normalizeLunarLocale(locale);
  return (
    LOCALIZED_SOLAR_FESTIVALS[normalized][`${solar.month}-${solar.day}`] ??
    weekFestivalFor(solar, normalized)
  );
}

/** Return the most relevant festival, preferring lunar festivals. */
export function festivalFor(lunar: LunarDate, solar: SolarDate, locale = "zh"): string | null {
  return lunarFestivalFor(lunar, locale) ?? solarFestivalFor(solar, locale);
}

function compactLunarDate(lunar: LunarDate, locale: LunarLocale): string {
  const normalized = normalizeLunarLocale(locale);
  if (normalized === "zh") {
    return lunar.day === 1 ? lunarMonthName(lunar) : lunarDayName(lunar);
  }

  const leap = lunar.isLeapMonth;
  switch (normalized) {
    case "ja":
      return `${leap ? "閏" : ""}${lunar.month}月${lunar.day}日`;
    case "ko":
      return `음력${leap ? " 윤" : " "}${lunar.month}.${lunar.day}`;
    case "de":
      return `Mond${leap ? " S" : ""} ${lunar.month}/${lunar.day}`;
    case "fr":
      return `Lunaire${leap ? " bis" : ""} ${lunar.month}/${lunar.day}`;
    case "ru":
      return `Луна${leap ? " висок." : ""} ${lunar.month}/${lunar.day}`;
    default:
      return `Lunar${leap ? " leap" : ""} ${lunar.month}/${lunar.day}`;
  }
}

export interface CalendarAnnotation {
  label: string;
  emphasized: boolean;
}

/**
 * The compact label under a Gregorian day in the month grid. Solar festivals,
 * lunar festivals, and solar terms are combined in that order.
 */
export function calendarAnnotation(
  lunar: LunarDate,
  solar: SolarDate,
  locale = "zh",
): CalendarAnnotation {
  const normalized = normalizeLunarLocale(locale);
  const labels = [
    solarFestivalFor(solar, normalized),
    lunarFestivalFor(lunar, normalized),
    solarTermFor(solar, normalized),
  ].filter((label): label is string => Boolean(label));
  if (labels.length > 0) {
    return { label: [...new Set(labels)].join("/"), emphasized: true };
  }
  return { label: compactLunarDate(lunar, normalized), emphasized: false };
}

export function lunarCellLabel(lunar: LunarDate, solar: SolarDate, locale = "zh"): string {
  return calendarAnnotation(lunar, solar, locale).label;
}
