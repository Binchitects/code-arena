use std::io::{BufReader, Read};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::relay::{self, Asked, Servers, Setup, read_frame, write_frame};
use super::*;

/// A folder, the servers' setting, and whether the folder is trusted.
fn setup(root: &Path, setting: Value, trusted: bool, off: &[&str]) -> Setup {
    let off: Vec<String> = off.iter().map(|s| s.to_string()).collect();
    Setup {
        root: root.to_path_buf(),
        data: root.join(".data"),
        home: None,
        env: passed(),
        ask: Arc::new(move || Asked { setting: setting.clone(), off: off.clone(), trusted, sandbox: Ok(None) }),
    }
}

/// What the faces were told, until `until` holds of the last or ten seconds went by.
fn heard(rx: &mut broadcast::Receiver<String>, mut until: impl FnMut(&Value) -> bool) -> Vec<Value> {
    let mut all = Vec::new();
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        match rx.try_recv() {
            Ok(t) => {
                let v: Value = serde_json::from_str(&t).expect("json");
                let done = until(&v);
                all.push(v);
                if done {
                    return all;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("not heard: {all:#?}");
}

fn state(v: &Value, server: &str, state: &str) -> bool {
    v["type"] == "lsp_state" && v["server"] == server && v["state"] == state
}

#[cfg(unix)]
#[test]
fn a_servers_messages_go_both_ways_rewritten_and_another_window_takes_it_over() {
    let d = tempfile::tempdir().expect("temp");
    let root = std::fs::canonicalize(d.path()).expect("real");
    // `cat` says back what it is sent: what reached it is what comes back.
    let s = setup(&root, json!({"echo": {"command": ["cat"], "languages": ["plaintext"]}}), true, &[]);
    let (out, mut rx) = broadcast::channel(256);
    let servers = Arc::new(Servers::default());
    servers.start("echo", Some("w1".into()), s.clone(), &out);
    let started = heard(&mut rx, |v| v["type"] == "lsp_state");
    assert!(state(&started[0], "echo", "running"), "{started:?}");
    assert_eq!(started[0]["client"], "w1");
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": "file:///elsewhere", "capabilities": {}}});
    servers.relay("echo", Some("w1".into()), init, &out);
    let open = json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "file:///src/a.txt", "text": "file:///x"}}});
    servers.relay("echo", Some("w1".into()), open, &out);
    let back = heard(&mut rx, |v| v["msg"]["method"] == "textDocument/didOpen");
    assert_eq!(back.len(), 2, "in the order sent: {back:?}");
    let p = &back[0]["msg"]["params"];
    assert_eq!(p["rootPath"], root.display().to_string(), "the driver decides the folder");
    assert_eq!(p["rootUri"], "file:///", "the folder's own URI, as the window's");
    assert_eq!(p["processId"], std::process::id());
    assert_eq!(p["workspaceFolders"][0]["uri"], "file:///");
    assert_eq!(p["initializationOptions"], json!({}));
    assert_eq!(back[1]["msg"]["params"]["textDocument"], json!({"uri": "file:///src/a.txt", "text": "file:///x"}));
    assert_eq!((&back[1]["server"], &back[1]["client"]), (&json!("echo"), &json!("w1")));
    assert_eq!(servers.seen("echo").state, "running");
    // Another window starts it: the first is told, and the server is its own.
    servers.start("echo", Some("w2".into()), s.clone(), &out);
    let taken = heard(&mut rx, |v| state(v, "echo", "running"));
    let ended = taken.iter().find(|v| v["msg"]["method"] == "codeArena/ended").expect("told");
    assert_eq!((&ended["client"], &ended["msg"]["params"]["takenOver"]), (&json!("w1"), &json!(true)));
    assert!(taken.iter().any(|v| state(v, "echo", "taken") && v["client"] == "w1"));
    assert_eq!(taken.last().expect("last")["client"], "w2");
    assert_eq!(servers.seen("echo").client.as_deref(), Some("w2"));
    // The first window's request is answered with an error; the same window's second start starts nothing.
    servers.relay("echo", Some("w1".into()), json!({"jsonrpc": "2.0", "id": 5, "method": "textDocument/hover"}), &out);
    let refused = heard(&mut rx, |v| v["type"] == "lsp_msg");
    assert_eq!((&refused[0]["client"], &refused[0]["msg"]["error"]["code"]), (&json!("w1"), &json!(-32803)));
    servers.start("echo", Some("w2".into()), s, &out);
    assert!(state(&heard(&mut rx, |v| v["type"] == "lsp_state")[0], "echo", "running"));
    // Stopped: shutdown and exit, then the end said.
    servers.stop("echo", Some("w2".into()), &out);
    let stopped = heard(&mut rx, |v| state(v, "echo", "stopped"));
    let ended = stopped.iter().find(|v| v["msg"]["method"] == "codeArena/ended").expect("told");
    assert_eq!(ended["msg"]["params"]["stopped"], true);
    assert_eq!(servers.log("echo")["running"], false);
    assert!(servers.running().is_empty());
}

#[cfg(unix)]
#[test]
fn a_server_that_ends_by_itself_is_started_again_three_times_in_three_minutes() {
    let d = tempfile::tempdir().expect("temp");
    let body = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
    // It answers initialize, says a line on its standard error, and ends.
    let script = format!(
        "read -r x; printf 'Content-Length: {}\\r\\n\\r\\n%s' '{body}'; echo oops >&2; sleep 0.2; exit 3",
        body.len()
    );
    let s = setup(d.path(), json!({"once": {"command": ["sh", "-c", script], "languages": ["plaintext"]}}), true, &[]);
    let (out, mut rx) = broadcast::channel(256);
    let servers = Arc::new(Servers::default());
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}});
    for n in 1..=4 {
        servers.start("once", None, s.clone(), &out);
        servers.relay("once", None, init.clone(), &out);
        let said = heard(&mut rx, |v| v["msg"]["method"] == "codeArena/ended");
        assert!(said.iter().any(|v| v["msg"]["result"] == json!({})), "the answer reached the window");
        let p = &said.last().expect("ended")["msg"]["params"];
        assert_eq!(p["exitCode"], 3);
        assert!(p["reason"].as_str().expect("reason").ends_with("(exit code 3). oops"), "{p}");
        assert_eq!(p["restart"], n <= 3, "start {n}");
        let st = heard(&mut rx, |v| v["type"] == "lsp_state");
        assert_eq!(st[0]["state"], if n <= 3 { "stopped" } else { "failed" });
    }
    assert_eq!(servers.seen("once").state, "failed");
    assert_eq!(servers.log("once")["lines"], json!(["oops"]));
    assert_eq!(servers.log("once")["exit"], 3);
    // Restart forgets the count; one that never answered initialize is not started again.
    let s =
        setup(d.path(), json!({"once": {"command": ["sh", "-c", "exit 2"], "languages": ["plaintext"]}}), true, &[]);
    servers.restart("once", None, s, &out);
    let said = heard(&mut rx, |v| v["msg"]["method"] == "codeArena/ended");
    assert_eq!(said.last().expect("ended")["msg"]["params"]["restart"], false);
}

#[cfg(unix)]
#[test]
fn a_start_is_refused_untrusted_unknown_off_missing_or_past_six() {
    let d = tempfile::tempdir().expect("temp");
    let (out, mut rx) = broadcast::channel(256);
    let servers = Arc::new(Servers::default());
    let cat = json!({"command": ["cat"], "languages": ["plaintext"]});
    let setting = json!({"a": cat, "b": cat, "c": cat, "d": cat, "e": cat, "f": cat, "g": cat,
        "nowhere": {"command": ["/no/such/server"], "languages": ["x"]}});
    let refused = |servers: &Arc<Servers>, rx: &mut broadcast::Receiver<String>, id: &str, s: Setup| {
        servers.start(id, None, s, &out);
        heard(rx, |v| v["type"] == "lsp_state").remove(0)
    };
    let r = refused(&servers, &mut rx, "a", setup(d.path(), setting.clone(), false, &[]));
    assert_eq!((&r["kind"], &r["state"]), (&json!("not_trusted"), &json!("restricted")));
    assert!(r["error"].as_str().expect("error").contains("trust the folder"));
    assert_eq!(servers.log("a")["refused"], Value::Null, "a refusal for the trust is not the server's");
    let r = refused(&servers, &mut rx, "zzz", setup(d.path(), setting.clone(), true, &[]));
    assert_eq!(r["kind"], "missing");
    let r = refused(&servers, &mut rx, "a", setup(d.path(), setting.clone(), true, &["a"]));
    assert_eq!((&r["kind"], &r["state"]), (&json!("off"), &json!("off")));
    let r = refused(&servers, &mut rx, "nowhere", setup(d.path(), setting.clone(), true, &[]));
    assert_eq!(r["kind"], "missing");
    assert!(r["install"].as_str().expect("install").starts_with("Give its command"));
    assert!(servers.log("nowhere")["refused"].as_str().expect("kept").contains("is not there"));
    for id in ["a", "b", "c", "d", "e", "f"] {
        let r = refused(&servers, &mut rx, id, setup(d.path(), setting.clone(), true, &[]));
        assert_eq!(r["state"], "running", "{id}: {r}");
    }
    let r = refused(&servers, &mut rx, "g", setup(d.path(), setting.clone(), true, &[]));
    assert_eq!(r["kind"], "too_many");
    // The folder's trust taken back: every one stops, its window told.
    servers.stop_all("The folder is no longer trusted.", None);
    let mut ended = 0;
    heard(&mut rx, |v| {
        if v["msg"]["method"] == "codeArena/ended" {
            assert_eq!(v["msg"]["params"]["reason"], "The folder is no longer trusted.");
            ended += 1;
        }
        ended == 6
    });
    assert!(servers.running().is_empty());
}

#[test]
fn frames_are_read_whole_from_any_pieces_and_what_is_not_the_protocol_is_said() {
    /// A reader that gives one byte at a time.
    struct Slow<'a>(&'a [u8]);
    impl Read for Slow<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match (self.0.split_first(), buf.is_empty()) {
                (Some((b, rest)), false) => {
                    buf[0] = *b;
                    self.0 = rest;
                    Ok(1)
                }
                _ => Ok(0),
            }
        }
    }
    let mut bytes = Vec::new();
    write_frame(&mut bytes, r#"{"a":1}"#).expect("w");
    bytes.extend_from_slice(b"Content-Type: x\r\ncontent-length: 4\r\n\r\nnope");
    write_frame(&mut bytes, "{\"b\":\"\u{e9}\"}").expect("w");
    let mut r = BufReader::with_capacity(1, Slow(&bytes));
    assert_eq!(read_frame(&mut r, 100).expect("a"), Some(json!({"a": 1})));
    assert_eq!(
        read_frame(&mut r, 100).expect("b"),
        Some(json!({"b": "\u{e9}"})),
        "a body that is not JSON is passed over"
    );
    assert_eq!(read_frame(&mut r, 100).expect("end"), None);
    let bad = |b: &[u8]| read_frame(&mut BufReader::new(b), 100).expect_err("bad").kind();
    assert_eq!(bad(b"garbage\r\n\r\n"), std::io::ErrorKind::InvalidData);
    assert_eq!(bad(b"X: 1\r\n\r\n{}"), std::io::ErrorKind::InvalidData);
    assert_eq!(bad(b"Content-Length: 1000\r\n\r\n{}"), std::io::ErrorKind::InvalidData);
}

#[test]
fn the_listing_says_each_servers_state() {
    let d = tempfile::tempdir().expect("temp");
    let servers = Servers::default();
    let setting = json!({"lua": {"command": ["/no/lua"], "languages": ["lua"]}, "bash": {"disabled": true}});
    let s = setup(d.path(), setting.clone(), false, &["yaml"]);
    let s = Setup { env: Vec::new(), ..s };
    let items = list(&servers, &s, &setting, &["yaml".to_string()], false);
    assert_eq!(items[0]["id"], "lua");
    assert_eq!(items.len(), 1 + catalog::CATALOG.len());
    let by = |id: &str| items.iter().find(|i| i["id"] == id).expect("item").clone();
    assert_eq!((by("lua")["state"].clone(), by("lua")["found"].clone()), (json!("missing"), json!(false)));
    assert_eq!(by("yaml")["state"], "off");
    assert_eq!(by("bash")["install"], Value::Null, "one turned off is not offered to install");
    assert!(by("gopls")["install"].as_str().expect("hint").starts_with("go install"));
    assert_eq!(by("css")["languages"], json!(["css", "scss", "less"]));
    assert!(named("rust-analyzer") && named("csharp-ls") && !named("../x") && !named(""));
}

#[test]
fn initialize_is_given_the_folder_and_the_options() {
    let found = catalog::Found {
        name: "x".into(),
        program: "/bin/x".into(),
        args: vec![],
        env: vec![],
        options: json!({"o": 1}).as_object().cloned().expect("o"),
        settings: Default::default(),
        given: true,
    };
    let mut m = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize"});
    relay::initialize(&mut m, Path::new("/w/proj"), &found, true);
    assert_eq!(m["params"]["processId"], Value::Null, "a sandboxed server is given no process id");
    assert_eq!(m["params"]["workspaceFolders"], json!([{"uri": "file:///w/proj", "name": "proj"}]));
    assert_eq!(m["params"]["initializationOptions"], json!({"o": 1}));
}
