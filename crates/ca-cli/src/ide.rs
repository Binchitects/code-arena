//! `code-arena ide`: the session in a window of its own. The server (ca-server) serves the page and its live stream;
//! this drives the session from what the page sends, and sends the page what the turn does: the answer as it is
//! written, each tool call, the questions it asks (a card in the thread), and how the turn ended.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ca_agent::{Answer, Approver, Events};
use ca_tools::Output;
use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot};

use crate::app::App;
use crate::session::{self, Opts, Prepared, Resume};

/// What one tool result shows in the page at most.
const SHOWN_RESULT: usize = 4000;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Answer>>>>;

fn send(out: &broadcast::Sender<String>, v: Value) {
    let _ = out.send(v.to_string());
}

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

/// What the session is working through: its plan, its goal, its jobs (the bars above the box).
#[derive(Clone)]
struct Tasks {
    plan: Arc<ca_tools::plan::PlanBook>,
    goals: Arc<ca_agent::goal::GoalBook>,
    jobs: Option<Arc<ca_tools::jobs::Jobs>>,
}

impl Tasks {
    fn of(p: &Prepared) -> Tasks {
        Tasks { plan: p.agent.ctx.plan.clone(), goals: p.goals.clone(), jobs: p.agent.ctx.jobs.clone() }
    }

    fn said(&self) -> Value {
        let plan = self.plan.get();
        let goals = self.goals.get();
        let jobs: Vec<Value> = self
            .jobs
            .as_ref()
            .map(|j| j.list())
            .unwrap_or_default()
            .into_iter()
            .map(|j| json!({"id": j.id, "command": j.command, "running": j.running, "exit": j.exit, "background": j.background}))
            .collect();
        json!({
            "type": "tasks",
            "plan": if plan.steps.is_empty() { Value::Null } else { serde_json::to_value(&plan).unwrap_or(Value::Null) },
            "goal": goals.current.as_ref().map(|g| json!({"text": g.text, "status": g.status, "turns": g.turns})),
            "waiting": goals.next.len(),
            "jobs": jobs,
        })
    }
}

/// What the turn does, as the page reads it.
struct PageEvents {
    out: broadcast::Sender<String>,
    tasks: Option<Tasks>,
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
            && let Some(t) = &self.tasks
        {
            send(&self.out, t.said());
        }
    }
    fn step_end(&mut self) {
        send(&self.out, json!({"type": "step_end"}));
    }
}

/// The session as a page that just connected needs it: who and where, and its last turns.
fn state(p: &Prepared, app: &App, models: &[String], running: bool) -> Value {
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
    json!({
        "type": "state",
        "session": p.session,
        "folder": p.folder,
        "root": p.agent.ctx.workspace.root().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "terminals": cfg!(unix),
        "branch": ca_agent::instructions::git_branch(p.agent.ctx.workspace.root()),
        "theme": theme(app),
        "model": p.agent.model,
        "mode": p.agent.policy.mode.name(),
        "thinking": thinking_name(&p.agent.thinking),
        "models": models,
        "context": context(p),
        "edition": app.edition().name(),
        "version": ca_core::VERSION,
        "running": running,
        "messages": messages,
        "view_state": std::fs::read_to_string(view_state_file(app, p.agent.ctx.workspace.root()))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or(Value::Null),
    })
}

/// Where the workbench's look for a folder is kept (where its views are, its parts' sizes): the data folder's
/// `ide/`, one file per folder, named by the folder's path (FNV-1a, so the same folder finds it again).
pub fn view_state_file(app: &App, root: &Path) -> PathBuf {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in root.to_string_lossy().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    app.paths.data.join("ide").join(format!("{h:016x}.json"))
}

fn keep_view_state(app: &App, root: &Path, value: &Value) -> std::io::Result<()> {
    let file = view_state_file(app, root);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, serde_json::to_string_pretty(value).unwrap_or_default())
}

fn thinking_name(t: &ca_llm::Thinking) -> String {
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
    let Some(setting) = ca_settings::schema::ALL.iter().find(|s| s.key == key) else {
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
fn status(p: &Prepared) -> Value {
    json!({"type": "status", "model": p.agent.model, "mode": p.agent.policy.mode.name(),
           "thinking": thinking_name(&p.agent.thinking), "context": context(p), "running": false,
           "branch": ca_agent::instructions::git_branch(p.agent.ctx.workspace.root())})
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
    Native { from_ui: tokio::sync::mpsc::UnboundedReceiver<ca_server::FromPage>, to_ui: broadcast::Sender<String> },
    Browser { port: u16, open: bool },
}

pub struct IdeArgs {
    pub opts: Opts,
    pub resume: Resume,
    pub face: Face,
}

pub async fn run(app: &mut App, args: IdeArgs) -> anyhow::Result<i32> {
    let ui = app.ui;
    // The face's messages in, the session's out: the same for the native window and a browser's page.
    let (mut from_page, out, served) = match args.face {
        Face::Native { from_ui, to_ui } => (from_ui, to_ui, None),
        Face::Browser { port, open } => {
            let s = ca_server::start(port, ca_server::built_page()).await?;
            (s.from_page, s.to_pages, Some((s.url, open)))
        }
    };
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let approver = Arc::new(PageApprover { out: out.clone(), pending: pending.clone(), next: AtomicU64::new(1) });
    let mut p = session::prepare(app, &Opts { interactive: true, ..args.opts.clone() }, approver, args.resume).await?;
    let inbox = Arc::new(Mutex::new(Vec::<String>::new()));
    p.agent.inbox = Some(inbox.clone());
    let side = crate::ide_files::Side::new(
        p.agent.ctx.workspace.clone(),
        p.agent.ctx.checkpoints.clone(),
        out.clone(),
        app.settings.env().get("SHELL"),
        &app.paths.data,
    );
    let side = Arc::new(side);
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
    while let Some((_, msg)) = from_page.recv().await {
        if side.handle(&msg) {
            continue;
        }
        match msg["type"].as_str().unwrap_or("") {
            "hello" | "state" => {
                send(&out, state(&p, app, &models, false));
                send(&out, Tasks::of(&p).said());
            }
            "stop_job" => {
                if let Some(j) = &p.agent.ctx.jobs {
                    j.stop(msg["id"].as_u64().unwrap_or(0) as u32);
                }
                send(&out, Tasks::of(&p).said());
            }
            "model" => {
                if let Some(m) = msg["model"].as_str().filter(|m| !m.is_empty()) {
                    p.agent.model = m.to_string();
                    let _ = p.store.lock().map(|s| s.set_model(&p.session, m, p.agent.policy.mode.name()));
                    if let Some(s) = &p.sync {
                        s.set_model(m);
                    }
                    send(&out, status(&p));
                }
            }
            "thinking" => {
                p.agent.thinking = ca_llm::Thinking::parse(msg["level"].as_str().unwrap_or(""));
                send(&out, status(&p));
            }
            "context" => send(&out, context_detail(&p)),
            "keep_view_state" => {
                if let Err(e) = keep_view_state(app, p.agent.ctx.workspace.root(), &msg["state"]) {
                    send(&out, json!({"type": "notice", "text": format!("The layout was not kept: {e}")}));
                }
            }
            "themes" => {
                let dirs =
                    crate::ide_themes::folders(&app.paths.config, app.home.as_deref(), p.agent.ctx.workspace.root());
                send(&out, json!({"type": "themes", "items": crate::ide_themes::all(&dirs)}));
            }
            "set_setting" => {
                let key = msg["key"].as_str().unwrap_or("");
                let root = p.agent.ctx.workspace.root().to_path_buf();
                match keep_setting(app, &root, key, &msg["value"], msg["where"] == "project") {
                    Ok(file) => {
                        send(
                            &out,
                            json!({"type": "setting_kept", "key": key, "value": msg["value"], "file": file.display().to_string()}),
                        );
                        if key == ca_settings::schema::COLOR_THEME.key {
                            send(&out, json!({"type": "theme", "theme": theme(app)}));
                        }
                    }
                    Err(e) => send(&out, json!({"type": "setting_refused", "key": key, "error": e})),
                }
            }
            "compact" => {
                let mut events = PageEvents { out: out.clone(), tasks: None };
                let said = match p.agent.compact(&mut events, true).await {
                    Ok(Some(_)) => "Compacted: the model reads a summary of the earlier part from now on.".to_string(),
                    Ok(None) => "Nothing older to summarize.".to_string(),
                    Err(e) => format!("The model gave no summary ({e}): the conversation is as it was."),
                };
                send(&out, json!({"type": "notice", "text": said}));
                send(&out, context_detail(&p));
                send(&out, status(&p));
            }
            // Back to before question q: the conversation, the files, or both; again: then that question once more.
            "rewind" | "again" => {
                let q = msg["q"].as_u64().unwrap_or(0) as usize;
                let what = msg["what"].as_str().unwrap_or("both");
                let (code, chat) = if msg["type"] == "again" { (true, true) } else { (what != "chat", what != "code") };
                match session::rewind_to(&mut p, q, code, chat) {
                    Ok(r) => {
                        send(&out, state(&p, app, &models, false));
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
                        send(&out, json!({"type": "notice", "text": said.join(" ")}));
                        let text = msg["text"].as_str().unwrap_or("").trim().to_string();
                        if msg["type"] == "again" && !text.is_empty() {
                            turn(&mut p, &mut from_page, &out, &pending, &inbox, &side, text).await;
                            session::keep_window(app, &p);
                            send(&out, status(&p));
                        }
                    }
                    Err(e) => send(&out, json!({"type": "notice", "text": e.to_string()})),
                }
            }
            "mode" => {
                if let Some(m) = msg["mode"].as_str().and_then(ca_agent::Mode::parse) {
                    p.agent.policy.mode = m;
                    send(&out, status(&p));
                }
            }
            "send" => {
                let text = msg["text"].as_str().unwrap_or("").trim().to_string();
                if !text.is_empty() {
                    turn(&mut p, &mut from_page, &out, &pending, &inbox, &side, text).await;
                    session::keep_window(app, &p);
                    send(&out, status(&p));
                }
            }
            _ => {}
        }
    }
    Ok(0)
}

/// One turn, and what the page says while it runs: answers to its questions, Stop, messages sent meanwhile.
async fn turn(
    p: &mut Prepared,
    from_page: &mut tokio::sync::mpsc::UnboundedReceiver<ca_server::FromPage>,
    out: &broadcast::Sender<String>,
    pending: &Pending,
    inbox: &Arc<Mutex<Vec<String>>>,
    side: &crate::ide_files::Side,
    text: String,
) {
    let ui = p.ui;
    // `/NAME words`: a command or a skill of the project's or yours.
    let message = if text.starts_with('/') {
        match ca_agent::library::load(&p.sources).slash(&text) {
            Some(expanded) => expanded,
            None => {
                send(
                    out,
                    json!({"type": "notice", "text": format!("No command or skill {}.", text.split_whitespace().next().unwrap_or(&text))}),
                );
                return;
            }
        }
    } else {
        text.clone()
    };
    let q = session::questions(p).map(|qs| qs.len() + 1).unwrap_or(0);
    send(out, json!({"type": "user", "text": text, "q": q}));
    send(out, json!({"type": "turn_start"}));
    session::begin_turn(p, ui, &message).await;
    p.agent.max_steps = p.max_steps;
    session::before_message(p, &message).await;
    let started = std::time::Instant::now();
    let jobs = p.agent.ctx.jobs.clone();
    let skip = p.agent.skip.clone();
    let tasks = Tasks::of(p);
    let mut events = PageEvents { out: out.clone(), tasks: Some(tasks.clone()) };
    let result = {
        let running = p.agent.turn(&message, &mut events);
        tokio::pin!(running);
        loop {
            tokio::select! {
                r = &mut running => break Some(r),
                Some((_, m)) = from_page.recv() => if side.handle(&m) {} else { match m["type"].as_str().unwrap_or("") {
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
                    "stop" => break None,
                    "skip" => {
                        skip.ask();
                    }
                    "send" => {
                        let t = m["text"].as_str().unwrap_or("").trim().to_string();
                        if !t.is_empty() {
                            if let Ok(mut q) = inbox.lock() {
                                q.push(t.clone());
                            }
                            send(out, json!({"type": "queued", "text": t}));
                        }
                    }
                    "hello" | "state" => {
                        send(out, json!({"type": "busy"}));
                        send(out, tasks.said());
                    }
                    "stop_job" => {
                        if let Some(j) = &tasks.jobs {
                            j.stop(m["id"].as_u64().unwrap_or(0) as u32);
                        }
                        send(out, tasks.said());
                    }
                    "context" => {}
                    _ => {}
                } },
            }
        }
    };
    if let Ok(mut g) = pending.lock() {
        g.clear();
    }
    send(out, tasks.said());
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Some(Ok(o)) => send(
            out,
            json!({"type": "turn_end", "ok": true, "ms": ms, "steps": o.steps, "tool_calls": o.tool_calls,
                   "usage": {"prompt": o.usage.prompt, "cached": o.usage.cached, "completion": o.usage.completion}}),
        ),
        Some(Err(e)) => send(out, json!({"type": "turn_end", "ok": false, "ms": ms, "error": e.to_string()})),
        None => {
            if let Some(j) = &jobs
                && let Some(t) = j.current_turn()
            {
                j.stop_turn(t);
            }
            send(out, json!({"type": "turn_end", "ok": false, "ms": ms, "stopped": true}));
        }
    }
}
