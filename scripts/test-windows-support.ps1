#!/usr/bin/env pwsh
# Exercise the M4 Windows support paths without touching normal Steward data:
# tray/hotkey creation, single-instance summon, autostart repair/removal,
# mixed-DPI monitor placement, Unicode keyboard input, and package-failure
# diagnostics.
#
# Run on an interactive Windows 11 acceptance VM. Isolated instance/hotkey
# names allow a local smoke run beside an existing install; the normative
# acceptance run still omits -AllowNonInteractive. The script temporarily
# replaces the current user's Steward autostart value and restores it in finally.

param(
    [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\steward-app.exe"),
    [string]$OutDir = (Join-Path $PSScriptRoot "..\target\acceptance\windows-support"),
    [string]$SummonHotkey = "",
    [switch]$SkipAutostart,
    [switch]$AllowNonInteractive
)

$ErrorActionPreference = "Stop"
if ($env:OS -ne "Windows_NT") {
    throw "this acceptance script requires Windows"
}
$Exe = (Resolve-Path -LiteralPath $Exe).Path
if ($env:PROCESSOR_ARCHITECTURE -ne "AMD64") {
    throw "this acceptance script currently targets Windows x64"
}

$runDir = Join-Path $OutDir (Get-Date -Format "yyyyMMdd-HHmmss")
New-Item -ItemType Directory -Force -Path $runDir | Out-Null
$dataDir = Join-Path $runDir "data"
$pluginRoot = Join-Path $dataDir "plugins"
$metricsPath = Join-Path $runDir "metrics.jsonl"
New-Item -ItemType Directory -Force -Path $pluginRoot | Out-Null

$environmentNames = @(
    "STEWARD_DATA_DIR",
    "STEWARD_PLUGINS_DIR",
    "STEWARD_METRICS_PATH",
    "STEWARD_SMOKE_PLUGIN",
    "STEWARD_SMOKE_COMMAND",
    "STEWARD_SMOKE_TITLE",
    "STEWARD_NSH_TRUST_ROOT",
    "STEWARD_INSTANCE_SUFFIX",
    "STEWARD_SUMMON_HOTKEY"
)
$savedEnvironment = @{}
foreach ($name in $environmentNames) {
    $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
}
$env:STEWARD_DATA_DIR = $dataDir
$env:STEWARD_PLUGINS_DIR = $pluginRoot
$env:STEWARD_METRICS_PATH = $metricsPath
$instanceSuffix = "m4-$PID"
$env:STEWARD_INSTANCE_SUFFIX = $instanceSuffix
$env:STEWARD_SUMMON_HOTKEY = if ($SummonHotkey) {
    $SummonHotkey
} else {
    "control+alt+shift+Comma"
}
Remove-Item Env:STEWARD_SMOKE_PLUGIN -ErrorAction SilentlyContinue
Remove-Item Env:STEWARD_SMOKE_COMMAND -ErrorAction SilentlyContinue
Remove-Item Env:STEWARD_SMOKE_TITLE -ErrorAction SilentlyContinue

$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$runValueName = "Steward"
$savedAutostart = [pscustomobject]@{
    Exists = $false
    Value = $null
}
try {
    $savedAutostart.Value = Get-ItemPropertyValue -LiteralPath $runKey -Name $runValueName
    $savedAutostart.Exists = $true
} catch {
    $savedAutostart.Exists = $false
}

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class StewardWin32Support {
    [StructLayout(LayoutKind.Sequential)]
    public struct POINT {
        public int X;
        public int Y;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct MONITORINFO {
        public uint cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public uint dwFlags;
    }

    public sealed class MonitorSnapshot {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
        public int WorkLeft;
        public int WorkTop;
        public int WorkRight;
        public int WorkBottom;
        public uint Dpi;
        public bool Primary;
    }

    private delegate bool MonitorEnumProc(
        IntPtr monitor,
        IntPtr hdc,
        IntPtr clip,
        IntPtr data
    );

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr FindWindowW(string className, string windowName);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hwnd);

    [DllImport("user32.dll")]
    public static extern IntPtr SendMessageW(
        IntPtr hwnd,
        uint message,
        IntPtr wParam,
        IntPtr lParam
    );

    [DllImport("user32.dll")]
    public static extern bool PostMessageW(
        IntPtr hwnd,
        uint message,
        IntPtr wParam,
        IntPtr lParam
    );

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);

    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    public static extern bool SetCursorPos(int x, int y);

    [DllImport("user32.dll")]
    public static extern bool GetCursorPos(out POINT point);

    [DllImport("user32.dll")]
    public static extern IntPtr MonitorFromPoint(POINT point, uint flags);

    [DllImport("user32.dll")]
    public static extern IntPtr MonitorFromWindow(IntPtr hwnd, uint flags);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern bool GetMonitorInfoW(IntPtr monitor, ref MONITORINFO info);

    [DllImport("user32.dll")]
    private static extern bool EnumDisplayMonitors(
        IntPtr hdc,
        IntPtr clip,
        MonitorEnumProc callback,
        IntPtr data
    );

    [DllImport("Shcore.dll")]
    private static extern int GetDpiForMonitor(
        IntPtr monitor,
        int dpiType,
        out uint dpiX,
        out uint dpiY
    );

    [DllImport("imm32.dll")]
    public static extern IntPtr ImmGetContext(IntPtr hwnd);

    [DllImport("imm32.dll")]
    public static extern bool ImmReleaseContext(IntPtr hwnd, IntPtr context);

    private static MonitorSnapshot Snapshot(IntPtr monitor) {
        MONITORINFO info = new MONITORINFO();
        info.cbSize = (uint)Marshal.SizeOf(typeof(MONITORINFO));
        if (!GetMonitorInfoW(monitor, ref info)) {
            return null;
        }
        uint dpiX;
        uint dpiY;
        if (GetDpiForMonitor(monitor, 0, out dpiX, out dpiY) != 0) {
            dpiX = 96;
        }
        return new MonitorSnapshot {
            Left = info.rcMonitor.Left,
            Top = info.rcMonitor.Top,
            Right = info.rcMonitor.Right,
            Bottom = info.rcMonitor.Bottom,
            WorkLeft = info.rcWork.Left,
            WorkTop = info.rcWork.Top,
            WorkRight = info.rcWork.Right,
            WorkBottom = info.rcWork.Bottom,
            Dpi = Math.Max(96u, dpiX),
            Primary = (info.dwFlags & 1u) != 0
        };
    }

    public static MonitorSnapshot GetMonitor(IntPtr monitor) {
        return Snapshot(monitor);
    }

    public static MonitorSnapshot[] GetMonitors() {
        List<MonitorSnapshot> monitors = new List<MonitorSnapshot>();
        EnumDisplayMonitors(
            IntPtr.Zero,
            IntPtr.Zero,
            delegate(IntPtr monitor, IntPtr hdc, IntPtr clip, IntPtr data) {
                MonitorSnapshot snapshot = Snapshot(monitor);
                if (snapshot != null) {
                    monitors.Add(snapshot);
                }
                return true;
            },
            IntPtr.Zero
        );
        return monitors.ToArray();
    }
}
"@

$results = [ordered]@{}
$failure = $null
$primary = $null
$second = $null
$warmup = $null
$packageProcess = $null
$hotkey = ""
$monitors = @()
$monitorChecks = @()
$savedCursor = New-Object StewardWin32Support+POINT
[void][StewardWin32Support]::GetCursorPos([ref]$savedCursor)

function Wait-Until([scriptblock]$Condition, [int]$TimeoutMs = 15000) {
    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    while ($stopwatch.ElapsedMilliseconds -lt $TimeoutMs) {
        if (& $Condition) {
            return $stopwatch.ElapsedMilliseconds
        }
        Start-Sleep -Milliseconds 25
    }
    return -1
}

function Get-Metric([string]$Event) {
    if (-not (Test-Path -LiteralPath $metricsPath)) {
        return @()
    }
    return @(
        Get-Content -LiteralPath $metricsPath |
            ForEach-Object { $_ | ConvertFrom-Json } |
            Where-Object { $_.event -eq $Event }
    )
}

function Wait-Metric([string]$Event, [int]$TimeoutMs = 10000) {
    $value = Wait-Until { @(Get-Metric $Event).Count -gt 0 } $TimeoutMs
    if ($value -lt 0) {
        throw "timed out waiting for metric '$Event' in $metricsPath"
    }
    return @(Get-Metric $Event | Select-Object -Last 1)[0]
}

function Find-Window([string]$ClassName, [string]$Title, [int]$ProcessId) {
    $classArg = if ($ClassName) { $ClassName } else { [NullString]::Value }
    $titleArg = if ($Title) { $Title } else { [NullString]::Value }
    $hwnd = [StewardWin32Support]::FindWindowW($classArg, $titleArg)
    if ($hwnd -eq [IntPtr]::Zero) {
        return [IntPtr]::Zero
    }
    $ownerPid = 0
    [void][StewardWin32Support]::GetWindowThreadProcessId($hwnd, [ref]$ownerPid)
    if ($ownerPid -eq $ProcessId) {
        return $hwnd
    }
    return [IntPtr]::Zero
}

function Read-SummonHotkey([string]$Hotkey) {
    if ($Hotkey) {
        return $Hotkey
    }
    $stdout = Join-Path $runDir "summon-hotkey.txt"
    $stderr = "$stdout.err"
    $process = Start-Process -FilePath $Exe -ArgumentList "--print-summon-hotkey" `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr -Wait -PassThru
    if ($process.ExitCode -eq 0 -and (Test-Path -LiteralPath $stdout)) {
        $value = (Get-Content -Raw -LiteralPath $stdout).Trim()
        if ($value) {
            return $value
        }
    }
    throw "the executable did not report its summon hotkey"
}

function Get-HotkeyId([string]$Hotkey) {
    $codeDiscriminants = @{ "Space" = 62; "Comma" = 4 }
    $modifierBits = @{ "shift" = 0x200; "control" = 0x8; "alt" = 0x1; "super" = 0x2000 }
    $modifiers = 0
    $key = $null
    foreach ($part in ($Hotkey -split "\+" | ForEach-Object { $_.Trim() })) {
        if ($modifierBits.ContainsKey($part.ToLowerInvariant())) {
            $modifiers = $modifiers -bor $modifierBits[$part.ToLowerInvariant()]
        } elseif (-not $key) {
            $key = $part
        }
    }
    if (-not $key -or -not $codeDiscriminants.ContainsKey($key)) {
        throw "unsupported acceptance hotkey '$Hotkey'; use Space or Comma"
    }
    return [uint32](($modifiers -shl 16) -bor $codeDiscriminants[$key])
}

function Invoke-Utility([string]$Arguments) {
    $process = Start-Process -FilePath $Exe -ArgumentList $Arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "utility '$Arguments' failed with exit code $($process.ExitCode)"
    }
}

function Write-DiagnosticReport([string]$Name) {
    $path = Join-Path $runDir "$Name.json"
    Invoke-Utility "--write-diagnostics `"$path`""
    if (-not (Test-Path -LiteralPath $path)) {
        throw "diagnostic report was not written: $path"
    }
    return (Get-Content -Raw -Encoding UTF8 -LiteralPath $path | ConvertFrom-Json)
}

function Stop-OwnedProcess($Process) {
    if ($null -eq $Process) {
        return
    }
    if (-not $Process.HasExited) {
        Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
        $Process.WaitForExit(5000) | Out-Null
    }
}

try {
    $hotkey = Read-SummonHotkey $SummonHotkey
    $hotkeyId = Get-HotkeyId $hotkey

    if (-not $SkipAutostart) {
        $expected = '"' + $Exe + '"'
        Set-ItemProperty -LiteralPath $runKey -Name $runValueName `
            -Value '"C:\missing\steward-app.exe"' -Type String
        Invoke-Utility "--sync-autostart"
        $actual = Get-ItemPropertyValue -LiteralPath $runKey -Name $runValueName
        if (-not $actual.Equals($expected, [StringComparison]::OrdinalIgnoreCase)) {
            throw "autostart repair wrote '$actual', expected '$expected'"
        }
        $autostartReport = Write-DiagnosticReport "autostart-repair"
        if (-not $autostartReport.autostart.enabled -or
            -not $autostartReport.autostart.pointsToCurrentExecutable) {
            throw "diagnostic report did not confirm the repaired autostart entry"
        }
        Invoke-Utility "--unregister-autostart"
        try {
            $null = Get-ItemPropertyValue -LiteralPath $runKey -Name $runValueName
            throw "unregister-autostart left the Run value behind"
        } catch [System.Management.Automation.ItemNotFoundException] {
            # Expected: the value no longer exists.
        }
        $results["autostartRepairAndRemoval"] = "pass"
    } else {
        $results["autostartRepairAndRemoval"] = "skip"
    }

    $primary = Start-Process -FilePath $Exe -PassThru
    $trayReady = Wait-Until {
        (Find-Window "tray_icon_app" $null $primary.Id) -ne [IntPtr]::Zero
    } 15000
    if ($trayReady -lt 0) {
        throw "tray window was not created"
    }
    $hotkeyWindow = Find-Window "global_hotkey_app" $null $primary.Id
    if ($hotkeyWindow -eq [IntPtr]::Zero) {
        throw "global hotkey window was not created"
    }
    [void](Wait-Metric "tray_lifecycle")
    [void](Wait-Metric "hotkey_lifecycle")
    $results["trayAndHotkeyCreation"] = "pass"

    $second = Start-Process -FilePath $Exe -PassThru
    if (-not $second.WaitForExit(5000)) {
        throw "secondary launch did not exit within five seconds"
    }
    if ($second.ExitCode -ne 0) {
        throw "secondary launch exited with $($second.ExitCode)"
    }
    if ($primary.HasExited) {
        throw "secondary launch terminated the primary instance"
    }
    $launcherReady = Wait-Until {
        $launcher = Find-Window $null "Steward" $primary.Id
        $launcher -ne [IntPtr]::Zero -and [StewardWin32Support]::IsWindowVisible($launcher)
    } 10000
    if ($launcherReady -lt 0) {
        throw "secondary launch did not summon the existing launcher"
    }
    $results["singleInstance"] = "pass"

    $report = Write-DiagnosticReport "runtime"
    if ($report.application.version -ne (Get-Item -LiteralPath $Exe).VersionInfo.ProductVersion) {
        throw "diagnostic application version does not match the executable"
    }
    $monitors = @([StewardWin32Support]::GetMonitors())
    if ($monitors.Count -eq 0) {
        throw "Win32 did not report any display monitors"
    }
    if ($report.displays.monitorCount -ne $monitors.Count) {
        throw "diagnostic monitor count $($report.displays.monitorCount) != Win32 count $($monitors.Count)"
    }
    $results["supportReport"] = "pass"

    $launcher = Find-Window $null "Steward" $primary.Id
    $monitorChecks = @()
    $cursorControlAvailable = $true
    foreach ($monitor in $monitors) {
        if ([StewardWin32Support]::IsWindowVisible($launcher)) {
            [void][StewardWin32Support]::SendMessageW(
                $hotkeyWindow,
                0x0312,
                [IntPtr]$hotkeyId,
                [IntPtr]::Zero
            )
            $hidden = Wait-Until {
                -not [StewardWin32Support]::IsWindowVisible(
                    (Find-Window $null "Steward" $primary.Id)
                )
            } 5000
            if ($hidden -lt 0) {
                throw "launcher did not hide before monitor placement check"
            }
        }

        $x = [int](($monitor.WorkLeft + $monitor.WorkRight) / 2)
        $y = [int](($monitor.WorkTop + $monitor.WorkBottom) / 2)
        if (-not [StewardWin32Support]::SetCursorPos($x, $y)) {
            if ($AllowNonInteractive) {
                $cursorControlAvailable = $false
                break
            }
            throw "could not move the cursor to monitor work area ($x,$y)"
        }
        [void][StewardWin32Support]::SendMessageW(
            $hotkeyWindow,
            0x0312,
            [IntPtr]$hotkeyId,
            [IntPtr]::Zero
        )
        $shown = Wait-Until {
            $candidate = Find-Window $null "Steward" $primary.Id
            $candidate -ne [IntPtr]::Zero -and
                [StewardWin32Support]::IsWindowVisible($candidate)
        } 10000
        if ($shown -lt 0) {
            throw "launcher did not summon on monitor centered at ($x,$y)"
        }

        $launcher = Find-Window $null "Steward" $primary.Id
        $windowMonitor = [StewardWin32Support]::GetMonitor(
            [StewardWin32Support]::MonitorFromWindow($launcher, 2)
        )
        $rect = New-Object StewardWin32Support+RECT
        if (-not [StewardWin32Support]::GetWindowRect($launcher, [ref]$rect)) {
            throw "could not read the summoned launcher rectangle"
        }
        $centerX = [int](($rect.Left + $rect.Right) / 2)
        $centerY = [int](($rect.Top + $rect.Bottom) / 2)
        if ($centerX -lt $monitor.WorkLeft -or $centerX -ge $monitor.WorkRight -or
            $centerY -lt $monitor.WorkTop -or $centerY -ge $monitor.WorkBottom) {
            throw "launcher center ($centerX,$centerY) is outside the target work area"
        }
        if ($windowMonitor.Dpi -ne $monitor.Dpi) {
            throw "launcher moved to DPI $($windowMonitor.Dpi), expected $($monitor.Dpi)"
        }
        $monitorMetric = @(Get-Metric "launcher_monitor" | Select-Object -Last 1)[0]
        if ($null -eq $monitorMetric -or $monitorMetric.value -ne $monitor.Dpi) {
            throw "launcher metrics did not report target DPI $($monitor.Dpi)"
        }
        $monitorChecks += [ordered]@{
            dpi = $monitor.Dpi
            workArea = @($monitor.WorkLeft, $monitor.WorkTop, $monitor.WorkRight, $monitor.WorkBottom)
            launcherRect = @($rect.Left, $rect.Top, $rect.Right, $rect.Bottom)
        }
    }
    $distinctDpi = @($monitors | ForEach-Object { $_.Dpi } | Sort-Object -Unique)
    $results["monitorSummon"] = if (-not $cursorControlAvailable) {
        "skip-cursor-control"
    } elseif ($distinctDpi.Count -gt 1) {
        "pass"
    } else {
        "pass-single-dpi"
    }

    $focusReady = Wait-Until {
        [StewardWin32Support]::GetForegroundWindow() -eq $launcher
    } 3000
    $hasKeyboardFocus = $focusReady -ge 0
    if (-not $hasKeyboardFocus -and -not $AllowNonInteractive) {
        throw "launcher does not own keyboard foreground after summon"
    }
    if ($hasKeyboardFocus) {
        [void][StewardWin32Support]::SendMessageW(
            $launcher,
            0x0102,
            [IntPtr]0x4E2D,
            [IntPtr]1
        )
        $inputMetric = Wait-Metric "launcher_input_update" 5000
        if ($inputMetric.value -lt 1) {
            throw "Unicode WM_CHAR did not reach the launcher input"
        }
        [void][StewardWin32Support]::SendMessageW(
            $launcher,
            0x0100,
            [IntPtr]0x28,
            [IntPtr]1
        )
        [void][StewardWin32Support]::SendMessageW(
            $launcher,
            0x0101,
            [IntPtr]0x28,
            [IntPtr]1
        )
        $navigationMetric = Wait-Metric "launcher_key" 5000
        if ($navigationMetric.key -ne "down" -and $navigationMetric.key -ne "ArrowDown") {
            throw "keyboard navigation did not reach the launcher key handler"
        }
        $imeContext = [StewardWin32Support]::ImmGetContext($launcher)
        if ($imeContext -ne [IntPtr]::Zero) {
            [void][StewardWin32Support]::ImmReleaseContext($launcher, $imeContext)
            $results["imeContextAndKeyboard"] = "pass"
        } elseif (-not $AllowNonInteractive) {
            throw "launcher has no attached IME context"
        } else {
            $results["imeContextAndKeyboard"] = "skip-no-ime-context"
        }
    } else {
        $results["imeContextAndKeyboard"] = "skip-noninteractive-desktop"
    }

    Stop-OwnedProcess $primary
    $trayGone = Wait-Until {
        (Find-Window "tray_icon_app" $null $primary.Id) -eq [IntPtr]::Zero
    } 5000
    if ($trayGone -lt 0) {
        throw "tray window survived process exit"
    }
    $results["trayDestruction"] = "pass"

    $invalidId = "com.steward.acceptance.invalid"
    $invalidDir = Join-Path $pluginRoot $invalidId
    New-Item -ItemType Directory -Force -Path $invalidDir | Out-Null
    @{
        schemaVersion = 2
        id = $invalidId
        name = "Invalid Package Acceptance"
        version = "1.0.0"
        package = "plugin.nshpkg"
        commands = @(@{
            name = "invalid-package"
            title = "Invalid Package Acceptance"
            trigger = @{ type = "command" }
        })
    } | ConvertTo-Json -Depth 6 | Set-Content -Encoding ASCII -LiteralPath (
        Join-Path $invalidDir "plugin.json"
    )
    Set-Content -Encoding ASCII -LiteralPath (Join-Path $invalidDir "plugin.nshpkg") `
        -Value "not a signed NSH package"

    $warmupMetrics = Join-Path $runDir "warmup.jsonl"
    $env:STEWARD_METRICS_PATH = $warmupMetrics
    $warmup = Start-Process -FilePath $Exe -PassThru
    if ((Wait-Until {
        (Find-Window "tray_icon_app" $null $warmup.Id) -ne [IntPtr]::Zero
    } 15000) -lt 0) {
        throw "plugin warm-up instance did not reach the tray"
    }
    Start-Sleep -Seconds 2
    Stop-OwnedProcess $warmup
    $warmupReport = Write-DiagnosticReport "plugin-warmup"
    if ($warmupReport.pluginCache.routeCount -ne 1) {
        throw "plugin route cache did not contain the staged invalid package"
    }

    $packageMetrics = Join-Path $runDir "package-failure.jsonl"
    $env:STEWARD_METRICS_PATH = $packageMetrics
    $env:STEWARD_SMOKE_PLUGIN = $invalidId
    $env:STEWARD_SMOKE_COMMAND = "invalid-package"
    $env:STEWARD_SMOKE_TITLE = "Invalid Package Acceptance"
    $packageProcess = Start-Process -FilePath $Exe -PassThru
    $packageReady = Wait-Until {
        if (-not (Test-Path -LiteralPath $packageMetrics)) {
            return $false
        }
        @(
            Get-Content -LiteralPath $packageMetrics |
                ForEach-Object { $_ | ConvertFrom-Json } |
                Where-Object { $_.event -eq "plugin_launch_failure" }
        ).Count -gt 0
    } 15000
    if ($packageReady -lt 0) {
        throw "invalid package did not produce a structured launch failure"
    }
    if ($packageProcess.HasExited) {
        throw "package verification failure terminated the shell"
    }
    $packageFailure = @(
        Get-Content -LiteralPath $packageMetrics |
            ForEach-Object { $_ | ConvertFrom-Json } |
            Where-Object { $_.event -eq "plugin_launch_failure" }
    ) | Select-Object -Last 1
    if ($packageFailure.error_code -ne "trust_failure") {
        throw "invalid package failed with '$($packageFailure.error_code)', expected trust_failure"
    }
    Start-Sleep -Milliseconds 500
    $packageReport = Write-DiagnosticReport "package-failure"
    if (@($packageReport.plugins | Where-Object { $_.id -eq $invalidId }).Count -ne 1) {
        throw "package diagnostic report omitted the invalid plugin"
    }
    if (@($packageReport.recentErrors | Where-Object { $_ -match "plugin launch failed" }).Count -eq 0) {
        throw "package diagnostic report omitted the verification failure"
    }
    $results["packageVerificationDiagnostics"] = "pass"
} catch {
    $failure = $_.Exception.Message
} finally {
    Stop-OwnedProcess $second
    Stop-OwnedProcess $packageProcess
    Stop-OwnedProcess $warmup
    Stop-OwnedProcess $primary
    if ([StewardWin32Support]::GetCursorPos([ref]$savedCursor)) {
        [void][StewardWin32Support]::SetCursorPos($savedCursor.X, $savedCursor.Y)
    }
    if (-not $SkipAutostart) {
        if ($savedAutostart.Exists) {
            Set-ItemProperty -LiteralPath $runKey -Name $runValueName `
                -Value $savedAutostart.Value -Type String
        } else {
            Remove-ItemProperty -LiteralPath $runKey -Name $runValueName `
                -ErrorAction SilentlyContinue
        }
    }
    foreach ($name in $environmentNames) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], "Process")
    }

    $summary = [ordered]@{
        executable = $Exe
        executableSha256 = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLowerInvariant()
        summonHotkey = $hotkey
        instanceSuffix = $instanceSuffix
        monitors = @(
            $monitors | ForEach-Object {
                [ordered]@{
                    dpi = $_.Dpi
                    primary = $_.Primary
                    workArea = @($_.WorkLeft, $_.WorkTop, $_.WorkRight, $_.WorkBottom)
                }
            }
        )
        monitorChecks = @($monitorChecks)
        results = $results
        failure = $failure
        diagnosticReports = @(
            Get-ChildItem -LiteralPath $runDir -Filter "*.json" -File |
                Where-Object { $_.Name -ne "summary.json" } |
                Select-Object -ExpandProperty FullName
        )
    }
    $summaryPath = Join-Path $runDir "summary.json"
    $summary | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 -LiteralPath $summaryPath
    Write-Host "Windows support acceptance evidence: $summaryPath"
}

if ($failure) {
    throw $failure
}
