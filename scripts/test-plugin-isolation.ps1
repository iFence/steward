#!/usr/bin/env pwsh
# Exercise plugin-host crash containment and the user-initiated restart backoff.
#
# The smoke hook launches one signed plugin, repeats the command after 2 s,
# and this script terminates the plugin host immediately after its first frame.
# The repeated command must be rejected while the shell stays alive.

param(
    [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\steward-app.exe"),
    [string]$Plugin = "calendar",
    [string]$PackagePath = "",
    [string]$TrustRootFile = "",
    [string]$OutDir = (Join-Path $PSScriptRoot "..\target\acceptance"),
    [int]$MemoryLimitMiB = 0
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Exe = (Resolve-Path -LiteralPath $Exe).Path
if (Get-Process steward-app -ErrorAction SilentlyContinue) {
    throw "stop every existing steward-app process before running isolation acceptance"
}

$pluginDir = Join-Path $root "packages\plugins\$Plugin"
$manifestPath = Join-Path $pluginDir "plugin.json"
$packagePath = if ($PackagePath) {
    (Resolve-Path -LiteralPath $PackagePath).Path
} else {
    Join-Path $pluginDir "dist\plugin.nshpkg"
}
if ($TrustRootFile) {
    $TrustRootFile = (Resolve-Path -LiteralPath $TrustRootFile).Path
}
if (-not (Test-Path -LiteralPath $manifestPath) -or -not (Test-Path -LiteralPath $packagePath)) {
    throw "signed plugin input is missing under $pluginDir"
}

$manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $manifestPath | ConvertFrom-Json
$command = [string]$manifest.commands[0].name
if (-not $command) {
    throw "plugin $Plugin has no command"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$runDir = Join-Path $OutDir ("plugin-isolation-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
$dataDir = Join-Path $runDir "data"
$pluginRoot = Join-Path $dataDir "plugins"
$pluginStage = Join-Path $pluginRoot ([string]$manifest.id)
New-Item -ItemType Directory -Force -Path $pluginStage | Out-Null
Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $pluginStage "plugin.json")
Copy-Item -LiteralPath $packagePath -Destination (Join-Path $pluginStage "plugin.nshpkg")
$metricsPath = Join-Path $runDir "metrics.jsonl"

$oldData = $env:STEWARD_DATA_DIR
$oldPlugins = $env:STEWARD_PLUGINS_DIR
$oldTrustRoot = $env:STEWARD_NSH_TRUST_ROOT
$oldMetrics = $env:STEWARD_METRICS_PATH
$oldPlugin = $env:STEWARD_SMOKE_PLUGIN
$oldCommand = $env:STEWARD_SMOKE_COMMAND
$oldTitle = $env:STEWARD_SMOKE_TITLE
$oldRepeat = $env:STEWARD_SMOKE_REPEAT
$oldRepeatDelay = $env:STEWARD_SMOKE_REPEAT_DELAY_MS
$oldMemoryLimit = $env:STEWARD_PLUGIN_MEMORY_LIMIT_MIB

function Read-Metrics([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) {
        return @()
    }
    return @(
        Get-Content -LiteralPath $Path |
            ForEach-Object { $_ | ConvertFrom-Json }
    )
}

function Wait-Metric(
    [string]$Path,
    [string]$Event,
    [int]$TimeoutMs = 20000
) {
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($stopwatch.ElapsedMilliseconds -lt $TimeoutMs) {
        $metric = Read-Metrics $Path | Where-Object { $_.event -eq $Event } |
            Select-Object -First 1
        if ($null -ne $metric) {
            return $metric
        }
        Start-Sleep -Milliseconds 20
    }
    throw "timed out waiting for metric '$Event' in $Path"
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
$childPid = 0
try {
    $env:STEWARD_DATA_DIR = $dataDir
    $env:STEWARD_PLUGINS_DIR = $pluginRoot
    $env:STEWARD_METRICS_PATH = $metricsPath
    $env:STEWARD_SMOKE_PLUGIN = [string]$manifest.id
    $env:STEWARD_SMOKE_COMMAND = $command
    $env:STEWARD_SMOKE_TITLE = [string]$manifest.name
    $env:STEWARD_SMOKE_REPEAT = "1"
    $env:STEWARD_SMOKE_REPEAT_DELAY_MS = "2000"
    if ($MemoryLimitMiB -gt 0) {
        $env:STEWARD_PLUGIN_MEMORY_LIMIT_MIB = [string]$MemoryLimitMiB
    } else {
        Remove-Item Env:STEWARD_PLUGIN_MEMORY_LIMIT_MIB -ErrorAction SilentlyContinue
    }
    if ($TrustRootFile) {
        $env:STEWARD_NSH_TRUST_ROOT = (Get-Content -Raw -LiteralPath $TrustRootFile).Trim()
    }

    $shell = Start-Process -FilePath $Exe -PassThru
    $containment = Wait-Metric $metricsPath "plugin_host_containment"
    if ($containment.backend -ne "windows_job_object") {
        throw "plugin host did not use Windows Job Object containment"
    }
    $childPid = [int]$containment.value
    $backoff = $null
    if ($MemoryLimitMiB -gt 0) {
        $failure = Wait-Metric $metricsPath "plugin_process_failure"
        if ($failure.error_code -ne "resource_limit") {
            throw "expected resource_limit, got '$($failure.error_code)'"
        }
    } else {
        $null = Wait-Metric $metricsPath "plugin_first_frame"
        Stop-Process -Id $childPid -Force -ErrorAction Stop
        $failure = Wait-Metric $metricsPath "plugin_process_failure"
        $backoff = Wait-Metric $metricsPath "plugin_launch_backoff"
    }
    if (-not (Wait-ProcessGone $childPid)) {
        throw "plugin host process $childPid did not exit"
    }
    $shell.Refresh()
    if ($shell.HasExited) {
        throw "the Steward shell exited after its plugin host crashed"
    }

    $hostCount = Read-Metrics $metricsPath | Where-Object {
        $_.event -eq "plugin_host_process_count"
    } | Select-Object -Last 1
    if ($null -eq $hostCount -or $hostCount.value -ne 0) {
        throw "plugin host process count did not return to zero after the crash"
    }

    $summary = [ordered]@{
        plugin = [string]$manifest.id
        childPid = $childPid
        shellPid = $shell.Id
        backend = $containment.backend
        memoryLimitBytes = $containment.memory_limit_bytes
        cpuRatePercent = $containment.cpu_rate_percent
        activeProcessLimit = $containment.active_process_limit
        handleLimit = $containment.handle_limit
        failureCode = $failure.error_code
        retryDelayMs = $failure.retry_delay_ms
        rejectedRetryAfterMs = if ($null -eq $backoff) { $null } else { $backoff.value }
        memoryLimitTestMiB = if ($MemoryLimitMiB -gt 0) { $MemoryLimitMiB } else { $null }
        shellSurvivedCrash = $true
        pluginHostProcessesAfterCrash = $hostCount.value
    }
    $summaryPath = Join-Path $runDir "summary.json"
    $summary | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 -LiteralPath $summaryPath
    $summary | ConvertTo-Json -Depth 4
    Write-Host "Plugin isolation evidence: $summaryPath"
} finally {
    if ($null -ne $shell) {
        Stop-Process -Id $shell.Id -Force -ErrorAction SilentlyContinue
        $shell.WaitForExit(5000) | Out-Null
    }
    if ($childPid -ne 0) {
        Stop-Process -Id $childPid -Force -ErrorAction SilentlyContinue
    }
    $env:STEWARD_DATA_DIR = $oldData
    $env:STEWARD_PLUGINS_DIR = $oldPlugins
    $env:STEWARD_NSH_TRUST_ROOT = $oldTrustRoot
    $env:STEWARD_METRICS_PATH = $oldMetrics
    $env:STEWARD_SMOKE_PLUGIN = $oldPlugin
    $env:STEWARD_SMOKE_COMMAND = $oldCommand
    $env:STEWARD_SMOKE_TITLE = $oldTitle
    $env:STEWARD_SMOKE_REPEAT = $oldRepeat
    $env:STEWARD_SMOKE_REPEAT_DELAY_MS = $oldRepeatDelay
    $env:STEWARD_PLUGIN_MEMORY_LIMIT_MIB = $oldMemoryLimit
}
