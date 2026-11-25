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

#[test]
fn print_without_signing_in_says_what_to_do() {
    let home = tempfile::tempdir().expect("home");
    let out = code_arena(home.path()).args(["-p", "hi"]).output().expect("runs");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not signed in: run code-arena login"));
}
