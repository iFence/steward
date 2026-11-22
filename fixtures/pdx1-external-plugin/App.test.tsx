import { expect, open, test } from "nsh/test";

test("handles UI interaction and a mocked extension API call", async () => {
  const app = await open("./App.tsx", {
    host: {
      params: {
        pluginId: "com.example.pdx1-external",
        command: "pdx1-external",
        input: "",
        locale: "en-US",
        accent: "#863bff",
        dark: true,
      },
      handlers: {
        "steward.feedback.toast": {},
      },
    },
  });

  await app.press("text:Clicks: 0");
  expect(app.texts()).toContain("Clicks: 1");

  await app.press("text:Send test toast");
  expect(app.texts()).toContain("Toast delivered");
  expect(app.host.calls()).toEqual([
    {
      name: "steward.feedback.toast",
      data: {
        title: "External plugin",
        message: "Installed SDK host call succeeded.",
      },
    },
  ]);
});
