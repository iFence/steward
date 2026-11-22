# Steward Plugin API Contract

> Status: PDX-3 complete. Host API `1.0.0`, stable line `1.x`, contract
> version `1`.

`contracts/plugin-api-v1.json` is the single machine-readable source of truth
for the public plugin bridge. It defines:

- host call names, bridge families, transports, and capability scopes;
- request and response JSON Schemas with committed SDK fixtures;
- lifecycle event payloads;
- stable public error codes and retryability;
- base host feature names;
- API families, platform quotas, failure semantics, and deprecation policy.

Generated TypeScript and Rust constants are committed:

- `packages/extension-api/src/contract.generated.ts`;
- `crates/plugin-contract/src/host_contract.generated.rs`.

The complete call, event, error, quota, and failure reference is generated at:

- `docs/plugin-api-reference.md`.

Run `pnpm contract:check` to verify that the generated TypeScript, Rust, and
reference outputs are current, the TypeScript SDK emits every registered call
with the declared transport, every Rust host handler is registered, no handler
bypasses the structured-error helper, all contract examples match their
declared schema, every family has a quota and stable failure mapping, and every
supported prior API line has compatibility fixtures accepted by the current
host DTOs.

Run `pnpm contract:generate` after editing the contract. Do not edit generated
files by hand.

## Cross-Layer Fixtures

The same first example from each call's `request` and `response` schema is
committed in the contract and compiled into the generated TypeScript fixture
set. These tests consume it:

- `@steward/api-contract-harness` invokes the real SDK helpers through NSH's
  headless host bridge and asserts the exact names and payloads observed by the
  embedding application;
- `steward-plugin-contract` deserializes every request and response fixture
  through shared Rust DTOs;
- `steward-app` feeds the request fixtures into the same Rust parsers used by
  the registered plugin-host handlers;
- `pnpm contract:check` cross-checks call registrations against the SDK and
  launcher source.

Removing, renaming, or changing the transport of a public call therefore fails
CI without requiring a live signed plugin launch.

## Structured Errors

NSH delivers a rejected host call as an `Error` whose message is a string.
Steward places a JSON object in that message:

```json
{
  "version": 1,
  "code": "permission_denied",
  "message": "clipboard write is not granted",
  "retryable": false
}
```

`@steward/extension-api` parses the JSON envelope directly and constructs
`StewardError` from its fields. It does not infer codes from prose or regular
expressions. A rejection that is not a valid Steward envelope falls back to
`host_failure`.

The stable codes are declared in the contract and generated into both language
surfaces. The JSON field names, envelope version, and code set are data covered
by tests; rejection prose is not part of the compatibility contract.

## Process Request IDs

The public `runProcess()` API accepts a caller-facing string ID with 1 to 128
characters from `[A-Za-z0-9._:-]`. The launcher-owned execution broker and its
wire protocol use a positive numeric request ID so a plugin host cannot address
arbitrary launcher state with an object or ambiguous scalar.

The SDK derives the numeric ID with a deterministic two-lane 53-bit mapping:

1. Start `high` at `0x811c9dc5` and `low` at `0x9e3779b9`.
2. For each UTF-16 code unit at index `i`, XOR `high` with the code unit and
   multiply by `0x01000193`; XOR `low` with `codeUnit + i` and multiply by
   `0x85ebca6b`.
3. Mask `high` to 21 bits and return `high * 2^32 + low`, using `1` if the
   result is zero.

The result is always a positive JavaScript safe integer. The same public ID
maps to the same numeric ID for both `runProcess()` and `cancelProcess()`.
`ProcessResult.requestId` returns the original public string.

This mapping is a collision-resistant correlation function, not a unique
identifier or a security boundary. A collision is possible. Collision behavior
is explicitly defined:

- the plugin host tracks pending requests by numeric ID for that plugin
  process only;
- a second concurrent request with an already-active numeric ID is rejected
  with `invalid_input` before a process is queued or spawned;
- the first active request is not replaced, merged, or cancelled;
- cancellation applies only to the currently active request with that numeric
  ID, so a rejected colliding request cannot redirect cancellation;
- SDK-generated IDs include a timestamp and random suffix to make practical
  concurrent collisions negligible;
- callers that provide custom IDs and intentionally run overlapping operations
  must choose values that do not collide under this mapping.

The host never interprets the public string ID and does not expose the numeric
mapping as a process handle.
