#!/usr/bin/env pwsh
# Build the Steward MSI installer with cargo-wix (WiX Toolset v3).
#
# Prerequisites:
#   - Rust (MSVC toolchain)
#   - cargo-wix:  cargo install cargo-wix
#   - WiX Toolset v3 (set the WIX environment variable, e.g.
#     C:\Program Files (x86)\WiX Toolset v3.14\)
#
# Usage:
#   .\scripts\package-msi.ps1            # build release + MSI
#   .\scripts\package-msi.ps1 -SkipBuild # reuse an existing release binary

param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

Push-Location $root
try {
    $wixArgs = @("-p", "steward-app", "--nocapture", "-L", "-sval")
    if ($SkipBuild) {
        $wixArgs += "--no-build"
    }
    & cargo wix @wixArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo wix failed with exit code $LASTEXITCODE"
    }
    Get-ChildItem "target\wix\*.msi" | ForEach-Object {
        Write-Host "Built MSI: $($_.FullName) ($([math]::Round($_.Length / 1MB, 1)) MB)"
    }
}
finally {
    Pop-Location
}
