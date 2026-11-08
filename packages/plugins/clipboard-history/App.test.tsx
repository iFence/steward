import { expect, open, test } from "nsh/test";

test("clipboard history renders its title", async () => {
  const app = await open("./App.tsx");
  expect(app.texts()).toContain("Clipboard History");
});
