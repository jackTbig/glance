#!/usr/bin/env bash
# 构建安装包：source 环境 → release 编译 → 把 .iss 转成带 BOM 的 UTF-8 → ISCC 编译 → dist/live-caption-setup-<ver>.exe
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source scripts/env.sh
cargo build --release
mkdir -p installer/build dist
PYTHONUTF8=1 python - <<'EOF'
import io
src = io.open("installer/live-caption.iss", encoding="utf-8").read()
io.open("installer/build/live-caption.iss", "w", encoding="utf-8-sig", newline="\r\n").write(src)
EOF
ISCC="D:/Tools/InnoSetup/ISCC.exe"
# ISCC（Inno 7）用 -q；脚本路径必须给 Windows 形式，否则 /d/... 会被 ISCC 当成 /d 选项
"$ISCC" -q "$(cygpath -w "$ROOT/installer/build/live-caption.iss")"
ls -la dist/
