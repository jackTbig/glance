# Passthrough pipeline test without synthetic keyboard (ASCII only):
# posts WM_HOTKEY straight to the global-hotkey hidden window, then reads WS_EX_TRANSPARENT.
# Usage: powershell -File scripts\passthrough_test.ps1 [rounds]
param([int]$Rounds = 4)
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public static class PT {
  [DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr h, int i);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
}
"@
$ID_C = 589845; $ID_P = 589858
function ExStyle {
  $p = Get-Process live-caption -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $p) { return "no process" }
  $s = [PT]::GetWindowLongW($p.MainWindowHandle, -20)
  return ("transparent={0} (0x{1:X})" -f (($s -band 0x20) -ne 0), $s)
}
$hk = [PT]::FindWindowW("global_hotkey_app", $null)
"hotkey hwnd=$hk"
if ($hk -eq [IntPtr]::Zero) { exit 1 }
$fail = 0
for ($r = 1; $r -le $Rounds; $r++) {
  [void][PT]::PostMessageW($hk, 0x0312, [IntPtr]$ID_P, [IntPtr]0); Start-Sleep -Milliseconds 500
  $a = ExStyle
  [void][PT]::PostMessageW($hk, 0x0312, [IntPtr]$ID_C, [IntPtr]0); Start-Sleep -Milliseconds 500
  $b = ExStyle
  [void][PT]::PostMessageW($hk, 0x0312, [IntPtr]$ID_P, [IntPtr]0); Start-Sleep -Milliseconds 500
  $c = ExStyle
  [void][PT]::PostMessageW($hk, 0x0312, [IntPtr]$ID_P, [IntPtr]0); Start-Sleep -Milliseconds 500
  $d = ExStyle
  $ok = ($a -like "transparent=True*") -and ($b -like "transparent=False*") -and ($c -like "transparent=True*") -and ($d -like "transparent=False*")
  if (-not $ok) { $fail++ }
  "round $r ok=$ok | P:$a | C:$b | P:$c | P:$d"
}
"failures=$fail / $Rounds"
