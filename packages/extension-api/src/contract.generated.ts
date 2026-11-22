// Generated from contracts/plugin-api-v1.json. Do not edit directly.
// Run `pnpm contract:generate` after changing the contract.

export const HOST_API_VERSION = "1.0.0" as const;
export const HOST_CONTRACT_VERSION = 1 as const;
export const HOST_ERROR_ENVELOPE_VERSION = 1 as const;
export const HOST_MINIMUM_SUPPORTED_API_VERSION = "0.3.0" as const;

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
export const HOST_API_FAMILIES = [
  {
    name: "preferences",
    description: "Read host-rendered, plugin-scoped preference values supplied at launch.",
    maxOperationTimeoutMs: 5000,
    quotas: [
      {
        subject: "preference key",
        limit: "256 UTF-8 bytes per key",
        behavior: "invalid_input",
      },
      {
        subject: "request payload",
        limit: "4 KiB",
        behavior: "resource_exhausted",
      },
    ],
  },
  {
    name: "feedback",
    description: "Show host-rendered toasts, confirmations, and progress events.",
    maxOperationTimeoutMs: 300000,
    quotas: [
      {
        subject: "toast",
        limit: "title 128 bytes; message 1024 bytes",
        behavior: "invalid_input",
      },
      {
        subject: "confirmation",
        limit: "title 128 bytes; message 4096 bytes; labels 64 bytes each",
        behavior: "invalid_input",
      },
      {
        subject: "progress",
        limit: "id 128 bytes; message 1024 bytes",
        behavior: "invalid_input",
      },
    ],
  },
  {
    name: "clipboard",
    description: "Read recent host clipboard entries or replace the current text value.",
    maxOperationTimeoutMs: 5000,
    quotas: [
      {
        subject: "clipboard history",
        limit: "50 entries; 100000 bytes per entry",
        behavior: "oldest entries are omitted",
      },
      {
        subject: "clipboard write",
        limit: "100000 bytes",
        behavior: "resource_exhausted",
      },
    ],
  },
  {
    name: "storage",
    description: "Read and write plugin-private string storage that survives package updates.",
    maxOperationTimeoutMs: 5000,
    quotas: [
      {
        subject: "storage key",
        limit: "256 UTF-8 bytes",
        behavior: "invalid_input",
      },
      {
        subject: "storage value",
        limit: "1 MiB",
        behavior: "resource_exhausted",
      },
    ],
  },
  {
    name: "calendar",
    description: "Read Steward's cached holiday and adjusted-workday schedule.",
    maxOperationTimeoutMs: 20000,
    quotas: [
      {
        subject: "calendar year",
        limit: "0 through 65535",
        behavior: "invalid_input",
      },
      {
        subject: "holiday requests",
        limit: "16 queued requests",
        behavior: "resource_exhausted",
      },
    ],
  },
  {
    name: "navigation",
    description: "Close the plugin surface, return to the launcher, and set a bounded page title.",
    maxOperationTimeoutMs: 5000,
    quotas: [
      {
        subject: "page title",
        limit: "1 through 128 UTF-8 bytes",
        behavior: "invalid_input",
      },
    ],
  },
  {
    name: "filesystem",
    description:
      "Ask the user for selected roots and perform bounded text and metadata operations inside active path grants.",
    maxOperationTimeoutMs: 30000,
    quotas: [
      {
        subject: "path",
        limit: "32768 bytes",
        behavior: "invalid_input",
      },
      {
        subject: "walk",
        limit: "20000 results; depth 64; timeout 30000 ms",
        behavior: "invalid_input or timeout",
      },
      {
        subject: "read and write",
        limit: "4 MiB",
        behavior: "resource_exhausted",
      },
      {
        subject: "filesystem work queue",
        limit: "16 queued operations",
        behavior: "resource_exhausted",
      },
    ],
  },
  {
    name: "system",
    description:
      "Discover installed applications and open or reveal user-selected paths through the host.",
    maxOperationTimeoutMs: 20000,
    quotas: [
      {
        subject: "installed applications",
        limit: "2000 entries",
        behavior: "additional entries are omitted",
      },
      {
        subject: "system work queue",
        limit: "8 queued operations",
        behavior: "resource_exhausted",
      },
    ],
  },
  {
    name: "network",
    description:
      "Perform scoped HTTPS requests with host-owned redirect, size, timeout, and grant enforcement.",
    maxOperationTimeoutMs: 30000,
    quotas: [
      {
        subject: "request",
        limit: "URL 8192 bytes; 32 headers; body 1 MiB",
        behavior: "invalid_input or resource_exhausted",
      },
      {
        subject: "response",
        limit: "4 MiB",
        behavior: "resource_exhausted",
      },
      {
        subject: "network operation",
        limit: "timeout 100 through 30000 ms; 5 redirects",
        behavior: "timeout or resource_exhausted",
      },
      {
        subject: "network queue",
        limit: "8 queued requests",
        behavior: "resource_exhausted",
      },
    ],
  },
  {
    name: "process",
    description: "Run an exact approved executable through the launcher broker without a shell.",
    maxOperationTimeoutMs: 120000,
    quotas: [
      {
        subject: "arguments",
        limit: "256 arguments; 32768 bytes total",
        behavior: "resource_exhausted",
      },
      {
        subject: "controlled process",
        limit: "timeout 1 through 120000 ms; output 4 MiB",
        behavior: "timeout or resource_exhausted",
      },
      {
        subject: "process work and pending requests",
        limit: "8 queued tasks; 64 pending requests",
        behavior: "resource_exhausted",
      },
    ],
  },
] as const;
export type HostApiFamily = (typeof HOST_API_FAMILIES)[number];
export const HOST_FAILURE_STATES = [
  {
    state: "disabled",
    code: "feature_unavailable",
    meaning: "The running host does not provide the requested API family.",
  },
  {
    state: "revoked",
    code: "permission_denied",
    meaning:
      "No active capability grant matches the plugin, publisher, version, permission, and scope.",
  },
  {
    state: "expired",
    code: "permission_denied",
    meaning: "A previously active resource grant or package-version grant no longer matches.",
  },
  {
    state: "malformed",
    code: "invalid_input",
    meaning: "The request fails schema, range, path, or semantic validation.",
  },
  {
    state: "timedOut",
    code: "timeout",
    meaning: "A bounded host operation exceeded its deadline.",
  },
  {
    state: "cancelled",
    code: "cancelled",
    meaning: "The plugin or user cancelled the operation before completion.",
  },
  {
    state: "resourceExhausted",
    code: "resource_exhausted",
    meaning: "The request or API family exceeded a payload, queue, concurrency, or output quota.",
  },
] as const;
export type HostFailureState = (typeof HOST_FAILURE_STATES)[number];

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
  description: string;
  request: unknown;
  response: unknown;
}

export const HOST_CALL_FIXTURES = [
  {
    name: "steward.preferences.get",
    family: "preferences",
    capability: "steward.preferences.get",
    transport: "call",
    description: "Read one resolved host-owned preference value by key.",
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
    description: "Read the complete resolved preference map for the current plugin.",
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
    description: "Ask the launcher to render a transient informational notice.",
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
    description: "Ask the user to confirm or cancel an action through a host-rendered dialog.",
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
    description:
      "Publish bounded progress and receive a steward.progress event for the same plugin.",
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
    description: "Read the newest bounded snapshot of host clipboard text entries.",
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
    description: "Replace the current host clipboard text with bounded plugin-provided text.",
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
    description: "Read one string from plugin-private storage.",
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
    description: "Persist one bounded string in plugin-private storage.",
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
    description: "Read the cached holiday and adjusted-workday schedule for one year.",
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
    description: "Request that the current plugin surface close.",
    request: null,
    response: null,
  },
  {
    name: "steward.navigation.back",
    family: "navigation",
    capability: "steward.navigation.back",
    transport: "call",
    description: "Leave the plugin page and restore launcher focus for embedded presentations.",
    request: null,
    response: {},
  },
  {
    name: "steward.navigation.close",
    family: "navigation",
    capability: "steward.navigation.close",
    transport: "call",
    description: "Close the plugin page and destroy the isolated plugin host process.",
    request: null,
    response: {},
  },
  {
    name: "steward.navigation.setTitle",
    family: "navigation",
    capability: "steward.navigation.setTitle",
    transport: "call",
    description: "Set the bounded title of the current plugin surface.",
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
    description:
      "Ask the user to select a file or folder and store the exact canonical path grant.",
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
    description: "Request cancellation of an in-flight filesystem walk by caller-supplied ID.",
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
    description: "Recursively list bounded metadata under an active selected path grant.",
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
    description: "Read bounded UTF-8 text from a file inside an active read grant.",
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
    description: "Write bounded UTF-8 text to a path inside an active write grant.",
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
    description: "List the active selected filesystem roots and their read/write modes.",
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
    description: "List bounded installed-application IDs and names from Steward's catalog.",
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
    description: "Open a selected path with its registered default handler.",
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
    description: "Reveal a selected path in Windows File Explorer.",
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
    description:
      "Open one selected path with an application ID from Steward's installed-app catalog.",
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
    description: "Perform one scoped HTTPS request under active host, method, and path grants.",
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
    description:
      "Run one exact approved executable through the launcher-owned controlled-process broker.",
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
    description: "Request cancellation of an active controlled process and its child tree.",
    request: {
      requestId: 8180697935898170,
    },
    response: {
      cancelled: true,
    },
  },
] as const satisfies readonly HostCallFixture[];
