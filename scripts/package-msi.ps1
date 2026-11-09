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
#   .\scripts\package-msi.ps1 -WixBin <dir> # use an explicit WiX Toolset directory

param(
    [switch]$SkipBuild,
    [string]$WixBin = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

# Plugins run through in-process NSH AppViews, so the MSI bundles only the app.
$releaseBin = Join-Path $root "target\release"
$appExe = Join-Path $releaseBin "steward-app.exe"

Push-Location $root
try {
    if (-not $SkipBuild) {
        & cargo build --release -p steward-app
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build --release failed with exit code $LASTEXITCODE"
        }
    }
    if (-not (Test-Path $appExe)) {
        throw "missing $appExe (run without -SkipBuild first)"
    }
    # Locate WiX: explicit -WixBin, WIX env var / PATH (cargo-wix default),
    # standard Chocolatey install roots, then the portable copy extracted under
    # %USERPROFILE%\.wix\wix314.
    $wixBin = ""
    if ($WixBin) {
        if (Test-Path (Join-Path $WixBin "candle.exe")) {
            $wixBin = $WixBin
        } elseif (Test-Path (Join-Path $WixBin "bin\candle.exe")) {
            $wixBin = Join-Path $WixBin "bin"
        } else {
            throw "WiX Toolset not found under explicit path $WixBin"
        }
    } elseif ($env:WIX) {
        if (Test-Path (Join-Path $env:WIX "candle.exe")) {
            $wixBin = $env:WIX
        } elseif (Test-Path (Join-Path $env:WIX "bin\candle.exe")) {
            $wixBin = Join-Path $env:WIX "bin"
        } else {
            throw "WiX Toolset not found under WIX=$env:WIX"
        }
    } elseif (-not (Get-Command candle -ErrorAction SilentlyContinue)) {
        $candidates = @(
            (Join-Path ${env:ProgramFiles(x86)} "WiX Toolset v3.14"),
            (Join-Path $env:ProgramFiles "WiX Toolset v3.14"),
            (Join-Path $env:USERPROFILE ".wix\wix314")
        )
        foreach ($candidate in $candidates) {
            if (Test-Path (Join-Path $candidate "candle.exe")) {
                $wixBin = $candidate
                break
            }
            if (Test-Path (Join-Path $candidate "bin\candle.exe")) {
                $wixBin = Join-Path $candidate "bin"
                break
            }
        }
        if ($wixBin) {
            Write-Host "Using WiX Toolset at $wixBin"
        } else {
            throw "WiX Toolset v3 not found: set WIX, add candle.exe to PATH, pass -WixBin, or extract the portable binaries under $env:USERPROFILE\.wix\wix314"
        }
    }

    # cargo-wix already links WixUIExtension and WixUtilExtension by default,
    # so `util:CloseApplication` in main.wxs needs no extra -ext arguments here.
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
