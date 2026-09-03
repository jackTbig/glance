"""冒充 claude -p stream-json：每收一行 user 消息，回一行 result。
文本含 SLEEP 时睡 20 秒（测超时）；译文带 pid 便于验证会话轮换。"""
import json
import os
import sys
import time

sys.stdin.reconfigure(encoding="utf-8")
sys.stdout.reconfigure(encoding="utf-8")
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    text = msg["message"]["content"]
    if "SLEEP" in text:
        time.sleep(20)
    out = {"type": "result", "subtype": "success", "result": f"pid={os.getpid()} zh={text}"}
    sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
    sys.stdout.flush()
