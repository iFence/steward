#!/usr/bin/env pwsh
# Exercise install, optional upgrade, repair, launch, and uninstall on a clean
# Windows 11 x64 acceptance VM. This intentionally refuses to modify a machine
# that already has Steward installed.

param(
    [Parameter(Mandatory = $true)]
    [string]$Msi,
    [string]$PreviousMsi = "",
    [string]$InstallDir = (Join-Path $env:ProgramFiles "Steward"),
    [string]$OutDir = (Join-Path $PSScriptRoot "..\target\acceptance")
)

$ErrorActionPreference = "Stop"
$Msi = (Resolve-Path -LiteralPath $Msi).Path
if ($PreviousMsi) {
    $PreviousMsi = (Resolve-Path -LiteralPath $PreviousMsi).Path
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "run this script from an elevated PowerShell session"
}
if (Test-Path -LiteralPath $InstallDir) {
    throw "$InstallDir already exists; run this script only on a clean acceptance VM"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$runDir = Join-Path $OutDir ("msi-lifecycle-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
New-Item -ItemType Directory -Force -Path $runDir | Out-Null
$dataDir = Join-Path $runDir "data"
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null

function Invoke-Msi([string]$Package, [string]$Action, [string]$Name) {
    $log = Join-Path $runDir "$Name.log"
    $arguments = @($Action, "`"$Package`"", "/qn", "/norestart", "/l*v", "`"$log`"")
    $process = Start-Process -FilePath "msiexec.exe" -ArgumentList $arguments -Wait -PassThru
    if ($process.ExitCode -notin @(0, 3010)) {
        throw "msiexec $Action failed for $Package with exit code $($process.ExitCode); see $log"
    }
}

function Assert-Installed([string]$ExpectedVersion) {
    $exe = Join-Path $InstallDir "steward-app.exe"
    if (-not (Test-Path -LiteralPath $exe)) {
        throw "installed executable is missing: $exe"
    }
    $actual = (Get-Item -LiteralPath $exe).VersionInfo.ProductVersion
    if ($actual -ne $ExpectedVersion) {
        throw "installed product version is '$actual', expected '$ExpectedVersion'"
    }
    $stdout = Join-Path $runDir "summon-hotkey.txt"
    $stderr = "$stdout.err"
    $env:STEWARD_DATA_DIR = $dataDir
    try {
        $process = Start-Process -FilePath $exe -ArgumentList "--print-summon-hotkey" `
            -RedirectStandardOutput $stdout -RedirectStandardError $stderr -Wait -PassThru
        if ($process.ExitCode -ne 0 -or -not (Get-Content -Raw -LiteralPath $stdout).Trim()) {
            throw "installed executable failed its launch smoke check"
        }
    } finally {
        Remove-Item Env:STEWARD_DATA_DIR -ErrorAction SilentlyContinue
    }
    Write-Host "Steward $ExpectedVersion installed and launched successfully."
}

try {
    if ($PreviousMsi) {
        Invoke-Msi $PreviousMsi "/i" "install-previous"
        $previousExe = Join-Path $InstallDir "steward-app.exe"
        $previousVersion = [string](Get-Item -LiteralPath $previousExe).VersionInfo.ProductVersion
        Assert-Installed $previousVersion
        Invoke-Msi $Msi "/i" "major-upgrade"
    } else {
        Invoke-Msi $Msi "/i" "clean-install"
    }

    $installedExe = Join-Path $InstallDir "steward-app.exe"
    $version = [string](Get-Item -LiteralPath $installedExe).VersionInfo.ProductVersion
    Assert-Installed $version

    Invoke-Msi $Msi "/famus" "repair"
    Assert-Installed $version

    Invoke-Msi $Msi "/x" "uninstall"
    if (Test-Path -LiteralPath $InstallDir) {
        throw "uninstall left the installation directory behind: $InstallDir"
    }
    Write-Host "MSI clean install, upgrade, repair, launch, and uninstall checks passed."
} finally {
    Remove-Item Env:STEWARD_DATA_DIR -ErrorAction SilentlyContinue
}
