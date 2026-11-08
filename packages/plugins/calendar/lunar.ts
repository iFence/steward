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
  "3-8": "妇女节",
  "5-1": "劳动节",
  "5-4": "青年节",
  "6-1": "儿童节",
  "9-10": "教师节",
  "10-1": "国庆节",
  "12-24": "平安夜",
  "12-25": "圣诞节",
};

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

/** Return the most relevant festival, preferring lunar festivals. */
export function festivalFor(lunar: LunarDate, solar: SolarDate): string | null {
  if (!lunar.isLeapMonth) {
    const lunarFestival = LUNAR_FESTIVALS[`${lunar.month}-${lunar.day}`];
    if (lunarFestival) return lunarFestival;
  }
  return SOLAR_FESTIVALS[`${solar.month}-${solar.day}`] ?? null;
}

/**
 * The compact label under a Gregorian day in the month grid. Festivals take
 * precedence, followed by lunar month starts and ordinary lunar day names.
 */
export function lunarCellLabel(lunar: LunarDate, solar: SolarDate): string {
  return (
    festivalFor(lunar, solar) ?? (lunar.day === 1 ? lunarMonthName(lunar) : lunarDayName(lunar))
  );
}
