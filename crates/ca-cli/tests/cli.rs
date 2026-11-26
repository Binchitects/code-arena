//! The program as people run it, against a fake Arena and gateway.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn code_arena(home: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_code-arena"));
    c.env("CODE_ARENA_HOME", home).env("NO_COLOR", "1").env("NO_PROXY", "127.0.0.1,localhost");
    for v in ["ARENA_URL", "ARENA_API_KEY", "ARENA_GATEWAY_URL", "ARENA_MCP_URL", "ARENA_MODEL", "CODE_ARENA_MODE"] {
        c.env_remove(v);
    }
    c
}

/// A model that answers one scripted event stream per request.
struct Script(Vec<String>, AtomicUsize);

impl Respond for Script {
    fn respond(&self, _r: &Request) -> ResponseTemplate {
        let i = self.1.fetch_add(1, Ordering::SeqCst).min(self.0.len() - 1);
        ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(self.0[i].clone())
    }
}

fn sse(event: Value) -> String {
    format!("data: {event}\n\ndata: [DONE]\n\n")
}

#[tokio::test]
async fn print_mode_reads_a_file_and_answers_in_json() {
    let gateway = MockServer::start().await;
    let read = sse(
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "read_file", "arguments": "{\"path\":\"notes.txt\"}"}}]}, "finish_reason": "tool_calls"}]}),
    );
    let answer = sse(
        json!({"choices": [{"delta": {"content": "The notes say 42."}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 120, "completion_tokens": 7}}),
    );
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(vec![read, answer], AtomicUsize::new(0)))
        .mount(&gateway)
        .await;

    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    std::fs::write(project.path().join("notes.txt"), "42\n").expect("write");
    std::fs::create_dir(project.path().join(".git")).expect("a repository");
    let out = code_arena(home.path())
        .args(["-p", "what do the notes say?", "--output", "json", "-C"])
        .arg(project.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    let v: Value = serde_json::from_str(stdout.trim()).expect("json");
    assert_eq!(v["result"], "The notes say 42.");
    assert_eq!(v["is_error"], false);
    assert_eq!(v["model"], "qwen");
    assert_eq!(v["usage"]["input_tokens"], 120);
    assert!(stderr.contains("● read_file notes.txt"), "{stderr}");
    assert!(!stderr.contains("sk-test-only") && !stdout.contains("sk-test-only"));
    // The project's folder was made, with its defaults.
    assert!(project.path().join(".code-arena/settings.json").is_file());
}

#[tokio::test]
async fn text_output_shows_each_call_with_its_result_and_no_blank_lines() {
    let gateway = MockServer::start().await;
    // A model that writes blank lines before it calls two reading tools at once, then answers.
    let calls = sse(json!({"choices": [{"delta": {"content": "\n\n", "tool_calls": [
        {"index": 0, "id": "a", "function": {"name": "list_dir", "arguments": "{}"}},
        {"index": 1, "id": "b", "function": {"name": "read_file", "arguments": "{\"path\":\"notes.txt\"}"}}
    ]}, "finish_reason": "tool_calls"}]}));
    let answer =
        sse(json!({"choices": [{"delta": {"content": "\nIt is a folder of notes."}, "finish_reason": "stop"}]}));
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(vec![calls, answer], AtomicUsize::new(0)))
        .mount(&gateway)
        .await;

    let home = tempfile::tempdir().expect("home");
    let folder = tempfile::tempdir().expect("folder");
    std::fs::write(folder.path().join("notes.txt"), "42\n").expect("write");
    let out = code_arena(home.path())
        .args(["-p", "what is this?", "-C"])
        .arg(folder.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert_eq!(stdout, "It is a folder of notes.\n");
    let lines: Vec<&str> = stderr.lines().collect();
    let list = lines.iter().position(|l| l.starts_with("● list_dir")).expect("list_dir shown");
    assert!(lines[list + 1].starts_with("  └ notes.txt"), "{stderr}");
    assert!(lines[list + 2].starts_with("● read_file notes.txt"), "{stderr}");
    assert!(lines[list + 3].starts_with("  └      1\t42"), "{stderr}");
    // A folder that is no repository is no project: it gets no .code-arena/.
    assert!(!folder.path().join(".code-arena").exists());
}

#[tokio::test]
async fn doctor_tells_an_8_0_arena_by_its_answers() {
    let arena = MockServer::start().await;
    let not_there = || ResponseTemplate::new(404).set_body_string("Status Code: 404; Not Found");
    for p in [
        "/api/code-arena/agents",
        "/api/code-arena/scheduled",
        "/api/code-arena/chats/00000000-0000-0000-0000-000000000000/project-work",
    ] {
        Mock::given(path(p)).respond_with(not_there()).mount(&arena).await;
    }
    Mock::given(method("PUT")).respond_with(not_there()).mount(&arena).await;
    // 8.0 has no route that makes a checklist.
    Mock::given(method("POST")).and(path("/api/code-arena/checklists")).respond_with(not_there()).mount(&arena).await;
    Mock::given(path("/api/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "Arena", "version": "8.0.0"})))
        .mount(&arena)
        .await;
    Mock::given(path("/api/code-arena/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"userName": "sara", "denied": []})))
        .mount(&arena)
        .await;
    Mock::given(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "qwen"}]})))
        .mount(&arena)
        .await;
    Mock::given(path("/mcp")).respond_with(ResponseTemplate::new(404)).mount(&arena).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"status": "invalid", "error": "no"})))
        .mount(&arena)
        .await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&arena).await;

    let home = tempfile::tempdir().expect("home");
    let out = code_arena(home.path())
        .arg("doctor")
        .env("ARENA_URL", arena.uri())
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", arena.uri())
        .output()
        .expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("account   sara"), "{stdout}");
    assert!(stdout.contains("automation.agents     this Arena does not have it"), "{stdout}");
    assert!(stdout.contains("checklists.edit       this Arena does not have it"), "{stdout}");
    assert!(stdout.contains("memory                yes"), "{stdout}");
    assert!(stdout.contains("says it is 8.0.0; features are read from its answers"), "{stdout}");
    // What it learned is kept for the next start.
    let servers = home.path().join("data/servers");
    assert_eq!(std::fs::read_dir(servers).expect("kept").count(), 1);
}

#[test]
fn version_names_the_licence_and_the_source() {
    let home = tempfile::tempdir().expect("home");
    let out = code_arena(home.path()).arg("version").output().expect("runs");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.starts_with(&format!("Code Arena {} (MIT licence)", env!("CARGO_PKG_VERSION"))), "{s}");
}

/// A model that answers every request the same, and keeps what each request held.
struct Echo(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);

impl Respond for Echo {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&r.body).expect("json");
        self.0.lock().expect("lock").push(body);
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse(json!({"choices": [{"delta": {"content": "Noted."}, "finish_reason": "stop"}]})))
    }
}

#[tokio::test]
async fn continue_takes_up_the_folders_last_session() {
    let gateway = MockServer::start().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(path("/v1/chat/completions")).respond_with(Echo(seen.clone())).mount(&gateway).await;
    let home = tempfile::tempdir().expect("home");
    let folder = tempfile::tempdir().expect("folder");
    let run = |args: &[&str]| {
        code_arena(home.path())
            .args(args)
            .arg("-C")
            .arg(folder.path())
            .env("ARENA_URL", "https://arena.example")
            .env("ARENA_API_KEY", "sk-test-only")
            .env("ARENA_GATEWAY_URL", gateway.uri())
            .env("ARENA_MODEL", "qwen")
            .output()
            .expect("runs")
    };
    let first = run(&["-p", "remember the word teal", "--output", "json"]);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let id = serde_json::from_slice::<Value>(&first.stdout).expect("json")["session"].as_str().expect("id").to_string();
    let second = run(&["-p", "which word?", "--continue", "--output", "json"]);
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&second.stdout).expect("json")["session"], id.as_str());
    let requests = seen.lock().expect("lock");
    let texts: Vec<String> = requests[1]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|m| m["content"].as_str().map(str::to_string))
        .collect();
    assert!(texts.iter().any(|t| t == "remember the word teal"), "{texts:?}");
    assert_eq!(texts.last().map(String::as_str), Some("which word?"));
    // A new session without --continue reads nothing of it.
    drop(requests);
    let third = run(&["-p", "fresh", "--output", "json"]);
    assert_ne!(serde_json::from_slice::<Value>(&third.stdout).expect("json")["session"], id.as_str());
}

/// A tiny MCP server in sh: one tool, `shout`, that answers with what it was given.
#[cfg(unix)]
const ECHO_SERVER: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"serverInfo":{"name":"shout"},"instructions":"Shout when asked."}}\n' "$id" ;;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"shout","description":"Says it loud","inputSchema":{"type":"object","properties":{"word":{"type":"string"}}}}]}}\n' "$id" ;;
    *'"tools/call"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"HELLO from the server"}]}}\n' "$id" ;;
  esac
done
"#;

#[cfg(unix)]
#[tokio::test]
async fn the_persons_own_mcp_server_answers_a_call() {
    let gateway = MockServer::start().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let call = sse(
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "mcp__echo__shout", "arguments": "{\"word\":\"hello\"}"}}]}, "finish_reason": "tool_calls"}]}),
    );
    let answer = sse(json!({"choices": [{"delta": {"content": "The server shouted."}, "finish_reason": "stop"}]}));
    struct Both(std::sync::Arc<std::sync::Mutex<Vec<Value>>>, Vec<String>);
    impl Respond for Both {
        fn respond(&self, r: &Request) -> ResponseTemplate {
            let mut s = self.0.lock().expect("lock");
            s.push(serde_json::from_slice(&r.body).expect("json"));
            let i = (s.len() - 1).min(self.1.len() - 1);
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(self.1[i].clone())
        }
    }
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Both(seen.clone(), vec![call, answer]))
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    let settings = json!({
        "mcpServers": {"echo": {"command": "sh", "args": ["-c", ECHO_SERVER], "trust": true}},
        "mcp.arenaTools": false,
        "mcp.codeIntelTools": false
    });
    std::fs::write(home.path().join("config/settings.json"), settings.to_string()).expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    let out = code_arena(home.path())
        .args(["-p", "shout hello", "--output", "json", "-C"])
        .arg(folder.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&out.stdout).expect("json")["result"], "The server shouted.");
    let requests = seen.lock().expect("lock");
    let tools: Vec<&str> =
        requests[0]["tools"].as_array().expect("tools").iter().filter_map(|t| t["function"]["name"].as_str()).collect();
    assert!(tools.contains(&"mcp__echo__shout"), "{tools:?}");
    let msgs = requests[1]["messages"].as_array().expect("messages");
    assert!(
        msgs.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("Shout when asked."))),
        "the server's instructions reach the model"
    );
    assert_eq!(msgs.last().expect("last")["content"], "HELLO from the server");
}

#[tokio::test]
async fn a_projects_command_runs_from_print_mode() {
    let gateway = MockServer::start().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(path("/v1/chat/completions")).respond_with(Echo(seen.clone())).mount(&gateway).await;
    let home = tempfile::tempdir().expect("home");
    let repo = tempfile::tempdir().expect("repo");
    std::fs::create_dir(repo.path().join(".git")).expect("a repository");
    let out = code_arena(home.path())
        .args(["-p", "/explain the parser", "-C"])
        .arg(repo.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let requests = seen.lock().expect("lock");
    let msgs = requests[0]["messages"].as_array().expect("messages");
    let asked = msgs.last().expect("last")["content"].as_str().expect("text");
    // The default .code-arena/commands/explain.md, with the words typed after the name.
    assert!(asked.starts_with("Explain the parser in this project"), "{asked}");
    // The default skill is listed for the model.
    assert!(msgs[0]["content"].as_str().expect("system").contains("- explain-module: Explain how one module"));
    let tools: Vec<&str> =
        requests[0]["tools"].as_array().expect("tools").iter().filter_map(|t| t["function"]["name"].as_str()).collect();
    assert!(
        tools.contains(&"skill") && tools.contains(&"read_result") && tools.contains(&"session_search"),
        "{tools:?}"
    );
}

/// Where this machine can make a sandbox: the agent's command cannot read what holds secrets, nor write outside.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn the_agents_commands_run_in_the_sandbox() {
    if ca_tools::sandbox::find().is_err() {
        return;
    }
    let fake_home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(fake_home.path().join(".ssh")).expect("ssh");
    std::fs::write(fake_home.path().join(".ssh/id_test"), "SECRET-KEY-TEXT").expect("key");
    // Not a temporary folder: commands may write the system's (on macOS, $TMPDIR under /var/folders).
    let outside = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("outside");
    let command = format!(
        "cat ~/.ssh/id_test 2>&1; echo ok > made.txt; echo x > {}/escaped.txt 2>&1; echo done",
        outside.path().display()
    );
    let call = sse(
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "run_shell", "arguments": json!({"command": command}).to_string()}}]}, "finish_reason": "tool_calls"}]}),
    );
    let answer = sse(json!({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}]}));
    let gateway = MockServer::start().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    struct Two(std::sync::Arc<std::sync::Mutex<Vec<Value>>>, Vec<String>);
    impl Respond for Two {
        fn respond(&self, r: &Request) -> ResponseTemplate {
            let mut s = self.0.lock().expect("lock");
            s.push(serde_json::from_slice(&r.body).expect("json"));
            let i = (s.len() - 1).min(1);
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(self.1[i].clone())
        }
    }
    Mock::given(path("/v1/chat/completions")).respond_with(Two(seen.clone(), vec![call, answer])).mount(&gateway).await;
    let home = tempfile::tempdir().expect("code-arena home");
    let folder = tempfile::tempdir().expect("folder");
    let out = code_arena(home.path())
        .args(["-p", "probe", "--mode", "yolo", "-C"])
        .arg(folder.path())
        .env("HOME", fake_home.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let requests = seen.lock().expect("lock");
    let result = requests[1]["messages"][3]["content"].as_str().expect("result").to_string();
    assert!(!result.contains("SECRET-KEY-TEXT"), "{result}");
    assert!(result.contains("done"), "{result}");
    assert!(folder.path().join("made.txt").exists(), "writes inside the folder: {result}");
    assert!(!outside.path().join("escaped.txt").exists(), "no write outside it: {result}");
}

/// Arena's chats, as far as a session's first send goes: the chat made, the messages kept.
struct Chats(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);

impl Respond for Chats {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&r.body).unwrap_or_default();
        let n = body["messages"].as_array().map(Vec::len).unwrap_or(0);
        if let Ok(mut seen) = self.0.lock() {
            seen.push(body);
        }
        let before: usize = self
            .0
            .lock()
            .map(|s| s.iter().map(|b| b["messages"].as_array().map(Vec::len).unwrap_or(0)).sum())
            .unwrap_or(0);
        let ids: Vec<String> = (0..n).map(|i| format!("m{}", before - n + i)).collect();
        ResponseTemplate::new(200).set_body_json(json!({"ids": ids, "count": n, "stamp": "s1", "summarized": false}))
    }
}

#[tokio::test]
async fn a_print_run_is_kept_in_a_chat_in_arena() {
    let gateway = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(
            vec![sse(json!({"choices": [{"delta": {"content": "Hello from here."}, "finish_reason": "stop"}]}))],
            AtomicUsize::new(0),
        ))
        .mount(&gateway)
        .await;
    let arena = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "c-1", "created": true})))
        .expect(1)
        .mount(&arena)
        .await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/c-1/messages"))
        .respond_with(Chats(seen.clone()))
        .mount(&arena)
        .await;
    // Every probe answers: this Arena has it all.
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&arena).await;

    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let out = code_arena(home.path())
        .args(["-p", "say hello", "-C"])
        .arg(project.path())
        .env("ARENA_URL", arena.uri())
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let seen = seen.lock().expect("seen");
    let sent: Vec<Value> = seen.iter().flat_map(|b| b["messages"].as_array().cloned().unwrap_or_default()).collect();
    assert_eq!(sent.len(), 2, "{seen:?}\n{stderr}");
    assert_eq!(sent[0]["role"], "user");
    assert_eq!(sent[0]["content"], "say hello");
    assert_eq!(sent[1]["content"], "Hello from here.");
    assert_eq!(sent[1]["model"], "qwen");
}

#[tokio::test]
async fn an_arena_out_of_reach_does_not_hold_a_print_run() {
    let gateway = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(
            vec![sse(json!({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}]}))],
            AtomicUsize::new(0),
        ))
        .mount(&gateway)
        .await;
    // Nothing listens on port 1, and no other test's server can take it (binding it needs root).
    let uri = "http://127.0.0.1:1".to_string();
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let started = std::time::Instant::now();
    let out = code_arena(home.path())
        .args(["-p", "hi", "-C"])
        .arg(project.path())
        .env("ARENA_URL", uri)
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(started.elapsed() < std::time::Duration::from_secs(8), "{:?}", started.elapsed());
}

/// A model that keeps what each request asked.
struct Heard(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);

impl Respond for Heard {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        if let Ok(mut h) = self.0.lock() {
            h.push(serde_json::from_slice(&r.body).unwrap_or_default());
        }
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse(json!({"choices": [{"delta": {"content": "noted"}, "finish_reason": "stop"}]})))
    }
}

#[tokio::test]
async fn what_the_web_added_is_read_at_the_next_turn() {
    let gateway = MockServer::start().await;
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(path("/v1/chat/completions")).respond_with(Heard(heard.clone())).mount(&gateway).await;
    let arena = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "c-1", "created": true})))
        .mount(&arena)
        .await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/c-1/messages"))
        .respond_with(Chats(seen.clone()))
        .mount(&arena)
        .await;
    // After the first run's two messages (m0, m1), the person asked something on the web.
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/c-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "c-1", "title": "t", "count": 3, "stamp": "w", "base": "m1", "fork": false, "more": false,
            "summary": null, "messages": [{"role": "user", "content": "asked on the web meanwhile", "id": "w2"}]
        })))
        .mount(&arena)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/c-1/turn"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"until": "2026-11-26T10:00:00Z"})))
        .mount(&arena)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/code-arena/chats/c-1/turn"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&arena)
        .await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&arena).await;

    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let run = |args: &[&str]| {
        code_arena(home.path())
            .args(args)
            .arg("-C")
            .arg(project.path())
            .env("ARENA_URL", arena.uri())
            .env("ARENA_API_KEY", "sk-test-only")
            .env("ARENA_GATEWAY_URL", gateway.uri())
            .env("ARENA_MODEL", "qwen")
            .output()
            .expect("runs")
    };
    let first = run(&["-p", "first question"]);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let second = run(&["-p", "second question", "--continue"]);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "{stderr}");
    assert!(stderr.contains("From the web: asked on the web meanwhile"), "{stderr}");
    let heard = heard.lock().expect("heard");
    let last = heard.last().expect("a request").to_string();
    let web = last.find("asked on the web meanwhile").expect("the web's message was read");
    let mine = last.find("second question").expect("this run's question");
    assert!(web < mine, "the web's message comes before this run's question");
    // This run's question went after the web's.
    let seen = seen.lock().expect("seen");
    let asked = seen.iter().find(|b| b["messages"][0]["content"] == "second question").expect("the question was sent");
    assert_eq!(asked["parent"], "w2", "{seen:?}");
}

fn call(id: &str, name: &str, args: Value) -> String {
    sse(
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": id, "function": {"name": name, "arguments": args.to_string()}}]}, "finish_reason": "tool_calls"}]}),
    )
}

#[tokio::test]
async fn a_plans_checks_are_enforced_kept_sent_and_carried_on() {
    let gateway = MockServer::start().await;
    let plan = json!({"title": "Tidy", "steps": [
        {"title": "Check the tree", "check": "true", "status": "doing"},
        {"title": "Write the notes", "depends_on": [1]}
    ]});
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(
            vec![
                call("c1", "plan", plan),
                call("c2", "plan", json!({"steps": [{"number": 1, "status": "done"}]})),
                call("c3", "run_shell", json!({"command": "true"})),
                call("c4", "plan", json!({"steps": [{"number": 1, "status": "done"}]})),
                sse(json!({"choices": [{"delta": {"content": "Step 1 is done."}, "finish_reason": "stop"}]})),
                sse(json!({"choices": [{"delta": {"content": "Going on with step 2."}, "finish_reason": "stop"}]})),
            ],
            AtomicUsize::new(0),
        ))
        .mount(&gateway)
        .await;
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let arena = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "c-1", "created": true})))
        .mount(&arena)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/c-1/messages"))
        .respond_with(Chats(heard.clone()))
        .mount(&arena)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/code-arena/chats/c-1/plan"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&arena)
        .await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&arena).await;

    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let run = |args: &[&str]| {
        code_arena(home.path())
            .args(args)
            .arg("-C")
            .arg(project.path())
            .env("ARENA_URL", arena.uri())
            .env("ARENA_API_KEY", "sk-test-only")
            .env("ARENA_GATEWAY_URL", gateway.uri())
            .env("ARENA_MODEL", "qwen")
            .env("CODE_ARENA_MODE", "yolo")
            .output()
            .expect("runs")
    };
    let first = run(&["-p", "tidy up"]);
    let stderr = String::from_utf8_lossy(&first.stderr);
    assert!(first.status.success(), "{stderr}");
    // Refused before its check ran, taken after.
    let requests = gateway.received_requests().await.expect("requests");
    assert!(
        requests.iter().any(|r| String::from_utf8_lossy(&r.body).contains("only once its check passed")),
        "{stderr}"
    );
    assert!(stderr.contains("1/2 done"), "{stderr}");
    let puts: Vec<Value> = arena
        .received_requests()
        .await
        .expect("arena")
        .iter()
        .filter(|r| r.method.as_str() == "PUT")
        .map(|r| serde_json::from_slice(&r.body).expect("json"))
        .collect();
    let last = puts.last().expect("the plan was sent");
    assert_eq!(last["steps"][0]["status"], "done");
    assert_eq!(last["steps"][0]["checkResult"], "passed");

    let second = run(&["-p", "go on", "--plan"]);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "{stderr}");
    assert!(stderr.contains("Carried on: \"Tidy\""), "{stderr}");
    let requests = gateway.received_requests().await.expect("requests");
    let asked = String::from_utf8_lossy(&requests.last().expect("one").body).to_string();
    assert!(asked.contains("carried on here, 1 of 2 steps done"), "{asked}");
}

#[tokio::test]
async fn a_checklist_on_in_the_chat_is_told_and_ticked() {
    let gateway = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(
            vec![
                sse(json!({"choices": [{"delta": {"content": "Started."}, "finish_reason": "stop"}]})),
                call("k1", "checklist_check", json!({"item": 1, "note": "cargo test: 3 passed"})),
                sse(json!({"choices": [{"delta": {"content": "Item 1 is ticked."}, "finish_reason": "stop"}]})),
            ],
            AtomicUsize::new(0),
        ))
        .mount(&gateway)
        .await;
    let arena = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "c-1", "created": true})))
        .mount(&arena)
        .await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/c-1/messages"))
        .respond_with(Chats(seen.clone()))
        .mount(&arena)
        .await;
    let run = json!({"name": "Before a merge", "items": [{"number": 1, "text": "the tests pass", "done": false}], "done": 0, "total": 1});
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/c-1/checklist"))
        .respond_with(ResponseTemplate::new(200).set_body_json(run))
        .mount(&arena)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/c-1/checklist/tick"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "Before a merge",
            "items": [{"number": 1, "text": "the tests pass", "done": true, "note": "cargo test: 3 passed", "by": "model"}],
            "done": 1, "total": 1})))
        .expect(1)
        .mount(&arena)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/c-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"unchanged": true, "stamp": "s"})))
        .mount(&arena)
        .await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&arena).await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({}))).mount(&arena).await;
    Mock::given(method("DELETE")).respond_with(ResponseTemplate::new(204)).mount(&arena).await;

    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let run = |args: &[&str]| {
        code_arena(home.path())
            .args(args)
            .arg("-C")
            .arg(project.path())
            .env("ARENA_URL", arena.uri())
            .env("ARENA_API_KEY", "sk-test-only")
            .env("ARENA_GATEWAY_URL", gateway.uri())
            .env("ARENA_MODEL", "qwen")
            .output()
            .expect("runs")
    };
    let first = run(&["-p", "start"]);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let second = run(&["-p", "go through it", "--continue"]);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "{stderr}");
    assert!(stderr.contains("● checklist_check item 1"), "{stderr}");
    let requests = gateway.received_requests().await.expect("requests");
    let asked = String::from_utf8_lossy(&requests.last().expect("one").body).to_string();
    assert!(asked.contains("The person turned on a checklist"), "{asked}");
    assert!(asked.contains("1. [x] the tests pass (cargo test: 3 passed)"), "{asked}");
}

#[tokio::test]
async fn arenas_memory_starts_the_session_and_what_matters_comes_with_a_message() {
    let gateway = MockServer::start().await;
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(path("/v1/chat/completions")).respond_with(Heard(heard.clone())).mount(&gateway).await;
    let arena = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/code-arena/memories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"on": true, "note": "- The person likes short answers.", "inNote": ["m1"], "memories": []}),
        ))
        .mount(&arena)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/memories/recall"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "note": "(Memories that may matter to this question; use them where they help:\n- Releases are cut from main.)",
            "memories": [{"id": "m2", "text": "Releases are cut from main."}]
        })))
        .mount(&arena)
        .await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&arena).await;
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let out = code_arena(home.path())
        .args(["-p", "how do we release?", "-C"])
        .arg(project.path())
        .env("ARENA_URL", arena.uri())
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let heard = heard.lock().expect("heard");
    let asked = heard.first().expect("asked").to_string();
    assert!(asked.contains("# Memory\\n\\n- The person likes short answers."), "{asked}");
    assert!(asked.contains("Releases are cut from main."), "{asked}");
    let tools: Vec<String> = heard[0]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap_or("").to_string())
        .collect();
    for t in [
        "remember",
        "recall",
        "remember_project",
        "skill_save",
        "search_history",
        "open_chat",
        "plan",
        "checklist_check",
    ] {
        assert!(tools.iter().any(|x| x == t), "{t} in {tools:?}");
    }
}

/// A provider of OpenAI's kind that refuses any field it does not know.
struct StrictProvider(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);

impl Respond for StrictProvider {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&r.body).unwrap_or_default();
        let known = ["model", "messages", "stream", "stream_options", "tools", "max_tokens", "tool_choice"];
        if let Some(extra) = body.as_object().and_then(|o| o.keys().find(|k| !known.contains(&k.as_str()))) {
            return ResponseTemplate::new(422)
                .set_body_json(json!({"error": {"message": format!("unknown field {extra}")}}));
        }
        self.0.lock().expect("seen").push(body.clone());
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse(json!({"choices": [{"delta": {"content": format!("{} answers.", body["model"].as_str().unwrap_or(""))}, "finish_reason": "stop"}]})))
    }
}

#[tokio::test]
async fn the_standalone_edition_answers_with_a_provider_of_ones_own_and_locks_what_needs_arena() {
    let provider = MockServer::start().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(path("/v1/chat/completions")).respond_with(StrictProvider(seen.clone())).mount(&provider).await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/credentials.json"),
        json!({"providers": {"gpu": {"url": provider.uri(), "models": ["qwen3-coder"]}}}).to_string(),
    )
    .expect("credentials");
    let project = tempfile::tempdir().expect("project");
    let run = |args: &[&str]| code_arena(home.path()).args(args).arg("-C").arg(project.path()).output().expect("runs");
    let out = run(&["-p", "hello"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("qwen3-coder answers."), "{stdout}\n{stderr}");
    assert_eq!(seen.lock().expect("seen").len(), 1, "one request, taken as it was");
    // Nothing of Arena's tools in the request.
    let tools = seen.lock().expect("seen")[0]["tools"].to_string();
    assert!(!tools.contains("checklist_check") && !tools.contains("search_history"), "{tools}");
    assert!(tools.contains("\"remember\"") && tools.contains("\"plan\""), "{tools}");
    let v = code_arena(home.path()).arg("version").output().expect("version");
    assert!(String::from_utf8_lossy(&v.stdout).contains(", standalone edition (MIT licence)"));
}

#[test]
fn with_nothing_set_up_both_editions_are_named() {
    let home = tempfile::tempdir().expect("home");
    let out = code_arena(home.path()).args(["-p", "hi"]).output().expect("runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("login --provider NAME") && stderr.contains("managed edition"), "{stderr}");
}

#[tokio::test]
async fn a_session_in_a_project_keeps_its_goals_and_tags_its_requests() {
    let gateway = MockServer::start().await;
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(path("/v1/chat/completions")).respond_with(Heard(heard.clone())).mount(&gateway).await;
    let arena = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "c-1", "created": true})))
        .mount(&arena)
        .await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/c-1/messages"))
        .respond_with(Chats(seen))
        .mount(&arena)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/c-1/project-work"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "project": {"id": "p-7", "name": "Payments"}, "mayChange": true,
            "instructions": "This chat is in the project \"Payments\".", "functions": ["project_plan", "project_goal", "project_task"]
        })))
        .mount(&arena)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/c-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"unchanged": true, "stamp": "s"})))
        .mount(&arena)
        .await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&arena).await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({}))).mount(&arena).await;
    Mock::given(method("DELETE")).respond_with(ResponseTemplate::new(204)).mount(&arena).await;
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let run = |args: &[&str]| {
        code_arena(home.path())
            .args(args)
            .arg("-C")
            .arg(project.path())
            .env("ARENA_URL", arena.uri())
            .env("ARENA_API_KEY", "sk-test-only")
            .env("ARENA_GATEWAY_URL", gateway.uri())
            .env("ARENA_MODEL", "qwen")
            .output()
            .expect("runs")
    };
    assert!(run(&["-p", "start"]).status.success());
    let second = run(&["-p", "go on", "--continue"]);
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    let heard = heard.lock().expect("heard");
    let last = heard.last().expect("asked");
    let tools = last["tools"].to_string();
    assert!(tools.contains("project_goal") && tools.contains("project_task"), "{tools}");
    assert!(last.to_string().contains("in the project \\\"Payments\\\""), "{last}");
    assert_eq!(last["metadata"]["tags"], json!(["chat:c-1", "project:p-7"]));
}
