# Steward Plugin Host API Compatibility

> Status: PC-0 through PC-4 implemented. The host API version is `0.5.0`.

Steward uses semantic versions for the public plugin host contract. The current
version is exported as `HOST_API_VERSION` from `steward-plugin-contract` and
mirrored by `@steward/extension-api` as `launchParams().hostApiVersion`.

## Version Negotiation

New third-party packages must declare `minHostVersion` in `plugin.json`.
Existing official packages without this field keep the migration-era release
policy until they are migrated to named permissions.

Steward refuses to launch a package when:

- `minHostVersion` is missing for a non-legacy official package;
- `minHostVersion` is not valid semantic version syntax;
- the package requires a newer host API than the running Steward.

The install inspector reports the same failures before activation. Patch
versions may add backward-compatible host calls or event fields. Minor versions
may add features, but existing calls and payload fields keep their meaning.
Major versions may remove or change calls. Steward will retain compatibility
tests for the previous supported API line before raising a major version.

## Feature Discovery

Every plugin receives `hostApiVersion` and `features` in its launch parameters.
The base features are:

- `host_info`
- `runtime_feature_discovery`
- `steward.feedback`
- `steward.navigation`
- `steward.preferences`

Capability-derived features use stable names such as
`steward.clipboard.write`, `steward.system.openUrl:https`, and `plugin.data`.
Plugins should use `hostInfo()`, `supportsFeature()`, or `requireFeature()` from
`@steward/extension-api` and degrade gracefully when a feature is absent.

## Stable Errors

The public machine-readable error codes are:

| Code                  | Meaning                                                                        |
| --------------------- | ------------------------------------------------------------------------------ |
| `permission_denied`   | The package did not declare the capability or no matching active grant exists. |
| `feature_unavailable` | The running host API does not provide a requested capability.                  |
| `timeout`             | A bounded host operation exceeded its deadline.                                |
| `resource_exhausted`  | A call or plugin exceeded a configured limit.                                  |
| `host_failure`        | Steward could not complete the host operation.                                 |
| `invalid_input`       | A request failed schema, range, path, or progress validation.                  |
| `cancelled`           | The operation was cancelled before completion.                                 |
| `confirmation_denied` | The user declined a host-rendered confirmation.                                |
| `io_failure`          | A filesystem or system-open operation failed at the OS boundary.               |

`StewardError` in `@steward/extension-api` exposes `code` and `retryable`.
Host-call rejection text is not part of the stable contract. Steward encodes a
versioned JSON envelope with `code`, `message`, and `retryable` fields in NSH's
error string. The SDK parses that data directly; it does not infer codes from
prose. See [plugin-api-contract.md](plugin-api-contract.md).

## Permissions And Grants

Capability declarations are requests, not grants. The inspector exposes each
request as a `(permission, scope)` pair plus a user-readable description.
Examples:

```text
steward.clipboard.write [host=steward]
steward.fs.read [path=${dataDir}]
steward.fs.read [path=D:\projects]
steward.fs.pick [host=steward]
steward.system.openPath [host=steward]
steward.net.fetch [host=api.example.com;schemes=https;methods=GET;...]
steward.exec.run [executable=git.exe]
```

Private `${dataDir}` access, plugin-private string storage, window-close, and
navigation lifecycle calls are automatic when declared. Sensitive permissions require an active grant
matching plugin ID, publisher key ID, semantic version range, permission, and
scope. Declaring `steward.*` or any other wildcard host permission is rejected
for new third-party packages.

Grant and revocation records remain in SQLite. Every launch records allowed and
denied declaration decisions, and sensitive host calls append allowed, denied,
failed, or cancelled outcomes without storing call payloads. New launches read
the current database state, so declaration revocation applies without
restarting Steward. Selected filesystem roots are checked again on every
filesystem or system-open call, so root revocation applies to an already-open
plugin process.
Settings > Plugins lists each declared capability with an explanation and
Allow/Revoke controls. The background action revalidates the installed signed
package before creating or removing a grant.

When an update adds a sensitive declaration, `PluginOperation` returns
`addedCapabilities` and `consentRequired`. Existing grants use exact scopes, so
a new resource never inherits an old broad grant.

## Preferences

Plugins declare host-rendered preferences in `plugin.json`. Steward validates
values before persisting them and supplies the resolved map in
`launchParams().preferences`. Preference rows are keyed by plugin ID and survive
package updates. New declarations should prefer `preferences()` or
`preference(key, fallback)` from `@steward/extension-api`; host calls are
available for compatibility and feature discovery:

```text
steward.preferences.get
steward.preferences.all
```

## Feedback

The host API exposes three feedback calls:

```text
steward.feedback.toast
steward.feedback.confirm
steward.feedback.progress
```

Toast and confirmation notices are emitted by the isolated plugin process and
rendered by the launcher. A confirmation host call remains pending until the
user confirms or cancels. Progress calls are bounded and returned to the plugin
as `steward.progress` events so long-running operations can update their own
NSH surface.

## Filesystem And System Resources

The `0.3.0` API adds:

```text
steward.fs.pick
steward.fs.walk
steward.fs.readText
steward.fs.writeText
steward.fs.grants
steward.fs.cancelWalk
steward.system.openPath
steward.system.revealPath
steward.system.openWith
steward.system.applications
```

`pickFolder()` and `pickFile()` create resource grants for the exact canonical
selection. `walk()` has bounded depth and result count, a caller-selected
deadline, and cancellation by walk ID. It returns reparse-point metadata but
never descends through those entries. `readText()` and `writeText()` cap text at
4 MiB and reject paths that canonicalize outside active grants.

`installedApplications()` returns IDs from Steward's existing app scanner.
`openWith()` revalidates both the path grant and the application ID at execution
time. No system-open call has a command-line parameter.

## Network And Controlled Execution

The `0.4.0` API adds:

```text
steward.net.fetch
steward.exec.run
steward.exec.cancel
```

Network use requires an active named host grant plus an active matching
`network.http` scope. Runtime requests authorize the initial URL and every
redirect, require HTTPS and a public DNS host, reject embedded credentials,
and enforce method, path, timeout, request-body, response-body, concurrency,
and redirect limits.

Controlled execution requires an active named host grant plus an active exact
`process.execute` scope. The isolated plugin host sends a structured request
to the launcher-owned broker; it does not spawn the target itself. The broker
resolves the `.exe`, verifies that the working directory is inside a live write
grant, clears inherited environment variables, invokes the process with an
argument array, and owns a kill-on-close Job Object. Shells and common
interpreter/runtime executables are rejected. Timeout, cancellation, output
exhaustion, plugin exit, and broker shutdown terminate the process tree.

Both calls append payload-free capability audit records with allowed, denied,
failed, or cancelled outcomes. Request and response bodies, arguments, headers,
and process output are not copied into audit storage.

## Presentation And Navigation

The `0.5.0` API adds:

```text
steward.navigation.back
steward.navigation.close
steward.navigation.setTitle
```

`launchParams().presentation` is `windowed` or `embedded`. Both modes retain
the isolated plugin-host process. Embedded commands use a frameless,
non-resizable, launcher-positioned hosted surface; they do not run the VM in
the shell process and do not create a render bridge.

The plugin receives `steward.visibility` events with `{ visible: boolean }`.
Back or Esc closes the page, destroys the plugin host process, and restores
launcher focus. `setTitle` accepts 1 to 128 bytes. Navigation calls are
automatic named permissions and append payload-free audit events.

## Compatibility Tests

The current test baseline covers:

- manifest schema parsing and version validation;
- launch-parameter and process-event payload round trips;
- resource-scoped declaration descriptions;
- third-party wildcard and missing-host-version rejection;
- exact grant matching and immediate revocation;
- legacy official-package compatibility;
- migrated official packages using named permissions;
- update capability-delta reporting;
- preference defaults, migrations, isolation, and bounded values;
- feedback event and confirmation-action round trips;
- headless preference, toast, confirmation, and progress mocks;
- scoped path grants, live root revocation, traversal rejection, cancellation,
  deadlines, and result limits;
- host-grant enforcement for every filesystem and system-open call;
- two unrelated headless plugins using the same filesystem and system-open API;
- scoped HTTPS method/host/path matching, redirect authorization, local-network
  rejection, reserved-header rejection, request/response limits, and timeout;
- exact executable authorization, shell/interpreter rejection, scrubbed
  environment, argument limits, output caps, cancellation, and Windows
  process-tree termination;
- headless Network Check and Git Repositories consumers of the new APIs;
- manifest presentation routing, protocol lifecycle round trips, and host-API
  mode mismatch rejection;
- headless UI Showcase navigation, title, and visibility mocks;
- package inspection policy and signed-package lifecycle tests.
