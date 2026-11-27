//! The IDE's settings as a face is told them, against the program as people run it: asked for, said again when a
//! setting is kept or a settings file changes, and the person's own files opened and saved.

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

/// The next two messages waited for, whichever comes first; the others are passed over.
async fn both(page: &mut Page, a: impl Fn(&Value) -> bool, b: impl Fn(&Value) -> bool) -> (Value, Value) {
    let (mut first, mut second) = (None, None);
    loop {
        let v = until(page, |v| a(v) || b(v)).await;
        if a(&v) {
            first = Some(v);
        } else {
            second = Some(v);
        }
        if let (Some(a), Some(b)) = (&first, &second) {
            return (a.clone(), b.clone());
        }
    }
}

async fn say(page: &mut Page, m: Value) {
    page.send(Message::text(m.to_string())).await.expect("send");
}

#[tokio::test]
async fn the_ide_says_its_settings_again_when_one_is_kept_or_a_file_changes_and_opens_the_persons_files() {
    let gateway = wiremock::MockServer::start().await;
    let home = tempfile::tempdir().expect("home");
    let config = home.path().join("config");
    std::fs::create_dir_all(&config).expect("config");
    std::fs::write(
        config.join("settings.json"),
        "{\n  // mine\n  \"mcp.arenaTools\": false,\n  \"mcp.codeIntelTools\": false,\n  \"arena.syncChats\": false,\n  \"editor.tabSize\": 8,\n  \"[python]\": {\n    \"editor.tabSize\": 2\n  }\n}\n",
    )
    .expect("settings");
    std::fs::write(config.join("credentials.json"), "{}").expect("credentials");
    let folder = tempfile::tempdir().expect("folder");
    std::fs::create_dir_all(folder.path().join(".code-arena")).expect("dir");
    std::fs::write(folder.path().join(".code-arena/settings.json"), "{\"editor.fontSize\": 16}").expect("project");
    let (_child, mut page) = ide(home.path(), folder.path(), &gateway.uri()).await;
    let page = &mut page;
    until(page, |v| v["type"] == "state").await;
    let settings = |v: &Value| v["type"] == "settings";

    // Asked for: every value in force, a language's own, where each comes from.
    say(page, json!({"type": "settings"})).await;
    let s = until(page, settings).await;
    assert_eq!((&s["values"]["editor.tabSize"], &s["values"]["editor.fontSize"]), (&json!(8), &json!(16)), "{s}");
    assert_eq!(s["values"]["editor.wordWrap"], "off");
    assert_eq!(s["by_lang"], json!({"python": {"editor.tabSize": 2}}));
    assert_eq!(s["sources"]["editor.tabSize"], config.join("settings.json").display().to_string());
    assert_eq!(s["problems"], json!([]));
    let workspace = s["files"]["workspace"].as_str().expect("its path").to_string();
    assert!(workspace.ends_with(".vscode/settings.json"), "{workspace}");

    // The project's file as VS Code keeps it is made when asked for, once.
    say(page, json!({"type": "workspace_settings"})).await;
    let made = until(page, |v| v["type"] == "workspace_settings").await;
    assert_eq!((made["path"].as_str(), made["made"].as_bool()), (Some(".vscode/settings.json"), Some(true)));
    assert_eq!(std::fs::read_to_string(&workspace).expect("made"), "{\n}\n");
    // The settings are said again for it: a file is there that was not.
    until(page, settings).await;

    // A setting kept for one language, in the project's file: said back with what it answers, and the settings
    // said again at once.
    say(
        page,
        json!({"type": "set_setting", "key": "editor.tabSize", "value": 3, "lang": "rust", "where": "workspace"}),
    )
    .await;
    let kept = until(page, |v| v["type"] == "setting_kept").await;
    assert_eq!(
        (kept["key"].as_str(), kept["lang"].as_str(), kept["where"].as_str(), kept["file"].as_str()),
        (Some("editor.tabSize"), Some("rust"), Some("workspace"), Some(workspace.as_str()))
    );
    let s = until(page, settings).await;
    assert_eq!(s["by_lang"], json!({"python": {"editor.tabSize": 2}, "rust": {"editor.tabSize": 3}}));
    assert_eq!(s["values"]["editor.tabSize"], 8);
    assert_eq!(
        std::fs::read_to_string(&workspace).expect("kept"),
        "{\n  \"[rust]\": {\n    \"editor.tabSize\": 3\n  }\n}\n"
    );
    // One that may be set for one language only where the schema says so, and a project's only when it is not
    // the person's alone.
    say(page, json!({"type": "set_setting", "key": "files.autoSave", "value": "afterDelay", "lang": "rust"})).await;
    let refused = until(page, |v| v["type"] == "setting_refused").await;
    assert_eq!(refused["error"], "files.autoSave cannot be set for one language");
    say(
        page,
        json!({"type": "set_setting", "key": "terminal.integrated.allowChords", "value": false, "where": "workspace"}),
    )
    .await;
    let refused = until(page, |v| v["type"] == "setting_refused").await;
    assert!(refused["error"].as_str().is_some_and(|e| e.contains("yours alone")), "{refused}");
    // A language's own setting may be the project's (it was refused before languages had their own).
    say(page, json!({"type": "set_setting", "key": "editor.wordWrap", "value": "on", "where": "project"})).await;
    until(page, |v| v["type"] == "setting_kept" && v["key"] == "editor.wordWrap").await;
    assert_eq!(until(page, settings).await["values"]["editor.wordWrap"], "on");

    // The person's own file, opened as it is and saved: the settings follow what was saved.
    say(page, json!({"type": "user_file", "name": "settings.json"})).await;
    let file = until(page, |v| v["type"] == "user_file" && v["name"] == "settings.json").await;
    let text = file["text"].as_str().expect("its text");
    assert!(text.contains("// mine") && file["modified"].as_u64().is_some(), "{file}");
    assert_eq!(file["path"], config.join("settings.json").display().to_string());
    let edited = text.replace("\"editor.tabSize\": 8", "\"editor.tabSize\": 6");
    say(page, json!({"type": "save_user", "name": "settings.json", "text": edited})).await;
    let (saved, s) = both(page, |v| v["type"] == "saved_user", settings).await;
    assert!(saved["name"] == "settings.json" && saved.get("error").is_none() && saved["modified"].is_u64(), "{saved}");
    assert_eq!(s["values"]["editor.tabSize"], 6);
    assert_eq!(std::fs::read_to_string(config.join("settings.json")).expect("saved"), edited);
    // No other file of the settings folder is read or written through here.
    say(page, json!({"type": "user_file", "name": "credentials.json"})).await;
    let no = until(page, |v| v["type"] == "user_file" && v["name"] == "credentials.json").await;
    assert!(no["error"].is_string() && no.get("text").is_none(), "{no}");
    say(page, json!({"type": "save_user", "name": "../credentials.json", "text": "x"})).await;
    assert!(until(page, |v| v["type"] == "saved_user").await["error"].is_string());
    assert_eq!(std::fs::read_to_string(config.join("credentials.json")).expect("left"), "{}");
    // The keys' file is not there yet: empty, and made by saving it.
    say(page, json!({"type": "user_file", "name": "keybindings.json"})).await;
    let keys = until(page, |v| v["type"] == "user_file" && v["name"] == "keybindings.json").await;
    assert_eq!((keys["text"].as_str(), keys["modified"].is_null()), (Some(""), true));

    // A file changed on disk by another program: the settings are said again unasked, with what was not taken;
    // the theme too when it is another, and the person's file as it is now.
    tokio::time::sleep(Duration::from_millis(20)).await;
    std::fs::write(
        config.join("settings.json"),
        "{\"mcp.arenaTools\": false, \"mcp.codeIntelTools\": false, \"arena.syncChats\": false, \"editor.tabSize\": \"wide\", \"workbench.colorTheme\": \"Light Modern\"}",
    )
    .expect("write");
    let s = until(page, settings).await;
    assert_eq!(s["values"]["editor.tabSize"], 4);
    assert_eq!(s["problems"][0]["key"], "editor.tabSize");
    assert_eq!(s["by_lang"], json!({"rust": {"editor.tabSize": 3}}));
    assert_eq!(until(page, |v| v["type"] == "theme").await["theme"], "Light Modern");
    let again = until(page, |v| v["type"] == "user_file" && v["name"] == "settings.json").await;
    assert!(again["text"].as_str().is_some_and(|t| t.contains("\"wide\"")), "{again}");
    // Asked for again, it is what was read last, and the session itself reads the same from now on.
    say(page, json!({"type": "settings"})).await;
    assert_eq!(until(page, settings).await["values"]["workbench.colorTheme"], "Light Modern");
    say(page, json!({"type": "state"})).await;
    assert_eq!(until(page, |v| v["type"] == "state").await["theme"], "Light Modern");
}

#[tokio::test]
async fn a_settings_file_changed_while_a_turn_runs_is_said_at_once_and_is_the_sessions_own_after_it() {
    // A model that takes its time, so that the turn is still running when the file changes.
    let gateway = wiremock::MockServer::start().await;
    let answer = json!({"choices": [{"delta": {"content": "Done."}, "finish_reason": "stop"}]});
    wiremock::Mock::given(wiremock::matchers::path("/v1/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {answer}\n\ndata: [DONE]\n\n"))
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&gateway)
        .await;
    let home = tempfile::tempdir().expect("home");
    let config = home.path().join("config");
    std::fs::create_dir_all(&config).expect("config");
    let quiet = "\"mcp.arenaTools\": false, \"mcp.codeIntelTools\": false, \"arena.syncChats\": false";
    std::fs::write(config.join("settings.json"), format!("{{{quiet}}}")).expect("settings");
    let folder = tempfile::tempdir().expect("folder");
    let (_child, mut page) = ide(home.path(), folder.path(), &gateway.uri()).await;
    let page = &mut page;
    until(page, |v| v["type"] == "state").await;
    say(page, json!({"type": "send", "text": "take your time"})).await;
    until(page, |v| v["type"] == "turn_start").await;
    // The file changes while the model is asked: the settings and the theme are said before the turn ends.
    std::fs::write(
        config.join("settings.json"),
        format!("{{{quiet}, \"editor.tabSize\": 2, \"workbench.colorTheme\": \"Light Modern\"}}"),
    )
    .expect("write");
    let s = until(page, |v| v["type"] == "settings" || v["type"] == "turn_end").await;
    assert_eq!(s["type"], "settings", "the settings are said while the turn runs, not after it");
    assert_eq!(s["values"]["editor.tabSize"], 2);
    assert_eq!(until(page, |v| v["type"] == "theme").await["theme"], "Light Modern");
    // Asked for meanwhile, they are as they were read last.
    say(page, json!({"type": "settings"})).await;
    assert_eq!(until(page, |v| v["type"] == "settings").await["values"]["workbench.colorTheme"], "Light Modern");
    // The turn ends as it would have, and the session itself has the settings read meanwhile.
    let end = until(page, |v| v["type"] == "turn_end").await;
    assert!(end["error"].is_null(), "{end}");
    say(page, json!({"type": "state"})).await;
    assert_eq!(until(page, |v| v["type"] == "state").await["theme"], "Light Modern");
}
