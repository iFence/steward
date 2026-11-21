import { expect, open, test } from "nsh/test";

const params = {
  accent: "#F05133",
  command: "git-repos",
  dark: true,
  input: "",
  locale: "en-US",
  pluginId: "it.aspirin.git-repos",
  preferences: { scanDepth: "24" },
};

test("git repositories discovers a repository with the generic walk API", async () => {
  const app = await open("./App.tsx", {
    host: {
      params,
      handlers: {
        "steward.fs.pick": () => ({ path: "D:\\projects" }),
        "steward.fs.walk": {
          root: "D:\\projects",
          entries: [
            {
              path: "D:\\projects\\steward\\.git",
              relativePath: "steward\\.git",
              directory: true,
              reparsePoint: false,
              size: 0,
            },
          ],
          truncated: false,
          elapsedMs: 4,
        },
        "steward.storage.get": () => null,
      },
    },
  });

  await app.press("text:Choose folder");
  expect(app.texts()).toContain("D:\\projects\\steward");
  expect(app.host.calls().map((call) => call.name)).toEqual([
    "steward.fs.pick",
    "steward.fs.walk",
    "steward.storage.get",
  ]);
});
