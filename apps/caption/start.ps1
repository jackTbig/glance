# Launch live-caption (ASCII only: Windows PowerShell 5.1 reads BOM-less UTF-8 scripts as ANSI)
Set-Location $PSScriptRoot
if (-not (Test-Path ".\config.toml")) { Copy-Item ".\config.example.toml" ".\config.toml" }
# build output lives in D:\DevCache\lc-target (see scripts\env.sh CARGO_TARGET_DIR)
$exe = "D:\DevCache\lc-target\release\live-caption.exe"
if (-not (Test-Path $exe)) { Write-Host "exe not found: $exe  -> in Git Bash run: source scripts/env.sh && cargo build --release"; exit 1 }
Start-Process -FilePath $exe -WorkingDirectory $PSScriptRoot
