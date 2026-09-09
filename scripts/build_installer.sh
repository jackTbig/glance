#!/usr/bin/env bash
# 构建统一安装包 → dist/translate-tool-setup-<ver>.exe
#
# 装两个程序：简译（必装，约 9 MB）+ 实时字幕（可选组件，含模型约 700 MB）。
# 字幕默认**不编译**——它要 Vulkan SDK + CMake + Ninja，首次 5–10 分钟。
# 要把字幕打进包里就加 --with-caption。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

WITH_CAPTION=0
for a in "$@"; do
  case "$a" in
    --with-caption) WITH_CAPTION=1 ;;
    *) echo "未知参数：$a（可用：--with-caption）" >&2; exit 2 ;;
  esac
done

echo "[1/5] 编译简译"
# 一律显式 -p，**不要用 --workspace**：那会连 apps/caption 一起编，
# 于是这台机器上没有 Vulkan SDK 的人连简译的安装包都打不出来。
cargo build --release -p translate-tool

# 字幕有自己的 CARGO_TARGET_DIR（见 apps/caption/scripts/env.sh）：共用一个 target
# 会让字幕一次全量编译把简译的增量构建堵在目录锁上。
CAPTION_DIR="${LC_TARGET_DIR:-D:/DevCache/lc-target}/release"
if [ "$WITH_CAPTION" = 1 ]; then
  echo "[2/5] 编译实时字幕（首次约 5–10 分钟，需要 Vulkan SDK）"
  # shellcheck disable=SC1091
  source apps/caption/scripts/env.sh
  cargo build --release -p live-caption
  CAPTION_DIR="${CARGO_TARGET_DIR:-D:/DevCache/lc-target}/release"
else
  echo "[2/5] 跳过字幕编译（加 --with-caption 才编）"
fi

if [ ! -f "$CAPTION_DIR/live-caption.exe" ]; then
  echo "  ⚠ $CAPTION_DIR 下没有 live-caption.exe，安装包里的「实时字幕」组件将无法勾选" >&2
  echo "    要带上它：bash scripts/build_installer.sh --with-caption" >&2
fi

echo "[3/5] 打包 Chrome 插件"
(cd extension && npm run build)

echo "[4/5] 准备 Inno 脚本"
mkdir -p installer/build dist
PYTHONUTF8=1 python - <<'PYEOF'
import io
src = io.open("installer/translate-tool.iss", encoding="utf-8").read()
io.open("installer/build/translate-tool.iss", "w", encoding="utf-8-sig", newline="\r\n").write(src)
PYEOF

echo "[5/5] 生成安装包"
ISCC="D:/Tools/InnoSetup/ISCC.exe"
# Inno 7 的 ISCC 用 -q（不是 /Q）；脚本路径必须给 Windows 形式，否则 /d/... 会被当成 /d 选项
TARGET_DIR="${TT_TARGET_DIR:-D:/DevCache/tt-target}/release"
"$ISCC" -q \
  "/DTargetDir=$(cygpath -w "$TARGET_DIR")" \
  "/DCaptionDir=$(cygpath -w "$CAPTION_DIR")" \
  "/DRepoDir=$(cygpath -w "$ROOT")" \
  "$(cygpath -w "$ROOT/installer/build/translate-tool.iss")"
ls -la dist/
