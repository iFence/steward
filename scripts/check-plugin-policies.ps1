#!/usr/bin/env pwsh
# Verify production plugin archives with Steward's installability and package
# policy checks. This complements `nsh verify`: it also enforces Steward's
# two-file archive contract, manifest/package identity, declared build, and
# release capability ceiling. Use -PublicAdmission to reject every declaration
# that needs manual policy review.

param(
    [string]$PackagesDir = (Join-Path $PSScriptRoot "..\target\plugins"),
    [string]$DataDir = "",
    [switch]$PublicAdmission
)

$ErrorActionPreference = "Stop"
$PackagesDir = (Resolve-Path -LiteralPath $PackagesDir).Path
$archives = @(Get-ChildItem -LiteralPath $PackagesDir -Filter "*.zip" -File)
if ($archives.Count -eq 0) {
    throw "no plugin archives found under $PackagesDir"
}
$ownsDataDir = -not $DataDir
if (-not $DataDir) {
    $DataDir = Join-Path ([System.IO.Path]::GetTempPath()) (
        "steward-policy-" + [guid]::NewGuid().ToString("N")
    )
}
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null

try {
    foreach ($archive in $archives) {
        $action = if ($PublicAdmission) { "review" } else { "check" }
        & cargo run --locked --quiet -p steward-plugin-cli --bin steward-plugin -- `
            --data-dir $DataDir $action $archive.FullName
        if ($LASTEXITCODE -ne 0) {
            throw "Steward package admission failed for $($archive.Name)"
        }
        Write-Host "Steward policy passed for $($archive.Name) ($action)."
    }
} finally {
    if ($ownsDataDir -and (Test-Path -LiteralPath $DataDir)) {
        Remove-Item -Recurse -Force -LiteralPath $DataDir
    }
}
