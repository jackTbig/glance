"""探针：常驻 claude 子进程逐句翻译，打印每句往返毫秒。
用法：python scripts/claude_probe.py "<claude 完整路径>" [模型名]
"""
import json
import subprocess
import sys
import time

SYSTEM = (
    "你是英译中会议同传。用户每条消息是一句英文会议发言，你只输出对应的简体中文译文，"
    "不加解释、不加引号、不加前缀。专有名词与产品名保留原文。"
)
claude = sys.argv[1]
model = sys.argv[2] if len(sys.argv) > 2 else "sonnet"
args = [
    claude, "-p", "--input-format", "stream-json", "--output-format", "stream-json",
    "--verbose", "--model", model, "--system-prompt", SYSTEM, "--disallowedTools", "*",
]
p = subprocess.Popen(
    args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    text=True, encoding="utf-8",
)

sentences = [
    "Good morning everyone, let's get started.",
    "We need to finalize the GPU server budget before the end of this month.",
    "The migration to the new data center is scheduled for the second week of October.",
]
for s in sentences:
    line = json.dumps({"type": "user", "message": {"role": "user", "content": s}, "parent_tool_use_id": None})
    t0 = time.perf_counter()
    p.stdin.write(line + "\n")
    p.stdin.flush()
    while True:
        out = p.stdout.readline()
        if not out:
            print("子进程退出")
            sys.exit(1)
        try:
            ev = json.loads(out)
        except json.JSONDecodeError:
            continue
        if ev.get("type") == "result":
            ms = (time.perf_counter() - t0) * 1000
            print(f"{ms:.0f}ms  {ev.get('subtype')}  {ev.get('result', '')!r}")
            break
p.stdin.close()
p.wait(timeout=10)
