import { expect, open, test } from "nsh/test";

const params = {
  accent: "#176B87",
  command: "local-files",
  dark: true,
  input: "",
  locale: "en-US",
  pluginId: "it.aspirin.local-files",
  preferences: {},
};

test("local file inspector consumes scoped filesystem and system APIs", async () => {
  const app = await open("./App.tsx", {
    host: {
      params,
      handlers: {
        "steward.fs.pick": (data: { mode: string }) => ({
          path: data.mode === "fileRead" ? "D:\\projects\\README.md" : "D:\\projects",
        }),
        "steward.fs.walk": {
          root: "D:\\projects",
          entries: [
            {
              path: "D:\\projects\\README.md",
              relativePath: "README.md",
              directory: false,
              reparsePoint: false,
              size: 12,
            },
          ],
          truncated: false,
          elapsedMs: 3,
        },
        "steward.fs.readText": "# Project\n\nScoped file preview.",
        "steward.system.revealPath": {},
        "steward.system.openPath": {},
        "steward.system.applications": [{ id: "editor.exe", name: "Editor" }],
        "steward.system.openWith": {},
      },
    },
  });

  await app.press("text:Choose folder");
  expect(app.texts()).toContain("1 entries in 3 ms");
  expect(app.texts()).toContain("[file] README.md");
  await app.press("text:Choose file");
  expect(app.texts()).toContain("Read 31 characters from D:\\projects\\README.md");
  await app.press("text:Load applications");
  expect(app.texts()).toContain("Found 1 installed applications.");
  await app.press("text:Open first with app");
  await app.press("text:Open first entry");
  await app.press("text:Reveal first entry");
  expect(app.host.calls().map((call) => call.name)).toEqual([
    "steward.fs.pick",
    "steward.fs.walk",
    "steward.fs.pick",
    "steward.fs.readText",
    "steward.system.applications",
    "steward.system.openWith",
    "steward.system.openPath",
    "steward.system.revealPath",
  ]);
});
