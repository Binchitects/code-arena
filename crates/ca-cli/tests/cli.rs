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

#[tokio::test]
async fn stream_json_says_each_event_on_a_line_then_the_result() {
    let gateway = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(
            vec![
                call("c1", "read_file", json!({"path": "a.txt"})),
                sse(json!({"choices": [{"delta": {"content": "It says hi."}, "finish_reason": "stop"}]})),
            ],
            AtomicUsize::new(0),
        ))
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    std::fs::write(project.path().join("a.txt"), "hi\n").expect("w");
    let out = code_arena(home.path())
        .args(["-p", "read a.txt", "--output", "stream-json", "-C"])
        .arg(project.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<Value> = stdout.lines().map(|l| serde_json::from_str(l).expect("a JSON line")).collect();
    let kinds: Vec<&str> = lines.iter().map(|l| l["type"].as_str().unwrap_or("")).collect();
    assert_eq!(kinds.first(), Some(&"start"));
    assert!(kinds.contains(&"tool_end"), "{kinds:?}");
    let tool = lines.iter().find(|l| l["type"] == "tool_end").expect("tool");
    assert_eq!(tool["name"], "read_file");
    assert_eq!(kinds.last(), Some(&"result"));
    assert_eq!(lines.last().expect("result")["result"], "It says hi.");
}

#[test]
fn doctor_says_how_to_give_a_ca_file_that_is_gone() {
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/credentials.json"),
        json!({"url": "https://arena.example", "apiKey": "sk-test-only", "ca": home.path().join("gone.crt")})
            .to_string(),
    )
    .expect("credentials");
    let out = code_arena(home.path()).arg("doctor").output().expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("could not be read") && stdout.contains("code-arena login --ca PATH"), "{stdout}");
    assert!(!stdout.contains("sk-test-only"));
}

#[tokio::test]
async fn the_persons_hooks_block_rewrite_add_and_send_the_model_on() {
    let gateway = MockServer::start().await;
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    // The model: a dangerous command, then an answer, then (sent on by the Stop hook) a last answer.
    struct Model(std::sync::Arc<std::sync::Mutex<Vec<Value>>>, AtomicUsize);
    impl Respond for Model {
        fn respond(&self, r: &Request) -> ResponseTemplate {
            self.0.lock().expect("heard").push(serde_json::from_slice(&r.body).unwrap_or_default());
            let ev = match self.1.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "run_shell", "arguments": "{\"command\":\"rm -rf build\"}"}}]}, "finish_reason": "tool_calls"}]})
                }
                1 => json!({"choices": [{"delta": {"content": "Done."}, "finish_reason": "stop"}]}),
                _ => json!({"choices": [{"delta": {"content": "Tests run too."}, "finish_reason": "stop"}]}),
            };
            ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(sse(ev))
        }
    }
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Model(heard.clone(), AtomicUsize::new(0)))
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let marker = project.path().join("stopped-once");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    let stop = format!("test -f {m} && exit 0; touch {m}; echo 'run the tests' >&2; exit 2", m = marker.display());
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"hooks": {
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "echo 'Builds go in build/.'"}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "grep -q 'rm -rf' && { echo 'no rm -rf' >&2; exit 2; } || exit 0"}]}],
            "Stop": [{"hooks": [{"type": "command", "command": stop}]}]
        }})
        .to_string(),
    )
    .expect("settings");
    // A project's own hooks do not run before they are trusted.
    std::fs::create_dir_all(project.path().join(".code-arena")).expect("p");
    let theirs = project.path().join("theirs-ran");
    std::fs::write(
        project.path().join(".code-arena/settings.json"),
        json!({"hooks": {"UserPromptSubmit": [{"hooks": [{"command": format!("touch {}", theirs.display())}]}]}})
            .to_string(),
    )
    .expect("project settings");
    let out = code_arena(home.path())
        .args(["-p", "clean the build", "--mode", "yolo", "-C"])
        .arg(project.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("Tests run too."), "the Stop hook sent the model on: {stdout}\n{stderr}");
    assert!(stderr.contains("hooks that do not run until you trust them"), "{stderr}");
    assert!(!theirs.exists(), "the project's hook ran untrusted");
    let heard = heard.lock().expect("heard");
    let first = heard[0].to_string();
    assert!(first.contains("Builds go in build/."), "UserPromptSubmit's context: {first}");
    let second = heard[1].to_string();
    assert!(second.contains("A hook blocked this call: no rm -rf"), "{second}");
    assert!(heard[2].to_string().contains("the work is not done: run the tests"));
}

#[tokio::test]
async fn a_task_goes_to_a_sub_agent_that_reports_back() {
    let gateway = MockServer::start().await;
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    struct Model(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);
    impl Respond for Model {
        fn respond(&self, r: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&r.body).unwrap_or_default();
            self.0.lock().expect("heard").push(body.clone());
            let text = body.to_string();
            let ev = if text.contains("# You are a sub-agent") {
                json!({"choices": [{"delta": {"content": "No bugs found in a.txt."}, "finish_reason": "stop"}]})
            } else if text.contains("No bugs found") {
                json!({"choices": [{"delta": {"content": "The reviewer found nothing."}, "finish_reason": "stop"}]})
            } else {
                json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "t1", "function": {"name": "task", "arguments": "{\"description\":\"review a.txt\",\"prompt\":\"Review a.txt for bugs.\",\"agent\":\"reviewer\"}"}}]}, "finish_reason": "tool_calls"}]})
            };
            ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(sse(ev))
        }
    }
    Mock::given(path("/v1/chat/completions")).respond_with(Model(heard.clone())).mount(&gateway).await;
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    std::fs::create_dir(project.path().join(".git")).expect("a repository");
    std::fs::write(project.path().join("a.txt"), "x\n").expect("w");
    let out = code_arena(home.path())
        .args(["-p", "have it reviewed", "-C"])
        .arg(project.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("The reviewer found nothing."), "{stdout}\n{stderr}");
    assert!(stderr.contains("reviewer starts: review a.txt") && stderr.contains("reviewer is done"), "{stderr}");
    let heard = heard.lock().expect("heard");
    let sub = heard.iter().find(|b| b.to_string().contains("# You are a sub-agent")).expect("the sub-agent asked");
    let sub_text = sub.to_string();
    assert!(sub_text.contains("Your instructions (reviewer)"), "its own instructions");
    let tools: Vec<&str> =
        sub["tools"].as_array().expect("tools").iter().filter_map(|t| t["function"]["name"].as_str()).collect();
    assert!(!tools.contains(&"task") && !tools.contains(&"plan"), "{tools:?}");
    assert!(sub["messages"].to_string().contains("Review a.txt for bugs."));
    assert!(!sub["messages"].to_string().contains("have it reviewed"), "it does not see the conversation");
}

#[tokio::test]
async fn a_goal_is_worked_on_turn_after_turn_until_its_check_passes() {
    let gateway = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(
            vec![
                sse(json!({"choices": [{"delta": {"content": "Looking at it first."}, "finish_reason": "stop"}]})),
                call("w1", "write_file", json!({"path": "done.txt", "content": "yes\n"})),
                sse(json!({"choices": [{"delta": {"content": "Wrote done.txt."}, "finish_reason": "stop"}]})),
            ],
            AtomicUsize::new(0),
        ))
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let out = code_arena(home.path())
        .args(["-p", "go", "--goal", "make done.txt --check test -f done.txt", "--mode", "yolo", "-C"])
        .arg(project.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stderr.contains("The goal is not met yet (`test -f done.txt` failed)"), "{stderr}");
    assert!(stderr.contains("Goal met: `test -f done.txt` succeeded"), "{stderr}");
    assert!(project.path().join("done.txt").is_file());
    let requests = gateway.received_requests().await.expect("requests");
    let second = requests.iter().filter(|r| r.url.path().ends_with("/chat/completions")).nth(1).expect("a second turn");
    assert!(String::from_utf8_lossy(&second.body).contains("Goal (turn 2): make done.txt"));
}

#[tokio::test]
async fn a_schedule_runs_its_prompt_as_a_session_of_its_own() {
    let gateway = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(
            vec![sse(
                json!({"choices": [{"delta": {"content": "Nightly report: all well."}, "finish_reason": "stop"}]}),
            )],
            AtomicUsize::new(0),
        ))
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let run = |args: &[&str]| {
        code_arena(home.path())
            .args(args)
            .current_dir(project.path())
            .env("HOME", home.path())
            .env("ARENA_URL", "https://arena.example")
            .env("ARENA_API_KEY", "sk-test-only")
            .env("ARENA_GATEWAY_URL", gateway.uri())
            .env("ARENA_MODEL", "qwen")
            .output()
            .expect("runs")
    };
    let add = run(&[
        "schedule", "add", "nightly", "--every", "1d", "--at", "02:00", "--mode", "plan", "report", "on", "the", "repo",
    ]);
    assert!(add.status.success(), "{}", String::from_utf8_lossy(&add.stderr));
    let list = String::from_utf8_lossy(&run(&["schedule", "list"]).stdout).to_string();
    assert!(list.contains("nightly  every 1d at 02:00  plan") && list.contains("report on the repo"), "{list}");
    let ran = run(&["schedule", "run", "nightly"]);
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stdout));
    let log = std::fs::read_to_string(home.path().join("config/schedules/nightly.log")).expect("log");
    assert!(log.contains("Nightly report: all well."), "{log}");
    assert!(String::from_utf8_lossy(&run(&["schedule", "list"]).stdout).contains("(ok;"));
    // Not due again before its interval.
    let due = run(&["schedule", "due"]);
    assert!(String::from_utf8_lossy(&due.stdout).trim().is_empty(), "nothing was due");
    if cfg!(target_os = "linux") {
        let inst = run(&["schedule", "install"]);
        assert!(inst.status.success());
        let timer = std::fs::read_to_string(home.path().join(".config/systemd/user/code-arena-schedules.timer"))
            .expect("timer");
        assert!(timer.contains("OnUnitActiveSec=5min"));
        assert!(run(&["schedule", "uninstall"]).status.success());
        assert!(!home.path().join(".config/systemd/user/code-arena-schedules.timer").exists());
    }
    assert!(run(&["schedule", "remove", "nightly"]).status.success());
}

#[tokio::test]
async fn steps_run_at_once_a_worker_in_its_own_worktree_merged_back() {
    let gateway = MockServer::start().await;
    struct Model;
    impl Respond for Model {
        fn respond(&self, r: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&r.body).unwrap_or_default();
            let text = body.to_string();
            let tools_done =
                body["messages"].as_array().map(|m| m.iter().filter(|x| x["role"] == "tool").count()).unwrap_or(0);
            let ev = if text.contains("# You are a sub-agent") && text.contains("Carry out step 1") {
                json!({"choices": [{"delta": {"content": "Step 1: the README names b.txt."}, "finish_reason": "stop"}]})
            } else if text.contains("# You are a sub-agent") {
                match tools_done {
                    0 => {
                        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "w", "function": {"name": "write_file", "arguments": "{\"path\":\"b.txt\",\"content\":\"made by the worker\\n\"}"}}]}, "finish_reason": "tool_calls"}]})
                    }
                    1 => {
                        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c", "function": {"name": "run_shell", "arguments": "{\"command\":\"test -f b.txt\"}"}}]}, "finish_reason": "tool_calls"}]})
                    }
                    _ => {
                        json!({"choices": [{"delta": {"content": "Wrote b.txt; its check passed."}, "finish_reason": "stop"}]})
                    }
                }
            } else {
                match tools_done {
                    0 => {
                        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "p", "function": {"name": "plan", "arguments": "{\"title\":\"Two\",\"steps\":[{\"title\":\"Read the README\"},{\"title\":\"Make b.txt\",\"files\":[\"b.txt\"],\"check\":\"test -f b.txt\"}]}"}}]}, "finish_reason": "tool_calls"}]})
                    }
                    1 => {
                        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "r", "function": {"name": "run_steps", "arguments": "{\"steps\":[1,2]}"}}]}, "finish_reason": "tool_calls"}]})
                    }
                    _ => json!({"choices": [{"delta": {"content": "Both steps are done."}, "finish_reason": "stop"}]}),
                }
            };
            ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(sse(ev))
        }
    }
    Mock::given(path("/v1/chat/completions")).respond_with(Model).mount(&gateway).await;
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let git = |args: &[&str]| {
        let o = std::process::Command::new("git").args(args).current_dir(project.path()).output().expect("git");
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "T"]);
    std::fs::write(project.path().join("README.md"), "Make b.txt.\n").expect("w");
    git(&["add", "."]);
    git(&["commit", "-qm", "start"]);
    let out = code_arena(home.path())
        .args(["-p", "do the two steps", "--mode", "auto-edit", "-C"])
        .arg(project.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("Both steps are done."), "{stdout}\n{stderr}");
    assert_eq!(std::fs::read_to_string(project.path().join("b.txt")).expect("merged"), "made by the worker\n");
    let reqs = gateway.received_requests().await.expect("requests");
    let told = String::from_utf8_lossy(&reqs.last().expect("last").body).to_string();
    assert!(
        told.contains(
            "Step 2 (Make b.txt): done. Changed in its worktree, its check `test -f b.txt` passed, and merged"
        ),
        "{told}"
    );
    assert!(told.contains("2 of 2 done"), "{told}");
    // No worktree is left behind.
    let wt =
        std::process::Command::new("git").args(["worktree", "list"]).current_dir(project.path()).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&wt.stdout).lines().count(), 1, "{}", String::from_utf8_lossy(&wt.stdout));
}

#[tokio::test]
async fn a_plugin_installed_and_enabled_brings_its_skill_hook_and_server() {
    let gateway = MockServer::start().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    struct Heard(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);
    impl Respond for Heard {
        fn respond(&self, r: &Request) -> ResponseTemplate {
            self.0.lock().expect("lock").push(serde_json::from_slice(&r.body).unwrap_or_default());
            let ev = json!({"choices": [{"delta": {"content": "Hi."}, "finish_reason": "stop"}]});
            ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(sse(ev))
        }
    }
    Mock::given(path("/v1/chat/completions")).respond_with(Heard(seen.clone())).mount(&gateway).await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "compat.agents": ["other"]}).to_string(),
    )
    .expect("settings");
    // A plugin in another agent's layout (its manifest in .other-plugin/), read once compat.agents names it.
    let src = tempfile::tempdir().expect("src");
    let p = src.path();
    for d in [".other-plugin", "skills/greet", "hooks"] {
        std::fs::create_dir_all(p.join(d)).expect("dir");
    }
    std::fs::write(
        p.join(".other-plugin/plugin.json"),
        r#"{"name": "kit", "version": "1.2.0", "description": "A kit."}"#,
    )
    .expect("manifest");
    std::fs::write(
        p.join("skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: Greets in the kit's way.\n---\nSay hi.\n",
    )
    .expect("skill");
    std::fs::write(p.join("note.txt"), "The kit's note.").expect("note");
    std::fs::write(
        p.join("hooks/hooks.json"),
        json!({"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "cat ${OTHER_PLUGIN_ROOT}/note.txt"}]}]}})
            .to_string(),
    )
    .expect("hooks");
    std::fs::write(
        p.join(".mcp.json"),
        json!({"mcpServers": {"echo": {"command": "sh", "args": ["-c", ECHO_SERVER]}}}).to_string(),
    )
    .expect("mcp");
    let run = |args: &[&str]| {
        let out = code_arena(home.path()).args(args).output().expect("runs");
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let said = run(&["plugin", "install", &p.display().to_string()]);
    assert!(said.contains("Installed kit (skills, hooks, MCP servers)"), "{said}");
    assert!(run(&["plugin", "list"]).contains("○ kit 1.2.0 A kit."));
    // Installed, not enabled: nothing of it is used.
    let folder = tempfile::tempdir().expect("folder");
    let ask = |seen: &std::sync::Arc<std::sync::Mutex<Vec<Value>>>| {
        let out = code_arena(home.path())
            .args(["-p", "hello", "--mode", "yolo", "-C"])
            .arg(folder.path())
            .env("ARENA_URL", "https://arena.example")
            .env("ARENA_API_KEY", "sk-test-only")
            .env("ARENA_GATEWAY_URL", gateway.uri())
            .env("ARENA_MODEL", "qwen")
            .output()
            .expect("runs");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        seen.lock().expect("lock").last().expect("a request").to_string()
    };
    let before = ask(&seen);
    assert!(!before.contains("Greets in the kit's way") && !before.contains("The kit's note."), "{before}");
    assert!(run(&["plugin", "enable", "kit"]).contains("kit is enabled"));
    assert!(run(&["plugin", "list"]).contains("● kit"));
    let after = ask(&seen);
    assert!(after.contains("Greets in the kit's way"), "the plugin's skill: {after}");
    assert!(after.contains("The kit's note."), "the plugin's hook, its folder put in: {after}");
    assert!(after.contains("kit-echo") && after.contains("shout"), "the plugin's server: {after}");
    run(&["plugin", "remove", "kit"]);
    assert!(run(&["plugin", "list"]).contains("No plugins yet"));
    assert!(!std::fs::read_to_string(home.path().join("config/settings.json")).expect("s").contains("\"kit\""));
}

/// A fake Arena 8.1 with the person's agents and scheduled tasks.
async fn arena_with_automation() -> MockServer {
    let arena = MockServer::start().await;
    Mock::given(path("/api/code-arena/capabilities"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"features": ["automation.agents", "automation.scheduled"]})),
        )
        .mount(&arena)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/code-arena/agents"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "a1", "name": "Reviewer", "description": "Reviews an MR.", "enabled": true, "mine": true, "trigger": "form"},
            {"id": "a2", "name": "Release notes", "enabled": true, "mine": false, "owner": "Sara", "trigger": "manual"}
        ])))
        .mount(&arena)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/agents/a1/run"))
        .and(wiremock::matchers::body_json(json!({"text": "look closely", "fields": {"mr": "12"}})))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"id": "r1"})))
        .mount(&arena)
        .await;
    Mock::given(path("/api/code-arena/agents/runs/r1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "r1", "workflow": "Reviewer", "status": "done",
            "steps": [{"kind": "llm", "name": "Read", "status": "done", "output": "MR 12 looks right."}]
        })))
        .mount(&arena)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/code-arena/scheduled"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "t1", "name": "Morning summary", "cron": "0 9 * * *", "timeZone": "Asia/Tehran", "enabled": true,
             "running": false, "nextRuns": ["2026-11-27T05:30:00Z"], "lastRun": {"id": "x", "status": "done"}, "trigger": "schedule"}
        ])))
        .mount(&arena)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/scheduled/t1/run"))
        .respond_with(
            ResponseTemplate::new(409)
                .set_body_json(json!({"status": "running", "error": "It is running now; wait for this run to end."})),
        )
        .mount(&arena)
        .await;
    Mock::given(path("/api/code-arena/scheduled/t1/runs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "x", "startedAt": "2026-11-26T05:30:00Z", "status": "failed", "error": "The model was out of reach.", "manual": false}
        ])))
        .mount(&arena)
        .await;
    arena
}

#[tokio::test]
async fn arenas_agents_and_scheduled_tasks_are_listed_and_run_from_the_shell() {
    let arena = arena_with_automation().await;
    let home = tempfile::tempdir().expect("home");
    let run = |args: &[&str]| {
        let out = code_arena(home.path())
            .args(args)
            .env("ARENA_URL", arena.uri())
            .env("ARENA_API_KEY", "sk-test-only")
            .output()
            .expect("runs");
        (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
    };
    let (code, said) = run(&["agents"]);
    assert_eq!(code, Some(0), "{said}");
    assert!(
        said.contains("1. Reviewer: Reviews an MR.") && said.contains("2. Release notes (shared by Sara)"),
        "{said}"
    );
    let (code, said) = run(&["agents", "run", "review", "look", "closely", "--field", "mr=12"]);
    assert_eq!(code, Some(0), "{said}");
    assert!(said.contains("Reviewer is done: MR 12 looks right."), "{said}");
    let (code, said) = run(&["tasks"]);
    assert_eq!(code, Some(0), "{said}");
    assert!(said.contains("Morning summary (0 9 * * * Asia/Tehran, last done, next 2026-11-27T05:30:00Z)"), "{said}");
    let (_, said) = run(&["tasks", "run", "1"]);
    assert!(said.contains("wait for this run to end"), "{said}");
    let (_, said) = run(&["tasks", "runs", "morning"]);
    assert!(said.contains("failed: The model was out of reach."), "{said}");
}

#[tokio::test]
async fn an_8_0_arena_says_it_has_no_agents_for_code_arena() {
    let arena = MockServer::start().await;
    let home = tempfile::tempdir().expect("home");
    let out = code_arena(home.path())
        .args(["agents"])
        .env("ARENA_URL", arena.uri())
        .env("ARENA_API_KEY", "sk-test-only")
        .output()
        .expect("runs");
    let said = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(said.contains("does not give Code Arena its Agent Studio's agents (Arena 8.1 on does)"), "{said}");
}

#[tokio::test]
async fn the_model_runs_one_of_the_persons_arena_agents_and_reads_its_answer() {
    let arena = arena_with_automation().await;
    let gateway = MockServer::start().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    struct Model(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);
    impl Respond for Model {
        fn respond(&self, r: &Request) -> ResponseTemplate {
            let mut s = self.0.lock().expect("lock");
            s.push(serde_json::from_slice(&r.body).unwrap_or_default());
            let ev = if s.len() == 1 {
                json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "run_arena_agent", "arguments": "{\"agent\":\"Reviewer\",\"text\":\"look closely\",\"fields\":{\"mr\":\"12\"}}"}}]}, "finish_reason": "tool_calls"}]})
            } else {
                json!({"choices": [{"delta": {"content": "The reviewer agrees."}, "finish_reason": "stop"}]})
            };
            ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(sse(ev))
        }
    }
    Mock::given(path("/v1/chat/completions")).respond_with(Model(seen.clone())).mount(&gateway).await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    // What the Arena has is asked once and kept (doctor, or any command that needs it, does that).
    assert!(
        code_arena(home.path())
            .args(["agents"])
            .env("ARENA_URL", arena.uri())
            .env("ARENA_API_KEY", "sk-test-only")
            .status()
            .expect("runs")
            .success()
    );
    let folder = tempfile::tempdir().expect("folder");
    let out = code_arena(home.path())
        .args(["-p", "ask the reviewer about MR 12", "--mode", "yolo", "-C"])
        .arg(folder.path())
        .env("ARENA_URL", arena.uri())
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("The reviewer agrees."), "{stdout}");
    let seen = seen.lock().expect("lock");
    let tools: Vec<&str> =
        seen[0]["tools"].as_array().expect("tools").iter().filter_map(|t| t["function"]["name"].as_str()).collect();
    assert!(tools.contains(&"arena_agents") && tools.contains(&"run_arena_agent"), "{tools:?}");
    assert!(seen[1].to_string().contains("Reviewer is done: MR 12 looks right."), "{}", seen[1]);
}

#[tokio::test]
async fn a_fork_carries_on_a_session_that_stays_as_it_was() {
    let gateway = MockServer::start().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    struct Heard(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);
    impl Respond for Heard {
        fn respond(&self, r: &Request) -> ResponseTemplate {
            self.0.lock().expect("lock").push(serde_json::from_slice(&r.body).unwrap_or_default());
            let ev = json!({"choices": [{"delta": {"content": "Noted."}, "finish_reason": "stop"}]});
            ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(sse(ev))
        }
    }
    Mock::given(path("/v1/chat/completions")).respond_with(Heard(seen.clone())).mount(&gateway).await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    let ask = |args: &[&str]| {
        let out = code_arena(home.path())
            .args(args)
            .args(["-C"])
            .arg(folder.path())
            .env("ARENA_URL", "https://arena.example")
            .env("ARENA_API_KEY", "sk-test-only")
            .env("ARENA_GATEWAY_URL", gateway.uri())
            .env("ARENA_MODEL", "qwen")
            .output()
            .expect("runs");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    let last = |seen: &std::sync::Arc<std::sync::Mutex<Vec<Value>>>| {
        seen.lock().expect("lock").last().expect("a request")["messages"].to_string()
    };
    ask(&["-p", "the first way"]);
    let said = ask(&["-p", "zebra branch", "--continue", "--fork-session"]);
    let (from, fork) = said
        .split("A fork of ")
        .nth(1)
        .and_then(|t| t.split_once(": "))
        .map(|(a, b)| (a.to_string(), b.split('.').next().unwrap_or("").to_string()))
        .unwrap_or_else(|| panic!("the fork is said: {said}"));
    assert_ne!(from, fork);
    let in_fork = last(&seen);
    assert!(in_fork.contains("the first way") && in_fork.contains("zebra branch"), "{in_fork}");
    // The session it came from has only its own.
    ask(&["-p", "back on the first", "--resume", &from]);
    let back = last(&seen);
    assert!(back.contains("the first way") && !back.contains("zebra branch"), "{back}");
}

#[tokio::test]
async fn a_projects_output_style_takes_the_place_of_the_coding_instructions() {
    let gateway = MockServer::start().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    struct Heard(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);
    impl Respond for Heard {
        fn respond(&self, r: &Request) -> ResponseTemplate {
            self.0.lock().expect("lock").push(serde_json::from_slice(&r.body).unwrap_or_default());
            let ev = json!({"choices": [{"delta": {"content": "Done."}, "finish_reason": "stop"}]});
            ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(sse(ev))
        }
    }
    Mock::given(path("/v1/chat/completions")).respond_with(Heard(seen.clone())).mount(&gateway).await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    let project = tempfile::tempdir().expect("project");
    std::fs::create_dir_all(project.path().join(".code-arena/output-styles")).expect("styles");
    std::fs::write(
        project.path().join(".code-arena/output-styles/reviewer.md"),
        "---\nname: Reviewer\ndescription: Reviews, never changes.\n---\nYou review writing: say what is unclear, change nothing.\n",
    )
    .expect("style");
    std::fs::write(project.path().join(".code-arena/settings.json"), json!({"outputStyle": "reviewer"}).to_string())
        .expect("project settings");
    let run = |style_dir: &std::path::Path| {
        let out = code_arena(home.path())
            .args(["-p", "look at the README", "-C"])
            .arg(style_dir)
            .env("ARENA_URL", "https://arena.example")
            .env("ARENA_API_KEY", "sk-test-only")
            .env("ARENA_GATEWAY_URL", gateway.uri())
            .env("ARENA_MODEL", "qwen")
            .output()
            .expect("runs");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    run(project.path());
    let system = seen.lock().expect("lock")[0]["messages"][0]["content"].as_str().unwrap_or_default().to_string();
    assert!(system.contains("# Output style: Reviewer\nYou review writing"), "{system}");
    assert!(!system.contains("Read before you change"), "{system}");
    // A style that is not there: said, and the default used.
    std::fs::write(project.path().join(".code-arena/settings.json"), json!({"outputStyle": "gone"}).to_string())
        .expect("project settings");
    let said = run(project.path());
    assert!(said.contains("There is no output style \"gone\""), "{said}");
    let system = seen.lock().expect("lock")[1]["messages"][0]["content"].as_str().unwrap_or_default().to_string();
    assert!(system.contains("Read before you change") && !system.contains("Output style"), "{system}");
}

#[tokio::test]
async fn a_full_shared_cache_is_asked_again_and_the_engines_window_is_learned_and_kept() {
    let gateway = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "qwen"}]})))
        .mount(&gateway)
        .await;
    Mock::given(path("/v1/model/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [
            {"model_name": "qwen", "model_info": {"max_input_tokens": 32768, "max_output_tokens": 4096}}
        ]})))
        .mount(&gateway)
        .await;
    struct Engine(AtomicUsize);
    impl Respond for Engine {
        fn respond(&self, _r: &Request) -> ResponseTemplate {
            match self.0.fetch_add(1, Ordering::SeqCst) {
                // Others filled the cache the engine's requests share.
                0 => ResponseTemplate::new(500)
                    .set_body_json(json!({"error": {"code": 500, "message": "Context size has been exceeded.", "type": "server_error"}})),
                // The engine holds less than the gateway says.
                1 => ResponseTemplate::new(400).set_body_json(json!({"error": {"code": 400,
                    "message": "request (25000 tokens) exceeds the available context size (20480 tokens), try increasing it",
                    "type": "exceed_context_size_error", "n_prompt_tokens": 25000, "n_ctx": 20480}})),
                _ => ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(sse(
                    json!({"choices": [{"delta": {"content": "Fits now."}, "finish_reason": "stop"}]}),
                )),
            }
        }
    }
    Mock::given(path("/v1/chat/completions")).respond_with(Engine(AtomicUsize::new(0))).mount(&gateway).await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    let out = code_arena(home.path())
        .args(["-p", "hello", "-C"])
        .arg(folder.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .output()
        .expect("runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("Fits now."));
    assert!(stderr.contains("shared by the requests it runs, was full: asking again in two seconds"), "{stderr}");
    assert!(stderr.contains("The engine holds 20.5k tokens, not the 32.8k the gateway says"), "{stderr}");
    let kept: Value =
        serde_json::from_str(&std::fs::read_to_string(home.path().join("data/engine-windows.json")).expect("kept"))
            .expect("json");
    let entry = kept.as_object().and_then(|o| o.values().next()).cloned().unwrap_or_default();
    assert_eq!((entry["said"].as_u64(), entry["holds"].as_u64()), (Some(32768), Some(20480)), "{kept}");
}

#[tokio::test]
async fn the_ide_streams_a_turn_to_its_page_and_refuses_others() {
    use futures_util::{SinkExt, StreamExt};
    use std::io::BufRead;
    use tokio_tungstenite::tungstenite::Message;
    let gateway = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(
            vec![format!(
                "data: {}\n\n{}",
                json!({"choices": [{"delta": {"reasoning_content": "Say it kindly."}}]}),
                sse(json!({"choices": [{"delta": {"content": "Hello from the IDE."}, "finish_reason": "stop"}],
                           "usage": {"prompt_tokens": 120, "completion_tokens": 7}}))
            )],
            AtomicUsize::new(0),
        ))
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    let mut child = code_arena(home.path())
        .args(["ide", "--no-open", "-C"])
        .arg(folder.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("runs");
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let stderr = child.stderr.take().expect("stderr");
    let _child = Kill(child);
    let url = std::io::BufReader::new(stderr)
        .lines()
        .map_while(Result::ok)
        .find_map(|l| l.split("The IDE: ").nth(1).map(str::to_string))
        .expect("the address");
    let (base, key) = url.split_once("/?k=").expect("key in it");
    let host = base.trim_start_matches("http://").to_string();
    // The address file is the person's only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.path().join("data/ide.json")).expect("kept").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let ws = |origin: &str| {
        let mut r = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
            format!("ws://{host}/ws").as_str(),
        )
        .expect("request");
        r.headers_mut().insert("cookie", format!("code_arena={key}").parse().expect("h"));
        r.headers_mut().insert("origin", origin.parse().expect("h"));
        r
    };
    assert!(tokio_tungstenite::connect_async(ws("https://evil.example")).await.is_err(), "another site's page");
    let (mut page, _) = tokio_tungstenite::connect_async(ws(base)).await.expect("the page connects");
    let mut seen: Vec<Value> = Vec::new();
    let mut next = async || -> Value {
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(20), page.next())
                .await
                .expect("in time")
                .expect("open")
                .expect("ok");
            if let Message::Text(t) = m {
                return serde_json::from_str(t.as_str()).expect("json");
            }
        }
    };
    let first = next().await;
    assert_eq!(
        (first["type"].as_str(), first["model"].as_str(), first["mode"].as_str()),
        (Some("state"), Some("qwen"), Some("ask"))
    );
    drop(next);
    page.send(Message::text(json!({"type": "send", "text": "say hello"}).to_string())).await.expect("send");
    loop {
        let m = tokio::time::timeout(std::time::Duration::from_secs(20), page.next())
            .await
            .expect("in time")
            .expect("open")
            .expect("ok");
        let Message::Text(t) = m else { continue };
        let v: Value = serde_json::from_str(t.as_str()).expect("json");
        let end = v["type"] == "turn_end";
        seen.push(v);
        if end {
            break;
        }
    }
    // The session's tasks come on connecting and after each turn; the turn's own messages in their order.
    assert!(seen.iter().any(|v| v["type"] == "tasks" && v["jobs"].is_array()), "{seen:?}");
    let kinds: Vec<&str> = seen.iter().filter_map(|v| v["type"].as_str()).filter(|k| *k != "tasks").collect();
    assert!(kinds.starts_with(&["user", "turn_start"]), "{kinds:?}");
    let text: String = seen.iter().filter(|v| v["type"] == "text").filter_map(|v| v["delta"].as_str()).collect();
    assert_eq!(text, "Hello from the IDE.");
    assert_eq!(seen.last().expect("end")["ok"], true, "{seen:?}");
    assert_eq!(seen.iter().find(|v| v["type"] == "user").expect("user")["q"], 1);
    // The thinking gives way to the words: how long it took is said between them; the end says the model and what
    // the turn used.
    let at = |k: &str| kinds.iter().position(|x| *x == k).unwrap_or(usize::MAX);
    assert!(at("reasoning") < at("thought") && at("thought") < at("text"), "{kinds:?}");
    assert_eq!(seen.iter().find(|v| v["type"] == "thought").expect("thought")["cut_short"], false);
    let end = seen.last().expect("end");
    assert_eq!((end["model"].as_str(), end["usage"]["prompt"].as_u64()), (Some("qwen"), Some(120)), "{end}");
    assert_eq!(end["usage"]["model"], "qwen");
    // What the page asks between turns, each answered by its kind.
    let until = async |page: &mut tokio_tungstenite::WebSocketStream<_>, kind: &str| -> Value {
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(20), page.next())
                .await
                .expect("in time")
                .expect("open")
                .expect("ok");
            let Message::Text(t) = m else { continue };
            let v: Value = serde_json::from_str(t.as_str()).expect("json");
            if v["type"] == kind {
                return v;
            }
        }
    };
    page.send(Message::text(json!({"type": "context"}).to_string())).await.expect("send");
    let d = until(&mut page, "context_detail").await;
    assert!(
        d["kinds"]
            .as_array()
            .is_some_and(|k| k.iter().any(|x| x["kind"] == "Instructions" && x["tokens"].as_u64() > Some(0))),
        "{d}"
    );
    assert!(d["context"]["size"].as_u64() > Some(0), "{d}");
    // Each part's pieces, the room kept for the answer and what is free, adding up to the window.
    let tools = d["kinds"].as_array().and_then(|k| k.iter().find(|x| x["kind"] == "Tools")).expect("tools");
    assert!(tools["pieces"].as_array().is_some_and(|p| p.iter().any(|x| x["name"] == "read_file")), "{tools}");
    let (used, reserve, free) = (d["context"]["used"].as_u64(), d["reserve"].as_u64(), d["free"].as_u64());
    assert_eq!(used.zip(reserve).zip(free).map(|((u, r), f)| u + r + f), d["context"]["size"].as_u64(), "{d}");
    assert_eq!(d["counted"], "estimate");
    // The step said how full the window was as the model counted it.
    let step = seen.iter().find(|v| v["type"] == "step_end").expect("step_end");
    assert!(step["context"]["used"].as_u64() >= Some(120), "{step}");
    // The threshold changed from the window: live and kept; one out of range is refused with what it asked.
    page.send(Message::text(json!({"type": "compact_at", "at": 70, "target": 20}).to_string())).await.expect("send");
    let set = until(&mut page, "compact_set").await;
    assert_eq!(
        (set["at"].as_u64(), set["target"].as_u64(), set["source"].as_str()),
        (Some(70), Some(20), Some("settings"))
    );
    assert_eq!(set["context"]["compact_at"], 70, "{set}");
    page.send(Message::text(json!({"type": "compact_at", "at": 19, "target": 5}).to_string())).await.expect("send");
    let no = until(&mut page, "compact_refused").await;
    assert_eq!(
        (no["at"].as_u64(), no["error"].as_str()),
        (Some(19), Some("The threshold is from 20% to 95% of the window."))
    );
    // The answer as the session kept it: its thinking, and what its turn used, for its footer after a reload.
    page.send(Message::text(json!({"type": "state"}).to_string())).await.expect("send");
    let st = until(&mut page, "state").await;
    let answer = st["messages"].as_array().and_then(|l| l.iter().find(|m| m["role"] == "assistant")).cloned();
    let answer = answer.expect("the answer kept");
    assert_eq!(answer["reasoning"], "Say it kindly.", "{answer}");
    assert_eq!((answer["usage"]["prompt"].as_u64(), answer["usage"]["model"].as_str()), (Some(120), Some("qwen")));
    page.send(Message::text(json!({"type": "model", "model": "qwen-large"}).to_string())).await.expect("send");
    assert_eq!(until(&mut page, "status").await["model"], "qwen-large");
    page.send(Message::text(json!({"type": "thinking", "level": "high"}).to_string())).await.expect("send");
    assert_eq!(until(&mut page, "status").await["thinking"], "high");
    // Back to before the first question: the thread is empty again.
    page.send(Message::text(json!({"type": "rewind", "q": 1, "what": "chat"}).to_string())).await.expect("send");
    let st = until(&mut page, "state").await;
    assert_eq!(st["messages"].as_array().map(Vec::len), Some(0), "{st}");
    assert!(
        until(&mut page, "notice").await["text"].as_str().is_some_and(|t| t.starts_with("Back to before: say hello"))
    );
    // The workbench's look is kept for the folder and comes back with the state.
    assert_eq!(st["view_state"], Value::Null, "nothing kept yet");
    let look = json!({"layout": {"primary": ["files"], "panel": ["terminal", "search"]}, "sizes": {"primary": 300.0}});
    page.send(Message::text(json!({"type": "keep_view_state", "state": look}).to_string())).await.expect("send");
    page.send(Message::text(json!({"type": "state"}).to_string())).await.expect("send");
    assert_eq!(until(&mut page, "state").await["view_state"], look);
}

/// What holds a model's first answer back until the test lets it go.
type Hold = std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>;

/// A model whose first answer waits until it is let go (a turn that is still running while more is asked), the
/// rest at once. The fake gateway answers on a thread of its own, so waiting here holds nothing else.
struct HeldFirst(Vec<String>, AtomicUsize, Hold);

impl Respond for HeldFirst {
    fn respond(&self, _r: &Request) -> ResponseTemplate {
        let n = self.1.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            let (go, told) = &*self.2;
            let waiting = go.lock().expect("lock");
            // Let go after a while whatever happens, so a test that fails does not hang.
            drop(told.wait_timeout_while(waiting, std::time::Duration::from_secs(15), |go| !*go));
        }
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(self.0[n.min(self.0.len() - 1)].clone())
    }
}

#[tokio::test]
async fn what_the_ide_is_asked_while_it_answers_is_answered_kept_for_the_turns_end_or_refused() {
    use futures_util::{SinkExt, StreamExt};
    use std::io::BufRead;
    use tokio_tungstenite::tungstenite::Message;
    let said = |text: &str| sse(json!({"choices": [{"delta": {"content": text}, "finish_reason": "stop"}]}));
    let gateway = MockServer::start().await;
    let hold = Hold::default();
    Mock::given(path("/v1/chat/completions"))
        .respond_with(HeldFirst(
            vec![said("The first answer."), said("The second answer."), said("The third answer.")],
            AtomicUsize::new(0),
            hold.clone(),
        ))
        .mount(&gateway)
        .await;
    // The models the pickers offer: one the gateway describes, one it does not.
    Mock::given(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "qwen"}, {"id": "small"}]})))
        .mount(&gateway)
        .await;
    Mock::given(path("/v1/model/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"model_name": "small",
            "model_info": {"max_input_tokens": 32768, "max_output_tokens": 4096, "supports_reasoning": false}}]})))
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    // A repository, so that it gets the project's default commands (`/explain`).
    std::fs::create_dir(folder.path().join(".git")).expect("a repository");
    let mut child = code_arena(home.path())
        .args(["ide", "--no-open", "-C"])
        .arg(folder.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("runs");
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let stderr = child.stderr.take().expect("stderr");
    let _child = Kill(child);
    let url = std::io::BufReader::new(stderr)
        .lines()
        .map_while(Result::ok)
        .find_map(|l| l.split("The IDE: ").nth(1).map(str::to_string))
        .expect("the address");
    let (base, key) = url.split_once("/?k=").expect("key in it");
    let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{}/ws", base.trim_start_matches("http://")).as_str(),
    )
    .expect("request");
    request.headers_mut().insert("cookie", format!("code_arena={key}").parse().expect("h"));
    request.headers_mut().insert("origin", base.parse().expect("h"));
    let (mut page, _) = tokio_tungstenite::connect_async(request).await.expect("the page connects");
    // Every message up to (and with) the first for which `end` holds.
    let until = async |page: &mut tokio_tungstenite::WebSocketStream<_>, end: &dyn Fn(&Value) -> bool| -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(20), page.next())
                .await
                .expect("in time")
                .expect("open")
                .expect("ok");
            let Message::Text(t) = m else { continue };
            let v: Value = serde_json::from_str(t.as_str()).expect("json");
            let done = end(&v);
            seen.push(v);
            if done {
                return seen;
            }
        }
    };
    let ask = async |page: &mut tokio_tungstenite::WebSocketStream<_>, m: Value| {
        page.send(Message::text(m.to_string())).await.expect("send");
    };
    // The session as it starts: the About box's fields with it, nothing locked with an Arena signed in to.
    let first = until(&mut page, &|v| v["type"] == "state").await.pop().expect("state");
    assert_eq!((first["license"].as_str(), first["source"].as_str()), (Some("MIT"), Some(ca_core::SOURCE)), "{first}");
    assert!(first["license_text"].as_str().is_some_and(|t| t.starts_with("MIT License")), "{first}");
    assert_eq!(first["manual"], "https://arena.example/help/code-arena");
    assert_eq!((&first["locked"], &first["log"]), (&json!({}), &Value::Null), "{first}");
    // What the pickers offer: the model as the gateway describes it, the levels of thinking, the modes.
    assert_eq!(first["models"], json!(["qwen", "small"]), "{first}");
    assert_eq!(
        first["model_info"],
        json!([{"id": "qwen", "tools": true, "thinking": true, "vision": false},
            {"id": "small", "context": 32768, "max_output": 4096, "tools": true, "thinking": false, "vision": false}])
    );
    assert_eq!(first["thinking_levels"][0], "default");
    let modes: Vec<&str> =
        first["modes"].as_array().expect("modes").iter().filter_map(|m| m["name"].as_str()).collect();
    assert_eq!(modes, ["ask", "auto-edit", "plan", "yolo"]);
    assert!(first["modes"][3]["description"].as_str().is_some_and(|d| !d.is_empty()));

    ask(&mut page, json!({"type": "send", "text": "say hello"})).await;
    until(&mut page, &|v| v["type"] == "turn_start").await;
    // A face that comes now (a page reloaded): the thread before the turn, then the turn so far said again.
    ask(&mut page, json!({"type": "hello"})).await;
    let st = until(&mut page, &|v| v["type"] == "state").await.pop().expect("state");
    assert_eq!(
        (&st["running"], &st["messages"], &st["earlier"], &st["first_turn"]),
        (&json!(true), &json!([]), &json!(0), &json!(1))
    );
    let again = until(&mut page, &|v| v["type"] == "tasks").await;
    let kinds: Vec<&str> = again.iter().filter(|v| v["replay"] == true).filter_map(|v| v["type"].as_str()).collect();
    assert_eq!(kinds, ["user", "turn_start"], "{again:?}");
    assert_eq!(again[0]["text"], "say hello");
    // While the model writes its answer: a change of model, a step back, a setting, the themes, a command,
    // another message.
    ask(&mut page, json!({"type": "model", "model": "qwen-large"})).await;
    ask(&mut page, json!({"type": "rewind", "q": 1, "what": "chat"})).await;
    ask(&mut page, json!({"type": "thinking", "level": "high"})).await;
    ask(&mut page, json!({"type": "mode", "mode": "auto-edit"})).await;
    ask(&mut page, json!({"type": "set_setting", "key": "workbench.colorTheme", "value": "Light Modern"})).await;
    ask(&mut page, json!({"type": "themes"})).await;
    ask(&mut page, json!({"type": "send", "text": "/explain the parser"})).await;
    ask(&mut page, json!({"type": "send", "text": "and then?"})).await;
    // One more, taken back before it is read: the faces are told what waits now.
    ask(&mut page, json!({"type": "send", "text": "never mind"})).await;
    ask(&mut page, json!({"type": "queue_cancel", "id": 3})).await;
    ask(&mut page, json!({"type": "queue_edit", "id": 3, "text": "too late"})).await;
    // All of it was taken while the model had not answered yet; only then does its answer come.
    let mut meanwhile = until(&mut page, &|v| v["type"] == "queue_error").await;
    let left = meanwhile.iter().find(|v| v["type"] == "queue").expect("what waits");
    assert_eq!(
        left,
        &json!({"type": "queue", "id": 3, "items": [
            {"id": 1, "text": "/explain the parser", "later": true},
            {"id": 2, "text": "and then?", "later": false}
        ]})
    );
    assert_eq!(meanwhile.last(), Some(&json!({"type": "queue_error", "id": 3})), "taken back already");
    *hold.0.lock().expect("lock") = true;
    hold.1.notify_all();
    meanwhile.extend(until(&mut page, &|v| v["type"] == "turn_end").await);
    let at = |all: &[Value], want: &dyn Fn(&Value) -> bool| all.iter().position(want);
    assert_eq!(meanwhile.last().expect("end")["ok"], true, "{meanwhile:?}");
    // Kept for the turn's end, each said so at once; the step back refused, with why.
    for (of, key) in [
        ("model", Value::Null),
        ("thinking", Value::Null),
        ("mode", Value::Null),
        ("set_setting", json!("workbench.colorTheme")),
    ] {
        assert!(
            at(&meanwhile, &|v| v["type"] == "deferred" && v["of"] == of && v["key"] == key).is_some(),
            "{of}: {meanwhile:?}"
        );
    }
    let why = "Wait for the answer, or stop it, to go back to an earlier question.";
    assert!(at(&meanwhile, &|v| v["type"] == "notice" && v["text"] == why).is_some(), "{meanwhile:?}");
    let refused = &meanwhile[at(&meanwhile, &|v| v["type"] == "refused").expect("refused")];
    assert_eq!(refused, &json!({"type": "refused", "of": "rewind", "reason": why, "q": 1}));
    // What needs no session is answered while the turn runs, and nothing kept is answered before it ends.
    assert!(at(&meanwhile, &|v| v["type"] == "themes" && v["items"].is_array()).is_some(), "{meanwhile:?}");
    // A command is not words for the running turn to read: it waits for the turn's end, shown as waiting.
    let command = json!({"type": "queued", "text": "/explain the parser", "later": true, "id": 1});
    assert!(at(&meanwhile, &|v| *v == command).is_some(), "{meanwhile:?}");
    assert!(at(&meanwhile, &|v| v["type"] == "deferred" && v["of"] == "send").is_some(), "{meanwhile:?}");
    assert!(
        at(&meanwhile, &|v| *v == json!({"type": "queued", "text": "and then?", "id": 2})).is_some(),
        "{meanwhile:?}"
    );
    assert!(at(&meanwhile, &|v| v["type"] == "status" || v["type"] == "setting_kept").is_none(), "{meanwhile:?}");
    assert!(at(&meanwhile, &|v| v["type"] == "state").is_none(), "nothing went back: {meanwhile:?}");

    // The turn ended: what waited is answered in the order asked, the command among it as a turn of its own (its
    // question marked late: the thread showed it as queued); then the message that came too late for the first
    // turn to read runs as a turn of its own, said so too.
    let after = until(&mut page, &|v| v["type"] == "turn_end").await;
    let model = at(&after, &|v| v["type"] == "status" && v["model"] == "qwen-large" && v["thinking"] == "default")
        .expect("the model, first");
    let thinking = at(&after, &|v| v["type"] == "status" && v["thinking"] == "high" && v["mode"] == "ask")
        .expect("then how much it thinks");
    let mode = at(&after, &|v| v["type"] == "status" && v["thinking"] == "high" && v["mode"] == "auto-edit")
        .expect("then the mode");
    let kept = at(&after, &|v| v["type"] == "setting_kept" && v["key"] == "workbench.colorTheme").expect("kept");
    let theme = at(&after, &|v| v["type"] == "theme" && v["theme"] == "Light Modern").expect("theme");
    let started = at(&after, &|v| v["type"] == "turn_start").expect("the command's turn");
    let asked = at(&after, &|v| v["type"] == "user").expect("the command as a question");
    assert_eq!(
        after[asked],
        json!({"type": "user", "text": "/explain the parser", "q": 2, "late": true, "queued": [1]})
    );
    assert!(
        model < thinking && thinking < mode && mode < kept && kept < theme && theme < asked && asked < started,
        "{after:?}"
    );
    let text: String = after.iter().filter(|v| v["type"] == "text").filter_map(|v| v["delta"].as_str()).collect();
    assert_eq!(text, "The second answer.");
    assert_eq!(after.last().expect("end")["ok"], true, "{after:?}");
    let last = until(&mut page, &|v| v["type"] == "turn_end").await;
    let asked = at(&last, &|v| v["type"] == "user").expect("the late message as a question");
    assert_eq!(last[asked], json!({"type": "user", "text": "and then?", "q": 3, "late": true, "queued": [2]}));
    assert!(at(&last, &|v| v["type"] == "turn_start").is_some_and(|started| asked < started), "{last:?}");
    let text: String = last.iter().filter(|v| v["type"] == "text").filter_map(|v| v["delta"].as_str()).collect();
    assert_eq!(text, "The third answer.");
    let settings = std::fs::read_to_string(home.path().join("config/settings.json")).expect("settings");
    assert!(settings.contains("Light Modern"), "{settings}");
    // Both were asked of the model chosen meanwhile: the command as what it stands for, then the message as the
    // person's.
    let requests = gateway.received_requests().await.expect("requests");
    let asked: Vec<Value> = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/chat/completions"))
        .map(|r| serde_json::from_slice(&r.body).expect("json"))
        .collect();
    assert_eq!(asked.len(), 3, "three turns");
    let models: Vec<&str> = asked.iter().filter_map(|a| a["model"].as_str()).collect();
    assert_eq!(models, ["qwen", "qwen-large", "qwen-large"]);
    let command = asked[1]["messages"].as_array().and_then(|m| m.last()).cloned().unwrap_or_default();
    assert!(
        command["content"].as_str().is_some_and(|c| c.starts_with("Explain the parser in this project")),
        "{command}"
    );
    let last = asked[2]["messages"].as_array().and_then(|m| m.last()).cloned().unwrap_or_default();
    assert_eq!((last["role"].as_str(), last["content"].as_str()), (Some("user"), Some("and then?")), "{last}");

    // Between turns the same messages are answered at once, and a name of the driver's own is not a page's to send.
    ask(&mut page, json!({"type": "_job_ended", "id": 1})).await;
    ask(&mut page, json!({"type": "rewind", "q": 2, "what": "chat"})).await;
    let back = until(&mut page, &|v| v["type"] == "state").await;
    assert!(at(&back, &|v| v["type"] == "refused" || v["type"] == "deferred").is_none(), "{back:?}");
    assert_eq!(back.last().expect("state")["messages"].as_array().map(Vec::len), Some(2), "{back:?}");
    // The page of turns before one, its key echoed.
    ask(&mut page, json!({"type": "older", "before_turn": 2})).await;
    let older = until(&mut page, &|v| v["type"] == "older").await.pop().expect("older");
    assert_eq!((&older["before_turn"], &older["earlier"], &older["first_turn"]), (&json!(2), &json!(0), &json!(1)));
    assert_eq!(older["messages"][0]["text"], "say hello", "{older}");
}

#[tokio::test]
async fn a_plan_proposed_in_the_ide_is_reviewed_and_the_review_runs_as_the_next_turn() {
    use futures_util::{SinkExt, StreamExt};
    use std::io::BufRead;
    use tokio_tungstenite::tungstenite::Message;
    let said = |text: &str| sse(json!({"choices": [{"delta": {"content": text}, "finish_reason": "stop"}]}));
    let plan = json!({"title": "Greet", "steps": [{"title": "Write the greeting"}, {"title": "Print it"}]});
    let gateway = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(vec![call("c1", "plan", plan), said("The plan is ready.")], AtomicUsize::new(0)))
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    let mut child = code_arena(home.path())
        .args(["ide", "--no-open", "-C"])
        .arg(folder.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("runs");
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let stderr = child.stderr.take().expect("stderr");
    let _child = Kill(child);
    let url = std::io::BufReader::new(stderr)
        .lines()
        .map_while(Result::ok)
        .find_map(|l| l.split("The IDE: ").nth(1).map(str::to_string))
        .expect("the address");
    let (base, key) = url.split_once("/?k=").expect("key in it");
    let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{}/ws", base.trim_start_matches("http://")).as_str(),
    )
    .expect("request");
    request.headers_mut().insert("cookie", format!("code_arena={key}").parse().expect("h"));
    request.headers_mut().insert("origin", base.parse().expect("h"));
    let (mut page, _) = tokio_tungstenite::connect_async(request).await.expect("the page connects");
    let until = async |page: &mut tokio_tungstenite::WebSocketStream<_>, end: &dyn Fn(&Value) -> bool| -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(20), page.next())
                .await
                .expect("in time")
                .expect("open")
                .expect("ok");
            let Message::Text(t) = m else { continue };
            let v: Value = serde_json::from_str(t.as_str()).expect("json");
            let done = end(&v);
            seen.push(v);
            if done {
                return seen;
            }
        }
    };
    let ask = async |page: &mut tokio_tungstenite::WebSocketStream<_>, m: Value| {
        page.send(Message::text(m.to_string())).await.expect("send");
    };
    until(&mut page, &|v| v["type"] == "state").await;
    // Nothing to review yet: said so, with the action it answers.
    ask(&mut page, json!({"type": "plan_review", "action": "approve"})).await;
    let no = until(&mut page, &|v| v["type"] == "plan_review").await.pop().expect("reply");
    assert!(no["error"].as_str().is_some_and(|e| e.contains("no plan waiting")), "{no}");
    // In plan mode the agent proposes one.
    ask(&mut page, json!({"type": "mode", "mode": "plan"})).await;
    ask(&mut page, json!({"type": "send", "text": "plan a greeting"})).await;
    let turn = until(&mut page, &|v| v["type"] == "turn_end").await;
    let tasks = turn.iter().rev().find(|v| v["type"] == "tasks" && v["plan"].is_object()).expect("the plan said");
    assert_eq!((&tasks["plan"]["review"], &tasks["plan"]["title"]), (&json!("proposed"), &json!("Greet")));
    // A change names what is to change.
    ask(&mut page, json!({"type": "plan_review", "action": "change", "step": 2})).await;
    let no = until(&mut page, &|v| v["type"] == "plan_review").await.pop().expect("reply");
    assert_eq!(no["error"], "Say what should change in step 2.", "{no}");
    // Approved: the mode is the one that carries it out, and the review is the next turn, said by the session.
    ask(&mut page, json!({"type": "plan_review", "action": "approve"})).await;
    let next = until(&mut page, &|v| v["type"] == "turn_end").await;
    let at = |f: &dyn Fn(&Value) -> bool| next.iter().position(f);
    let status = at(&|v| v["type"] == "status" && v["mode"] == "auto-edit").expect("the mode");
    let user = at(&|v| v["type"] == "user").expect("the next turn");
    assert!(status < user, "{next:?}");
    assert_eq!(next[user]["carried"], true, "{next:?}");
    assert!(next[user]["text"].as_str().is_some_and(|t| t.starts_with("I approve the plan.")), "{next:?}");
    let approved = at(&|v| v["type"] == "tasks" && v["plan"]["review"] == "approved").expect("the plan approved");
    assert!(approved < user, "{next:?}");
    // Cleared: no plan, and the plan this session left is not offered back to it.
    ask(&mut page, json!({"type": "plan_clear"})).await;
    let cleared = until(&mut page, &|v| v["type"] == "tasks").await;
    let text = cleared.iter().find(|v| v["type"] == "plan_clear").map(|v| v["text"].clone());
    assert_eq!(text, Some(json!("The plan is cleared.")), "{cleared:?}");
    let tasks = cleared.last().expect("tasks");
    assert!(tasks["plan"].is_null() && tasks["plan_left"].is_null(), "{tasks}");
}

#[tokio::test]
async fn a_goal_set_in_the_ide_is_worked_on_until_its_check_passes_and_loops_are_added_and_stopped() {
    use futures_util::{SinkExt, StreamExt};
    use std::io::BufRead;
    use tokio_tungstenite::tungstenite::Message;
    let said = |text: &str| sse(json!({"choices": [{"delta": {"content": text}, "finish_reason": "stop"}]}));
    let gateway = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(vec![said("Done.")], AtomicUsize::new(0)))
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    let mut child = code_arena(home.path())
        .args(["ide", "--no-open", "-C"])
        .arg(folder.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("runs");
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let stderr = child.stderr.take().expect("stderr");
    let _child = Kill(child);
    let url = std::io::BufReader::new(stderr)
        .lines()
        .map_while(Result::ok)
        .find_map(|l| l.split("The IDE: ").nth(1).map(str::to_string))
        .expect("the address");
    let (base, key) = url.split_once("/?k=").expect("key in it");
    let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{}/ws", base.trim_start_matches("http://")).as_str(),
    )
    .expect("request");
    request.headers_mut().insert("cookie", format!("code_arena={key}").parse().expect("h"));
    request.headers_mut().insert("origin", base.parse().expect("h"));
    let (mut page, _) = tokio_tungstenite::connect_async(request).await.expect("the page connects");
    let until = async |page: &mut tokio_tungstenite::WebSocketStream<_>, end: &dyn Fn(&Value) -> bool| -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(20), page.next())
                .await
                .expect("in time")
                .expect("open")
                .expect("ok");
            let Message::Text(t) = m else { continue };
            let v: Value = serde_json::from_str(t.as_str()).expect("json");
            let done = end(&v);
            seen.push(v);
            if done {
                return seen;
            }
        }
    };
    let ask = async |page: &mut tokio_tungstenite::WebSocketStream<_>, m: Value| {
        page.send(Message::text(m.to_string())).await.expect("send");
    };
    until(&mut page, &|v| v["type"] == "state").await;
    // A budget out of bounds is refused, with the key it answers.
    ask(&mut page, json!({"type": "goal_set", "id": 7, "text": "x", "check": null, "budget": {"steps": 0}})).await;
    let no = until(&mut page, &|v| v["type"] == "goal_set").await.pop().expect("reply");
    assert_eq!((&no["error"], &no["id"]), (&json!("The budget is out of bounds."), &json!(7)), "{no}");
    // Set: worked on at once, as a turn the session says itself, and met once its check passes.
    let goal = json!({"type": "goal_set", "text": "say hello", "check": {"kind": "command", "text": "true"},
                      "budget": {"steps": 50, "minutes": 10, "tokens": 100_000, "until_met": false}});
    ask(&mut page, goal).await;
    let turn = until(&mut page, &|v| v["type"] == "tasks" && v["goal"]["status"] == "met").await;
    let user = turn.iter().find(|v| v["type"] == "user").expect("the goal's turn");
    assert_eq!(user["carried"], true, "{turn:?}");
    let met = turn.last().expect("tasks");
    assert_eq!((&met["goal"]["turns"], &met["goal"]["check"]["by"]), (&json!(1), &json!("person")), "{met}");
    assert!(met["goal"]["outcome"].as_str().is_some_and(|o| o.starts_with("Goal met")), "{met}");
    assert_eq!(met["next"], json!([]), "{met}");
    // Nothing waits to be started next.
    ask(&mut page, json!({"type": "goal_next"})).await;
    let no = until(&mut page, &|v| v["type"] == "goal_next").await.pop().expect("reply");
    assert_eq!(no["error"], "No goal waits.", "{no}");
    // A loop added is listed with its runs, and stopped.
    ask(&mut page, json!({"type": "loop_add", "every": "1h", "prompt": "check the notes", "edits": false})).await;
    let added = until(&mut page, &|v| v["type"] == "loops").await;
    let reply = added.iter().find(|v| v["type"] == "loop_add").expect("reply");
    assert_eq!((&reply["id"], &reply["every"]), (&json!(1), &json!("1h")), "{reply}");
    let list = added.last().expect("loops");
    assert_eq!(
        (&list["items"][0]["every_words"], &list["items"][0]["prompt"]),
        (&json!("1h"), &json!("check the notes"))
    );
    ask(&mut page, json!({"type": "loop_stop", "id": 1})).await;
    let stopped = until(&mut page, &|v| v["type"] == "loops").await;
    assert_eq!(stopped.last().expect("loops")["items"], json!([]), "{stopped:?}");
}

#[tokio::test]
async fn what_the_ide_is_sent_is_kept_for_the_folder_and_listed_newest_first_with_the_terminals() {
    use futures_util::{SinkExt, StreamExt};
    use std::io::BufRead;
    use tokio_tungstenite::tungstenite::Message;
    let said = |text: &str| sse(json!({"choices": [{"delta": {"content": text}, "finish_reason": "stop"}]}));
    let gateway = MockServer::start().await;
    let hold = Hold::default();
    Mock::given(path("/v1/chat/completions"))
        .respond_with(HeldFirst(
            vec![said("The first answer."), said("The second answer.")],
            AtomicUsize::new(0),
            hold.clone(),
        ))
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    std::fs::create_dir(folder.path().join(".git")).expect("a repository");
    let mut child = code_arena(home.path())
        .args(["ide", "--no-open", "-C"])
        .arg(folder.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("runs");
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let stderr = child.stderr.take().expect("stderr");
    let _child = Kill(child);
    let url = std::io::BufReader::new(stderr)
        .lines()
        .map_while(Result::ok)
        .find_map(|l| l.split("The IDE: ").nth(1).map(str::to_string))
        .expect("the address");
    let (base, key) = url.split_once("/?k=").expect("key in it");
    let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{}/ws", base.trim_start_matches("http://")).as_str(),
    )
    .expect("request");
    request.headers_mut().insert("cookie", format!("code_arena={key}").parse().expect("h"));
    request.headers_mut().insert("origin", base.parse().expect("h"));
    let (mut page, _) = tokio_tungstenite::connect_async(request).await.expect("the page connects");
    // Every message up to (and with) the first for which `end` holds.
    let until = async |page: &mut tokio_tungstenite::WebSocketStream<_>, end: &dyn Fn(&Value) -> bool| -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(20), page.next())
                .await
                .expect("in time")
                .expect("open")
                .expect("ok");
            let Message::Text(t) = m else { continue };
            let v: Value = serde_json::from_str(t.as_str()).expect("json");
            let done = end(&v);
            seen.push(v);
            if done {
                return seen;
            }
        }
    };
    let ask = async |page: &mut tokio_tungstenite::WebSocketStream<_>, m: Value| {
        page.send(Message::text(m.to_string())).await.expect("send");
    };
    let listed = |seen: Vec<Value>| seen.last().cloned().expect("the folder's history");
    until(&mut page, &|v| v["type"] == "state").await;
    // Nothing was sent in this folder yet; the reply says the request's key again.
    ask(&mut page, json!({"type": "history", "key": "h1"})).await;
    let none = listed(until(&mut page, &|v| v["type"] == "history").await);
    assert_eq!(none, json!({"type": "history", "items": [], "key": "h1"}));

    // A message sent: the question says its key again (the face that sent it knows it was taken).
    ask(&mut page, json!({"type": "send", "text": "say hello", "key": "w1"})).await;
    let begun = until(&mut page, &|v| v["type"] == "turn_start").await;
    let asked = begun.iter().find(|v| v["type"] == "user").expect("the question");
    assert_eq!(*asked, json!({"type": "user", "text": "say hello", "q": 1, "key": "w1"}));
    // While the answer is held: a message for the turn to read, and a command that waits for the turn's end. Each
    // is told as queued with its key, and the history is answered meanwhile, the newest first.
    ask(&mut page, json!({"type": "send", "text": "and then?", "key": "w2"})).await;
    ask(&mut page, json!({"type": "send", "text": "/explain the parser", "key": "w3"})).await;
    ask(&mut page, json!({"type": "history"})).await;
    let meanwhile = until(&mut page, &|v| v["type"] == "history").await;
    let queued: Vec<&Value> = meanwhile.iter().filter(|v| v["type"] == "queued").collect();
    assert_eq!(
        queued,
        [
            &json!({"type": "queued", "text": "and then?", "key": "w2", "id": 1}),
            &json!({"type": "queued", "text": "/explain the parser", "later": true, "key": "w3", "id": 2})
        ],
        "{meanwhile:?}"
    );
    assert!(meanwhile.contains(&json!({"type": "deferred", "of": "send", "key": "w3"})), "{meanwhile:?}");
    assert_eq!(
        listed(meanwhile),
        json!({"type": "history", "items": ["/explain the parser", "and then?", "say hello"]})
    );
    {
        let (go, told) = &*hold;
        *go.lock().expect("lock") = true;
        told.notify_all();
    }
    // The command runs as a turn of its own once the first has ended, said again with its key.
    until(&mut page, &|v| v["type"] == "turn_end").await;
    let second = until(&mut page, &|v| v["type"] == "turn_end").await;
    let asked = second.iter().find(|v| v["type"] == "user").expect("the command as a question");
    assert_eq!(
        (&asked["text"], &asked["late"], &asked["key"]),
        (&json!("/explain the parser"), &json!(true), &json!("w3"))
    );
    until(&mut page, &|v| v["type"] == "turn_end").await;
    // Sent on after the turn it waited for, a message is not kept a second time; one sent again moves to the front.
    ask(&mut page, json!({"type": "send", "text": "say hello"})).await;
    until(&mut page, &|v| v["type"] == "turn_end").await;
    ask(&mut page, json!({"type": "history"})).await;
    assert_eq!(
        listed(until(&mut page, &|v| v["type"] == "history").await)["items"],
        json!(["say hello", "/explain the parser", "and then?"])
    );
    // The file is the terminal's own (one per folder, the person's only): ↑ there brings the same lines back.
    let kept: Vec<std::path::PathBuf> = std::fs::read_dir(home.path().join("data/history"))
        .expect("the history's folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    let lines = std::fs::read_to_string(&kept[0]).expect("read");
    assert_eq!(lines.lines().count(), 4, "{lines}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&kept[0]).expect("kept").permissions().mode() & 0o777, 0o600);
    }
}

#[tokio::test]
async fn lines_of_files_sent_from_the_ide_are_read_for_the_model_and_named_under_the_question() {
    use futures_util::{SinkExt, StreamExt};
    use std::io::BufRead;
    use tokio_tungstenite::tungstenite::Message;
    let said = sse(json!({"choices": [{"delta": {"content": "Seen."}, "finish_reason": "stop"}]}));
    let gateway = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(Script(vec![said], AtomicUsize::new(0)))
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    std::fs::create_dir(folder.path().join(".git")).expect("a repository");
    let body: String = (1..=20).map(|i| format!("let line{i} = {i};\n")).collect();
    std::fs::write(folder.path().join("cart.rs"), body).expect("a file");
    let mut child = code_arena(home.path())
        .args(["ide", "--no-open", "-C"])
        .arg(folder.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("runs");
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let stderr = child.stderr.take().expect("stderr");
    let _child = Kill(child);
    let url = std::io::BufReader::new(stderr)
        .lines()
        .map_while(Result::ok)
        .find_map(|l| l.split("The IDE: ").nth(1).map(str::to_string))
        .expect("the address");
    let (base, key) = url.split_once("/?k=").expect("key in it");
    let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{}/ws", base.trim_start_matches("http://")).as_str(),
    )
    .expect("request");
    request.headers_mut().insert("cookie", format!("code_arena={key}").parse().expect("h"));
    request.headers_mut().insert("origin", base.parse().expect("h"));
    let (mut page, _) = tokio_tungstenite::connect_async(request).await.expect("the page connects");
    let until = async |page: &mut tokio_tungstenite::WebSocketStream<_>, end: &dyn Fn(&Value) -> bool| -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(20), page.next())
                .await
                .expect("in time")
                .expect("open")
                .expect("ok");
            let Message::Text(t) = m else { continue };
            let v: Value = serde_json::from_str(t.as_str()).expect("json");
            let done = end(&v);
            seen.push(v);
            if done {
                return seen;
            }
        }
    };
    until(&mut page, &|v| v["type"] == "state").await;
    let sent = json!({"type": "send", "text": "Explain this", "key": "p1",
        "context": [{"path": "cart.rs", "start": 10, "end": 11}, {"path": "../outside.rs", "start": 1, "end": 1}]});
    page.send(Message::text(sent.to_string())).await.expect("send");
    let turn = until(&mut page, &|v| v["type"] == "turn_end").await;
    // The question shows the words, its lines named under it; the file outside the folder was not sent.
    let asked = turn.iter().find(|v| v["type"] == "user").expect("the question");
    assert_eq!(asked["text"], "Explain this", "{asked}");
    assert_eq!(asked["files"], json!([{"path": "cart.rs", "start": 10, "end": 11}]), "{asked}");
    // The model read them with three lines around.
    let requests = gateway.received_requests().await.expect("requests");
    let read = String::from_utf8_lossy(&requests.last().expect("a request").body).to_string();
    assert!(read.contains("<file path=\\\"cart.rs\\\">"), "{read}");
    assert!(read.contains("let line7 = 7;") && read.contains("let line14 = 14;"), "{read}");
    assert!(!read.contains("let line6 = 6;") && !read.contains("let line15 = 15;"), "{read}");
    // Kept with the question: the session's state gives them back.
    page.send(Message::text(json!({"type": "state"}).to_string())).await.expect("send");
    let state = until(&mut page, &|v| v["type"] == "state").await;
    let messages = &state.last().expect("state")["messages"];
    let question = messages.as_array().into_iter().flatten().find(|m| m["role"] == "user").expect("kept");
    assert_eq!(question["files"][0]["path"], "cart.rs", "{question}");
}

#[tokio::test]
async fn an_extension_is_installed_from_a_vsix_the_ide_is_given_and_listed_to_every_face() {
    use futures_util::{SinkExt, StreamExt};
    use std::io::{BufRead, Write};
    use tokio_tungstenite::tungstenite::Message;
    let gateway = MockServer::start().await;
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    std::fs::create_dir_all(folder.path().join(".vscode")).expect("a .vscode");
    std::fs::write(
        folder.path().join(".vscode/extensions.json"),
        r#"{"recommendations": ["acme.night", "someone.else"]}"#,
    )
    .expect("recommendations");
    // A .vsix with one colour theme, in the project (named by its path there).
    let mut z = zip::ZipWriter::new(std::fs::File::create(folder.path().join("night.vsix")).expect("vsix"));
    let o = zip::write::SimpleFileOptions::default();
    for (name, body) in [
        (
            "extension/package.json",
            r#"{"publisher": "acme", "name": "night", "version": "1.0.0", "contributes": {"themes": [{"label": "Night", "uiTheme": "vs-dark", "path": "./night.json"}]}}"#,
        ),
        ("extension/night.json", r##"{"colors": {"editor.background": "#000000"}}"##),
    ] {
        z.start_file(name, o).expect("start");
        z.write_all(body.as_bytes()).expect("write");
    }
    z.finish().expect("zip");
    let mut child = code_arena(home.path())
        .args(["ide", "--no-open", "-C"])
        .arg(folder.path())
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway.uri())
        .env("ARENA_MODEL", "qwen")
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("runs");
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let stderr = child.stderr.take().expect("stderr");
    let _child = Kill(child);
    let url = std::io::BufReader::new(stderr)
        .lines()
        .map_while(Result::ok)
        .find_map(|l| l.split("The IDE: ").nth(1).map(str::to_string))
        .expect("the address");
    let (base, key) = url.split_once("/?k=").expect("key in it");
    let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{}/ws", base.trim_start_matches("http://")).as_str(),
    )
    .expect("request");
    request.headers_mut().insert("cookie", format!("code_arena={key}").parse().expect("h"));
    request.headers_mut().insert("origin", base.parse().expect("h"));
    let (mut page, _) = tokio_tungstenite::connect_async(request).await.expect("the page connects");
    let next = async |page: &mut tokio_tungstenite::WebSocketStream<_>, of: &str| -> Value {
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(20), page.next())
                .await
                .expect("in time")
                .expect("open")
                .expect("ok");
            let Message::Text(t) = m else { continue };
            let v: Value = serde_json::from_str(t.as_str()).expect("json");
            if v["type"] == of {
                return v;
            }
        }
    };
    let ask = async |page: &mut tokio_tungstenite::WebSocketStream<_>, m: Value| {
        page.send(Message::text(m.to_string())).await.expect("send");
    };
    next(&mut page, "state").await;
    ask(&mut page, json!({"type": "extensions"})).await;
    assert_eq!(next(&mut page, "extensions").await, json!({"type": "extensions", "items": []}));
    ask(&mut page, json!({"type": "ext_install", "path": "night.vsix"})).await;
    let installed = next(&mut page, "ext_install").await;
    assert_eq!(
        (installed["path"].as_str(), installed["item"]["id"].as_str()),
        (Some("night.vsix"), Some("acme.night"))
    );
    // Every face hears the list and what the extensions bring, as they now are.
    assert_eq!(next(&mut page, "extensions").await["items"][0]["id"], "acme.night");
    let brought = next(&mut page, "ext_contributions").await;
    assert_eq!(
        (brought["themes"][0]["label"].as_str(), brought["themes"][0]["extension"].as_str()),
        (Some("Night"), Some("acme.night"))
    );
    assert!(home.path().join("data/extensions/acme.night-1.0.0/night.json").is_file());
    ask(&mut page, json!({"type": "ext_recommended"})).await;
    let said = next(&mut page, "ext_recommended").await;
    let stands: Vec<&str> =
        said["recommendations"].as_array().expect("list").iter().filter_map(|r| r["status"].as_str()).collect();
    assert_eq!(stands, ["installed", "unknown"]);
}

/// `ide --connect`: Arena's connect route is dialled with the person's key and the folder's id, and the next run
/// dials with the same id; without Arena it is refused.
#[tokio::test]
async fn ide_connect_dials_code_studio_with_the_folder_kept() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("config")).expect("config");
    std::fs::write(
        home.path().join("config/settings.json"),
        json!({"mcp.arenaTools": false, "mcp.codeIntelTools": false, "arena.syncChats": false}).to_string(),
    )
    .expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    let out = code_arena(home.path()).args(["ide", "--connect", "-C"]).arg(folder.path()).output().expect("runs");
    assert!(String::from_utf8_lossy(&out.stderr).contains("Code Studio is Arena's: sign in first"), "{out:?}");

    let gateway = MockServer::start().await;
    let arena = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = arena.local_addr().expect("addr").port();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let child = code_arena(home.path())
            .args(["ide", "--connect", "-C"])
            .arg(folder.path())
            .env("ARENA_URL", format!("http://127.0.0.1:{port}"))
            .env("ARENA_API_KEY", "sk-test-only")
            .env("ARENA_GATEWAY_URL", gateway.uri())
            .env("ARENA_MODEL", "qwen")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("runs");
        struct Kill(std::process::Child);
        impl Drop for Kill {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let _child = Kill(child);
        // Arena's other calls (whose the key is, its settings) are answered "not here": the dial is the one looked for.
        let head = loop {
            let (mut s, _) = tokio::time::timeout(std::time::Duration::from_secs(60), arena.accept())
                .await
                .expect("dialled")
                .expect("ok");
            let mut head = Vec::new();
            let mut b = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = tokio::time::timeout(std::time::Duration::from_secs(10), s.read(&mut b))
                    .await
                    .expect("head")
                    .expect("read");
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&b[..n]);
            }
            let head = String::from_utf8_lossy(&head).to_lowercase();
            if head.starts_with("get /api/code-arena/connect?") {
                break head;
            }
            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        };
        assert!(head.starts_with("get /api/code-arena/connect?id="), "{head}");
        assert!(head.contains("authorization: bearer sk-test-only"), "{head}");
        assert!(head.contains("upgrade: websocket"), "{head}");
        let id = head["get /api/code-arena/connect?id=".len()..].split('&').next().expect("id").to_string();
        assert_eq!(id.len(), 32, "{head}");
        ids.push(id);
    }
    assert_eq!(ids[0], ids[1], "the folder keeps its id");
}
