//! The `task` tool: the model hands a piece of work to a sub-agent (one of the `agents/NAME.md` the project or the
//! person wrote, or a general one), which works in a context of its own (its instructions, model and tools) and
//! reports back; its report is the call's result. Several calls in one answer run at once (four at most). Its calls ask
//! as the session's do; it has no `task` of its own (no sub-agent of a sub-agent) and not the session's plan.

use std::sync::{Arc, Mutex};

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

/// A sub-agent's steps, shown to the person as lines of their own.
struct Steps {
    name: String,
    say: Arc<dyn Fn(String) + Send + Sync>,
}

impl Events for Steps {
    fn notice(&mut self, t: &str) {
        (self.say)(format!("  ↳ {}: {t}", self.name));
    }
    fn tool_end(&mut self, call: &Call<'_>, o: &Output, _took: std::time::Duration) {
        let s: String = call.subject.lines().next().unwrap_or("").chars().take(80).collect();
        (self.say)(format!("  ↳ {}: ● {} {s}{}", self.name, call.name, if o.is_error { " (failed)" } else { "" }));
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
    async fn run(&self, args: Value, _ctx: &Context) -> Output {
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
            attached: Default::default(),
        };
        (crew.say)(format!("  ↳ {name} starts: {description}"));
        let mut events = Steps { name: name.clone(), say: crew.say.clone() };
        let mut result = sub.turn(&prompt, &mut events).await;
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
                result = sub
                    .turn(
                        &format!("(Code Arena: a hook of the person's says the task is not done: {why})"),
                        &mut events,
                    )
                    .await;
                if let Ok(o) = &mut result {
                    o.steps += before.steps;
                    o.usage.add(before.usage);
                }
            }
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
