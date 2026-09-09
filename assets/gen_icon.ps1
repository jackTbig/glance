# Generate the app icon (design A: speech radical + arrow) at every size we need.
# ASCII only: PowerShell 5.1 reads BOM-less UTF-8 as ANSI and would eat the strings.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File assets\gen_icon.ps1
#
# Writes assets\icon_<size>.png and assets\icon.ico (16/24/32/48/64/128/256).
param([string]$OutDir = $PSScriptRoot)

Add-Type -AssemblyName System.Drawing

function Draw-Icon([int]$S) {
  $bmp = New-Object System.Drawing.Bitmap($S, $S, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = 'AntiAlias'

  # rounded square, blue gradient
  $r = [int]($S * 0.22)
  $path = New-Object System.Drawing.Drawing2D.GraphicsPath
  $path.AddArc(0, 0, $r*2, $r*2, 180, 90)
  $path.AddArc($S-$r*2, 0, $r*2, $r*2, 270, 90)
  $path.AddArc($S-$r*2, $S-$r*2, $r*2, $r*2, 0, 90)
  $path.AddArc(0, $S-$r*2, $r*2, $r*2, 90, 90)
  $path.CloseFigure()
  $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.Point(0,0)),
    (New-Object System.Drawing.Point($S,$S)),
    ([System.Drawing.Color]::FromArgb(255, 40, 86, 190)),
    ([System.Drawing.Color]::FromArgb(255, 78, 150, 240)))
  $g.FillPath($brush, $path)

  $w = [System.Drawing.Brushes]::White
  $pen = New-Object System.Drawing.Pen($w, [float]($S * 0.085))
  $pen.StartCap = 'Round'; $pen.EndCap = 'Round'; $pen.LineJoin = 'Round'

  # left: two dots over a vertical stroke -- the speech radical of the glyph "yi" (translate)
  $d = [int][math]::Max(2, $S * 0.085)
  $g.FillEllipse($w, [int]($S*0.24), [int]($S*0.24), $d, $d)
  $g.FillEllipse($w, [int]($S*0.24), [int]($S*0.40), $d, $d)
  $g.DrawLine($pen, [float]($S*0.28), [float]($S*0.58), [float]($S*0.28), [float]($S*0.76))

  # right: an arrow -- "turned into another tongue"
  $g.DrawLine($pen, [float]($S*0.46), [float]($S*0.50), [float]($S*0.74), [float]($S*0.50))
  $g.DrawLine($pen, [float]($S*0.62), [float]($S*0.36), [float]($S*0.76), [float]($S*0.50))
  $g.DrawLine($pen, [float]($S*0.62), [float]($S*0.64), [float]($S*0.76), [float]($S*0.50))

  $brush.Dispose(); $path.Dispose(); $pen.Dispose(); $g.Dispose()
  return $bmp
}

$sizes = 16,24,32,48,64,128,256
$files = @()
foreach ($s in $sizes) {
  $bmp = Draw-Icon $s
  $p = Join-Path $OutDir ("icon_{0}.png" -f $s)
  $bmp.Save($p, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  $files += $p
  Write-Output "wrote $p"
}

# Pack the PNGs into a multi-size .ico by hand (System.Drawing has no ICO writer).
$icoPath = Join-Path $OutDir "icon.ico"
$fs = [System.IO.File]::Create($icoPath)
$bw = New-Object System.IO.BinaryWriter($fs)
$bw.Write([UInt16]0); $bw.Write([UInt16]1); $bw.Write([UInt16]$sizes.Count)   # ICONDIR
$offset = 6 + 16 * $sizes.Count
$blobs = @()
for ($i = 0; $i -lt $sizes.Count; $i++) {
  $bytes = [System.IO.File]::ReadAllBytes($files[$i])
  $blobs += ,$bytes
  $s = $sizes[$i]
  $bw.Write([Byte]$(if ($s -ge 256) { 0 } else { $s }))   # width  (0 = 256)
  $bw.Write([Byte]$(if ($s -ge 256) { 0 } else { $s }))   # height
  $bw.Write([Byte]0)          # palette
  $bw.Write([Byte]0)          # reserved
  $bw.Write([UInt16]1)        # colour planes
  $bw.Write([UInt16]32)       # bits per pixel
  $bw.Write([UInt32]$bytes.Length)
  $bw.Write([UInt32]$offset)
  $offset += $bytes.Length
}
foreach ($b in $blobs) { $bw.Write($b) }
$bw.Flush(); $bw.Close(); $fs.Close()
Write-Output "wrote $icoPath"
