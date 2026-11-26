//! `code-arena ide`: the session in a window of its own. The server (ca-server) serves the page and its live stream;
//! this drives the session from what the page sends, and sends the page what the turn does: the answer as it is
//! written, each tool call, the questions it asks (a card in the thread), and how the turn ended.
//!
//! This is the driver's one loop and its own messages (the session's state, a turn, its questions, the model,
//! the mode, a setting). Everything else the IDE asks is an area's, in a file of its own: `ide_ext` says how an
//! area joins the loop, and what becomes of a message that comes while a turn runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ca_agent::{Answer, Approver, Events};
use ca_tools::Output;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::app::App;
use crate::ide_ext::{self, Ctx, Driver, Kept, Later, Rule, Sent, Shared, TurnEnd, send};
use crate::ide_files::Side;
use crate::session::{self, Opts, Prepared, Resume};

/// The driver's own messages, each with what becomes of it while a turn runs (`ide_ext`'s turn rules). Those
/// answered at once are the turn's own (`answer`, `stop`, `skip`, a message sent meanwhile) and what needs the
/// session's handles only.
pub const MESSAGES: &[(&str, Rule)] = &[
    ("hello", Rule::Turn),
    ("state", Rule::Turn),
    ("send", Rule::Turn),
    ("answer", Rule::Turn),
    ("stop", Rule::Turn),
    ("skip", Rule::Turn),
    ("stop_job", Rule::Turn),
    ("themes", Rule::Turn),
    ("keep_view_state", Rule::Turn),
    ("context", Rule::Latest),
    ("model", Rule::Queue),
    ("thinking", Rule::Queue),
    ("mode", Rule::Queue),
    ("set_setting", Rule::Queue),
    ("compact", Rule::Refuse("compact the conversation")),
    ("rewind", Rule::Refuse("go back to an earlier question")),
    ("again", Rule::Refuse("ask again")),
];

/// The licence this program is offered under, and its text (the About box).
const LICENSE_NAME: &str = "MIT";
const LICENSE_TEXT: &str = include_str!("../../../LICENSE");

/// What one tool result shows in the page at most.
const SHOWN_RESULT: usize = 4000;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Answer>>>>;

/// The turn's questions, as cards in the page's thread; the first page to answer decides.
struct PageApprover {
    out: broadcast::Sender<String>,
    pending: Pending,
    next: AtomicU64,
}

#[async_trait::async_trait]
impl Approver for PageApprover {
    async fn ask(&self, tool: &str, subject: &str) -> Option<Answer> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = self.pending.lock() {
            p.insert(id, tx);
        }
        send(&self.out, json!({"type": "ask", "id": id, "tool": tool, "subject": subject}));
        // A turn stopped (its questions dropped) answers no.
        Some(rx.await.unwrap_or(Answer::No))
    }
}

/// What the session is working through: its plan, its goal, its jobs (the bars above the box), and what the
/// areas add. From the session's handles, so it is said while a turn runs too.
fn tasks(sh: &Shared) -> Value {
    let plan = sh.plan.get();
    let goals = sh.goals.get();
    let jobs: Vec<Value> = sh
        .jobs
        .as_ref()
        .map(|j| j.list())
        .unwrap_or_default()
        .into_iter()
        .map(|j| json!({"id": j.id, "command": j.command, "running": j.running, "exit": j.exit, "background": j.background}))
        .collect();
    let mut v = json!({
        "type": "tasks",
        "plan": if plan.steps.is_empty() { Value::Null } else { serde_json::to_value(&plan).unwrap_or(Value::Null) },
        "goal": goals.current.as_ref().map(|g| json!({"text": g.text, "status": g.status, "turns": g.turns})),
        "waiting": goals.next.len(),
        "jobs": jobs,
    });
    ide_ext::tasks(sh, &mut v);
    v
}

/// What the turn does, as the page reads it.
struct PageEvents {
    out: broadcast::Sender<String>,
    /// The session's handles, for the bars (none while the conversation is compacted: nothing of theirs changes).
    tasks: Option<Shared>,
}

impl Events for PageEvents {
    fn text(&mut self, delta: &str) {
        send(&self.out, json!({"type": "text", "delta": delta}));
    }
    fn reasoning(&mut self, delta: &str) {
        send(&self.out, json!({"type": "reasoning", "delta": delta}));
    }
    fn notice(&mut self, text: &str) {
        send(&self.out, json!({"type": "notice", "text": text}));
    }
    fn tool_start(&mut self, name: &str, subject: &str) {
        send(&self.out, json!({"type": "tool_start", "name": name, "subject": subject}));
    }
    fn tool_end(&mut self, name: &str, subject: &str, output: &Output) {
        let text: String = output.text.chars().take(SHOWN_RESULT).collect();
        let cut = output.text.chars().count() > SHOWN_RESULT;
        send(
            &self.out,
            json!({"type": "tool_end", "name": name, "subject": subject, "ok": !output.is_error, "text": text, "cut": cut}),
        );
        // The plan, a goal's check or a job changed with it: the bars are told at once.
        if matches!(name, "plan" | "goal_check" | "run_shell" | "command_output" | "stop_command")
            && let Some(sh) = &self.tasks
        {
            send(&self.out, tasks(sh));
        }
    }
    fn step_end(&mut self) {
        send(&self.out, json!({"type": "step_end"}));
    }
}

/// The manual's Code Arena page in the Arena signed in to (the help links to it); none without an Arena.
fn manual(arena: Option<&str>) -> Value {
    arena.map(|url| json!(format!("{}/help/code-arena", url.trim_end_matches('/')))).unwrap_or(Value::Null)
}

/// What this edition does not have, each with why: the commands and the settings that need Farzan Arena.
fn locked(edition: ca_settings::Edition) -> Value {
    let names = crate::edition::NEEDS_ARENA.iter().filter_map(|c| Some((*c, crate::edition::locked(edition, c)?)));
    let settings = crate::edition::SETTINGS_NEED_ARENA
        .iter()
        .filter_map(|k| Some((*k, crate::edition::setting_locked(edition, k)?)));
    Value::Object(names.chain(settings).map(|(name, why)| (name.to_string(), json!(why))).collect())
}

/// The session as a page that just connected needs it: who and where, and its last turns.
fn state(p: &Prepared, app: &App, sh: &Shared, running: bool) -> Value {
    let numbers: HashMap<i64, usize> =
        session::questions(p).unwrap_or_default().into_iter().map(|(n, seq, _)| (seq, n)).collect();
    let messages: Vec<Value> = p
        .store
        .lock()
        .ok()
        .and_then(|s| s.page(&p.session, 30, None).ok())
        .map(|page| {
            page.messages
                .into_iter()
                .filter(|m| !matches!(m.message.role, ca_llm::Role::System))
                .map(|m| {
                    json!({
                        "role": m.message.role,
                        "text": m.message.content,
                        "tools": m.message.tool_calls.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
                        "turn": m.turn,
                        "q": numbers.get(&m.seq),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut v = json!({
        "type": "state",
        "session": p.session,
        "folder": p.folder,
        "root": p.agent.ctx.workspace.root().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "terminals": cfg!(unix),
        // The terminals' shell by its name, for their tabs ("bash", "zsh").
        "shell": app.settings.env().get("SHELL").and_then(|s| s.rsplit('/').next().map(str::to_string)).unwrap_or_else(|| "sh".into()),
        "branch": ca_agent::instructions::git_branch(p.agent.ctx.workspace.root()),
        "theme": theme(app),
        "model": p.agent.model,
        "mode": p.agent.policy.mode.name(),
        "thinking": thinking_name(&p.agent.thinking),
        "models": sh.models.as_slice(),
        "context": context(p),
        "edition": app.edition().name(),
        "version": ca_core::VERSION,
        // The About box: the licence, the source, where the log is kept, and the help's link to the manual.
        "license": LICENSE_NAME,
        "license_text": LICENSE_TEXT,
        "source": ca_core::SOURCE,
        "log": crate::log::path().map(|f| f.display().to_string()),
        "manual": manual(app.connection.url.as_deref()),
        "locked": locked(app.edition()),
        "running": running,
        "messages": messages,
        "view_state": std::fs::read_to_string(view_state_at(&app.paths.data, p.agent.ctx.workspace.root()))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or(Value::Null),
    });
    ide_ext::state(sh, Some(p), &mut v);
    v
}

/// Where the workbench's look for a folder is kept (where its views are, its parts' sizes): the data folder's
/// `ide/`, one file per folder, named by the folder's path (FNV-1a, so the same folder finds it again).
fn view_state_at(data: &Path, root: &Path) -> PathBuf {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in root.to_string_lossy().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    data.join("ide").join(format!("{h:016x}.json"))
}

fn keep_view_state(data: &Path, root: &Path, value: &Value) -> std::io::Result<()> {
    let file = view_state_at(data, root);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, serde_json::to_string_pretty(value).unwrap_or_default())
}

pub fn thinking_name(t: &ca_llm::Thinking) -> String {
    match t {
        ca_llm::Thinking::Default => "default".into(),
        ca_llm::Thinking::Off => "off".into(),
        ca_llm::Thinking::Level(l) => l.clone(),
    }
}

/// How full the window is: the ring in the status line.
fn context(p: &Prepared) -> Value {
    let w = &p.agent.window;
    json!({"used": p.agent.context_used(), "size": w.size, "compact_at": w.compact_at})
}

/// What fills the next request, by kind and its biggest messages: the Context panel.
fn context_detail(p: &Prepared) -> Value {
    let b = p.agent.context_breakdown();
    let w = &p.agent.window;
    let kinds = [
        ("Instructions", b.system),
        ("Tools", b.tools),
        ("Your messages", b.you),
        ("Answers", b.answers),
        ("Tool results", b.tool_results),
        ("Notes", b.notes),
    ];
    json!({
        "type": "context_detail",
        "context": context(p),
        "kinds": kinds.iter().map(|(k, c)| json!({"kind": k, "tokens": w.tokens(*c)})).collect::<Vec<_>>(),
        "biggest": b.biggest.iter().map(|(at, role, chars, start)| json!({"at": at, "role": role, "tokens": w.tokens(*chars), "start": start})).collect::<Vec<_>>(),
    })
}

/// A setting changed from the page: checked as the schema says (its kind, where it may be set, the edition's
/// locks), kept in the person's settings.json or the project's, and read again. `null` takes it out (its default).
pub fn keep_setting(app: &mut App, root: &Path, key: &str, value: &Value, project: bool) -> Result<PathBuf, String> {
    let Some(setting) = ca_settings::schema::find(key) else {
        return Err(format!("there is no setting {key}"));
    };
    if let Some(why) = crate::edition::setting_locked(app.edition(), key) {
        return Err(why);
    }
    if project && setting.scope != ca_settings::Scope::Project {
        return Err(format!("{key} is yours alone: a project's settings cannot set it"));
    }
    if !value.is_null() && !setting.accepts(value) {
        return Err(format!("{key} does not take {value}"));
    }
    let project_file = root.join(".code-arena").join("settings.json");
    let file = if project { project_file.clone() } else { app.paths.settings_file() };
    let done = if value.is_null() {
        ca_settings::settings::forget(&file, key)
    } else {
        ca_settings::settings::keep(&file, key, value)
    };
    done.map_err(|e| format!("{}: {e}", file.display()))?;
    app.settings =
        ca_settings::Settings::load(&app.paths.settings_file(), Some(&project_file), app.settings.env().clone());
    Ok(file)
}

/// The colour theme chosen (`workbench.colorTheme`; empty: the system's light or dark).
fn theme(app: &App) -> String {
    app.settings.string(&ca_settings::schema::COLOR_THEME).unwrap_or_default()
}

/// What the status line shows, without the thread (after a turn, or a change: the page keeps its thread).
fn status(p: &Prepared, sh: &Shared) -> Value {
    let mut v = json!({"type": "status", "model": p.agent.model, "mode": p.agent.policy.mode.name(),
           "thinking": thinking_name(&p.agent.thinking), "context": context(p), "running": false,
           "branch": ca_agent::instructions::git_branch(p.agent.ctx.workspace.root())});
    ide_ext::status(sh, Some(p), &mut v);
    v
}

/// A browser in app mode (a window of its own, its own profile), else the system's way to open an address.
fn open(url: &str, data: &Path) -> String {
    let profile = data.join("ide-profile");
    let app_args = |b: &str| {
        (
            b.to_string(),
            vec![format!("--app={url}"), format!("--user-data-dir={}", profile.display()), "--no-first-run".into()],
        )
    };
    let mut tries: Vec<(String, Vec<String>)> = Vec::new();
    if cfg!(target_os = "macos") {
        for b in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ] {
            if Path::new(b).exists() {
                tries.push(app_args(b));
            }
        }
        tries.push(("open".into(), vec![url.into()]));
    } else if cfg!(windows) {
        for b in ["msedge", "chrome", "brave"] {
            tries.push((
                "cmd".into(),
                vec!["/C".into(), "start".into(), String::new(), b.into(), format!("--app={url}")],
            ));
        }
        tries.push(("cmd".into(), vec!["/C".into(), "start".into(), String::new(), url.into()]));
    } else {
        let path = std::env::var_os("PATH").unwrap_or_default();
        for b in
            ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge", "brave-browser"]
        {
            if std::env::split_paths(&path).any(|d| d.join(b).is_file()) {
                tries.push(app_args(b));
            }
        }
        tries.push(("xdg-open".into(), vec![url.into()]));
    }
    for (program, args) in tries {
        let started = std::process::Command::new(&program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if started.is_ok() {
            return program.rsplit(['/', '\\']).next().unwrap_or(&program).to_string();
        }
    }
    String::new()
}

/// Where the address is kept for this run (only the person may read it): a launcher reads it.
fn keep_address(data: &Path, url: &str) -> Option<PathBuf> {
    let file = data.join("ide.json");
    std::fs::create_dir_all(data).ok()?;
    let text = json!({"url": url, "pid": std::process::id()}).to_string();
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::remove_file(&file);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).ok()?;
        f.write_all(text.as_bytes()).ok()?;
    }
    #[cfg(not(unix))]
    std::fs::write(&file, text).ok()?;
    Some(file)
}

/// Where the IDE is shown: the native window (its messages in-process), or a browser (its page and its WebSocket).
pub enum Face {
    Native { from_ui: mpsc::UnboundedReceiver<ca_server::FromPage>, to_ui: broadcast::Sender<String> },
    Browser { port: u16, open: bool },
}

pub struct IdeArgs {
    pub opts: Opts,
    pub resume: Resume,
    pub face: Face,
}

/// Where the driver's messages come from, and the questions a turn waits on.
struct Wires {
    /// From the faces.
    from_page: mpsc::UnboundedReceiver<ca_server::FromPage>,
    /// From the driver itself (an area's background work: `Shared::post`).
    own: mpsc::UnboundedReceiver<Value>,
    pending: Pending,
}

/// Who started a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum How {
    /// The person sent it.
    Typed,
    /// The person sent it while a turn ran, too late for that turn to read: the thread shows it already.
    Late,
    /// The session says it itself (`said_next`): a plan's review, a goal's next turn.
    Carried,
}

/// What the session runs next by itself, when nothing waits to be answered: first what the person sent too late
/// for the last turn to read, then what the session says next (`said_next`).
fn next_turn(said_next: &mut Option<String>, inbox: &Mutex<Vec<String>>) -> Option<(String, How)> {
    let late: Vec<String> = inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
    if !late.is_empty() {
        return Some((late.join("\n"), How::Late));
    }
    said_next.take().map(|text| (text, How::Carried))
}

/// Stop ends the chain, and so does a turn that failed: after either the session does not carry on by itself
/// (what it had to say next is dropped), so nothing runs on against the person's word or a model out of reach.
/// What the person sent meanwhile still runs.
fn chain_ends(end: &TurnEnd, said_next: &mut Option<String>) {
    if end.stopped || end.error.is_some() {
        *said_next = None;
    }
}

/// The driver's own messages that need the session's handles only: answered whether a turn runs or not.
/// `false`: not one of them.
fn own_shared(sh: &Shared, m: &Value) -> bool {
    match ide_ext::kind(m) {
        "stop_job" => {
            if let Some(j) = &sh.jobs {
                j.stop(m["id"].as_u64().unwrap_or(0) as u32);
            }
            send(&sh.out, tasks(sh));
        }
        "keep_view_state" => {
            if let Err(e) = keep_view_state(&sh.paths.data, sh.workspace.root(), &m["state"]) {
                ide_ext::say(&sh.out, &format!("The layout was not kept: {e}"));
            }
        }
        // The themes' folders are read off the loop: a turn is not kept waiting for them.
        "themes" => {
            let dirs = crate::ide_themes::folders(&sh.paths.config, sh.home.as_deref(), sh.workspace.root());
            let out = sh.out.clone();
            tokio::task::spawn_blocking(move || {
                send(&out, json!({"type": "themes", "items": crate::ide_themes::all(&dirs)}));
            });
        }
        _ => return false,
    }
    true
}

/// What became of a message of the driver's own that needs the session.
enum Own {
    /// It is an area's.
    Not,
    Done,
    /// A turn is to run with it (`again`).
    Turn(Sent),
}

/// The driver's own messages that need the session, answered between turns.
async fn own_idle(m: &Value, p: &mut Prepared, app: &mut App, sh: &Shared) -> Own {
    let out = &sh.out;
    match ide_ext::kind(m) {
        "model" => {
            if let Some(name) = m["model"].as_str().filter(|n| !n.is_empty()) {
                p.agent.model = name.to_string();
                let _ = p.store.lock().map(|s| s.set_model(&p.session, name, p.agent.policy.mode.name()));
                if let Some(s) = &p.sync {
                    s.set_model(name);
                }
                send(out, status(p, sh));
            }
        }
        "thinking" => {
            p.agent.thinking = ca_llm::Thinking::parse(m["level"].as_str().unwrap_or(""));
            send(out, status(p, sh));
        }
        "mode" => {
            if let Some(mode) = m["mode"].as_str().and_then(ca_agent::Mode::parse) {
                p.agent.policy.mode = mode;
                send(out, status(p, sh));
            }
        }
        "context" => send(out, context_detail(p)),
        "set_setting" => {
            let key = m["key"].as_str().unwrap_or("");
            let root = p.agent.ctx.workspace.root().to_path_buf();
            match keep_setting(app, &root, key, &m["value"], m["where"] == "project") {
                Ok(file) => {
                    send(
                        out,
                        json!({"type": "setting_kept", "key": key, "value": m["value"], "file": file.display().to_string()}),
                    );
                    if key == ca_settings::schema::COLOR_THEME.key {
                        send(out, json!({"type": "theme", "theme": theme(app)}));
                    }
                }
                Err(e) => send(out, json!({"type": "setting_refused", "key": key, "error": e})),
            }
        }
        "compact" => {
            let mut events = PageEvents { out: out.clone(), tasks: None };
            let said = match p.agent.compact(&mut events, true).await {
                Ok(Some(_)) => "Compacted: the model reads a summary of the earlier part from now on.".to_string(),
                Ok(None) => "Nothing older to summarize.".to_string(),
                Err(e) => format!("The model gave no summary ({e}): the conversation is as it was."),
            };
            ide_ext::say(out, &said);
            send(out, context_detail(p));
            send(out, status(p, sh));
        }
        // Back to before question q: the conversation, the files, or both; again: then that question once more.
        "rewind" | "again" => {
            let again = m["type"] == "again";
            let q = m["q"].as_u64().unwrap_or(0) as usize;
            let what = m["what"].as_str().unwrap_or("both");
            let (code, chat) = if again { (true, true) } else { (what != "chat", what != "code") };
            match session::rewind_to(p, q, code, chat) {
                Ok(r) => {
                    send(out, state(p, app, sh, false));
                    let mut said = vec![format!("Back to before: {}", r.text)];
                    if !r.put_back.is_empty() {
                        said.push(format!("Put back {}.", r.put_back.join(", ")));
                    }
                    if !r.removed.is_empty() {
                        said.push(format!("Removed {} (a turn made it).", r.removed.join(", ")));
                    }
                    if !r.failed.is_empty() {
                        said.push(format!("Could not put back: {}.", r.failed.join("; ")));
                    }
                    ide_ext::say(out, &said.join(" "));
                    let text = m["text"].as_str().unwrap_or("").trim().to_string();
                    if again && !text.is_empty() {
                        return Own::Turn(Sent::new(m, text, false));
                    }
                }
                Err(e) => ide_ext::say(out, &e.to_string()),
            }
        }
        _ => return Own::Not,
    }
    Own::Done
}

/// After a handler had the session in its hands: its handles are taken again, and when the session or the folder
/// is another, the faces get it whole and the areas' background work begins for it.
fn settled(sh: &mut Shared, p: &Prepared, app: &App, drv: &Driver, side: &Arc<Side>) {
    let another = sh.session != p.session || !Arc::ptr_eq(&sh.side, side);
    *sh = Shared::of(p, app, drv, side);
    if another {
        send(&drv.out, state(p, app, sh, false));
        send(&drv.out, tasks(sh));
        ide_ext::start(sh);
    }
}

pub async fn run(app: &mut App, args: IdeArgs) -> anyhow::Result<i32> {
    let ui = app.ui;
    // The face's messages in, the session's out: the same for the native window and a browser's page.
    let (from_page, out, served) = match args.face {
        Face::Native { from_ui, to_ui } => (from_ui, to_ui, None),
        Face::Browser { port, open } => {
            let s = ca_server::start(port, ca_server::built_page()).await?;
            (s.from_page, s.to_pages, Some((s.url, open)))
        }
    };
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let approver: Arc<dyn ca_agent::Approver> =
        Arc::new(PageApprover { out: out.clone(), pending: pending.clone(), next: AtomicU64::new(1) });
    let opts = Opts { interactive: true, ..args.opts.clone() };
    let mut p = session::prepare(app, &opts, approver.clone(), args.resume).await?;
    let inbox = Arc::new(Mutex::new(Vec::<String>::new()));
    p.agent.inbox = Some(inbox.clone());
    let side = crate::ide_files::Side::new(
        p.agent.ctx.workspace.clone(),
        p.agent.ctx.checkpoints.clone(),
        out.clone(),
        app.settings.env().get("SHELL"),
        &app.paths.data,
    );
    let mut side = Arc::new(side);
    side.start_watching();
    ui.say(&format!("{} {} · {} · {}", ui.bold(ca_core::PRODUCT), ca_core::VERSION, p.agent.model, ui.dim(&p.folder)));
    if let Some((url, open_it)) = &served {
        let kept = keep_address(&app.paths.data, url);
        ui.say(&format!("The IDE: {url}"));
        if let Some(f) = kept {
            ui.say(&ui.dim(&format!("(the address is kept in {}; Ctrl+C stops the IDE)", f.display())));
        }
        if *open_it {
            match open(url, &app.paths.data) {
                b if b.is_empty() => ui.say(&ui.yellow("No browser could be opened here: open the address above.")),
                b => ui.say(&ui.dim(&format!("Opened with {b}."))),
            }
        }
    }
    // The models the pickers offer: the gateway's (and the providers' of one's own), asked once.
    let models: Vec<String> =
        match tokio::time::timeout(std::time::Duration::from_secs(15), p.agent.gateway.models()).await {
            Ok(Ok(ms)) => ms.into_iter().map(|m| m.id).collect(),
            _ => vec![p.agent.model.clone()],
        };
    let (post, own) = mpsc::unbounded_channel();
    let mut w = Wires { from_page, own, pending };
    let mut drv = Driver {
        out: out.clone(),
        post,
        approver,
        inbox,
        opts,
        models,
        arena: app.arena().ok(),
        later: Later::default(),
        kept: Kept::default(),
    };
    let mut sh = Shared::of(&p, app, &drv, &side);
    ide_ext::start(&sh);
    loop {
        // What waited for a turn to end comes first, in the order it was asked; then what the session runs next
        // by itself; then what the faces say.
        let msg = if let Some(m) = drv.later.next() {
            m
        } else if let Some((text, how)) = next_turn(&mut p.said_next, &drv.inbox) {
            let sent = Sent::new(&Value::Null, text, false);
            turn(&mut w, &mut p, app, &mut drv, &mut side, &mut sh, sent, how).await;
            continue;
        } else {
            tokio::select! {
                biased;
                m = w.from_page.recv() => match m {
                    // A name of the driver's own is not a face's to send.
                    Some((_, m)) if !ide_ext::kind(&m).starts_with('_') => m,
                    Some(_) => continue,
                    None => break,
                },
                Some(m) = w.own.recv() => m,
            }
        };
        if side.handle(&msg) {
            continue;
        }
        let name = ide_ext::kind(&msg);
        match ide_ext::rule(name) {
            None | Some(Rule::Side) => {}
            Some(Rule::Turn) => match name {
                "hello" | "state" => {
                    send(&out, state(&p, app, &sh, false));
                    send(&out, tasks(&sh));
                }
                "send" => {
                    let text = msg["text"].as_str().unwrap_or("").trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    let mut sent = Sent::new(&msg, text, false);
                    ide_ext::shape(&sh, &mut sent);
                    if !sent.done {
                        let mut ctx = Ctx { app: &mut *app, p: &mut p, side: &mut side, drv: &mut drv, sh: &sh };
                        ide_ext::command(&mut ctx, &mut sent).await;
                        settled(&mut sh, &p, app, &drv, &side);
                    }
                    if !sent.done {
                        turn(&mut w, &mut p, app, &mut drv, &mut side, &mut sh, sent, How::Typed).await;
                    }
                }
                // A running turn's alone: there is none.
                "answer" | "stop" | "skip" => {}
                _ => {
                    if !own_shared(&sh, &msg) {
                        ide_ext::shared(&sh, &msg);
                    }
                }
            },
            Some(Rule::Queue | Rule::Latest | Rule::Refuse(_)) => {
                let own = own_idle(&msg, &mut p, app, &sh).await;
                if matches!(own, Own::Not) {
                    let mut ctx = Ctx { app: &mut *app, p: &mut p, side: &mut side, drv: &mut drv, sh: &sh };
                    ide_ext::idle(&mut ctx, &msg).await;
                }
                settled(&mut sh, &p, app, &drv, &side);
                if let Own::Turn(mut sent) = own {
                    ide_ext::shape(&sh, &mut sent);
                    if !sent.done {
                        turn(&mut w, &mut p, app, &mut drv, &mut side, &mut sh, sent, How::Typed).await;
                    }
                }
            }
        }
    }
    Ok(0)
}

/// A message that came while a turn runs: answered now, kept for the turn's end or refused, by its rule.
/// `true`: the person stopped the turn.
fn during(m: &Value, sh: &Shared, drv: &mut Driver, pending: &Pending) -> bool {
    if sh.side.handle(m) || !ide_ext::meanwhile(m, &mut drv.later, &drv.out) {
        return false;
    }
    let out = &drv.out;
    match ide_ext::kind(m) {
        "answer" => {
            let id = m["id"].as_u64().unwrap_or(0);
            let answer = match m["answer"].as_str().unwrap_or("") {
                "yes" => Answer::Yes,
                "always" => Answer::Always,
                _ => Answer::No,
            };
            let tx = pending.lock().ok().and_then(|mut g| g.remove(&id));
            if let Some(tx) = tx {
                let _ = tx.send(answer);
                send(out, json!({"type": "answered", "id": id, "answer": m["answer"]}));
            }
        }
        "stop" => return true,
        "skip" => {
            sh.skip.ask();
        }
        // Sent meanwhile: the turn reads it at its next step (or, when an area says so, it waits whole for the
        // turn's end: a command).
        "send" => {
            let text = m["text"].as_str().unwrap_or("").trim().to_string();
            if !text.is_empty() {
                let mut sent = Sent::new(m, text, true);
                ide_ext::shape(sh, &mut sent);
                if sent.later {
                    ide_ext::defer(m, &mut drv.later, out, false);
                } else if !sent.done {
                    if let Ok(mut q) = drv.inbox.lock() {
                        q.push(sent.model.clone());
                    }
                    send(out, sent.with_fields(json!({"type": "queued", "text": sent.shown})));
                }
            }
        }
        "hello" | "state" => {
            send(out, json!({"type": "busy"}));
            send(out, tasks(sh));
        }
        _ => {
            if !own_shared(sh, m) {
                ide_ext::shared(sh, m);
            }
        }
    }
    false
}

/// One turn, and what the faces say while it runs: answers to its questions, Stop, messages sent meanwhile.
#[allow(clippy::too_many_arguments)]
async fn turn(
    w: &mut Wires,
    p: &mut Prepared,
    app: &mut App,
    drv: &mut Driver,
    side: &mut Arc<Side>,
    sh: &mut Shared,
    sent: Sent,
    how: How,
) {
    let ui = p.ui;
    let out = drv.out.clone();
    // `/NAME words`: a command or a skill of the project's or yours.
    let message = if how == How::Typed && sent.model.starts_with('/') {
        match ca_agent::library::load(&p.sources).slash(&sent.model) {
            Some(expanded) => expanded,
            None => {
                let name = sent.model.split_whitespace().next().unwrap_or(&sent.model);
                ide_ext::say(&out, &format!("No command or skill {name}."));
                return;
            }
        }
    } else {
        sent.model.clone()
    };
    let q = session::questions(p).map(|qs| qs.len() + 1).unwrap_or(0);
    match how {
        How::Typed => send(&out, sent.with_fields(json!({"type": "user", "text": sent.shown, "q": q}))),
        How::Carried => send(&out, json!({"type": "user", "text": sent.shown, "q": q, "carried": true})),
        // The thread shows it already, as sent while the last turn ran.
        How::Late => {}
    }
    send(&out, json!({"type": "turn_start"}));
    ide_ext::before_turn(&mut Ctx { app: &mut *app, p: &mut *p, side: &mut *side, drv: &mut *drv, sh: &*sh }, &message)
        .await;
    session::begin_turn(p, ui, &message).await;
    // A turn the session carries on keeps the steps its goal's budget left it.
    if how != How::Carried {
        p.agent.max_steps = p.max_steps;
    }
    session::before_message(p, &message).await;
    // The session as the turn begins: what answers meanwhile reads this.
    *sh = Shared::of(p, app, drv, side);
    let started = std::time::Instant::now();
    let jobs = p.agent.ctx.jobs.clone();
    let mut events = PageEvents { out: out.clone(), tasks: Some(sh.clone()) };
    let result = {
        let running = p.agent.turn(&message, &mut events);
        tokio::pin!(running);
        loop {
            let m = tokio::select! {
                r = &mut running => break Some(r),
                Some((_, m)) = w.from_page.recv() => {
                    if ide_ext::kind(&m).starts_with('_') {
                        continue;
                    }
                    m
                }
                Some(m) = w.own.recv() => m,
            };
            if during(&m, sh, drv, &w.pending) {
                break None;
            }
        }
    };
    if let Ok(mut g) = w.pending.lock() {
        g.clear();
    }
    send(&out, tasks(sh));
    let took = started.elapsed();
    let ms = took.as_millis() as u64;
    let mut end = TurnEnd { message, outcome: None, stopped: false, error: None, took, carried: how == How::Carried };
    match result {
        Some(Ok(o)) => {
            send(
                &out,
                json!({"type": "turn_end", "ok": true, "ms": ms, "steps": o.steps, "tool_calls": o.tool_calls,
                       "usage": {"prompt": o.usage.prompt, "cached": o.usage.cached, "completion": o.usage.completion}}),
            );
            end.outcome = Some(o);
        }
        Some(Err(e)) => {
            send(&out, json!({"type": "turn_end", "ok": false, "ms": ms, "error": e.to_string()}));
            end.error = Some(e.to_string());
        }
        None => {
            if let Some(j) = &jobs
                && let Some(t) = j.current_turn()
            {
                j.stop_turn(t);
            }
            send(&out, json!({"type": "turn_end", "ok": false, "ms": ms, "stopped": true}));
            end.stopped = true;
        }
    }
    ide_ext::after_turn(&mut Ctx { app: &mut *app, p: &mut *p, side: &mut *side, drv: &mut *drv, sh: &*sh }, &end)
        .await;
    chain_ends(&end, &mut p.said_next);
    session::keep_window(app, p);
    settled(sh, p, app, drv, side);
    send(&out, status(p, sh));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_runs_what_came_too_late_first_then_what_it_says_next_itself() {
        let inbox = Mutex::new(vec!["and the tests".to_string(), "then commit".to_string()]);
        let mut said = Some("Goal (turn 2): make it pass".to_string());
        // What the person sent while the last turn wrote its last words, as one message of its own.
        assert_eq!(next_turn(&mut said, &inbox), Some(("and the tests\nthen commit".into(), How::Late)));
        assert!(inbox.lock().expect("lock").is_empty(), "taken out: the turn's first step does not read it again");
        assert!(said.is_some(), "what the session says next waits its turn");
        assert_eq!(next_turn(&mut said, &inbox), Some(("Goal (turn 2): make it pass".into(), How::Carried)));
        assert_eq!(next_turn(&mut said, &inbox), None, "then the faces are listened to again");
    }

    #[test]
    fn a_turn_stopped_or_failed_ends_the_chain_but_not_what_the_person_sent_meanwhile() {
        let end = |stopped: bool, error: Option<&str>| TurnEnd {
            message: "Goal (turn 2): make it pass".into(),
            outcome: None,
            stopped,
            error: error.map(str::to_string),
            took: std::time::Duration::from_secs(1),
            carried: true,
        };
        let next = || Some("Goal (turn 3): make it pass".to_string());
        let mut said = next();
        chain_ends(&end(false, None), &mut said);
        assert!(said.is_some(), "a turn that ended by itself carries on");
        chain_ends(&end(false, Some("the gateway is out of reach")), &mut said);
        assert_eq!(said, None, "not on against a model that does not answer");
        let inbox = Mutex::new(vec!["no, the other file".to_string()]);
        let mut said = next();
        chain_ends(&end(true, None), &mut said);
        assert_eq!(next_turn(&mut said, &inbox), Some(("no, the other file".into(), How::Late)));
        assert_eq!(next_turn(&mut said, &inbox), None, "the session says nothing more by itself");
    }

    #[test]
    fn the_about_boxs_fields_and_what_this_edition_locks() {
        assert_eq!(manual(Some("https://arena.example/")), json!("https://arena.example/help/code-arena"));
        assert_eq!(manual(None), Value::Null);
        assert!(LICENSE_TEXT.starts_with("MIT License"), "the licence's own text");
        let standalone = locked(ca_settings::Edition::Standalone);
        assert!(standalone["/web"].as_str().is_some_and(|why| why.contains("managed edition")), "{standalone}");
        assert!(standalone["arena.syncChats"].as_str().is_some_and(|why| why.contains("Farzan Arena")));
        assert_eq!(locked(ca_settings::Edition::Managed), json!({}));
    }

    #[test]
    fn the_workbenchs_look_is_kept_for_its_folder() {
        let data = tempfile::tempdir().expect("data");
        let look = json!({"layout": {"primary": ["files"]}});
        keep_view_state(data.path(), Path::new("/work/one"), &look).expect("kept");
        let file = view_state_at(data.path(), Path::new("/work/one"));
        assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(&file).expect("read")).expect("json"), look);
        assert_ne!(file, view_state_at(data.path(), Path::new("/work/two")), "another folder, another file");
    }
}
