#!/usr/bin/env python3
"""The status line and /output-style in a real terminal: the person's command is told the session as JSON and its
line is drawn above the prompt, again after each turn; /output-style lists the styles and one chosen is in the next
request's system prompt, kept in the settings.

Run: python3 tests/pty/chat_style.py target/debug/code-arena. Exit code 0 when it holds.
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
        ev = {"choices": [{"delta": {"content": f"Answer {len(seen)}."}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1200, "completion_tokens": 30}}
        out = f"data: {json.dumps(ev)}\n\ndata: [DONE]\n\n".encode()
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); srv.daemon_threads = True
threading.Thread(target=srv.serve_forever, daemon=True).start()
home = tempfile.mkdtemp(); work = tempfile.mkdtemp(); tools = tempfile.mkdtemp()
# The status line: a script of the person's, reading what it is told.
script = os.path.join(tools, "status.py")
open(script, "w").write("import json, sys\nj = json.load(sys.stdin)\nprint(f\"[{j['model']['display_name']} | {j['mode']} | {j['output_style']['name']} | in {j['cost']['total_input_tokens']}]\")\n")
os.makedirs(os.path.join(home, "config"))
settings = os.path.join(home, "config", "settings.json")
open(settings, "w").write(json.dumps({"arena.syncChats": False, "mcp.arenaTools": False, "mcp.codeIntelTools": False,
                                      "statusLine": {"type": "command", "command": f"{sys.executable} -I {script}"}}))
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
assert read_until(rb"\[qwen \| ask \| default \| in 0\]"), buf
os.write(fd, b"hello\r"); assert read_until(rb"Answer 1\."), buf
assert read_until(rb"\[qwen \| ask \| default \| in 1200\]"), buf
os.write(fd, b"/output-style\r")
assert read_until(rb"\xe2\x97\x8f default"), buf
assert read_until(rb"learning"), buf
os.write(fd, b"/output-style explanatory\r")
assert read_until(rb"The output style is explanatory from the next message"), buf
os.write(fd, b"and now\r"); assert read_until(rb"Answer 2\."), buf
system = seen[-1]["messages"][0]["content"]
assert "# Output style: explanatory" in system and "Read before you change" in system, system[:400]
assert "hello" in json.dumps(seen[-1]["messages"]), "the conversation is kept"
assert read_until(rb"\| explanatory \|"), buf
assert json.load(open(settings))["outputStyle"] == "explanatory"
os.write(fd, b"\x04")
_, status = os.waitpid(pid, 0)
assert status == 0, status
print("exit status:", status, "| requests:", len(seen))
