#!/usr/bin/env python3
"""The chat in a real terminal: a message typed, a write asked about and allowed with y, the answer, Ctrl+D.

A fake gateway (a local HTTP server) answers as a model would. Run: python3 tests/pty/chat_approval.py target/debug/code-arena
Exit code 0 when it holds; an assertion says what did not.
"""
import http.server, json, os, pty, select, sys, threading, time, tempfile, re
binary = sys.argv[1]
answers = [
  {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"write_file","arguments":json.dumps({"path":"hello.txt","content":"hi\n"})}}]},"finish_reason":"tool_calls"}]},
  {"choices":[{"delta":{"content":"I wrote hello.txt."},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":5}},
]
seen = []
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        seen.append(body)
        a = answers[min(len(seen)-1, len(answers)-1)]
        out = f"data: {json.dumps(a)}\n\ndata: [DONE]\n\n".encode()
        self.send_response(200); self.send_header('Content-Type','text/event-stream'); self.send_header('Content-Length',str(len(out))); self.end_headers(); self.wfile.write(out)
srv = http.server.HTTPServer(('127.0.0.1',0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
home = tempfile.mkdtemp(); work = tempfile.mkdtemp()
env = dict(os.environ, CODE_ARENA_HOME=home, ARENA_URL="https://arena.example", ARENA_API_KEY="sk-test-only",
           ARENA_GATEWAY_URL=f"http://127.0.0.1:{srv.server_port}", ARENA_MODEL="qwen", NO_PROXY="127.0.0.1", TERM="xterm-256color")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(work); os.execve(binary, [binary, "chat"], env)
buf = b""
def read_until(pat, t=10):
    global buf
    end = time.time()+t
    # What already came may hold it: look there first, then read on.
    if re.search(pat, buf): return True
    while time.time() < end:
        r,_,_ = select.select([fd],[],[],0.1)
        if r:
            try: buf += os.read(fd, 4096)
            except OSError: break
            if re.search(pat, buf): return True
    return False
assert read_until(rb"Ctrl\+D leaves"), buf
os.write(fd, b"write the file\r")
assert read_until(rb"Allow write_file"), buf
os.write(fd, b"y")
assert read_until(rb"I wrote hello.txt"), buf
assert read_until(rb"requests"), buf
os.write(fd, b"\x04")
_, status = os.waitpid(pid, 0)
text = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", buf).decode(errors="replace")
print(text)
print("exit status:", status, "| file:", open(os.path.join(work,"hello.txt")).read().strip(), "| requests:", len(seen))
