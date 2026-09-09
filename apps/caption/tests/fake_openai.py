"""冒充 OpenAI 兼容接口：POST /v1/chat/completions 以 SSE 流式返回 "pid=<pid> zh=<用户文本>"。
用法：python tests/fake_openai.py <port>"""
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1])


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(n) or b"{}")
        content = body["messages"][-1]["content"]
        text = content.split("请翻译：", 1)[-1].strip()
        n_ctx = content.count("\nEN: ") + (1 if content.startswith("EN: ") else 0)
        prov = 1 if "还没说完的半句" in content else 0
        prev = 1 if "此前已译出：" in content else 0
        full = f"pid={os.getpid()} ctx={n_ctx} prov={prov} prev={prev} zh={text}"
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for i, word in enumerate(full.split(" ")):
            chunk = word if i == 0 else " " + word
            ev = {"choices": [{"delta": {"content": chunk}}]}
            self.wfile.write(f"data: {json.dumps(ev, ensure_ascii=False)}\n\n".encode("utf-8"))
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


HTTPServer(("127.0.0.1", PORT), H).serve_forever()
