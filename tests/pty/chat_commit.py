#!/usr/bin/env python3
"""/commit in a real terminal: the model groups the working tree's changes into commits (a fake gateway answers
propose_commits), they are shown, and /commit yes makes them, each with only its own lines.

Run: python3 tests/pty/chat_commit.py target/debug/code-arena. Exit code 0 when it holds.
"""
import http.server, json, os, pty, re, select, subprocess, sys, tempfile, threading, time
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
        text = json.dumps(body)
        assert "propose_commits" in text, text[:300]
        ids = re.findall(r"### (h\d+) · (\S+)", body["messages"][-1]["content"])
        a_ids = [i for i, f in ids if f != "b.txt"]
        b_ids = [i for i, f in ids if f == "b.txt"]
        groups = [{"message": "fix: a says two\n\nThe count was wrong.", "hunks": a_ids},
                  {"message": "feat: b, a new file", "hunks": b_ids}]
        call = {"index": 0, "id": "p1", "function": {"name": "propose_commits", "arguments": json.dumps({"groups": groups})}}
        ev = {"choices": [{"delta": {"tool_calls": [call]}, "finish_reason": "tool_calls"}]}
        out = f"data: {json.dumps(ev)}\n\ndata: [DONE]\n\n".encode()
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
srv = http.server.HTTPServer(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
home = tempfile.mkdtemp(); work = tempfile.mkdtemp()
def git(*a): return subprocess.run(["git", *a], cwd=work, capture_output=True, text=True, check=True).stdout
git("init", "-q"); git("config", "user.email", "t@example.com"); git("config", "user.name", "T")
open(os.path.join(work, "a.txt"), "w").write("one\n"); git("add", "."); git("commit", "-qm", "start")
open(os.path.join(work, "a.txt"), "w").write("two\n")
open(os.path.join(work, "b.txt"), "w").write("new\n")
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
os.write(fd, b"/commit\r")
assert read_until(rb"/commit yes makes them"), buf
assert b"fix: a says two" in buf and b"feat: b, a new file" in buf, buf
# Nothing is committed before the person's word.
assert git("log", "--format=%s").split("\n")[0] == "start"
os.write(fd, b"/commit yes\r")
assert read_until(rb"[0-9a-f]{7} feat: b, a new file"), buf
os.write(fd, b"\x04")
_, status = os.waitpid(pid, 0)
log = git("log", "--format=%s", "-3").split("\n")
assert log[:3] == ["feat: b, a new file", "fix: a says two", "start"], log
assert "a.txt" in git("show", "--name-only", "--format=", "HEAD~1") and "b.txt" not in git("show", "--name-only", "--format=", "HEAD~1")
assert "Code-Arena-Session:" in git("log", "-1", "--format=%b")
assert git("status", "--porcelain").strip() == ""
assert status == 0, status
print("exit status:", status, "| commits:", log[:2])
