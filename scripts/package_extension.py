#!/usr/bin/env python3
"""打包 Chrome 扩展成商店上传用的 ZIP。

商店的规矩（https://developer.chrome.com/docs/webstore/prepare）：
- manifest.json 必须在压缩包**根目录**（不能套一层文件夹）
- manifest 里**不许有 "key" 字段**——商店自己分配扩展 ID，带 key 直接拒收：
  "key field is not allowed in manifest"。key 只是给解压加载钉 ID 用的，
  解压版还得靠它把 ID 钉在 bhngnlkgffjimhcmmhfkgcgbohdecoch，所以**不能删源码里那份**。
- 必填字段：name / version / icons / description（≤132 字符）
- 包大小上限 2GB；首次上传若想保住某个扩展 ID，可在根目录放 key.pem（私钥）

所以这里只做一件事：**压缩包里那份 manifest 去掉 key**，
extension/dist/manifest.json（解压版用的那份）一个字不动。

    python scripts/package_extension.py             # 先构建再打包
    python scripts/package_extension.py --no-build  # 只打包现成的 extension/dist
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

# 商店必填 / 硬性限制
REQUIRED_FIELDS = ("name", "version", "icons", "description")
MAX_DESCRIPTION = 132
MAX_PACKAGE = 2 * 1024 * 1024 * 1024


def build() -> None:
    if sys.platform == "win32":
        subprocess.run("npm run build", cwd=EXT, check=True, shell=True)
    else:
        subprocess.run(["npm", "run", "build"], cwd=EXT, check=True)


def check_package(manifest: dict, entries: list[str]) -> list[str]:
    """按商店的规矩自查一遍，把问题挑出来（不改内容，只报错）"""
    problems = []
    if "key" in manifest:
        problems.append("manifest 里还有 key 字段，商店会拒收（key field is not allowed in manifest）")
    for f in REQUIRED_FIELDS:
        if not manifest.get(f):
            problems.append(f"manifest 缺必填字段 {f}")
    desc = manifest.get("description", "")
    if len(desc) > MAX_DESCRIPTION:
        problems.append(f"description {len(desc)} 字符，超过商店的 {MAX_DESCRIPTION} 上限")
    if any(e.startswith("_metadata/") for e in entries):
        problems.append("包里不能有 _metadata/（那是 CRX 的东西，商店只收裸扩展）")
    if any("node_modules/" in e for e in entries):
        problems.append("包里混进了 node_modules/")
    if not any(e == "manifest.json" for e in entries):
        problems.append("manifest.json 不在压缩包根目录")
    return problems


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--no-build", action="store_true", help="不重新构建，只打包现成的 dist")
    args = ap.parse_args()

    if not args.no_build:
        build()
    manifest_path = DIST / "manifest.json"
    if not manifest_path.exists():
        sys.exit(f"没有 {manifest_path}，先构建再打包")

    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    version = manifest["version"]
    OUT.mkdir(exist_ok=True)
    target = OUT / f"glance-extension-{version}.zip"

    files = sorted(p for p in DIST.rglob("*") if p.is_file())
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as z:
        for p in files:
            rel = p.relative_to(DIST).as_posix()
            if rel == "manifest.json":
                # 商店那一份去掉 key；解压版那份不动
                store_manifest = {k: v for k, v in manifest.items() if k != "key"}
                z.writestr(rel, json.dumps(store_manifest, ensure_ascii=False, indent=2) + "\n")
            else:
                z.write(p, rel)

    # 自查：读回压缩包，看商店会看到什么
    with zipfile.ZipFile(target) as z:
        entries = z.namelist()
        packed_manifest = json.loads(z.read("manifest.json").decode("utf-8"))
    problems = check_package(packed_manifest, entries)
    if problems:
        sys.exit("打包出来的包不合商店的规矩：\n  - " + "\n  - ".join(problems))

    size_mb = target.stat().st_size / 1024 / 1024
    if target.stat().st_size > MAX_PACKAGE:
        sys.exit(f"包太大：{size_mb:.0f} MB，商店上限 2GB")

    print(f"打包完成 → {target}")
    print(f"  {len(entries)} 个文件，{size_mb:.2f} MB")
    print(f"  manifest 版本 {packed_manifest['version']}，key 字段已去掉（解压版的那份保留）")
    print("  商店上传：Chrome Web Store → 新建项目 → 上传该 ZIP")


if __name__ == "__main__":
    main()
