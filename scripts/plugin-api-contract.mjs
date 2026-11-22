#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const contractPath = resolve(root, "contracts/plugin-api-v1.json");
const generatedTsPath = resolve(root, "packages/extension-api/src/contract.generated.ts");
const generatedRustPath = resolve(root, "crates/plugin-contract/src/host_contract.generated.rs");
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
    if (schema.maxItems !== undefined && value.length > schema.maxItems) {
      fail(`${location} example has more than ${schema.maxItems} items`);
    }
    if (schema.items) {
      value.forEach((item, index) => validateExample(schema.items, item, `${location}[${index}]`));
    }
  }
  if (value && actualType === "object") {
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

function generatedTypeScript() {
  const calls = contract.calls.map((call) => ({
    name: call.name,
    family: call.family,
    capability: call.capability,
    transport: call.transport,
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
`;
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
].map(([path, contents]) => [path, formatted(contents, path)]);

if (write) {
  for (const [path, contents] of outputs) {
    writeFileSync(path, contents);
  }
  console.log(`Generated ${outputs.map(([path]) => relative(root, path)).join(" and ")}.`);
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
  else console.log("Plugin API contract, generated surfaces, SDK calls, and host handlers agree.");
}
