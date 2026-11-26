#!/usr/bin/env python3
"""Typing while a turn works, in a real terminal: a command typed meanwhile runs when the turn ends (not sent to the
model), and ↑ takes back a line sent meanwhile, which then waits at the prompt.

Run: python3 tests/pty/chat_typed.py target/debug/code-arena. Exit code 0 when it holds.
"""
import http.server, json, os, pty, re, select, sys, tempfile, threading, time
binary = sys.argv[1]
seen = []
first_started = threading.Event()
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        out = json.dumps({"data": [{"id": "qwen"}]}).encode()
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        seen.append(body)
        if len(seen) == 1:
            first_started.set()
            # A slow answer: time to type meanwhile.
            time.sleep(2.5)
        a = {"choices": [{"delta": {"content": f"Answer {len(seen)}."}, "finish_reason": "stop"}]}
        out = f"data: {json.dumps(a)}\n\ndata: [DONE]\n\n".encode()
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
class S(http.server.ThreadingHTTPServer):
    daemon_threads = True
srv = S(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
home = tempfile.mkdtemp(); work = tempfile.mkdtemp()
env = dict(os.environ, CODE_ARENA_HOME=home, ARENA_URL="https://arena.example", ARENA_API_KEY="sk-test-only",
           ARENA_GATEWAY_URL=f"http://127.0.0.1:{srv.server_port}", ARENA_MODEL="qwen", NO_PROXY="127.0.0.1", TERM="xterm-256color")
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
assert read_until(rb"Ctrl\+D leaves"), buf
os.write(fd, b"first question\r")
assert first_started.wait(10)
os.write(fd, b"/cost\r")
assert read_until(rb"/cost runs when the turn ends"), buf
os.write(fd, b"second thoughts\r")
time.sleep(0.2)
os.write(fd, b"\x1b[A")
assert read_until(rb"Took back \"second thoughts\""), buf
assert read_until(rb"Answer 1\."), buf
# The command ran after the turn, and was never the model's.
assert read_until(rb"This run: "), buf
assert len(seen) == 1, seen
assert "/cost" not in json.dumps(seen[0]) and "second thoughts" not in json.dumps(seen[0])
# The line taken back waits at the prompt: Enter sends it.
os.write(fd, b"\r")
assert read_until(rb"Answer 2\."), buf
assert "second thoughts" in json.dumps(seen[1]["messages"][-1]), seen[1]["messages"][-1]
os.write(fd, b"\x04")
_, status = os.waitpid(pid, 0)
assert status == 0, status
print("exit status:", status, "| requests:", len(seen))
