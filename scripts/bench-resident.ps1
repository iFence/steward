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

param(
    [string]$Exe = "$PSScriptRoot\..\target\release\steward-app.exe"
)

$ErrorActionPreference = "Stop"

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win32Bench {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr FindWindowW(string lpClassName, string lpWindowName);
    [DllImport("user32.dll")]
    public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);
}
"@

function Find-HotkeyWindow { [Win32Bench]::FindWindowW("global_hotkey_app", [NullString]::Value) }
function Find-TrayWindow { [Win32Bench]::FindWindowW("tray_icon_app", [NullString]::Value) }
function Find-LauncherWindow { [Win32Bench]::FindWindowW([NullString]::Value, "Steward") }

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

# Stop any existing dev instances so hotkey/tray registration is clean.
Get-Process steward-app -ErrorAction SilentlyContinue | Stop-Process -Force

$p = Start-Process -FilePath $Exe -PassThru
$proc = Get-Process -Id $p.Id

$bootMs = Wait-Until { (Find-TrayWindow) -ne [IntPtr]::Zero } 15000
if ($bootMs -lt 0) { throw "tray window not found within timeout" }
Start-Sleep -Milliseconds 300  # let hotkey registration + cache read settle
$resident = Get-MemoryMB $proc

[void][Win32Bench]::PostMessageW((Find-HotkeyWindow), 0x0312, [IntPtr]1, [IntPtr]0)  # WM_HOTKEY
$firstMs = Wait-Until {
    $launcher = Find-LauncherWindow
    $launcher -ne [IntPtr]::Zero -and [Win32Bench]::IsWindowVisible($launcher)
} 15000
$open = Get-MemoryMB $proc

# Esc dismisses the launcher (hide or close per CLOSE_ON_HIDE).
$launcher = Find-LauncherWindow
[void][Win32Bench]::PostMessageW($launcher, 0x0100, [IntPtr]0x1B, [IntPtr]0)  # WM_KEYDOWN VK_ESCAPE
$closedMs = Wait-Until {
    $l2 = Find-LauncherWindow
    $l2 -eq [IntPtr]::Zero -or -not [Win32Bench]::IsWindowVisible($l2)
} 10000
Start-Sleep -Milliseconds 300
$afterClose = Get-MemoryMB $proc

[void][Win32Bench]::PostMessageW((Find-HotkeyWindow), 0x0312, [IntPtr]1, [IntPtr]0)
$secondMs = Wait-Until {
    $launcher = Find-LauncherWindow
    $launcher -ne [IntPtr]::Zero -and [Win32Bench]::IsWindowVisible($launcher)
} 15000

Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue

"== Steward resident benchmark ($(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')) =="
"boot -> tray ready        : $bootMs ms"
"resident RSS (tray-only)  : $($resident.WorkingSetMB) MB working set / $($resident.PrivateMB) MB private"
"first summon -> visible   : $firstMs ms"
"RSS with launcher open    : $($open.WorkingSetMB) MB working set / $($open.PrivateMB) MB private"
"esc -> window dismissed   : $closedMs ms"
"RSS after dismiss         : $($afterClose.WorkingSetMB) MB working set / $($afterClose.PrivateMB) MB private"
"second summon -> visible  : $secondMs ms"
