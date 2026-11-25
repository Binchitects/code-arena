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
}

/// Keeps each message in the session's store as it is added.
pub struct StoreRecorder {
    pub store: Arc<Mutex<Store>>,
    pub session: String,
}

impl Recorder for StoreRecorder {
    fn record(&mut self, message: &Message, begins_turn: bool) {
        let Ok(s) = self.store.lock() else { return };
        if let Err(e) = s.append(&self.session, message, begins_turn) {
            eprintln!("The session could not be kept: {e}");
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

    let gateway = app.gateway()?;
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
    let compact_at = match app.settings.integer(&schema::COMPACT_AT) {
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

    let thinking =
        Thinking::parse(&o.thinking.clone().or_else(|| app.settings.string(&schema::THINKING)).unwrap_or_default());
    let shell_setting = app.settings.string(&schema::SHELL);
    let (shell, _) = ca_tools::shell_for(shell_setting.as_deref());
    let person = instructions::person_candidates(&app.paths.config, app.home.as_deref());
    let read = instructions::read_all(&person, ws.root(), ws.cwd());
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
        ctx: Context {
            workspace: Arc::new(ws),
            shell: shell_setting,
            shell_timeout: Duration::from_secs(app.settings.integer(&schema::SHELL_TIMEOUT).max(1) as u64),
        },
        policy: Policy::new(mode, Rules::from_setting(&app.settings.value(&schema::PERMISSIONS))),
        approver,
        max_steps: app.settings.integer(&schema::MAX_STEPS).max(1) as usize,
        system: instructions::system_prompt(&env, &read),
        history,
        recorder: Some(Box::new(StoreRecorder { store: store.clone(), session: session.clone() })),
        inbox: None,
        deferred: vec![],
        joins: None,
        window,
        aside: Some(Box::new(StoreRecorder { store: store.clone(), session: session.clone() })),
        notes: vec![],
    };
    // A resumed session's summary is Code Arena's note, not the person's words.
    agent.notes = agent
        .history
        .iter()
        .enumerate()
        .map(|(i, m)| i == 0 && m.content.starts_with("(Code Arena: the earlier part"))
        .collect();
    // What the session's store gives the model: results kept aside, and the session's own messages.
    agent.tools.extend(crate::session_tools::tools(store.clone(), session.clone(), folder.clone()));
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
    Ok(Prepared { agent, store, session, folder, resumed, servers })
}
