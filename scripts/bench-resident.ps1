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
    [string]$Exe = "$PSScriptRoot\..\target\release\steward-app.exe",
    # The global summon hotkey as registered by the app (persisted in the
    # settings table, default "control+alt+Space"). Must match the app's
    # current binding or the synthetic WM_HOTKEY is ignored.
    [string]$SummonHotkey = "control+alt+Space"
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
$SummonHotkeyId = Get-HotkeyId $SummonHotkey

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

# Stop any existing dev instances so hotkey/tray registration is clean.
Get-Process steward-app -ErrorAction SilentlyContinue | Stop-Process -Force

$p = Start-Process -FilePath $Exe -PassThru
$proc = Get-Process -Id $p.Id
$appPid = $p.Id

$bootMs = Wait-Until { (Find-TrayWindow $appPid) -ne [IntPtr]::Zero } 15000
if ($bootMs -lt 0) { throw "tray window not found within timeout" }
Start-Sleep -Milliseconds 300  # let hotkey registration + cache read settle
$resident = Get-MemoryMB $proc

[void][Win32Bench]::PostMessageW((Find-HotkeyWindow $appPid), 0x0312, [IntPtr]$SummonHotkeyId, [IntPtr]0)  # WM_HOTKEY
$firstMs = Wait-Until {
    $launcher = Find-LauncherWindow $appPid
    $launcher -ne [IntPtr]::Zero -and [Win32Bench]::IsWindowVisible($launcher)
} 15000
$open = Get-MemoryMB $proc

# Esc dismisses the launcher (hide or close per CLOSE_ON_HIDE).
$launcher = Find-LauncherWindow $appPid
[void][Win32Bench]::PostMessageW($launcher, 0x0100, [IntPtr]0x1B, [IntPtr]0)  # WM_KEYDOWN VK_ESCAPE
$closedMs = Wait-Until {
    $l2 = Find-LauncherWindow $appPid
    $l2 -eq [IntPtr]::Zero -or -not [Win32Bench]::IsWindowVisible($l2)
} 10000
Start-Sleep -Milliseconds 300
$afterClose = Get-MemoryMB $proc

[void][Win32Bench]::PostMessageW((Find-HotkeyWindow $appPid), 0x0312, [IntPtr]$SummonHotkeyId, [IntPtr]0)
$secondMs = Wait-Until {
    $launcher = Find-LauncherWindow $appPid
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
