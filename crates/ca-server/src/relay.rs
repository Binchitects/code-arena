//! The relay (`code-arena ide --connect`): this folder in Arena's Code Studio. Code Arena dials Arena's
//! `/api/code-arena/connect` (a WebSocket, with the person's API key) and Code Studio's IDE reaches this run through it,
//! as a page on this machine would: its stream (`/ws`) is joined to the session in-process, message by message, so
//! the run's own key never leaves the machine and nothing else of this server is reached. Only the person signed in
//! to Arena as the key's owner reaches it, and only while this runs. A lost connection is dialled again, after a
//! second and then twice as long each time, up to 30 seconds; the folder keeps its id (kept beside its state), so it
//! keeps its place in Code Studio between runs. Deleting it there ends the relay; a refusal (the key, the
//! permission) ends it too.
//!
//! The frames are Arena's (MachineTunnels), JSON text each:
//! ```text
//! to here:    {"t":"req","id":1,"m":"GET","p":"/api/state","h":{...},"b":"base64"}   {"t":"cancel","id":1}
//!             {"t":"ws","id":2,"p":"/ws"}   {"t":"wsd","id":2,"x":"text"}   {"t":"wsc","id":2}
//! from here:  {"t":"res","id":1,"s":404,"h":{...}}   {"t":"end","id":1}   {"t":"err","id":1,"e":"why"}
//!             {"t":"wsok","id":2}   {"t":"wsd","id":2,"x":"text"}   {"t":"wsc","id":2}
//! ```
//! This server has no calls besides its stream, so every request is answered "Not here" (404).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error, Message};

use super::{MOST_MESSAGE, Shared};

/// The first wait before dialling again; it doubles each time, up to [`MOST_WAIT`].
pub const LEAST_WAIT: Duration = Duration::from_secs(1);

/// The longest wait before dialling again.
pub const MOST_WAIT: Duration = Duration::from_secs(30);

/// The longest name a folder has in Code Studio, in characters.
const MOST_NAME: usize = 200;

/// The way into this run's session, as a page connected to its server has it: what the relay's pages send goes to
/// the session with a page number of its own, and what every page is sent reaches them too.
#[derive(Clone)]
pub struct Local(pub(super) Arc<Shared>);

/// Where the relay dials and as whom.
#[derive(Clone)]
pub struct Dial {
    /// Arena's address, `https://DOMAIN`.
    pub arena: String,
    /// The person's API key: sent only in the dial's Authorization header, never shown.
    pub key: String,
    /// The folder's id in Code Studio: 32 hex digits, kept between runs ([`kept_id`]).
    pub id: String,
    /// The folder's name there: "FOLDER on MACHINE".
    pub name: String,
    /// The project of its git origin (`owner/repo`), when it has one.
    pub project: Option<String>,
    /// Its git branch, when it is on one.
    pub branch: Option<String>,
    /// The first wait before dialling again, and the longest.
    pub least: Duration,
    pub most: Duration,
}

impl std::fmt::Debug for Dial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dial").field("arena", &self.arena).field("id", &self.id).field("name", &self.name).finish()
    }
}

impl Dial {
    /// A dial to `arena` as the key's owner, with the usual waits; the name is cut to what Code Studio keeps.
    pub fn new(arena: &str, key: &str, id: &str, name: &str, project: Option<String>, branch: Option<String>) -> Dial {
        Dial {
            arena: arena.trim_end_matches('/').to_string(),
            key: key.to_string(),
            id: id.to_string(),
            name: name.chars().take(MOST_NAME).collect(),
            project: project.filter(|p| !p.is_empty()),
            branch: branch.filter(|b| !b.is_empty()),
            least: LEAST_WAIT,
            most: MOST_WAIT,
        }
    }

    /// The address dialled: Arena's connect route (`wss` for `https`), with the folder's id, name, project and branch.
    pub fn url(&self) -> String {
        let base = if let Some(rest) = self.arena.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.arena.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.arena.clone()
        };
        let mut q = format!("id={}&name={}", escape(&self.id), escape(&self.name));
        if let Some(p) = &self.project {
            q.push_str(&format!("&project={}", escape(p)));
        }
        if let Some(b) = &self.branch {
            q.push_str(&format!("&branch={}", escape(b)));
        }
        format!("{base}/api/code-arena/connect?{q}")
    }

    /// Where Code Studio is, for the person to open.
    pub fn studio(&self) -> String {
        format!("{}/code", self.arena)
    }
}

/// Why the relay stopped (it does not stop for a lost connection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// Arena refused the folder (its HTTP status): the key is not known there, Code Studio is not the person's, or
    /// the dial was wrong.
    Refused(u16),
    /// The folder was deleted in Code Studio.
    Deleted,
}

/// The wait after `wait`: twice as long, up to `most`.
pub fn next_wait(wait: Duration, most: Duration) -> Duration {
    wait.saturating_mul(2).min(most)
}

/// The text escaped for an address's query (all but the unreserved characters as `%XX`).
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The folder's id kept in `file`: the one there (32 hex digits), else a new one written there.
pub fn kept_id(file: &Path) -> String {
    if let Ok(text) = std::fs::read_to_string(file) {
        let t = text.trim();
        if t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return t.to_ascii_lowercase();
        }
    }
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        // No randomness to be had: an id from the time, still the folder's own while the file keeps it.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        b = now.to_le_bytes();
    }
    let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, &id);
    id
}

/// The project of a git config's `origin` (`owner/repo`, without `.git`), from its URL in either form
/// (`https://host/owner/repo.git`, `git@host:owner/repo.git`).
pub fn origin_project(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_origin = l.replace(' ', "") == "[remote\"origin\"]";
            continue;
        }
        if !in_origin {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        if k.trim() != "url" {
            continue;
        }
        let url = v.trim();
        let path = match url.split_once("://") {
            Some((_, rest)) => rest.split_once('/').map_or("", |(_, p)| p),
            None => url.split_once(':').map_or(url, |(_, p)| p),
        };
        let path = path.trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        return (!path.is_empty()).then(|| path.to_string());
    }
    None
}

/// The branch a git `HEAD` names (`ref: refs/heads/NAME`); none when it is detached.
pub fn head_branch(head: &str) -> Option<String> {
    head.trim().strip_prefix("ref: refs/heads/").map(str::to_string).filter(|b| !b.is_empty())
}

/// The folder's project and branch, from the git repository it is in (its own `.git` or a parent's).
pub fn git_facts(folder: &Path) -> (Option<String>, Option<String>) {
    let Some(root) = folder.ancestors().find(|d| d.join(".git").is_dir()) else {
        return (None, None);
    };
    let git = root.join(".git");
    let project = std::fs::read_to_string(git.join("config")).ok().as_deref().and_then(origin_project);
    let branch = std::fs::read_to_string(git.join("HEAD")).ok().as_deref().and_then(head_branch);
    (project, branch)
}

/// What the relay says to the person (connected, lost, refused), a line each.
pub type Say = Arc<dyn Fn(&str) + Send + Sync>;

/// The relay, dialled and dialled again, until Arena refuses it or the folder is deleted there.
pub async fn run(dial: Dial, local: Local, say: Say) -> Ended {
    let mut wait = dial.least;
    loop {
        let Ok(mut request) = dial.url().into_client_request() else {
            say("Arena's address is not one a WebSocket can dial: code-arena login --url https://YOUR-ARENA.");
            return Ended::Refused(400);
        };
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", dial.key)) {
            request.headers_mut().insert(AUTHORIZATION, v);
        }
        let config = WebSocketConfig::default().max_message_size(Some(MOST_MESSAGE));
        match tokio_tungstenite::connect_async_with_config(request, Some(config), true).await {
            Ok((ws, _)) => {
                wait = dial.least;
                say(&format!(
                    "Connected to Code Studio: open it at {}, this folder is \"{}\" there.",
                    dial.studio(),
                    dial.name
                ));
                match serve(ws, &local).await {
                    Some(why) if why.starts_with("Deleted") => {
                        say(&format!("Code Studio closed the connection: {why}"));
                        return Ended::Deleted;
                    }
                    Some(why) if !why.is_empty() => say(&format!("Code Studio closed the connection: {why}")),
                    _ => say("The connection to Code Studio was lost; connecting again."),
                }
                tokio::time::sleep(wait).await;
            }
            Err(Error::Http(r)) if matches!(r.status().as_u16(), 400 | 401 | 403 | 409) => {
                let status = r.status().as_u16();
                let why = match status {
                    403 => "Code Studio is not among what you may use",
                    401 => "your API key is not known there: code-arena login",
                    _ => "see Code Studio",
                };
                say(&format!("Arena refused to connect this folder to Code Studio (HTTP {status}): {why}."));
                return Ended::Refused(status);
            }
            Err(e) => {
                say(&format!(
                    "Code Studio is out of reach ({}); trying again in {} s.",
                    explain(&e),
                    wait.as_secs().max(1)
                ));
                tokio::time::sleep(wait).await;
                wait = next_wait(wait, dial.most);
            }
        }
    }
}

/// A dial's failure in a few words (an answer that is not a WebSocket's says its status).
fn explain(e: &Error) -> String {
    match e {
        Error::Http(r) => format!("HTTP {}", r.status().as_u16()),
        Error::Io(io) => io.to_string(),
        other => other.to_string(),
    }
}

/// One of Code Studio's streams, joined to the session as a page of its own.
struct Joined {
    /// Its page number, as the session knows it.
    page: u64,
    /// What every page is sent, carried to it.
    carry: JoinHandle<()>,
}

/// The frames from Arena until the connection ends; Arena's close reason, if it gave one. The streams opened end with
/// it, each said `bye` to the session.
async fn serve<S>(ws: WebSocketStream<S>, local: &Local) -> Option<String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::text(text)).await.is_err() {
                break;
            }
        }
    });
    let mut joined: HashMap<i64, Joined> = HashMap::new();
    let mut why = None;
    while let Some(Ok(m)) = stream.next().await {
        match m {
            Message::Text(t) => {
                let Ok(frame) = serde_json::from_str::<Value>(t.as_str()) else { continue };
                if let Some(id) = frame["id"].as_i64() {
                    frame_in(&frame, id, local, &tx, &mut joined);
                }
            }
            Message::Close(c) => {
                why = c.map(|c| c.reason.as_str().to_string());
                break;
            }
            _ => {}
        }
    }
    for (_, j) in joined.drain() {
        leave(local, j);
    }
    writer.abort();
    why
}

/// A stream ended: no more carried to it, and the session told the page left.
fn leave(local: &Local, j: Joined) {
    j.carry.abort();
    let _ = local.0.from_page.send((j.page, json!({"type": "bye"})));
}

/// One frame from Arena, answered.
fn frame_in(
    frame: &Value,
    id: i64,
    local: &Local,
    tx: &mpsc::UnboundedSender<String>,
    joined: &mut HashMap<i64, Joined>,
) {
    let send = |v: Value| {
        let _ = tx.send(v.to_string());
    };
    match frame["t"].as_str().unwrap_or("") {
        "req" => {
            send(json!({"t": "res", "id": id, "s": 404, "h": {"Content-Type": "text/plain; charset=utf-8"}}));
            send(json!({"t": "end", "id": id}));
        }
        "ws" => {
            let path = frame["p"].as_str().unwrap_or("/");
            if path.split('?').next() != Some("/ws") || joined.contains_key(&id) {
                send(json!({"t": "err", "id": id, "e": "Only the IDE's stream goes through."}));
                return;
            }
            let s = &local.0;
            let page = s.next.fetch_add(1, Ordering::SeqCst);
            let mut out = s.to_pages.subscribe();
            send(json!({"t": "wsok", "id": id}));
            let _ = s.from_page.send((page, json!({"type": "hello"})));
            let tx = tx.clone();
            let carry = tokio::spawn(async move {
                loop {
                    let text = match out.recv().await {
                        Ok(text) => text,
                        // Too far behind: the page asks for the whole state again.
                        Err(broadcast::error::RecvError::Lagged(_)) => json!({"type": "behind"}).to_string(),
                        Err(broadcast::error::RecvError::Closed) => {
                            let _ = tx.send(json!({"t": "wsc", "id": id}).to_string());
                            break;
                        }
                    };
                    if tx.send(json!({"t": "wsd", "id": id, "x": text}).to_string()).is_err() {
                        break;
                    }
                }
            });
            joined.insert(id, Joined { page, carry });
        }
        "wsd" => {
            // What a page sends, as the server takes it: text, not too big, JSON with its type.
            let (Some(j), Some(x)) = (joined.get(&id), frame["x"].as_str()) else { return };
            if x.len() > MOST_MESSAGE {
                return;
            }
            if let Ok(v) = serde_json::from_str::<Value>(x)
                && v.get("type").and_then(Value::as_str).is_some()
            {
                let _ = local.0.from_page.send((j.page, v));
            }
        }
        "wsc" => {
            if let Some(j) = joined.remove(&id) {
                leave(local, j);
            }
        }
        // Nothing here takes long enough to stop.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
    use tokio_tungstenite::tungstenite::http::StatusCode;

    #[test]
    fn the_wait_doubles_from_a_second_up_to_thirty() {
        let mut w = LEAST_WAIT;
        let mut seen = vec![w.as_secs()];
        for _ in 0..6 {
            w = next_wait(w, MOST_WAIT);
            seen.push(w.as_secs());
        }
        assert_eq!(seen, [1, 2, 4, 8, 16, 30, 30]);
    }

    #[test]
    fn the_address_is_arenas_connect_route_with_the_folder() {
        let d = Dial::new("https://arena.example/", "k", "ab", "app on box", Some("acme/app".into()), None);
        assert_eq!(d.url(), "wss://arena.example/api/code-arena/connect?id=ab&name=app%20on%20box&project=acme%2Fapp");
        assert_eq!(d.studio(), "https://arena.example/code");
        let d = Dial::new("http://127.0.0.1:9", "k", "ab", "x", None, Some("main".into()));
        assert_eq!(d.url(), "ws://127.0.0.1:9/api/code-arena/connect?id=ab&name=x&branch=main");
        let long = "é".repeat(300);
        assert_eq!(Dial::new("https://a", "k", "i", &long, None, None).name.chars().count(), 200);
        // The key is never shown.
        assert!(!format!("{:?}", Dial::new("https://a", "fake-key-1", "i", "n", None, None)).contains("fake-key-1"));
    }

    #[test]
    fn the_id_is_kept_and_a_broken_one_replaced() {
        let dir = std::env::temp_dir().join(format!("ca-relay-{}", std::process::id()));
        let file = dir.join("ide").join("f.studio");
        let _ = std::fs::remove_dir_all(&dir);
        let id = kept_id(&file);
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(kept_id(&file), id);
        std::fs::write(&file, "not an id").expect("write");
        let other = kept_id(&file);
        assert_ne!(other, id);
        assert_eq!(other.len(), 32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_project_and_branch_come_from_git() {
        let config = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://h/x/y.git\n[remote \"origin\"]\n\turl = git@github.com:acme/app.git\n";
        assert_eq!(origin_project(config).as_deref(), Some("acme/app"));
        assert_eq!(origin_project("[remote \"origin\"]\n url = https://h.example/a/b/c\n").as_deref(), Some("a/b/c"));
        assert_eq!(origin_project("[core]\n"), None);
        assert_eq!(head_branch("ref: refs/heads/feature/x\n").as_deref(), Some("feature/x"));
        assert_eq!(head_branch("0123abcd\n"), None);
    }

    /// A fake Arena: each connection's address and Authorization, and its WebSocket to drive.
    async fn arena() -> (u16, mpsc::UnboundedReceiver<(String, String, WebSocketStream<tokio::net::TcpStream>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((s, _)) = listener.accept().await {
                let seen = Arc::new(std::sync::Mutex::new((String::new(), String::new())));
                let keep = seen.clone();
                let cb = move |req: &Request, res: Response| -> Result<Response, ErrorResponse> {
                    let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
                    if let Ok(mut k) = keep.lock() {
                        *k = (req.uri().to_string(), auth.to_string());
                    }
                    Ok(res)
                };
                if let Ok(ws) = tokio_tungstenite::accept_hdr_async(s, cb).await {
                    let (uri, auth) = seen.lock().map(|k| k.clone()).unwrap_or_default();
                    let _ = tx.send((uri, auth, ws));
                }
            }
        });
        (port, rx)
    }

    fn dial(port: u16) -> Dial {
        let mut d = Dial::new(
            &format!("http://127.0.0.1:{port}"),
            "fake-key",
            "0123456789abcdef0123456789abcdef",
            "app on box",
            None,
            None,
        );
        d.least = Duration::from_millis(10);
        d.most = Duration::from_millis(40);
        d
    }

    async fn frame(ws: &mut WebSocketStream<tokio::net::TcpStream>) -> Value {
        loop {
            let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("in time")
                .expect("open")
                .expect("ok");
            if let Message::Text(t) = m {
                return serde_json::from_str(t.as_str()).expect("json");
            }
        }
    }

    async fn from_page(s: &mut super::super::Started) -> (u64, Value) {
        tokio::time::timeout(Duration::from_secs(5), s.from_page.recv()).await.expect("in time").expect("one")
    }

    #[tokio::test]
    async fn the_stream_goes_both_ways_and_a_lost_connection_is_dialled_again() {
        let mut s = super::super::start(0, &[]).await.expect("start");
        let (port, mut conns) = arena().await;
        let said: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let keep = said.clone();
        let say: Say = Arc::new(move |t: &str| keep.lock().expect("lock").push(t.to_string()));
        let relay = tokio::spawn(run(dial(port), s.local.clone(), say));

        let (uri, auth, mut ws) = conns.recv().await.expect("dialled");
        assert_eq!(auth, "Bearer fake-key");
        assert!(
            uri.starts_with("/api/code-arena/connect?id=0123456789abcdef0123456789abcdef&name=app%20on%20box"),
            "{uri}"
        );
        // A call: nothing here but the stream.
        ws.send(Message::text(r#"{"t":"req","id":1,"m":"GET","p":"/api/state"}"#)).await.expect("send");
        assert_eq!(frame(&mut ws).await["s"], 404);
        assert_eq!(frame(&mut ws).await, json!({"t": "end", "id": 1}));
        // Another socket than the stream: refused.
        ws.send(Message::text(r#"{"t":"ws","id":2,"p":"/api/terminals/socket"}"#)).await.expect("send");
        assert_eq!(frame(&mut ws).await["t"], "err");
        // The stream: a page of its own, both ways.
        ws.send(Message::text(r#"{"t":"ws","id":3,"p":"/ws"}"#)).await.expect("send");
        assert_eq!(frame(&mut ws).await, json!({"t": "wsok", "id": 3}));
        let (page, hello) = from_page(&mut s).await;
        assert_eq!(hello["type"], "hello");
        ws.send(Message::text(json!({"t": "wsd", "id": 3, "x": r#"{"type":"send","text":"hi"}"#}).to_string()))
            .await
            .expect("send");
        let (p, v) = from_page(&mut s).await;
        assert_eq!((p, v["text"].as_str()), (page, Some("hi")));
        // Without a type: not passed on.
        ws.send(Message::text(json!({"t": "wsd", "id": 3, "x": "[1]"}).to_string())).await.expect("send");
        s.to_pages.send(r#"{"type":"state"}"#.into()).expect("sent");
        assert_eq!(frame(&mut ws).await, json!({"t": "wsd", "id": 3, "x": r#"{"type":"state"}"#}));

        // Lost: the page leaves, and the same folder is dialled again.
        drop(ws);
        let (p, bye) = from_page(&mut s).await;
        assert_eq!((p, bye["type"].as_str()), (page, Some("bye")));
        let (again, _, ws) = conns.recv().await.expect("dialled again");
        assert!(again.contains("id=0123456789abcdef0123456789abcdef"), "{again}");
        // Deleted in Code Studio: the relay ends.
        let mut ws = ws;
        ws.close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
            code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
            reason: "Deleted in Code Studio.".into(),
        }))
        .await
        .expect("close");
        let ended = tokio::time::timeout(Duration::from_secs(5), relay).await.expect("in time").expect("ran");
        assert_eq!(ended, Ended::Deleted);
        let said = said.lock().expect("lock").clone();
        assert!(said[0].starts_with("Connected to Code Studio: open it at http://127.0.0.1:"), "{said:?}");
        assert!(said.iter().any(|l| l.contains("lost; connecting again")), "{said:?}");
        assert!(!said.iter().any(|l| l.contains("fake-key")), "{said:?}");
    }

    #[tokio::test]
    async fn a_refusal_ends_it_and_out_of_reach_is_tried_again() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            // The first dial finds nothing that answers as a WebSocket; the second is refused.
            let (first, _) = listener.accept().await.expect("first");
            drop(first);
            let (s, _) = listener.accept().await.expect("second");
            let cb = |_: &Request, _: Response| -> Result<Response, ErrorResponse> {
                let mut r = ErrorResponse::new(Some("no".into()));
                *r.status_mut() = StatusCode::UNAUTHORIZED;
                Err(r)
            };
            let _ = tokio_tungstenite::accept_hdr_async(s, cb).await;
        });
        let s = super::super::start(0, &[]).await.expect("start");
        let said: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let keep = said.clone();
        let say: Say = Arc::new(move |t: &str| keep.lock().expect("lock").push(t.to_string()));
        let ended =
            tokio::time::timeout(Duration::from_secs(5), run(dial(port), s.local.clone(), say)).await.expect("in time");
        assert_eq!(ended, Ended::Refused(401));
        let said = said.lock().expect("lock").clone();
        assert!(said[0].starts_with("Code Studio is out of reach"), "{said:?}");
        assert!(said[1].contains("(HTTP 401): your API key is not known there"), "{said:?}");
    }
}
