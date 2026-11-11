import { expect, open, test } from "nsh/test";
import { festivalFor, lunarCellLabel, solarToLunar } from "./lunar";

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
});

test("calendar renders solar and lunar dates and copies a selected day", async () => {
  const app = await open("./App.tsx", {
    host: {
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

  await app.press("id:calendar-day-2024-02-10");
  expect(app.texts().join(" ")).toContain("农历 甲辰龙年 正月初一");
  expect(app.clipboard()).toBe("2024-02-10");

  await app.press("id:calendar-next-month");
  expect(app.texts().join(" ")).toContain("2024年3月");

  await app.host.send("steward.theme", { accent: "#26a69a", dark: false });
  expect(app.spanStyle("id:calendar-month-title >> text:2024年3月").color).toContain("#202024");
});
