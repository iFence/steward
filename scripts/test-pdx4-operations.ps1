#!/usr/bin/env pwsh
# Exercise the PDX-4 publisher registry, key lifecycle, public marketplace
# quarantine, capability deltas, and root-signed revocation response. Clean-VM
# evidence remains a separate release gate because it cannot run in ordinary CI.

$ErrorActionPreference = "Stop"
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path

Push-Location $root
try {
    & cargo test -p steward-plugin-store --test store pdx4_
    if ($LASTEXITCODE -ne 0) {
        throw "PDX-4 ecosystem operations tests failed"
    }
    foreach ($document in @(
        "docs/plugin-operations.md",
        "docs/plugin-public-admission.md",
        "SECURITY.md"
    )) {
        if (-not (Test-Path -LiteralPath (Join-Path $root $document))) {
            throw "PDX-4 required operations document is missing: $document"
        }
    }
    Write-Host "PDX-4 ecosystem operations passed."
} finally {
    Pop-Location
}
