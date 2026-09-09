# Hotkey / passthrough smoke test (ASCII only).
# Sends Ctrl+Alt+P (toggle passthrough) and Ctrl+Alt+C (clickable) with keybd_event,
# reads WS_EX_TRANSPARENT (0x20) on the main window after each, and prints matching log lines.
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public static class HK {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr h, int i);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
}
"@
function Chord([byte]$vk) {
  [HK]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero); [HK]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
  [HK]::keybd_event($vk, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 60
  [HK]::keybd_event($vk, 0, 2, [UIntPtr]::Zero)
  [HK]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero); [HK]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 700
}
function ExStyle {
  $p = Get-Process live-caption -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $p) { return "no process" }
  $h = $p.MainWindowHandle
  $s = [HK]::GetWindowLongW($h, -20)
  return ("hwnd={0} exstyle=0x{1:X} transparent={2}" -f $h, $s, (($s -band 0x20) -ne 0))
}
"start:  " + (ExStyle)
Chord 0x50   # P -> toggle passthrough on
"after Ctrl+Alt+P: " + (ExStyle)
Chord 0x43   # C -> clickable
"after Ctrl+Alt+C: " + (ExStyle)
Chord 0x50
"after Ctrl+Alt+P again: " + (ExStyle)
Chord 0x50
"after Ctrl+Alt+P toggle back: " + (ExStyle)
# tray click -> panel window should appear
$tray = [HK]::FindWindowW("tray_icon_app", $null)
[void][HK]::PostMessageW($tray, 6002, [IntPtr]0, [IntPtr]0x0202)   # WM_LBUTTONUP
Start-Sleep -Milliseconds 1200
$panel = [HK]::FindWindowW($null, "live-caption 控制")
"tray hwnd=$tray panel hwnd=$panel"
$log = Get-ChildItem "D:\My_data\Claude\live-caption\logs" -Filter "live-caption.*" | Sort-Object LastWriteTime | Select-Object -Last 1
Get-Content $log.FullName -Encoding utf8 | Select-String -Pattern "hotkey|tray|热键" | Select-Object -Last 8 | ForEach-Object { $_.Line }

