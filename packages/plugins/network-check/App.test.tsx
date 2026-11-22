import { expect, open, test } from "nsh/test";

test("network check consumes the scoped HTTPS API", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.secrets.get": null,
        "steward.net.fetch": {
          status: 200,
          url: "https://api.github.com/rate_limit",
          headers: [{ name: "content-type", value: "application/json" }],
          body: JSON.stringify({ rate: { remaining: 59, limit: 60 } }),
          redirects: 0,
          elapsedMs: 12,
        },
      },
    },
  });

  await app.press("text:Run HTTPS check");

  expect(app.texts()).toContain("HTTPS 200 in 12 ms");
  expect(app.texts()).toContain("GitHub API requests remaining: 59 / 60");
  expect(app.host.calls().map((call) => call.name)).toEqual([
    "steward.secrets.get",
    "steward.net.fetch",
  ]);
});

test("network check saves and clears an optional API token without echoing it", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.secrets.set": {},
        "steward.secrets.delete": { deleted: true },
      },
    },
  });

  await app.type("id:github-token", "github_pat_test");
  await app.press("text:Save token");
  expect(app.texts()).toContain("GitHub token saved in the system-protected secret store.");
  expect(app.texts().join("\n")).not.toContain("github_pat_test");
  expect(app.host.calls().map((call) => call.name)).toEqual(["steward.secrets.set"]);

  await app.press("text:Clear token");
  expect(app.texts()).toContain("Saved GitHub token deleted.");
  expect(app.host.calls().map((call) => call.name)).toEqual([
    "steward.secrets.set",
    "steward.secrets.delete",
  ]);
});
