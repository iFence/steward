import { expect, open, test } from "nsh/test";

test("showcase increments", async () => {
  const app = await open("./App.tsx");
  await app.press("Increment");
  expect(app.texts()).toContain("Clicks: 1");
});
