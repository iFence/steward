#!/usr/bin/env pwsh
# Gate PDX-5.1 encrypted plugin-private secrets across contract, Rust host,
# SDK bridge, and two unrelated NSH plugin consumers.

$ErrorActionPreference = "Stop"

$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
Push-Location $root
try {
    foreach ($command in @("node", "cargo", "corepack")) {
        if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
            throw "$command is required for the PDX-5 secret storage gate"
        }
    }

    & node scripts/plugin-api-contract.mjs
    if ($LASTEXITCODE -ne 0) {
        throw "plugin API secret contract check failed"
    }

    & cargo test -p steward-plugin-contract
    if ($LASTEXITCODE -ne 0) {
        throw "plugin secret contract DTO tests failed"
    }

    & cargo test -p steward-plugin-host plugin_private_secret_calls
    if ($LASTEXITCODE -ne 0) {
        throw "plugin secret authorization tests failed"
    }

    & cargo test -p steward-app plugin_secrets
    if ($LASTEXITCODE -ne 0) {
        throw "plugin secret storage tests failed"
    }

    & corepack pnpm --filter @steward/extension-api build
    if ($LASTEXITCODE -ne 0) {
        throw "extension API build failed"
    }

    & corepack pnpm test:contract
    if ($LASTEXITCODE -ne 0) {
        throw "TypeScript secret bridge fixtures failed"
    }

    & corepack pnpm --filter @steward/plugin-network-check test
    if ($LASTEXITCODE -ne 0) {
        throw "Network Check secret lifecycle failed"
    }

    & corepack pnpm --filter @steward/plugin-secret-notes test
    if ($LASTEXITCODE -ne 0) {
        throw "Secret Notes lifecycle failed"
    }

    Write-Host "PDX-5.1 encrypted plugin secrets passed."
} finally {
    Pop-Location
}
