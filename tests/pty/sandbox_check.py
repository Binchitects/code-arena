#!/usr/bin/env python3
"""The agent's command in the sandbox, where this machine has one: a secret in the home folder stays hidden, a write inside the folder lands, one outside does not.

A fake gateway (a local HTTP server) answers as a model would. Run: python3 tests/pty/sandbox_check.py target/debug/code-arena
Exit code 0 when it holds; an assertion says what did not.
"""
import http.server, json, os, subprocess, sys, threading, tempfile
seen=[]
answers=[{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"run_shell","arguments":json.dumps({"command":"cat ~/.ssh/id_test 2>&1; echo ok > made.txt; echo x > OUT/escaped.txt 2>&1; echo done"})}}]},"finish_reason":"tool_calls"}]},
         {"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]}]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def do_POST(self):
        b=json.loads(self.rfile.read(int(self.headers['Content-Length']))); seen.append(b)
        out=f"data: {json.dumps(answers[min(len(seen)-1,1)])}\n\ndata: [DONE]\n\n".encode()
        self.send_response(200); self.send_header('Content-Length',str(len(out))); self.end_headers(); self.wfile.write(out)
import re
srv=http.server.HTTPServer(('127.0.0.1',0),H); threading.Thread(target=srv.serve_forever,daemon=True).start()
home=tempfile.mkdtemp(); work=tempfile.mkdtemp(); fake=tempfile.mkdtemp(); out_dir=tempfile.mkdtemp()
os.makedirs(fake+"/.ssh"); open(fake+"/.ssh/id_test","w").write("SECRET-KEY-TEXT")
env=dict(os.environ,HOME=fake,CODE_ARENA_HOME=home,ARENA_URL="https://arena.example",ARENA_API_KEY="sk-test-only",ARENA_GATEWAY_URL=f"http://127.0.0.1:{srv.server_port}",ARENA_MODEL="qwen",NO_PROXY="127.0.0.1")
answers[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]=answers[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"].replace("OUT",out_dir)
r=subprocess.run([sys.argv[1],"-p","x","--mode","yolo","-C",work],env=env,capture_output=True,text=True,timeout=60)
res=seen[1]["messages"][3]["content"]
ok = "SECRET" not in res and "done" in res and os.path.exists(work+"/made.txt") and not os.path.exists(out_dir+"/escaped.txt")
print("ok" if ok else "FAIL: "+res)
sys.exit(0 if ok else 1)
