#!/usr/bin/env pwsh
# Verify production plugin archives with Steward's installability and package
# policy checks. This complements `nsh verify`: it also enforces Steward's
# two-file archive contract, manifest/package identity, declared build, and
# release capability ceiling. Use -PublicAdmission to reject every declaration
# that needs manual policy review.

param(
    [string]$PackagesDir = (Join-Path $PSScriptRoot "..\target\plugins"),
    [string]$DataDir = "",
    [string]$ReportDir = "",
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
    if ($PublicAdmission) {
        if (-not $ReportDir) {
            $ReportDir = Join-Path $PackagesDir "admission"
        }
        New-Item -ItemType Directory -Force -Path $ReportDir | Out-Null
        $reviewed = @()
        foreach ($archive in $archives) {
            $reportPath = Join-Path $ReportDir ($archive.BaseName + ".json")
            $json = (& cargo run --locked --quiet -p steward-plugin-cli --bin steward-plugin -- `
                --data-dir $DataDir review $archive.FullName --output $reportPath --json |
                Out-String)
            if ($LASTEXITCODE -ne 0) {
                throw "Steward public admission failed for $($archive.Name); report: $reportPath"
            }
            $report = $json | ConvertFrom-Json
            if (-not $report.accepted -or $report.policyVersion -ne 1) {
                throw "invalid public admission report for $($archive.Name)"
            }
            $reviewed += [pscustomobject]@{
                Archive = $archive.FullName
                Name = $archive.BaseName
                Report = $reportPath
                PluginId = $report.pluginId
                Version = $report.version
                Build = [UInt64]$report.build
            }
            Write-Host "Steward policy passed for $($archive.Name) (review)."
        }

        foreach ($group in ($reviewed | Group-Object PluginId)) {
            $ordered = @($group.Group | Sort-Object Build)
            for ($index = 1; $index -lt $ordered.Count; $index++) {
                $current = $ordered[$index]
                $previous = $ordered[$index - 1]
                $json = (& cargo run --locked --quiet -p steward-plugin-cli --bin steward-plugin -- `
                    --data-dir $DataDir review $current.Archive --previous $previous.Archive `
                    --output $current.Report --json | Out-String)
                if ($LASTEXITCODE -ne 0) {
                    throw "Steward capability-delta review failed for $($current.Name)"
                }
                $report = $json | ConvertFrom-Json
                if ($report.previousVersion -ne $previous.Version) {
                    throw "capability delta for $($current.Name) used the wrong previous release"
                }
            }
        }
        Write-Host "Public admission reports written to $ReportDir."
    } else {
        foreach ($archive in $archives) {
            & cargo run --locked --quiet -p steward-plugin-cli --bin steward-plugin -- `
                --data-dir $DataDir check $archive.FullName
            if ($LASTEXITCODE -ne 0) {
                throw "Steward package policy failed for $($archive.Name)"
            }
            Write-Host "Steward policy passed for $($archive.Name) (check)."
        }
    }
} finally {
    if ($ownsDataDir -and (Test-Path -LiteralPath $DataDir)) {
        Remove-Item -Recurse -Force -LiteralPath $DataDir
    }
}
