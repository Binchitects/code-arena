//! The `task` tool: the model hands a piece of work to a sub-agent (one of the `agents/NAME.md` the project or the
//! person wrote, or a general one), which works in a context of its own (its instructions, model and tools) and
//! reports back; its report is the call's result. Several calls in one answer run at once (four at most). Its calls ask
//! as the session's do; it has no `task` of its own (no sub-agent of a sub-agent) and not the session's plan.
//!
//! While it works, the crew's `watch` (when there is one, and the turn said the call's id) is told how it goes, by
//! [`SubEvent`]: `Started` once, a `Step` for each piece of its work (its thinking and words gathered, told at most
//! every [`SAY_EVERY`] and whenever a call or a step begins or ends), `Ended` once, even when its turn is dropped.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ca_llm::{Gateway, Thinking, ToolDef};
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::{Value, json};

use crate::context::Window;
use crate::library::Source;
use crate::policy::{Approver, Policy};
use crate::{Agent, Call, Events, TurnError};

/// What a sub-agent starts from: the session as it is (kept up to date by the session: its model, its mode).
#[derive(Clone)]
pub struct Crew {
    pub gateway: Gateway,
    pub model: String,
    pub thinking: Thinking,
    pub ctx: Context,
    pub policy: Policy,
    pub approver: Arc<dyn Approver>,
    /// The session's system prompt: the sub-agent's starts from it.
    pub system: String,
    /// The tools a sub-agent may have (the session's, but `task` and `plan`).
    pub tools: Vec<Arc<dyn Tool>>,
    pub sources: Vec<Source>,
    pub hooks: Arc<crate::hooks::Hooks>,
    pub max_steps: usize,
    pub window: Window,
    /// What a sub-agent's steps show the person (a line each).
    pub say: Arc<dyn Fn(String) + Send + Sync>,
    /// Who is told how each sub-agent goes (the IDE's Tasks view), when someone is.
    pub watch: Option<Watch>,
}

/// Who is told how a sub-agent goes.
pub type Watch = Arc<dyn Fn(SubEvent) + Send + Sync>;

/// How often a sub-agent's thinking and words are told while they stream.
pub const SAY_EVERY: Duration = Duration::from_millis(300);

/// How a sub-agent goes, each told with the id of the `task` call it answers.
#[derive(Debug, Clone)]
pub enum SubEvent {
    Started(Begun),
    Step { call: String, work: Work },
    Ended { call: String, end: End },
}

/// A sub-agent as it began.
#[derive(Debug, Clone)]
pub struct Begun {
    pub call: String,
    /// Its agent's name ("sub-agent": a general one).
    pub name: String,
    pub description: String,
    /// What it was asked, whole.
    pub prompt: String,
    pub tools: Vec<String>,
    pub model: String,
    /// One of its tools changes files or runs commands.
    pub edits: bool,
    /// Asked, it stops where it is.
    pub stop: Arc<Stop>,
}

/// A piece of a sub-agent's work.
#[derive(Debug, Clone, PartialEq)]
pub enum Work {
    /// Its thinking since the last piece.
    Thinking(String),
    /// Its words since the last piece.
    Text(String),
    Notice(String),
    ToolStart {
        id: String,
        name: String,
        subject: String,
    },
    ToolEnd {
        id: String,
        ok: bool,
        text: String,
        ms: u64,
    },
    /// A step ended, its window this full.
    StepEnd {
        tokens: u64,
    },
}

/// How a sub-agent ended.
#[derive(Debug, Clone, PartialEq)]
pub struct End {
    /// "done", "failed" or "stopped".
    pub status: &'static str,
    /// Its last words (empty: it said none, or it did not finish).
    pub report: String,
    pub error: Option<String>,
}

/// A sub-agent's stop: asked once, its turn ends where it is.
#[derive(Debug, Default)]
pub struct Stop {
    asked: AtomicBool,
    notify: tokio::sync::Notify,
}

impl Stop {
    pub fn ask(&self) {
        self.asked.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    pub fn asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }

    /// Comes back once it is asked (at once when it was).
    async fn wait(&self) {
        if !self.asked() {
            self.notify.notified().await;
        }
    }
}

/// The most of a call's result a step tells.
const TOLD_RESULT: usize = 2000;

/// Tells the watcher a sub-agent ended, once: when its run ends, or when it is dropped (its turn was stopped).
struct Ending {
    watch: Option<(Watch, String)>,
}

impl Ending {
    fn end(&mut self, end: End) {
        if let Some((w, call)) = self.watch.take() {
            w(SubEvent::Ended { call, end });
        }
    }
}

impl Drop for Ending {
    fn drop(&mut self) {
        self.end(End { status: "stopped", report: String::new(), error: Some("The turn was stopped.".into()) });
    }
}

pub struct TaskTool {
    pub crew: Arc<Mutex<Option<Crew>>>,
    pub at_once: Arc<tokio::sync::Semaphore>,
}

impl TaskTool {
    pub fn new(crew: Arc<Mutex<Option<Crew>>>) -> Self {
        TaskTool { crew, at_once: Arc::new(tokio::sync::Semaphore::new(4)) }
    }
}

/// A sub-agent's steps, shown to the person as lines of their own and told to the watcher.
struct Steps {
    name: String,
    say: Arc<dyn Fn(String) + Send + Sync>,
    watch: Option<(Watch, String)>,
    thinking: String,
    text: String,
    told: Instant,
    tokens: u64,
}

impl Steps {
    fn tell(&self, work: Work) {
        if let Some((w, call)) = &self.watch {
            w(SubEvent::Step { call: call.clone(), work });
        }
    }

    /// What it thought and said since the last piece, told.
    fn flush(&mut self) {
        self.told = Instant::now();
        if !self.thinking.is_empty() {
            let t = std::mem::take(&mut self.thinking);
            self.tell(Work::Thinking(t));
        }
        if !self.text.is_empty() {
            let t = std::mem::take(&mut self.text);
            self.tell(Work::Text(t));
        }
    }

    fn streamed(&mut self) {
        if self.told.elapsed() >= SAY_EVERY {
            self.flush();
        }
    }
}

impl Events for Steps {
    fn text(&mut self, delta: &str) {
        if self.watch.is_some() {
            if !self.thinking.is_empty() {
                self.flush();
            }
            self.text.push_str(delta);
            self.streamed();
        }
    }
    fn reasoning(&mut self, delta: &str) {
        if self.watch.is_some() {
            if !self.text.is_empty() {
                self.flush();
            }
            self.thinking.push_str(delta);
            self.streamed();
        }
    }
    fn notice(&mut self, t: &str) {
        (self.say)(format!("  ↳ {}: {t}", self.name));
        self.flush();
        self.tell(Work::Notice(t.to_string()));
    }
    fn tool_start(&mut self, call: &Call<'_>) {
        self.flush();
        self.tell(Work::ToolStart { id: call.id.into(), name: call.name.into(), subject: call.subject.into() });
    }
    fn tool_end(&mut self, call: &Call<'_>, o: &Output, took: Duration) {
        let s: String = call.subject.lines().next().unwrap_or("").chars().take(80).collect();
        (self.say)(format!("  ↳ {}: ● {} {s}{}", self.name, call.name, if o.is_error { " (failed)" } else { "" }));
        self.flush();
        let text = told(&o.text);
        self.tell(Work::ToolEnd { id: call.id.into(), ok: !o.is_error, text, ms: took.as_millis() as u64 });
    }
    fn tool_not_run(&mut self, call: &Call<'_>, why: &str, _denied: bool) {
        self.flush();
        self.tell(Work::ToolStart { id: call.id.into(), name: call.name.into(), subject: call.subject.into() });
        self.tell(Work::ToolEnd { id: call.id.into(), ok: false, text: told(why), ms: 0 });
    }
    fn step_window(&mut self, used: u64, _size: u64, _compact_at: Option<u64>) {
        self.tokens = used;
    }
    fn step_end(&mut self) {
        self.flush();
        self.tell(Work::StepEnd { tokens: self.tokens });
    }
}

/// A result as a step tells it: its start.
fn told(text: &str) -> String {
    match text.char_indices().nth(TOLD_RESULT) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    }
}

const SUB_TOOLS_LEFT_OUT: &[&str] = &["task", "plan", "run_steps", "goal_check"];

#[async_trait::async_trait]
impl Tool for TaskTool {
    fn def(&self) -> ToolDef {
        let lib = self.crew.lock().ok().and_then(|c| c.as_ref().map(|c| crate::library::load(&c.sources)));
        let agents: Vec<String> = lib
            .map(|l| l.agents.iter().map(|a| format!("{}: {}", a.name, a.description)).collect())
            .unwrap_or_default();
        ToolDef {
            name: "task".into(),
            description: format!(
                "Hands a piece of work to a sub-agent, which works in a context of its own and reports back: to search or read widely without filling your window, to review, or to do independent parts at once (several task calls in one answer run together). Give it all it needs in prompt: it does not see this conversation. Its report is what it says at the end.{}",
                if agents.is_empty() {
                    String::new()
                } else {
                    format!(" The sub-agents there are (by agent; none: a general one): {}.", agents.join("; "))
                }
            ),
            parameters: json!({"type": "object", "properties": {
                "description": {"type": "string", "description": "What it does, in a few words (shown to the person)."},
                "prompt": {"type": "string", "description": "The task, whole: what to do, where, and what to report."},
                "agent": {"type": "string", "description": "Which sub-agent (leave out for a general one)."}
            }, "required": ["description", "prompt"]}),
        }
    }
    fn kind(&self) -> Kind {
        // Handing it on changes nothing: the sub-agent's own calls ask as the session's do.
        Kind::Read
    }
    fn subject(&self, a: &Value) -> String {
        let d = a.get("description").and_then(Value::as_str).unwrap_or_default();
        match a.get("agent").and_then(Value::as_str) {
            Some(n) => format!("{n}: {d}"),
            None => d.to_string(),
        }
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let answers = ctx_call(ctx);
        let Some(crew) = self.crew.lock().ok().and_then(|c| c.clone()) else {
            return Output::error("Sub-agents are not set up in this run.");
        };
        let prompt = args.get("prompt").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        let description = args.get("description").and_then(Value::as_str).unwrap_or("a task").to_string();
        if prompt.is_empty() {
            return Output::error("Give the sub-agent its task in prompt: it does not see this conversation.");
        }
        let lib = crate::library::load(&crew.sources);
        let def = match args.get("agent").and_then(Value::as_str).filter(|n| !n.is_empty()) {
            Some(n) => match lib.agents.iter().find(|a| a.name == n) {
                Some(a) => Some(a.clone()),
                None => {
                    let names: Vec<&str> = lib.agents.iter().map(|a| a.name.as_str()).collect();
                    return Output::error(format!(
                        "There is no sub-agent {n}{}. Leave out agent for a general one.",
                        if names.is_empty() { String::new() } else { format!(" (there are: {})", names.join(", ")) }
                    ));
                }
            },
            None => None,
        };
        let name = def.as_ref().map(|d| d.name.clone()).unwrap_or_else(|| "sub-agent".into());
        let Ok(_slot) = self.at_once.clone().acquire_owned().await else {
            return Output::error("No sub-agent may start now.");
        };
        let tools: Vec<Arc<dyn Tool>> = crew
            .tools
            .iter()
            .filter(|t| !SUB_TOOLS_LEFT_OUT.contains(&t.def().name.as_str()))
            .filter(|t| def.as_ref().is_none_or(|d| d.tools.is_empty() || d.tools.iter().any(|n| *n == t.def().name)))
            .cloned()
            .collect();
        let system = format!(
            "{}\n\n# You are a sub-agent\n\nThe session's agent handed you one task; you work on it in a context of your own and report back. Do the task, then end with your report: what you found or did, with paths and lines, and what is left. Your report is all the session's agent sees of your work.{}",
            crew.system,
            def.as_ref()
                .map(|d| format!("\n\n# Your instructions ({})\n\n{}", d.name, d.instructions))
                .unwrap_or_default()
        );
        let mut ctx = crew.ctx.clone();
        // Its own plan: never the session's.
        ctx.plan = Arc::new(ca_tools::plan::PlanBook::default());
        let mut sub = Agent {
            gateway: crew.gateway.clone(),
            model: def
                .as_ref()
                .and_then(|d| d.model.clone())
                .filter(|m| !m.is_empty() && m != "inherit")
                .unwrap_or(crew.model.clone()),
            thinking: crew.thinking.clone(),
            max_tokens: None,
            tools,
            deferred: vec![],
            joins: None,
            ctx,
            policy: crew.policy.clone(),
            approver: crew.approver.clone(),
            max_steps: crew.max_steps,
            system,
            history: vec![],
            recorder: None,
            inbox: None,
            window: crew.window.clone(),
            aside: None,
            notes: vec![],
            tags: vec![],
            skip: Arc::default(),
            second_reading: false,
            repo_key: None,
            hooks: crew.hooks.clone(),
        };
        (crew.say)(format!("  ↳ {name} starts: {description}"));
        let watch = crew.watch.clone().zip(answers);
        let stop = Arc::new(Stop::default());
        if let Some((w, call)) = &watch {
            w(SubEvent::Started(Begun {
                call: call.clone(),
                name: name.clone(),
                description: description.clone(),
                prompt: prompt.clone(),
                tools: sub.tools.iter().map(|t| t.def().name).collect(),
                model: sub.model.clone(),
                edits: sub.tools.iter().any(|t| t.kind() != Kind::Read),
                stop: stop.clone(),
            }));
        }
        let mut ending = Ending { watch: watch.clone() };
        let mut events = Steps {
            name: name.clone(),
            say: crew.say.clone(),
            watch,
            thinking: String::new(),
            text: String::new(),
            told: Instant::now(),
            tokens: 0,
        };
        let mut result = tokio::select! {
            r = sub.turn(&prompt, &mut events) => r,
            _ = stop.wait() => return stopped(&name, &mut events, &mut ending),
        };
        // A SubagentStop hook may send it on, once.
        if let Ok(out) = &result
            && crew.hooks.has("SubagentStop")
        {
            let said = crew
                .hooks
                .run("SubagentStop", &name, json!({"agent": name, "description": description, "report": out.answer}))
                .await;
            if let Some(why) = said.block {
                (crew.say)(format!("  ↳ {name}: a SubagentStop hook sends it on: {why}"));
                let before = out.clone();
                let again = format!("(Code Arena: a hook of the person's says the task is not done: {why})");
                result = tokio::select! {
                    r = sub.turn(&again, &mut events) => r,
                    _ = stop.wait() => return stopped(&name, &mut events, &mut ending),
                };
                if let Ok(o) = &mut result {
                    o.steps += before.steps;
                    o.usage.add(before.usage);
                }
            }
        }
        events.flush();
        match &result {
            Ok(out) => ending.end(End { status: "done", report: out.answer.trim().to_string(), error: None }),
            Err(e) => ending.end(End { status: "failed", report: String::new(), error: Some(e.to_string()) }),
        }
        match result {
            Ok(out) => {
                (crew.say)(format!("  ↳ {name} is done ({} steps)", out.steps));
                let mut o = Output::ok(format!(
                    "{name} reports ({description}, {} steps):\n{}",
                    out.steps,
                    if out.answer.trim().is_empty() { "(it said nothing at the end)".to_string() } else { out.answer }
                ));
                o.wrote = out.wrote;
                o
            }
            Err(TurnError::Steps(n)) => Output::error(format!("{name} stopped after {n} steps without a report.")),
            Err(e) => Output::error(format!("{name} could not finish: {e}")),
        }
    }
}

/// The call a run answers, as the turn said it.
fn ctx_call(ctx: &Context) -> Option<String> {
    ctx.call.clone().filter(|c| !c.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    type Heard = Arc<Mutex<Vec<SubEvent>>>;

    fn heard() -> (Watch, Heard) {
        let all: Heard = Arc::default();
        let kept = all.clone();
        (Arc::new(move |e| kept.lock().expect("held").push(e)), all)
    }

    fn steps(watch: Watch) -> Steps {
        Steps {
            name: "sub-agent".into(),
            say: Arc::new(|_| {}),
            watch: Some((watch, "c1".into())),
            thinking: String::new(),
            text: String::new(),
            told: Instant::now(),
            tokens: 0,
        }
    }

    fn work(all: &Heard) -> Vec<Work> {
        let all = all.lock().expect("held");
        all.iter()
            .filter_map(|e| match e {
                SubEvent::Step { work, .. } => Some(work.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn words_are_gathered_and_told_when_a_call_or_a_step_begins() {
        let (w, all) = heard();
        let mut s = steps(w);
        s.reasoning("Let ");
        s.reasoning("me see.");
        s.text("Reading");
        let args = Value::Null;
        s.tool_start(&Call { id: "r1", name: "read_file", subject: "a.rs", arguments: &args });
        s.step_window(900, 32_000, None);
        s.step_end();
        assert_eq!(
            work(&all),
            [
                Work::Thinking("Let me see.".into()),
                Work::Text("Reading".into()),
                Work::ToolStart { id: "r1".into(), name: "read_file".into(), subject: "a.rs".into() },
                Work::StepEnd { tokens: 900 },
            ]
        );
    }

    #[test]
    fn an_ending_is_told_once_and_a_dropped_one_as_stopped() {
        let (w, all) = heard();
        let mut e = Ending { watch: Some((w.clone(), "c1".into())) };
        e.end(End { status: "done", report: "ok".into(), error: None });
        drop(e);
        drop(Ending { watch: Some((w, "c2".into())) });
        let all = all.lock().expect("held");
        let ends: Vec<(&str, &str)> = all
            .iter()
            .filter_map(|e| match e {
                SubEvent::Ended { call, end } => Some((call.as_str(), end.status)),
                _ => None,
            })
            .collect();
        assert_eq!(ends, [("c1", "done"), ("c2", "stopped")]);
    }

    #[tokio::test]
    async fn a_stop_asked_before_it_is_waited_for_is_kept() {
        let stop = Stop::default();
        stop.ask();
        stop.wait().await;
        assert!(stop.asked());
    }
}

/// A sub-agent the person stopped: what it did so far is told, and the model hears it was stopped.
fn stopped(name: &str, events: &mut Steps, ending: &mut Ending) -> Output {
    events.flush();
    ending.end(End { status: "stopped", report: String::new(), error: Some("Stopped by you.".into()) });
    Output::error(format!("The person stopped {name} before it reported."))
}
