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
      };
  }
}

export function monthTitleFor(year: number, month: number, copy: CalendarCopy): string {
  if (copy.locale === "zh" || copy.locale === "ja") return `${year}年${month}月`;
  if (copy.locale === "ko") return `${year}년 ${month}월`;
  return `${copy.months[month - 1]} ${year}`;
}
