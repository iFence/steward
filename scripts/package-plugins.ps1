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
#   .\scripts\package-plugins.ps1 -ForceSign # always rebuild signed packages

param(
    [string[]]$Plugin = @(),
    [string]$OutDir = "",
    [switch]$ForceSign
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
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
    $id = [string]$meta.id
    $version = $meta.version
    if (
        [string]::IsNullOrWhiteSpace($id) -or
        $meta.schemaVersion -notin @(2, 3) -or
        $meta.package -ne "plugin.nshpkg"
    ) {
        throw "plugin.json in $dir is not NSH schema v2 or v3"
    }
    if (-not $version) {
        throw "plugin.json in $dir has no version"
    }

    $package = Join-Path $dir "dist\plugin.nshpkg"
    if ($ForceSign -or -not (Test-Path -LiteralPath $package)) {
        $key = $env:STEWARD_NSH_RELEASE_KEY
        $cert = $env:STEWARD_NSH_RELEASE_CERT
        if (-not $key -or -not $cert -or -not (Get-Command nsh -ErrorAction SilentlyContinue)) {
            throw "missing signed package for ${name}: set STEWARD_NSH_RELEASE_KEY/STEWARD_NSH_RELEASE_CERT and install nsh"
        }
        if (Test-Path -LiteralPath $package) {
            Remove-Item -Force -LiteralPath $package
        }
        & nsh publish (Join-Path $dir "App.tsx") --key $key --cert $cert -o $package
        if ($LASTEXITCODE -ne 0) {
            throw "nsh publish failed for $name"
        }
        if (-not (Test-Path -LiteralPath $package)) {
            throw "nsh publish reported success but did not create $package"
        }
    }

    $stage = Join-Path $env:TEMP "steward-pkg-$id"
    if (Test-Path $stage) {
        Remove-Item -Recurse -Force -LiteralPath $stage
    }
    New-Item -ItemType Directory -Force -Path (Join-Path $stage $id) | Out-Null
    Copy-Item -LiteralPath $manifest -Destination (Join-Path $stage $id)
    Copy-Item -LiteralPath $package -Destination (Join-Path $stage $id)

    $zip = Join-Path $OutDir "steward-plugin-$name-$version.zip"
    if (Test-Path $zip) {
        Remove-Item -Force -LiteralPath $zip
    }
    # Compress-Archive writes backslash separators on Windows PowerShell 5.1,
    # which violates the zip path contract enforced by package inspection.
    $zipStream = [System.IO.File]::Open(
        $zip,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $zipArchive = New-Object System.IO.Compression.ZipArchive(
            $zipStream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false
        )
        try {
            foreach ($fileName in @("plugin.json", "plugin.nshpkg")) {
                $entry = $zipArchive.CreateEntry(
                    "$id/$fileName",
                    [System.IO.Compression.CompressionLevel]::Optimal
                )
                $source = Join-Path (Join-Path $stage $id) $fileName
                $input = [System.IO.File]::OpenRead($source)
                try {
                    $output = $entry.Open()
                    try {
                        $input.CopyTo($output)
                    } finally {
                        $output.Dispose()
                    }
                } finally {
                    $input.Dispose()
                }
            }
        } finally {
            $zipArchive.Dispose()
        }
    } finally {
        $zipStream.Dispose()
    }
    $sizeKb = [math]::Round((Get-Item $zip).Length / 1KB, 1)
    Write-Host "Built plugin package: $zip ($sizeKb KB)"
    Remove-Item -Recurse -Force -LiteralPath $stage
}
