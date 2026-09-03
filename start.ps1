Set-Location $PSScriptRoot
if (-not (Test-Path ".\config.toml")) { Copy-Item ".\config.example.toml" ".\config.toml" }
# 编译产物在 D:\DevCache\lc-target（见 scripts\env.sh 的 CARGO_TARGET_DIR）
$exe = "D:\DevCache\lc-target\release\live-caption.exe"
if (-not (Test-Path $exe)) { Write-Host "未找到 $exe，先在 Git Bash 里执行：source scripts/env.sh && cargo build --release"; exit 1 }
Start-Process -FilePath $exe -WorkingDirectory $PSScriptRoot
