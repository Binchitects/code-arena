//! The language servers running: each one a process of its own, its JSON-RPC messages relayed both ways between
//! it and the window, framed as the Language Server Protocol frames them (`Content-Length`) on its standard input
//! and output, and its last 200 lines of standard error kept for its log.
//!
//! Each server id has a thread of its own that does what is asked of it in order (start, the window's messages,
//! stop), so a `didOpen` sent right after `lsp_start` reaches the server after its start, and a `didChange` after
//! the `didOpen` before it. Nothing here waits on the driver's loop.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use ca_tools::sandbox::Sandbox;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::catalog::{self, Found, Look, Versions};
use super::uris::{Uris, file_uri};
use crate::ide_ext::send;

/// The most servers running at once.
pub const MOST: usize = 6;

/// How long a server stopping has to end by itself (shutdown, then exit) before it is killed.
pub const STOP_WAIT: Duration = Duration::from_secs(2);

/// The lines of a server's standard error kept for its log.
pub const LOG_LINES: usize = 200;

/// How often a server that ends by itself is started again, and in how long.
pub const RESTARTS: (usize, Duration) = (3, Duration::from_secs(180));

/// The most a server sends in one message.
const MOST_FROM_SERVER: usize = 64 * 1024 * 1024;

/// The most the window sends in one message (a file of 5 MB opened, and then some).
const MOST_FROM_WINDOW: usize = 16 * 1024 * 1024;

/// The messages that may wait for the faces before a server is made to wait: the faces' queue holds 4096.
const WAITING: usize = 2048;

/// The id of the shutdown request the driver sends a server it stops.
const SHUTDOWN_ID: &str = "codeArena/shutdown";

/// What a window is told when another took its server over.
pub const TAKEN_OVER: &str = "Another window took this language server over.";

/// What is asked once a start is to be done, off the driver's loop: the settings, the folder's trust, the sandbox.
pub struct Asked {
    /// The `languageServers` setting.
    pub setting: Value,
    /// The ids the `codeArena.languageServers.*` settings turn off.
    pub off: Vec<String>,
    pub trusted: bool,
    /// The sandbox the servers run in (none: as the driver), or why there must be one and is none.
    pub sandbox: Result<Option<Sandbox>, String>,
}

/// The folder the servers are for, and how what a start needs is asked.
#[derive(Clone)]
pub struct Setup {
    pub root: PathBuf,
    pub data: PathBuf,
    pub home: Option<PathBuf>,
    /// The environment's harmless names, passed on to the servers.
    pub env: Vec<(OsString, OsString)>,
    pub ask: Arc<dyn Fn() -> Asked + Send + Sync>,
}

impl Setup {
    fn look(&self) -> Look<'_> {
        Look { root: &self.root, home: self.home.as_deref(), env: &self.env }
    }
}

/// What a server's thread is asked to do.
enum Cmd {
    Start {
        client: Option<String>,
        setup: Setup,
        restart: bool,
    },
    Send {
        client: Option<String>,
        msg: Value,
    },
    Stop {
        why: String,
        reply: bool,
        client: Option<String>,
    },
    /// Its process's output ended (the run of this number).
    Ended {
        run: u64,
        error: Option<String>,
    },
}

/// Every server's thread, and what the listing and the log read of them.
#[derive(Default)]
pub struct Servers {
    inner: Mutex<Inner>,
    pub versions: Versions,
}

#[derive(Default)]
struct Inner {
    threads: HashMap<String, mpsc::Sender<Cmd>>,
    info: HashMap<String, Info>,
}

/// What is known of one server.
#[derive(Default)]
struct Info {
    /// Its window and folder while it runs.
    running: Option<(Option<String>, PathBuf)>,
    /// The standard error of the one running, else of the last that ran.
    said: Option<Arc<Mutex<Said>>>,
    exit: Option<i32>,
    /// Why its last start was refused, until one starts.
    refused: Option<String>,
    /// Why it ended by itself and is not started again.
    failed: Option<String>,
    /// When it ended by itself lately.
    crashes: Vec<Instant>,
}

/// The last lines a server wrote to standard error.
#[derive(Default)]
pub struct Said(VecDeque<String>);

impl Said {
    fn push(&mut self, line: String) {
        if self.0.len() == LOG_LINES {
            self.0.pop_front();
        }
        self.0.push_back(line);
    }

    fn lines(&self) -> Vec<String> {
        self.0.iter().cloned().collect()
    }
}

/// One server as the listing shows it.
pub struct Seen {
    /// running, stopped or failed.
    pub state: &'static str,
    pub client: Option<String>,
    pub failed: Option<String>,
}

impl Servers {
    fn thread(self: &Arc<Self>, id: &str, out: &broadcast::Sender<String>) -> mpsc::Sender<Cmd> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = inner.threads.get(id) {
            return tx.clone();
        }
        let (tx, rx) = mpsc::channel();
        inner.threads.insert(id.to_string(), tx.clone());
        let (me, id, out, back) = (self.clone(), id.to_string(), out.clone(), tx.clone());
        std::thread::Builder::new()
            .name(format!("lsp {id}"))
            .spawn(move || Worker { servers: me, id, out, tx: back, run: None, number: 0 }.serve(rx))
            .ok();
        tx
    }

    /// Starts a server for a window (or answers that it runs). Another window's server of the same id is taken
    /// over: that window is told, and its server stopped.
    pub fn start(self: &Arc<Self>, id: &str, client: Option<String>, setup: Setup, out: &broadcast::Sender<String>) {
        let _ = self.thread(id, out).send(Cmd::Start { client, setup, restart: false });
    }

    /// Stops a server and starts it again, its count of ends forgotten.
    pub fn restart(self: &Arc<Self>, id: &str, client: Option<String>, setup: Setup, out: &broadcast::Sender<String>) {
        self.with(id, |i| i.crashes.clear());
        let _ = self.thread(id, out).send(Cmd::Start { client, setup, restart: true });
    }

    /// Stops a server; `reply`: its `lsp_state` is said.
    pub fn stop(self: &Arc<Self>, id: &str, client: Option<String>, out: &broadcast::Sender<String>) {
        let _ = self.thread(id, out).send(Cmd::Stop { why: "Stopped.".into(), reply: true, client });
    }

    /// A window's message for a server; a request to one that does not run is answered with an error.
    pub fn relay(self: &Arc<Self>, id: &str, client: Option<String>, msg: Value, out: &broadcast::Sender<String>) {
        let tx = self.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get(id).cloned();
        match tx {
            Some(tx) => {
                let _ = tx.send(Cmd::Send { client, msg });
            }
            None => not_running(out, id, client.as_deref(), &msg),
        }
    }

    /// Stops every server, or those of another folder than `keep`, their windows told why.
    pub fn stop_all(&self, why: &str, keep: Option<&Path>) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for (id, tx) in &inner.threads {
            let running = inner.info.get(id).and_then(|i| i.running.as_ref());
            if running.is_some_and(|(_, root)| keep != Some(root.as_path())) {
                let _ = tx.send(Cmd::Stop { why: why.to_string(), reply: false, client: None });
            }
        }
    }

    /// The ids running.
    pub fn running(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut ids: Vec<String> =
            inner.info.iter().filter(|(_, i)| i.running.is_some()).map(|(id, _)| id.clone()).collect();
        ids.sort();
        ids
    }

    pub fn seen(&self, id: &str) -> Seen {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let i = inner.info.get(id);
        let running = i.and_then(|i| i.running.as_ref());
        let failed = i.and_then(|i| i.failed.clone().or_else(|| i.refused.clone()));
        Seen {
            state: if running.is_some() {
                "running"
            } else if failed.is_some() {
                "failed"
            } else {
                "stopped"
            },
            client: running.and_then(|(c, _)| c.clone()),
            failed,
        }
    }

    /// A server's log: `{lines, exit, refused, running}` of the one running, else the last that ran.
    pub fn log(&self, id: &str) -> Value {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let i = inner.info.get(id);
        let lines = i.and_then(|i| i.said.as_ref()).map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).lines());
        json!({
            "lines": lines.unwrap_or_default(),
            "exit": i.and_then(|i| i.exit),
            "refused": i.and_then(|i| i.refused.clone()),
            "running": i.is_some_and(|i| i.running.is_some()),
        })
    }

    fn with<T>(&self, id: &str, f: impl FnOnce(&mut Info) -> T) -> T {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(inner.info.entry(id.to_string()).or_default())
    }
}

/// A request to a server that does not run, answered as the protocol answers a failed one; a notification goes
/// nowhere.
fn not_running(out: &broadcast::Sender<String>, id: &str, client: Option<&str>, msg: &Value) {
    if msg.get("method").is_some() && msg.get("id").is_some_and(|i| !i.is_null()) {
        let error = json!({"code": -32803, "message": format!("The language server {id} is not running.")});
        let reply = json!({"jsonrpc": "2.0", "id": msg["id"], "error": error});
        send(out, json!({"type": "lsp_msg", "server": id, "client": client, "msg": reply}));
    }
}

/// One server's thread.
struct Worker {
    servers: Arc<Servers>,
    id: String,
    out: broadcast::Sender<String>,
    tx: mpsc::Sender<Cmd>,
    run: Option<Run>,
    number: u64,
}

/// One server running.
struct Run {
    number: u64,
    child: Child,
    to: mpsc::Sender<Option<Value>>,
    uris: Arc<Uris>,
    client: Option<String>,
    root: PathBuf,
    found: Found,
    sandboxed: bool,
    /// It answered initialize.
    ready: Arc<AtomicBool>,
    initialize: Arc<Mutex<Option<Value>>>,
    stopping: Arc<AtomicBool>,
    shut_down: Arc<AtomicBool>,
    said: Arc<Mutex<Said>>,
}

/// Why a start was refused: the kind the window tells apart, and the words.
struct Refusal {
    kind: &'static str,
    state: &'static str,
    error: String,
    install: Option<String>,
}

impl Worker {
    fn serve(mut self, rx: mpsc::Receiver<Cmd>) {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Start { client, setup, restart } => self.start(client, setup, restart),
                Cmd::Send { client, msg } => self.relay(client, msg),
                Cmd::Stop { why, reply, client } => {
                    let was = self.run.as_ref().map(|r| r.client.clone());
                    if self.run.is_some() {
                        self.stop(&why, false);
                    }
                    if reply {
                        self.state(json!({"state": "stopped"}), was.unwrap_or(client).as_deref());
                    }
                }
                Cmd::Ended { run, error } => {
                    if self.run.as_ref().is_some_and(|r| r.number == run) {
                        self.ended(error);
                    }
                }
            }
        }
    }

    /// Says this server's `lsp_state` to the faces.
    fn state(&self, mut v: Value, client: Option<&str>) {
        v["type"] = json!("lsp_state");
        v["server"] = json!(self.id);
        v["client"] = json!(client);
        send(&self.out, v);
    }

    fn start(&mut self, client: Option<String>, setup: Setup, restart: bool) {
        if let Some(run) = &self.run {
            let same = run.client == client && run.root == setup.root;
            if same && !restart {
                let v = json!({"state": "running", "pid": run.child.id(), "sandboxed": run.sandboxed, "path": run.found.program.display().to_string()});
                self.state(v, client.as_deref());
                return;
            }
            let why = if run.client != client && run.root == setup.root { TAKEN_OVER } else { "Restarted." };
            self.stop(why, why == TAKEN_OVER);
        }
        match self.launch(client.clone(), &setup) {
            Ok(()) => {
                let run = self.run.as_ref().expect("started");
                let v = json!({"state": "running", "pid": run.child.id(), "sandboxed": run.sandboxed, "path": run.found.program.display().to_string()});
                self.state(v, client.as_deref());
            }
            Err(r) => {
                if r.kind != "not_trusted" {
                    self.servers.with(&self.id, |i| i.refused = Some(r.error.clone()));
                }
                let v = json!({"state": r.state, "kind": r.kind, "error": r.error, "install": r.install});
                self.state(v, client.as_deref());
            }
        }
    }

    fn launch(&mut self, client: Option<String>, setup: &Setup) -> Result<(), Refusal> {
        let refuse = |kind, state, error: String| Refusal { kind, state, error, install: None };
        let asked = (setup.ask)();
        if !asked.trusted {
            // As VS Code's Restricted Mode: a language server runs the folder's code (tsserver plugins, build
            // scripts, go list).
            return Err(refuse(
                "not_trusted",
                "restricted",
                "Language servers run this folder's code: trust the folder to start them.".into(),
            ));
        }
        let entries = catalog::entries(&asked.setting);
        let Some(entry) = entries.iter().find(|e| e.id == self.id) else {
            return Err(refuse("missing", "missing", format!("There is no language server \"{}\".", self.id)));
        };
        if asked.off.contains(&self.id) {
            return Err(refuse(
                "off",
                "off",
                format!("Turned off in the settings (codeArena.languageServers.{}.enabled).", self.id),
            ));
        }
        let found = catalog::find(entry, &setup.look()).map_err(|error| Refusal {
            kind: "missing",
            state: "missing",
            install: (!entry.own.as_ref().is_some_and(|o| o.disabled)).then(|| catalog::install(&self.id)),
            error,
        })?;
        if self.servers.running().iter().filter(|r| **r != self.id).count() >= MOST {
            return Err(refuse(
                "too_many",
                "failed",
                format!("{MOST} language servers are running: stop one first (Stop Language Server)."),
            ));
        }
        let sandbox = asked.sandbox.map_err(|e| refuse("failed", "failed", e))?;
        let mut child = spawn(&found, &setup.root, &setup.env, sandbox.as_ref())
            .map_err(|e| refuse("failed", "failed", format!("{} could not be started: {e}", found.name)))?;
        self.number += 1;
        let number = self.number;
        let said = Arc::new(Mutex::new(Said::default()));
        let (to, written) = mpsc::channel::<Option<Value>>();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let name = found.name.clone();
        let said_w = said.clone();
        std::thread::spawn(move || write_all(stdin, written, &name, &said_w));
        if let Some(err) = child.stderr.take() {
            let said = said.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(err).split(b'\n').map_while(Result::ok) {
                    let line = String::from_utf8_lossy(&line).trim_end_matches('\r').to_string();
                    said.lock().unwrap_or_else(|e| e.into_inner()).push(line);
                }
            });
        }
        let run = Run {
            number,
            to,
            uris: Arc::new(Uris::new(&setup.root)),
            client: client.clone(),
            root: setup.root.clone(),
            sandboxed: sandbox.is_some(),
            ready: Arc::default(),
            initialize: Arc::default(),
            stopping: Arc::default(),
            shut_down: Arc::default(),
            said: said.clone(),
            found,
            child,
        };
        if let Some(stdout) = stdout {
            let reading = Reading {
                id: self.id.clone(),
                client,
                out: self.out.clone(),
                tx: self.tx.clone(),
                number,
                uris: run.uris.clone(),
                ready: run.ready.clone(),
                initialize: run.initialize.clone(),
                stopping: run.stopping.clone(),
                shut_down: run.shut_down.clone(),
            };
            std::thread::spawn(move || reading.read(stdout));
        }
        self.servers.with(&self.id, |i| {
            i.running = Some((run.client.clone(), run.root.clone()));
            i.said = Some(said);
            i.exit = None;
            i.refused = None;
            i.failed = None;
        });
        self.run = Some(run);
        Ok(())
    }

    /// A window's message for the server: its URIs as the server's, and an initialize given what the driver
    /// decides.
    fn relay(&mut self, client: Option<String>, mut msg: Value) {
        let Some(run) = self.run.as_ref().filter(|r| r.client == client) else {
            return not_running(&self.out, &self.id, client.as_deref(), &msg);
        };
        if !msg.is_object() {
            return;
        }
        run.uris.to_server(&mut msg);
        if msg["method"] == "initialize" && msg.get("id").is_some() {
            *run.initialize.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg["id"].clone());
            initialize(&mut msg, &run.root, &run.found, run.sandboxed);
        }
        let _ = run.to.send(Some(msg));
    }

    /// Stops the server, as a client that is done does: shutdown, then exit; whatever is left 2 s later (at least
    /// 1 s after exit) is killed with every process it started. Its window is told why, last.
    fn stop(&mut self, why: &str, taken: bool) {
        let Some(mut run) = self.run.take() else { return };
        run.stopping.store(true, Ordering::SeqCst);
        let clock = Instant::now();
        let left = || STOP_WAIT.saturating_sub(clock.elapsed());
        let exited = |run: &mut Run| run.child.try_wait().ok().flatten().is_some();
        if !exited(&mut run) {
            let _ = run.to.send(Some(json!({"jsonrpc": "2.0", "id": SHUTDOWN_ID, "method": "shutdown"})));
            wait(left(), || run.shut_down.load(Ordering::SeqCst));
            let _ = run.to.send(Some(json!({"jsonrpc": "2.0", "method": "exit"})));
            let _ = run.to.send(None);
            wait(left().max(Duration::from_secs(1)), || exited(&mut run));
        }
        let code = finish(&mut run.child);
        self.servers.with(&self.id, |i| {
            i.running = None;
            i.exit = code;
        });
        self.ended_said(&run, why, code, true, taken, false);
    }

    /// Its output ended without being stopped: it ended by itself, or wrote what is not the protocol.
    fn ended(&mut self, error: Option<String>) {
        let Some(mut run) = self.run.take() else { return };
        let _ = run.to.send(None);
        wait(Duration::from_secs(1), || run.child.try_wait().ok().flatten().is_some());
        let code = finish(&mut run.child);
        let last: Vec<String> = {
            let said = run.said.lock().unwrap_or_else(|e| e.into_inner()).lines();
            said.into_iter()
                .rev()
                .filter(|l| !l.trim().is_empty())
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        };
        let mut reason = error.unwrap_or_else(|| match code {
            Some(c) => format!("The language server ended (exit code {c})."),
            None => "The language server ended.".into(),
        });
        if !last.is_empty() {
            reason = format!("{reason} {}", last.join(" "));
        }
        let ready = run.ready.load(Ordering::SeqCst);
        let again = self.servers.with(&self.id, |i| {
            let now = Instant::now();
            i.crashes.retain(|t| now.duration_since(*t) < RESTARTS.1);
            i.crashes.push(now);
            i.running = None;
            i.exit = code;
            let again = ready && i.crashes.len() <= RESTARTS.0;
            if !again {
                i.failed = Some(reason.clone());
            }
            again
        });
        self.ended_said(&run, &reason, code, false, false, again);
    }

    /// `codeArena/ended` for the server's window, and its `lsp_state` for every face.
    fn ended_said(&self, run: &Run, reason: &str, code: Option<i32>, stopped: bool, taken: bool, again: bool) {
        let params =
            json!({"reason": reason, "exitCode": code, "takenOver": taken, "stopped": stopped, "restart": again});
        let msg = json!({"jsonrpc": "2.0", "method": "codeArena/ended", "params": params});
        send(&self.out, json!({"type": "lsp_msg", "server": self.id, "client": run.client, "msg": msg}));
        let state = match () {
            _ if taken => "taken",
            _ if stopped || again => "stopped",
            _ => "failed",
        };
        let v = json!({"state": state, "error": (!stopped).then_some(reason), "exit": code, "restart": again});
        self.state(v, run.client.as_deref());
    }
}

/// Waits until `done` or the time is up.
fn wait(limit: Duration, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + limit;
    while !done() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Kills what is left of a server (its process group, everything it started) and says its exit code.
fn finish(child: &mut Child) -> Option<i32> {
    if child.try_wait().ok().flatten().is_none() {
        #[cfg(unix)]
        let _ =
            nix::sys::signal::killpg(nix::unistd::Pid::from_raw(child.id() as i32), nix::sys::signal::Signal::SIGKILL);
        let _ = child.kill();
    }
    child.wait().ok().and_then(|s| s.code())
}

/// What the driver decides of initialize: the folder, the process id, the options; the window chooses none.
pub fn initialize(msg: &mut Value, root: &Path, found: &Found, sandboxed: bool) {
    if !msg["params"].is_object() {
        msg["params"] = json!({});
    }
    let uri = file_uri(root);
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
    let p = &mut msg["params"];
    // In the sandbox's own process namespace the driver's process id means nothing: a server that watches it would
    // end at once.
    p["processId"] = if sandboxed { Value::Null } else { json!(std::process::id()) };
    p["rootUri"] = json!(uri);
    p["rootPath"] = json!(root.display().to_string());
    p["workspaceFolders"] = json!([{"uri": uri, "name": name}]);
    p["initializationOptions"] = Value::Object(found.options.clone());
}

/// Starts a server's process in the folder: in the sandbox when there is one (the folder and the build tools'
/// caches writable, the secrets hidden, no network), with the environment's harmless names and what the server
/// adds, in a process group of its own.
fn spawn(found: &Found, root: &Path, env: &[(OsString, OsString)], sandbox: Option<&Sandbox>) -> io::Result<Child> {
    let program = found.program.display().to_string();
    let (program, args) = match sandbox {
        Some(s) => {
            let mut s = s.clone();
            s.network = false;
            let mut readable: Vec<PathBuf> = vec![root.to_path_buf()];
            for a in std::iter::once(&program).chain(&found.args) {
                let p = Path::new(a);
                if p.is_absolute()
                    && p.is_file()
                    && let Some(dir) = p.parent()
                {
                    readable.push(dir.to_path_buf());
                }
            }
            s.wrap_reading(&program, &found.args, root, &readable)
        }
        None => (program, found.args.clone()),
    };
    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .current_dir(root)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .env("NO_COLOR", "1")
        .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
        .env("DOTNET_NOLOGO", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in &found.env {
        match value {
            Some(v) => cmd.env(name, v),
            None => cmd.env_remove(name),
        };
    }
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd.spawn()
}

/// Writes the window's messages to the server, framed, until told to close its input.
fn write_all(
    stdin: Option<std::process::ChildStdin>,
    rx: mpsc::Receiver<Option<Value>>,
    name: &str,
    said: &Mutex<Said>,
) {
    let Some(mut stdin) = stdin else { return };
    while let Ok(Some(msg)) = rx.recv() {
        let body = msg.to_string();
        if body.len() > MOST_FROM_WINDOW {
            let line =
                format!("[Code Arena] A message of {} MB for {name} was not sent: at most 16 MB go.", body.len() >> 20);
            said.lock().unwrap_or_else(|e| e.into_inner()).push(line);
            continue;
        }
        if write_frame(&mut stdin, &body).is_err() {
            // The server is gone: its reader ends the run, and tells the window.
            break;
        }
    }
}

/// One message, framed.
pub fn write_frame(w: &mut impl Write, body: &str) -> io::Result<()> {
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(body.as_bytes())?;
    w.flush()
}

/// The next message of a server's, or None at the end of its output. A frame that is not the protocol's is an
/// error of kind `InvalidData`; a body that is not JSON is skipped.
pub fn read_frame(r: &mut impl BufRead, most: usize) -> io::Result<Option<Value>> {
    loop {
        let mut length: Option<usize> = None;
        let mut any = false;
        loop {
            let mut line = Vec::new();
            if r.by_ref().take(8192).read_until(b'\n', &mut line)? == 0 {
                return if any {
                    Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the output ended in a header"))
                } else {
                    Ok(None)
                };
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end_matches(['\r', '\n']);
            if text.is_empty() {
                if any {
                    break;
                }
                continue;
            }
            any = true;
            let (name, value) = text.split_once(':').ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, format!("a header that is not one: {text}"))
            })?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok();
            }
        }
        let length =
            length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a message without Content-Length"))?;
        if length > most {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("a message of {} MB, past {} MB", length >> 20, most >> 20),
            ));
        }
        let mut body = vec![0; length];
        r.read_exact(&mut body)?;
        if let Ok(v) = serde_json::from_slice::<Value>(&body) {
            return Ok(Some(v));
        }
    }
}

/// What reads a server's output.
struct Reading {
    id: String,
    client: Option<String>,
    out: broadcast::Sender<String>,
    tx: mpsc::Sender<Cmd>,
    number: u64,
    uris: Arc<Uris>,
    ready: Arc<AtomicBool>,
    initialize: Arc<Mutex<Option<Value>>>,
    stopping: Arc<AtomicBool>,
    shut_down: Arc<AtomicBool>,
}

impl Reading {
    fn read(self, stdout: impl Read) {
        let mut r = BufReader::new(stdout);
        let error = loop {
            match read_frame(&mut r, MOST_FROM_SERVER) {
                Ok(Some(mut msg)) => {
                    if msg["id"] == SHUTDOWN_ID && msg.get("method").is_none() {
                        self.shut_down.store(true, Ordering::SeqCst);
                        continue;
                    }
                    if self.stopping.load(Ordering::SeqCst) {
                        // What it says while it stops goes nowhere: its window is gone, or is being told.
                        continue;
                    }
                    if msg.get("method").is_none()
                        && msg.get("result").is_some()
                        && self.initialize.lock().unwrap_or_else(|e| e.into_inner()).as_ref() == msg.get("id")
                    {
                        self.ready.store(true, Ordering::SeqCst);
                    }
                    self.uris.to_window(&mut msg);
                    // The faces' queue is not let overflow: a face that falls behind loses messages.
                    wait(Duration::from_secs(10), || self.out.len() < WAITING);
                    send(&self.out, json!({"type": "lsp_msg", "server": self.id, "client": self.client, "msg": msg}));
                }
                Ok(None) => break None,
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    break Some(format!("The language server sent what is not the Language Server Protocol: {e}."));
                }
                Err(_) => break None,
            }
        };
        let _ = self.tx.send(Cmd::Ended { run: self.number, error });
    }
}
