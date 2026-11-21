import { expect, open, test } from "nsh/test";

const ENTRIES = [
  { id: "1", text: "Newest clipboard item", copied_at: 1_700_000_003 },
  { id: "2", text: "Pinned clipboard item", copied_at: 1_700_000_002 },
  { id: "3", text: "Old clipboard item", copied_at: 1_700_000_001 },
];

function host(copied: string[], refreshCount: { value: number }, pinned = '["2"]') {
  return {
    handlers: {
      "steward.clipboard.history": () => {
        refreshCount.value += 1;
        return ENTRIES;
      },
      "steward.clipboard.write": (data: unknown) => {
        copied.push((data as { text: string }).text);
        return {};
      },
      "steward.navigation.back": () => ({}),
      "steward.navigation.setTitle": () => ({}),
      "steward.storage.get": (data: unknown) =>
        (data as { key: string }).key === "pinned" ? pinned : null,
      "steward.storage.set": (_data: unknown) => ({}),
    },
    params: {
      accent: "#863bff",
      command: "clipboard-history",
      dark: true,
      input: "clipboard-history",
      locale: "en-US",
      pluginId: "it.aspirin.clipboard-history",
      presentation: "embedded",
    },
  };
}

test("clipboard history opens as a searchable embedded page", async () => {
  const copied: string[] = [];
  const refreshCount = { value: 0 };
  const app = await open("./App.tsx", { host: host(copied, refreshCount) });

  await app.waitFor("id:clipboard-entry-1");
  const texts = app.texts();
  expect(texts).toContain("Clipboard History");
  expect(texts.indexOf("Pinned clipboard item")).toBeLessThan(
    texts.indexOf("Newest clipboard item"),
  );
  expect(
    app.host.calls().filter((call) => call.name === "steward.navigation.setTitle"),
  ).toContainEqual({ name: "steward.navigation.setTitle", data: { title: "Clipboard History" } });

  await app.type("id:clipboard-search", "newest");
  expect(app.shows("id:clipboard-entry-1")).toBe(true);
  expect(app.shows("id:clipboard-entry-2")).toBe(false);

  await app.press("id:clipboard-entry-1");
  expect(copied).toEqual(["Newest clipboard item"]);
  expect(app.texts()).toContain("Copied to clipboard");
});

test("clipboard history refreshes when visible and restores launcher focus", async () => {
  const copied: string[] = [];
  const refreshCount = { value: 0 };
  const app = await open("./App.tsx", { host: host(copied, refreshCount) });

  await app.waitFor("id:clipboard-entry-1");
  expect(refreshCount.value).toBe(1);
  await app.host.send("steward.visibility", { visible: false });
  await app.host.send("steward.visibility", { visible: true });
  expect(refreshCount.value).toBe(2);

  await app.press("id:clipboard-back");
  expect(app.host.calls().filter((call) => call.name === "steward.navigation.back").length).toBe(1);
});

test("clipboard history pins entries without copying them", async () => {
  const copied: string[] = [];
  const refreshCount = { value: 0 };
  const storageWrites: string[] = [];
  const appHost = host(copied, refreshCount);
  appHost.handlers["steward.storage.set"] = (data: unknown) => {
    storageWrites.push((data as { value: string }).value);
    return {};
  };
  const app = await open("./App.tsx", { host: appHost });

  await app.waitFor("text:Pinned");
  await app.press("id:clipboard-pin-2");
  expect(copied).toEqual([]);
  expect(storageWrites).toEqual(["[]"]);

  await app.press("id:clipboard-pin-1");
  expect(storageWrites[1]).toBe('["1"]');
});

test("clipboard history localizes and supports keyboard copy", async () => {
  const copied: string[] = [];
  const refreshCount = { value: 0 };
  const appHost = host(copied, refreshCount);
  appHost.params.locale = "zh-CN";
  const app = await open("./App.tsx", { host: appHost });

  await app.waitFor("id:clipboard-entry-1");
  expect(app.texts()).toContain("剪贴板历史");
  await app.keyboard.press("down");
  await app.keyboard.press("enter");
  expect(copied).toEqual(["Newest clipboard item"]);
  expect(app.texts()).toContain("已复制到剪贴板");
});
