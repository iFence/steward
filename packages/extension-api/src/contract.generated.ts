// Generated from contracts/plugin-api-v1.json. Do not edit directly.
// Run `pnpm contract:generate` after changing the contract.

export const HOST_API_VERSION = "0.5.0" as const;
export const HOST_CONTRACT_VERSION = 1 as const;
export const HOST_ERROR_ENVELOPE_VERSION = 1 as const;

export const HOST_ERROR_CODES = [
  "permission_denied",
  "feature_unavailable",
  "timeout",
  "resource_exhausted",
  "host_failure",
  "invalid_input",
  "cancelled",
  "confirmation_denied",
  "io_failure",
] as const;
export type StewardErrorCode = (typeof HOST_ERROR_CODES)[number];
export const HOST_RETRYABLE_ERROR_CODES = [
  "timeout",
  "resource_exhausted",
  "host_failure",
  "cancelled",
] as const;

export const HOST_FEATURE_NAMES = [
  "host_info",
  "runtime_feature_discovery",
  "steward.feedback",
  "steward.navigation",
  "steward.preferences",
] as const;
export type HostFeatureName = (typeof HOST_FEATURE_NAMES)[number];

export const HOST_EVENT_NAMES = [
  "steward.command",
  "steward.theme",
  "steward.visibility",
  "steward.progress",
] as const;
export type HostEventName = (typeof HOST_EVENT_NAMES)[number];

export const HOST_CALL_NAMES = [
  "steward.preferences.get",
  "steward.preferences.all",
  "steward.feedback.toast",
  "steward.feedback.confirm",
  "steward.feedback.progress",
  "steward.clipboard.history",
  "steward.clipboard.write",
  "steward.storage.get",
  "steward.storage.set",
  "steward.calendar.holidays",
  "steward.window.close",
  "steward.navigation.back",
  "steward.navigation.close",
  "steward.navigation.setTitle",
  "steward.fs.pick",
  "steward.fs.cancelWalk",
  "steward.fs.walk",
  "steward.fs.readText",
  "steward.fs.writeText",
  "steward.fs.grants",
  "steward.system.applications",
  "steward.system.openPath",
  "steward.system.revealPath",
  "steward.system.openWith",
  "steward.net.fetch",
  "steward.exec.run",
  "steward.exec.cancel",
] as const;
export type HostCallName = (typeof HOST_CALL_NAMES)[number];

export const HOST_CAPABILITIES = [
  {
    permission: "steward.preferences.get",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.preferences.all",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.feedback.toast",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.feedback.confirm",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.feedback.progress",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.clipboard.history",
    scope: "host=steward",
    sensitive: true,
  },
  {
    permission: "steward.clipboard.write",
    scope: "host=steward",
    sensitive: true,
  },
  {
    permission: "steward.storage.get",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.storage.set",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.calendar.holidays",
    scope: "host=steward",
    sensitive: true,
  },
  {
    permission: "steward.window.close",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.navigation.back",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.navigation.close",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.navigation.setTitle",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.fs.pick",
    scope: "host=steward",
    sensitive: true,
  },
  {
    permission: "steward.fs.walk",
    scope: "path=<user-selected-root>",
    sensitive: true,
  },
  {
    permission: "steward.fs.readText",
    scope: "path=<user-selected-root>",
    sensitive: true,
  },
  {
    permission: "steward.fs.writeText",
    scope: "path=<user-selected-root>",
    sensitive: true,
  },
  {
    permission: "steward.fs.grants",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.fs.cancelWalk",
    scope: "host=steward",
    sensitive: false,
  },
  {
    permission: "steward.system.applications",
    scope: "system=installed",
    sensitive: true,
  },
  {
    permission: "steward.system.openPath",
    scope: "path=<user-selected-root>",
    sensitive: true,
  },
  {
    permission: "steward.system.revealPath",
    scope: "path=<user-selected-root>",
    sensitive: true,
  },
  {
    permission: "steward.system.openWith",
    scope: "path=<user-selected-root>;application=<installed-id>",
    sensitive: true,
  },
  {
    permission: "steward.net.fetch",
    scope:
      "host=<granted-host>;schemes=<allowed-schemes>;methods=<allowed-methods>;pathPrefixes=<allowed-prefixes>",
    sensitive: true,
  },
  {
    permission: "steward.exec.run",
    scope: "executable=<approved-exe>",
    sensitive: true,
  },
  {
    permission: "steward.exec.cancel",
    scope: "host=steward",
    sensitive: false,
  },
] as const;

export interface HostErrorPayload {
  version: typeof HOST_ERROR_ENVELOPE_VERSION;
  code: StewardErrorCode;
  message: string;
  retryable: boolean;
}

export interface HostCallFixture {
  name: HostCallName;
  family: string;
  capability: string;
  transport: "call" | "post";
  request: unknown;
  response: unknown;
}

export const HOST_CALL_FIXTURES = [
  {
    name: "steward.preferences.get",
    family: "preferences",
    capability: "steward.preferences.get",
    transport: "call",
    request: {
      key: "theme",
    },
    response: "dark",
  },
  {
    name: "steward.preferences.all",
    family: "preferences",
    capability: "steward.preferences.all",
    transport: "call",
    request: null,
    response: {
      theme: "dark",
    },
  },
  {
    name: "steward.feedback.toast",
    family: "feedback",
    capability: "steward.feedback.toast",
    transport: "call",
    request: {
      title: "Indexed",
      message: "Repository index is ready.",
    },
    response: {},
  },
  {
    name: "steward.feedback.confirm",
    family: "feedback",
    capability: "steward.feedback.confirm",
    transport: "call",
    request: {
      title: "Delete repository",
      message: "Move this repository to the Recycle Bin?",
      confirmLabel: "Move",
      cancelLabel: "Cancel",
      destructive: true,
    },
    response: {
      confirmed: true,
    },
  },
  {
    name: "steward.feedback.progress",
    family: "feedback",
    capability: "steward.feedback.progress",
    transport: "call",
    request: {
      id: "index",
      message: "Indexing",
      current: 2,
      total: 10,
    },
    response: {},
  },
  {
    name: "steward.clipboard.history",
    family: "clipboard",
    capability: "steward.clipboard.history",
    transport: "call",
    request: null,
    response: [
      {
        id: "42",
        text: "Steward",
        copied_at: 1763769600,
      },
    ],
  },
  {
    name: "steward.clipboard.write",
    family: "clipboard",
    capability: "steward.clipboard.write",
    transport: "call",
    request: {
      text: "Steward",
    },
    response: {},
  },
  {
    name: "steward.storage.get",
    family: "storage",
    capability: "steward.storage.get",
    transport: "call",
    request: {
      key: "favorites",
    },
    response: "[]",
  },
  {
    name: "steward.storage.set",
    family: "storage",
    capability: "steward.storage.set",
    transport: "call",
    request: {
      key: "favorites",
      value: "[]",
    },
    response: {},
  },
  {
    name: "steward.calendar.holidays",
    family: "calendar",
    capability: "steward.calendar.holidays",
    transport: "call",
    request: {
      year: 2026,
    },
    response: {
      "2026-01-01": "holiday",
    },
  },
  {
    name: "steward.window.close",
    family: "navigation",
    capability: "steward.window.close",
    transport: "post",
    request: null,
    response: null,
  },
  {
    name: "steward.navigation.back",
    family: "navigation",
    capability: "steward.navigation.back",
    transport: "call",
    request: null,
    response: {},
  },
  {
    name: "steward.navigation.close",
    family: "navigation",
    capability: "steward.navigation.close",
    transport: "call",
    request: null,
    response: {},
  },
  {
    name: "steward.navigation.setTitle",
    family: "navigation",
    capability: "steward.navigation.setTitle",
    transport: "call",
    request: {
      title: "Repositories",
    },
    response: {},
  },
  {
    name: "steward.fs.pick",
    family: "filesystem",
    capability: "steward.fs.pick",
    transport: "call",
    request: {
      mode: "folderWrite",
    },
    response: {
      path: "D:\\projects",
    },
  },
  {
    name: "steward.fs.cancelWalk",
    family: "filesystem",
    capability: "steward.fs.cancelWalk",
    transport: "call",
    request: {
      id: "contract-walk-1",
    },
    response: {
      cancelled: true,
    },
  },
  {
    name: "steward.fs.walk",
    family: "filesystem",
    capability: "steward.fs.walk",
    transport: "call",
    request: {
      id: "contract-walk-1",
      root: "D:\\projects",
      maxResults: 2000,
      maxDepth: 24,
      timeoutMs: 10000,
    },
    response: {
      root: "D:\\projects",
      entries: [
        {
          path: "D:\\projects\\steward",
          relativePath: "steward",
          directory: true,
          reparsePoint: false,
          size: 0,
        },
      ],
      truncated: false,
      elapsedMs: 4,
    },
  },
  {
    name: "steward.fs.readText",
    family: "filesystem",
    capability: "steward.fs.readText",
    transport: "call",
    request: {
      path: "D:\\projects\\steward\\README.md",
      maxBytes: 1048576,
    },
    response: "# Steward\n",
  },
  {
    name: "steward.fs.writeText",
    family: "filesystem",
    capability: "steward.fs.writeText",
    transport: "call",
    request: {
      path: "D:\\projects\\steward\\notes.txt",
      contents: "hello\n",
    },
    response: {
      path: "D:\\projects\\steward\\notes.txt",
    },
  },
  {
    name: "steward.fs.grants",
    family: "filesystem",
    capability: "steward.fs.grants",
    transport: "call",
    request: null,
    response: [
      {
        path: "D:\\projects",
        read: true,
        write: true,
      },
    ],
  },
  {
    name: "steward.system.applications",
    family: "system",
    capability: "steward.system.applications",
    transport: "call",
    request: null,
    response: [
      {
        id: "C:\\Program Files\\Editor\\editor.exe",
        name: "Editor",
      },
    ],
  },
  {
    name: "steward.system.openPath",
    family: "system",
    capability: "steward.system.openPath",
    transport: "call",
    request: {
      path: "D:\\projects\\steward",
    },
    response: {
      path: "D:\\projects\\steward",
    },
  },
  {
    name: "steward.system.revealPath",
    family: "system",
    capability: "steward.system.revealPath",
    transport: "call",
    request: {
      path: "D:\\projects\\steward",
    },
    response: {
      path: "D:\\projects\\steward",
    },
  },
  {
    name: "steward.system.openWith",
    family: "system",
    capability: "steward.system.openWith",
    transport: "call",
    request: {
      applicationId: "C:\\Program Files\\Editor\\editor.exe",
      path: "D:\\projects\\steward\\README.md",
    },
    response: {},
  },
  {
    name: "steward.net.fetch",
    family: "network",
    capability: "steward.net.fetch",
    transport: "call",
    request: {
      url: "https://api.github.com/rate_limit",
      method: "GET",
      headers: {
        Accept: "application/vnd.github+json",
      },
      timeoutMs: 10000,
      maxResponseBytes: 262144,
    },
    response: {
      status: 200,
      url: "https://api.github.com/rate_limit",
      headers: [
        {
          name: "content-type",
          value: "application/json",
        },
      ],
      body: '{"rate":{"remaining":59,"limit":60}}',
      redirects: 0,
      elapsedMs: 12,
    },
  },
  {
    name: "steward.exec.run",
    family: "process",
    capability: "steward.exec.run",
    transport: "call",
    request: {
      requestId: 8180697935898170,
      executable: "git.exe",
      arguments: ["status", "--porcelain=v1"],
      workingDirectory: "D:\\projects\\steward",
      timeoutMs: 30000,
      maxOutputBytes: 1048576,
    },
    response: {
      exitCode: 0,
      stdout: "## main...origin/main\n",
      stderr: "",
      durationMs: 8,
      timedOut: false,
      cancelled: false,
    },
  },
  {
    name: "steward.exec.cancel",
    family: "process",
    capability: "steward.exec.cancel",
    transport: "call",
    request: {
      requestId: 8180697935898170,
    },
    response: {
      cancelled: true,
    },
  },
] as const satisfies readonly HostCallFixture[];
