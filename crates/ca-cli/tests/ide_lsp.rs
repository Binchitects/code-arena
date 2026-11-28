//! The language servers as a face is told them, against the program as people run it: the person's own server
//! listed, started, its messages relayed both ways with the folder's paths as the server's, and every server
//! stopped when the folder's trust is taken back.

use std::io::BufRead;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

type Page = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Kill(std::process::Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The IDE started on a folder, and a page connected to it.
async fn ide(home: &Path, folder: &Path, gateway: &str) -> (Kill, Page) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_code-arena"));
    c.env("CODE_ARENA_HOME", home).env("NO_COLOR", "1").env("NO_PROXY", "127.0.0.1,localhost");
    for v in ["ARENA_MCP_URL", "CODE_ARENA_MODE"] {
        c.env_remove(v);
    }
    let mut child = c
        .args(["ide", "--no-open", "-C"])
        .arg(folder)
        .env("ARENA_URL", "https://arena.example")
        .env("ARENA_API_KEY", "sk-test-only")
        .env("ARENA_GATEWAY_URL", gateway)
        .env("ARENA_MODEL", "qwen")
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("runs");
    let stderr = child.stderr.take().expect("stderr");
    let child = Kill(child);
    let url = std::io::BufReader::new(stderr)
        .lines()
        .map_while(Result::ok)
        .find_map(|l| l.split("The IDE: ").nth(1).map(str::to_string))
        .expect("the address");
    let (base, key) = url.split_once("/?k=").expect("key in it");
    let host = base.trim_start_matches("http://");
    let mut r = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{host}/ws").as_str(),
    )
    .expect("request");
    r.headers_mut().insert("cookie", format!("code_arena={key}").parse().expect("h"));
    r.headers_mut().insert("origin", base.parse().expect("h"));
    let (page, _) = tokio_tungstenite::connect_async(r).await.expect("the page connects");
    (child, page)
}

/// The next message that is the one waited for; the others are passed over.
async fn until(page: &mut Page, is: impl Fn(&Value) -> bool) -> Value {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(20), page.next())
            .await
            .expect("in time")
            .expect("open")
            .expect("ok");
        if let Message::Text(t) = m {
            let v: Value = serde_json::from_str(t.as_str()).expect("json");
            if is(&v) {
                return v;
            }
        }
    }
}

async fn say(page: &mut Page, m: Value) {
    page.send(Message::text(m.to_string())).await.expect("send");
}

#[cfg(unix)]
#[tokio::test]
async fn a_persons_own_server_is_listed_started_relayed_and_stopped_with_the_trust() {
    let gateway = wiremock::MockServer::start().await;
    let home = tempfile::tempdir().expect("home");
    let config = home.path().join("config");
    std::fs::create_dir_all(&config).expect("config");
    // `cat` says back what it is sent: what reached it is what comes back.
    let settings = json!({
        "mcp.arenaTools": false,
        "mcp.codeIntelTools": false,
        "arena.syncChats": false,
        "tools.sandbox": "off",
        "languageServers": {"echo": {"command": ["cat"], "languages": ["plaintext"], "settings": {"echo": {"a": 1}}}},
    });
    std::fs::write(config.join("settings.json"), settings.to_string()).expect("settings");
    std::fs::write(config.join("credentials.json"), "{}").expect("credentials");
    let folder = tempfile::tempdir().expect("folder");
    let root = std::fs::canonicalize(folder.path()).expect("real");
    let data = home.path().join("data");
    std::fs::create_dir_all(&data).expect("data");
    let trust = json!({"folders": {root.display().to_string(): true}}).to_string();
    std::fs::write(data.join("trust.json"), trust).expect("trust");
    let (_child, mut page) = ide(home.path(), &root, &gateway.uri()).await;
    let page = &mut page;
    until(page, |v| v["type"] == "state").await;

    say(page, json!({"type": "lsp_servers"})).await;
    let l = until(page, |v| v["type"] == "lsp_servers").await;
    assert_eq!(l["trusted"], true);
    let echo = &l["items"][0];
    assert_eq!(
        (echo["id"].as_str(), echo["found"].as_bool(), echo["state"].as_str()),
        (Some("echo"), Some(true), Some("stopped")),
        "{l}"
    );
    assert_eq!(echo["settings"], json!({"echo": {"a": 1}}));
    assert!(l["items"].as_array().expect("items").iter().any(|i| i["id"] == "rust-analyzer"));

    say(page, json!({"type": "lsp_start", "server": "echo", "client": "w"})).await;
    let s = until(page, |v| v["type"] == "lsp_state").await;
    assert_eq!(
        (s["state"].as_str(), s["server"].as_str(), s["client"].as_str()),
        (Some("running"), Some("echo"), Some("w")),
        "{s}"
    );
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}});
    say(page, json!({"type": "lsp_send", "server": "echo", "client": "w", "msg": init})).await;
    let open = json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "file:///notes.txt"}}});
    say(page, json!({"type": "lsp_send", "server": "echo", "client": "w", "msg": open})).await;
    let back = until(page, |v| v["type"] == "lsp_msg" && v["msg"]["method"] == "initialize").await;
    assert_eq!(back["msg"]["params"]["rootPath"], root.display().to_string());
    assert_eq!(back["msg"]["params"]["rootUri"], "file:///");
    let back = until(page, |v| v["type"] == "lsp_msg" && v["msg"]["method"] == "textDocument/didOpen").await;
    assert_eq!(back["msg"]["params"]["textDocument"]["uri"], "file:///notes.txt");

    // The trust taken back: it stops, its window told, and it is not started again.
    say(page, json!({"type": "trust_set", "trusted": false})).await;
    let ended = until(page, |v| v["type"] == "lsp_msg" && v["msg"]["method"] == "codeArena/ended").await;
    assert!(ended["msg"]["params"]["reason"].as_str().expect("why").contains("no longer trusted"), "{ended}");
    assert_eq!(ended["msg"]["params"]["stopped"], true);
    say(page, json!({"type": "lsp_start", "server": "echo", "client": "w"})).await;
    let s = until(page, |v| v["type"] == "lsp_state" && v["kind"].is_string()).await;
    assert_eq!((s["kind"].as_str(), s["state"].as_str()), (Some("not_trusted"), Some("restricted")), "{s}");
    say(page, json!({"type": "lsp_log", "server": "echo"})).await;
    let log = until(page, |v| v["type"] == "lsp_log").await;
    assert_eq!((log["server"].as_str(), log["running"].as_bool()), (Some("echo"), Some(false)), "{log}");
}
