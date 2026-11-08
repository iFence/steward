import { expect, open, test } from "nsh/test";

test("calendar renders the current month", async () => {
  const app = await open("./App.tsx");
  expect(app.texts().join(" ")).toContain(String(new Date().getFullYear()));
});
