#!/usr/bin/env pwsh
# Build distributable plugin packages: one zip per plugin under
# `packages/plugins`, written to `target/plugins`.
#
# Each zip contains a single top-level folder named after the plugin, so
# extracting it into the app's plugin root (`%APPDATA%\Steward\plugins`)
# creates `<root>\<plugin>\plugin.json` directly.
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
    $meta = Get-Content -Raw -LiteralPath $manifest | ConvertFrom-Json
    $version = $meta.version
    if (-not $version) {
        throw "plugin.json in $dir has no version"
    }

    $stage = Join-Path $env:TEMP "steward-pkg-$name"
    if (Test-Path $stage) {
        Remove-Item -Recurse -Force -LiteralPath $stage
    }
    New-Item -ItemType Directory -Force -Path (Join-Path $stage $name) | Out-Null
    Copy-Item -LiteralPath $manifest -Destination (Join-Path $stage $name)
    if (Test-Path (Join-Path $dir "dist")) {
        Copy-Item -LiteralPath (Join-Path $dir "dist") -Destination (Join-Path $stage $name) -Recurse
    }

    $zip = Join-Path $OutDir "steward-plugin-$name-$version.zip"
    if (Test-Path $zip) {
        Remove-Item -Force -LiteralPath $zip
    }
    Compress-Archive -Path (Join-Path $stage $name) -DestinationPath $zip
    $sizeKb = [math]::Round((Get-Item $zip).Length / 1KB, 1)
    Write-Host "Built plugin package: $zip ($sizeKb KB)"
    Remove-Item -Recurse -Force -LiteralPath $stage
}
