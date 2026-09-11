"""算实时字幕「实际编进 exe 的源码」的指纹。

安装包据此判断：用户已装的字幕与新包里的是不是同一份——是就跳过拷贝，
并在升级提示页上告诉用户「实时字幕已是最新，无需升级」。

为什么不直接比 exe 的哈希：工作区统一版本号，每次升版本号 Cargo 都会重编字幕、
重新链接，exe 字节必变，字幕就永远被判「有更新」，等于没做。
所以这里剔掉一切**只因版本号变化**而变的东西（根 Cargo.toml 的 version 行、
Cargo.lock 里工作区成员自己的条目），其余会影响字幕二进制的都算进来。
取舍原则：宁可多判一次「有更新」（多拷 77 MB），不能漏判（用户留着旧字幕）。

改了字幕的依赖范围（比如字幕新依赖了一个工作区 crate），要同步改下面的 TREES / FILES。

用法：python scripts/caption_stamp.py [仓库根目录]   → 标准输出打印 64 位十六进制指纹
"""
import hashlib
import io
import os
import re
import sys

ROOT = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# 字幕编进 exe 的源码：它自己 + 它依赖的两个工作区 crate
TREES = ["apps/caption/src", "crates/ai-translate/src", "crates/desktop-shell/src"]
FILES = ["apps/caption/Cargo.toml", "crates/ai-translate/Cargo.toml", "crates/desktop-shell/Cargo.toml"]
# 工作区成员：它们在 Cargo.lock 里的条目只随版本号变，不代表依赖变了
WORKSPACE_MEMBERS = {"translate-tool", "live-caption", "ai-translate", "desktop-shell", "text-capture", "mock-llm"}


def read_bytes(rel):
    with open(os.path.join(ROOT, rel), "rb") as f:
        # 不同机器检出的换行可能不同，统一掉，免得同一份源码算出两个指纹
        return f.read().replace(b"\r\n", b"\n")


def root_cargo_without_version():
    """根 Cargo.toml 里的 [profile.release] 会改变二进制，要算；version 行只随发版变，不算"""
    t = io.open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8").read().replace("\r\n", "\n")
    return re.sub(r'(?m)^version\s*=\s*"[^"]*"\s*$', "", t)


def lock_without_members():
    """第三方依赖升级（cargo update）会改变二进制，要算；工作区成员自己的条目只随版本号变，不算"""
    t = io.open(os.path.join(ROOT, "Cargo.lock"), encoding="utf-8").read().replace("\r\n", "\n")
    blocks = t.split("\n[[package]]\n")
    kept = []
    for b in blocks:
        m = re.search(r'(?m)^name = "([^"]+)"$', b)
        if m and m.group(1) in WORKSPACE_MEMBERS:
            continue
        kept.append(b)
    return "\n[[package]]\n".join(kept)


def fingerprint():
    paths = list(FILES)
    for tree in TREES:
        for d, _, names in os.walk(os.path.join(ROOT, tree)):
            for n in names:
                paths.append(os.path.relpath(os.path.join(d, n), ROOT).replace(os.sep, "/"))
    h = hashlib.sha256()
    for rel in sorted(paths):
        h.update(rel.encode("utf-8") + b"\0" + hashlib.sha256(read_bytes(rel)).digest())
    h.update(b"root-cargo\0" + hashlib.sha256(root_cargo_without_version().encode("utf-8")).digest())
    h.update(b"cargo-lock\0" + hashlib.sha256(lock_without_members().encode("utf-8")).digest())
    return h.hexdigest()


if __name__ == "__main__":
    print(fingerprint())
