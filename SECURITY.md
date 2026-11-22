# Security Policy

Steward protects signed plugin packages, capability grants, process isolation,
and marketplace operations. A signed plugin still runs as the current Windows
user; the plugin VM, capability broker, and Job Object limits are containment
boundaries, not protection from malware that already has that user's privileges.

## Supported Versions

The latest stable release and the immediately previous minor release receive
security fixes. The latest public SDK release for each supported host API line
receives compatibility fixes. Pre-release builds are evaluated case by case.

## Reporting A Vulnerability

Do not open a public issue for a suspected vulnerability. Send a private report
to the maintainers through the repository's private security-advisory channel
with:

- affected Steward, SDK, and plugin versions;
- package ID, build, publisher key ID, and SHA-256 when available;
- reproduction steps or a minimal proof of concept;
- impact and whether active exploitation is suspected; and
- a safe contact method.

Do not send release private keys, offline root private keys, clipboard or file
contents, request bodies, or process environment values. If a report requires a
secret to reproduce a key-management issue, coordinate a separate secure
exchange.

We aim to acknowledge active-compromise reports within 24 hours and other
reports within two business days. A confirmed bad public package can be
quarantined while offline root revocation is coordinated.

## Scope

In scope includes signature or trust bypass, admission-policy bypass,
capability escalation, path confinement failures, command injection,
cross-process or cross-plugin data exposure, process containment failures,
marketplace index or quarantine bypass, publisher-key lifecycle errors, and
secret leakage from Steward diagnostics.

Out of scope includes malware running with the current user's full privileges
without a Steward boundary crossing, unsupported pre-release builds, and
third-party plugin bugs that do not indicate a Steward platform failure.

See [docs/plugin-support-policy.md](docs/plugin-support-policy.md) and
[docs/plugin-operations.md](docs/plugin-operations.md) for support, publisher,
quarantine, and dispute procedures.
