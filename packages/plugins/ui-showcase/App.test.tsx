import { expect, open, test } from "nsh/test";

const params = {
  accent: "#863bff",
  command: "uishowcase",
  dark: true,
  input: "",
  locale: "en-US",
  pluginId: "it.aspirin.ui-showcase",
  preferences: {
    density: "compact",
    editor: "",
    showHints: false,
    title: "Hosted preferences",
    workspace: "",
  },
};

test("showcase reads host-owned preferences", async () => {
  const app = await open("./App.tsx", { host: { params } });

  expect(app.texts()).toContain("Hosted preferences");
  expect(app.texts()).not.toContain(
    "Native TSX, signals, hosted preferences and host-controlled capabilities.",
  );
});

test("showcase mocks hosted feedback without a desktop host", async () => {
  const app = await open("./App.tsx", {
    host: {
      params,
      handlers: {
        "steward.feedback.toast": {},
        "steward.feedback.confirm": () => ({ confirmed: true }),
        "steward.feedback.progress": {},
      },
    },
  });

  await app.press("text:Show toast");
  expect(app.texts()).toContain("Toast sent");
  await app.press("text:Confirm");
  expect(app.texts()).toContain("Confirmed");
  expect(app.host.calls().map((call) => call.name)).toEqual([
    "steward.feedback.toast",
    "steward.feedback.progress",
    "steward.feedback.confirm",
  ]);

  await app.host.send("steward.progress", {
    id: "download",
    message: "Downloading",
    current: 1,
    total: 4,
  });
  expect(app.texts()).toContain("Downloading");
});

test("showcase handles a denied confirmation", async () => {
  const app = await open("./App.tsx", {
    host: {
      params,
      handlers: {
        "steward.feedback.toast": {},
        "steward.feedback.progress": {},
        "steward.feedback.confirm": () => {
          throw new Error("confirmation denied");
        },
      },
    },
  });

  await app.press("text:Confirm");
  expect(app.texts()).toContain("Cancelled");
});
