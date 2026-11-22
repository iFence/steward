import { expect, open, test } from "nsh/test";

test("secret notes consumes every plugin-private secret operation", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.secrets.set": {},
        "steward.secrets.get": "stored-value",
        "steward.secrets.delete": { deleted: true },
        "steward.secrets.list": ["api-key"],
      },
    },
  });

  await app.type("id:secret-key", "api-key");
  await app.type("id:secret-value", "stored-value");
  await app.press("text:Save");
  expect(app.texts()).toContain("Saved api-key.");
  expect(app.texts()).toContain("Saved keys: api-key");
  expect(app.texts().join("\n")).not.toContain("stored-value");

  await app.press("text:Load");
  expect(app.texts()).toContain("Found api-key.");
  await app.press("text:Delete");
  expect(app.texts()).toContain("Deleted api-key.");

  expect(app.host.calls().map((call) => call.name)).toEqual([
    "steward.secrets.set",
    "steward.secrets.list",
    "steward.secrets.get",
    "steward.secrets.delete",
    "steward.secrets.list",
  ]);
});
