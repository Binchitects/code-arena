# A stand-in for Arena's gateway for UI tests: an OpenAI-compatible /v1/models and a streaming /v1/chat/completions
# that answer by what the last message asks, so a test can make the agent stream Markdown, edit a file, run a
# command or make a plan without a real model. No key is checked; nothing leaves the machine.
#
#   python3 scripts/ui/fake-model.py 18733
#
# The last user message decides: "edit" edits src/main.rs, "run" runs `ls -la`, "plan" makes a plan, "ask" runs a
# command that needs an approval in ask mode, "think" thinks for some seconds before it answers (with its usage),
# anything else streams a Markdown answer with a code block. A tool's
# result is answered with a short line, so each turn ends.
import http.server
import json
import sys
import time

n = [0]

ANSWER = """## What I found

The folder holds **two files** and a `src/` folder. Here is the entry point:

```rust
fn main() {
    println!("hello");
}
```

- `notes.txt` says the answer is 42
- `src/main.rs` prints a greeting

Tell me what to change next."""

# Asked with "table" in the message: a table, a code block named for a file, citations and links.
TABLES = """| File | Lines | Kind |
|:--|--:|:-:|
| src/main.rs | 3 | code |
| notes.txt | 10 | text |
| zeta.md | 2 | notes |

See `src/main.rs:2` and notes.txt, or [the docs](https://example.com).

```rust:src/main.rs
fn main() {
    println!("hello");
}
```"""


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        out = json.dumps({"data": [{"id": "qwen"}, {"id": "qwen-large"}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        n[0] += 1
        last = body["messages"][-1]
        text = last.get("content") or ""
        if isinstance(text, list):
            text = " ".join(p.get("text", "") for p in text if isinstance(p, dict))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()

        def ev(delta, finish=None):
            chunk = {"choices": [{"delta": delta, "finish_reason": finish}]}
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
            self.wfile.flush()

        def call(name, args):
            ev({"tool_calls": [{"index": 0, "id": f"c{n[0]}", "function": {"name": name, "arguments": json.dumps(args)}}]}, "tool_calls")

        if last["role"] == "tool":
            ev({"content": "Done."})
            ev({}, "stop")
        elif "think" in text:
            # Asked without thinking (Skip thinking asks again so): the answer at once.
            thinks = body.get("chat_template_kwargs", {}).get("enable_thinking") is not False
            for word in ("Let me weigh this. " * 40 if thinks else "").split(" ")[:-1]:
                ev({"reasoning_content": word + " "})
                time.sleep(0.08)
            for word in ANSWER.split(" "):
                ev({"content": word + " "})
            done = {"choices": [{"delta": {}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 8600, "completion_tokens": 1200, "prompt_tokens_details": {"cached_tokens": 6278}}}
            self.wfile.write(f"data: {json.dumps(done)}\n\n".encode())
        elif "plan" in text:
            steps = [
                {"title": "Read the code", "status": "done"},
                {"title": "Change the greeting", "status": "doing"},
                {"title": "Run the tests", "status": "todo", "check": "true"},
            ]
            call("plan", {"title": "A friendlier greeting", "steps": steps})
        elif "edit" in text:
            ev({"content": "Changing the greeting."})
            call("edit_file", {"path": "src/main.rs", "old_string": "\"hello\"", "new_string": "\"hello, world\""})
        elif "ask" in text:
            ev({"content": "I'll run it."})
            call("run_shell", {"command": "echo asked"})
        elif "run" in text:
            ev({"content": "I'll look at the folder first."})
            call("run_shell", {"command": "ls -la"})
        else:
            answer = TABLES if "table" in text else ANSWER
            for word in answer.split(" "):
                ev({"content": word + " "})
                time.sleep(0.005)
            ev({}, "stop")
        self.wfile.write(b"data: [DONE]\n\n")


http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
