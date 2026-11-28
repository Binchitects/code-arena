//! Goal mode in the terminal: `/goal TEXT [--check CMD | --when CONDITION] [--steps N] [--minutes N] [--tokens N]
//! [--until-met]`, `/goal` (where it stands), `pause`, `resume`, `cancel`, `next`, `drop N`; and
//! `code-arena -p --goal TEXT`. After each goal turn the check runs (a command in the sandbox as run_shell runs one; a
//! condition read by a fresh sub-agent), and the next turn starts by itself while the goal is worked on. The IDE's
//! goal messages (`ide_goal.rs`) change the goals through the same functions.

use std::sync::Arc;

use ca_agent::goal::{self, Budget, Check, Checker, Goal, GoalBook, Goals, TurnSpent};
use ca_tools::Tool;
use serde_json::json;

use crate::session::Prepared;

/// The check, as the session runs it.
pub struct SessionChecker {
    pub ctx: ca_tools::Context,
    pub crew: Arc<std::sync::Mutex<Option<ca_agent::task::Crew>>>,
}

#[async_trait::async_trait]
impl Checker for SessionChecker {
    async fn command(&self, text: &str) -> (bool, String) {
        let o =
            ca_tools::RunShell.run(json!({"command": text, "timeout": goal::CHECK_TIME.as_secs()}), &self.ctx).await;
        let tail: Vec<&str> = o.text.lines().rev().take(30).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        (!o.is_error, tail.join("\n"))
    }

    async fn condition(&self, text: &str) -> (bool, String) {
        let t = ca_agent::task::TaskTool::new(self.crew.clone());
        let o = t
            .run(
                json!({"description": "checking the goal", "prompt": format!(
                    "Check whether this condition holds in the working directory now, by reading the code and its files (change nothing):\n\n{text}\n\nAnswer with MET or NOT MET on the first line, then in a few lines what you looked at and why."
                )}),
                &self.ctx,
            )
            .await;
        if o.is_error {
            return (false, o.text);
        }
        // "NAME reports (…):\nMET\n…"
        let report = o.text.split_once('\n').map(|(_, r)| r).unwrap_or(&o.text).trim().to_string();
        let first = report.lines().next().unwrap_or("").trim().trim_end_matches(['.', ':']).to_ascii_uppercase();
        let why = report.split_once('\n').map(|(_, w)| w.trim().to_string()).unwrap_or_default();
        (first.starts_with("MET"), if why.is_empty() { report } else { why })
    }
}

/// `/goal` and its words. What to say.
pub fn command(rest: &str, p: &mut Prepared) -> String {
    let book = p.goals.clone();
    let words: Vec<&str> = rest.split_whitespace().collect();
    let said = |r: Result<String, String>| r.unwrap_or_else(|e| e);
    match words.first().copied() {
        None => book.get().describe(),
        Some("pause") => said(book.change(pause)),
        Some("resume") => said(book.change(resume)),
        Some("cancel") => said(book.change(cancel)),
        Some("next") if words.len() == 1 => said(book.change(start_next)),
        Some("drop") => match words.get(1).and_then(|n| n.parse::<usize>().ok()) {
            Some(n) => said(book.change(|g| drop_waiting(g, n))),
            None => "/goal drop N: the waiting goal's number.".into(),
        },
        Some(_) => match parse(rest) {
            Ok(goal) => said(book.change(|g| g.set(goal))),
            Err(e) => e,
        },
    }
}

/// The goal worked on paused: no turn carries it on until it is resumed.
pub fn pause(g: &mut Goals) -> Result<String, String> {
    match &mut g.current {
        Some(c) if c.status == goal::WORKING => {
            c.status = goal::PAUSED.into();
            Ok("The goal is paused: /goal resume carries it on.".into())
        }
        _ => Err("No goal is being worked on.".into()),
    }
}

/// The paused goal worked on again.
pub fn resume(g: &mut Goals) -> Result<String, String> {
    match &mut g.current {
        Some(c) if c.status == goal::PAUSED => {
            c.status = goal::WORKING.into();
            Ok("The goal is worked on again from the next turn (send a message, or press Enter on an empty line)."
                .into())
        }
        _ => Err("No goal is paused.".into()),
    }
}

/// The goal worked on or paused given up.
pub fn cancel(g: &mut Goals) -> Result<String, String> {
    match &mut g.current {
        Some(c) if c.status == goal::WORKING || c.status == goal::PAUSED => {
            c.status = goal::CANCELLED.into();
            c.outcome = Some(format!("Cancelled by the person after {}.", c.usage()));
            Ok("The goal is cancelled.".into())
        }
        _ => Err("No goal to cancel.".into()),
    }
}

/// The first goal waiting started, once the one before it has ended (met, spent, stuck or cancelled).
pub fn start_next(g: &mut Goals) -> Result<String, String> {
    if g.current.as_ref().is_some_and(|c| c.status == goal::WORKING || c.status == goal::PAUSED) {
        return Err("A goal is being worked on: the next starts when it is met.".into());
    }
    if !g.start_next() {
        return Err("No goal waits.".into());
    }
    Ok(format!("Next goal: {}", g.current.as_ref().map(|c| c.text.as_str()).unwrap_or("")))
}

/// The goal waiting at `n` (from 1) dropped.
pub fn drop_waiting(g: &mut Goals, n: usize) -> Result<String, String> {
    if (1..=g.next.len()).contains(&n) {
        let d = g.next.remove(n - 1);
        Ok(format!("Dropped: {}", d.text))
    } else {
        Err(format!("No goal waits at {n}: /goal lists them."))
    }
}

/// `TEXT [--check CMD | --when CONDITION] [--steps N] [--minutes N] [--tokens N] [--until-met]`.
pub fn parse(rest: &str) -> Result<Goal, String> {
    let mut text = Vec::new();
    let mut check: Option<Check> = None;
    let mut budget = Budget::default();
    let mut words = rest.split_whitespace().peekable();
    while let Some(w) = words.next() {
        let rest_of = |words: &mut std::iter::Peekable<std::str::SplitWhitespace<'_>>| {
            let mut v = Vec::new();
            while let Some(x) = words.peek() {
                if x.starts_with("--") {
                    break;
                }
                v.push(*x);
                words.next();
            }
            v.join(" ")
        };
        let number = |s: String, what: &str| s.parse::<u64>().map_err(|_| format!("--{what} takes a number"));
        match w {
            "--check" => check = Some(Check { kind: "command".into(), text: rest_of(&mut words), by: "person".into() }),
            "--when" => {
                check = Some(Check { kind: "condition".into(), text: rest_of(&mut words), by: "person".into() })
            }
            "--steps" => budget.steps = number(rest_of(&mut words), "steps")?,
            "--minutes" => budget.minutes = number(rest_of(&mut words), "minutes")?,
            "--tokens" => budget.tokens = number(rest_of(&mut words), "tokens")?,
            "--until-met" => budget.until_met = true,
            other => text.push(other),
        }
    }
    if check.as_ref().is_some_and(|c| c.text.is_empty()) {
        return Err("--check takes the command, --when the condition.".into());
    }
    Ok(Goal::new(&text.join(" "), check, budget))
}

/// After a turn: when a goal was worked on, its check runs and the next goal turn's message is given back (`None`: the
/// goal is not worked on any more). What to say goes through `say`.
pub async fn after_turn(
    p: &mut Prepared,
    out: Option<&ca_agent::TurnOutcome>,
    took: std::time::Duration,
    say: &dyn Fn(String),
) -> Option<String> {
    if !p.goals.get().working() {
        return None;
    }
    let spent = TurnSpent {
        steps: out.map(|o| o.steps as u64).unwrap_or(0),
        tokens: out.map(|o| o.usage.prompt + o.usage.completion).unwrap_or(0),
        ms: took.as_millis() as u64,
        tool_calls: out.map(|o| o.tool_calls as u64).unwrap_or(0),
        stopped: out.is_none(),
    };
    crate::session::refresh_crew(p, p_ui(p));
    let checker = SessionChecker { ctx: p.agent.ctx.clone(), crew: p.crew.clone() };
    let said = goal::after_turn(&p.goals, spent, &checker).await;
    if !said.is_empty() {
        say(said);
    }
    let next = p.goals.get();
    let prompt = next.prompt()?;
    // A goal turn takes no more steps than the budget has left.
    if let Some(left) = next.current.as_ref().and_then(|g| g.steps_left()) {
        p.agent.max_steps = p.agent.max_steps.min(left.max(1) as usize);
    }
    Some(prompt)
}

fn p_ui(p: &Prepared) -> crate::ui::Ui {
    p.ui
}

/// The goal's book, as the store keeps it; every change kept and sent to Arena.
pub fn book(
    store: &Arc<std::sync::Mutex<ca_session::Store>>,
    session: &str,
    sync: Option<Arc<ca_sync::ChatSync>>,
) -> Arc<GoalBook> {
    let book = Arc::new(GoalBook::default());
    if let Some(json) = store.lock().ok().and_then(|s| s.goals(session).ok().flatten())
        && let Ok(g) = serde_json::from_str(&json)
    {
        book.load(g);
    }
    let (st, id) = (store.clone(), session.to_string());
    book.on_change(Arc::new(move |g| {
        let json = (g.current.is_some() || !g.next.is_empty()).then(|| serde_json::to_string(g).unwrap_or_default());
        if let Ok(s) = st.lock() {
            let _ = s.set_goals(&id, json.as_deref());
        }
        if let Some(sy) = &sync {
            sy.poke();
        }
    }));
    book
}
