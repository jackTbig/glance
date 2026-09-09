# Tray menu smoke test (ASCII only): posts the tray callback message to open the menu,
# picks the N-th item with keyboard, then checks the log for "tray menu:".
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\tray_test.ps1 [itemIndex]
param([int]$Item = 1)
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public static class T {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@
[void][T]::SetProcessDPIAware()
$h = [T]::FindWindowW("tray_icon_app", $null)
"tray hwnd=$h"
if ($h -eq [IntPtr]::Zero) { "no tray window"; exit 1 }
# 6002 = tray-icon callback message, lparam 0x0205 = WM_RBUTTONUP -> opens the context menu at cursor
[void][T]::PostMessageW($h, 6002, [IntPtr]0, [IntPtr]0x0205)
Start-Sleep -Milliseconds 700
for ($i = 0; $i -lt $Item; $i++) { [T]::keybd_event(0x28, 0, 0, [UIntPtr]::Zero); [T]::keybd_event(0x28, 0, 2, [UIntPtr]::Zero); Start-Sleep -Milliseconds 120 }
[T]::keybd_event(0x0D, 0, 0, [UIntPtr]::Zero); [T]::keybd_event(0x0D, 0, 2, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 1500
$log = Get-ChildItem "D:\My_data\Claude\live-caption\logs" -Filter "live-caption.*" | Sort-Object LastWriteTime | Select-Object -Last 1
Get-Content $log.FullName -Encoding utf8 | Select-String -Pattern "tray menu|tray" | Select-Object -Last 3 | ForEach-Object { $_.Line }
