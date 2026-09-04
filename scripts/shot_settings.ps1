# Capture the settings window of live-caption with PrintWindow (works even when covered).
# Picks the visible top-level window of the process whose title is not exactly "live-caption".
# Usage: powershell -File scripts\shot_settings.ps1 -Out C:\tmp\settings.png
param([string]$Out)
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices; using System.Text; using System.Collections.Generic;
public static class SS {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  public static IntPtr Find(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid && IsWindowVisible(h)) { var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256); var t = sb.ToString(); if (t.Length > 0 && t != "live-caption") { found = h; return false; } } return true; }, IntPtr.Zero);
    return found;
  }
}
"@
[void][SS]::SetProcessDPIAware(); Add-Type -AssemblyName System.Drawing
$p = Get-Process live-caption -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $p) { "no process"; exit 1 }
$h = [SS]::Find([uint32]$p.Id)
if ($h -eq [IntPtr]::Zero) { "settings window not found"; exit 1 }
$r = New-Object SS+RECT; [void][SS]::GetWindowRect($h, [ref]$r); $w = $r.R - $r.L; $hh = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap $w, $hh; $g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc(); $ok = [SS]::PrintWindow($h, $hdc, 2); $g.ReleaseHdc($hdc)
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $bmp.Dispose()
"saved $Out ($w x $hh) ok=$ok"
