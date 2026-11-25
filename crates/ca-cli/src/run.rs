//! `-p`: one answer, printed, then out. The answer on stdout, the progress on stderr. It cannot ask, so what would
//! ask is refused unless the mode allows it.

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, bail};
use ca_agent::instructions::{self, Environment};
use ca_agent::policy::Policy;
use ca_agent::{Agent, Events, Mode, NoAsking, Rules};
use ca_llm::Thinking;
use ca_settings::schema;
use ca_tools::{Context, Output};
use ca_workspace::Workspace;
use serde_json::json;

use crate::app::App;
use crate::ui::{self, Ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

pub struct RunArgs {
    pub prompt: String,
    pub cwd: Option<PathBuf>,
    pub add_dirs: Vec<PathBuf>,
    pub mode: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub output: OutputFormat,
}

/// The progress of a turn on stderr; the answer's words on stdout as they come (text output).
struct Progress {
    ui: Ui,
    stream: bool,
    at_line_start: bool,
}

impl Events for Progress {
    fn text(&mut self, d: &str) {
        if self.stream {
            let mut o = std::io::stdout().lock();
            let _ = o.write_all(d.as_bytes());
            let _ = o.flush();
            self.at_line_start = d.ends_with('\n');
        }
    }
    fn notice(&mut self, t: &str) {
        self.ui.say(&self.ui.yellow(t));
    }
    fn step_end(&mut self) {
        if self.stream && !self.at_line_start {
            println!();
            self.at_line_start = true;
        }
    }
    fn tool_start(&mut self, name: &str, subject: &str) {
        let s: String = subject.lines().next().unwrap_or("").chars().take(100).collect();
        self.ui.say(&format!("{} {name} {}", self.ui.dim("●"), self.ui.dim(&s)));
    }
    fn tool_end(&mut self, _name: &str, o: &Output) {
        let first = o.text.lines().next().unwrap_or("").chars().take(120).collect::<String>();
        let line = format!("  └ {first}");
        self.ui.say(&if o.is_error { self.ui.red(&line) } else { self.ui.dim(&line) });
    }
}

fn read_prompt(prompt: &str) -> anyhow::Result<String> {
    if prompt != "-" {
        return Ok(prompt.to_string());
    }
    if std::io::stdin().is_terminal() {
        bail!("-p - reads the prompt from a pipe: git diff | code-arena -p -");
    }
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s)?;
    Ok(s)
}

pub async fn run_once(app: &mut App, args: RunArgs) -> anyhow::Result<i32> {
    let ui = app.ui;
    let prompt = read_prompt(&args.prompt)?;
    if prompt.trim().is_empty() {
        bail!("the prompt is empty");
    }
    let cwd = match &args.cwd {
        Some(c) => c.clone(),
        None => std::env::current_dir()?,
    };
    let mut added = app.settings.strings(&schema::ALLOWED_PATHS).into_iter().map(PathBuf::from).collect::<Vec<_>>();
    added.extend(args.add_dirs.iter().cloned());
    let ws = Workspace::open(&cwd, &added).with_context(|| format!("the folder {}", cwd.display()))?;

    // The project's own settings, now that the project is known.
    let project_settings = ws.root().join(".code-arena").join("settings.json");
    *app = App::load(Some(project_settings))?;
    if app.settings.bool(&schema::PROJECT_FOLDER) {
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

    if let Some(m) = &args.mode {
        app.settings.set_flag(&schema::MODE, json!(m));
    }
    let mode_word = app.settings.string(&schema::MODE).unwrap_or_else(|| "ask".into());
    let Some(mode) = Mode::parse(&mode_word) else {
        bail!("--mode {mode_word}: the modes are ask, auto-edit, plan and yolo")
    };
    if mode == Mode::Yolo {
        ui.say(&ui.yellow("yolo: every command and edit runs without asking. Use it in a folder you can throw away."));
    }

    let gateway = app.gateway()?;
    let model = match args.model.clone().or_else(|| app.settings.string(&schema::MODEL)) {
        Some(m) => m,
        None => gateway
            .models()
            .await?
            .into_iter()
            .next()
            .map(|m| m.id)
            .context("the gateway lists no model for your key")?,
    };
    let thinking =
        Thinking::parse(&args.thinking.clone().or_else(|| app.settings.string(&schema::THINKING)).unwrap_or_default());
    let shell_setting = app.settings.string(&schema::SHELL);
    let (shell, _) = ca_tools::shell_for(shell_setting.as_deref());
    let person = instructions::person_candidates(&app.paths.config, app.home.as_deref());
    let read = instructions::read_all(&person, ws.root(), ws.cwd());
    let env = Environment {
        os: std::env::consts::OS.into(),
        shell,
        cwd: ws.cwd().display().to_string(),
        branch: instructions::git_branch(ws.root()),
        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
        mode: mode.name().into(),
        interactive: false,
    };
    let mut agent = Agent {
        gateway,
        model: model.clone(),
        thinking,
        max_tokens: None,
        tools: ca_tools::builtin(),
        ctx: Context {
            workspace: Arc::new(ws),
            shell: shell_setting,
            shell_timeout: Duration::from_secs(app.settings.integer(&schema::SHELL_TIMEOUT).max(1) as u64),
        },
        policy: Policy::new(mode, Rules::from_setting(&app.settings.value(&schema::PERMISSIONS))),
        approver: Arc::new(NoAsking),
        max_steps: app.settings.integer(&schema::MAX_STEPS).max(1) as usize,
        system: instructions::system_prompt(&env, &read),
        history: vec![],
    };
    let mut progress = Progress { ui, stream: args.output == OutputFormat::Text, at_line_start: true };
    let result = agent.turn(&prompt, &mut progress).await;
    match (args.output, result) {
        (OutputFormat::Text, Ok(out)) => {
            if !progress.at_line_start {
                println!();
            }
            let u = out.usage;
            let cached = (u.cached * 100).checked_div(u.prompt).map(|p| format!(" ({p}% cached)")).unwrap_or_default();
            ui.say(&ui.dim(&format!(
                "{} in{cached} · {} out · {} requests",
                ui::short(u.prompt),
                ui::short(u.completion),
                out.steps
            )));
            Ok(0)
        }
        (OutputFormat::Text, Err(e)) => {
            ui.say(&ui.red(&e.to_string()));
            Ok(1)
        }
        (OutputFormat::Json, r) => {
            let (result, is_error, usage) = match r {
                Ok(o) => (o.answer, false, o.usage),
                Err(e) => (e.to_string(), true, Default::default()),
            };
            let body = json!({
                "result": result,
                "is_error": is_error,
                "session": null,
                "model": model,
                "usage": {"input_tokens": usage.prompt, "cached_tokens": usage.cached, "output_tokens": usage.completion},
            });
            println!("{body}");
            Ok(i32::from(is_error))
        }
    }
}
