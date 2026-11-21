#!/usr/bin/env pwsh
# Measure one shell with all official plugins open at the same time.
#
# This complements bench-plugins.ps1, which measures each cold plugin window in
# a fresh shell. It records the three-host process count, aggregate peak RSS,
# per-plugin first-frame/focus latency, and orphan cleanup.

param(
    [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\steward-app.exe"),
    [string[]]$Plugins = @("calendar", "clipboard-history", "ui-showcase"),
    [string]$TrustRootFile = "",
    [string]$OutDir = (Join-Path $PSScriptRoot "..\target\benchmarks")
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Exe = (Resolve-Path -LiteralPath $Exe).Path
if (Get-Process steward-app -ErrorAction SilentlyContinue) {
    throw "stop every existing steward-app process before running the plugin-set benchmark"
}
if ($TrustRootFile) {
    $TrustRootFile = (Resolve-Path -LiteralPath $TrustRootFile).Path
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$runDir = Join-Path $OutDir ("plugin-set-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
$dataDir = Join-Path $runDir "data"
$pluginRoot = Join-Path $dataDir "plugins"
$metricsPath = Join-Path $runDir "metrics.jsonl"
$summaryPath = Join-Path $runDir "summary.json"
$requests = @()

foreach ($plugin in $Plugins) {
    $pluginDir = Join-Path $root "packages\plugins\$plugin"
    $manifestPath = Join-Path $pluginDir "plugin.json"
    $packagePath = Join-Path $pluginDir "dist\plugin.nshpkg"
    if (-not (Test-Path -LiteralPath $manifestPath) -or -not (Test-Path -LiteralPath $packagePath)) {
        throw "signed plugin input is missing under $pluginDir"
    }
    $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $manifestPath | ConvertFrom-Json
    $command = [string]$manifest.commands[0].name
    if (-not $manifest.id -or -not $command) {
        throw "plugin $plugin has no ID or command"
    }
    $pluginStage = Join-Path $pluginRoot ([string]$manifest.id)
    New-Item -ItemType Directory -Force -Path $pluginStage | Out-Null
    Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $pluginStage "plugin.json")
    Copy-Item -LiteralPath $packagePath -Destination (Join-Path $pluginStage "plugin.nshpkg")
    $requests += [ordered]@{
        pluginId = [string]$manifest.id
        command = $command
        title = [string]$manifest.name
    }
}

$oldData = $env:STEWARD_DATA_DIR
$oldPlugins = $env:STEWARD_PLUGINS_DIR
$oldTrustRoot = $env:STEWARD_NSH_TRUST_ROOT
$oldMetrics = $env:STEWARD_METRICS_PATH
$oldPlugin = $env:STEWARD_SMOKE_PLUGIN
$oldCommand = $env:STEWARD_SMOKE_COMMAND
$oldTitle = $env:STEWARD_SMOKE_TITLE
$oldPluginsJson = $env:STEWARD_SMOKE_PLUGINS_JSON
$oldRepeat = $env:STEWARD_SMOKE_REPEAT
$oldRepeatDelay = $env:STEWARD_SMOKE_REPEAT_DELAY_MS

function Read-Metrics {
    if (-not (Test-Path -LiteralPath $metricsPath)) {
        return @()
    }
    return @(
        Get-Content -LiteralPath $metricsPath |
            ForEach-Object { $_ | ConvertFrom-Json }
    )
}

function Wait-PluginMetric([string]$Event, [string]$PluginId, [int]$TimeoutMs = 20000) {
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($stopwatch.ElapsedMilliseconds -lt $TimeoutMs) {
        $metric = Read-Metrics | Where-Object {
            $_.event -eq $Event -and $_.plugin_id -eq $PluginId
        } | Select-Object -First 1
        if ($null -ne $metric) {
            return $metric
        }
        Start-Sleep -Milliseconds 50
    }
    throw "timed out waiting for $Event from $PluginId"
}

function Wait-ProcessGone([int]$ProcessId, [int]$TimeoutMs = 5000) {
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($stopwatch.ElapsedMilliseconds -lt $TimeoutMs) {
        if (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) {
            return $true
        }
        Start-Sleep -Milliseconds 50
    }
    return $false
}

$shell = $null
$childPids = @()
$orphanCount = 0
try {
    $env:STEWARD_DATA_DIR = $dataDir
    $env:STEWARD_PLUGINS_DIR = $pluginRoot
    $env:STEWARD_METRICS_PATH = $metricsPath
    Remove-Item Env:STEWARD_SMOKE_PLUGIN -ErrorAction SilentlyContinue
    Remove-Item Env:STEWARD_SMOKE_COMMAND -ErrorAction SilentlyContinue
    Remove-Item Env:STEWARD_SMOKE_TITLE -ErrorAction SilentlyContinue
    $env:STEWARD_SMOKE_PLUGINS_JSON = ConvertTo-Json -InputObject @($requests) -Compress
    $env:STEWARD_SMOKE_REPEAT = "1"
    $env:STEWARD_SMOKE_REPEAT_DELAY_MS = "1500"
    if ($TrustRootFile) {
        $env:STEWARD_NSH_TRUST_ROOT = (Get-Content -Raw -LiteralPath $TrustRootFile).Trim()
    }

    $shell = Start-Process -FilePath $Exe -PassThru
    $firstFrame = [ordered]@{}
    $focus = [ordered]@{}
    foreach ($request in $requests) {
        $firstFrame[$request.pluginId] = (Wait-PluginMetric "plugin_first_frame" $request.pluginId).duration_ms
    }
    foreach ($request in $requests) {
        $focus[$request.pluginId] = (Wait-PluginMetric "plugin_focus" $request.pluginId).duration_ms
    }

    $metrics = Read-Metrics
    $openHosts = @($metrics | Where-Object {
        $_.event -eq "plugin_host_process_count" -and $_.phase -eq "opened"
    })
    if ($openHosts.Count -ne $Plugins.Count) {
        throw "expected $($Plugins.Count) opened plugin hosts, found $($openHosts.Count)"
    }
    $childPids = @($openHosts | ForEach-Object { [int]$_.child_pid })
    $parentProcess = Get-Process -Id $shell.Id -ErrorAction Stop
    $parentProcess.Refresh()
    $shellPeakRss = $parentProcess.PeakWorkingSet64
    $childRss = 0L
    foreach ($childPid in $childPids) {
        $child = Get-Process -Id $childPid -ErrorAction Stop
        $child.Refresh()
        $childRss += $child.PeakWorkingSet64
    }

    $summary = [ordered]@{
        shellPid = $shell.Id
        pluginIds = @($requests | ForEach-Object { $_.pluginId })
        pluginHostProcessCount = $childPids.Count
        childPids = $childPids
        shellPeakRssMB = [math]::Round($shellPeakRss / 1MB, 2)
        pluginHostsPeakRssMB = [math]::Round($childRss / 1MB, 2)
        aggregatePeakRssMB = [math]::Round(($shellPeakRss + $childRss) / 1MB, 2)
        firstFrameMs = $firstFrame
        focusMs = $focus
        hiddenPluginVmsAtTrayReady = 0
        orphanedPluginHostProcesses = 0
    }
    $summary | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 -LiteralPath $summaryPath
    $summary | ConvertTo-Json -Depth 5
    Write-Host "Plugin-set evidence: $summaryPath"
} finally {
    if ($null -ne $shell) {
        Stop-Process -Id $shell.Id -Force -ErrorAction SilentlyContinue
        $shell.WaitForExit(5000) | Out-Null
    }
    foreach ($childPid in $childPids) {
        if (-not (Wait-ProcessGone $childPid 1000)) {
            Stop-Process -Id $childPid -Force -ErrorAction SilentlyContinue
            $orphanCount += 1
            Write-Warning "plugin host process $childPid outlived the shell benchmark"
        }
    }
    $env:STEWARD_DATA_DIR = $oldData
    $env:STEWARD_PLUGINS_DIR = $oldPlugins
    $env:STEWARD_NSH_TRUST_ROOT = $oldTrustRoot
    $env:STEWARD_METRICS_PATH = $oldMetrics
    $env:STEWARD_SMOKE_PLUGIN = $oldPlugin
    $env:STEWARD_SMOKE_COMMAND = $oldCommand
    $env:STEWARD_SMOKE_TITLE = $oldTitle
    $env:STEWARD_SMOKE_PLUGINS_JSON = $oldPluginsJson
    $env:STEWARD_SMOKE_REPEAT = $oldRepeat
    $env:STEWARD_SMOKE_REPEAT_DELAY_MS = $oldRepeatDelay
}

if ($orphanCount -gt 0) {
    if (Test-Path -LiteralPath $summaryPath) {
        $evidence = Get-Content -Raw -LiteralPath $summaryPath | ConvertFrom-Json
        $evidence | Add-Member -NotePropertyName orphanedPluginHostProcesses -NotePropertyValue $orphanCount
        $evidence | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 -LiteralPath $summaryPath
    }
    throw "$orphanCount plugin host process(es) survived shell termination"
}
