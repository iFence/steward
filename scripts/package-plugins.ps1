#!/usr/bin/env pwsh
# Build distributable plugin packages: one zip per plugin under
# `packages/plugins`, written to `target/plugins`.
#
# Each zip contains a single top-level folder named after the plugin with
# `plugin.json` and a signed `plugin.nshpkg`.
#
# Usage:
#   .\scripts\package-plugins.ps1            # package every plugin
#   .\scripts\package-plugins.ps1 -Plugin calendar

param(
    [string[]]$Plugin = @(),
    [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$pluginsRoot = Join-Path $root "packages\plugins"
if (-not $OutDir) {
    $OutDir = Join-Path $root "target\plugins"
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$names = if ($Plugin.Count -gt 0) {
    $Plugin
} else {
    Get-ChildItem $pluginsRoot -Directory | Select-Object -ExpandProperty Name
}

foreach ($name in $names) {
    $dir = Join-Path $pluginsRoot $name
    $manifest = Join-Path $dir "plugin.json"
    if (-not (Test-Path $manifest)) {
        Write-Warning "skip $name (no plugin.json in $dir)"
        continue
    }
    # Read UTF-8 explicitly: Windows PowerShell 5.1 defaults to the ANSI code
    # page, which mangles the non-ASCII keywords a manifest may carry (`日历`).
    $meta = Get-Content -Raw -Encoding UTF8 -LiteralPath $manifest | ConvertFrom-Json
    $version = $meta.version
    if ($meta.schemaVersion -ne 2 -or $meta.package -ne "plugin.nshpkg") {
        throw "plugin.json in $dir is not NSH schema v2"
    }
    if (-not $version) {
        throw "plugin.json in $dir has no version"
    }

    $package = Join-Path $dir "dist\plugin.nshpkg"
    if (-not (Test-Path -LiteralPath $package)) {
        $key = $env:STEWARD_NSH_RELEASE_KEY
        $cert = $env:STEWARD_NSH_RELEASE_CERT
        if (-not $key -or -not $cert -or -not (Get-Command nsh -ErrorAction SilentlyContinue)) {
            throw "missing signed package for ${name}: set STEWARD_NSH_RELEASE_KEY/STEWARD_NSH_RELEASE_CERT and install nsh"
        }
        & nsh publish (Join-Path $dir "App.tsx") --key $key --cert $cert -o $package
        if ($LASTEXITCODE -ne 0) {
            throw "nsh publish failed for $name"
        }
    }

    $stage = Join-Path $env:TEMP "steward-pkg-$name"
    if (Test-Path $stage) {
        Remove-Item -Recurse -Force -LiteralPath $stage
    }
    New-Item -ItemType Directory -Force -Path (Join-Path $stage $name) | Out-Null
    Copy-Item -LiteralPath $manifest -Destination (Join-Path $stage $name)
    Copy-Item -LiteralPath $package -Destination (Join-Path $stage $name)

    $zip = Join-Path $OutDir "steward-plugin-$name-$version.zip"
    if (Test-Path $zip) {
        Remove-Item -Force -LiteralPath $zip
    }
    Compress-Archive -Path (Join-Path $stage $name) -DestinationPath $zip
    $sizeKb = [math]::Round((Get-Item $zip).Length / 1KB, 1)
    Write-Host "Built plugin package: $zip ($sizeKb KB)"
    Remove-Item -Recurse -Force -LiteralPath $stage
}
