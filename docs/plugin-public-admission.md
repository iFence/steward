# Plugin Public Admission

> Status: PC-5 implementation in progress. Automated capability-policy review
> and strict marketplace synchronization are implemented. A hosted publisher
> registration service, distribution operations, and the clean-Windows-VM
> release gate remain incomplete.

Steward does not yet operate a public third-party marketplace. This document
defines the policy and evidence required before one can be enabled. Local
marketplace workflows remain available for development and manual review.

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

The report also includes the policy version, publisher key ID, required and
current host API versions, denied capability categories, and capability
declarations grouped by permission and scope.

Normal `inspect`, `check`, and local `marketplace sync` behavior is unchanged:
an otherwise valid package with a review item can remain visible for manual
review. Public marketplace indexing uses the stricter mode:

```powershell
steward-plugin marketplace sync --public
```

Any package that fails one check is skipped and its non-observable finding is
written to the synchronization errors. Accepted entries do not require a
second manual review flag and record the admission policy version in the
marketplace index.

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

The current publisher identity is the NSH release key ID certified by a root in
`assets/nsh/trust-roots.json`. A self-signed or uncertified key is never treated
as a publisher identity. Changing the publisher of an installed plugin ID
requires removing the old package first.

Root-signed revocation lists can revoke a release key or an exact package
SHA-256. Apply them through:

```powershell
steward-plugin revoke apply .\revocations.json
```

The NSH trust store validates the sequence and root signature before accepting
the list. Revoked packages fail inspection and launch. A hosted publisher
registration service must add account recovery, key rotation, dispute handling,
and operator audit records before public admission can ship; those operations
do not exist yet.

## Release Gate

PC-5 remains blocked until all of the following are complete:

- signed-package install, update, crash, rollback, revocation, and uninstall
  acceptance pass on a clean Windows VM;
- plugins from at least three independent domains exercise scoped filesystem,
  network, process, preference, and navigation capabilities;
- hosted publisher registration and revocation operations have documented
  ownership and recovery procedures;
- public repository and marketplace administration can quarantine a package
  and publish a root-signed revocation response;
- release review records the remaining same-user security boundary.

Plugin hosts and approved tools run as the current Windows user. The broker,
capability checks, process tree ownership, and quotas reduce accidental or
malicious behavior, but they are not an OS-level sandbox against a same-user
attacker.
