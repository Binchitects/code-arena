//! `-p`: one answer, printed, then out. The answer on stdout, the progress on stderr. It cannot ask, so what would
//! ask is refused unless the mode allows it.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::bail;
use ca_agent::{Events, NoAsking};
use ca_tools::Output;
use serde_json::json;

use crate::app::App;
use crate::session::{self, Opts, Resume};
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
    pub resume: Resume,
}

/// The progress of a turn on stderr; the answer's words on stdout as they come (text output).
pub struct Progress {
    ui: Ui,
    stream: bool,
    pub at_line_start: bool,
    /// Whitespace that began a step's words: held until words follow, so a step that only calls tools prints no
    /// blank lines.
    held: String,
    /// Whether this step printed any words yet.
    spoke: bool,
}

impl Progress {
    pub fn new(ui: Ui, stream: bool) -> Self {
        Self { ui, stream, at_line_start: true, held: String::new(), spoke: false }
    }
}

impl Events for Progress {
    fn text(&mut self, d: &str) {
        if !self.stream {
            return;
        }
        if !self.spoke {
            let words = d.trim_start();
            if words.is_empty() {
                self.held.push_str(d);
                return;
            }
            // The step's first words: what came before them was only whitespace.
            self.held.clear();
            self.spoke = true;
            self.write(words);
            return;
        }
        self.write(d);
    }
    fn notice(&mut self, t: &str) {
        self.ui.say(&self.ui.yellow(t));
    }
    fn step_end(&mut self) {
        if self.stream && !self.at_line_start {
            ui::out("\n");
            self.at_line_start = true;
        }
        self.held.clear();
        self.spoke = false;
    }
    fn tool_end(&mut self, name: &str, subject: &str, o: &Output) {
        let s: String = subject.lines().next().unwrap_or("").chars().take(100).collect();
        self.ui.say(&format!("{} {name} {}", self.ui.dim("●"), self.ui.dim(&s)));
        let first = o.text.lines().next().unwrap_or("").chars().take(120).collect::<String>();
        let line = format!("  └ {first}");
        self.ui.say(&if o.is_error { self.ui.red(&line) } else { self.ui.dim(&line) });
    }
}

impl Progress {
    fn write(&mut self, d: &str) {
        ui::out(d);
        self.at_line_start = d.ends_with('\n');
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
    let opts = Opts {
        cwd: args.cwd,
        add_dirs: args.add_dirs,
        mode: args.mode,
        model: args.model,
        thinking: args.thinking,
        interactive: false,
    };
    let mut p = session::prepare(app, &opts, Arc::new(NoAsking), args.resume).await?;
    let model = p.agent.model.clone();
    // `-p "/NAME words"`: a command or a skill of the project's or yours.
    let prompt = if prompt.trim_start().starts_with('/') {
        ca_agent::library::load(&p.sources).slash(prompt.trim()).unwrap_or(prompt)
    } else {
        prompt
    };
    let mut progress = Progress::new(ui, args.output == OutputFormat::Text);
    let result = p.agent.turn(&prompt, &mut progress).await;
    match (args.output, result) {
        (OutputFormat::Text, Ok(out)) => {
            if !progress.at_line_start {
                ui::out("\n");
            }
            ui.say(&ui.dim(&usage_line(&out)));
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
                "session": p.session,
                "model": model,
                "usage": {"input_tokens": usage.prompt, "cached_tokens": usage.cached, "output_tokens": usage.completion},
            });
            println!("{body}");
            Ok(i32::from(is_error))
        }
    }
}

/// `12.4k in (81% cached) · 310 out · 3 requests`
pub fn usage_line(out: &ca_agent::TurnOutcome) -> String {
    let u = out.usage;
    let cached = (u.cached * 100).checked_div(u.prompt).map(|p| format!(" ({p}% cached)")).unwrap_or_default();
    format!("{} in{cached} · {} out · {} requests", ui::short(u.prompt), ui::short(u.completion), out.steps)
}
