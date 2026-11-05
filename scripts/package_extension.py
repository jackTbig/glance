#!/usr/bin/env python3
"""打包 Chrome 扩展成商店上传用的 ZIP。

商店要的是「manifest.json 在压缩包根目录」的 ZIP，条目名必须用正斜杠：
Windows 的 .NET ZipFile 写出来的是反斜杠，Chrome 会当成文件名的一部分，
结果图标与 KaTeX 字体全找不到。这里用 Python 的 zipfile，arcname 一律 as_posix()。

    python scripts/package_extension.py             # 先构建再打包
    python scripts/package_extension.py --no-build  # 只打包现有的 extension/dist
"""
import argparse
import json
import pathlib
import subprocess
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
EXT = ROOT / "extension"
DIST = EXT / "dist"
OUT = ROOT / "dist"


def build() -> None:
    if sys.platform == "win32":
        subprocess.run("npm run build", cwd=EXT, check=True, shell=True)
    else:
        subprocess.run(["npm", "run", "build"], cwd=EXT, check=True)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--no-build", action="store_true", help="不重新构建，只打包现成的 dist")
    args = ap.parse_args()

    if not args.no_build:
        build()
    if not (DIST / "manifest.json").exists():
        sys.exit(f"没有 {DIST / 'manifest.json'}，先构建再打包")

    version = json.loads((DIST / "manifest.json").read_text(encoding="utf-8"))["version"]
    OUT.mkdir(exist_ok=True)
    target = OUT / f"glance-extension-{version}.zip"
    files = sorted(p for p in DIST.rglob("*") if p.is_file())
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as z:
        for p in files:
            z.write(p, p.relative_to(DIST).as_posix())

    size_mb = target.stat().st_size / 1024 / 1024
    print(f"打包完成 → {target}")
    print(f"  {len(files)} 个文件，{size_mb:.2f} MB")
    print("  商店上传：Chrome Web Store → 新建项目 → 上传该 ZIP")


if __name__ == "__main__":
    main()
