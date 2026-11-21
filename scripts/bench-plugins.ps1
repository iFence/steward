#!/usr/bin/env pwsh
# Measure signed-plugin first-frame and existing-window focus latency.
#
# Run on the release acceptance VM after `cargo build --release -p steward-app`.
# Each iteration starts a fresh app process against the cached route catalog,
# opens the selected plugin, then repeats the command to exercise focus.

param(
    [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\steward-app.exe"),
    [string]$Plugin = "calendar",
    [int]$Iterations = 20,
    [string]$OutDir = (Join-Path $PSScriptRoot "..\target\benchmarks"),
    [string]$PackagePath = "",
    [string]$TrustRootFile = ""
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Exe = (Resolve-Path -LiteralPath $Exe).Path
if (-not (Test-Path -LiteralPath $Exe)) {
    throw "release executable not found: $Exe"
}
if (Get-Process steward-app -ErrorAction SilentlyContinue) {
    throw "stop every existing steward-app process before running the plugin benchmark"
}
if ($Iterations -lt 1) {
    throw "-Iterations must be at least 1"
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
if ($manifest.schemaVersion -ne 2 -or -not $manifest.id) {
    throw "invalid plugin.json under $pluginDir"
}
$command = [string]$manifest.commands[0].name
if (-not $command) {
    throw "plugin $Plugin has no command"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$runId = Get-Date -Format "yyyyMMdd-HHmmss"
$runDir = Join-Path $OutDir "plugins-$Plugin-$runId"
New-Item -ItemType Directory -Force -Path $runDir | Out-Null
$dataDir = Join-Path $runDir "data"
$pluginRoot = Join-Path $dataDir "plugins"
$pluginStage = Join-Path $pluginRoot ([string]$manifest.id)
New-Item -ItemType Directory -Force -Path $pluginStage | Out-Null
Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $pluginStage "plugin.json")
Copy-Item -LiteralPath $packagePath -Destination (Join-Path $pluginStage "plugin.nshpkg")

$oldData = $env:STEWARD_DATA_DIR
$oldPlugins = $env:STEWARD_PLUGINS_DIR
$oldTrustRoot = $env:STEWARD_NSH_TRUST_ROOT
$oldMetrics = $env:STEWARD_METRICS_PATH
$oldPlugin = $env:STEWARD_SMOKE_PLUGIN
$oldCommand = $env:STEWARD_SMOKE_COMMAND
$oldTitle = $env:STEWARD_SMOKE_TITLE
$oldRepeat = $env:STEWARD_SMOKE_REPEAT
$oldRepeatDelay = $env:STEWARD_SMOKE_REPEAT_DELAY_MS

function Read-Metric([string]$Path, [string]$Event) {
    if (-not (Test-Path -LiteralPath $Path)) {
        return @()
    }
    return @(
        Get-Content -LiteralPath $Path |
            ForEach-Object { $_ | ConvertFrom-Json } |
            Where-Object { $_.event -eq $Event }
    )
}

function Wait-Events([string]$Path, [string[]]$Events, [int]$TimeoutMs = 20000) {
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($stopwatch.ElapsedMilliseconds -lt $TimeoutMs) {
        $allReady = $true
        foreach ($eventName in $Events) {
            if (@(Read-Metric $Path $eventName).Count -eq 0) {
                $allReady = $false
                break
            }
        }
        if ($allReady) {
            return $true
        }
        Start-Sleep -Milliseconds 50
    }
    return $false
}

function Get-P95($Values) {
    $sorted = @($Values | Sort-Object)
    if ($sorted.Count -eq 0) {
        throw "cannot calculate p95 from an empty sample set"
    }
    $index = [math]::Ceiling($sorted.Count * 0.95) - 1
    return [double]$sorted[[math]::Max(0, $index)]
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

try {
    $env:STEWARD_DATA_DIR = $dataDir
    $env:STEWARD_PLUGINS_DIR = $pluginRoot
    if ($TrustRootFile) {
        $env:STEWARD_NSH_TRUST_ROOT = (Get-Content -Raw -LiteralPath $TrustRootFile).Trim()
    }

    # Populate the metadata cache once. Route discovery remains metadata-only;
    # no plugin VM is started during this warm-up.
    $warmupMetrics = Join-Path $runDir "warmup.jsonl"
    $env:STEWARD_METRICS_PATH = $warmupMetrics
    Remove-Item Env:STEWARD_SMOKE_PLUGIN -ErrorAction SilentlyContinue
    $warmup = Start-Process -FilePath $Exe -PassThru
    try {
        if (-not (Wait-Events $warmupMetrics @("hidden_plugin_vm_count"))) {
            throw "warm-up app did not reach tray-ready metrics"
        }
        Start-Sleep -Seconds 2
    } finally {
        Stop-Process -Id $warmup.Id -Force -ErrorAction SilentlyContinue
        $warmup.WaitForExit(5000) | Out-Null
    }
    $hidden = Read-Metric $warmupMetrics "hidden_plugin_vm_count" |
        Select-Object -First 1
    if ($null -eq $hidden -or $hidden.value -ne 0) {
        throw "tray-ready hidden plugin VM count must be zero"
    }

    $firstFrame = @()
    $focus = @()
    $pluginRss = @()
    $aggregateRss = @()
    $pluginHandles = @()
    $env:STEWARD_SMOKE_PLUGIN = [string]$manifest.id
    $env:STEWARD_SMOKE_COMMAND = $command
    $env:STEWARD_SMOKE_TITLE = [string]$manifest.name
    $env:STEWARD_SMOKE_REPEAT = "1"
    $env:STEWARD_SMOKE_REPEAT_DELAY_MS = "600"

    for ($i = 1; $i -le $Iterations; $i++) {
        $metricsPath = Join-Path $runDir ("iteration-{0:D2}.jsonl" -f $i)
        Remove-Item -LiteralPath $metricsPath -ErrorAction SilentlyContinue
        $env:STEWARD_METRICS_PATH = $metricsPath
        $process = Start-Process -FilePath $Exe -PassThru
        $childPid = 0
        try {
            $ready = Wait-Events $metricsPath @("plugin_first_frame", "plugin_focus")
            if (-not $ready) {
                if ($process.HasExited) {
                    throw "iteration $i exited before producing metrics (exit $($process.ExitCode))"
                }
                throw "iteration $i did not produce first-frame and focus metrics"
            }
            $firstFrame += (Read-Metric $metricsPath "plugin_first_frame" |
                    Select-Object -First 1).duration_ms
            $focus += (Read-Metric $metricsPath "plugin_focus" |
                    Select-Object -First 1).duration_ms

            $containment = Read-Metric $metricsPath "plugin_host_containment" |
                Select-Object -First 1
            if ($null -eq $containment -or $containment.backend -ne "windows_job_object") {
                throw "iteration $i did not report Windows Job Object containment"
            }
            $childPid = [int]$containment.value
            $hostCount = Read-Metric $metricsPath "plugin_host_process_count" |
                Select-Object -Last 1
            if ($null -eq $hostCount -or $hostCount.value -ne 1) {
                throw "iteration $i expected exactly one plugin host process"
            }

            $parentProcess = Get-Process -Id $process.Id -ErrorAction Stop
            $childProcess = Get-Process -Id $childPid -ErrorAction Stop
            $parentProcess.Refresh()
            $childProcess.Refresh()
            $pluginRss += [math]::Round($childProcess.PeakWorkingSet64 / 1MB, 2)
            $aggregate = $parentProcess.PeakWorkingSet64 + $childProcess.PeakWorkingSet64
            $aggregateRss += [math]::Round($aggregate / 1MB, 2)
            $pluginHandles += [int]$childProcess.HandleCount
        } finally {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            $process.WaitForExit(5000) | Out-Null
            if ($childPid -ne 0 -and -not (Wait-ProcessGone $childPid)) {
                Stop-Process -Id $childPid -Force -ErrorAction SilentlyContinue
                throw "iteration $i left plugin host process $childPid alive after the shell exited"
            }
        }
    }

    $summary = [ordered]@{
        plugin = [string]$manifest.id
        command = $command
        iterations = $Iterations
        hiddenPluginVms = 0
        hiddenPluginHostProcesses = 0
        firstFrameP95Ms = [math]::Round((Get-P95 $firstFrame), 2)
        focusP95Ms = [math]::Round((Get-P95 $focus), 2)
        pluginHostPeakRssP95MB = [math]::Round((Get-P95 $pluginRss), 2)
        aggregatePeakRssP95MB = [math]::Round((Get-P95 $aggregateRss), 2)
        pluginHostPeakHandles = ($pluginHandles | Measure-Object -Maximum).Maximum
        memoryLimitMiB = [math]::Round($containment.memory_limit_bytes / 1MB, 0)
        cpuRatePercent = $containment.cpu_rate_percent
        handleLimit = $containment.handle_limit
        firstFrameSamplesMs = @($firstFrame)
        focusSamplesMs = @($focus)
        pluginHostPeakRssSamplesMB = @($pluginRss)
        aggregatePeakRssSamplesMB = @($aggregateRss)
        pluginHostHandleSamples = @($pluginHandles)
    }
    $summaryPath = Join-Path $runDir "summary.json"
    $summary | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 -LiteralPath $summaryPath
    $summary | ConvertTo-Json -Depth 4
    Write-Host "Plugin benchmark evidence: $summaryPath"
} finally {
    $env:STEWARD_DATA_DIR = $oldData
    $env:STEWARD_PLUGINS_DIR = $oldPlugins
    $env:STEWARD_NSH_TRUST_ROOT = $oldTrustRoot
    $env:STEWARD_METRICS_PATH = $oldMetrics
    $env:STEWARD_SMOKE_PLUGIN = $oldPlugin
    $env:STEWARD_SMOKE_COMMAND = $oldCommand
    $env:STEWARD_SMOKE_TITLE = $oldTitle
    $env:STEWARD_SMOKE_REPEAT = $oldRepeat
    $env:STEWARD_SMOKE_REPEAT_DELAY_MS = $oldRepeatDelay
}
