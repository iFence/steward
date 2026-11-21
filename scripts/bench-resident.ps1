# Steward resident-mode benchmark (Windows).
#
# Measures the M2 resident lifecycle introduced with lazy GPUI loading:
#   boot -> tray ready, tray-only RSS, first summon latency (includes the
#   one-time GPUI/DirectX init), RSS with the launcher open, RSS after the
#   window is dismissed (hide or close, per CLOSE_ON_HIDE), and the second
#   summon latency.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts/bench-resident.ps1
#
# The synthetic WM_HOTKEY must carry the app's *registered* summon hotkey id.
# The app reads it from the settings table (`summon_hotkey` key in
# %APPDATA%\Steward\steward.db) at startup, falling back to the default
# "control+alt+Space" when nothing is persisted. The bench auto-detects it via
# the app's `--print-summon-hotkey` CLI, otherwise falls back to -SummonHotkey
# (or the default). A mismatch makes the app ignore the injected message and
# the summon measurement times out - pass -SummonHotkey to override.

param(
    [string]$Exe = "$PSScriptRoot\..\target\release\steward-app.exe",
    # The global summon hotkey as registered by the app (persisted in the
    # settings table, default "control+alt+Space"). Must match the app's
    # current binding or the synthetic WM_HOTKEY is ignored.
    [string]$SummonHotkey = "control+alt+Space",
    [int]$Samples = 20,
    [string]$DataDir = ""
)

$ErrorActionPreference = "Stop"
if ($Samples -lt 2) {
    throw "-Samples must be at least 2"
}
if (-not $DataDir) {
    $DataDir = Join-Path $PSScriptRoot (
        "..\target\benchmarks\resident-" + (Get-Date -Format "yyyyMMdd-HHmmss")
    )
}
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
$DataDir = (Resolve-Path -LiteralPath $DataDir).Path
$oldDataDir = $env:STEWARD_DATA_DIR
$oldPluginsDir = $env:STEWARD_PLUGINS_DIR
$oldMetricsPath = $env:STEWARD_METRICS_PATH
$env:STEWARD_DATA_DIR = $DataDir
$env:STEWARD_PLUGINS_DIR = Join-Path $DataDir "plugins"
New-Item -ItemType Directory -Force -Path $env:STEWARD_PLUGINS_DIR | Out-Null
$metricsPath = Join-Path $DataDir "metrics.jsonl"
Remove-Item -LiteralPath $metricsPath -ErrorAction SilentlyContinue
$env:STEWARD_METRICS_PATH = $metricsPath

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win32Bench {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr FindWindowW(string lpClassName, string lpWindowName);
    [DllImport("user32.dll")]
    public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")]
    public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
    [DllImport("user32.dll")]
    public static extern uint GetLastError();
}
"@

# The global-hotkey crate registers hotkeys with id = (mods.bits() << 16) | key as u32
# (global-hotkey-0.8.0 hotkey.rs), NOT a sequential counter. A synthetic WM_HOTKEY
# must carry that exact id or the event is ignored. Modifiers bits come from
# keyboard-types: SHIFT=0x200, CONTROL=0x8, ALT=0x1, SUPER=0x2000. Key codes are
# the Code enum discriminants (keyboard-types-0.7.0 code.rs): Space=62, Comma=4.
$CodeDiscriminants = @{ "Space" = 62; "Comma" = 4 }
$ModifierBits = @{ "shift" = 0x200; "control" = 0x8; "alt" = 0x1; "super" = 0x2000 }

function Get-HotkeyId([string]$HotkeyString) {
    $parts = $HotkeyString -split '\+' | ForEach-Object { $_.Trim() }
    $mods = 0
    $key = $null
    foreach ($part in $parts) {
        if ($ModifierBits.ContainsKey($part.ToLower())) {
            $mods = $mods -bor $ModifierBits[$part.ToLower()]
        } elseif (-not $key) {
            $key = $part
        }
    }
    if (-not $key -or -not $CodeDiscriminants.ContainsKey($key)) {
        throw "unsupported hotkey '$HotkeyString' (only Space/Comma keys supported)"
    }
    [uint32](($mods -shl 16) -bor $CodeDiscriminants[$key])
}

# Best-effort: ask the app for the effective summon hotkey via its headless
# `--print-summon-hotkey` CLI. The GUI-subsystem binary has no console stdout,
# so output is captured through Start-Process redirection. Returns $null when
# the exe is missing or the query fails, in which case the caller keeps its
# -SummonHotkey/default.
function Read-SummonHotkeyFromApp {
    param([string]$ExePath)
    if (-not (Test-Path $ExePath)) { return $null }
    $out = Join-Path $env:TEMP ("steward-hotkey-" + [guid]::NewGuid().ToString("N") + ".txt")
    $err = "$out.err"
    try {
        $p = Start-Process -FilePath $ExePath -ArgumentList "--print-summon-hotkey" `
            -RedirectStandardOutput $out -RedirectStandardError $err -Wait -PassThru
        if ($p.ExitCode -eq 0 -and (Test-Path $out)) {
            $val = (Get-Content $out -Raw).Trim()
            if ($val) { return $val }
        }
    } catch {
        # ignore; fall back to the caller's binding
    } finally {
        Remove-Item $out, $err -ErrorAction SilentlyContinue
    }
    return $null
}

# Auto-detect the registered hotkey when the caller did not override it.
$ResolvedHotkey = $SummonHotkey
if (-not $PSBoundParameters.ContainsKey('SummonHotkey')) {
    $detected = Read-SummonHotkeyFromApp $Exe
    if ($detected) { $ResolvedHotkey = $detected }
}
$SummonHotkeyId = Get-HotkeyId $ResolvedHotkey
Write-Host "using summon hotkey '$ResolvedHotkey' -> WM_HOTKEY id 0x$('{0:X}' -f $SummonHotkeyId)"

function Get-WindowPid([IntPtr]$hWnd) {
    $pidOut = 0
    [void][Win32Bench]::GetWindowThreadProcessId($hWnd, [ref]$pidOut)
    return $pidOut
}

function Find-HotkeyWindow([int]$AppPid) {
    $h = [Win32Bench]::FindWindowW("global_hotkey_app", [NullString]::Value)
    if ($h -ne [IntPtr]::Zero -and (Get-WindowPid $h) -eq $AppPid) { return $h }
    return [IntPtr]::Zero
}
function Find-TrayWindow([int]$AppPid) {
    $h = [Win32Bench]::FindWindowW("tray_icon_app", [NullString]::Value)
    if ($h -ne [IntPtr]::Zero -and (Get-WindowPid $h) -eq $AppPid) { return $h }
    return [IntPtr]::Zero
}
function Find-LauncherWindow([int]$AppPid) {
    $h = [Win32Bench]::FindWindowW([NullString]::Value, "Steward")
    if ($h -ne [IntPtr]::Zero -and (Get-WindowPid $h) -eq $AppPid) { return $h }
    return [IntPtr]::Zero
}

function Wait-Until([scriptblock]$Condition, [int]$TimeoutMs = 15000) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if (& $Condition) { return $sw.ElapsedMilliseconds }
        Start-Sleep -Milliseconds 5
    }
    return -1
}

function Get-MemoryMB($proc) {
    $proc.Refresh()
    [pscustomobject]@{
        WorkingSetMB = [math]::Round($proc.WorkingSet64 / 1MB, 1)
        PrivateMB = [math]::Round($proc.PrivateMemorySize64 / 1MB, 1)
    }
}

function Get-P95($Values) {
    $sorted = @($Values | Sort-Object)
    if ($sorted.Count -eq 0) {
        throw "cannot calculate p95 from an empty sample set"
    }
    $index = [math]::Ceiling($sorted.Count * 0.95) - 1
    return [double]$sorted[[math]::Max(0, $index)]
}

function Get-MetricValues([string]$Event) {
    if (-not (Test-Path -LiteralPath $metricsPath)) {
        return @()
    }
    return @(
        Get-Content -LiteralPath $metricsPath |
            ForEach-Object { $_ | ConvertFrom-Json } |
            Where-Object { $_.event -eq $Event }
    )
}

# Stop any existing dev instances so hotkey/tray registration is clean.
Get-Process steward-app -ErrorAction SilentlyContinue | Stop-Process -Force

$p = Start-Process -FilePath $Exe -PassThru
try {
    $proc = Get-Process -Id $p.Id
    $appPid = $p.Id

    $bootMs = Wait-Until { (Find-TrayWindow $appPid) -ne [IntPtr]::Zero } 15000
    if ($bootMs -lt 0) { throw "tray window not found within timeout" }
    Start-Sleep -Milliseconds 300  # let hotkey registration + cache read settle
    $resident = Get-MemoryMB $proc
    $hiddenVm = Get-MetricValues "hidden_plugin_vm_count" | Select-Object -Last 1
    $hiddenHosts = Get-MetricValues "plugin_host_process_count" | Select-Object -Last 1
    if ($null -eq $hiddenVm -or $hiddenVm.value -ne 0) {
        throw "tray-ready hidden plugin VM count must be zero"
    }
    if ($null -eq $hiddenHosts -or $hiddenHosts.value -ne 0) {
        throw "tray-ready hidden plugin host process count must be zero"
    }

[void][Win32Bench]::SendMessageW((Find-HotkeyWindow $appPid), 0x0312, [IntPtr]$SummonHotkeyId, [IntPtr]0)  # WM_HOTKEY
$firstMs = Wait-Until {
    $launcher = Find-LauncherWindow $appPid
    $launcher -ne [IntPtr]::Zero -and [Win32Bench]::IsWindowVisible($launcher)
} 15000
if ($firstMs -lt 0) {
    throw "launcher never became visible after WM_HOTKEY id 0x$('{0:X}' -f $SummonHotkeyId) for '$ResolvedHotkey'. The app may register a different summon hotkey (settings table 'summon_hotkey' in %APPDATA%\Steward\steward.db); pass it via -SummonHotkey."
}
    $open = Get-MemoryMB $proc

# Close the benchmark launcher, then repeat summon/close cycles. Closing lets
# the app clear its cached window handle before the next synthetic hotkey;
# merely calling ShowWindow leaves that cache stale.
    $summonSamples = @([double]$firstMs)
    $launcher = Find-LauncherWindow $appPid
    [void][Win32Bench]::SendMessageW($launcher, 0x0010, [IntPtr]0, [IntPtr]0)  # WM_CLOSE
    $closedMs = Wait-Until {
        $l2 = Find-LauncherWindow $appPid
        $l2 -eq [IntPtr]::Zero
    } 10000
    if ($closedMs -lt 0) {
        throw "launcher did not close after the first summon"
    }
    Start-Sleep -Milliseconds 300
    $afterClose = Get-MemoryMB $proc

    for ($sample = 2; $sample -le $Samples; $sample++) {
    [void][Win32Bench]::SendMessageW((Find-HotkeyWindow $appPid), 0x0312, [IntPtr]$SummonHotkeyId, [IntPtr]0)
    $summonMs = Wait-Until {
        $launcher = Find-LauncherWindow $appPid
        $launcher -ne [IntPtr]::Zero -and [Win32Bench]::IsWindowVisible($launcher)
    } 15000
    if ($summonMs -lt 0) {
        throw "summon sample $sample never became visible (hotkey '$ResolvedHotkey', id 0x$('{0:X}' -f $SummonHotkeyId))"
    }
    $summonSamples += [double]$summonMs

    if ($sample -lt $Samples) {
        [void][Win32Bench]::SendMessageW(
            (Find-LauncherWindow $appPid),
            0x0010,
            [IntPtr]0,
            [IntPtr]0
        )
        $dismissed = Wait-Until {
            $next = Find-LauncherWindow $appPid
            $next -eq [IntPtr]::Zero
        } 10000
        if ($dismissed -lt 0) {
            throw "launcher did not dismiss before summon sample $($sample + 1)"
        }
        Start-Sleep -Milliseconds 150
    }
    }
    $secondMs = $summonSamples[1]
    $summonP95Ms = [math]::Round((Get-P95 $summonSamples), 2)

"== Steward resident benchmark ($(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')) =="
"boot -> tray ready        : $bootMs ms"
"resident RSS (tray-only)  : $($resident.WorkingSetMB) MB working set / $($resident.PrivateMB) MB private"
"first summon -> visible   : $firstMs ms"
"RSS with launcher open    : $($open.WorkingSetMB) MB working set / $($open.PrivateMB) MB private"
"launcher close            : $closedMs ms"
"RSS after dismiss         : $($afterClose.WorkingSetMB) MB working set / $($afterClose.PrivateMB) MB private"
"second summon -> visible  : $secondMs ms"
    "summon p95 ($Samples samples): $summonP95Ms ms"

    $summary = [ordered]@{
        executable = $Exe
        executableSha256 = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLowerInvariant()
        coldStartToTrayReadyMs = $bootMs
        residentWorkingSetMB = $resident.WorkingSetMB
        residentPrivateMB = $resident.PrivateMB
        launcherWorkingSetMB = $open.WorkingSetMB
        launcherPrivateMB = $open.PrivateMB
        firstSummonMs = $firstMs
        summonP95Ms = $summonP95Ms
        summonSamplesMs = @($summonSamples)
        hiddenPluginVmsAtTrayReady = $hiddenVm.value
        hiddenPluginHostProcessesAtTrayReady = $hiddenHosts.value
    }
    $summaryPath = Join-Path $DataDir "summary.json"
    $summary | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 -LiteralPath $summaryPath
    Write-Host "Resident benchmark evidence: $summaryPath"
} finally {
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    $p.WaitForExit(5000) | Out-Null
    $env:STEWARD_DATA_DIR = $oldDataDir
    $env:STEWARD_PLUGINS_DIR = $oldPluginsDir
    $env:STEWARD_METRICS_PATH = $oldMetricsPath
}
