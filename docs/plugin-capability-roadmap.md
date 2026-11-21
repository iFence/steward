# Steward Plugin Capability Roadmap

> Status: PC-0 through PC-2 are implemented. PC-3 is next. This document defines a
> general third-party plugin capability roadmap. `git-repos` is one reference
> workload used to validate the platform, not the boundary of the API design.
>
> This work belongs to Phase 4 platform admission and follows the completed M5
> local plugin ecosystem. It must also be read together with
> [plugin-platform-refactor.md](plugin-platform-refactor.md) and
> [plugin-ecosystem.md](plugin-ecosystem.md).

## 1. Goals

Steward should let a signed third-party TSX plugin:

- render native NSH UI without modifying Steward source code;
- declare and request narrow capabilities;
- receive explicit user consent before accessing sensitive resources;
- use stable, versioned SDK contracts;
- behave predictably across install, update, rollback, and uninstall;
- remain isolated from the launcher when it fails or exceeds resource limits.

Platform APIs should be reusable across unrelated plugin domains. Loading local
data, calling an API, opening a system resource, running an approved tool, and
presenting a page should not require a plugin-specific host service.

### Design principles

1. Prefer generic capabilities over domain-specific business APIs.
2. Grant access to explicit resources, not broad implementation privileges.
3. Keep permission declarations separate from user grants and runtime checks.
4. Keep expensive work asynchronous and outside the first-render path.
5. Preserve process isolation unless a reviewed embedded-mode design replaces
   it with an equally strong boundary.
6. Use representative plugins from different domains to validate each public
   capability.

### Reference workload: `git-repos`

`git-repos` is the first demanding integration example because it exercises
several platform concerns together. A complete implementation may include:

- scan directories selected by the user for Git repositories;
- filter, favorite, and rank repositories;
- identify ordinary repositories, worktrees, and optional submodules;
- open a repository directory in Explorer or an editor;
- open a remote repository page in the default browser;
- clone a repository into a selected directory;
- refuse unsafe removal, and move a safe repository to the Recycle Bin.

This workload validates the general filesystem, system-open, process, preference,
and navigation capabilities below. Steward should not add `git-repos` business
logic to the shell, and it should not require a Git-specific host API for the
baseline implementation.

Steward does not need to reproduce every Raycast feature. Other validation
workloads should cover network APIs, document viewers, media or file utilities,
and tools that operate on user-selected projects.

## 2. Current Stack Audit

| Area                         | Current state                                                                                                               | General consequence                                                                                                             |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| NSH UI and QuickJS           | NSH 0.6.1 TSX, signals, inputs, navigation primitives, and tests are available                                              | Third-party plugin UI can be built now                                                                                          |
| Package lifecycle            | Scaffold, inspect, install, update, remove, rollback, local marketplace, and policy checks are implemented                  | Distribution and iteration are ready                                                                                            |
| Process isolation            | One plugin host process per open plugin, disposable child process, Job Object limits, and crash containment are implemented | A plugin crash does not take down the shell                                                                                     |
| Capability storage           | Persistent grants, revocation, audit records, and Settings allow/revoke controls are implemented                            | Grants are resolved for every plugin launch and scoped by publisher, version, capability, and resource                          |
| Release policy               | Deny-by-default ceiling intersected with active user grants; legacy wildcard only for unupdated official installs           | Selected filesystem roots and system-open are available; process execution, network fetch, and unrestricted roots remain denied |
| Host bridge                  | Clipboard, storage, calendar, preferences, feedback, scoped filesystem, application discovery, and system-open calls        | No process, network, or navigation API                                                                                          |
| Host permission granularity  | Official plugins now use named host calls; unupdated installs retain legacy `steward.*`                                     | Third-party packages cannot declare wildcard host permissions                                                                   |
| Plugin manifest              | Strict schema v2 with routing commands, optional `minHostVersion`, and hosted preference schema                             | No presentation mode                                                                                                            |
| Plugin window                | The plugin host child opens its own GPUI window                                                                             | The launcher and a plugin cannot currently share one OS window                                                                  |
| Process containment          | Each plugin process has an active-process limit of one                                                                      | A plugin host cannot directly spawn a helper, including `git.exe`, without changing containment                                 |
| Public marketplace admission | Not started                                                                                                                 | Public third-party publication remains blocked                                                                                  |

The current stack is sufficient for UI-only plugins and the local lifecycle.
It is not sufficient for general third-party system integration; `git-repos`
exposes several of those missing capabilities in one workload.

## 3. Third-Party Plugin Baseline

The following capability groups are the minimum practical baseline for
third-party plugins. They are listed in dependency order.

### 3.1 Versioned public contract

- A semantic host API version and minimum-host requirement.
- Feature discovery so plugins can degrade when a host capability is absent.
- Stable error codes for denied permission, unavailable feature, timeout,
  resource exhaustion, and host failure.
- A versioning policy for TypeScript helpers, host event payloads, and manifest
  schema changes.
- Automated compatibility tests against the previous supported SDK version.

### 3.2 Granular permissions and user grants

- Replace `steward.*` with named host permissions for third-party plugins.
- Keep wildcard permission compatibility only for existing official plugins
  during migration.
- Scope grants by plugin, publisher, version range, capability, and resource.
- Require renewed user consent when an update requests a new sensitive
  capability.
- Show user-readable explanations for each capability.
- Audit every allowed and denied high-risk call.
- Support grant revocation while Steward is running.

### 3.3 Plugin preferences

- A declarative preference schema for text, boolean, select, folder, and
  application fields.
- Defaults, validation, and migration between package versions.
- Plugin-private persistence without exposing other plugins' settings.
- A Steward-hosted settings surface for common preference types, while still
  allowing plugins to render custom NSH settings.

### 3.4 User-selected filesystem access

- File and folder pickers.
- Read-only and read-write grants for user-selected roots.
- Canonical path validation before every operation.
- No traversal through symbolic links or Windows reparse points outside a
  grant.
- A generic bounded directory-walk API to avoid thousands of individual
  `readdir` round trips.
- Result limits, cancellation, and a scan deadline.
- Persistent and revocable grants stored in the existing capability tables.
- Private `${dataDir}` access remains available and does not need a prompt.

### 3.5 System resource opening

- Open a file or directory with its registered default handler.
- Reveal a path in File Explorer.
- Open a path with an application selected by the user.
- Open an installed application through a stable application identifier.
- Use the default browser for authorized `http`/`https` URLs.
- Never accept an arbitrary shell command line for these operations.

### 3.6 Network access

- HTTPS requests scoped by host, method, and optional path prefix.
- Redirect validation against the same grant.
- Explicit user approval for each domain group.
- No raw sockets, arbitrary ports, or local-network access in the initial
  third-party profile.
- Request body, response body, concurrency, and timeout limits.

### 3.7 Controlled process execution

- Exact executable allowlists rather than arbitrary shell access.
- User approval that identifies the executable and why it is needed.
- No shell interpolation.
- Argument count and length limits.
- A working directory confined to an authorized write root.
- Scrubbed environment, timeout, output limit, and cancellation.
- Process-tree termination when the plugin exits.
- Execution through a parent-owned capability broker or another mechanism that
  is not blocked by the plugin host's one-active-process limit.

### 3.8 User feedback

- Toast-style non-modal status.
- Confirmation dialogs for destructive or externally visible actions.
- Error presentation with a stable machine-readable code.
- Progress for long-running clone, download, and removal operations.

### 3.9 Presentation and navigation

- Static command, prefix, and regex routes are sufficient for the first
  third-party baseline.
- Optional windowed or embedded presentation selected by the plugin command.
- Back, close, page-title, and visibility lifecycle APIs.
- Keyboard focus transfer between the launcher and plugin pages.
- Destroy the plugin VM and host process when an embedded page is left.
- Inline launcher results and dynamic search providers are not required for the
  first third-party baseline or for the `git-repos` reference workload.

### 3.10 Diagnostics and operations

- Plugin-scoped structured logs.
- Capability and process-limit events in support diagnostics.
- Package, publisher, signature, grant, and rollback visibility in Settings.
- Clear recovery for a blocked or revoked plugin.

### 3.11 API shape rule

Public capability names should describe reusable resources and operations:

| Capability area | Preferred public shape                          | Avoid                        |
| --------------- | ----------------------------------------------- | ---------------------------- |
| Files           | `steward.fs.*` scoped to user-selected roots    | `steward.gitRepos.scan`      |
| System launch   | `steward.system.openPath`, `.openWith`          | `steward.editor.openProject` |
| Process         | `steward.exec.run` with an executable grant     | A generic shell              |
| Network         | `steward.net.fetch` with host/method/path scope | Unrestricted sockets         |
| Preferences     | `steward.preferences.*` with a declared schema  | Per-plugin host code         |
| Navigation      | `steward.navigation.*` for generic page changes | Git-specific window APIs     |

Domain behavior such as Git remote parsing, repository ranking, favorites, and
removal policy should remain in the plugin. A host-side bounded walk or process
broker is an infrastructure capability, not a Git-specific service.

## 4. Reference Capability Mapping

The following mapping is an example of how one workload consumes the general
capability model. New capabilities must be justified by more than one plugin
domain.

| `git-repos` requirement           | General platform capability                                   | Sufficient today                 |
| --------------------------------- | ------------------------------------------------------------- | -------------------------------- |
| Show repositories and favorites   | NSH UI and `${dataDir}` storage                               | Yes                              |
| Copy repository path              | Clipboard write                                               | Yes                              |
| Open GitHub/GitLab page           | Authorized `https` open                                       | Yes                              |
| Choose scan folders               | Folder picker and persistent path grants                      | Yes                              |
| Locate `.git` entries             | Bounded filesystem walk                                       | Yes                              |
| Read Git config files             | Scoped text-file read                                         | Yes                              |
| Detect submodules                 | Scoped file read plus plugin parsing                          | Yes                              |
| Open repository directory         | Open-path host capability                                     | Yes                              |
| Open repository in an editor      | Application selection and open-with host capability           | Yes                              |
| Check clean/unpushed state        | Controlled `git` execution                                    | No                               |
| Clone repository                  | Controlled `git` execution and network-capable process policy | No                               |
| Move repository to Recycle Bin    | Host-owned destructive filesystem operation                   | No                               |
| Enter plugin from launcher        | Existing command routing                                      | Yes, as a separate plugin window |
| Enter plugin on the launcher page | Embedded presentation and cross-process surface design        | No                               |

## 5. Milestones

Dependencies:

```text
PC-0 Contract and permissions
  |
  +-- PC-1 Preferences and feedback
        |
        +-- PC-2 Filesystem and system resources
              |
              +-- PC-3 Network and controlled execution
                    |
                    +-- PC-5 Third-party admission

PC-0
  |
  +-- PC-4 Embedded presentation
```

PC-4 is a parallel UX track. It is not required for ordinary windowed plugins,
but it is required for Raycast-like navigation.

### PC-0: Contract and Permission Foundation

Status: complete. Contract, package inspection, persisted-grant launch
enforcement, feature discovery, stable errors, update escalation reporting,
capability audit hooks, Settings grant/revocation controls, and migration of
all official plugins to named permissions are implemented.

Deliverables:

- Define host API semantic versions and `minHostVersion`.
- Introduce named permissions for all new high-risk calls.
- Reject `steward.*` wildcard host permission for new third-party packages.
- Connect launch policy to persisted user grants and revocation state.
- Add capability descriptions to package inspection.
- Require renewed approval when an update adds sensitive capabilities.
- Add runtime feature discovery and stable unsupported-feature errors.
- Document the compatibility policy.

Acceptance:

- A package cannot access a capability that has no active user grant.
- Revocation takes effect for new launches immediately.
- An update with a new capability cannot silently inherit the previous grant.
- Existing official plugins retain compatibility during migration.
- Grant, denial, and revocation events produce audit records.
- API compatibility tests cover manifest, host calls, and event payloads.

Implementation evidence (2026-11-21):

- Settings lists every declared capability, scope, explanation, and automatic,
  granted, denied, or legacy state, with Allow and Revoke actions.
- Grant and revoke operations run on the plugin-management worker, revalidate
  the current signed package and declaration, and use publisher plus a caret
  version range scoped to the installed version.
- Calendar `0.2.4`, Clipboard History `0.1.2`, and UI Showcase `1.0.2` declare
  `minHostVersion: "0.1.0"` and named host permissions.
- Host authorization and Settings capability tests cover automatic grants,
  named official grants, exact declaration matching, update denial, revocation,
  and audit records.

### PC-1: Preferences and User Feedback

Status: complete. Depends on PC-0.

Deliverables:

- Declarative plugin preferences with text, boolean, select, folder, and
  application fields.
- Steward-hosted preference editing for standard fields.
- Plugin-specific custom settings remain possible in NSH.
- Toast and confirmation APIs.
- Error objects with codes, user messages, and retryability.
- Progress events suitable for long-running clone and removal operations.

Acceptance:

- Preferences are isolated per plugin and survive updates.
- Invalid preference values fail validation rather than reaching host calls.
- Folder and application preference fields use the same consent and path rules
  as their runtime APIs.
- A destructive action can require confirmation before the host executes it.
- Headless plugin tests can mock every preference and feedback API.

Implementation evidence (2026-11-21):

- `plugin.json` v2 accepts a bounded `preferences` schema with text, boolean,
  select, folder, and application fields, defaults, validation, and key
  migrations.
- Preferences are stored by plugin ID in SQLite, survive updates, and are not
  shared between plugins. Settings renders the declared fields without loading
  a QuickJS VM.
- The host API is `0.2.0` and exposes resolved preferences in launch parameters,
  plus named `steward.preferences.*` and `steward.feedback.*` host calls.
- Toast, destructive confirmation, and progress APIs cross the isolated plugin
  process boundary. Confirmation is resolved by the launcher; progress is
  delivered back to the plugin as a stable event.
- UI Showcase declares and consumes all five preference kinds. Its headless NSH
  tests mock preferences, toast, confirmation denial, and progress events.

### PC-2: Scoped Filesystem and System Resources

Status: complete. Depends on PC-1.

Deliverables:

- Folder and file picker integration.
- Persistent read-only and read-write path grants.
- Bounded recursive directory walk with cancellation and result limits.
- Scoped file read and optional write APIs for selected roots.
- Canonicalization and reparse-point protection.
- Open path, reveal in Explorer, and open-with application APIs.
- Installed-application discovery for user-selected launch targets.

Acceptance:

- A plugin cannot read or list a path outside its active grants.
- A symlink, junction, or other reparse point cannot escape a grant.
- A user can revoke a root while the plugin remains installed.
- A cancelled or timed-out walk returns a structured error without blocking the
  plugin UI.
- Opening a path with an arbitrary shell command line is impossible.
- At least two unrelated plugins can use the same filesystem and system-open
  APIs; `git-repos` is one of the reference workloads.

Implementation evidence (2026-11-21):

- Host API `0.3.0` adds `steward.fs.pick`, `.walk`, `.readText`, `.writeText`,
  `.grants`, `.cancelWalk`, `steward.system.openPath`, `.revealPath`,
  `.openWith`, and `.applications`.
- Native folder/file dialogs create persistent read or read-write grants for
  exact canonical selections in the existing capability tables. Settings lists
  those dynamic roots and can revoke them while plugins remain installed.
- Every filesystem and system-open call reloads active grants and canonicalizes
  the requested path. Out-of-root, UNC, traversal, and escaped reparse-point
  paths fail closed. Walks skip reparse entries, enforce depth/result/deadline
  limits, and support cancellation without running on the plugin UI thread.
- Installed-application discovery reuses Steward's Start Menu/AppsFolder
  scanner. Open-with accepts only a catalog application ID and a selected path;
  it has no command-line argument.
- `packages/plugins/local-files` and `packages/plugins/git-repos` are unrelated
  headless-tested consumers of the same APIs. The Git workload discovers
  repositories, filters and favorites them, reads remotes, opens repository
  paths, and reveals them without Git-specific shell code.

### PC-3: Network and Controlled Execution

Status: planned. Depends on completed PC-2.

Deliverables:

- Scoped HTTPS fetch permissions.
- Redirect and response-size enforcement.
- Exact-executable permission grants.
- A parent-owned execution broker that is not constrained by the plugin host's
  one-active-process Job Object limit.
- Process timeout, output cap, cancellation, environment scrubbing, and
  process-tree cleanup.
- Generic approval for exact executables and working directories rather than
  adding domain-specific business logic to Steward.

Acceptance:

- A denied domain or executable fails before any I/O starts.
- Shell metacharacters in arguments are never interpreted by a shell.
- `git clone`, `git status`, `git remote`, and the unpushed-commit check work
  from the plugin host process through the broker.
- A timed-out or cancelled process tree leaves no orphaned child.
- Network and process calls produce capability audit entries.
- At least one network-focused plugin and one tool-integration plugin pass
  without adding domain-specific shell code; `git-repos` is one consumer.

### PC-4: Embedded Presentation and Navigation

Status: planned, technical design required. May run in parallel with PC-1
through PC-3.

Deliverables:

- Add an optional presentation mode to a new backward-compatible manifest schema.
- Define `windowed` and `embedded` behavior per command.
- Add back, close, title, and visibility lifecycle APIs.
- Define focus transfer and Esc behavior.
- Decide how an embedded plugin remains process-isolated.
- Destroy the plugin host after leaving the embedded page.
- Preserve the zero-hidden-plugin-process invariant.

A true single-window implementation cannot directly embed the current plugin
host child's `AppView`: the child owns a separate GPUI application and window.
The design milestone must choose one of:

1. a trusted first-party embedded mode that runs the plugin VM in the shell;
2. a cross-process render/surface bridge;
3. a closely managed frameless hosted window that visually matches the launcher.

Option 1 is simplest but weakens process isolation. Option 2 preserves isolation
but has the highest engineering cost. Option 3 is an interim UX approximation,
not a true single-window implementation.

Acceptance:

- Static command routing can open the selected presentation mode.
- Back and close restore launcher focus and destroy the plugin runtime.
- There are no hidden plugin VMs or host processes after leaving the page.
- Keyboard, IME, DPI, monitor changes, and window activation follow the
  documented acceptance matrix.
- Third-party plugins cannot bypass process isolation by requesting embedded
  mode.

### PC-5: Third-Party Admission

Status: blocked on PC-0 through PC-3 and the Phase 3 clean-VM gate.

Deliverables:

- Public capability-policy review.
- Publisher registration and revocation operating process.
- Plugin API version admission and compatibility checks.
- Permission display and re-consent during update.
- Marketplace policy that rejects undeclared or overly broad permissions.
- Automated checks for filesystem escapes, command injection, oversized
  output, and missing cancellation.
- Support diagnostics that identify the denied capability without exposing
  plugin-private data.

Acceptance:

- A third-party plugin can be scaffolded, tested, signed, installed, updated,
  rolled back, revoked, and uninstalled without modifying Steward source.
- External-style plugins from at least three domains jointly cover filesystem,
  network, process, preferences, and navigation capabilities.
- Phase 3 signed-package install, update, crash, and rollback acceptance passes
  on a clean Windows VM.
- The release review clearly documents the remaining same-user security
  boundary.

## 6. Validation Workloads

Validation workloads prove that no milestone has been designed around one
plugin's business model. One plugin may cover several rows, but every public
capability should have at least one consumer outside `git-repos` before public
admission.

| Workload                       | Capabilities exercised                          | Milestone         |
| ------------------------------ | ----------------------------------------------- | ----------------- |
| UI-only dashboard or converter | NSH UI, `${dataDir}`, private storage           | Existing          |
| API or feed reader             | Preferences, scoped HTTPS, pagination, errors   | PC-1 through PC-3 |
| Local project utility          | Folder grants, bounded walk, scoped file read   | PC-2              |
| `git-repos`                    | Folder grants, walk, open, process, preferences | PC-2 through PC-4 |
| Document or media inspector    | File picker, file read, default and open-with   | PC-2              |
| Embedded search or browser     | Presentation, navigation, and runtime teardown  | PC-4              |

For the `git-repos` workload specifically:

- after PC-2 it should support repository discovery, filtering, favorites, link
  opening, and Explorer launch;
- after PC-3 it should support editor launch, clone, clean/unpushed checks, and
  safe removal;
- after PC-4 it may support launcher-page entry instead of a separate window.

These are acceptance examples, not separate public APIs.

## 7. Deliberate Raycast Non-Goals

The first Steward implementation does not need:

- Raycast API compatibility;
- Raycast Quicklinks;
- an "Open in All Applications" action;
- installed-browser detection when the default browser already handles remote
  URLs;
- five hard-coded editor shortcuts;
- inline repository results in the launcher;
- dynamic plugin result providers;
- arbitrary web views or arbitrary native child windows;
- unrestricted filesystem roots or unrestricted command execution.

Repository search can remain inside the plugin page. Static `command`, `prefix`,
and `regex` routes are sufficient for `git repos` and many other plugins to
expose ordinary commands.

## 8. Security Invariants

- Release builds run verified signed packages only.
- Third-party permission declarations are not grants.
- `steward.*` is not a valid new third-party permission.
- Every high-risk operation is checked again at execution time.
- Paths are canonicalized and confined to user-approved roots.
- Destructive filesystem actions are host-owned and use the Recycle Bin.
- Arbitrary shell command lines are never accepted.
- Process output, runtime, file data, and result counts are bounded.
- A capability upgrade requires new user consent.
- Leaving a plugin surface destroys its VM and host process.
- A plugin cannot register routes or execute searches while its VM is not open.

## 9. Verification Strategy

Each milestone must include:

- Rust unit tests for permission resolution and path confinement;
- plugin SDK type tests and host-mocked NSH tests;
- integration tests using a temporary plugin package and temporary roots;
- adversarial tests for `..`, junctions, symlinks, reparse points, UNC paths,
  long paths, Unicode, and case-insensitive duplicate paths;
- command-injection tests for metacharacters, whitespace, quoting, and oversized
  arguments;
- process timeout, cancellation, orphan, and output-limit tests;
- package installation, update, permission escalation, rollback, and revocation
  tests;
- plugin first-frame measurements with scanning performed after the first frame;
- Windows clean-VM acceptance before public third-party admission.

## 10. Recommended Immediate Order

1. Complete PC-0 permission and contract design. (Done)
2. Implement PC-1 preferences and user feedback. (Done)
3. Implement PC-2 folder grants, bounded walk, and open-path capabilities.
   (Done)
4. Validate PC-2 with at least one non-Git local-data plugin and the read-only
   `git-repos` workload. (Done: Local Files and Git Repositories)
5. Implement PC-3 only after the controlled execution broker design is approved.
6. Validate PC-3 with a network plugin, a tool-integration plugin, and the
   action-oriented `git-repos` workload.
7. Prototype PC-4 separately and record the isolation-versus-embedding decision.
8. Start public third-party admission only after PC-0 through PC-3, validation
   across multiple plugin domains, and the Phase 3 clean-VM gate are complete.
