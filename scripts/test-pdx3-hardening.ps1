#!/usr/bin/env pwsh
# Gate the stable plugin API line: contract generation, cross-layer fixtures,
# supported prior-line DTOs, host version negotiation, and SDK type checks.

$ErrorActionPreference = "Stop"

$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
Push-Location $root
try {
    foreach ($command in @("node", "cargo", "corepack")) {
        if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
            throw "$command is required for the PDX-3 hardening gate"
        }
    }

    & node scripts/plugin-api-contract.mjs
    if ($LASTEXITCODE -ne 0) {
        throw "plugin API contract hardening check failed"
    }

    & cargo test -p steward-plugin-contract
    if ($LASTEXITCODE -ne 0) {
        throw "plugin contract compatibility tests failed"
    }

    & cargo test -p steward-plugin-host --lib
    if ($LASTEXITCODE -ne 0) {
        throw "plugin host compatibility tests failed"
    }

    & corepack pnpm --filter @steward/extension-api build
    if ($LASTEXITCODE -ne 0) {
        throw "extension API build failed"
    }

    & corepack pnpm test:contract
    if ($LASTEXITCODE -ne 0) {
        throw "TypeScript contract harness failed"
    }

    & corepack pnpm --filter @steward/extension-api typecheck
    if ($LASTEXITCODE -ne 0) {
        throw "extension API typecheck failed"
    }

    Write-Host "PDX-3 public API v1 hardening passed."
} finally {
    Pop-Location
}
