# Steward Plugin SDK Distribution

> Status: PDX-1 implementation complete on 2026-11-22. The release workflow
> publishes the first public artifact with the next tagged Steward release.

`@steward/extension-api` is the only Steward-specific TypeScript dependency for
a third-party command-oriented plugin. It is built into JavaScript with
declarations and installed from a versioned tarball. A plugin project does not
need the Steward source checkout and must not import
`packages/extension-api/src/host.ts`.

## Compatibility Matrix

The machine-readable matrix is
[`contracts/plugin-compatibility.json`](../contracts/plugin-compatibility.json).

| Component              | Current version | Compatibility rule                                   |
| ---------------------- | --------------- | ---------------------------------------------------- |
| Steward Host API       | `0.5.0`         | Packages declare the minimum in `plugin.json`        |
| Plugin SDK             | `0.5.0`         | Must match the current Host API line                 |
| NSH CLI and runtime    | `0.6.1`         | Exact pin for plugin compilation and signed packages |
| Plugin manifest schema | `3`             | Schema `2` remains readable for legacy packages      |
| Minimum host version   | `0.5.0`         | New external packages copy this value                |

Patch releases keep existing calls, payload fields, and meanings. Minor
releases may add calls, fields, events, or features. A major Host API release
will require an explicit migration line; no older major line is currently
claimed as supported.

`pnpm sdk:pack` checks this table against `contracts/plugin-api-v1.json`, the
SDK package version, and the pinned `nsh-core` version before producing an
artifact.

## Artifact Location

The current distribution registry is the Steward GitHub Releases page:

```text
https://github.com/iFence/steward/releases
```

Each tagged release attaches:

```text
steward-extension-api-0.5.0.tgz
SHA256SUMS-sdk.txt
```

The SDK version is independent of the Steward application version. The release
tag identifies the distribution event; the tarball filename and embedded
`package.json` version identify the SDK contract.

Install from the latest Steward release channel with:

```powershell
pnpm add "https://github.com/iFence/steward/releases/latest/download/steward-extension-api-0.5.0.tgz"
```

Use a tagged URL instead of `latest` when an exact release must be reproduced:

```powershell
pnpm add "https://github.com/iFence/steward/releases/download/vX.Y.Z/steward-extension-api-0.5.0.tgz"
```

For an offline release artifact or a local mirror:

```powershell
pnpm add "file:D:\artifacts\steward-extension-api-0.5.0.tgz"
```

Verify the downloaded asset against `SHA256SUMS-sdk.txt` before installing it.
The package exposes compiled ESM JavaScript and declarations through explicit
exports:

```json
{
  "main": "./dist/host.js",
  "types": "./dist/host.d.ts",
  "exports": {
    ".": {
      "types": "./dist/host.d.ts",
      "import": "./dist/host.js",
      "default": "./dist/host.js"
    }
  }
}
```

## Create A Plugin

Install the documented prerequisites: Windows, Rust `1.97.1`, Node.js 20 or
newer, pnpm 11, and NSH CLI `0.6.1`.

```powershell
cargo install --locked --version 0.6.1 nsh-cli
cargo install --locked --git https://github.com/iFence/steward steward-plugin-cli
```

Create a project outside the Steward checkout:

```powershell
steward-plugin new .\my-plugin `
  --id com.example.my-plugin `
  --name "My Plugin"
```

The generated dependency points at the versioned SDK tarball attached to a
Steward release. It never uses `workspace:*` or a raw TypeScript source path.
For an offline tarball:

```powershell
steward-plugin new .\my-plugin `
  --id com.example.my-plugin `
  --name "My Plugin" `
  --sdk "file:D:\artifacts\steward-extension-api-0.5.0.tgz"
```

The template contains:

```text
App.tsx          UI interaction and a toast host call
App.test.tsx     headless interaction plus mocked host-call assertion
plugin.json      Steward routing metadata and minHostVersion
nsh.json         package ID, build, and steward.feedback.toast capability
package.json     SDK dependency, typecheck, test, and build scripts
tsconfig.json    NSH compiler configuration
README.md        local commands and installation notes
```

Run the project without the Steward monorepo:

```powershell
cd .\my-plugin
pnpm install
nsh check App.tsx
nsh test .
nsh build App.tsx --release -o dist
```

Signing and installation require a certificate whose root Steward trusts. The
existing development signing flow is documented in
[plugin-development-zh-CN.md](plugin-development-zh-CN.md).

## Release And CI

Build and validate the SDK artifact from the Steward repository:

```powershell
pnpm sdk:pack
Get-Content .\target\sdk\SHA256SUMS-sdk.txt
```

The command removes stale SDK tarballs from `target/sdk`, builds declarations
and JavaScript, refuses raw source in the package, verifies package exports,
and writes a SHA-256 checksum.

The external fixture at `fixtures/pdx1-external-plugin` is excluded from
`pnpm-workspace.yaml`. The PDX-1 CI gate copies it into an isolated target
directory, installs only the packaged SDK tarball with a frozen lockfile, then
runs typecheck, headless test, build, development signing, `nsh verify`,
two-file packaging, public-admission review, strict inspection, and Steward
store installation.

```powershell
pnpm test:pdx1
```

The release workflow packages the same SDK through `pnpm sdk:pack`, uploads its
checksum, and attaches both files to the tagged GitHub Release.
