#!/usr/bin/env pwsh
# Build the Steward MSI installer with cargo-wix (WiX Toolset v3).
#
# Prerequisites:
#   - Rust (MSVC toolchain)
#   - cargo-wix:  cargo install cargo-wix
#   - WiX Toolset v3: installed (WIX env var or candle.exe on PATH), or the
#     portable binaries extracted under %USERPROFILE%\.wix\wix314:
#       https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip
#
# Usage:
#   .\scripts\package-msi.ps1            # build release + MSI
#   .\scripts\package-msi.ps1 -SkipBuild # reuse an existing release binary

param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

# The MSI bundles steward-plugin-runtime.exe next to the app executable (the
# plugin host resolves it as a sibling), so both release binaries must exist.
$releaseBin = Join-Path $root "target\release"
$appExe = Join-Path $releaseBin "steward-app.exe"
$runtimeExe = Join-Path $releaseBin "steward-plugin-runtime.exe"

Push-Location $root
try {
    if (-not $SkipBuild) {
        & cargo build --release -p steward-app -p steward-plugin-runtime
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build --release failed with exit code $LASTEXITCODE"
        }
    }
    if (-not (Test-Path $appExe)) {
        throw "missing $appExe (run without -SkipBuild first)"
    }
    if (-not (Test-Path $runtimeExe)) {
        throw "missing $runtimeExe (run without -SkipBuild first)"
    }

    # Locate WiX: explicit -b argument, WIX env var / PATH (cargo-wix default),
    # then the portable copy extracted under %USERPROFILE%\.wix\wix314.
    $wixBin = ""
    if ($env:WIX) {
        $wixBin = Join-Path $env:WIX "bin"
    } elseif (-not (Get-Command candle -ErrorAction SilentlyContinue)) {
        $portable = Join-Path $env:USERPROFILE ".wix\wix314"
        if (Test-Path (Join-Path $portable "candle.exe")) {
            $wixBin = $portable
        } else {
            throw "WiX Toolset v3 not found: set WIX, add candle.exe to PATH, or extract the portable binaries to $portable"
        }
    }

    $wixArgs = @("-p", "steward-app", "--nocapture", "-L", "-sval")
    if ($wixBin) {
        $wixArgs += @("-b", $wixBin)
    }
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
