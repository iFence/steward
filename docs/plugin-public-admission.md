# Plugin Public Admission

> Status: PC-5/PDX-4 implementation complete. Automated capability-policy
> review, strict marketplace synchronization, publisher operations, operator
> audit, quarantine, and root-signed revocation responses are implemented. The
> public launch remains blocked on clean-Windows-VM and production independent
> publisher evidence.

Steward's public marketplace operations are implemented but remain disabled
until the clean-VM and production-admission gates pass. This document defines
the policy and evidence required before public launch. Local marketplace
workflows remain available for development and manual review.

## Admission Pipeline

Public admission policy version `1` applies to a verified distributable zip. It
uses the same NSH package verification, trust store, manifest parser, and
release capability ceiling as installation and launch.

The automated gate checks:

1. The package signature is valid.
2. A configured NSH trust root trusts the release certificate.
3. The trusted release certificate supplies a stable publisher key ID.
4. The manifest and signed package agree on ID and version, and use a valid
   positive build number.
5. `minHostVersion` is present, valid, and no newer than the running host API.
6. The package passes Steward's release policy.
7. Every declaration is named and inside the release profile. Wildcards,
   unknown declarations, raw sockets, unrestricted filesystem roots,
   non-HTTPS requests, shell or interpreter execution, and other capabilities
   that need manual review reject the package.

The CLI returns stable check codes so publishing automation does not need to
parse human-readable messages:

```powershell
steward-plugin review .\steward-plugin-example-1.0.0.zip --json
```

Check codes are:

- `signature`
- `trust_chain`
- `publisher_identity`
- `release_policy`
- `capability_scope`
- `public_contract`

The report also includes the policy version, package and publisher identities,
required and current host API versions, denied capability categories, and
capability declarations grouped by permission and scope. With `--previous`, it
records the prior version and added/removed declarations. `--output` writes the
same report for release review.

Normal `inspect`, `check`, and local `marketplace sync` behavior is unchanged:
an otherwise valid package with a review item can remain visible for manual
review. Public marketplace indexing uses the stricter mode:

```powershell
steward-plugin marketplace sync --public
```

Any package that fails one check is skipped and its non-observable finding is
written to the synchronization errors. Accepted entries do not require a
second manual review flag and record the admission policy version in the
marketplace index. They also require an active registered publisher key and no
active marketplace quarantine.

## Permission And Update Review

Third-party permission declarations are requests, not grants. Sensitive
capabilities still require a matching active user grant scoped by plugin ID,
publisher key ID, version range, capability, and resource. Settings displays
the declaration, explanation, publisher, and current grant state.

An update that introduces a sensitive declaration reports the capability
delta and requires renewed user approval. Existing grants are not inherited by
the new declaration. Revoking a grant while Steward is running applies to the
next capability check without restarting the plugin host.

Public publication review must include the same delta in its release notes.
Automated admission can identify the declarations, but it does not decide
whether a new capability is justified for the plugin's user-facing purpose.

## Publisher And Revocation Operations

The current cryptographic publisher identity is the NSH release key ID certified
by a root in `assets/nsh/trust-roots.json`. A self-signed or uncertified key is
never treated as a publisher identity. Public publishing also requires an
operator registration record for that key, an active publisher account, and an
active key state. Changing the publisher of an installed plugin ID requires
removing the old package first.

The operator plane provides registration, scheduled key rotation, compromise
recovery, status suspension, key revocation, and a hash-chained audit:

```powershell
steward-plugin publisher register .\release.zip --id acme.plugins `
  --name "Acme Plugins" --contact plugins@acme.example `
  --domain acme.example --operator "ops:alice"
steward-plugin publisher rotate acme.plugins --package .\next-key.zip `
  --operator "ops:alice" --reason "scheduled rotation"
steward-plugin publisher rotate acme.plugins --package .\recovery.zip `
  --operator "security:bob" --reason "INC-2026-014" --recovery
steward-plugin publisher audit --json
```

See [plugin-operations.md](plugin-operations.md) for ownership, incident, and
deployment requirements.

Root-signed revocation lists can revoke a release key or an exact package
SHA-256. Apply them through:

```powershell
steward-plugin revoke apply .\revocations.json
```

The NSH trust store validates the sequence and root signature before accepting
the list. Revoked packages fail inspection and launch.

Public marketplace operators can quarantine an exact package hash, a
plugin/version, a release key, or a publisher. Every public sync and resolution
rechecks quarantine state, so a stale index cannot bypass the incident. A
quarantine can generate the root-signed package/key revocation response without
storing the offline root key:

```powershell
steward-plugin marketplace quarantine --package-sha256 <sha256> `
  --reason "malicious update" --operator "incident:carol" --incident INC-2026-021
steward-plugin marketplace revoke-quarantine <quarantine-id> .\revocations-2.json `
  --root-key .\offline-root.secret --previous .\revocations-1.json `
  --sequence 2 --operator "root-custodian:dave"
```

## Release Gate

The implementation gate is complete. Public launch remains blocked until all of
the following acceptance evidence exists:

- signed-package install, update, crash, rollback, revocation, and uninstall
  acceptance pass on a clean Windows VM;
- plugins from at least three independent domains exercise scoped filesystem,
  network, process, preference, and navigation capabilities;
- hosted publisher registration and revocation operations have documented
  ownership and recovery procedures;
- public repository and marketplace administration can quarantine a package
  and publish a root-signed revocation response;
- release review records the remaining same-user security boundary.

The hosted publisher, quarantine, root-revocation, and same-user documentation
items are covered by [plugin-operations.md](plugin-operations.md). The clean-VM
and production independent-publisher evidence still require operator sign-off.

Plugin hosts and approved tools run as the current Windows user. The broker,
capability checks, process tree ownership, and quotas reduce accidental or
malicious behavior, but they are not an OS-level sandbox against a same-user
attacker.
