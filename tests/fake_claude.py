"""冒充 claude -p stream-json：每收一行 user 消息，先按词流式回 text_delta，再回一行 result。
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
    full = f"pid={os.getpid()} zh={text}"
    for i, word in enumerate(full.split(" ")):
        chunk = word if i == 0 else " " + word
        ev = {"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": chunk}}}
        sys.stdout.write(json.dumps(ev, ensure_ascii=False) + "\n")
        sys.stdout.flush()
        time.sleep(0.02)
    out = {"type": "result", "subtype": "success", "result": full}
    sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
    sys.stdout.flush()
