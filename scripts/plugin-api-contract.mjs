#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const contractPath = resolve(root, "contracts/plugin-api-v1.json");
const generatedTsPath = resolve(root, "packages/extension-api/src/contract.generated.ts");
const generatedRustPath = resolve(root, "crates/plugin-contract/src/host_contract.generated.rs");
const apiReferencePath = resolve(root, "docs/plugin-api-reference.md");
const compatibilityPath = resolve(root, "contracts/plugin-compatibility.json");
const compatibilityFixturesPath = resolve(root, "contracts/plugin-api-compatibility-v1.json");
const sdkPath = resolve(root, "packages/extension-api/src/host.ts");
const handlersPath = resolve(root, "crates/app/src/plugin_window.rs");
const capabilitiesPath = resolve(root, "crates/plugin-contract/src/capabilities.rs");
const prettierCli = resolve(root, "node_modules/prettier/bin/prettier.cjs");
const hostRejectionPaths = [
  "crates/app/src/plugin_window.rs",
  "crates/app/src/plugin_filesystem.rs",
  "crates/app/src/plugin_network.rs",
  "crates/app/src/plugin_system.rs",
  "crates/app/src/plugin_execution.rs",
];

const write = process.argv.includes("--write");
const contract = JSON.parse(readFileSync(contractPath, "utf8"));

function fail(message) {
  throw new Error(`plugin API contract: ${message}`);
}

function assertUnique(values, label) {
  const duplicates = values.filter((value, index) => values.indexOf(value) !== index);
  if (duplicates.length)
    fail(`${label} contains duplicates: ${[...new Set(duplicates)].join(", ")}`);
}

function semanticVersion(value, label) {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z.-]+)?$/.exec(value);
  if (!match) fail(`${label} is not semantic version syntax: ${value}`);
  return match.slice(1, 4).map(Number);
}

function compareSemanticVersions(left, right) {
  const leftParts = semanticVersion(left, "left version");
  const rightParts = semanticVersion(right, "right version");
  for (let index = 0; index < 3; index += 1) {
    if (leftParts[index] !== rightParts[index]) return leftParts[index] - rightParts[index];
  }
  return 0;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function validateExample(schema, value, location) {
  if (!schema || typeof schema !== "object") fail(`${location} is not a schema`);
  const allowedTypes = Array.isArray(schema.type) ? schema.type : schema.type ? [schema.type] : [];
  const actualType =
    value === null
      ? "null"
      : Array.isArray(value)
        ? "array"
        : Number.isInteger(value)
          ? "integer"
          : typeof value;
  if (allowedTypes.length && !allowedTypes.includes(actualType)) {
    fail(`${location} example has type ${actualType}, expected ${allowedTypes.join(" or ")}`);
  }
  if (schema.enum && !schema.enum.includes(value)) {
    fail(`${location} example is not one of ${schema.enum.join(", ")}`);
  }
  if (schema.const !== undefined && value !== schema.const) {
    fail(`${location} example must equal ${JSON.stringify(schema.const)}`);
  }
  if (typeof value === "number") {
    if (schema.minimum !== undefined && value < schema.minimum) {
      fail(`${location} example is below ${schema.minimum}`);
    }
    if (schema.maximum !== undefined && value > schema.maximum) {
      fail(`${location} example is above ${schema.maximum}`);
    }
  }
  if (typeof value === "string") {
    if (schema.minLength !== undefined && value.length < schema.minLength) {
      fail(`${location} example is shorter than ${schema.minLength}`);
    }
    if (schema.maxLength !== undefined && value.length > schema.maxLength) {
      fail(`${location} example is longer than ${schema.maxLength}`);
    }
  }
  if (Array.isArray(value)) {
    if (schema.minItems !== undefined && value.length < schema.minItems) {
      fail(`${location} example has fewer than ${schema.minItems} items`);
    }
    if (schema.maxItems !== undefined && value.length > schema.maxItems) {
      fail(`${location} example has more than ${schema.maxItems} items`);
    }
    if (schema.items) {
      value.forEach((item, index) => validateExample(schema.items, item, `${location}[${index}]`));
    }
  }
  if (value && actualType === "object") {
    if (schema.minProperties !== undefined && Object.keys(value).length < schema.minProperties) {
      fail(`${location} example has fewer than ${schema.minProperties} fields`);
    }
    if (schema.maxProperties !== undefined && Object.keys(value).length > schema.maxProperties) {
      fail(`${location} example has more than ${schema.maxProperties} fields`);
    }
    for (const key of schema.required ?? []) {
      if (!(key in value)) fail(`${location} example is missing required field ${key}`);
    }
    const properties = schema.properties ?? {};
    for (const [key, item] of Object.entries(value)) {
      if (properties[key]) {
        validateExample(properties[key], item, `${location}.${key}`);
      } else if (schema.additionalProperties === false) {
        fail(`${location} example has unknown field ${key}`);
      } else if (schema.additionalProperties && typeof schema.additionalProperties === "object") {
        validateExample(schema.additionalProperties, item, `${location}.${key}`);
      }
    }
  }
}

function literalHostCalls(source) {
  const calls = [];
  const matcher = /callHost|postHost|host\.(call|post)/g;
  for (const match of source.matchAll(matcher)) {
    const transport = match[0].startsWith("post") || match[1] === "post" ? "post" : "call";
    const open = source.indexOf("(", match.index + match[0].length);
    if (open < 0) continue;
    let cursor = open + 1;
    while (/\s/.test(source[cursor] ?? "")) cursor += 1;
    if (source[cursor] !== '"') continue;
    cursor += 1;
    const end = source.indexOf('"', cursor);
    if (end < 0) continue;
    calls.push({ transport, name: source.slice(cursor, end) });
  }
  return calls;
}

function registeredHandlers(source, capabilitiesSource) {
  const constants = new Map();
  for (const match of capabilitiesSource.matchAll(
    /pub const ([A-Z][A-Z0-9_]*): &str = "([^"]+)";/g,
  )) {
    constants.set(match[1], match[2]);
  }
  const names = [];
  for (const match of source.matchAll(/host\.handle\(\s*(?:"([^"]+)"|([A-Z][A-Z0-9_]*))/g)) {
    const name = match[1] ?? constants.get(match[2]);
    if (!name) fail(`cannot resolve host handler constant ${match[2]}`);
    names.push(name);
  }
  return names;
}

function validateCompatibility(sdkCalls) {
  const compatibility = JSON.parse(readFileSync(compatibilityPath, "utf8"));
  const fixtures = JSON.parse(readFileSync(compatibilityFixturesPath, "utf8"));
  const sdkSource = readFileSync(sdkPath, "utf8");
  const current = compatibility.current;
  const currentCalls = new Map(contract.calls.map((call) => [call.name, call]));
  const currentSdkCalls = new Set(sdkCalls.map((call) => call.name));

  if (fixtures.schemaVersion !== 1) fail("compatibility fixture schemaVersion must be 1");
  if (!Array.isArray(fixtures.lines) || fixtures.lines.length === 0) {
    fail("compatibility fixtures contain no API lines");
  }
  assertUnique(
    fixtures.lines.map((line) => line.hostApi),
    "compatibility fixture API lines",
  );

  const supportedLines = new Map(compatibility.supported.map((line) => [line.hostApi, line]));
  for (const line of fixtures.lines) {
    semanticVersion(line.hostApi.replace(/\.x$/, ".0"), `${line.hostApi}.hostApi`);
    semanticVersion(line.sdk.replace(/\.x$/, ".0"), `${line.hostApi}.sdk`);
    semanticVersion(line.minimumHostVersion, `${line.hostApi}.minimumHostVersion`);
    if (!supportedLines.has(line.hostApi)) {
      fail(`compatibility fixture line ${line.hostApi} is absent from plugin-compatibility.json`);
    }
    if (compareSemanticVersions(line.minimumHostVersion, contract.hostApiVersion) > 0) {
      fail(`compatibility fixture line ${line.hostApi} requires a future host`);
    }
    if (!Array.isArray(line.calls) || line.calls.length === 0) {
      fail(`compatibility fixture line ${line.hostApi} has no calls`);
    }
    for (const fixture of line.calls) {
      const call = currentCalls.get(fixture.name);
      if (!call) fail(`${line.hostApi} fixture references unknown call ${fixture.name}`);
      if (!fixture.name.startsWith("steward.preferences.") && !currentSdkCalls.has(fixture.name)) {
        fail(`${line.hostApi} fixture call ${fixture.name} is absent from the current SDK`);
      }
      if (
        !new RegExp(
          `export\\s+(?:async\\s+)?function\\s+${escapeRegExp(fixture.sdkHelper)}(?:\\s*<[^>]+>)?\\s*\\(`,
        ).test(sdkSource)
      ) {
        fail(`${line.hostApi} fixture helper ${fixture.sdkHelper} is not exported by the SDK`);
      }
      validateExample(call.request, fixture.request, `${line.hostApi}.${fixture.name}.request`);
      validateExample(call.response, fixture.response, `${line.hostApi}.${fixture.name}.response`);
    }
  }

  for (const line of compatibility.supported) {
    if (!fixtures.lines.some((fixture) => fixture.hostApi === line.hostApi)) {
      fail(`supported API line ${line.hostApi} has no compatibility fixture`);
    }
  }
  semanticVersion(current.hostApiVersion, "compatibility.current.hostApiVersion");
  semanticVersion(current.sdkVersion, "compatibility.current.sdkVersion");
  semanticVersion(current.minimumHostVersion, "compatibility.current.minimumHostVersion");
  semanticVersion(
    current.minimumSupportedHostVersion,
    "compatibility.current.minimumSupportedHostVersion",
  );
  if (current.hostApiVersion !== contract.hostApiVersion) {
    fail("compatibility matrix host API version differs from the contract");
  }
  const currentLines = compatibility.supported.filter((line) => line.status === "current");
  if (
    currentLines.length !== 1 ||
    !currentLines[0].hostApi.startsWith(
      `${contract.hostApiVersion.split(".").slice(0, 2).join(".")}.`,
    )
  ) {
    fail("compatibility matrix must contain exactly one current host API line");
  }
}

function generatedTypeScript() {
  const calls = contract.calls.map((call) => ({
    name: call.name,
    family: call.family,
    capability: call.capability,
    transport: call.transport,
    description: call.description,
    request: call.request.examples[0],
    response: call.response.examples[0],
  }));
  const features = contract.features;
  const events = contract.events;
  const errors = contract.errors;
  const capabilities = contract.capabilities;

  return `// Generated from contracts/plugin-api-v1.json. Do not edit directly.
// Run \`pnpm contract:generate\` after changing the contract.

export const HOST_API_VERSION = ${JSON.stringify(contract.hostApiVersion)} as const;
export const HOST_CONTRACT_VERSION = ${contract.contractVersion} as const;
export const HOST_ERROR_ENVELOPE_VERSION = ${contract.errorEnvelopeVersion} as const;
export const HOST_MINIMUM_SUPPORTED_API_VERSION = ${JSON.stringify(
    contract.stability.minimumSupportedApiVersion,
  )} as const;

export const HOST_ERROR_CODES = ${JSON.stringify(
    errors.map((error) => error.code),
    null,
    2,
  )} as const;
export type StewardErrorCode = (typeof HOST_ERROR_CODES)[number];
export const HOST_RETRYABLE_ERROR_CODES = ${JSON.stringify(
    errors.filter((error) => error.retryable).map((error) => error.code),
    null,
    2,
  )} as const;

export const HOST_FEATURE_NAMES = ${JSON.stringify(features, null, 2)} as const;
export type HostFeatureName = (typeof HOST_FEATURE_NAMES)[number];

export const HOST_EVENT_NAMES = ${JSON.stringify(
    events.map((event) => event.name),
    null,
    2,
  )} as const;
export type HostEventName = (typeof HOST_EVENT_NAMES)[number];

export const HOST_CALL_NAMES = ${JSON.stringify(
    calls.map((call) => call.name),
    null,
    2,
  )} as const;
export type HostCallName = (typeof HOST_CALL_NAMES)[number];

export const HOST_CAPABILITIES = ${JSON.stringify(capabilities, null, 2)} as const;
export const HOST_API_FAMILIES = ${JSON.stringify(contract.families, null, 2)} as const;
export type HostApiFamily = (typeof HOST_API_FAMILIES)[number];
export const HOST_FAILURE_STATES = ${JSON.stringify(contract.failureStates, null, 2)} as const;
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

export const HOST_CALL_FIXTURES = ${JSON.stringify(calls, null, 2)} as const satisfies readonly HostCallFixture[];
`;
}

function generatedRust() {
  const rustStringArray = (name, values) =>
    `#[rustfmt::skip]\npub const ${name}: &[&str] = &[\n${values
      .map((value) => `    ${JSON.stringify(value)},`)
      .join("\n")}\n];`;
  return `// Generated from contracts/plugin-api-v1.json. Do not edit directly.
// Run \`pnpm contract:generate\` after changing the contract.

#[rustfmt::skip]
pub const HOST_API_VERSION: &str = ${JSON.stringify(contract.hostApiVersion)};
#[rustfmt::skip]
pub const HOST_CONTRACT_VERSION: u32 = ${contract.contractVersion};
#[rustfmt::skip]
pub const HOST_ERROR_ENVELOPE_VERSION: u32 = ${contract.errorEnvelopeVersion};
#[rustfmt::skip]
pub const HOST_MINIMUM_SUPPORTED_API_VERSION: &str = ${JSON.stringify(
    contract.stability.minimumSupportedApiVersion,
  )};
${rustStringArray(
  "HOST_ERROR_CODES",
  contract.errors.map((error) => error.code),
)}
${rustStringArray(
  "HOST_RETRYABLE_ERROR_CODES",
  contract.errors.filter((error) => error.retryable).map((error) => error.code),
)}
${rustStringArray(
  "HOST_CALL_NAMES",
  contract.calls.map((call) => call.name),
)}
${rustStringArray(
  "HOST_EVENT_NAMES",
  contract.events.map((event) => event.name),
)}
${rustStringArray("HOST_FEATURE_NAMES", contract.features)}
${rustStringArray(
  "HOST_FAMILY_NAMES",
  contract.families.map((family) => family.name),
)}
${rustStringArray(
  "HOST_FAILURE_STATES",
  contract.failureStates.map((failure) => failure.state),
)}
${rustStringArray(
  "HOST_FAILURE_STATE_CODES",
  contract.failureStates.map((failure) => failure.code),
)}
`;
}

function markdownCell(value) {
  return String(value).replaceAll("|", "\\|").replaceAll("\n", " ");
}

function jsonBlock(value) {
  return `\`\`\`json\n${JSON.stringify(value, null, 2)}\n\`\`\``;
}

function generatedApiReference() {
  const familyNames = contract.families.map((family) => family.name);
  const failureStates = contract.failureStates
    .map((failure) => `\`${failure.state}\` -> \`${failure.code}\``)
    .join(", ");
  const lines = [
    "# Steward Plugin API Reference",
    "",
    "> Generated from `contracts/plugin-api-v1.json`. Do not edit this file directly.",
    "> Run `pnpm contract:generate` after changing the contract.",
    "",
    `Host API version: \`${contract.hostApiVersion}\`. Contract version: \`${contract.contractVersion}\`.`,
    `Stable API line: \`${contract.stability.line}\`. Minimum supported host API: \`${contract.stability.minimumSupportedApiVersion}\`.`,
    "",
    "This reference covers the command-oriented Steward bridge. NSH UI primitives and",
    "standard NSH modules are documented separately in [nsh-plugin-api.md](nsh-plugin-api.md).",
    "",
    "## Versioning",
    "",
    `- Patch: ${contract.stability.policy.patch}`,
    `- Minor: ${contract.stability.policy.minor}`,
    `- Major: ${contract.stability.policy.major}`,
    `- Deprecation: ${contract.stability.policy.deprecation}`,
    "",
    "## Failure Semantics",
    "",
    `Every API family maps the same failure states to stable codes: ${failureStates}.`,
    "A rejection is delivered as the versioned JSON error envelope documented in",
    "[plugin-api-contract.md](plugin-api-contract.md). Plugins must branch on `code`, not prose.",
    "",
    "| State | Code | Meaning |",
    "| --- | --- | --- |",
    ...contract.failureStates.map(
      (failure) =>
        `| \`${failure.state}\` | \`${failure.code}\` | ${markdownCell(failure.meaning)} |`,
    ),
    "",
    "## Calls",
  ];

  for (const familyName of familyNames) {
    const family = contract.families.find((candidate) => candidate.name === familyName);
    const calls = contract.calls.filter((call) => call.family === familyName);
    lines.push(
      "",
      `### ${familyName[0].toUpperCase()}${familyName.slice(1)}`,
      "",
      family.description,
      "",
      "| Quota | Limit | Exceeded behavior |",
      "| --- | --- | --- |",
      ...family.quotas.map(
        (quota) =>
          `| ${markdownCell(quota.subject)} | ${markdownCell(quota.limit)} | ${markdownCell(quota.behavior)} |`,
      ),
      "",
      `Maximum host-operation deadline: ${family.maxOperationTimeoutMs} ms.`,
    );
    for (const call of calls) {
      lines.push(
        "",
        `#### \`${call.name}\``,
        "",
        call.description,
        "",
        `- Transport: \`${call.transport}\``,
        `- Capability: \`${call.capability}\``,
        "",
        "Request example:",
        "",
        jsonBlock(call.request.examples[0]),
        "",
        "Response example:",
        "",
        jsonBlock(call.response.examples[0]),
      );
    }
  }

  lines.push("", "## Events", "");
  for (const event of contract.events) {
    lines.push(
      `### \`${event.name}\``,
      "",
      event.description,
      "",
      jsonBlock(event.payload.examples[0]),
      "",
    );
  }

  lines.push(
    "## Stable Errors",
    "",
    "| Code | Retryable | Meaning |",
    "| --- | --- | --- |",
    ...contract.errors.map(
      (error) =>
        `| \`${error.code}\` | ${error.retryable ? "yes" : "no"} | ${markdownCell(error.description)} |`,
    ),
    "",
    "## Deprecations",
    "",
  );
  if (contract.stability.deprecations.length === 0) {
    lines.push("No public API entries are deprecated in the 1.x line.");
  } else {
    for (const deprecation of contract.stability.deprecations) {
      lines.push(
        `- \`${deprecation.name}\` is deprecated since \`${deprecation.deprecatedIn}\`, ` +
          `is replaced by \`${deprecation.replacement}\`, and is scheduled for removal in ` +
          `\`${deprecation.removeIn}\`.`,
      );
    }
  }

  return `${lines.join("\n").trim()}\n`;
}

function validateContract() {
  assertUnique(
    contract.calls.map((call) => call.name),
    "calls",
  );
  assertUnique(
    contract.events.map((event) => event.name),
    "events",
  );
  assertUnique(
    contract.errors.map((error) => error.code),
    "errors",
  );
  assertUnique(contract.features, "features");
  assertUnique(
    contract.capabilities.map((capability) => `${capability.permission}\0${capability.scope}`),
    "capabilities",
  );
  assertUnique(
    contract.families.map((family) => family.name),
    "API families",
  );
  assertUnique(
    contract.failureStates.map((failure) => failure.state),
    "failure states",
  );
  semanticVersion(contract.hostApiVersion, "hostApiVersion");
  semanticVersion(contract.stability.stableSince, "stability.stableSince");
  semanticVersion(
    contract.stability.minimumSupportedApiVersion,
    "stability.minimumSupportedApiVersion",
  );
  if (contract.stability.stableSince !== contract.hostApiVersion) {
    fail("the new stability line must start at the current host API version");
  }
  if (
    compareSemanticVersions(
      contract.stability.minimumSupportedApiVersion,
      contract.hostApiVersion,
    ) > 0
  ) {
    fail("minimumSupportedApiVersion cannot be newer than hostApiVersion");
  }
  for (const policy of ["patch", "minor", "major", "deprecation"]) {
    if (!contract.stability.policy?.[policy]?.trim()) {
      fail(`stability policy is missing ${policy}`);
    }
  }
  for (const deprecation of contract.stability.deprecations) {
    for (const field of ["name", "deprecatedIn", "replacement", "removeIn"]) {
      if (!deprecation[field]) fail(`deprecation is missing ${field}`);
    }
    semanticVersion(deprecation.deprecatedIn, `${deprecation.name}.deprecatedIn`);
    semanticVersion(deprecation.removeIn, `${deprecation.name}.removeIn`);
    if (compareSemanticVersions(deprecation.deprecatedIn, deprecation.removeIn) >= 0) {
      fail(`${deprecation.name} removal version must follow its deprecation version`);
    }
  }
  const failureCodes = new Set(contract.errors.map((error) => error.code));
  for (const failure of contract.failureStates) {
    if (!failureCodes.has(failure.code)) {
      fail(`failure state ${failure.state} uses unknown error code ${failure.code}`);
    }
    if (!failure.meaning?.trim()) fail(`failure state ${failure.state} has no meaning`);
  }
  const familyNames = new Set(contract.families.map((family) => family.name));
  for (const family of contract.families) {
    if (!family.description?.trim()) fail(`family ${family.name} has no description`);
    if (!Number.isSafeInteger(family.maxOperationTimeoutMs) || family.maxOperationTimeoutMs <= 0) {
      fail(`family ${family.name} has an invalid maximum timeout`);
    }
    if (!Array.isArray(family.quotas) || family.quotas.length === 0) {
      fail(`family ${family.name} has no quotas`);
    }
    for (const quota of family.quotas) {
      for (const field of ["subject", "limit", "behavior"]) {
        if (!quota[field]?.trim()) {
          fail(`family ${family.name} quota is missing ${field}`);
        }
      }
    }
  }

  const capabilityNames = new Set(contract.capabilities.map((capability) => capability.permission));
  if (!contract.errorEnvelope?.examples?.[0]) {
    fail("errorEnvelope has no example");
  }
  if (contract.errorEnvelope.properties?.version?.const !== contract.errorEnvelopeVersion) {
    fail("errorEnvelope version must match errorEnvelopeVersion");
  }
  validateExample(contract.errorEnvelope, contract.errorEnvelope.examples[0], "errorEnvelope");
  const schemaErrorCodes = contract.errorEnvelope.properties?.code?.enum ?? [];
  if (
    schemaErrorCodes.length !== contract.errors.length ||
    schemaErrorCodes.some((code, index) => code !== contract.errors[index].code)
  ) {
    fail("errorEnvelope code enum must match the errors list");
  }
  for (const call of contract.calls) {
    if (!call.description?.trim()) fail(`${call.name} has no generated reference description`);
    if (!familyNames.has(call.family)) fail(`${call.name} uses unknown family ${call.family}`);
    if (!["call", "post"].includes(call.transport)) {
      fail(`${call.name} has invalid transport ${call.transport}`);
    }
    if (!capabilityNames.has(call.capability)) {
      fail(`${call.name} uses undeclared capability ${call.capability}`);
    }
    for (const [kind, schema] of [
      ["request", call.request],
      ["response", call.response],
    ]) {
      if (!Array.isArray(schema.examples) || schema.examples.length === 0) {
        fail(`${call.name} has no ${kind} example`);
      }
      schema.examples.forEach((example, index) =>
        validateExample(schema, example, `${call.name}.${kind}.examples[${index}]`),
      );
    }
  }
  for (const event of contract.events) {
    if (!event.description?.trim()) {
      fail(`${event.name} has no generated reference description`);
    }
    if (!Array.isArray(event.payload.examples) || event.payload.examples.length === 0) {
      fail(`${event.name} has no payload example`);
    }
    validateExample(event.payload, event.payload.examples[0], `${event.name}.payload`);
  }

  const contractCalls = new Set(contract.calls.map((call) => call.name));
  const sdkCalls = literalHostCalls(readFileSync(sdkPath, "utf8"));
  assertUnique(
    sdkCalls.map((call) => call.name),
    "SDK host calls",
  );
  for (const call of sdkCalls) {
    if (!contractCalls.has(call.name)) fail(`SDK calls unregistered host call ${call.name}`);
    const expected = contract.calls.find((candidate) => candidate.name === call.name);
    if (expected.transport !== call.transport) {
      fail(
        `${call.name} is ${call.transport} in the SDK but ${expected.transport} in the contract`,
      );
    }
  }
  const sdkCallNames = new Set(sdkCalls.map((call) => call.name));
  for (const call of contract.calls) {
    if (!call.name.startsWith("steward.preferences.") && !sdkCallNames.has(call.name)) {
      fail(`contract call ${call.name} is not emitted by the TypeScript SDK`);
    }
  }

  const handlers = registeredHandlers(
    readFileSync(handlersPath, "utf8"),
    readFileSync(capabilitiesPath, "utf8"),
  );
  assertUnique(handlers, "Rust host handlers");
  for (const name of contractCalls) {
    if (!handlers.includes(name)) fail(`Rust host does not register contract call ${name}`);
  }
  for (const name of handlers) {
    if (!contractCalls.has(name)) fail(`Rust host registers unregistered call ${name}`);
  }
  for (const path of hostRejectionPaths) {
    const source = readFileSync(resolve(root, path), "utf8");
    if (source.includes(".reject(")) {
      fail(`${path} bypasses structured host errors with .reject()`);
    }
  }

  validateCompatibility(sdkCalls);
}

validateContract();

function formatted(contents, path) {
  if (path.endsWith(".rs")) {
    return contents;
  }
  return execFileSync(process.execPath, [prettierCli, "--stdin-filepath", relative(root, path)], {
    cwd: root,
    input: contents,
    encoding: "utf8",
  });
}

const outputs = [
  [generatedTsPath, generatedTypeScript()],
  [generatedRustPath, generatedRust()],
  [apiReferencePath, generatedApiReference()],
].map(([path, contents]) => [path, formatted(contents, path)]);

if (write) {
  for (const [path, contents] of outputs) {
    writeFileSync(path, contents);
  }
  console.log(`Generated ${outputs.map(([path]) => relative(root, path)).join(", ")}.`);
} else {
  let stale = false;
  for (const [path, expected] of outputs) {
    let actual = "";
    try {
      actual = readFileSync(path, "utf8");
    } catch {
      stale = true;
    }
    if (actual !== expected) {
      stale = true;
      console.error(`${relative(root, path)} is out of date. Run pnpm contract:generate.`);
    }
  }
  if (stale) process.exitCode = 1;
  else
    console.log(
      "Plugin API contract, generated reference, SDK calls, compatibility fixtures, and host handlers agree.",
    );
}
