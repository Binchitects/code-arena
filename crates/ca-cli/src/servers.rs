//! The session's MCP servers: Arena's tools, Farzan Graph's, and the person's own (`mcpServers`). They connect in
//! the background; a session waits a few seconds for them, and one that answers later joins when it does. A server
//! that went away is connected again at the next call.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ca_agent::Joined;
use ca_core::Secret;
use ca_llm::ToolDef;
use ca_mcp::{Client, McpError, StdioSpec, ToolInfo};
use ca_settings::{env::expand, schema};
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::app::App;

/// How to reach one server.
#[derive(Clone)]
pub enum Spec {
    Http { url: String, key: Option<Secret>, headers: Vec<(String, Secret)>, ca: Option<PathBuf> },
    Stdio(StdioSpec),
}

impl Spec {
    async fn connect(&self) -> Result<Client, McpError> {
        let mut c = match self {
            Spec::Http { url, key, headers, ca } => Client::http(url, key.clone(), headers.clone(), ca.as_deref())?,
            Spec::Stdio(s) => Client::stdio(s.clone()).await?,
        };
        c.initialize().await?;
        Ok(c)
    }
}

/// Where a server stands, for `/mcp` and the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Connecting,
    Connected {
        tools: usize,
    },
    /// The Arena has no such endpoint: said once, quietly.
    Unavailable(String),
    Failed(String),
}

#[derive(Clone)]
pub struct Server {
    pub name: String,
    pub title: String,
    pub spec: Spec,
    /// Tools named `mcp__<name>__<tool>` (the person's own); Arena's and Farzan Graph's keep their names.
    pub prefixed: bool,
    pub trusted: bool,
    pub state: Arc<Mutex<State>>,
    conn: Arc<tokio::sync::Mutex<Option<Client>>>,
}

pub type Servers = Arc<Vec<Server>>;

impl Server {
    fn new(name: &str, title: &str, spec: Spec, prefixed: bool, trusted: bool) -> Self {
        Server {
            name: name.into(),
            title: title.into(),
            spec,
            prefixed,
            trusted,
            state: Arc::new(Mutex::new(State::Connecting)),
            conn: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    fn set(&self, s: State) {
        if let Ok(mut g) = self.state.lock() {
            *g = s;
        }
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or(State::Connecting)
    }

    /// Connects (again) and lists the tools.
    async fn open(&self) -> Result<(Vec<ToolInfo>, Option<String>), McpError> {
        let mut c = self.spec.connect().await?;
        let tools = c.list_tools().await?;
        let instructions = c.instructions.clone();
        *self.conn.lock().await = Some(c);
        Ok((tools, instructions))
    }

    /// A call, connecting again once when the server went away.
    async fn call(&self, tool: &str, args: Value, idle: Duration) -> Result<ca_mcp::CallResult, McpError> {
        for attempt in 0..2 {
            let mut guard = self.conn.lock().await;
            if guard.is_none() {
                *guard = Some(self.spec.connect().await?);
            }
            let Some(c) = guard.as_mut() else { continue };
            match c.call_tool(tool, args.clone(), idle, &mut |_| {}).await {
                Err(e) if e.gone() && attempt == 0 => {
                    *guard = None;
                }
                r => return r,
            }
        }
        Err(McpError::Ended(self.name.clone()))
    }
}

/// The servers this session has, from the connection and the settings.
pub fn configured(app: &App, reserved: &[String]) -> (Vec<Server>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let c = &app.connection;
    if c.signed_in()
        && app.settings.bool(&schema::ARENA_TOOLS)
        && let Some(url) = c.mcp_url()
    {
        out.push(Server::new(
            "arena",
            "Arena's tools",
            Spec::Http { url, key: c.api_key.clone(), headers: vec![], ca: c.ca.clone() },
            false,
            false,
        ));
    }
    if c.signed_in()
        && app.settings.bool(&schema::CODE_INTEL_TOOLS)
        && let Some(url) = c.code_intel_url()
    {
        out.push(Server::new(
            "code-intel",
            "Farzan Graph's tools",
            Spec::Http { url, key: c.api_key.clone(), headers: vec![], ca: c.ca.clone() },
            false,
            false,
        ));
    }
    let mut own: Vec<(String, Value)> = app
        .settings
        .value(&schema::MCP_SERVERS)
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for p in crate::plugins::enabled(&app.paths.data, &app.settings.strings(&schema::PLUGINS)) {
        own.extend(crate::plugins::servers(&p));
    }
    for (name, v) in &own {
        let name = name.as_str();
        if v.get("disabled").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if reserved.iter().any(|r| r == name) || name == "arena" || name == "code-intel" {
            notes.push(format!("The MCP server {name:?} has a name Code Arena keeps for its own: give it another."));
            continue;
        }
        let trusted = v.get("trust").and_then(Value::as_bool) == Some(true);
        let mut missing = Vec::new();
        let spec = if let Some(url) = v.get("url").and_then(Value::as_str) {
            let headers = v
                .get("headers")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .filter_map(|(k, h)| {
                    let (text, m) = expand(h.as_str()?);
                    missing.extend(m);
                    Some((k.clone(), Secret::new(text)))
                })
                .collect();
            Spec::Http { url: expand(url).0, key: None, headers, ca: c.ca.clone() }
        } else if let Some(cmd) = v.get("command").and_then(Value::as_str) {
            let args = v
                .get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str().map(|s| expand(s).0))
                .collect();
            let env = v
                .get("env")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(k, e)| match e.as_str() {
                    Some(s) => {
                        let (text, m) = expand(s);
                        missing.extend(m);
                        (k.clone(), Some(text))
                    }
                    None => (k.clone(), None),
                })
                .collect();
            let cwd = v.get("cwd").and_then(Value::as_str).map(PathBuf::from);
            Spec::Stdio(StdioSpec { name: name.to_string(), command: cmd.into(), args, env, cwd })
        } else {
            notes.push(format!("The MCP server {name:?} has neither a command nor a url: it is skipped."));
            continue;
        };
        if !missing.is_empty() {
            notes.push(format!("The MCP server {name:?} names variables that are not set: {}.", missing.join(", ")));
        }
        out.push(Server::new(name, name, spec, true, trusted));
    }
    (out, notes)
}

/// One server's tool, as the agent calls it.
pub struct McpTool {
    server: Server,
    remote: String,
    def: ToolDef,
    kind: Kind,
    idle: Duration,
    media_dir: PathBuf,
}

#[async_trait::async_trait]
impl Tool for McpTool {
    fn def(&self) -> ToolDef {
        self.def.clone()
    }
    fn kind(&self) -> Kind {
        self.kind
    }
    fn subject(&self, args: &Value) -> String {
        let s = args.to_string();
        if s.chars().count() > 200 { format!("{}…", s.chars().take(200).collect::<String>()) } else { s }
    }
    fn group(&self) -> String {
        self.server.name.clone()
    }
    async fn run(&self, args: Value, _ctx: &Context) -> Output {
        match self.server.call(&self.remote, args, self.idle).await {
            Err(McpError::Timeout(s)) => Output::error(format!(
                "{} did not answer for {s} s: the call was given up, and the server told.",
                self.server.title
            )),
            Err(e) => Output::error(format!("{}: {e}", self.server.title)),
            Ok(r) => {
                let mut text = r.text;
                for (i, m) in r.media.iter().enumerate() {
                    match save_media(&self.media_dir, &self.def.name, i, m) {
                        Ok(p) => text.push_str(&format!(
                            "\n(Saved {} as {}: tell the person where it is.)",
                            m.mime,
                            p.display()
                        )),
                        Err(e) => text.push_str(&format!("\n(A {} came back but could not be saved: {e})", m.mime)),
                    }
                }
                let mut o = if r.is_error { Output::error(text) } else { Output::ok(text) };
                if o.text.is_empty() {
                    o.text = "(no output)".into();
                }
                o
            }
        }
    }
}

/// What a tool returned, kept under `.code-arena/media/` (git-ignored), named after the tool and the time.
fn save_media(dir: &std::path::Path, tool: &str, n: usize, m: &ca_mcp::Media) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n")?;
    }
    let ext = match m.mime.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "video/mp4" => "mp4",
        "application/pdf" => "pdf",
        _ => "bin",
    };
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let suffix = if n > 0 { format!("-{}", n + 1) } else { String::new() };
    let safe: String =
        tool.chars().map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    let path = dir.join(format!("{safe}-{stamp}{suffix}.{ext}"));
    std::fs::write(&path, &m.bytes)?;
    Ok(path)
}

/// The note that lists tools not sent whole.
fn deferred_note(server: &Server, tools: &[Arc<dyn Tool>], instructions: Option<&str>) -> String {
    let mut s = format!("(Code Arena: {} joined.", server.title);
    if let Some(i) = instructions.filter(|i| !i.trim().is_empty()) {
        s.push_str(&format!(" Its instructions: {}", i.trim()));
    }
    if !tools.is_empty() {
        s.push_str(" These of its tools are listed, not loaded: call load_tools with their names to use them (or call one by name).\n");
        for t in tools {
            let d = t.def();
            let first = d.description.lines().next().unwrap_or("").chars().take(120).collect::<String>();
            s.push_str(&format!("- {}: {first}\n", d.name));
        }
    }
    s.push(')');
    s
}

/// Connects every server in the background. Those that answer within `wait` are in the first list; the rest join
/// the agent through the channel when they connect.
pub async fn start(
    servers: Servers,
    taken: Vec<String>,
    wait: Duration,
    on_demand: usize,
    media_dir: PathBuf,
) -> (Vec<Joined>, mpsc::UnboundedReceiver<Joined>) {
    let (tx, rx) = mpsc::unbounded_channel();
    // Each server says once that its first try is over, connected or not: the wait ends when all have.
    let (settled_tx, mut settled) = mpsc::unbounded_channel::<()>();
    let names = Arc::new(Mutex::new(taken));
    for s in servers.iter() {
        let s = s.clone();
        let tx = tx.clone();
        let settled_tx = settled_tx.clone();
        let names = names.clone();
        let media_dir = media_dir.clone();
        tokio::spawn(async move {
            let mut delay = Duration::from_secs(5);
            let mut first_try = true;
            loop {
                match s.open().await {
                    Ok((tools, instructions)) => {
                        s.set(State::Connected { tools: tools.len() });
                        let made = make_tools(&s, tools, &names, &media_dir);
                        let chars: usize =
                            made.iter().map(|t| t.def().description.len() + t.def().parameters.to_string().len()).sum();
                        let (sent, listed) =
                            if on_demand > 0 && chars > on_demand { (vec![], made) } else { (made, vec![]) };
                        let note = (instructions.is_some() || !listed.is_empty())
                            .then(|| deferred_note(&s, &listed, instructions.as_deref()));
                        let _ = tx.send(Joined { tools: sent, deferred: listed, note });
                        let _ = settled_tx.send(());
                        return;
                    }
                    Err(e @ McpError::NotThere { .. }) => {
                        s.set(State::Unavailable(e.to_string()));
                        let _ = settled_tx.send(());
                        return;
                    }
                    Err(e @ McpError::Refused { .. }) => {
                        s.set(State::Failed(e.to_string()));
                        let _ = settled_tx.send(());
                        return;
                    }
                    Err(e) => {
                        s.set(State::Failed(format!("{e} (trying again in {} s)", delay.as_secs())));
                        if first_try {
                            first_try = false;
                            let _ = settled_tx.send(());
                        }
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(Duration::from_secs(300));
                    }
                }
            }
        });
    }
    drop(tx);
    drop(settled_tx);
    let mut first = Vec::new();
    let mut rx = rx;
    let mut left = servers.len();
    let deadline = tokio::time::Instant::now() + wait;
    while left > 0 {
        tokio::select! {
            Some(()) = settled.recv() => left -= 1,
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    // What connected meanwhile is in the first list; the rest joins later.
    while let Ok(j) = rx.try_recv() {
        first.push(j);
    }
    (first, rx)
}

fn make_tools(
    s: &Server,
    tools: Vec<ToolInfo>,
    names: &Arc<Mutex<Vec<String>>>,
    media_dir: &std::path::Path,
) -> Vec<Arc<dyn Tool>> {
    let mut out: Vec<Arc<dyn Tool>> = Vec::new();
    let Ok(mut taken) = names.lock() else { return out };
    for t in tools {
        let name = if s.prefixed {
            format!("mcp__{}__{}", s.name, t.name)
        } else if taken.contains(&t.name) {
            // Farzan Graph's own copy of a tool Arena serves is left out: no tool comes twice.
            if s.name == "code-intel" {
                continue;
            }
            format!("arena_{}", t.name)
        } else {
            t.name.clone()
        };
        if taken.contains(&name) {
            continue;
        }
        taken.push(name.clone());
        // What a server says changes nothing never asks; the rest ask as commands do (a trusted server's run
        // without asking by a kept rule, and plan mode still refuses them).
        let kind = if t.read_only { Kind::Read } else { Kind::Shell };
        out.push(Arc::new(McpTool {
            server: s.clone(),
            remote: t.name,
            def: ToolDef { name, description: t.description, parameters: t.input_schema },
            kind,
            idle: t.timeout.unwrap_or(ca_mcp::DEADLINE),
            media_dir: media_dir.to_path_buf(),
        }));
    }
    out
}

/// `/mcp`: each server, connected or not and why.
pub fn describe(servers: &Servers) -> Vec<String> {
    servers
        .iter()
        .map(|s| {
            let state = match s.state() {
                State::Connecting => "connecting".to_string(),
                State::Connected { tools } => format!("connected, {tools} tools"),
                State::Unavailable(w) => format!("not available here ({w})"),
                State::Failed(w) => format!("not connected: {w}"),
            };
            format!("{:<14} {state}", s.title)
        })
        .collect()
}
