# Plugin Support Policy

This policy defines the public support expectations for Steward plugin API
`1.x` and public third-party packages admitted under the same policy version.
It does not promise that every plugin behaves correctly; it defines how Steward
operators triage and communicate failures.

## Supported Versions

- Host API `1.x` remains the supported public command-oriented plugin line.
- The latest stable Steward release and the immediately previous minor release
  receive security fixes.
- The latest SDK release for each supported host line receives build and
  compatibility fixes.
- Public marketplace admission uses the current policy version. Packages
  admitted under an older policy remain listed unless trust, key, quarantine,
  compatibility, or a new policy review removes them.
- Pre-release SDKs and host builds are best-effort and must be labeled as such.

## Response Targets

These are targets, not contractual service-level guarantees:

| Severity | Initial response target | Containment target |
| --- | --- | --- |
| Active compromise or confirmed malicious package | 24 hours | Quarantine and root-revocation decision within 72 hours |
| Signature, trust, admission, or privilege bypass | 2 business days | Mitigation or documented workaround within 7 days |
| Plugin crash, data loss, or compatibility regression | 3 business days | Next patch release when reproducible |
| Documentation and usability issue | 5 business days | Next scheduled documentation update |

Reports involving offline root signing may take longer to contain, but the
marketplace quarantine must be decided within the listed target.

## What Support May Request

Support may request the plugin ID, version, build, publisher key ID, package
SHA-256, host API version, stable error or admission code, operating-system
version, and Steward support diagnostics. These fields identify the package and
failure stage without exposing plugin-private data.

Support must not request clipboard contents, plugin storage contents, file
contents, network request bodies, process environment values, release private
keys, private root keys, or publisher account credentials.

## Publisher Responsibilities

Publishers are responsible for:

- maintaining a monitored security and support contact;
- keeping release keys and CI secrets out of source control and logs;
- publishing a new build after key rotation rather than reusing a revoked key;
- describing newly added sensitive capabilities in release notes;
- responding to reports about their package; and
- preserving package hashes and signing provenance for dispute resolution.

## Security And Disputes

Send vulnerability reports through the private process in
[SECURITY.md](../SECURITY.md). Do not include exploit details in a public issue
before coordination.

A publisher may dispute admission or quarantine by providing the incident
reference, package hash, affected versions, reproduction steps, signing
provenance, and requested remedy. The incident commander records the decision,
reason, operator identity, and any root-signed revocation sequence. Releasing a
quarantine preserves the original incident and audit history.

## Security Boundary

Steward hosts, brokers, and approved processes run as the current Windows user.
Package signatures, capability checks, process isolation, resource limits, and
revocation protect against untrusted packages and accidental misuse. They are
not an OS-level sandbox against malware already running as the same user.
