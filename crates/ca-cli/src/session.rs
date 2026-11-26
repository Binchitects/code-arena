//! What `-p` and the chat share: the folder, the project's settings and `.code-arena/`, the model, the agent, and
//! the session it is kept in.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, bail};
use ca_agent::instructions::{self, Environment};
use ca_agent::policy::Policy;
use ca_agent::{Agent, Approver, Mode, Recorder, Rules};
use ca_llm::{Message, Thinking};
use ca_session::Store;
use ca_settings::schema;
use ca_tools::Context;
use ca_workspace::Workspace;
use serde_json::json;

use crate::app::App;

#[derive(Clone)]
pub struct Opts {
    pub cwd: Option<PathBuf>,
    pub add_dirs: Vec<PathBuf>,
    pub mode: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub interactive: bool,
    /// `--plan`: carry on the plan an earlier session of this folder left.
    pub carry_plan: bool,
}

/// Which session to work in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    New,
    /// This folder's last one (`--continue`).
    Latest,
    Id(String),
}

pub struct Prepared {
    pub agent: Agent,
    pub store: Arc<Mutex<Store>>,
    pub session: String,
    pub folder: String,
    /// Said when a session was taken up: how many messages, and how many the model reads as a summary.
    pub resumed: Option<String>,
    pub servers: crate::servers::Servers,
    /// Where commands, skills and sub-agents are read from (again at each message).
    pub sources: Vec<ca_agent::library::Source>,
    /// Tools turned off for this session (`/tools off`).
    pub disabled: Vec<Arc<dyn ca_tools::Tool>>,
    /// The chat in Arena this session is kept with, when there is an Arena to keep it with.
    pub sync: Option<Arc<ca_sync::ChatSync>>,
    /// What went on in the background (the chat's sync), said at the next prompt.
    pub notices: Notices,
    /// Words a command says to the agent as the person's next message (a plan's review).
    pub said_next: Option<String>,
    pub memory: Arc<ca_memory::Memory>,
    /// The last turn was long: the next is reminded to keep what it worked out.
    pub nudge: bool,
}

/// Lines to say at the next prompt: each with whether it is a problem.
pub type Notices = Arc<Mutex<Vec<(bool, String)>>>;

/// Before a turn: what the web added comes in (said, and read again by the model), and the chat is held.
pub async fn begin_turn(p: &mut Prepared, ui: crate::ui::Ui) {
    let Some(sync) = p.sync.clone() else { return };
    let taken = sync.begin_turn(p.agent.inbox.clone()).await;
    if p.agent.tags.is_empty()
        && let Some(chat) = sync.chat_id()
    {
        p.agent.tags = vec![format!("chat:{chat}")];
    }
    say_notices(&p.notices, ui);
    for line in &taken.said {
        ui.say(&ui.dim(line));
    }
    if taken.any() {
        reload_history(p);
    }
}

/// Built-in skills an Arena admin turned off, as Arena last said.
fn arena_off(data: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(ca_agent::library::arena_dir(data).join("off.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Arena's skills, kept for this session and the next (read at each message, so they are used as soon as they come).
fn refresh_arena_skills(app: &App, notices: &Notices) {
    if !app.connection.signed_in() {
        return;
    }
    let Ok(arena) = app.arena() else { return };
    let data = app.paths.data.clone();
    let builtin = app.settings.bool(&schema::BUILTIN_SKILLS);
    let notices = notices.clone();
    tokio::spawn(async move {
        let Ok(Ok(got)) = tokio::time::timeout(Duration::from_secs(15), arena.skills()).await else { return };
        let skills: Vec<(String, std::collections::BTreeMap<String, String>)> =
            got.skills.into_iter().map(|k| (k.name, k.files)).collect();
        match ca_agent::library::keep_arena(&data, &skills) {
            Ok(problems) => {
                if let Ok(mut n) = notices.lock() {
                    n.extend(problems.into_iter().map(|p| (true, p)));
                }
            }
            Err(e) => {
                if let Ok(mut n) = notices.lock() {
                    n.push((true, format!("Arena's skills could not be kept: {e}")));
                }
            }
        }
        let _ = std::fs::write(
            ca_agent::library::arena_dir(&data).join("off.json"),
            serde_json::to_string(&got.off).unwrap_or_default(),
        );
        let _ = ca_agent::library::keep_builtin(&data, &got.off, builtin);
    });
}

/// Before the person's message: what is remembered that matters to it (a few hundred tokens at most), and after a long
/// turn the reminder to keep what it worked out.
pub async fn before_message(p: &mut Prepared, message: &str) {
    if let Some(note) = p.memory.recall(message, 1_500).await {
        p.agent.note(note);
    }
    if std::mem::take(&mut p.nudge) {
        p.agent.note("(Code Arena: the last turn was a long one. If it worked out a way of doing something worth doing again, keep it as a skill with skill_save; a lasting fact about this project, with remember_project.)".into());
    }
}

/// What `/memory` and `/remember` do.
pub async fn memory_command(p: &mut Prepared, name: &str, rest: &str, ui: crate::ui::Ui) -> String {
    let m = p.memory.clone();
    if m.denied() {
        return ca_memory::DENIED.into();
    }
    if name == "/remember" {
        let (person, text) = match rest.strip_prefix("--person") {
            Some(t) => (true, t.trim()),
            None => (false, rest),
        };
        return match m.remember(person, text, "fact").await {
            Ok((true, place)) => format!("Remembered {place}."),
            Ok((false, place)) => format!("Already remembered {place}."),
            Err(e) => e,
        };
    }
    if let Some(n) = rest.strip_prefix("forget").map(str::trim) {
        return match n.parse::<usize>() {
            Ok(n) => match m.forget(n).await {
                Ok(t) => format!("Forgot: {t}"),
                Err(e) => e,
            },
            Err(_) => "/memory forget N: the number /memory gives it.".into(),
        };
    }
    let all = m.listed();
    let mut out = vec![if m.online() {
        format!("Arena's memory (the web chat's too), for {}:", m.project_key)
    } else {
        "Kept on this machine (it goes to Arena when Code Arena next reaches it):".into()
    }];
    if all.is_empty() {
        out.push(ui.dim("  nothing yet: the agent keeps what a later session should know with remember, and you with /remember TEXT."));
    }
    for (i, (_, scope, text)) in all.iter().enumerate() {
        out.push(format!("{:>3}. {text}  {}", i + 1, ui.dim(&format!("({scope})"))));
    }
    out.push(ui.dim("/memory forget N forgets one; /remember TEXT keeps one for this project (/remember --person TEXT for every project)."));
    out.join("\n")
}

/// What the model reads, from the session's store again (after the web's messages came in).
pub fn reload_history(p: &mut Prepared) {
    let Ok(store) = p.store.lock() else { return };
    let Ok((history, _)) = model_history(&store, &p.session) else { return };
    drop(store);
    p.agent.notes = history
        .iter()
        .enumerate()
        .map(|(i, m)| i == 0 && m.content.starts_with("(Code Arena: the earlier part"))
        .collect();
    p.agent.history = history;
}

/// The plan as the terminal shows it.
pub fn render_plan(plan: &ca_tools::plan::Plan, ui: crate::ui::Ui) -> String {
    use ca_tools::plan::{BLOCKED, DOING, DONE, PROPOSED};
    if plan.is_empty() {
        return ui.dim("(no plan)");
    }
    let mut lines = vec![format!(
        "{}{}{}",
        ui.bold(&plan.title),
        ui.dim(&format!("  {}/{} done", plan.done(), plan.steps.len())),
        if plan.review.as_deref() == Some(PROPOSED) { ui.yellow("  proposed") } else { String::new() }
    )];
    if let Some(g) = &plan.goal {
        lines.push(ui.dim(g));
    }
    for s in &plan.steps {
        let mark = match s.status.as_str() {
            DONE => ui.green("☒ "),
            DOING => ui.yellow("◐ "),
            BLOCKED => ui.red("⊘ "),
            _ => "☐ ".into(),
        };
        let title = match s.status.as_str() {
            DONE => ui.dim(&s.title),
            DOING => ui.bold(&s.title),
            _ => s.title.clone(),
        };
        let check = match &s.check {
            None => String::new(),
            Some(c) => {
                let c: String = c.replace('\n', " ").chars().take(60).collect();
                format!(
                    "{}{}",
                    ui.dim(&format!("  ✓ {c}")),
                    match s.check_result.as_deref() {
                        Some("passed") => ui.green(" passed"),
                        Some("failed") => ui.red(" failed"),
                        _ => String::new(),
                    }
                )
            }
        };
        lines.push(format!("{mark}{}. {title}{check}", s.number));
    }
    lines.join("\n")
}

/// `/plan approve | approve ask | change N what | another why | reject why`: the person's review of the plan the agent
/// proposed in plan mode. The plan and the mode change here; what comes back is said to the agent as the person's next
/// message.
pub fn review_plan(p: &mut Prepared, action: &str, rest: &str) -> Result<String, String> {
    use ca_tools::plan::{APPROVED, PROPOSED};
    let plan = p.agent.ctx.plan.get();
    if plan.review.as_deref() != Some(PROPOSED) || plan.is_empty() {
        return Err("There is no plan waiting for its review: in plan mode (/mode plan) the agent proposes one.".into());
    }
    let words = rest.trim();
    let why = if words.is_empty() {
        ".".to_string()
    } else {
        format!(": {words}{}", if words.ends_with(['.', '!', '?']) { "" } else { "." })
    };
    match action {
        "approve" => {
            let ask = words.eq_ignore_ascii_case("ask");
            p.agent.policy.mode = if ask { ca_agent::Mode::Ask } else { ca_agent::Mode::AutoEdit };
            let mut approved = plan;
            approved.review = Some(APPROVED.into());
            p.agent.ctx.plan.set(approved);
            Ok("I approve the plan. Carry it out now, step by step: mark a step doing as you start it, run its check when it is done, mark it done only once the check passed, and change the plan (saying why) when something turns out otherwise.".into())
        }
        "change" => {
            let (n, what) = words.split_once(char::is_whitespace).unwrap_or((words, ""));
            let n: u32 = n.trim_start_matches('#').parse().map_err(|_| {
                let have: Vec<String> = plan.steps.iter().map(|s| s.number.to_string()).collect();
                format!("Say which step to change: one of {}.", have.join(", "))
            })?;
            let step = plan.steps.iter().find(|s| s.number == n).ok_or_else(|| format!("The plan has no step {n}."))?;
            if what.trim().is_empty() {
                return Err(format!("Say what should change in step {n}."));
            }
            Ok(format!("Change step {n} of the plan ({}): {}\n\nThen propose the plan again.", step.title, what.trim()))
        }
        "another" => Ok(format!("Propose another way to do this{why} Make the plan again with the plan tool.")),
        "reject" => {
            p.agent.ctx.plan.set(ca_tools::plan::Plan::default());
            Ok(format!("I reject the plan{why} Do not carry it out; I will say what to do next."))
        }
        _ => Err("A plan's review is approve, approve ask, change N, another or reject.".into()),
    }
}

/// `/plan undo N`: the files as they were before step N began (and before every step begun after it), those steps to do
/// again; the agent is told. What commands changed is not undone.
pub fn undo_step(p: &mut Prepared, n: u32) -> Result<String, String> {
    let Some((steps, r)) = p.agent.ctx.checkpoints.undo_step(n) else {
        let begun = p.agent.ctx.checkpoints.steps();
        return Err(if begun.is_empty() {
            "No step has begun in this run: there is nothing to undo.".into()
        } else {
            let b: Vec<String> = begun.iter().map(u32::to_string).collect();
            format!("Step {n} has not begun in this run: steps {} have.", b.join(", "))
        });
    };
    let mut plan = p.agent.ctx.plan.get();
    for s in plan.steps.iter_mut().filter(|s| steps.contains(&s.number)) {
        s.status = ca_tools::plan::TODO.into();
        s.check_result = None;
        s.check_output = None;
        s.checked_at = None;
    }
    p.agent.ctx.plan.set(plan);
    let ws = &p.agent.ctx.workspace;
    let files: Vec<String> = r.put_back.iter().chain(&r.removed).map(|f| ws.display(f)).collect();
    let nums: Vec<String> = steps.iter().map(u32::to_string).collect();
    let said = format!(
        "I undid step{} {} of the plan: {}{} What commands changed is not undone. The steps are to do again.",
        if steps.len() == 1 { "" } else { "s" },
        nums.join(", "),
        if files.is_empty() {
            "no file had been written since it began.".to_string()
        } else {
            format!("{} {} as before it began.", files.join(", "), if files.len() == 1 { "is" } else { "are" })
        },
        if r.failed.is_empty() {
            String::new()
        } else {
            let f: Vec<String> = r.failed.iter().map(|(p, e)| format!("{} ({e})", ws.display(p))).collect();
            format!(" Not put back: {}.", f.join(", "))
        }
    );
    p.agent.note(format!("(Code Arena: {said})"));
    Ok(said)
}

/// `/plan resume`, `code-arena --plan`: the plan an earlier session of this folder left, carried on here; the agent is
/// told where it stands and to see what that session changed before going on.
pub fn resume_plan(p: &mut Prepared) -> Result<String, String> {
    if !p.agent.ctx.plan.get().is_empty() {
        return Err("This session has a plan of its own: /plan shows it.".into());
    }
    let left = p.store.lock().ok().and_then(|s| s.plan_left(&p.folder, &p.session).ok().flatten());
    let Some((_from, updated, json)) = left else {
        return Err("No earlier session of this folder left a plan.".into());
    };
    let plan: ca_tools::plan::Plan =
        serde_json::from_str(&json).map_err(|e| format!("The plan left could not be read: {e}"))?;
    if plan.is_empty() || plan.done() == plan.steps.len() {
        return Err("The plan the last session left is done: nothing to carry on.".into());
    }
    let when = chrono::DateTime::from_timestamp_millis(updated)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    let (done, total) = (plan.done(), plan.steps.len());
    p.agent.note(format!(
        "(Code Arena: the plan an earlier session of this folder left ({when}) is carried on here, {done} of {total} steps done:\n\n{}\n\nBefore going on, see what that session changed (git status, git log, the files of the step that was doing), then carry on from the step that stands next.)",
        plan.text()
    ));
    let title = plan.title.clone();
    p.agent.ctx.plan.set(plan);
    Ok(format!("Carried on: \"{title}\" from the session of {when}, {done} of {total} steps done."))
}

/// Says what waited, as the person reads it between turns.
pub fn say_notices(notices: &Notices, ui: crate::ui::Ui) {
    let lines: Vec<(bool, String)> = notices.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default();
    for (problem, line) in lines {
        ui.say(&if problem { ui.yellow(&line) } else { ui.dim(&line) });
    }
}

/// Keeps each message in the session's store as it is added.
pub struct StoreRecorder {
    pub store: Arc<Mutex<Store>>,
    pub session: String,
    /// Told of each message, to send it to Arena.
    pub sync: Option<Arc<ca_sync::ChatSync>>,
}

impl Recorder for StoreRecorder {
    fn record(&mut self, message: &Message, begins_turn: bool) -> Option<i64> {
        let s = self.store.lock().ok()?;
        match s.append(&self.session, message, begins_turn) {
            Ok(seq) => {
                drop(s);
                if let Some(sync) = &self.sync {
                    sync.poke();
                }
                Some(seq)
            }
            Err(e) => {
                eprintln!("The session could not be kept: {e}");
                None
            }
        }
    }

    fn compacted(&mut self, summary: &str, kept: usize) {
        let Ok(s) = self.store.lock() else { return };
        let total = s.summary(&self.session).map(|x| x.messages).unwrap_or(0);
        let through = total - kept as i64 - 1;
        if through >= 0
            && let Err(e) = s.compact(&self.session, through, summary)
        {
            eprintln!("The compaction could not be kept: {e}");
        }
        drop(s);
        if let Some(sync) = &self.sync {
            sync.poke();
        }
    }

    fn draft(&mut self, text: &str, reasoning: &str) {
        self.draft_kept(text, reasoning);
    }
}

impl StoreRecorder {
    fn draft_kept(&mut self, text: &str, reasoning: &str) {
        if let Ok(s) = self.store.lock() {
            let _ = s.draft(&self.session, text, reasoning);
        }
        if let Some(sync) = &self.sync {
            sync.draft(text, reasoning);
        }
    }
}

impl ca_agent::Aside for StoreRecorder {
    fn keep(&mut self, text: &str) -> Option<String> {
        self.store.lock().ok()?.keep_result(&self.session, text).ok()
    }
}

pub fn open_store(app: &App) -> anyhow::Result<Store> {
    Store::open(&app.paths.data.join("sessions.db")).context("the session store")
}

/// What the model reads of a stored session: its summary as a message of Code Arena's, then the rest.
pub fn model_history(store: &Store, id: &str) -> anyhow::Result<(Vec<Message>, String)> {
    let view = store.model_view(id)?;
    let total = store.summary(id)?.messages;
    let mut history = Vec::new();
    let said = match (&view.summary, view.through) {
        (Some(s), Some(t)) => {
            history.push(Message::user(format!(
                "(Code Arena: the earlier part of this conversation, messages 1 to {}, is summarized here; every message is kept.)\n\n{s}",
                t + 1
            )));
            format!("{total} messages (compacted: the model reads a summary of the first {})", t + 1)
        }
        _ => format!("{total} messages"),
    };
    history.extend(view.messages);
    Ok((history, said))
}

pub async fn prepare(app: &mut App, o: &Opts, approver: Arc<dyn Approver>, resume: Resume) -> anyhow::Result<Prepared> {
    let ui = app.ui;
    let cwd = match &o.cwd {
        Some(c) => c.clone(),
        None => std::env::current_dir()?,
    };
    let mut added = app.settings.strings(&schema::ALLOWED_PATHS).into_iter().map(PathBuf::from).collect::<Vec<_>>();
    added.extend(o.add_dirs.iter().cloned());
    let ws = Workspace::open(&cwd, &added).with_context(|| format!("the folder {}", cwd.display()))?;

    // The project's own settings, now that the project is known.
    *app = App::load(Some(ws.root().join(".code-arena").join("settings.json")))?;
    // Only a repository is taken for a project: a folder such as the Desktop gets no .code-arena/.
    if app.settings.bool(&schema::PROJECT_FOLDER) && ws.in_repository() {
        match ca_workspace::ensure_project_folder(ws.root()) {
            Ok(c) if !c.files.is_empty() => ui.say(&ui.dim(&format!(
                "Made {} with its default files ({} says what each is for).",
                c.folder.display(),
                c.folder.join("README.md").display()
            ))),
            Ok(_) => {}
            Err(e) => ui.say(&ui.yellow(&format!("Could not make .code-arena/: {e}"))),
        }
    }

    if let Some(m) = &o.mode {
        app.settings.set_flag(&schema::MODE, json!(m));
    }
    let mode_word = app.settings.string(&schema::MODE).unwrap_or_else(|| "ask".into());
    let Some(mode) = Mode::parse(&mode_word) else {
        bail!("--mode {mode_word}: the modes are ask, auto-edit, plan and yolo")
    };
    if mode == Mode::Yolo {
        ui.say(&ui.yellow("yolo: every command and edit runs without asking. Use it in a folder you can throw away."));
    }

    let folder = ws.cwd().display().to_string();
    let store = open_store(app)?;
    let (session, history, resumed) = match &resume {
        Resume::New => (None, vec![], None),
        Resume::Latest => match store.latest(&folder)? {
            Some(s) => (Some(s.id), vec![], Some(())),
            None => {
                ui.say(&ui.dim("No earlier session in this folder: a new one begins."));
                (None, vec![], None)
            }
        },
        Resume::Id(id) => (Some(store.summary(id).map(|s| s.id)?), vec![], Some(())),
    };
    let (session, history, resumed) = match (session, resumed) {
        (Some(id), Some(())) => {
            if store.recover_draft(&id)?.is_some() {
                ui.say(&ui.yellow("The last answer was being written when Code Arena closed: what it got to is kept."));
            }
            let (h, said) = model_history(&store, &id)?;
            (id, h, Some(said))
        }
        _ => (String::new(), history, None),
    };

    let mut gateway = app.gateway()?;
    // Managed: Arena's admins decide whether the person's own providers are used (its chat config's
    // codeArenaProviders; not said: they are).
    if app.edition() == ca_settings::Edition::Managed && gateway.providers().next().is_some() {
        let allowed = match app.arena() {
            Ok(a) => tokio::time::timeout(Duration::from_secs(8), a.config())
                .await
                .ok()
                .and_then(|c| c.ok())
                .and_then(|c| c.get("codeArenaProviders").and_then(serde_json::Value::as_bool))
                .unwrap_or(true),
            Err(_) => true,
        };
        if !allowed {
            gateway.set_providers(vec![]);
            ui.say(&ui.yellow("Arena's admins do not let Code Arena use other providers (Admin → Settings → Code Arena): only Arena's models are offered."));
        }
    }
    let listed = gateway.models().await.unwrap_or_default();
    let stored_model =
        if session.is_empty() { None } else { store.summary(&session).ok().map(|s| s.model).filter(|m| !m.is_empty()) };
    let model = match o.model.clone().or(stored_model).or_else(|| app.settings.string(&schema::MODEL)) {
        Some(m) => m,
        None => listed.first().map(|m| m.id.clone()).context("the gateway lists no model for your key")?,
    };
    let info = listed.iter().find(|m| m.id == model);
    let configured = app.settings.integer(&schema::CONTEXT).max(0) as u64;
    let size = info.and_then(|m| m.context).or((configured > 0).then_some(configured));
    // When it compacts: as the person set it here, else as their Arena account says (Your account → Chat).
    let mut compact_at_setting = app.settings.integer(&schema::COMPACT_AT);
    if !app.settings.given(&schema::COMPACT_AT)
        && app.edition() == ca_settings::Edition::Managed
        && let Ok(a) = app.arena()
        && let Ok(Ok(Some(account))) = tokio::time::timeout(Duration::from_secs(8), a.compact_at()).await
    {
        compact_at_setting = account;
    }
    let compact_at = match compact_at_setting {
        0 => None,
        p => Some(p.clamp(20, 95) as u64),
    };
    let window = ca_agent::Window::new(
        size,
        info.and_then(|m| m.max_output),
        compact_at,
        app.settings.integer(&schema::COMPACT_TARGET) as u64,
    );
    let session = if session.is_empty() { store.create(&folder, &model, mode.name())? } else { session };
    store.set_model(&session, &model, mode.name())?;
    let store = Arc::new(Mutex::new(store));
    let notices: Notices = Arc::new(Mutex::new(Vec::new()));
    refresh_arena_skills(app, &notices);
    let sync = start_sync(app, &store, &session, &folder, &model, &notices, ca_memory::keys::project_key(ws.root()));

    let thinking =
        Thinking::parse(&o.thinking.clone().or_else(|| app.settings.string(&schema::THINKING)).unwrap_or_default());
    let shell_setting = app.settings.string(&schema::SHELL);
    let (shell, _) = ca_tools::shell_for(shell_setting.as_deref());
    let person = instructions::person_candidates(&app.paths.config, app.home.as_deref());
    let read = instructions::read_all(&person, ws.root(), ws.cwd());
    // The built-in skills, as this version has them; Arena's, as it last gave them (refreshed below).
    let _ = ca_agent::library::keep_builtin(
        &app.paths.data,
        &arena_off(&app.paths.data),
        app.settings.bool(&schema::BUILTIN_SKILLS),
    );
    let sources = ca_agent::library::sources(ws.root(), &app.paths.config, app.home.as_deref(), Some(&app.paths.data));
    let library = ca_agent::library::load(&sources);
    // What the agent remembers: Arena's (else this machine's files), and the project's .remember/.
    let memory = Arc::new(ca_memory::Memory::new(
        if app.connection.signed_in() { app.arena().ok() } else { None },
        ws.cwd(),
        &app.paths.config,
        &app.paths.data,
        app.home.as_deref(),
    ));
    let remembered = {
        let said = std::sync::Mutex::new(Vec::new());
        let block = memory.start(&|s| said.lock().map(|mut v| v.push(s)).unwrap_or(())).await;
        for s in said.into_inner().unwrap_or_default() {
            ui.say(&ui.dim(&s));
        }
        let project_memory = app.settings.bool(&schema::PROJECT_MEMORY);
        if project_memory { format!("{block}{}", ca_memory::project::for_prompt(&memory.root)) } else { block }
    };
    let env = Environment {
        os: std::env::consts::OS.into(),
        shell,
        cwd: folder.clone(),
        branch: instructions::git_branch(ws.root()),
        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
        mode: mode.name().into(),
        interactive: o.interactive,
    };
    let ws_root = ws.root().to_path_buf();
    let media_dir = if ws.in_repository() {
        ws_root.join(".code-arena").join("media")
    } else {
        ws.cwd().join(".code-arena").join("media")
    };
    let mut agent = Agent {
        gateway,
        model,
        thinking,
        max_tokens: None,
        tools: ca_tools::builtin(),
        ctx: Context::new(
            Arc::new(ws),
            shell_setting,
            Duration::from_secs(app.settings.integer(&schema::SHELL_TIMEOUT).max(1) as u64),
        ),
        policy: Policy::new(mode, Rules::from_setting(&app.settings.value(&schema::PERMISSIONS))),
        approver,
        max_steps: app.settings.integer(&schema::MAX_STEPS).max(1) as usize,
        system: {
            let mut s = instructions::system_prompt(&env, &read);
            if let Some(n) = library.skills_note() {
                s.push('\n');
                s.push_str(&n);
            }
            s.push_str(&remembered);
            s
        },
        history,
        recorder: Some(Box::new(StoreRecorder { store: store.clone(), session: session.clone(), sync: sync.clone() })),
        inbox: None,
        deferred: vec![],
        joins: None,
        window,
        aside: Some(Box::new(StoreRecorder { store: store.clone(), session: session.clone(), sync: None })),
        notes: vec![],
        tags: vec![],
        skip: Arc::default(),
    };
    // The sandbox, as the settings and this system allow.
    let sandbox_mode = app.settings.string(&schema::SANDBOX).unwrap_or_else(|| "auto".into());
    if sandbox_mode != "off" {
        match ca_tools::sandbox::find() {
            Ok(kind) => {
                let allowed = agent.ctx.workspace.allowed().to_vec();
                let extra: Vec<PathBuf> =
                    app.settings.strings(&schema::SANDBOX_WRITABLE).into_iter().map(PathBuf::from).collect();
                let own = vec![app.paths.config.clone(), app.paths.data.clone()];
                let runtime = app.settings.env().get("XDG_RUNTIME_DIR").map(PathBuf::from);
                let s = ca_tools::sandbox::Sandbox::new(
                    kind,
                    app.settings.bool(&schema::SANDBOX_NETWORK),
                    &allowed,
                    &extra,
                    // The system's home: its secrets stay hidden from a portable copy too (CODE_ARENA_HOME).
                    dirs::home_dir().as_deref(),
                    &own,
                    runtime.as_deref(),
                );
                agent.ctx.sandbox = Some(Arc::new(s));
            }
            Err(why) if sandbox_mode == "on" => bail!("tools.sandbox is on, but {why}"),
            Err(why) => {
                ui.say(&ui.dim(&format!("No sandbox: {why}. Commands run as you; the mode is what guards them.")))
            }
        }
    }
    // The session's working plan: as the store has it, every change kept (and sent to Arena with the chat), each step's
    // writes kept by the checkpoints for /plan undo.
    if let Some(json) = store.lock().ok().and_then(|s| s.plan(&session).ok().flatten())
        && let Ok(plan) = serde_json::from_str::<ca_tools::plan::Plan>(&json)
    {
        if resumed.is_some() && !plan.is_empty() {
            agent.note(format!("(Code Arena: your working plan as it stands.)\n{}", plan.text()));
        }
        agent.ctx.plan.load(plan);
    }
    {
        let (st, id, sy) = (store.clone(), session.clone(), sync.clone());
        agent.ctx.plan.on_change(Arc::new(move |plan| {
            let json = (!plan.is_empty()).then(|| serde_json::to_string(plan).unwrap_or_default());
            if let Ok(s) = st.lock() {
                let _ = s.set_plan(&id, json.as_deref());
            }
            if let Some(sy) = &sy {
                sy.poke();
            }
        }));
        let cp = agent.ctx.checkpoints.clone();
        agent.ctx.plan.on_begin(Arc::new(move |n| cp.begin_step(n)));
    }
    // A conversation keeps commands with no time limit and in the background; -p ends with its answer.
    if o.interactive {
        agent.ctx.jobs = Some(Arc::new(ca_tools::jobs::Jobs::default()));
    }
    // A resumed session's summary is Code Arena's note, not the person's words.
    agent.notes = agent
        .history
        .iter()
        .enumerate()
        .map(|(i, m)| i == 0 && m.content.starts_with("(Code Arena: the earlier part"))
        .collect();
    // What the session's store gives the model: results kept aside, and the session's own messages.
    agent.tools.extend(crate::session_tools::tools(store.clone(), session.clone(), folder.clone()));
    agent.tools.push(ca_agent::library::skill_tool(sources.clone()));
    let project_memory = app.settings.bool(&schema::PROJECT_MEMORY);
    agent.tools.extend(
        ca_memory::tools(memory.clone()).into_iter().filter(|t| project_memory || t.def().name != "remember_project"),
    );
    // Checklists are kept in Arena, with the session's chat.
    if let Some(s) = &sync {
        agent.tools.push(Arc::new(crate::checklist::CheckTool { sync: s.clone() }));
    }
    // The MCP servers: those that answer within a few seconds now, the others when they do.
    let local: Vec<String> = agent.tools.iter().map(|t| t.def().name).collect();
    let (configured, notes) = crate::servers::configured(app, &local);
    for n in notes {
        ui.say(&ui.yellow(&n));
    }
    for s in configured.iter().filter(|s| s.trusted) {
        agent.policy.rules.allow(&format!("mcp__{}__*", s.name));
    }
    let servers: crate::servers::Servers = Arc::new(configured);
    let wait = if o.interactive { app.settings.integer(&schema::SERVER_WAIT).max(0) as u64 } else { 25 };
    let on_demand = app.settings.integer(&schema::TOOL_CHARS).max(0) as usize;
    let (joined, later) =
        crate::servers::start(servers.clone(), local, Duration::from_secs(wait), on_demand, media_dir).await;
    for j in joined {
        agent.tools.extend(j.tools);
        agent.deferred.extend(j.deferred);
        if let Some(n) = j.note {
            agent.note(n);
        }
    }
    agent.joins = Some(later);
    for line in crate::servers::describe(&servers) {
        if !line.contains("connected,") {
            ui.say(&ui.dim(&line));
        }
    }
    let mut prepared = Prepared {
        agent,
        store,
        session,
        folder,
        resumed,
        servers,
        sources,
        disabled: vec![],
        sync,
        notices,
        said_next: None,
        memory,
        nudge: false,
    };
    if prepared.resumed.is_some() {
        crate::checklist::remind(&mut prepared).await;
        crate::projects::take_up(&mut prepared, false).await;
    }
    if o.carry_plan {
        match resume_plan(&mut prepared) {
            Ok(said) => {
                ui.say(&ui.dim(&said));
                ui.say(&render_plan(&prepared.agent.ctx.plan.get(), ui));
            }
            Err(why) => ui.say(&ui.yellow(&why)),
        }
    }
    Ok(prepared)
}

/// The session kept in step with a chat in Arena, when it is signed in and `arena.syncChats` is on.
fn start_sync(
    app: &App,
    store: &Arc<Mutex<Store>>,
    session: &str,
    folder: &str,
    model: &str,
    notices: &Notices,
    project: String,
) -> Option<Arc<ca_sync::ChatSync>> {
    if !app.settings.bool(&schema::SYNC_CHATS) || !app.connection.signed_in() {
        return None;
    }
    let arena = app.arena().ok()?;
    let caps = ca_arena::Kept::file(&app.paths.servers_dir(), arena.base());
    let n = notices.clone();
    let notice: ca_sync::Notice = Arc::new(move |problem, line| {
        if let Ok(mut q) = n.lock() {
            q.push((problem, line));
        }
    });
    let sync = ca_sync::ChatSync::new(
        arena,
        store.clone(),
        session.to_string(),
        folder.to_string(),
        Some(project),
        model.to_string(),
        notice,
    );
    sync.start_checked(caps);
    Some(sync)
}
