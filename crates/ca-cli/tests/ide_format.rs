//! A file's formatters as a face is told them, and a text formatted by one, against the program as people run it:
//! the person's own formatter found by the file's ending, run on the text, its fault's place said, and nothing run
//! in a folder that is not trusted.

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
async fn the_persons_own_formatter_is_listed_runs_on_the_text_and_runs_only_in_a_trusted_folder() {
    let gateway = wiremock::MockServer::start().await;
    let home = tempfile::tempdir().expect("home");
    let config = home.path().join("config");
    std::fs::create_dir_all(&config).expect("config");
    let settings = json!({
        "mcp.arenaTools": false,
        "mcp.codeIntelTools": false,
        "arena.syncChats": false,
        "formatters": {
            "upper": {"name": "Upper", "command": "sh", "args": ["-c", "tr a-z A-Z"], "extensions": [".txt"]},
            "picky": {"command": "sh", "args": ["-c", "echo '${fileBasename}:2:3: nope' >&2; exit 1"], "extensions": [".bad"]},
        },
    });
    std::fs::write(config.join("settings.json"), settings.to_string()).expect("settings");
    std::fs::write(config.join("credentials.json"), "{}").expect("credentials");
    let folder = tempfile::tempdir().expect("folder");
    let root = std::fs::canonicalize(folder.path()).expect("real");
    let data = home.path().join("data");
    std::fs::create_dir_all(&data).expect("data");
    let trust = |on: bool| json!({"folders": {root.display().to_string(): on}}).to_string();
    std::fs::write(data.join("trust.json"), trust(true)).expect("trust");
    let (_child, mut page) = ide(home.path(), &root, &gateway.uri()).await;
    let page = &mut page;
    until(page, |v| v["type"] == "state").await;

    // Listed for the file by its ending, with the key it answers.
    say(page, json!({"type": "formatters", "path": "notes.txt", "id": 1})).await;
    let l = until(page, |v| v["type"] == "formatters").await;
    assert_eq!(
        (l["path"].as_str(), l["id"].as_i64(), l["language"].as_str()),
        (Some("notes.txt"), Some(1), Some("plaintext"))
    );
    assert_eq!(l["default"], "upper", "{l}");
    let item = &l["items"][0];
    assert_eq!(
        (item["name"].as_str(), item["source"].as_str(), item["available"].as_bool()),
        (Some("Upper"), Some("settings"), Some(true))
    );
    assert_eq!(item["range"], false);

    // Formatted: the whole text back, with the formatter that did it.
    say(page, json!({"type": "format", "path": "notes.txt", "text": "hello\n", "id": 2})).await;
    let f = until(page, |v| v["type"] == "formatted").await;
    assert_eq!(
        (f["text"].as_str(), f["formatter"].as_str(), f["id"].as_i64()),
        (Some("HELLO\n"), Some("upper"), Some(2)),
        "{f}"
    );

    // A selection, to a formatter of whole files only.
    let range = json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}});
    say(page, json!({"type": "format", "path": "notes.txt", "text": "hello\n", "range": range})).await;
    let f = until(page, |v| v["type"] == "formatted").await;
    assert_eq!(f["kind"], "no_range", "{f}");

    // Its fault said with its place.
    say(page, json!({"type": "format", "path": "x.bad", "text": "x"})).await;
    let f = until(page, |v| v["type"] == "formatted").await;
    assert_eq!((f["kind"].as_str(), f["line"].as_u64(), f["col"].as_u64()), (Some("failed"), Some(2), Some(3)), "{f}");
    assert_eq!(f["error"], "picky: x.bad:2:3: nope");

    // A formatter the file has not.
    say(page, json!({"type": "format", "path": "notes.txt", "text": "x", "formatter": "esbenp.prettier-vscode"})).await;
    let f = until(page, |v| v["type"] == "formatted").await;
    assert_eq!(f["kind"], "not_found", "{f}");

    // Not trusted: nothing runs.
    std::fs::write(data.join("trust.json"), trust(false)).expect("trust");
    say(page, json!({"type": "format", "path": "notes.txt", "text": "hello"})).await;
    let f = until(page, |v| v["type"] == "formatted").await;
    assert_eq!((f["kind"].as_str(), f["text"].as_str()), (Some("not_trusted"), None), "{f}");
    say(page, json!({"type": "formatters", "path": "notes.txt"})).await;
    let l = until(page, |v| v["type"] == "formatters").await;
    assert_eq!(l["items"][0]["available"], false, "{l}");
    assert!(l["items"][0]["reason"].as_str().is_some_and(|r| r.contains("not trusted")), "{l}");
}
