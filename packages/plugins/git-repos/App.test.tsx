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

test("git repositories uses the controlled process API for repository state", async () => {
  const app = await open("./App.tsx", {
    host: {
      params,
      handlers: {
        "steward.fs.pick": (data: { mode: string }) => ({
          path: data.mode === "folderWrite" ? "D:\\projects" : "D:\\projects\\steward",
        }),
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
          elapsedMs: 2,
        },
        "steward.fs.readText": '[remote "origin"]\n  url = https://example.com/acme/steward.git\n',
        "steward.storage.get": () => null,
        "steward.exec.run": (data: { arguments: string[] }) => {
          const command = data.arguments[0];
          return {
            exitCode: 0,
            stdout:
              command === "status"
                ? "## main...origin/main\n"
                : command === "remote"
                  ? "origin https://example.com/acme/steward.git (fetch)\n"
                  : command === "rev-parse"
                    ? "origin/main\n"
                    : command === "rev-list"
                      ? "2\n"
                      : "",
            stderr: "",
            durationMs: 8,
            timedOut: false,
            cancelled: false,
          };
        },
      },
    },
  });

  await app.press("text:Choose folder");
  await app.press("text:D:\\projects\\steward");
  await app.press("text:Git state");

  expect(app.texts()).toContain("Clean; 2 unpushed commits; remotes loaded");
  await app.press("text:Clone");
  expect(app.texts()).toContain(
    "Cloned https://example.com/acme/steward.git into D:\\projects\\steward",
  );
  expect(app.host.calls().filter((call) => call.name === "steward.exec.run")).toHaveLength(5);
});
