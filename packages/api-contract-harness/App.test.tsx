import { expect, open, test } from "nsh/test";
import {
  HOST_API_FAMILIES,
  HOST_CALL_FIXTURES,
  HOST_ERROR_CODES,
  HOST_FAILURE_STATES,
  HOST_MINIMUM_SUPPORTED_API_VERSION,
  type HostCallFixture,
  type StewardErrorCode,
} from "@steward/extension-api";

const params = {
  accent: "#863bff",
  command: "api-contract",
  dark: true,
  input: "",
  locale: "en-US",
  pluginId: "com.steward.api-contract-harness",
  preferences: { theme: "dark" },
};

function handlers() {
  return Object.fromEntries(HOST_CALL_FIXTURES.map((call) => [call.name, call.response])) as Record<
    string,
    unknown
  >;
}

function expectedCalls(fixtures: readonly HostCallFixture[]) {
  return fixtures
    .filter((call) => call.transport === "call")
    .map((call) => ({ name: call.name, data: call.request }));
}

function expectedPosts(fixtures: readonly HostCallFixture[]) {
  return fixtures
    .filter((call) => call.transport === "post")
    .map((call) => ({ name: call.name, data: call.request }));
}

test("contract fixtures cover every host bridge family", () => {
  expect([...new Set(HOST_CALL_FIXTURES.map((call) => call.family))]).toEqual([
    "preferences",
    "feedback",
    "clipboard",
    "storage",
    "secrets",
    "calendar",
    "navigation",
    "filesystem",
    "system",
    "network",
    "process",
  ]);
});

test("v1 hardening exposes quotas and stable failure semantics for every family", () => {
  expect(HOST_MINIMUM_SUPPORTED_API_VERSION).toBe("0.3.0");
  expect([...new Set(HOST_CALL_FIXTURES.map((call) => call.family))]).toEqual(
    HOST_API_FAMILIES.map((family) => family.name),
  );
  expect(HOST_FAILURE_STATES.map((state) => state.state)).toEqual([
    "disabled",
    "revoked",
    "expired",
    "malformed",
    "timedOut",
    "cancelled",
    "resourceExhausted",
  ]);
  for (const failure of HOST_FAILURE_STATES) {
    expect(HOST_ERROR_CODES).toContain(failure.code);
  }
  for (const family of HOST_API_FAMILIES) {
    expect(family.maxOperationTimeoutMs).toBeGreaterThan(0);
    expect(family.quotas.length).toBeGreaterThan(0);
  }
  for (const call of HOST_CALL_FIXTURES) {
    expect(call.description.length).toBeGreaterThan(0);
  }
});

test("every committed SDK fixture produces the contract wire payload", async () => {
  const app = await open("./App.tsx", {
    host: { params, handlers: handlers() },
  });

  for (const call of HOST_CALL_FIXTURES) {
    await app.press(`text:${call.name}`);
    if (call.name === "steward.exec.run") {
      expect(app.texts()).toContain("Status: process:contract-process-1");
    }
  }

  expect(app.host.calls()).toEqual(expectedCalls(HOST_CALL_FIXTURES));
  expect(app.host.posts()).toEqual(expectedPosts(HOST_CALL_FIXTURES));
});

test("structured host errors become StewardError data without prose parsing", async () => {
  const payload = {
    version: 1,
    code: "permission_denied" satisfies StewardErrorCode,
    message: "clipboard write is not granted",
    retryable: false,
  };
  const app = await open("./App.tsx", {
    host: {
      params,
      handlers: {
        ...handlers(),
        "steward.clipboard.write": () => {
          throw new Error(JSON.stringify(payload));
        },
      },
    },
  });

  await app.press("text:structured error");
  expect(app.texts()).toContain("Status: permission_denied:false:clipboard write is not granted");
});

test("SDK rejects a quota violation before sending a host call", async () => {
  const app = await open("./App.tsx", {
    host: { params, handlers: handlers() },
  });

  await app.press("text:quota error");
  expect(app.texts()).toContain(
    "Status: resource_exhausted:true:clipboard text exceeds the 100000 byte limit",
  );
  expect(app.host.calls()).toEqual([]);
});

test("SDK rejects oversized secrets before sending a host call", async () => {
  const app = await open("./App.tsx", {
    host: { params, handlers: handlers() },
  });

  await app.press("text:secret quota error");
  expect(app.texts()).toContain(
    "Status: resource_exhausted:true:secret value exceeds the 2560 byte limit",
  );
  expect(app.host.calls()).toEqual([]);
});
