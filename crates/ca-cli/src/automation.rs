//! The person's agents (Agent Studio) and scheduled tasks in Arena, 8.1 on: `/arena-agents` and `/arena-tasks` in a
//! session, `code-arena agents` and `code-arena tasks` from the shell, and for the model `arena_agents` and
//! `run_arena_agent`. An Arena that lacks them (8.0) says so, as its answers to the probes told.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ca_arena::automation::{Agent, AgentRun, Started, Task, pick};
use ca_arena::{ArenaClient, Feature, State};
use ca_llm::ToolDef;
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::{Value, json};

use crate::app::App;

/// How long a run is followed before it is left to Arena (it goes on there).
const FOLLOW: Duration = Duration::from_secs(600);

/// Whether this Arena has `f` for the person, and if not, why in a line.
pub async fn usable(paths: &ca_core::Paths, arena: &ArenaClient, f: Feature) -> Result<(), String> {
    let caps = arena
        .capabilities_kept(&ca_arena::Kept::file(&paths.servers_dir(), arena.base()))
        .await
        .map_err(|e| e.to_string())?;
    let what = if f == Feature::AutomationAgents { "Agent Studio's agents" } else { "scheduled tasks" };
    match caps.state(f) {
        State::On => Ok(()),
        State::Absent => Err(format!("This Arena does not give Code Arena its {what} (Arena 8.1 on does).")),
        State::Off(w) | State::Denied(w) if !w.is_empty() => Err(w),
        s => Err(format!("This Arena's {what} cannot be used now: {s:?}.")),
    }
}

/// Whether the kept answers say `f` is there (no call made: for the tools a session starts with).
pub fn kept_has(app: &App, arena: &ArenaClient, f: Feature) -> bool {
    ca_arena::Kept::read(&ca_arena::Kept::file(&app.paths.servers_dir(), arena.base()))
        .is_some_and(|k| k.capabilities.has(f))
}

/// Follows a run until it ends, or `FOLLOW` passes.
pub async fn follow(arena: &ArenaClient, run: &str, every: Duration) -> Result<AgentRun, String> {
    let end = Instant::now() + FOLLOW;
    loop {
        let r = arena.agent_run(run).await.map_err(|e| e.to_string())?;
        if r.ended() || r.status == "waiting" || Instant::now() >= end {
            return Ok(r);
        }
        tokio::time::sleep(every).await;
    }
}

/// A run as it ended, in a few lines.
pub fn told(r: &AgentRun) -> String {
    let name = r.workflow.clone().unwrap_or_else(|| "The agent".into());
    match r.status.as_str() {
        "done" => format!("{name} is done: {}", r.answer().unwrap_or("(it said nothing)")),
        "failed" => format!(
            "{name} failed: {}",
            r.error.clone().or_else(|| r.steps.iter().rev().find_map(|s| s.error.clone())).unwrap_or_default()
        ),
        "stopped" => format!("{name} was stopped."),
        "waiting" => format!("{name} waits for its owner's approval in Arena (Agent Studio → its runs)."),
        s => format!("{name} is still {s} in Arena (run {}).", r.id),
    }
}

fn agent_line(i: usize, a: &Agent) -> String {
    format!(
        "{:>3}. {}{}{}{}",
        i + 1,
        a.name,
        a.description.as_ref().filter(|d| !d.is_empty()).map(|d| format!(": {d}")).unwrap_or_default(),
        if a.mine {
            String::new()
        } else {
            format!(" (shared by {})", a.owner.clone().unwrap_or_else(|| "someone".into()))
        },
        if a.enabled { "" } else { " [off]" }
    )
}

fn task_line(i: usize, t: &Task) -> String {
    let when = if t.trigger.as_deref().is_some_and(|x| x != "schedule") {
        format!("on {} events", t.trigger.clone().unwrap_or_default())
    } else {
        format!("{} {}", t.cron, t.time_zone.clone().unwrap_or_default()).trim().to_string()
    };
    let last = t.last_run.as_ref().map(|r| format!(", last {}", r.status)).unwrap_or_default();
    let next = t.next_runs.first().map(|n| format!(", next {n}")).unwrap_or_default();
    format!(
        "{:>3}. {} ({when}{last}{next}){}{}",
        i + 1,
        t.name,
        if t.enabled { "" } else { " [off]" },
        if t.running { " running now" } else { "" }
    )
}

/// `/arena-agents`, `/arena-agents run N|NAME [TEXT]`: the run followed in the background, told when it ends.
pub async fn agents_command(app: &App, rest: &str, notices: &crate::session::Notices) -> String {
    let arena = match app.arena() {
        Ok(a) => a,
        Err(e) => return e.to_string(),
    };
    if let Err(w) = usable(&app.paths, &arena, Feature::AutomationAgents).await {
        return w;
    }
    let list = match arena.agents().await {
        Ok(l) => l,
        Err(e) => return e.to_string(),
    };
    let Some(after) = rest.strip_prefix("run") else {
        if list.is_empty() {
            return "You have no agents in Arena yet (Agent Studio makes them).".into();
        }
        let mut out: Vec<String> = list.iter().enumerate().map(|(i, a)| agent_line(i, a)).collect();
        out.push("/arena-agents run N [TEXT] runs one; the model may run them too (run_arena_agent).".into());
        return out.join("\n");
    };
    let after = after.trim();
    let (which, text) = after.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((after, ""));
    let a = match pick(&list, |a| a.name.as_str(), which) {
        Ok(a) => a.clone(),
        Err(w) => return format!("No such agent: {w}. /arena-agents lists them."),
    };
    let run = match arena.run_agent(&a.id, (!text.is_empty()).then_some(text), &[]).await {
        Ok(r) => r,
        Err(e) => return e.to_string(),
    };
    let n = notices.clone();
    let name = a.name.clone();
    tokio::spawn(async move {
        let line = match follow(&arena, &run, Duration::from_secs(2)).await {
            Ok(r) => (r.status == "failed", told(&r)),
            Err(e) => (true, format!("{name}: {e}")),
        };
        if let Ok(mut q) = n.lock() {
            q.push(line);
        }
    });
    format!("{} is running in Arena; you are told here when it ends.", a.name)
}

/// `/arena-tasks`, `/arena-tasks run N|NAME`, `/arena-tasks runs N|NAME`.
pub async fn tasks_command(app: &App, rest: &str) -> String {
    let arena = match app.arena() {
        Ok(a) => a,
        Err(e) => return e.to_string(),
    };
    tasks_said(&arena, app, rest).await.unwrap_or_else(|e| e)
}

async fn tasks_said(arena: &ArenaClient, app: &App, rest: &str) -> Result<String, String> {
    usable(&app.paths, arena, Feature::AutomationScheduled).await?;
    let list = arena.scheduled().await.map_err(|e| e.to_string())?;
    let (word, which) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
    let chosen =
        || pick(&list, |t| t.name.as_str(), which).map_err(|w| format!("No such task: {w}. /arena-tasks lists them."));
    match word {
        "" => {
            if list.is_empty() {
                return Ok("You have no scheduled tasks in Arena yet.".into());
            }
            let mut out: Vec<String> = list.iter().enumerate().map(|(i, t)| task_line(i, t)).collect();
            out.push("/arena-tasks run N runs one now; /arena-tasks runs N says how its last runs went.".into());
            Ok(out.join("\n"))
        }
        "run" => {
            let t = chosen()?;
            Ok(match arena.run_scheduled(&t.id).await.map_err(|e| e.to_string())? {
                Started::Started => {
                    format!("{} is running in Arena now (/arena-tasks runs {} says how it went).", t.name, which)
                }
                Started::Running(w) => w,
            })
        }
        "runs" => {
            let t = chosen()?;
            let runs = arena.scheduled_runs(&t.id).await.map_err(|e| e.to_string())?;
            if runs.is_empty() {
                return Ok(format!("{} has not run yet.", t.name));
            }
            Ok(runs
                .iter()
                .take(10)
                .map(|r| {
                    format!(
                        "{} {}{}{}{}",
                        r.started_at.clone().unwrap_or_default(),
                        r.status,
                        if r.manual { " (by hand)" } else { "" },
                        r.error.as_ref().map(|e| format!(": {e}")).unwrap_or_default(),
                        r.link.as_ref().map(|l| format!("  {l}")).unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        _ => Err("/arena-tasks [run N | runs N]".into()),
    }
}

/// `code-arena agents [run NAME [TEXT…] [--field K=V]…]`: the run waited for, its answer printed.
pub async fn agents_cli(app: &App, args: &[String]) -> anyhow::Result<i32> {
    let arena = app.arena()?;
    if let Err(w) = usable(&app.paths, &arena, Feature::AutomationAgents).await {
        eprintln!("{w}");
        return Ok(1);
    }
    let list = arena.agents().await?;
    match args.first().map(String::as_str) {
        None | Some("list") => {
            for (i, a) in list.iter().enumerate() {
                println!("{}", agent_line(i, a));
            }
            Ok(0)
        }
        Some("run") if args.len() >= 2 => {
            let a = pick(&list, |a| a.name.as_str(), &args[1]).map_err(|w| anyhow::anyhow!("no such agent: {w}"))?;
            let mut fields = Vec::new();
            let mut words = Vec::new();
            let mut it = args[2..].iter();
            while let Some(w) = it.next() {
                if w == "--field" {
                    let kv = it.next().ok_or_else(|| anyhow::anyhow!("--field K=V"))?;
                    let (k, v) = kv.split_once('=').ok_or_else(|| anyhow::anyhow!("--field K=V, not {kv}"))?;
                    fields.push((k.to_string(), v.to_string()));
                } else {
                    words.push(w.as_str());
                }
            }
            let text = words.join(" ");
            let run = arena.run_agent(&a.id, (!text.is_empty()).then_some(text.as_str()), &fields).await?;
            let r = follow(&arena, &run, Duration::from_secs(2)).await.map_err(anyhow::Error::msg)?;
            println!("{}", told(&r));
            Ok(if r.status == "done" { 0 } else { 1 })
        }
        _ => {
            eprintln!("code-arena agents [list | run NAME [TEXT…] [--field K=V]…]");
            Ok(2)
        }
    }
}

/// `code-arena tasks [run NAME | runs NAME]`.
pub async fn tasks_cli(app: &App, args: &[String]) -> anyhow::Result<i32> {
    let arena = app.arena()?;
    let rest = args.join(" ");
    let rest = if rest == "list" { String::new() } else { rest };
    match tasks_said(&arena, app, &rest).await {
        Ok(t) => {
            println!("{t}");
            Ok(0)
        }
        Err(e) => {
            eprintln!("{e}");
            Ok(1)
        }
    }
}

/// `arena_agents`: the agents the person may run.
pub struct AgentsTool {
    pub arena: ArenaClient,
}

#[async_trait::async_trait]
impl Tool for AgentsTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "arena_agents".into(),
            description: "Lists the person's agents in Arena (Agent Studio's workflows, theirs or shared with them): name, what each does, how it starts. run_arena_agent runs one.".into(),
            parameters: json!({"type": "object", "properties": {}}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, _args: &Value) -> String {
        String::new()
    }
    async fn run(&self, _args: Value, _ctx: &Context) -> Output {
        match self.arena.agents().await {
            Ok(l) if l.is_empty() => Output::ok("The person has no agents in Arena."),
            Ok(l) => Output::ok(
                l.iter()
                    .enumerate()
                    .map(|(i, a)| format!("{} [starts by {}]", agent_line(i, a), a.trigger.clone().unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Err(e) => Output::error(e.to_string()),
        }
    }
}

/// `run_arena_agent`: runs one of the person's agents in Arena and waits for what it gives.
pub struct RunAgentTool {
    pub arena: ArenaClient,
}

#[async_trait::async_trait]
impl Tool for RunAgentTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "run_arena_agent".into(),
            description: "Runs one of the person's agents in Arena (by name, as arena_agents lists them) with a text, or the fields of one started by a form, and waits for what it gives (ten minutes at most; it goes on in Arena after that). It runs as the person: its steps may do what the agent does (post, send, change).".into(),
            parameters: json!({"type": "object", "properties": {
                "agent": {"type": "string", "description": "The agent's name (or its number in arena_agents)."},
                "text": {"type": "string", "description": "What it is given."},
                "fields": {"type": "object", "additionalProperties": {"type": "string"}, "description": "For an agent started by a form: its fields by name."}
            }, "required": ["agent"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Shell
    }
    fn subject(&self, args: &Value) -> String {
        args.get("agent").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    async fn run(&self, args: Value, _ctx: &Context) -> Output {
        let which = args.get("agent").and_then(Value::as_str).unwrap_or_default();
        let list = match self.arena.agents().await {
            Ok(l) => l,
            Err(e) => return Output::error(e.to_string()),
        };
        let a = match pick(&list, |a| a.name.as_str(), which) {
            Ok(a) => a,
            Err(w) => return Output::error(format!("No such agent: {w}.")),
        };
        let fields: Vec<(String, String)> = args
            .get("fields")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
            .collect();
        let text = args.get("text").and_then(Value::as_str).filter(|t| !t.is_empty());
        let run = match self.arena.run_agent(&a.id, text, &fields).await {
            Ok(r) => r,
            Err(e) => return Output::error(e.to_string()),
        };
        match follow(&self.arena, &run, Duration::from_secs(2)).await {
            Ok(r) if r.status == "done" => Output::ok(told(&r)),
            Ok(r) => Output::error(told(&r)),
            Err(e) => Output::error(e),
        }
    }
}

/// The tools a signed-in session gets when its Arena has agents.
pub fn tools(app: &App) -> Vec<Arc<dyn Tool>> {
    let Ok(arena) = app.arena() else { return vec![] };
    if !app.connection.signed_in() || !kept_has(app, &arena, Feature::AutomationAgents) {
        return vec![];
    }
    vec![Arc::new(AgentsTool { arena: arena.clone() }), Arc::new(RunAgentTool { arena })]
}
