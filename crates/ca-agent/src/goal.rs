//! Goal mode: the agent works turn after turn until a goal is met as its check says (a command's exit code, or a
//! condition a fresh sub-agent reads), within the budget the person set (steps, minutes, tokens; or until met); then it
//! says how it checked. The person pauses, resumes or cancels it; more goals wait their turn. Kept with the session and
//! sent to Arena with the chat.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ca_llm::ToolDef;
use ca_tools::{Context, Kind, Output, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const WORKING: &str = "working";
pub const PAUSED: &str = "paused";
pub const MET: &str = "met";
pub const BUDGET: &str = "budget";
pub const STUCK: &str = "stuck";
pub const CANCELLED: &str = "cancelled";

/// Turns in a row with no tool call while the check fails: the agent is not getting on.
const MOST_IDLE_TURNS: u32 = 3;
/// Turns that may go by before the agent wrote its check down.
const MOST_TURNS_UNCHECKED: u32 = 3;
const MOST_GOALS: usize = 20;
/// How long a command check may run.
pub const CHECK_TIME: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// "command" or "condition".
    pub kind: String,
    pub text: String,
    /// "person" (set with the goal) or "agent" (written down with goal_check).
    pub by: String,
}

impl Check {
    pub fn describe(&self) -> String {
        if self.kind == "command" {
            format!("`{}` succeeds", self.text)
        } else {
            format!("a fresh reader finds that {}", self.text)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Budget {
    pub steps: u64,
    pub minutes: u64,
    pub tokens: u64,
    #[serde(default)]
    pub until_met: bool,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { steps: 200, minutes: 60, tokens: 5_000_000, until_met: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub text: String,
    #[serde(default)]
    pub check: Option<Check>,
    #[serde(default)]
    pub budget: Budget,
    pub status: String,
    #[serde(default)]
    pub turns: u32,
    #[serde(default)]
    pub steps: u64,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub ms: u64,
    #[serde(default)]
    pub idle_turns: u32,
    #[serde(default)]
    pub passed: Option<bool>,
    #[serde(default)]
    pub said: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
}

impl Goal {
    pub fn new(text: &str, check: Option<Check>, budget: Budget) -> Self {
        Goal {
            text: text.trim().to_string(),
            check,
            budget,
            status: WORKING.into(),
            turns: 0,
            steps: 0,
            tokens: 0,
            ms: 0,
            idle_turns: 0,
            passed: None,
            said: None,
            outcome: None,
        }
    }

    pub fn usage(&self) -> String {
        format!(
            "{} turn{}, {} steps, {} min, {}k tokens",
            self.turns,
            if self.turns == 1 { "" } else { "s" },
            self.steps,
            self.ms / 60_000,
            self.tokens / 1000
        )
    }

    /// What of the budget is spent, when some of it is.
    pub fn spent(&self) -> Option<String> {
        if self.budget.until_met {
            return None;
        }
        let b = &self.budget;
        if self.steps >= b.steps {
            Some(format!("its {} steps are used", b.steps))
        } else if self.ms / 60_000 >= b.minutes {
            Some(format!("its {} minutes are used", b.minutes))
        } else if self.tokens >= b.tokens {
            Some(format!("its {}k tokens are used", b.tokens / 1000))
        } else {
            None
        }
    }

    /// Steps the next turn may take, within the budget.
    pub fn steps_left(&self) -> Option<u64> {
        (!self.budget.until_met).then(|| self.budget.steps.saturating_sub(self.steps))
    }

    /// Arena's shape for it (`PUT /api/code-arena/chats/{id}/goal`).
    pub fn to_arena(&self) -> Value {
        json!({
            "text": self.text,
            "check": self.check.as_ref().map(|c| json!({"kind": c.kind, "text": c.text, "by": c.by})),
            "budget": {"steps": self.budget.steps, "minutes": self.budget.minutes, "tokens": self.budget.tokens},
            "status": self.status, "turns": self.turns, "steps": self.steps, "tokens": self.tokens, "ms": self.ms,
            "passed": self.passed, "said": self.said, "outcome": self.outcome,
        })
    }
}

/// The session's goals: the one worked on (or the last, ended) and those waiting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goals {
    #[serde(default)]
    pub current: Option<Goal>,
    #[serde(default)]
    pub next: Vec<Goal>,
}

impl Goals {
    pub fn working(&self) -> bool {
        self.current.as_ref().is_some_and(|g| g.status == WORKING)
    }

    /// A goal set: worked on now when none is, else waiting its turn. What to say.
    pub fn set(&mut self, goal: Goal) -> Result<String, String> {
        if goal.text.is_empty() {
            return Err("Say what the goal is: /goal TEXT.".into());
        }
        let busy = self.current.as_ref().is_some_and(|g| g.status == WORKING || g.status == PAUSED);
        if busy {
            if self.next.len() >= MOST_GOALS {
                return Err(format!("{MOST_GOALS} goals wait already: /goal drop N drops one."));
            }
            let mut g = goal;
            g.status = "queued".into();
            self.next.push(g);
            return Ok(format!(
                "It waits its turn ({} waiting): it starts when the goal before it is met.",
                self.next.len()
            ));
        }
        self.current = Some(goal);
        Ok("The agent works on it from now on, turn after turn, until it is met.".into())
    }

    /// The next goal waiting, started.
    fn start_next(&mut self) -> bool {
        if self.next.is_empty() {
            return false;
        }
        let mut g = self.next.remove(0);
        g.status = WORKING.into();
        self.current = Some(g);
        true
    }

    /// The message the next goal turn starts with (`None`: no goal is worked on).
    pub fn prompt(&self) -> Option<String> {
        let g = self.current.as_ref().filter(|g| g.status == WORKING)?;
        let budget = if g.budget.until_met {
            "No budget: work until it is met, as long as it takes (a long session: keep notes in files and commit as you go).".to_string()
        } else {
            format!(
                "Budget left: {} steps, {} minutes, {}k tokens.",
                g.budget.steps.saturating_sub(g.steps),
                g.budget.minutes.saturating_sub(g.ms / 60_000),
                g.budget.tokens.saturating_sub(g.tokens) / 1000
            )
        };
        let after = match self.next.len() {
            0 => String::new(),
            1 => " 1 more goal waits after this one; Code Arena starts it when this one is met.".into(),
            n => format!(" {n} more goals wait after this one; Code Arena starts each when this one is met."),
        };
        let check = match &g.check {
            Some(c) => format!("It is checked once you end your turn: {}{}.", c.describe(), if c.by == "person" { " (the person's check)" } else { " (your check)" }),
            None => "First, before any change, write down how it will be checked with goal_check: a command that succeeds only when it is met (the tests, a build), or a condition to read. Then work.".into(),
        };
        if g.turns == 0 {
            return Some(format!(
                "(Code Arena) Goal: {}\n\nWork toward this goal until it is met, across as many turns as it takes: Code Arena carries on after each turn until the check passes. {check} {budget}{after}\nEnd your turn when you believe it is met, or when you need the check's word on where it stands.",
                g.text
            ));
        }
        let last = match (g.passed, &g.said) {
            (Some(false), Some(s)) if !s.is_empty() => format!("The check did not pass yet:\n{s}\n\n"),
            _ => String::new(),
        };
        Some(format!(
            "(Code Arena) Goal (turn {}): {}\n\n{last}The goal is not met yet: carry on toward it. {check} {budget}",
            g.turns + 1,
            g.text
        ))
    }

    fn end(&mut self, status: &str, outcome: String) -> String {
        if let Some(g) = &mut self.current {
            g.status = status.into();
            g.outcome = Some(outcome.clone());
        }
        outcome
    }

    pub fn describe(&self) -> String {
        let Some(g) = &self.current else {
            return "No goal is set. /goal TEXT sets one: the agent works until it is met (more /goal TEXT wait their turn after it).".into();
        };
        let mut lines = vec![format!("Goal ({}): {}", g.status, g.text)];
        lines.push(match &g.check {
            Some(c) => {
                format!("  checked: {} ({})", c.describe(), if c.by == "person" { "yours" } else { "the agent's" })
            }
            None => "  checked: not written down yet (the agent does it first)".into(),
        });
        lines.push(format!(
            "  spent: {}{}",
            g.usage(),
            if g.budget.until_met {
                " (no budget: until met)".to_string()
            } else {
                format!(" of {} steps, {} min, {}k tokens", g.budget.steps, g.budget.minutes, g.budget.tokens / 1000)
            }
        ));
        if let Some(o) = &g.outcome {
            lines.push(format!("  {o}"));
        } else if let Some(s) = &g.said {
            lines.push(format!("  last check: {}", s.lines().next().unwrap_or("")));
        }
        for (i, n) in self.next.iter().enumerate() {
            lines.push(format!("  waiting {}: {}", i + 1, n.text));
        }
        lines.join("\n")
    }
}

/// The session's goals, shared by `goal_check`, the session and its turns.
#[derive(Default)]
pub struct GoalBook {
    goals: Mutex<Goals>,
    on_change: Mutex<Option<Arc<dyn Fn(&Goals) + Send + Sync>>>,
}

impl std::fmt::Debug for GoalBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalBook").field("goals", &self.get()).finish()
    }
}

impl GoalBook {
    pub fn get(&self) -> Goals {
        self.goals.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn load(&self, goals: Goals) {
        if let Ok(mut g) = self.goals.lock() {
            *g = goals;
        }
    }

    pub fn on_change(&self, f: Arc<dyn Fn(&Goals) + Send + Sync>) {
        if let Ok(mut c) = self.on_change.lock() {
            *c = Some(f);
        }
    }

    /// Changes them, and tells the session (kept, sent to Arena).
    pub fn change<T>(&self, f: impl FnOnce(&mut Goals) -> T) -> T {
        let (out, now) = {
            let mut g = self.goals.lock().unwrap_or_else(|e| e.into_inner());
            let out = f(&mut g);
            (out, g.clone())
        };
        if let Some(cb) = self.on_change.lock().ok().and_then(|c| c.clone()) {
            cb(&now);
        }
        out
    }
}

/// How the check is run: a command, or a condition read by a fresh sub-agent.
#[async_trait::async_trait]
pub trait Checker: Send + Sync {
    async fn command(&self, text: &str) -> (bool, String);
    async fn condition(&self, text: &str) -> (bool, String);
}

/// What a goal turn spent and whether it did anything.
#[derive(Debug, Clone, Copy, Default)]
pub struct TurnSpent {
    pub steps: u64,
    pub tokens: u64,
    pub ms: u64,
    pub tool_calls: u64,
    /// The person stopped it (Ctrl+C).
    pub stopped: bool,
}

/// A goal turn ended: what it spent is counted, the check is run, and the goal is met, carries on, or stops (its budget,
/// no progress, the person). What to tell the person.
pub async fn after_turn(book: &GoalBook, spent: TurnSpent, checker: &dyn Checker) -> String {
    let Some(goal) = book.get().current.filter(|g| g.status == WORKING) else { return String::new() };
    let mut goal = goal;
    goal.turns += 1;
    goal.steps += spent.steps;
    goal.tokens += spent.tokens;
    goal.ms += spent.ms;
    if spent.stopped {
        goal.status = PAUSED.into();
        let said = format!("Stopped: the goal is paused ({}). /goal resume carries it on.", goal.usage());
        book.change(|g| g.current = Some(goal));
        return said;
    }
    let Some(check) = goal.check.clone() else {
        let g2 = goal.clone();
        return book.change(|g| {
            g.current = Some(g2.clone());
            if g2.turns >= MOST_TURNS_UNCHECKED {
                g.end(STUCK, format!("The agent did not write down how to check the goal in {} turns: it stopped. Set it again with a check of your own.", g2.turns))
            } else if let Some(s) = g2.spent() {
                g.end(BUDGET, format!("The goal's budget is spent ({s}) before it had a check."))
            } else {
                "The goal has no check yet: the agent is asked to write one down.".into()
            }
        });
    };
    let (passed, said) =
        if check.kind == "command" { checker.command(&check.text).await } else { checker.condition(&check.text).await };
    goal.passed = Some(passed);
    goal.said = Some(said.clone());
    let one_line =
        |s: &str| s.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("").chars().take(300).collect::<String>();
    book.change(|g| {
        g.current = Some(goal.clone());
        if passed {
            let how = if check.kind == "command" {
                format!("`{}` succeeded (exit code 0)", check.text)
            } else {
                format!("a fresh reader found that this holds: {}", check.text)
            };
            let met = g.end(MET, format!("Goal met: {how}, after {}.", goal.usage()));
            if g.start_next() {
                let text = g.current.as_ref().map(|c| c.text.clone()).unwrap_or_default();
                return format!("{met}\nNext goal: {text}");
            }
            return met;
        }
        if let Some(s) = goal.spent() {
            return g.end(BUDGET, format!("The goal is not met and {s}: it stopped after {}. The last check said: {}", goal.usage(), one_line(&said)));
        }
        let idle = if spent.tool_calls == 0 { goal.idle_turns + 1 } else { 0 };
        if let Some(c) = &mut g.current {
            c.idle_turns = idle;
        }
        if idle >= MOST_IDLE_TURNS {
            return g.end(STUCK, format!(
                "The agent ended {idle} turns in a row without doing anything while the check still fails: the goal stopped. The last check said: {}",
                one_line(&said)
            ));
        }
        format!(
            "The goal is not met yet ({}): the agent carries on. {}.",
            if check.kind == "command" { format!("`{}` failed", check.text) } else { "the condition does not hold".into() },
            goal.usage()
        )
    })
}

/// `goal_check`: the agent writes down how its goal will be checked, before it works on it.
pub struct GoalCheckTool {
    pub book: Arc<GoalBook>,
}

#[async_trait::async_trait]
impl Tool for GoalCheckTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "goal_check".into(),
            description: "Writes down how the goal the person set will be checked, before you work on it: a command that succeeds only when the goal is met (the tests, a build: its exit code decides), or a condition a fresh reader can judge from the files. Code Arena runs the check after each of your turns, and carries on until it passes.".into(),
            parameters: json!({"type": "object", "properties": {
                "kind": {"type": "string", "enum": ["command", "condition"]},
                "text": {"type": "string", "description": "The command, or the condition in words."}
            }, "required": ["kind", "text"]}),
        }
    }
    fn kind(&self) -> Kind {
        // A command run after every turn: it asks as a command does.
        Kind::Shell
    }
    fn subject(&self, a: &Value) -> String {
        a.get("text").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    async fn run(&self, args: Value, _ctx: &Context) -> Output {
        let kind = args.get("kind").and_then(Value::as_str).unwrap_or("");
        let text = args.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if !matches!(kind, "command" | "condition") || text.is_empty() {
            return Output::error("Give kind (command or condition) and text.");
        }
        let check = Check { kind: kind.into(), text, by: "agent".into() };
        self.book.change(|g| match &mut g.current {
            Some(c) if c.status == WORKING => {
                if c.check.as_ref().is_some_and(|k| k.by == "person") {
                    return Output::error("The person gave the goal its check: it stays as they wrote it.");
                }
                let said = format!("Written down: the goal is met when {}.", check.describe());
                c.check = Some(check);
                Output::ok(said)
            }
            _ => Output::error("No goal is being worked on."),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Vec<bool>, Mutex<usize>);
    #[async_trait::async_trait]
    impl Checker for Fixed {
        async fn command(&self, _t: &str) -> (bool, String) {
            let mut i = self.1.lock().expect("i");
            let r = self.0[(*i).min(self.0.len() - 1)];
            *i += 1;
            (r, if r { "Exit code 0.".into() } else { "Exit code 1.\n2 tests failed".into() })
        }
        async fn condition(&self, _t: &str) -> (bool, String) {
            (true, "the README says so".into())
        }
    }

    fn spent(tools: u64) -> TurnSpent {
        TurnSpent { steps: 3, tokens: 1000, ms: 1000, tool_calls: tools, stopped: false }
    }

    #[tokio::test]
    async fn worked_on_until_its_check_passes_then_the_next_starts() {
        let book = GoalBook::default();
        let cmd = Check { kind: "command".into(), text: "cargo test".into(), by: "person".into() };
        book.change(|g| g.set(Goal::new("tests pass", Some(cmd), Budget::default()))).expect("set");
        assert!(
            book.change(|g| g.set(Goal::new("docs", None, Budget::default())))
                .expect("queued")
                .contains("waits its turn")
        );
        assert!(book.get().prompt().expect("prompt").contains("checked once you end your turn: `cargo test` succeeds"));
        let checker = Fixed(vec![false, true], Mutex::new(0));
        let s = after_turn(&book, spent(2), &checker).await;
        assert!(s.contains("not met yet"), "{s}");
        assert!(book.get().prompt().expect("prompt").contains("2 tests failed"));
        let s = after_turn(&book, spent(1), &checker).await;
        assert!(s.contains("Goal met") && s.contains("Next goal: docs"), "{s}");
        let g = book.get();
        assert_eq!(g.current.as_ref().map(|c| c.text.as_str()), Some("docs"));
        assert!(g.prompt().expect("prompt").contains("write down how it will be checked with goal_check"));
    }

    #[tokio::test]
    async fn no_progress_no_check_a_spent_budget_and_the_persons_stop() {
        let checker = Fixed(vec![false], Mutex::new(0));
        let book = GoalBook::default();
        let cmd = Check { kind: "command".into(), text: "make".into(), by: "agent".into() };
        book.change(|g| g.set(Goal::new("build", Some(cmd.clone()), Budget::default()))).expect("set");
        for _ in 0..2 {
            after_turn(&book, spent(0), &checker).await;
        }
        assert!(after_turn(&book, spent(0), &checker).await.contains("without doing anything"));
        assert_eq!(book.get().current.expect("g").status, STUCK);

        let book = GoalBook::default();
        book.change(|g| g.set(Goal::new("x", None, Budget::default()))).expect("set");
        for _ in 0..2 {
            after_turn(&book, spent(1), &checker).await;
        }
        assert!(after_turn(&book, spent(1), &checker).await.contains("did not write down"));

        let book = GoalBook::default();
        let small = Budget { steps: 4, ..Budget::default() };
        book.change(|g| g.set(Goal::new("y", Some(cmd.clone()), small))).expect("set");
        assert!(after_turn(&book, spent(1), &checker).await.contains("not met yet"));
        assert!(after_turn(&book, spent(1), &checker).await.contains("its 4 steps are used"));

        let book = GoalBook::default();
        book.change(|g| g.set(Goal::new("z", Some(cmd), Budget::default()))).expect("set");
        let s = after_turn(&book, TurnSpent { stopped: true, ..spent(1) }, &checker).await;
        assert!(s.contains("paused"));
    }

    #[tokio::test]
    async fn the_agent_writes_its_check_unless_the_person_gave_one() {
        let book = Arc::new(GoalBook::default());
        book.change(|g| g.set(Goal::new("docs", None, Budget::default()))).expect("set");
        let t = GoalCheckTool { book: book.clone() };
        let (_d, ctx) = {
            let d = tempfile::tempdir().expect("temp");
            let ws = ca_workspace::Workspace::open(d.path(), &[]).expect("ws");
            (d, Context::new(Arc::new(ws), None, Duration::from_secs(5)))
        };
        let o = t.run(json!({"kind": "condition", "text": "the README explains install"}), &ctx).await;
        assert!(!o.is_error, "{}", o.text);
        assert_eq!(book.get().current.expect("g").check.expect("c").by, "agent");
        let checker = Fixed(vec![false], Mutex::new(0));
        assert!(after_turn(&book, spent(1), &checker).await.contains("a fresh reader found"));
    }
}
