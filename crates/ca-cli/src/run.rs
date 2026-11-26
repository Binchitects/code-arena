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
    /// The model's thinking, shown dimmed before its answer (the chat; not -p).
    pub show_thinking: bool,
    thinking: bool,
}

impl Progress {
    pub fn new(ui: Ui, stream: bool) -> Self {
        Self {
            ui,
            stream,
            at_line_start: true,
            held: String::new(),
            spoke: false,
            show_thinking: false,
            thinking: false,
        }
    }
}

impl Events for Progress {
    fn reasoning(&mut self, d: &str) {
        if !self.stream || !self.show_thinking || (!self.thinking && d.trim().is_empty()) {
            return;
        }
        self.thinking = true;
        ui::out(&self.ui.dim(d));
        self.at_line_start = d.ends_with('\n');
    }
    fn text(&mut self, d: &str) {
        if !self.stream {
            return;
        }
        if self.thinking && !d.trim().is_empty() {
            // The thinking is over: the answer on a line of its own.
            self.thinking = false;
            ui::out(if self.at_line_start { "\n" } else { "\n\n" });
            self.at_line_start = true;
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
        self.thinking = false;
    }
    fn tool_end(&mut self, name: &str, subject: &str, o: &Output) {
        let s: String = subject.lines().next().unwrap_or("").chars().take(100).collect();
        self.ui.say(&format!("{} {name} {}", self.ui.dim("●"), self.ui.dim(&s)));
        // An edit shows its diff (a dozen lines at most); anything else its first line.
        let shown = if matches!(name, "edit_file" | "write_file") && !o.is_error { 13 } else { 1 };
        for (i, l) in o.text.lines().take(shown).enumerate() {
            let l: String = l.chars().take(160).collect();
            let line = if i == 0 { format!("  └ {l}") } else { format!("    {l}") };
            let painted = if o.is_error {
                self.ui.red(&line)
            } else if i > 0 && l.starts_with('+') {
                self.ui.green(&line)
            } else if i > 0 && l.starts_with('-') {
                self.ui.red(&line)
            } else {
                self.ui.dim(&line)
            };
            self.ui.say(&painted);
        }
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
    session::begin_turn(&mut p, ui).await;
    let result = p.agent.turn(&prompt, &mut progress).await;
    // The turn's messages reach the chat in Arena before the run ends (or are sent with the next session's).
    if let Some(sync) = &p.sync
        && !sync.flush(std::time::Duration::from_secs(10)).await
        && sync.pending() > 0
    {
        ui.say(&ui.dim(&format!(
            "{} message(s) wait to reach the chat in Arena: they go with this session's next run.",
            sync.pending()
        )));
    }
    if let Some(sync) = &p.sync {
        sync.end_turn().await;
    }
    crate::session::say_notices(&p.notices, ui);
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
    let requests = if out.steps == 1 { "request" } else { "requests" };
    format!("{} in{cached} · {} out · {} {requests}", ui::short(u.prompt), ui::short(u.completion), out.steps)
}
