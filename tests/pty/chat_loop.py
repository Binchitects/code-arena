#!/usr/bin/env python3
"""/loop in a real terminal: a prompt run at once and then every so often as a background sub-agent, reading only;
its report reaches the agent with the next message.

Run: python3 tests/pty/chat_loop.py target/debug/code-arena. Exit code 0 when it holds.
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
        sub = "# You are a sub-agent" in json.dumps(body)
        text = "The build is green." if sub else "Noted."
        ev = {"choices": [{"delta": {"content": text}, "finish_reason": "stop"}]}
        out = f"data: {json.dumps(ev)}\n\ndata: [DONE]\n\n".encode()
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
assert read_until(rb"Ctrl\+D leaves"), buf
os.write(fd, b"/loop 1m check the build\r")
assert read_until(rb"Loop 1: every 1m, reading only"), buf
# The first run goes at once: wait for its request to the model.
end = time.time() + 15
while time.time() < end and not any("# You are a sub-agent" in json.dumps(b) for b in seen):
    time.sleep(0.05)
# Its report arrives once its run ends: /loop again (which draws the prompt anew) until it shows.
for _ in range(20):
    os.write(fd, b"/loop\r")
    if read_until(rb"Loop 1 ran \(1\): The build is green", 1):
        break
assert re.search(rb"Loop 1 ran \(1\): The build is green", buf), buf
assert read_until(rb"check the build \(1 runs"), buf
sub = next(b for b in seen if "# You are a sub-agent" in json.dumps(b))
tools = [t["function"]["name"] for t in sub.get("tools", [])]
assert "task" not in tools, tools
os.write(fd, b"how is it?\r")
assert read_until(rb"Noted\."), buf
last = json.dumps(seen[-1]["messages"])
assert "loop 1's run 1" in last and "The build is green." in last, last
os.write(fd, b"\x04")
_, status = os.waitpid(pid, 0)
assert status == 0, status
print("exit status:", status, "| requests:", len(seen))
