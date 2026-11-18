export type CalendarLocale = "en" | "zh" | "ja" | "ko" | "de" | "fr" | "ru";

export interface CalendarCopy {
  locale: CalendarLocale;
  dateSeparator: string;
  today: string;
  previousYear: string;
  previous: string;
  next: string;
  nextYear: string;
  unavailable: string;
  holidayBadge: string;
  workdayBadge: string;
  week: string;
  weekdays: string[];
  months: string[];
  weekLabel: (week: number) => string;
  daysBefore: (days: number) => string;
  daysAfter: (days: number) => string;
}

function englishDayCount(days: number): string {
  return `${days} ${days === 1 ? "day" : "days"}`;
}

function germanDayCount(days: number): string {
  return `${days} ${days === 1 ? "Tag" : "Tagen"}`;
}

function frenchDayCount(days: number): string {
  return `${days} ${days === 1 ? "jour" : "jours"}`;
}

function russianDayCount(days: number): string {
  const lastTwo = days % 100;
  const last = days % 10;
  const word =
    lastTwo >= 11 && lastTwo <= 14
      ? "дней"
      : last === 1
        ? "день"
        : last >= 2 && last <= 4
          ? "дня"
          : "дней";
  return `${days} ${word}`;
}

export function normalizeCalendarLocale(locale: string): CalendarLocale {
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

export function calendarCopyFor(locale: string): CalendarCopy {
  switch (normalizeCalendarLocale(locale)) {
    case "zh":
      return {
        locale: "zh",
        dateSeparator: "，",
        today: "今天",
        previousYear: "上一年",
        previous: "上个月",
        next: "下个月",
        nextYear: "下一年",
        unavailable: "农历数据不可用",
        holidayBadge: "休",
        workdayBadge: "班",
        week: "周",
        weekdays: ["一", "二", "三", "四", "五", "六", "日"],
        months: [
          "1月",
          "2月",
          "3月",
          "4月",
          "5月",
          "6月",
          "7月",
          "8月",
          "9月",
          "10月",
          "11月",
          "12月",
        ],
        weekLabel: (week) => `第${week}周`,
        daysBefore: (days) => `${days}天前`,
        daysAfter: (days) => `${days}天后`,
      };
    case "ja":
      return {
        locale: "ja",
        dateSeparator: "、",
        today: "今日",
        previousYear: "前年",
        previous: "先月",
        next: "翌月",
        nextYear: "翌年",
        unavailable: "旧暦データなし",
        holidayBadge: "休",
        workdayBadge: "出",
        week: "週",
        weekdays: ["月", "火", "水", "木", "金", "土", "日"],
        months: [
          "1月",
          "2月",
          "3月",
          "4月",
          "5月",
          "6月",
          "7月",
          "8月",
          "9月",
          "10月",
          "11月",
          "12月",
        ],
        weekLabel: (week) => `第${week}週`,
        daysBefore: (days) => `${days}日前`,
        daysAfter: (days) => `${days}日後`,
      };
    case "ko":
      return {
        locale: "ko",
        dateSeparator: ", ",
        today: "오늘",
        previousYear: "이전 해",
        previous: "이전 달",
        next: "다음 달",
        nextYear: "다음 해",
        unavailable: "음력 데이터 없음",
        holidayBadge: "휴",
        workdayBadge: "근",
        week: "주",
        weekdays: ["월", "화", "수", "목", "금", "토", "일"],
        months: [
          "1월",
          "2월",
          "3월",
          "4월",
          "5월",
          "6월",
          "7월",
          "8월",
          "9월",
          "10월",
          "11월",
          "12월",
        ],
        weekLabel: (week) => `${week}주`,
        daysBefore: (days) => `${days}일 전`,
        daysAfter: (days) => `${days}일 후`,
      };
    case "de":
      return {
        locale: "de",
        dateSeparator: ", ",
        today: "Heute",
        previousYear: "Vorheriges Jahr",
        previous: "Vorheriger Monat",
        next: "Nächster Monat",
        nextYear: "Nächstes Jahr",
        unavailable: "Lunardaten nicht verfügbar",
        holidayBadge: "Frei",
        workdayBadge: "Arb.",
        week: "KW",
        weekdays: ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"],
        months: [
          "Januar",
          "Februar",
          "März",
          "April",
          "Mai",
          "Juni",
          "Juli",
          "August",
          "September",
          "Oktober",
          "November",
          "Dezember",
        ],
        weekLabel: (week) => `KW ${week}`,
        daysBefore: (days) => `vor ${germanDayCount(days)}`,
        daysAfter: (days) => `in ${germanDayCount(days)}`,
      };
    case "fr":
      return {
        locale: "fr",
        dateSeparator: ", ",
        today: "Aujourd’hui",
        previousYear: "Année précédente",
        previous: "Mois précédent",
        next: "Mois suivant",
        nextYear: "Année suivante",
        unavailable: "Données lunaires indisponibles",
        holidayBadge: "Repos",
        workdayBadge: "Trav.",
        week: "Sem.",
        weekdays: ["Lun", "Mar", "Mer", "Jeu", "Ven", "Sam", "Dim"],
        months: [
          "janvier",
          "février",
          "mars",
          "avril",
          "mai",
          "juin",
          "juillet",
          "août",
          "septembre",
          "octobre",
          "novembre",
          "décembre",
        ],
        weekLabel: (week) => `Sem. ${week}`,
        daysBefore: (days) => `il y a ${frenchDayCount(days)}`,
        daysAfter: (days) => `dans ${frenchDayCount(days)}`,
      };
    case "ru":
      return {
        locale: "ru",
        dateSeparator: ", ",
        today: "Сегодня",
        previousYear: "Предыдущий год",
        previous: "Предыдущий месяц",
        next: "Следующий месяц",
        nextYear: "Следующий год",
        unavailable: "Лунные данные недоступны",
        holidayBadge: "Вых.",
        workdayBadge: "Раб.",
        week: "Нед.",
        weekdays: ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"],
        months: [
          "январь",
          "февраль",
          "март",
          "апрель",
          "май",
          "июнь",
          "июль",
          "август",
          "сентябрь",
          "октябрь",
          "ноябрь",
          "декабрь",
        ],
        weekLabel: (week) => `Нед. ${week}`,
        daysBefore: (days) => `${russianDayCount(days)} назад`,
        daysAfter: (days) => `через ${russianDayCount(days)}`,
      };
    default:
      return {
        locale: "en",
        dateSeparator: ", ",
        today: "Today",
        previousYear: "Previous year",
        previous: "Previous month",
        next: "Next month",
        nextYear: "Next year",
        unavailable: "Lunar data unavailable",
        holidayBadge: "Off",
        workdayBadge: "Work",
        week: "Wk",
        weekdays: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
        months: [
          "January",
          "February",
          "March",
          "April",
          "May",
          "June",
          "July",
          "August",
          "September",
          "October",
          "November",
          "December",
        ],
        weekLabel: (week) => `Week ${week}`,
        daysBefore: (days) => `${englishDayCount(days)} ago`,
        daysAfter: (days) => `in ${englishDayCount(days)}`,
      };
  }
}

export function monthTitleFor(year: number, month: number, copy: CalendarCopy): string {
  if (copy.locale === "zh" || copy.locale === "ja") return `${year}年${month}月`;
  if (copy.locale === "ko") return `${year}년 ${month}월`;
  return `${copy.months[month - 1]} ${year}`;
}

export function relativeDaysLabel(days: number, copy: CalendarCopy): string {
  if (days === 0) return "";
  return days < 0 ? copy.daysBefore(-days) : copy.daysAfter(days);
}
