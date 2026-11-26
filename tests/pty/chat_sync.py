#!/usr/bin/env python3
"""The chat kept with a chat in Arena, in a real terminal: a question sent there, a message added on the web shown at
the prompt as it comes (from the watch stream), read by the model at the next turn; /sync says where it stands.

One local HTTP server is both the fake gateway and the fake Arena. Run: python3 tests/pty/chat_sync.py target/debug/code-arena
Exit code 0 when it holds; an assertion says what did not.
"""
import http.server, json, os, pty, re, select, sys, tempfile, threading, time
binary = sys.argv[1]
lock = threading.Lock()
asked = []      # what the model was asked
kept = []       # what Arena was sent, in order, with the ids it gave
web = []        # what the web added, given at the next read
watchers = []   # the watch streams' wakers
news = threading.Event()

class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def send(self, code, body, ctype="application/json"):
        out = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(code); self.send_header("Content-Type", ctype); self.send_header("Content-Length", str(len(out)))
        self.end_headers(); self.wfile.write(out)
    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(n) or b"{}") if n else {}
    def do_GET(self):
        p = self.path.split("?")[0]
        if p == "/api/code-arena/watch":
            self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.send_header("Cache-Control", "no-cache")
            self.send_header("Connection", "close"); self.end_headers()
            try:
                self.wfile.write(b'data: {"type":"watching"}\n\n'); self.wfile.flush()
                while True:
                    if news.wait(1):
                        news.clear()
                        self.wfile.write(b'data: {"type":"chat","id":"c-1","what":"messages"}\n\n'); self.wfile.flush()
                    else:
                        self.wfile.write(b": still here\n\n"); self.wfile.flush()
            except OSError:
                # Code Arena went: the stream ends.
                return
        if p == "/api/code-arena/chats/c-1":
            with lock:
                given, web[:] = list(web), []
            base = kept[-1]["id"] if kept else None
            if not given:
                return self.send(200, {"unchanged": True, "stamp": "s"})
            for m in given:
                kept.append(m)
            return self.send(200, {"id": "c-1", "title": "t", "count": len(kept), "stamp": f"s{len(kept)}", "base": base,
                                   "fork": False, "more": False, "summary": None, "messages": given})
        # Every other probe: there.
        return self.send(200, [])
    def do_POST(self):
        p = self.path.split("?")[0]
        b = self.body()
        if p == "/v1/chat/completions":
            with lock: asked.append(b)
            a = {"choices": [{"delta": {"content": f"Answer {len(asked)}."}, "finish_reason": "stop"}],
                 "usage": {"prompt_tokens": 50, "completion_tokens": 3}}
            return self.send(200, f"data: {json.dumps(a)}\n\ndata: [DONE]\n\n".encode(), "text/event-stream")
        if p == "/api/code-arena/chats":
            return self.send(200, {"id": "c-1", "created": True})
        if p == "/api/code-arena/chats/c-1/messages":
            ids = []
            with lock:
                for m in b.get("messages", []):
                    m = dict(m, id=f"m{len(kept)}"); kept.append(m); ids.append(m["id"])
            return self.send(200, {"ids": ids, "count": len(kept), "stamp": f"s{len(kept)}", "summarized": False})
        if p == "/api/code-arena/chats/c-1/turn":
            return self.send(200, {"until": "2026-11-26T10:00:00Z"})
        if p == "/api/code-arena/chats/c-1/queue/take":
            return self.send(200, {"messages": []})
        return self.send(400, {"status": "invalid", "error": "not here"})
    def do_PUT(self):
        self.body(); self.send(204, b"")
    def do_DELETE(self):
        self.send(204, b"")

class S(http.server.ThreadingHTTPServer):
    daemon_threads = True
srv = S(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
url = f"http://127.0.0.1:{srv.server_port}"
home = tempfile.mkdtemp(); work = tempfile.mkdtemp()
env = dict(os.environ, CODE_ARENA_HOME=home, ARENA_URL=url, ARENA_API_KEY="sk-test-only", ARENA_GATEWAY_URL=url,
           ARENA_MODEL="qwen", NO_PROXY="127.0.0.1", TERM="xterm-256color")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(work); os.execve(binary, [binary, "chat"], env)
buf = b""
def read_until(pat, t=10):
    global buf
    end = time.time() + t
    if re.search(pat, buf): return True
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try: buf += os.read(fd, 4096)
            except OSError: break
            if re.search(pat, buf): return True
    return False
def wait_for(cond, t=10):
    end = time.time() + t
    while time.time() < end:
        if cond(): return True
        time.sleep(0.05)
    return False

assert read_until(rb"Ctrl\+D leaves"), buf
os.write(fd, b"first question\r")
assert read_until(rb"Answer 1\."), buf
assert read_until(rb"compacts at"), buf
# The turn's messages reach Arena.
assert wait_for(lambda: len(kept) >= 2), kept
assert kept[0]["content"] == "first question" and kept[1]["content"] == "Answer 1.", kept
# Someone writes on the web while the prompt waits: it shows at once.
with lock:
    web.append({"role": "user", "content": "asked on the web", "id": "w1"})
    web.append({"role": "assistant", "content": "The web's answer.", "id": "w2"})
news.set()
assert read_until(rb"From the web: asked on the web"), buf
assert read_until(rb"The web's answer\."), buf
os.write(fd, b"/sync\r")
assert read_until(rb"Kept with the chat"), buf
assert read_until(rb"come as they are written"), buf
os.write(fd, b"second question\r")
assert read_until(rb"Answer 2\."), buf
last = json.dumps(asked[-1])
assert "asked on the web" in last and last.index("asked on the web") < last.index("second question"), last
assert wait_for(lambda: any(m.get("content") == "second question" for m in kept)), kept
os.write(fd, b"\x04")
_, status = os.waitpid(pid, 0)
# Leaving waits for the last answer to reach Arena.
assert any(m.get("content") == "Answer 2." for m in kept), kept
assert [m["content"] for m in kept].count("first question") == 1, kept
text = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", buf).decode(errors="replace")
print(text)
assert status == 0, status
print("exit status:", status, "| sent to Arena:", len(kept), "| requests:", len(asked))
