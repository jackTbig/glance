#!/usr/bin/env bash
# 构建安装包：release 编译 → 打包插件 → .iss 转带 BOM 的 UTF-8 → ISCC → dist/translate-tool-setup-<ver>.exe
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "[1/4] 编译桌面端"
cargo build --release -p translate-tool

echo "[2/4] 打包 Chrome 插件"
(cd extension && npm run build)

echo "[3/4] 准备 Inno 脚本"
mkdir -p installer/build dist
PYTHONUTF8=1 python - <<'PYEOF'
import io
src = io.open("installer/translate-tool.iss", encoding="utf-8").read()
io.open("installer/build/translate-tool.iss", "w", encoding="utf-8-sig", newline="\r\n").write(src)
PYEOF

echo "[4/4] 生成安装包"
ISCC="D:/Tools/InnoSetup/ISCC.exe"
# Inno 7 的 ISCC 用 -q（不是 /Q）；脚本路径必须给 Windows 形式，否则 /d/... 会被当成 /d 选项
TARGET_DIR="${CARGO_TARGET_DIR:-D:/DevCache/tt-target}/release"
"$ISCC" -q   "/DTargetDir=$(cygpath -w "$TARGET_DIR")"   "/DRepoDir=$(cygpath -w "$ROOT")"   "$(cygpath -w "$ROOT/installer/build/translate-tool.iss")"
ls -la dist/
