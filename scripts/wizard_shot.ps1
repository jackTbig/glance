# Screenshot the first two pages of the Inno wizard, for checking the upgrade notice page.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\wizard_shot.ps1 -Setup <setup.exe> -OutDir <dir>
#
# NEVER send keystrokes to the foreground window for this. On 2026-09-11 an AppActivate by title
# picked the wrong window and an Enter keystroke landed in the user's File Explorer. This script
# only touches the wizard's own window handle and clicks its Next button by handle (BM_CLICK).
# It closes the wizard afterwards without installing anything.
# ASCII only: PowerShell 5.1 reads BOM-less UTF-8 as ANSI.
param([Parameter(Mandatory=$true)][string]$Setup, [Parameter(Mandatory=$true)][string]$OutDir)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public class WZ {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr c);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool f);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  public struct RECT { public int L,T,R,B; }

  public static IntPtr FindWizard(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint p; GetWindowThreadProcessId(h, out p);
      if (p != pid || !IsWindowVisible(h)) return true;
      RECT r; GetWindowRect(h, out r);
      if ((r.R - r.L) < 300) return true;
      found = h; return false;
    }, IntPtr.Zero);
    return found;
  }

  // The Next button's caption starts with CJK char 0x4E0B (Chinese UI)
  public static IntPtr FindNext(IntPtr wiz) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(wiz, delegate(IntPtr h, IntPtr l) {
      var sb = new StringBuilder(64); GetWindowText(h, sb, 64);
      string t = sb.ToString();
      if (t.Length > 0 && t[0] == (char)0x4E0B && IsWindowVisible(h)) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }

  public static void Front(IntPtr h) {
    uint p; uint fgT = (uint)GetWindowThreadProcessId(GetForegroundWindow(), out p);
    uint me = GetCurrentThreadId();
    AttachThreadInput(fgT, me, true); SetForegroundWindow(h); AttachThreadInput(fgT, me, false);
  }

  public static string Rect(IntPtr h) { RECT r; GetWindowRect(h, out r); return r.L + "," + r.T + "," + (r.R-r.L) + "," + (r.B-r.T); }
}
"@
[WZ]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null

function ShotWindow($h, $path) {
  $p = [WZ]::Rect($h).Split(',') | ForEach-Object { [int]$_ }
  $bmp = New-Object System.Drawing.Bitmap($p[2], $p[3])
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($p[0], $p[1], 0, 0, (New-Object System.Drawing.Size($p[2], $p[3])))
  $half = New-Object System.Drawing.Bitmap($bmp, (New-Object System.Drawing.Size([int]($p[2]/2), [int]($p[3]/2))))
  $half.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
}

# Inno unpacks itself into a *.tmp process that owns the wizard window
function SetupProcs { Get-Process | Where-Object { $_.ProcessName -like "*setup-*" } }

Start-Process $Setup -ArgumentList '/CURRENTUSER'
$wiz = [IntPtr]::Zero
for ($i = 0; $i -lt 120 -and $wiz -eq [IntPtr]::Zero; $i++) {
  Start-Sleep -Milliseconds 500
  foreach ($pr in (SetupProcs | Where-Object { $_.ProcessName -like "*.tmp" })) {
    $w = [WZ]::FindWizard([uint32]$pr.Id); if ($w -ne [IntPtr]::Zero) { $wiz = $w }
  }
}
if ($wiz -eq [IntPtr]::Zero) { Write-Output "wizard window not found within 60s"; SetupProcs | Stop-Process -Force; exit 1 }
Start-Sleep -Seconds 2
[WZ]::Front($wiz); Start-Sleep -Milliseconds 800
ShotWindow $wiz (Join-Path $OutDir "wizard_page1.png")

$next = [WZ]::FindNext($wiz)
if ($next -ne [IntPtr]::Zero) {
  [WZ]::SendMessage($next, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
  Start-Sleep -Seconds 2
  [WZ]::Front($wiz); Start-Sleep -Milliseconds 600
  ShotWindow $wiz (Join-Path $OutDir "wizard_page2.png")
}
SetupProcs | Stop-Process -Force
Write-Output "saved wizard_page1.png / wizard_page2.png, wizard closed without installing"
