#!/usr/bin/env pwsh
# Verify every packaged release plugin against the committed production root
# and enforce the two-file distribution layout.

param(
    [string]$PackagesDir = (Join-Path $PSScriptRoot "..\target\plugins"),
    [string]$TrustRoots = (Join-Path $PSScriptRoot "..\assets\nsh\trust-roots.json")
)

$ErrorActionPreference = "Stop"
$PackagesDir = (Resolve-Path -LiteralPath $PackagesDir).Path
$TrustRoots = (Resolve-Path -LiteralPath $TrustRoots).Path
if (-not (Get-Command nsh -ErrorAction SilentlyContinue)) {
    throw "nsh is required to verify release packages"
}

$roots = @(
    Get-Content -Raw -Encoding UTF8 -LiteralPath $TrustRoots |
        ConvertFrom-Json
)
if ($roots.Count -eq 0) {
    throw "no production NSH trust root is configured"
}

$archives = @(Get-ChildItem -LiteralPath $PackagesDir -Filter "*.zip" -File)
if ($archives.Count -eq 0) {
    throw "no plugin archives found under $PackagesDir"
}

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("steward-verify-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
try {
    $rootFiles = @()
    for ($index = 0; $index -lt $roots.Count; $index++) {
        $path = Join-Path $tempRoot "root-$index.pub"
        [System.IO.File]::WriteAllText(
            $path,
            ([string]$roots[$index]).Trim() + [Environment]::NewLine,
            [System.Text.UTF8Encoding]::new($false)
        )
        $rootFiles += $path
    }

    foreach ($archive in $archives) {
        $extract = Join-Path $tempRoot $archive.BaseName
        New-Item -ItemType Directory -Force -Path $extract | Out-Null
        Expand-Archive -LiteralPath $archive.FullName -DestinationPath $extract
        $files = @(Get-ChildItem -LiteralPath $extract -Recurse -File)
        $packages = @($files | Where-Object { $_.Name -eq "plugin.nshpkg" })
        $manifests = @($files | Where-Object { $_.Name -eq "plugin.json" })
        if ($files.Count -ne 2 -or $packages.Count -ne 1 -or $manifests.Count -ne 1) {
            throw "$($archive.Name) must contain exactly plugin.json and plugin.nshpkg"
        }
        if ($packages[0].DirectoryName -ne $manifests[0].DirectoryName) {
            throw "$($archive.Name) metadata and signed package must share one plugin directory"
        }

        $nshArgs = @("verify", $packages[0].FullName)
        foreach ($root in $rootFiles) {
            $nshArgs += @("--root", $root)
        }
        & nsh @nshArgs
        if ($LASTEXITCODE -ne 0) {
            throw "production-root verification failed for $($archive.Name)"
        }
        Write-Host "Verified $($archive.Name) against the production trust root."
    }
} finally {
    if (Test-Path -LiteralPath $tempRoot) {
        Remove-Item -Recurse -Force -LiteralPath $tempRoot
    }
}
