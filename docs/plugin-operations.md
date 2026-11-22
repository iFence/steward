# Plugin Publisher And Marketplace Operations

> Status: PDX-4 implementation complete. Publisher registration, release-key
> rotation and recovery, quarantine, root-signed revocation responses, release
> admission reports, capability deltas, support policy, and operator audit
> records are implemented. Public launch remains conditioned on independent
> publisher evidence and clean-Windows-VM acceptance recorded in
> [plugin-public-admission.md](plugin-public-admission.md).

## Operating Roles

Public admission separates package trust from publishing authority.

| Role | Responsibility |
| --- | --- |
| Publisher | Owns plugin code and release keys, provides support contacts, and reports compromise or ownership changes |
| Marketplace operator | Registers publishers, verifies domain and contact evidence, rotates or suspends publisher state, and reviews release reports |
| Security incident commander | Opens quarantine incidents, coordinates root revocation, and decides when a quarantine can be released |
| Root custodian | Holds the offline NSH root key and signs revocation sequences after an independent incident decision |

One person may fill more than one role during an early beta, but a public
incident must record both the operator and the root-signing decision. Root
private keys and release private keys are never stored in the Steward data
directory or committed to the repository.

## Publisher Registration

Registration uses a package signed by the requested release key and certified
by a configured NSH trust root. The package proves key ownership; the operator
is still responsible for verifying the publisher's contact information and
claimed domains.

```powershell
steward-plugin publisher register .\publisher-release.zip `
  --id acme.plugins `
  --name "Acme Plugins" `
  --contact "plugins@acme.example" `
  --domain acme.example `
  --operator "ops:alice"
```

The publisher ID is an account identifier. The NSH release-key ID is the
cryptographic publishing identity. A release key cannot be assigned to two
publisher accounts.

Publisher state is stored in `<data-dir>/plugin-operations.json`. Public
marketplace synchronization only indexes a package when its release key belongs
to an active publisher and has `active` key status. Local marketplace
synchronization remains available for development and manual review.

## Key Rotation And Recovery

Scheduled rotation retires the current key and activates a new key from a
package that passes signature, trust, host-version, archive, and capability
policy checks:

```powershell
steward-plugin publisher rotate acme.plugins `
  --package .\publisher-next-key.zip `
  --operator "ops:alice" `
  --reason "scheduled annual rotation"
```

If a release key is compromised, use recovery mode. All older, non-revoked keys
for that publisher are revoked and the replacement key becomes active:

```powershell
steward-plugin publisher rotate acme.plugins `
  --package .\publisher-recovery-key.zip `
  --operator "security:bob" `
  --reason "INC-2026-014 release key compromise" `
  --recovery
```

A single key can also be revoked without changing unrelated publishers:

```powershell
steward-plugin publisher revoke-key acme.plugins `
  --key-id <release-key-id> `
  --operator "security:bob" `
  --reason "retired key compromised"
```

The old package remains a valid signed artifact, but its key no longer
authorizes new public marketplace entries. Users only gain local revocation
protection after the corresponding root-signed NSH revocation list is
distributed and applied.

Inspect the current registry and tamper-evident audit chain with:

```powershell
steward-plugin publisher list
steward-plugin publisher show acme.plugins --json
steward-plugin publisher audit --json
```

Audit records are sequential and hash-chained. Loading the operations state
validates every record's sequence, previous hash, and content hash. The file is
atomic but is not a remote write-once store; a hosted deployment must place it
on a backed-up volume with access restricted to the operations service.

## Public Marketplace Quarantine

Quarantine prevents a target from entering or resolving through the public
marketplace. Exactly one selector is accepted per incident record.

```powershell
# Exact signed-package hash
steward-plugin marketplace quarantine `
  --package-sha256 <sha256> `
  --reason "malicious update reported by users" `
  --operator "incident:carol" `
  --incident "INC-2026-021"

# All versions, or one version, of a plugin ID
steward-plugin marketplace quarantine `
  --plugin-id com.acme.plugin `
  --version 2.1.0 `
  --reason "security review failed" `
  --operator "incident:carol"

# All packages from one release key or publisher
steward-plugin marketplace quarantine `
  --publisher-id acme.plugins `
  --reason "publisher account takeover under investigation" `
  --operator "incident:carol"
```

Every public synchronization and marketplace resolution rechecks active
quarantines. A stale index therefore cannot bypass quarantine. List and release
records with:

```powershell
steward-plugin marketplace quarantines --json
steward-plugin marketplace release-quarantine <quarantine-id> `
  --operator "incident:carol" `
  --reason "false positive confirmed with publisher and reporter"
```

Release does not erase the original incident or audit record.

## Root-Signed Revocation Response

Quarantine is a marketplace control. Root-signed NSH revocation removes local
trust and is the response for compromised publishers or known-bad packages:

```powershell
steward-plugin marketplace revoke-quarantine <quarantine-id> `
  .\revocations-2.json `
  --root-key .\offline-root.secret `
  --previous .\revocations-1.json `
  --sequence 2 `
  --operator "root-custodian:dave"
```

Package and plugin targets add exact package hashes. Release-key and publisher
targets add release-key IDs. The resulting JSON is validated by NSH before
application:

```powershell
steward-plugin revoke apply .\revocations-2.json
```

The root custodian signs after the incident commander records the scope and
reason. Publish the new sequence through the update channel, keep the prior
sequence for rollback auditing, and never reuse a sequence number.

## Release Review Evidence

Public release review writes a machine-readable admission report for every
archive:

```powershell
steward-plugin review .\plugin-2.1.0.zip `
  --previous .\plugin-2.0.0.zip `
  --output .\admission\plugin-2.1.0.json
```

The report records the policy version, package and publisher identities, host
API requirement, every stable admission check, capability groups, and added or
removed declarations compared with the previous release. Reviewers must
justify newly added sensitive declarations in the public release notes.

`scripts/check-plugin-policies.ps1 -PublicAdmission` writes reports under
`target/plugins/admission` and computes deltas between consecutive builds of
each plugin. The release workflow uploads those reports beside the signed
archives.

## Support And Incidents

- Supported app and plugin versions, response targets, and the same-user
  security boundary are in [plugin-support-policy.md](plugin-support-policy.md).
- Vulnerabilities are reported through the private process in
  [SECURITY.md](../SECURITY.md), not through public issue details.
- Publishers may dispute an admission or quarantine decision with the incident
  reference, package hash, reproduction details, and requested remedy. The
  incident commander records the decision and releases or narrows quarantine
  when evidence supports it.
- User support may request plugin ID, version, publisher key ID, package hash,
  stable admission code, and support diagnostics. Request bodies, clipboard
  contents, file contents, environment secrets, and plugin-private storage are
  not collected.

## Clean-VM Acceptance

The clean-VM operator must record evidence for production-signed install,
update, crash containment, rollback, quarantine, root revocation, reinstall,
and uninstall. The lifecycle scripts provide local coverage, but they do not
replace an isolated Windows VM with a fresh user profile and production trust
root. Until that evidence exists, the public marketplace remains disabled.
