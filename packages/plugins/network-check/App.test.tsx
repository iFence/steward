import { expect, open, test } from "nsh/test";

test("network check consumes the scoped HTTPS API", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
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
  expect(app.host.calls().map((call) => call.name)).toEqual(["steward.net.fetch"]);
});
