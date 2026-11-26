#!/usr/bin/env python3
"""/fork N in a real terminal: the session carries on in a copy of what came before question N; the one it came from
stays as it was, and /resume goes back to it.

Run: python3 tests/pty/chat_fork.py target/debug/code-arena. Exit code 0 when it holds.
"""
import http.server, json, os, pty, re, select, sys, tempfile, threading, time
binary = sys.argv[1]
seen = []
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        out = json.dumps({"data": [{"id": "qwen"}]}).encode()
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        seen.append(body)
        ev = {"choices": [{"delta": {"content": f"Answer {len(seen)}."}, "finish_reason": "stop"}]}
        out = f"data: {json.dumps(ev)}\n\ndata: [DONE]\n\n".encode()
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); srv.daemon_threads = True
threading.Thread(target=srv.serve_forever, daemon=True).start()
home = tempfile.mkdtemp(); work = tempfile.mkdtemp()
os.makedirs(os.path.join(home, "config"))
open(os.path.join(home, "config", "settings.json"), "w").write(json.dumps({"arena.syncChats": False, "mcp.arenaTools": False, "mcp.codeIntelTools": False}))
env = dict(os.environ, CODE_ARENA_HOME=home, ARENA_URL="https://arena.example", ARENA_API_KEY="sk-test-only",
           ARENA_GATEWAY_URL=f"http://127.0.0.1:{srv.server_port}", ARENA_MODEL="qwen", NO_PROXY="127.0.0.1", TERM="xterm-256color")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(work); os.execve(binary, [binary, "chat"], env)
buf = b""
def read_until(pat, t=15):
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
def last(): return json.dumps(seen[-1]["messages"])
assert read_until(rb"Ctrl\+D leaves"), buf
os.write(fd, b"alpha path\r"); assert read_until(rb"Answer 1\."), buf
os.write(fd, b"beta path\r"); assert read_until(rb"Answer 2\."), buf
os.write(fd, b"/fork 2\r")
assert read_until(rb"In a fork now: (\S+), from before question 2\. (\S+) stays as it was"), buf
m = re.search(rb"In a fork now: (\S+), from before question 2\. (\S+) stays", buf)
fork, origin = m.group(1).decode(), m.group(2).decode()
os.write(fd, b"gamma path\r"); assert read_until(rb"Answer 3\."), buf
assert "alpha path" in last() and "beta path" not in last() and "gamma path" in last(), last()
os.write(fd, f"/resume {origin}\r".encode()); assert read_until(rb"Resumed " + origin.encode()), buf
os.write(fd, b"and now\r"); assert read_until(rb"Answer 4\."), buf
assert "beta path" in last() and "gamma path" not in last(), last()
os.write(fd, b"\x04")
_, status = os.waitpid(pid, 0)
assert status == 0, status
print("exit status:", status, "| fork:", fork, "| origin:", origin)
