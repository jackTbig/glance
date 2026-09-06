# End-to-end check: start app -> play the English fixture twice -> screenshot -> wait -> dump log + transcript -> stop app
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\e2e_check.ps1 [screenshot.png]
# (ASCII only: Windows PowerShell 5.1 reads BOM-less UTF-8 scripts as ANSI)
param([string]$Shot = "")
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
if (-not (Test-Path ".\config.toml")) { Copy-Item ".\config.example.toml" ".\config.toml" }
$exe = "D:\DevCache\lc-target\release\live-caption.exe"
$wav = Join-Path $root "tests\fixtures\en-10s.wav"

# DPI aware, otherwise CopyFromScreen only grabs the top-left part of a scaled display
Add-Type -TypeDefinition @"
using System.Runtime.InteropServices;
public static class DpiHelper { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }
"@
[void][DpiHelper]::SetProcessDPIAware()
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

Get-Process live-caption -ErrorAction SilentlyContinue | Stop-Process -Force

$p = Start-Process -FilePath $exe -WorkingDirectory $root -PassThru
Write-Host "app pid=$($p.Id), warming up model + claude session for 20s"
Start-Sleep -Seconds 20

$player = New-Object Media.SoundPlayer $wav
for ($i = 1; $i -le 2; $i++) {
    Write-Host "play #$i"
    $player.PlaySync()
    Start-Sleep -Seconds 2
    if ($i -eq 1 -and $Shot -ne "") {
        Start-Sleep -Seconds 4
        $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
        $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
        $bmp.Save($Shot, [System.Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose(); $bmp.Dispose()
        Write-Host "screenshot $($b.Width)x$($b.Height) -> $Shot"
    }
}
Write-Host "waiting 15s for asr + translate to drain"
Start-Sleep -Seconds 15

Write-Host "===== log lines (asr/translate/warn/error), this run only"
$since = (Get-Date).AddMinutes(-2).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm")
Get-ChildItem ".\logs" -Filter "live-caption.*" | Sort-Object LastWriteTime | Select-Object -Last 1 | ForEach-Object {
    Get-Content $_.FullName -Encoding utf8 | Select-String -Pattern "asr |translate|WARN|ERROR" | ForEach-Object { $_.Line } | Where-Object { $_ -ge $since }
}
Write-Host "===== transcript"
$t = Get-ChildItem ".\transcripts" -Filter *.md | Sort-Object LastWriteTime | Select-Object -Last 1
if ($t) { Get-Content $t.FullName -Encoding utf8 } else { Write-Host "(no transcript file)" }

Write-Host "===== process"
$alive = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
if ($alive) { Write-Host "app still running (ok), stopping"; Stop-Process -Id $p.Id -Force } else { Write-Host "APP EXITED (bad)" }
