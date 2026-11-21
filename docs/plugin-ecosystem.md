# Plugin Ecosystem v1

> Status: M5 complete. Local package lifecycle, inspection, marketplace
> indexing, publisher identity, revocation, and automated policy checks are
> implemented. PC-5 public admission is in progress; see
> [plugin-public-admission.md](plugin-public-admission.md).

## Package Manager

`steward-plugin` is the command-line companion to the Plugins page in Settings:

```powershell
cargo run -p steward-plugin-cli --bin steward-plugin -- help
```

Use `--data-dir <path>` before the command to isolate tests. The installed
plugin directory can be overridden with `--plugins-dir`, and an additional NSH
trust root can be supplied with `--trust-root`. By default the CLI uses
`%APPDATA%\Steward`, the committed production trust roots, and
`%APPDATA%\Steward\plugins`.

Supported source forms are:

- a plugin zip containing one top-level directory with exactly `plugin.json`
  and `plugin.nshpkg`;
- a two-file plugin directory.

Packages are inspected with the same NSH trust store and release capability
policy used at launch. Steward never installs a package whose signature,
certificate, trust root, revocation state, build, manifest identity, or archive
layout is invalid.

## Scaffold A Plugin

```powershell
cargo run -p steward-plugin-cli --bin steward-plugin -- new .\my-plugin `
  --id com.example.my-plugin --name "My Plugin" --command my-plugin
```

The generated files are `App.tsx`, `nsh.json`, `plugin.json`, `package.json`,
`tsconfig.json`, `README.md`, and `.gitignore`. Continue with:

```powershell
cd .\my-plugin
nsh check App.tsx
nsh test .
nsh build App.tsx --release -o dist
```

The scaffold does not create a signing key or package. Production signing
continues to use the configured NSH release key and certificate.

## Inspect And Check

```powershell
steward-plugin inspect .\my-plugin.zip
steward-plugin check .\my-plugin.zip
steward-plugin check .\my-plugin.zip --strict --json
steward-plugin review .\my-plugin.zip --json
```

Inspection reports the app ID/version/build, package SHA-256, claimed publisher
key, signature validity, trust result, minimum NSH runtime, and policy findings.
Normal `check` fails on invalid signatures, untrusted packages, identity
mismatches, unsafe archive layouts, and malformed metadata. `--strict` also
fails when a package declares a capability outside the current release grant;
the declaration remains visible as a review item because NSH intersects it with
the release grant at launch.

`review` applies the public-marketplace admission policy. It fails for every
review item, wildcard declaration, missing publisher identity, incompatible
host requirement, or other release-policy violation. The JSON form includes
stable check codes and capability groups for publishing automation.

## Lifecycle

```powershell
steward-plugin install .\plugin.zip
steward-plugin update .\newer-plugin.zip
steward-plugin list
steward-plugin remove com.example.plugin
steward-plugin rollback com.example.plugin
steward-plugin rollback com.example.plugin --version 1.0.0
```

An update requires a higher semantic version and NSH build number. Before
activation, Steward moves the current package into managed history under
`%APPDATA%\Steward\plugin-history\<plugin-id>`. The five newest versions are
retained. State is recorded atomically in `plugin-store.json`, and the route
cache is refreshed after each operation.

Rollback restores a retained, re-verified package. It is intentionally subject
to NSH's anti-downgrade rule: after a newer build has been accepted as healthy,
NSH refuses older builds. In that case Steward reports the security refusal and
requires a newly signed forward build; it does not reset or weaken the trust
store.

## Local Marketplace

Place distributable zips under:

```text
%APPDATA%\Steward\marketplace\packages\
```

Then index and use them:

```powershell
steward-plugin marketplace sync
steward-plugin marketplace sync --public
steward-plugin marketplace list
steward-plugin marketplace install com.example.plugin
steward-plugin marketplace update com.example.plugin
```

`marketplace sync` verifies each archive before adding it to `index.json`. The
index records the artifact hash, signed package hash, version/build, and release
publisher key ID. Package bytes are verified again at installation time, so a
changed index or artifact cannot bypass trust.

`marketplace sync` indexes otherwise valid packages for local manual review.
`marketplace sync --public` applies the stricter public-admission gate and
rejects any package whose capability declarations need manual review. Accepted
entries record the admission policy version in the index. This command does not
enable the hosted public marketplace.

The Settings > Plugins page uses the same store in a background worker. It
shows installed packages, signature/trust state, publisher key ID, package
hash, policy-review count, local marketplace entries, and update/rollback
actions.

## Publisher Identity And Revocation

The publisher identity shown by Steward is the NSH release key ID certified by
a trusted root. Certificate bodies and package files cannot substitute a
different key. A package signed by a self-consistent but uncertified key is
shown as signature-valid but untrusted.

Apply a root-signed revocation list:

```powershell
steward-plugin revoke apply .\revocations.json
```

Create a new list with an offline root key:

```powershell
steward-plugin revoke create .\revocations-2.json `
  --root-key .\offline-root.secret `
  --previous .\revocations-1.json `
  --sequence 2 `
  --key-id <release-key-id> `
  --package-sha256 <package-sha256>
```

The root key is read only for the signing operation and is never written to
Steward state or logs. Applied revocation lists are validated by NSH before
being persisted and are also used by every subsequent plugin launch.

## Automated Policy Checks

Rust tests cover inspection, publisher identity, update history, rollback,
marketplace indexing, revocation, unsafe archive rejection, and scaffolding.
The release workflow runs `scripts/check-plugin-policies.ps1 -PublicAdmission`
against every production-signed plugin archive after `nsh verify`.

Current release policy review categories include:

- shell, interpreter, inherited-environment, and exit-control process requests;
- raw sockets, non-HTTPS requests, unrestricted hosts, and nonstandard ports;
- web views and web-view input/media;
- direct filesystem roots other than `${dataDir}`, while scoped path access is
  obtained through `steward.fs.pick`;
- generic NSH file picker, sharing, speech, notifications, audio, and
  application control;
- home-directory discovery and extra native windows;
- named host calls outside the current clipboard, storage, calendar, window,
  preference, feedback, scoped filesystem, installed-application, and
  system-open, scoped HTTPS, and controlled-execution allowlist;
- URL schemes other than `https` and `mailto`.

These declarations are reported rather than treated as an automatic
admissibility failure because NSH intersects them with the release grant and
the effective capability is still deny-by-default. Public marketplace
admission uses stricter automatic rejection without changing the package
format. See [plugin-public-admission.md](plugin-public-admission.md) for the
policy, publisher lifecycle, and release gate.

The filesystem/system-open allowlist only declares an API gate. A
`steward.fs.pick` call must still be followed by the user choosing a concrete
file or folder; that exact canonical selection is persisted as a separate path
grant. `openPath`, `revealPath`, `readText`, `writeText`, and `walk` re-read the
path grants on every call.

Scoped HTTPS and controlled execution follow the same two-layer rule. The
named host call is an API gate; a separate active `network.http` or exact
`process.execute` grant authorizes the concrete resource. Runtime handlers
reload those grants, and the launcher-owned execution broker owns the process
tree so the plugin host keeps its one-active-process limit.
