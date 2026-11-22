#!/usr/bin/env pwsh
# Exercise the PDX-2 developer loop without touching the installed Steward
# profile: create a plugin, rebuild/sign/replace twice at the same version,
# queue static reloads, run doctor, and verify app-side profile isolation.

$ErrorActionPreference = "Stop"

$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
foreach ($command in @("cargo", "nsh")) {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "$command is required for the PDX-2 development lifecycle test"
    }
}

$tempRoot = Join-Path $root ("target\pdx2-development-" + [guid]::NewGuid().ToString("N"))
$project = Join-Path $tempRoot "project"
$dataDir = Join-Path $tempRoot "data"
$pluginsDir = Join-Path $dataDir "plugins"
$signing = Join-Path $tempRoot "signing"
$cargoTarget = Join-Path $root "target"
$pluginId = "com.example.pdx2-development"

try {
    New-Item -ItemType Directory -Force -Path $project, $signing | Out-Null

    @'
import { mount, Text, View } from "nsh";

function App() {
  return (
    <View style={{ flex: 1, alignItems: "center", justifyContent: "center" }}>
      <Text>PDX-2 revision one</Text>
    </View>
  );
}

mount(App);
'@ | Set-Content -Encoding ASCII -LiteralPath (Join-Path $project "App.tsx")

    @'
{
  "schemaVersion": 3,
  "id": "com.example.pdx2-development",
  "name": "PDX-2 Development",
  "version": "0.1.0",
  "minHostVersion": "0.5.0",
  "package": "plugin.nshpkg",
  "commands": [
    {
      "name": "pdx2-dev",
      "title": "PDX-2 Development",
      "trigger": { "type": "command" }
    }
  ]
}
'@ | Set-Content -Encoding ASCII -LiteralPath (Join-Path $project "plugin.json")

    @'
{
  "name": "PDX-2 Development",
  "id": "com.example.pdx2-development",
  "version": "0.1.0",
  "build": 1,
  "capabilities": {
    "host": []
  }
}
'@ | Set-Content -Encoding ASCII -LiteralPath (Join-Path $project "nsh.json")

    $rootKey = Join-Path $signing "root"
    $releaseKey = Join-Path $signing "release"
    $certificate = Join-Path $signing "release.cert"
    & nsh keygen $rootKey
    if ($LASTEXITCODE -ne 0) {
        throw "PDX-2 root key generation failed"
    }
    & nsh keygen $releaseKey
    if ($LASTEXITCODE -ne 0) {
        throw "PDX-2 release key generation failed"
    }
    & nsh certify `
        --root "$rootKey.secret" `
        --key "$releaseKey.pub" `
        --apps $pluginId `
        --days 1 `
        -o $certificate
    if ($LASTEXITCODE -ne 0) {
        throw "PDX-2 development certificate failed"
    }
    $trustRoot = (Get-Content -Raw -Encoding UTF8 -LiteralPath "$rootKey.pub").Trim()

    $cliArgs = @(
        "run", "--quiet", "-p", "steward-plugin-cli", "--",
        "--developer-mode",
        "--developer-data-dir", $dataDir,
        "--developer-plugins-dir", $pluginsDir,
        "--trust-root", $trustRoot
    )
    $commonDevArgs = @(
        "dev", $project,
        "--build-command", "nsh build App.tsx --release -o dist",
        "--signing-key", "$releaseKey.secret",
        "--signing-cert", $certificate
    )

    & cargo @cliArgs @commonDevArgs
    if ($LASTEXITCODE -ne 0) {
        throw "first PDX-2 development build failed"
    }
    $installedPackage = Join-Path $pluginsDir "$pluginId\plugin.nshpkg"
    if (-not (Test-Path -LiteralPath $installedPackage)) {
        throw "PDX-2 package was not installed into the isolated plugin directory"
    }
    $firstHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installedPackage).Hash

    $appSource = Join-Path $project "App.tsx"
    $updatedSource = (Get-Content -Raw -Encoding UTF8 -LiteralPath $appSource) -replace "revision one", "revision two"
    Set-Content -Encoding ASCII -LiteralPath $appSource -Value $updatedSource
    & cargo @cliArgs @commonDevArgs
    if ($LASTEXITCODE -ne 0) {
        throw "second same-version PDX-2 development replacement failed"
    }
    $secondHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installedPackage).Hash
    if ($firstHash -eq $secondHash) {
        throw "same-version development replacement did not activate the rebuilt package"
    }

    $reloadDirectory = Join-Path $dataDir "developer\reloads"
    $reloadCount = @(Get-ChildItem -LiteralPath $reloadDirectory -Filter "*.json" -File).Count
    if ($reloadCount -lt 2) {
        throw "PDX-2 did not queue a static reload for each rebuild"
    }

    & cargo @cliArgs doctor $pluginId
    if ($LASTEXITCODE -ne 0) {
        throw "doctor rejected the installed PDX-2 plugin"
    }

    $badLayout = Join-Path $tempRoot "bad-layout"
    New-Item -ItemType Directory -Force -Path $badLayout | Out-Null
    Copy-Item -LiteralPath (Join-Path $pluginsDir "$pluginId\plugin.json") -Destination $badLayout
    Copy-Item -LiteralPath $installedPackage -Destination $badLayout
    Set-Content -LiteralPath (Join-Path $badLayout "README.txt") -Value "unexpected"
    $previousErrorAction = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        & cargo @cliArgs doctor $badLayout *> $null
        $doctorExitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorAction
    }
    if ($doctorExitCode -eq 0) {
        throw "doctor accepted an invalid package layout"
    }

    $report = Join-Path $tempRoot "diagnostics.json"
    $sentinel = Join-Path $tempRoot "must-not-be-used"
    $defaultApp = Join-Path $root "target\debug\steward-app.exe"
    $appIsRunning = @(
        Get-Process -Name "steward-app" -ErrorAction SilentlyContinue |
            Where-Object {
                $_.Path -and
                [string]::Equals(
                    [System.IO.Path]::GetFullPath($_.Path),
                    [System.IO.Path]::GetFullPath($defaultApp),
                    [System.StringComparison]::OrdinalIgnoreCase
                )
            }
    ).Count -gt 0
    $appTargetArgs = @()
    if ($appIsRunning) {
        # The default executable is held by a running local app. Use an
        # isolated target only for this verification process; CI uses the
        # shared cache because no app is running there.
        $appTargetArgs = @("--target-dir", (Join-Path $tempRoot "cargo-target"))
    }
    $previousDataDir = $env:STEWARD_DATA_DIR
    try {
        $env:STEWARD_DATA_DIR = $sentinel
        & cargo run --quiet @appTargetArgs -p steward-app -- `
            --developer-mode `
            --developer-data-dir $dataDir `
            --developer-plugins-dir $pluginsDir `
            --developer-trust-root "$rootKey.pub" `
            --write-diagnostics $report
        if ($LASTEXITCODE -ne 0) {
            throw "developer-mode diagnostics command failed"
        }
    } finally {
        $env:STEWARD_DATA_DIR = $previousDataDir
    }
    $diagnostics = Get-Content -Raw -Encoding UTF8 -LiteralPath $report | ConvertFrom-Json
    if ($diagnostics.data.dataDir -ne $dataDir) {
        throw "developer mode used '$($diagnostics.data.dataDir)' instead of isolated data '$dataDir'"
    }

    Write-Host "PDX-2 development lifecycle passed."
} finally {
    if (Test-Path -LiteralPath $tempRoot) {
        Remove-Item -Recurse -Force -LiteralPath $tempRoot
    }
}
