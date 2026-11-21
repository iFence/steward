import { expect, open, test } from "nsh/test";
import { calendarCopyFor, monthTitleFor, relativeDaysLabel } from "./calendar-i18n";
import { holidayScheduleFromApi, officialDayKindFor, solarTermFor } from "./calendar-meta";
import { festivalFor, lunarCellLabel, solarToLunar } from "./lunar";

const TEST_HOLIDAYS = {
  "2024-02-04": "workday",
  "2024-02-10": "holiday",
  "2024-04-04": "holiday",
  "2024-04-07": "workday",
} as const;

function holidayHandler(data: unknown): Record<string, string> {
  const year = (data as { year: number }).year;
  return Object.fromEntries(
    Object.entries(TEST_HOLIDAYS).filter(([date]) => date.startsWith(`${year}-`)),
  );
}

test("calendar supplies copy for every Steward UI language", () => {
  const today = [
    ["en-US", "Today"],
    ["zh-CN", "今天"],
    ["ja-JP", "今日"],
    ["ko-KR", "오늘"],
    ["de-DE", "Heute"],
    ["fr-FR", "Aujourd’hui"],
    ["ru-RU", "Сегодня"],
  ] as const;

  for (const [locale, expected] of today) {
    expect(calendarCopyFor(locale).today).toBe(expected);
  }

  expect(calendarCopyFor("zh-CN").back).toBe("返回");
  expect(calendarCopyFor("de-DE").weekdays[0]).toBe("Mo");
  expect(monthTitleFor(2024, 2, calendarCopyFor("fr-FR")).toLowerCase()).toContain("février");
  expect(relativeDaysLabel(-3, calendarCopyFor("zh-CN"))).toBe("3天前");
  expect(relativeDaysLabel(2, calendarCopyFor("zh-CN"))).toBe("2天后");
  expect(relativeDaysLabel(0, calendarCopyFor("zh-CN"))).toBe("");
});

test("calendar converts known solar dates to the Chinese lunar calendar", () => {
  const newYear = solarToLunar(2024, 2, 10);
  if (!newYear) throw new Error("2024-02-10 should convert");
  expect(newYear.year).toBe(2024);
  expect(newYear.month).toBe(1);
  expect(newYear.day).toBe(1);
  expect(newYear.isLeapMonth).toBe(false);
  expect(festivalFor(newYear, { year: 2024, month: 2, day: 10 })).toBe("春节");

  const leapMonth = solarToLunar(2020, 5, 23);
  if (!leapMonth) throw new Error("2020-05-23 should convert");
  expect(leapMonth.month).toBe(4);
  expect(leapMonth.day).toBe(1);
  expect(leapMonth.isLeapMonth).toBe(true);

  const midAutumn = solarToLunar(2024, 9, 17);
  if (!midAutumn) throw new Error("2024-09-17 should convert");
  expect(midAutumn.month).toBe(8);
  expect(midAutumn.day).toBe(15);
  expect(lunarCellLabel(midAutumn, { year: 2024, month: 9, day: 17 })).toBe("中秋节");
  expect(lunarCellLabel(newYear, { year: 2024, month: 2, day: 10 }, "en")).toBe("New Year");
  expect(lunarCellLabel(midAutumn, { year: 2024, month: 9, day: 17 }, "ja")).toBe("中秋節");
});

test("calendar resolves solar terms and official day status", () => {
  expect(solarTermFor({ year: 2024, month: 4, day: 4 }, "zh")).toBe("清明");
  expect(solarTermFor({ year: 2024, month: 8, day: 22 }, "zh")).toBe("处暑");
  expect(solarTermFor({ year: 2024, month: 4, day: 4 }, "en")).toBe("Qingming");
  expect(officialDayKindFor({ year: 2024, month: 2, day: 10 }, TEST_HOLIDAYS)).toBe("holiday");
  expect(officialDayKindFor({ year: 2024, month: 2, day: 4 }, TEST_HOLIDAYS)).toBe("workday");
  expect(officialDayKindFor({ year: 2024, month: 2, day: 5 }, TEST_HOLIDAYS)).toBeNull();
});

test("calendar parses official holiday API responses", () => {
  expect(
    holidayScheduleFromApi({
      code: 0,
      holiday: {
        "02-10": { holiday: true, date: "2024-02-10" },
        "02-04": { holiday: false, date: "2024-02-04" },
      },
    }),
  ).toEqual({
    "2024-02-04": "workday",
    "2024-02-10": "holiday",
  });
});

test("calendar reuses a cached yearly holiday schedule", async () => {
  const cacheKey = "calendar.holiday.v2.2024";
  const cached = {
    fetchedAt: Date.now(),
    schedule: TEST_HOLIDAYS,
  };
  const app = await open("./App.tsx", {
    storage: {
      [cacheKey]: JSON.stringify(cached),
    },
    host: {
      handlers: {
        "steward.clipboard.write": {},
        "steward.calendar.holidays": () => {
          throw new Error("the cached year must not be requested");
        },
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-02",
        locale: "zh-CN",
        pluginId: "it.aspirin.calendar",
      },
    },
  });

  await app.waitFor("id:calendar-badge-2024-02-10");
  expect(app.host.calls().filter((call) => call.name === "steward.calendar.holidays").length).toBe(
    0,
  );
  expect(app.storage()[cacheKey]).toBe(JSON.stringify(cached));
});

test("calendar keeps stale holiday data available when refresh fails", async () => {
  const cacheKey = "calendar.holiday.v2.2024";
  const stale = {
    fetchedAt: 0,
    schedule: TEST_HOLIDAYS,
  };
  const app = await open("./App.tsx", {
    storage: {
      [cacheKey]: JSON.stringify(stale),
    },
    host: {
      handlers: {
        "steward.calendar.holidays": () => {
          throw new Error("offline");
        },
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-02",
        locale: "zh-CN",
        pluginId: "it.aspirin.calendar",
      },
    },
  });

  await app.waitFor("id:calendar-badge-2024-02-10");
  expect(app.shows("id:calendar-badge-2024-02-10")).toBe(true);
  expect(app.storage()[cacheKey]).toBe(JSON.stringify(stale));
});

test("calendar silently hides official badges when holiday data is unavailable", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.clipboard.write": {},
        "steward.calendar.holidays": () => {
          throw new Error("offline");
        },
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-02",
        locale: "zh-CN",
        pluginId: "it.aspirin.calendar",
      },
    },
  });

  expect(app.shows("id:calendar-root")).toBe(true);
  expect(app.shows("id:calendar-badge-2024-02-10")).toBe(false);
});

test("calendar retries holiday data when the embedded page becomes visible", async () => {
  let attempts = 0;
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.calendar.holidays": (data: unknown) => {
          attempts += 1;
          if (attempts === 1) throw new Error("offline");
          return holidayHandler(data);
        },
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-02",
        locale: "zh-CN",
        pluginId: "it.aspirin.calendar",
        presentation: "embedded",
      },
    },
  });

  await app.host.send("steward.visibility", { visible: false });
  await app.host.send("steward.visibility", { visible: true });
  await app.waitFor("id:calendar-badge-2024-02-10");

  expect(attempts).toBe(2);
  expect(app.shows("id:calendar-badge-2024-02-10")).toBe(true);
});

test("calendar fetches holiday data in the standalone development host", async () => {
  const app = await open("./App.tsx", {
    network: {
      "GET https://timor.tech/api/holiday/year/2024": {
        body: {
          code: 0,
          holiday: {
            "02-10": { holiday: true, date: "2024-02-10" },
            "02-04": { holiday: false, date: "2024-02-04" },
          },
        },
      },
    },
    host: {
      handlers: {
        "steward.clipboard.write": {},
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-02",
        locale: "zh-CN",
        pluginId: "",
      },
    },
  });

  await app.waitFor("id:calendar-badge-2024-02-10");
  await app.press("id:calendar-next-month");
  expect(
    app.requests().filter((request) => request.url === "https://timor.tech/api/holiday/year/2024")
      .length,
  ).toBe(1);
  expect(app.storage()["calendar.holiday.v2.2024"]).toContain("2024-02-10");
});

test("calendar honors hosted week start and week-number preferences", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.calendar.holidays": holidayHandler,
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-02",
        locale: "en-US",
        pluginId: "it.aspirin.calendar",
        preferences: {
          showWeekNumbers: false,
          weekStartsOn: "sunday",
        },
      },
    },
  });

  expect(app.shows("id:calendar-day-2024-01-28")).toBe(true);
  expect(app.shows("id:calendar-day-2024-02-01")).toBe(true);
  expect(app.shows("id:calendar-day-2024-03-09")).toBe(true);
  expect(app.shows("id:calendar-week-header")).toBe(false);
  expect(app.shows("id:calendar-week-05")).toBe(false);
});

test("calendar uses embedded navigation and keeps the page title in sync", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.calendar.holidays": holidayHandler,
        "steward.navigation.back": {},
        "steward.navigation.setTitle": {},
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-02",
        locale: "zh-CN",
        pluginId: "it.aspirin.calendar",
        presentation: "embedded",
      },
    },
  });

  await app.press("id:calendar-next-month");
  const titleCalls = app.host.calls().filter((call) => call.name === "steward.navigation.setTitle");
  expect(titleCalls[titleCalls.length - 1]?.data).toEqual({ title: "2024年3月" });

  await app.press("id:calendar-back");
  expect(app.host.calls().filter((call) => call.name === "steward.navigation.back").length).toBe(1);
});

test("calendar renders solar and lunar dates and selects a day without copying", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.clipboard.write": {},
        "steward.calendar.holidays": holidayHandler,
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-02",
        locale: "zh-CN",
        pluginId: "it.aspirin.calendar",
      },
    },
  });

  const initialText = app.texts().join(" ");
  expect(initialText).toContain("2024年2月");
  expect(initialText).toContain("春节");
  await app.waitFor("id:calendar-badge-2024-02-04");
  await app.waitFor("id:calendar-badge-2024-02-10");
  expect(app.shows("id:calendar-badge-2024-02-04")).toBe(true);
  expect(app.shows("id:calendar-badge-2024-02-10")).toBe(true);
  expect(initialText).not.toContain("公历与农历");
  expect(initialText).not.toContain("选中日期");
  expect(initialText).not.toContain("点击日期可复制公历");
  expect(initialText).not.toContain("1900–2100");
  expect(app.spanStyle("id:calendar-week-05 >> text:05").color).toContain("#94949c");
  expect(app.spanStyle("id:calendar-week-10 >> text:10").color).toContain("#94949c");

  await app.press("id:calendar-day-2024-02-10");
  expect(app.texts().join(" ")).not.toContain("农历 甲辰龙年 正月初一");
  expect(app.spanStyle("id:calendar-day-2024-02-10 >> text:10").color).toContain("#ff6d00");
  expect(app.host.calls().filter((call) => call.name === "steward.clipboard.write")).toEqual([]);

  await app.press("id:calendar-next-month");
  expect(app.texts().join(" ")).toContain("2024年3月");
  expect(app.shows("id:calendar-day-2024-02-10")).toBe(false);
  expect(app.shows("id:calendar-day-2024-03-01")).toBe(true);

  await app.press("id:calendar-next-year");
  expect(app.texts().join(" ")).toContain("2025年3月");
  expect(app.shows("id:calendar-day-2024-03-15")).toBe(false);
  expect(app.shows("id:calendar-day-2025-03-15")).toBe(true);

  await app.press("id:calendar-prev-year");
  expect(app.texts().join(" ")).toContain("2024年3月");
  expect(app.shows("id:calendar-day-2025-03-15")).toBe(false);
  expect(app.shows("id:calendar-day-2024-03-15")).toBe(true);

  await app.host.send("steward.theme", { accent: "#26a69a", dark: false });
  expect(app.spanStyle("id:calendar-month-title >> text:2024年3月").color).toContain("#202024");
});

test("calendar renders a solar term in the month grid", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.clipboard.write": {},
        "steward.calendar.holidays": holidayHandler,
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-04",
        locale: "zh-CN",
        pluginId: "it.aspirin.calendar",
      },
    },
  });

  expect(app.texts().join(" ")).toContain("2024年4月");
  expect(app.texts().join(" ")).toContain("清明");
  await app.waitFor("id:calendar-badge-2024-04-04");
  await app.waitFor("id:calendar-badge-2024-04-07");
  expect(app.shows("id:calendar-badge-2024-04-04")).toBe(true);
  expect(app.shows("id:calendar-badge-2024-04-07")).toBe(true);
});

test("calendar distinguishes the selected day from today", async () => {
  const current = new Date();
  const year = current.getFullYear();
  const month = current.getMonth() + 1;
  const selectedDay = current.getDate() === 1 ? 2 : 1;
  const pad = (value: number) => String(value).padStart(2, "0");
  const todayIso = `${year}-${pad(month)}-${pad(current.getDate())}`;
  const selectedIso = `${year}-${pad(month)}-${pad(selectedDay)}`;
  const relativeLabel = (day: number) => {
    const dayValue = (value: number) => Math.floor(Date.UTC(year, month - 1, value) / 86_400_000);
    const difference = dayValue(day) - dayValue(current.getDate());
    return difference < 0 ? `${-difference}天前` : difference > 0 ? `${difference}天后` : "";
  };
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.clipboard.write": {},
        "steward.calendar.holidays": holidayHandler,
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: `calendar ${year}-${month}`,
        locale: "zh-CN",
        pluginId: "it.aspirin.calendar",
      },
    },
  });

  expect(app.shows("id:calendar-relative-day")).toBe(false);
  await app.press(`id:calendar-day-${selectedIso}`);
  expect(app.spanStyle(`id:calendar-day-${todayIso} >> text:${current.getDate()}`).color).toContain(
    "#ffffff",
  );
  expect(app.spanStyle(`id:calendar-day-${selectedIso} >> text:${selectedDay}`).color).toContain(
    "#ff6d00",
  );
  expect(app.shows("id:calendar-relative-day")).toBe(true);
  expect(app.texts()).toContain(relativeLabel(selectedDay));

  await app.keyboard.press("right");
  expect(
    app.spanStyle(`id:calendar-day-${selectedIso} >> text:${selectedDay}`).color,
  ).not.toContain("#ff6d00");
  expect(
    app.spanStyle(
      `id:calendar-day-${year}-${pad(month)}-${pad(selectedDay + 1)} >> text:${selectedDay + 1}`,
    ).color,
  ).toContain("#ff6d00");
  expect(app.texts()).toContain(relativeLabel(selectedDay + 1));

  await app.keyboard.press("down");
  expect(
    app.spanStyle(
      `id:calendar-day-${year}-${pad(month)}-${pad(selectedDay + 8)} >> text:${selectedDay + 8}`,
    ).color,
  ).toContain("#ff6d00");
  expect(app.texts()).toContain(relativeLabel(selectedDay + 8));

  await app.keyboard.press("left");
  await app.keyboard.press("up");
  expect(app.spanStyle(`id:calendar-day-${selectedIso} >> text:${selectedDay}`).color).toContain(
    "#ff6d00",
  );
  expect(app.texts()).toContain(relativeLabel(selectedDay));
  expect(app.host.calls().filter((call) => call.name === "steward.clipboard.write")).toEqual([]);
});

test("calendar arrow keys move the selected date across months", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.calendar.holidays": holidayHandler,
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-02",
        locale: "zh-CN",
        pluginId: "it.aspirin.calendar",
      },
    },
  });

  await app.press("id:calendar-day-2024-02-29");
  await app.keyboard.press("right");
  expect(app.texts().join(" ")).toContain("2024年3月");
  expect(app.spanStyle("id:calendar-day-2024-03-01 >> text:1").color).toContain("#ff6d00");

  await app.keyboard.press("left");
  expect(app.texts().join(" ")).toContain("2024年2月");
  expect(app.spanStyle("id:calendar-day-2024-02-29 >> text:29").color).toContain("#ff6d00");

  await app.keyboard.press("down");
  expect(app.texts().join(" ")).toContain("2024年3月");
  expect(app.spanStyle("id:calendar-day-2024-03-07 >> text:7").color).toContain("#ff6d00");
});

test("calendar fills leading and trailing days from adjacent months", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.clipboard.write": {},
        "steward.calendar.holidays": holidayHandler,
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-02",
        locale: "zh-CN",
        pluginId: "it.aspirin.calendar",
      },
    },
  });

  expect(app.texts().join(" ")).toContain("2024年2月");
  expect(app.spanStyle("id:calendar-day-2024-01-31 >> text:31").color).toContain("#94949c");
  expect(app.spanStyle("id:calendar-day-2024-02-01 >> text:1").color).toContain("#ff6d00");
  expect(app.spanStyle("id:calendar-day-2024-03-10 >> text:10").color).toContain("#94949c");

  await app.press("id:calendar-day-2024-03-01");
  expect(app.texts().join(" ")).toContain("2024年3月");
  expect(app.spanStyle("id:calendar-day-2024-03-01 >> text:1").color).toContain("#ff6d00");
  expect(app.host.calls().filter((call) => call.name === "steward.clipboard.write")).toEqual([]);
});

test("calendar fits its minimum viewport without scrolling", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.clipboard.write": {},
        "steward.calendar.holidays": holidayHandler,
      },
      params: {
        accent: "#ff6d00",
        command: "calendar",
        dark: true,
        input: "calendar 2024-02",
        locale: "zh-CN",
        pluginId: "it.aspirin.calendar",
      },
    },
  });

  await app.setViewport(720, 600);

  expect(app.shows("id:calendar-root")).toBe(true);
  expect(app.shows("id:calendar-day-2024-02-01")).toBe(true);
  expect(app.shows("id:calendar-day-2024-03-10")).toBe(true);
});
