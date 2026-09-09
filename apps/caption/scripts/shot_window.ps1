# Screenshot one top-level window by title into a PNG (DPI aware).
# Usage: powershell -File scripts\shot_window.ps1 -Title "live-caption 设置" -Out C:\tmp\x.png
param([string]$Title, [string]$Out)
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public static class SW {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
}
"@
[void][SW]::SetProcessDPIAware()
Add-Type -AssemblyName System.Drawing
$h = [SW]::FindWindowW($null, $Title)
if ($h -eq [IntPtr]::Zero) { "window not found: $Title"; exit 1 }
$r = New-Object SW+RECT; [void][SW]::GetWindowRect($h, [ref]$r)
$w = $r.R - $r.L; $hh = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap $w, $hh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L, $r.T, 0, 0, (New-Object System.Drawing.Size $w, $hh))
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
"saved $Out ($w x $hh at $($r.L),$($r.T))"
