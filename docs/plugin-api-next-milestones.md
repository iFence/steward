# Steward Plugin API Next Milestones

> Status: PDX-0 complete. PDX-1 implementation is complete and CI-gated; its
> first public SDK artifact will be attached by the next tagged release. Current
> baseline is the command-oriented plugin platform in
> [plugin-capability-roadmap.md](plugin-capability-roadmap.md), with the
> `runProcess` request-ID mismatch repaired on 2026-11-22.
>
> Goal: turn the current internal/official-plugin workflow into a reliable,
> distributable, externally testable plugin development API without weakening
> Steward's signed-package, capability, and process-isolation boundaries.

## Scope

The next stage targets command-oriented UI plugins:

- a developer can create a plugin outside the Steward repository;
- the plugin can use the published SDK and a supported template;
- local development covers type checking, headless tests, preview, signed
  integration, logs, and failure recovery;
- SDK and host versions have an explicit compatibility contract;
- a third-party package can pass automated admission and be installed,
  updated, rolled back, revoked, and removed without changing Steward source.

Background services, dynamic launcher search providers, cross-plugin IPC, and
arbitrary WebView/process access remain outside this stage. They require a
separate reviewed design after the command-oriented API is stable.

## Dependency Order

```text
PDX-0 Contract reliability
  |
  +-- PDX-1 SDK distribution and external onboarding
        |
        +-- PDX-2 Developer loop
              |
              +-- PDX-3 Public API v1 hardening
                    |
                    +-- PDX-4 Third-party admission and ecosystem operations
                          |
                          +-- PDX-5 Deferred capability expansion
```

PDX-0 is a release blocker for every later milestone. PDX-1 and PDX-2 may be
developed in parallel only after the PDX-0 contract tests are in CI.

## PDX-0: Contract Reliability

Status: complete on 2026-11-22.

Objective: eliminate SDK-to-host drift and make every advertised host call
testable before exposing it to external developers.

### Completed

- Align `runProcess()` with the host wire contract by sending a positive safe
  integer request ID while preserving the public string ID for callers.
- Add an official-plugin regression check that inspects the actual
  `steward.exec.run` payload produced by `@steward/extension-api`.
- Add a Rust parser regression test for the numeric request-ID shape used by
  the SDK.

### Required Work

- [x] PDX-0.1: `contracts/plugin-api-v1.json` defines host call names,
      request/response schemas and fixtures, events, errors, features, and
      capability scopes;
- [x] PDX-0.2: generated TypeScript and Rust constants are checked in CI, while
      shared Rust DTOs validate the committed payload fixtures;
- [x] PDX-0.3: `@steward/api-contract-harness` observes every SDK wire payload
      through NSH's real host bridge, and `steward-app` feeds the same fixtures
      through its registered host-call parsers;
- [x] PDX-0.4: all launcher host rejections use the versioned structured error
      envelope; the SDK parses JSON directly without regular-expression inference;
- [x] PDX-0.5: `pnpm test:plugins` discovers and tests every official plugin,
      including Network Check, while the contract harness exercises each bridge
      family and every public call;
- [x] PDX-0.6: `docs/plugin-api-contract.md` defines the numeric process-ID
      mapping and concurrent collision behavior.

### Acceptance Gate

- no public SDK call can construct a payload rejected by the matching host
  handler;
- removing or renaming a host call fails a generated-contract check;
- TypeScript and Rust tests use the same committed contract fixtures;
- all official plugins run in CI;
- stable error codes are asserted as data, not inferred from prose alone.

## PDX-1: SDK Distribution And External Onboarding

Status: implementation complete and CI-gated on 2026-11-22. The first public
SDK tarball is attached by the release workflow with the next tagged Steward
release.

Objective: make a new plugin project possible without cloning the Steward
monorepo.

### Required Work

- [x] PDX-1.1: build `@steward/extension-api` into distributable JavaScript and
      declaration files with explicit package exports;
- [x] PDX-1.2: publish versioned SDK artifacts and document the registry/location
      developers should use;
- [x] PDX-1.3: publish a create-plugin command or template with `App.tsx`,
      `plugin.json`, `nsh.json`, `package.json`, `tsconfig.json`, a headless test,
      and a capability example;
- [x] PDX-1.4: make scaffolded projects depend on a released SDK version rather
      than `workspace:*` or raw source paths;
- [x] PDX-1.5: add an external fixture project outside the pnpm workspace and run
      install, typecheck, test, build, sign, package, and install in CI;
- [x] PDX-1.6: document the Host API version, SDK version, NSH version, and
      compatibility window in one developer-facing matrix.

Evidence:

- `pnpm sdk:pack` emits `target/sdk/steward-extension-api-0.5.0.tgz` and a
  SHA-256 manifest after validating the compatibility matrix and package
  contents;
- `steward-plugin new` generates the interactive, host-call-tested template
  and defaults to the versioned release tarball;
- `fixtures/pdx1-external-plugin` installs only the packaged SDK and is checked
  by `pnpm test:pdx1` through the complete signed-package lifecycle;
- `docs/plugin-sdk-distribution.md` is the developer-facing registry,
  scaffold, artifact, and compatibility reference.

### Acceptance Gate

- a clean Windows environment can create and build a plugin using only
  documented public prerequisites;
- the external fixture imports no source file from the Steward checkout;
- package installation is reproducible from the released SDK artifact;
- the template's generated test proves both a UI interaction and a mocked
  host call.

## PDX-2: Developer Loop

Status: implementation complete and CI-gated on 2026-11-22. Depends on PDX-0.

Objective: reduce the edit-build-sign-install-restart loop to a predictable
developer workflow.

### Required Work

- [x] PDX-2.1: add a developer mode that uses an isolated data directory, plugin
  directory, and development trust root without affecting the installed app;
- [x] PDX-2.2: support selecting a local plugin zip or package directory for
  installation from Settings > Plugins, matching the CLI behavior;
- [x] PDX-2.3: provide one command for rebuild, package, replace, and reopen;
- [x] PDX-2.4: provide a static-bundle reload fallback when NSH GPUI hot reload is
  unavailable or fails;
- [x] PDX-2.5: expose plugin-scoped logs, first-frame time, host-call failures,
  denied capabilities, process exits, and recent crash details in a developer
  diagnostics surface;
- [x] PDX-2.6: include capability-call tracing with secrets and request payloads
  omitted by default;
- [x] PDX-2.7: add a canonical "plugin failed to launch" doctor command that checks
  IDs, versions, trust roots, capabilities, manifest/package agreement, and
  package layout.

Evidence:

- `steward-app --developer-mode` resolves a separate data, plugin, trust, and
  instance profile before storage initialization;
- `steward-plugin --developer-mode dev <project>` rebuilds, signs, packages,
  validates, replaces, and queues a static reload;
- Settings installs ZIP archives and exact two-file directories through the
  shared store path;
- Settings exposes scoped first-frame, host-call, capability, exit, crash, and
  log diagnostics without request payloads;
- `steward-plugin doctor <id-or-source>` reports the failing launch stage;
- `pnpm test:pdx2` covers isolated profile resolution, same-version replacement,
  static reload requests, doctor pass/failure, and production-profile isolation.

### Acceptance Gate

- a source edit can be seen in a running development plugin without manually
  signing and editing environment variables;
- a plugin failure identifies the responsible plugin and failure stage from a
  developer-visible diagnostic;
- Settings and CLI install the same local package with the same validation;
- development mode cannot weaken production trust or grant policy.

## PDX-3: Public API v1 Hardening

Status: planned. Depends on PDX-1 and PDX-2.

Objective: declare a stable plugin API line with explicit compatibility and
support guarantees.

### Required Work

- PDX-3.1: freeze the command-oriented public API surface for a `1.0` host
  contract, including launch data, events, errors, preferences, feedback,
  navigation, storage, clipboard, filesystem, system-open, network, and process
  calls;
- PDX-3.2: add compatibility fixtures for every supported prior API line;
- PDX-3.3: define deprecation timelines, patch/minor guarantees, and migration
  guidance for manifest and SDK changes;
- PDX-3.4: generate the API reference from the contract and remove duplicated
  hand-maintained call tables;
- PDX-3.5: define platform quotas and failure behavior for every API family;
- PDX-3.6: test disabled, revoked, expired, malformed, timed-out, cancelled,
  and resource-exhausted paths for each host family;
- PDX-3.7: record the same-user security boundary and unsupported capabilities
  in the public developer documentation.

### Acceptance Gate

- an older supported SDK still builds and launches against the new host;
- a newer package fails with a stable `feature_unavailable` or host-version
  error instead of failing during runtime calls;
- every public API has generated reference text and a contract test;
- deprecations have a documented removal version and replacement.

## PDX-4: Third-Party Admission And Ecosystem Operations

Status: planned. Depends on PDX-3 and the Phase 3 clean-VM gate.

Objective: accept external packages into a public distribution channel with
auditable publisher and revocation operations.

### Required Work

- PDX-4.1: validate plugins from at least three independent publishers and
  unrelated domains outside the Steward monorepo;
- PDX-4.2: complete clean-Windows-VM acceptance for signed install, update,
  crash containment, rollback, revocation, and uninstall;
- PDX-4.3: implement hosted publisher registration, release-key rotation,
  recovery, and operator audit records;
- PDX-4.4: implement public marketplace quarantine and root-signed revocation
  operations with defined ownership and incident procedures;
- PDX-4.5: require public admission reports and capability deltas in release
  review;
- PDX-4.6: publish support expectations, security reporting, and publisher
  dispute procedures.

### Acceptance Gate

- all clean-VM lifecycle scenarios pass with production-signed artifacts;
- independent plugins exercise filesystem, network, process, preferences, and
  navigation capabilities without host-side business-specific code;
- a publisher key can be rotated or revoked without breaking unrelated
  publishers;
- a bad package can be quarantined and revoked from the public channel.

## PDX-5: Deferred Capability Expansion

Status: deferred until PDX-0 through PDX-4 are complete.

Objective: expand beyond command-oriented UI plugins only when a reviewed
workload demonstrates that the existing capability model is insufficient.

Candidate work, in evaluation order:

- secure secret storage and credential lifecycle;
- binary file read/write and host-owned Recycle Bin operations;
- file and clipboard change subscriptions;
- richer notifications and progress surfaces;
- streaming or resumable network transfer;
- dynamic launcher results and search providers;
- background service lifecycle and cross-plugin communication;
- additional packaged runtimes such as WASM.

Each candidate requires:

- at least two unrelated plugin workloads;
- a capability and consent model;
- resource limits and cancellation semantics;
- audit behavior without sensitive payload storage;
- process-isolation analysis;
- migration and compatibility tests.

## Immediate Work Queue

1. Attach the PDX-1 SDK artifact and checksum to the next tagged Steward
   release, then verify the documented public download on a clean Windows
   environment.
2. Run the PDX-2 developer workflow on a clean Windows VM with the released app
   and SDK before inviting external beta testers.

## Non-Goals For This Stage

- in-process plugin code inside the launcher shell;
- unrestricted filesystem or shell access;
- public marketplace launch before the clean-VM and publisher-operation gates;
- dynamic result providers or background VMs;
- compatibility with Raycast or Node extension APIs.
