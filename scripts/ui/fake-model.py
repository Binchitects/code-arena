# A stand-in for Arena's gateway for UI tests: an OpenAI-compatible /v1/models and a streaming /v1/chat/completions
# that answer by what the last message asks, so a test can make the agent stream Markdown, edit a file, run a
# command or make a plan without a real model. No key is checked; nothing leaves the machine.
#
#   python3 scripts/ui/fake-model.py 18733
#
# The last user message decides: "delegate" starts two sub-agents (each thinks, reads src/main.rs and reports),
# "edit" edits src/main.rs, "run" runs `ls -la`, "plan" makes a plan, "ask" runs a command that needs an approval
# in ask mode, "picture" runs a command that makes a picture in .code-arena/media/,
# "build" runs a command with no time limit (some seconds of output; its end is answered with a line),
# "think" thinks for some seconds before it answers (with its usage),
# anything else streams a Markdown answer with a code block. A tool's
# result is answered with a short line, so each turn ends.
import http.server
import json
import sys
import time

n = [0]

# A command that draws a 160 x 120 picture (a blue to orange fade) as a PNG in the project's media folder.
PICTURE = """mkdir -p .code-arena/media && python3 - <<'PY'
import struct, zlib
w, h = 160, 120
rows = b"".join(b"\\0" + bytes(c for x in range(w) for c in (40 + x, 90 + y, 220 - x)) for y in range(h))
chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
png = b"\\x89PNG\\r\\n\\x1a\\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
open(".code-arena/media/fade.png", "wb").write(png)
PY
echo made .code-arena/media/fade.png"""

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

# Asked with "diagram" in the message: a Mermaid diagram, and one that does not parse.
DIAGRAM = """How it flows:

```mermaid
graph TD
  A[Start] --> B{Is it?}
  B -->|Yes| C[OK]
  B -->|No| D[Find out]
```

And one written wrong:

```mermaid
graph TD
  A -->
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

        if last["role"] == "tool" and "survey" in json.dumps(body["messages"][1:2]).lower():
            # A sub-agent's call came back: it reports, slowly.
            for word in "## Findings\nsrc/main.rs prints a greeting and nothing else.".split(" "):
                ev({"content": word + " "})
                time.sleep(0.2)
            ev({}, "stop")
        elif last["role"] == "tool":
            ev({"content": "Done."})
            ev({}, "stop")
        elif "(Code Arena: job" in text:
            ev({"content": "The build passed."})
            ev({}, "stop")
        elif "build" in text:
            ev({"content": "Building; it takes a while."})
            steps = "for i in 1 2 3 4 5 6 7 8; do echo compiling part $i; sleep 1; done; echo built"
            call("run_shell", {"command": steps, "no_time_limit": True})
        elif "Survey" in text:
            # A sub-agent: it thinks a while, then reads.
            for word in ("Looking at the file first. " * 6).split(" ")[:-1]:
                ev({"reasoning_content": word + " "})
                time.sleep(0.15)
            call("read_file", {"path": "src/main.rs"})
        elif "delegate" in text:
            ev({"content": "Two sub-agents will look."})
            asks = [("Review the greeting", "Survey src/main.rs and report what it prints."),
                    ("Check the tests", "Survey the tests and report what they cover.")]
            ev({"tool_calls": [{"index": i, "id": f"c{n[0]}{i}", "function": {"name": "task", "arguments": json.dumps(
                {"description": d, "prompt": p})}} for i, (d, p) in enumerate(asks)]}, "tool_calls")
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
        elif "picture" in text:
            ev({"content": "Drawing it."})
            call("run_shell", {"command": PICTURE})
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
            answer = TABLES if "table" in text else DIAGRAM if "diagram" in text else ANSWER
            for word in answer.split(" "):
                ev({"content": word + " "})
                time.sleep(0.005)
            ev({}, "stop")
        self.wfile.write(b"data: [DONE]\n\n")


http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
