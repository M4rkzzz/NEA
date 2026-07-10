param(
    [int]$DurationSeconds = 180,
    [string]$OutputPath = (Join-Path $PSScriptRoot "window-input-monitor.jsonl")
)

$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class WindowInputDiagnostics
{
    public const uint GW_HWNDFIRST = 0;
    public const uint GW_HWNDNEXT = 2;
    public const uint GW_OWNER = 4;
    public const uint GA_ROOT = 2;
    public const uint GA_ROOTOWNER = 3;

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int X; public int Y; }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

    [StructLayout(LayoutKind.Sequential)]
    public struct GUITHREADINFO
    {
        public int cbSize;
        public int flags;
        public IntPtr hwndActive;
        public IntPtr hwndFocus;
        public IntPtr hwndCapture;
        public IntPtr hwndMenuOwner;
        public IntPtr hwndMoveSize;
        public IntPtr hwndCaret;
        public RECT rcCaret;
    }

    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern IntPtr GetFocus();
    [DllImport("user32.dll")] public static extern IntPtr GetCapture();
    [DllImport("user32.dll")] public static extern short GetAsyncKeyState(int key);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT point);
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT point);
    [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
    [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr hwnd, uint command);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr hwnd, int index);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint threadId, ref GUITHREADINFO info);

    public static string WindowText(IntPtr hwnd)
    {
        var value = new StringBuilder(512);
        GetWindowText(hwnd, value, value.Capacity);
        return value.ToString();
    }

    public static string ClassName(IntPtr hwnd)
    {
        var value = new StringBuilder(256);
        GetClassName(hwnd, value, value.Capacity);
        return value.ToString();
    }

    public static IntPtr[] TopLevelWindows()
    {
        var result = new List<IntPtr>();
        var hwnd = GetWindow(IntPtr.Zero, GW_HWNDFIRST);
        while (hwnd != IntPtr.Zero && result.Count < 1000)
        {
            result.Add(hwnd);
            hwnd = GetWindow(hwnd, GW_HWNDNEXT);
        }
        return result.ToArray();
    }
}
"@

function Get-ProcessName([uint32]$ProcessId) {
    try { return (Get-Process -Id $ProcessId -ErrorAction Stop).ProcessName }
    catch { return "<exited>" }
}

function Get-WindowInfo([IntPtr]$Handle) {
    if ($Handle -eq [IntPtr]::Zero) { return $null }
    [uint32]$processId = 0
    $threadId = [WindowInputDiagnostics]::GetWindowThreadProcessId($Handle, [ref]$processId)
    $rect = New-Object WindowInputDiagnostics+RECT
    [void][WindowInputDiagnostics]::GetWindowRect($Handle, [ref]$rect)
    [ordered]@{
        hwnd = ('0x{0:X}' -f $Handle.ToInt64())
        pid = $processId
        tid = $threadId
        process = Get-ProcessName $processId
        class = [WindowInputDiagnostics]::ClassName($Handle)
        title = [WindowInputDiagnostics]::WindowText($Handle)
        visible = [WindowInputDiagnostics]::IsWindowVisible($Handle)
        enabled = [WindowInputDiagnostics]::IsWindowEnabled($Handle)
        minimized = [WindowInputDiagnostics]::IsIconic($Handle)
        style = ('0x{0:X}' -f [WindowInputDiagnostics]::GetWindowLongPtr($Handle, -16).ToInt64())
        exStyle = ('0x{0:X}' -f [WindowInputDiagnostics]::GetWindowLongPtr($Handle, -20).ToInt64())
        parent = ('0x{0:X}' -f [WindowInputDiagnostics]::GetParent($Handle).ToInt64())
        owner = ('0x{0:X}' -f [WindowInputDiagnostics]::GetWindow($Handle, 4).ToInt64())
        root = ('0x{0:X}' -f [WindowInputDiagnostics]::GetAncestor($Handle, 2).ToInt64())
        rootOwner = ('0x{0:X}' -f [WindowInputDiagnostics]::GetAncestor($Handle, 3).ToInt64())
        rect = @($rect.Left, $rect.Top, $rect.Right, $rect.Bottom)
    }
}

function Get-GuiThreadState([uint32]$ThreadId) {
    $info = New-Object WindowInputDiagnostics+GUITHREADINFO
    $info.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($info)
    if (-not [WindowInputDiagnostics]::GetGUIThreadInfo($ThreadId, [ref]$info)) { return $null }
    [ordered]@{
        tid = $ThreadId
        flags = ('0x{0:X}' -f $info.flags)
        active = Get-WindowInfo $info.hwndActive
        focus = Get-WindowInfo $info.hwndFocus
        capture = Get-WindowInfo $info.hwndCapture
        menuOwner = Get-WindowInfo $info.hwndMenuOwner
        moveSize = Get-WindowInfo $info.hwndMoveSize
        caret = Get-WindowInfo $info.hwndCaret
    }
}

function Get-RelevantWindows {
    $items = @()
    $z = 0
    foreach ($handle in [WindowInputDiagnostics]::TopLevelWindows()) {
        [uint32]$processId = 0
        [void][WindowInputDiagnostics]::GetWindowThreadProcessId($handle, [ref]$processId)
        $name = Get-ProcessName $processId
        $title = [WindowInputDiagnostics]::WindowText($handle)
        if ($name -match 'oopz|msedgewebview2' -or $title -match 'OOPZ') {
            $window = Get-WindowInfo $handle
            $window.z = $z
            $items += $window
        }
        $z++
    }
    return $items
}

function Write-Snapshot([string]$Reason) {
    $foregroundHandle = [WindowInputDiagnostics]::GetForegroundWindow()
    [uint32]$foregroundPid = 0
    $foregroundTid = [WindowInputDiagnostics]::GetWindowThreadProcessId($foregroundHandle, [ref]$foregroundPid)
    $point = New-Object WindowInputDiagnostics+POINT
    [void][WindowInputDiagnostics]::GetCursorPos([ref]$point)
    $hit = [WindowInputDiagnostics]::WindowFromPoint($point)
    $record = [ordered]@{
        timestamp = (Get-Date).ToString('o')
        elapsedMs = [int]$stopwatch.ElapsedMilliseconds
        reason = $Reason
        cursor = @($point.X, $point.Y)
        leftDown = (([WindowInputDiagnostics]::GetAsyncKeyState(1) -band 0x8000) -ne 0)
        foreground = Get-WindowInfo $foregroundHandle
        hit = Get-WindowInfo $hit
        hitRoot = Get-WindowInfo ([WindowInputDiagnostics]::GetAncestor($hit, 2))
        foregroundGui = Get-GuiThreadState $foregroundTid
        windows = @(Get-RelevantWindows)
    }
    Add-Content -LiteralPath $OutputPath -Value ($record | ConvertTo-Json -Depth 8 -Compress) -Encoding utf8
}

$parent = Split-Path -Parent $OutputPath
if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
Remove-Item -LiteralPath $OutputPath -Force -ErrorAction SilentlyContinue
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$lastForeground = [IntPtr]::Zero
$lastHit = [IntPtr]::Zero
$lastLeftDown = $false
$lastOopzMinimized = $null
$lastHeartbeat = -1000

Write-Snapshot "started"
while ($stopwatch.Elapsed.TotalSeconds -lt $DurationSeconds) {
    $foreground = [WindowInputDiagnostics]::GetForegroundWindow()
    $point = New-Object WindowInputDiagnostics+POINT
    [void][WindowInputDiagnostics]::GetCursorPos([ref]$point)
    $hit = [WindowInputDiagnostics]::WindowFromPoint($point)
    $leftDown = (([WindowInputDiagnostics]::GetAsyncKeyState(1) -band 0x8000) -ne 0)
    $oopzWindow = @(Get-RelevantWindows | Where-Object { $_.process -ieq 'oopz' } | Select-Object -First 1)
    $oopzMinimized = if ($oopzWindow.Count) { [bool]$oopzWindow[0].minimized } else { $null }

    $reasons = @()
    if ($foreground -ne $lastForeground) { $reasons += "foreground-changed" }
    if ($hit -ne $lastHit) { $reasons += "hit-changed" }
    if ($leftDown -ne $lastLeftDown) { $reasons += $(if ($leftDown) { "left-down" } else { "left-up" }) }
    if ($oopzMinimized -ne $lastOopzMinimized) { $reasons += "oopz-minimized=$oopzMinimized" }
    if (($stopwatch.ElapsedMilliseconds - $lastHeartbeat) -ge 1000) {
        $reasons += "heartbeat"
        $lastHeartbeat = $stopwatch.ElapsedMilliseconds
    }
    if ($reasons.Count) { Write-Snapshot ($reasons -join ',') }

    $lastForeground = $foreground
    $lastHit = $hit
    $lastLeftDown = $leftDown
    $lastOopzMinimized = $oopzMinimized
    Start-Sleep -Milliseconds 50
}
Write-Snapshot "finished"
Write-Output $OutputPath
