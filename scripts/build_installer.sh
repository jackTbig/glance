#!/usr/bin/env bash
# 构建统一安装包 → dist/简译Glance-setup-<ver>.exe
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

echo "[1/6] 编译简译"
# 一律显式 -p，**不要用 --workspace**：那会连 apps/caption 一起编，
# 于是这台机器上没有 Vulkan SDK 的人连简译的安装包都打不出来。
cargo build --release -p translate-tool

# 字幕有自己的 CARGO_TARGET_DIR（见 apps/caption/scripts/env.sh）：共用一个 target
# 会让字幕一次全量编译把简译的增量构建堵在目录锁上。
CAPTION_DIR="${LC_TARGET_DIR:-D:/DevCache/lc-target}/release"
if [ "$WITH_CAPTION" = 1 ]; then
  echo "[2/6] 编译实时字幕（首次约 5–10 分钟，需要 Vulkan SDK）"
  # shellcheck disable=SC1091
  source apps/caption/scripts/env.sh
  cargo build --release -p live-caption
  CAPTION_DIR="${CARGO_TARGET_DIR:-D:/DevCache/lc-target}/release"
  # 指纹只在「刚按当前源码编过」时写：不编的话 exe 可能是旧源码的产物，写当前源码的指纹就是撒谎
  python scripts/caption_stamp.py > "$CAPTION_DIR/live-caption.stamp"
else
  echo "[2/6] 跳过字幕编译（加 --with-caption 才编）"
fi

if [ ! -f "$CAPTION_DIR/live-caption.exe" ]; then
  echo "  ⚠ $CAPTION_DIR 下没有 live-caption.exe，安装包里的「实时字幕」组件将无法勾选" >&2
  echo "    要带上它：bash scripts/build_installer.sh --with-caption" >&2
fi

# 字幕指纹：安装时与已装的比对，一致就跳过并提示「无需升级」。
# 拿不到（本次没编字幕）就给 unknown，安装包会一律按「有更新」处理——宁可多拷，不能漏更新。
CAPTION_STAMP="unknown"
if [ "$WITH_CAPTION" = 1 ] && [ -f "$CAPTION_DIR/live-caption.stamp" ]; then
  CAPTION_STAMP="$(tr -d '\r\n' < "$CAPTION_DIR/live-caption.stamp")"
fi

echo "[3/6] 打包 Chrome 插件"
(cd extension && npm run build)

# Jack 本机 Chrome 从这个稳定目录加载插件（见 CLAUDE.md「插件加载目录必须固定」）：
# 目录存在就同步，否则发新版后 Chrome 里跑的还是旧插件。目录不存在则跳过（别的机器没有它）。
STABLE_EXT_DIR="D:/DevCache/glance-extension"
if [ -d "$STABLE_EXT_DIR" ]; then
  cp -r extension/dist/. "$STABLE_EXT_DIR/"
  echo "  已同步插件到 $STABLE_EXT_DIR"
fi

echo "[4/6] 核对 AppId"
# 升级检测按 AppId 去注册表找旧版的卸载记录。.iss 里 [Setup] 的 AppId 与 [Code] 里查注册表用的
# GUID 是两处手写的——对不上的话「覆盖升级」提示页永远不出现，而且不会报任何错。
PYTHONUTF8=1 python - <<'PYEOF'
import io, re, sys
s = io.open("installer/translate-tool.iss", encoding="utf-8").read()
ids = set(re.findall(r"[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}", s))
if len(ids) != 1:
    sys.exit("AppId 与升级检测里的 GUID 不一致：%s" % sorted(ids))
print("  AppId 一致：" + ids.pop())
PYEOF

echo "[5/6] 准备 Inno 脚本"
mkdir -p installer/build dist
PYTHONUTF8=1 python - <<'PYEOF'
import io
src = io.open("installer/translate-tool.iss", encoding="utf-8").read()
io.open("installer/build/translate-tool.iss", "w", encoding="utf-8-sig", newline="\r\n").write(src)
PYEOF

# 模型大小：安装时已装的同名模型大小一致就跳过（模型文件从不原地改，换模型必换文件名）
MODEL_TURBO="D:/My_data/Claude/workspace/local-asr/ggml-large-v3-turbo-q5_0.bin"
MODEL_SMALL="D:/models/whisper/ggml-base.en.bin"
TURBO_SIZE="$(stat -c %s "$MODEL_TURBO")"
SMALL_SIZE="$(stat -c %s "$MODEL_SMALL")"

echo "[6/6] 生成安装包"
# 离线词库：由 offline-dict 的 build-dict 从 ECDICT 的 CSV 生成（65 MB 的 CSV 不进仓库）。
# 缺词库不致命——少一个可选组件而已，但要**明确说出来**：
# 装完发现没有离线查词，很难想到是打包时少了这个文件。
DICT_BIN="${TT_DICT_BIN:-D:/DevCache/tt-dict/ecdict.bin}"
DICT_CSV="${TT_DICT_CSV:-D:/DevCache/tt-dict/ecdict.csv}"
if [ ! -f "$DICT_BIN" ] && [ -f "$DICT_CSV" ]; then
  echo "  词库还没编译，从 $DICT_CSV 生成…"
  cargo run --release -q -p offline-dict --bin build-dict -- "$DICT_CSV" "$DICT_BIN"
fi
if [ -f "$DICT_BIN" ]; then
  DICT_SIZE=$(stat -c %s "$DICT_BIN")
  echo "  离线词库 $DICT_BIN（$((DICT_SIZE / 1048576)) MB）"
else
  echo "  ⚠ 没有离线词库（$DICT_BIN），这次打出来的包里「离线词典」组件是空的" >&2
  echo "    补法：curl -L -o $DICT_CSV https://raw.githubusercontent.com/skywind3000/ECDICT/master/ecdict.csv" >&2
  # 给 ISCC 一个存在的占位文件：Source 指向不存在的路径会直接编译失败
  DICT_BIN="$ROOT/docs/第三方组件授权.md"
  DICT_SIZE=0
fi

ISCC="D:/Tools/InnoSetup/ISCC.exe"
# Inno 7 的 ISCC 用 -q（不是 /Q）；脚本路径必须给 Windows 形式，否则 /d/... 会被当成 /d 选项
TARGET_DIR="${TT_TARGET_DIR:-D:/DevCache/tt-target}/release"
# **必须关掉 Git Bash 的参数路径转换**：像 /DTurboSize=574041195 这种值不像 Windows 路径的参数，
# 会被改写成 C:/Program Files/Git/DTurboSize=…，ISCC 把它当成第二个脚本名，报「不能指定多个脚本文件名」。
MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*" "$ISCC" -q \
  "/DTargetDir=$(cygpath -w "$TARGET_DIR")" \
  "/DCaptionDir=$(cygpath -w "$CAPTION_DIR")" \
  "/DRepoDir=$(cygpath -w "$ROOT")" \
  "/DCaptionStamp=$CAPTION_STAMP" \
  "/DTurboSize=$TURBO_SIZE" \
  "/DSmallSize=$SMALL_SIZE" \
  "/DDictBin=$(cygpath -w "$DICT_BIN")" \
  "/DDictSize=$DICT_SIZE" \
  "$(cygpath -w "$ROOT/installer/build/translate-tool.iss")"
echo "  字幕指纹：${CAPTION_STAMP:0:16}…"
ls -la dist/
