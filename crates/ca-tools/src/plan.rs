//! The agent's working plan (the `plan` tool): a title, a goal and numbered steps, each with what done means (a check:
//! a command whose success shows it, or words to look by), the files and symbols it changes, the steps it waits for,
//! and notes. A step whose check is a command is done only once that command passed since the step began: `run_shell`
//! records how each check went. The plan is kept with the session (said again after each compaction), shown in the
//! terminal, and kept in Arena with the session's chat (the Plans board). Arena's rules hold: steps count from 1, each
//! waits only for steps the plan has, never in a circle, one doing at a time.

use std::sync::{Arc, Mutex};

use ca_llm::ToolDef;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{Context, Kind, Output, Tool};

pub const MOST_STEPS: usize = 200;
const MOST_FILES: usize = 50;
const MOST_SYMBOLS: usize = 20;

pub const TODO: &str = "todo";
pub const DOING: &str = "doing";
pub const BLOCKED: &str = "blocked";
pub const DONE: &str = "done";
const STATES: [&str; 4] = [TODO, DOING, BLOCKED, DONE];

/// Made in plan mode: the person reviews it before it is carried out.
pub const PROPOSED: &str = "proposed";
pub const APPROVED: &str = "approved";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub number: u32,
    pub title: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// "passed" or "failed", since the step began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
    /// What a change to its symbols reaches, as Farzan Graph found it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
}

impl Step {
    fn new(number: u32) -> Self {
        Step {
            number,
            title: String::new(),
            status: TODO.into(),
            check: None,
            files: vec![],
            symbols: vec![],
            depends_on: vec![],
            notes: None,
            check_result: None,
            check_output: None,
            checked_at: None,
            impact: None,
        }
    }

    fn clear_check(&mut self) {
        self.check_result = None;
        self.check_output = None;
        self.checked_at = None;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// [`PROPOSED`], [`APPROVED`], or none (made outside plan mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    /// What a second reading said of it before the person's review (empty: it held); none: not read again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critique: Option<String>,
}

/// A plan refused: the words say what to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError(pub String);

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn cut(s: &str, most: usize) -> String {
    if s.chars().count() <= most { s.to_string() } else { s.chars().take(most).collect() }
}

fn tail(s: &str, most: usize) -> String {
    let n = s.chars().count();
    if n <= most { s.to_string() } else { format!("…{}", s.chars().skip(n - most + 1).collect::<String>()) }
}

/// Other agents' words for a status, as the plan's own.
fn status_word(s: &str) -> Option<&'static str> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "todo" | "to do" | "pending" | "not_started" | "open" => TODO,
        "doing" | "in_progress" | "in progress" | "active" | "started" => DOING,
        "done" | "completed" | "complete" | "finished" => DONE,
        "blocked" | "failed" | "stuck" => BLOCKED,
        _ => return None,
    })
}

/// A call as the model wrote it, read kindly: the steps as a string of JSON, other names for a step's fields (id,
/// description, dependsOn), other agents' status words. What was read so is said back.
fn kindly(args: &Value, mended: &mut Vec<String>) -> Value {
    let mut a = args.clone();
    if let Some(Value::String(text)) = a.get("steps")
        && let Ok(parsed @ Value::Array(_)) = serde_json::from_str::<Value>(text)
    {
        a["steps"] = parsed;
        mended.push("steps came as a string of JSON: read as the list".into());
    }
    if let Some(Value::Array(steps)) = a.get_mut("steps") {
        for step in steps.iter_mut().filter_map(Value::as_object_mut) {
            let alias = |step: &mut Map<String, Value>, name: &str, others: &[&str]| {
                if step.contains_key(name) {
                    return;
                }
                for o in others {
                    if let Some(v) = step.remove(*o) {
                        step.insert(name.into(), v);
                        return;
                    }
                }
            };
            alias(step, "number", &["id", "step", "index"]);
            alias(step, "title", &["name", "description", "content", "task"]);
            alias(step, "depends_on", &["dependsOn", "dependencies", "after"]);
            alias(step, "check", &["done_when", "verify", "test"]);
            if let Some(Value::String(n)) = step.get("number")
                && let Ok(n) = n.trim().trim_start_matches('#').parse::<u32>()
            {
                step.insert("number".into(), json!(n));
            }
            if let Some(Value::String(s)) = step.get("status")
                && !STATES.contains(&s.as_str())
                && let Some(known) = status_word(s)
            {
                step.insert("status".into(), json!(known));
            }
        }
    }
    a
}

fn str_of<'a>(o: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    o.get(k).and_then(Value::as_str)
}

fn strings(v: &Value, most: usize, each: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in v.as_array().into_iter().flatten() {
        let s = match x {
            Value::String(s) => s.trim().replace('\\', "/"),
            other => other.to_string(),
        };
        if !s.is_empty() && !out.contains(&s) && out.len() < most {
            out.push(cut(&s, each));
        }
    }
    out
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.title.is_empty()
    }

    pub fn current(&self) -> Option<&Step> {
        self.steps.iter().find(|s| s.status == DOING)
    }

    pub fn done(&self) -> usize {
        self.steps.iter().filter(|s| s.status == DONE).count()
    }

    /// The plan with the tool's changes: its title and goal, and steps (with a number: that step changed, or taken out
    /// with `remove`; without: one added at the end); `clear` starts again from nothing. `is_command` says whether a
    /// check is a command to run (a program here), not words to look by. Gives what was read kindly or done for it.
    pub fn apply(&self, args: &Value, is_command: &dyn Fn(&str) -> bool) -> Result<(Plan, Vec<String>), PlanError> {
        let mut mended = Vec::new();
        let args = kindly(args, &mut mended);
        let base =
            if args.get("clear").and_then(Value::as_bool) == Some(true) { Plan::default() } else { self.clone() };
        let title = match args.get("title").and_then(Value::as_str).map(str::trim) {
            Some(t) if !t.is_empty() => cut(t, 200),
            _ => base.title.clone(),
        };
        let goal = match args.get("goal").and_then(Value::as_str) {
            Some(g) if g.trim().is_empty() => None,
            Some(g) => Some(cut(g.trim(), 4000)),
            None => base.goal.clone(),
        };
        let mut steps = base.steps.clone();
        let changes: Vec<Map<String, Value>> = args
            .get("steps")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_object().cloned()).collect())
            .unwrap_or_default();
        let mut begun_now: Vec<u32> = Vec::new();
        for change in &changes {
            let remove = change.get("remove").and_then(Value::as_bool) == Some(true);
            let new_title = str_of(change, "title").map(str::trim).filter(|t| !t.is_empty());
            let number = change.get("number").and_then(Value::as_u64).map(|n| n as u32);
            let last = steps.iter().map(|s| s.number).max().unwrap_or(0);
            let at = match number {
                Some(n) => match steps.iter().position(|s| s.number == n) {
                    Some(i) => Some(i),
                    // A new step with the number it will have (a plan written whole, its steps numbered): added.
                    None if !remove && new_title.is_some() && n > last => {
                        steps.push(Step::new(n));
                        Some(steps.len() - 1)
                    }
                    None => {
                        let have: Vec<String> = steps.iter().map(|s| s.number.to_string()).collect();
                        return Err(PlanError(format!(
                            "The plan has no step {n}: leave out number to add a step (its steps are {}).",
                            if have.is_empty() { "none yet".into() } else { have.join(", ") }
                        )));
                    }
                },
                None => {
                    if new_title.is_none() {
                        return Err(PlanError("A new step needs a title.".into()));
                    }
                    steps.push(Step::new(last + 1));
                    Some(steps.len() - 1)
                }
            };
            let Some(at) = at else { continue };
            if remove {
                let gone = steps.remove(at).number;
                for s in &mut steps {
                    s.depends_on.retain(|d| *d != gone);
                }
                continue;
            }
            let mut step = steps[at].clone();
            if let Some(t) = new_title {
                step.title = cut(t, 300);
            }
            if let Some(c) = str_of(change, "check") {
                step.check = if c.trim().is_empty() { None } else { Some(cut(c.trim(), 1000)) };
                step.clear_check();
            }
            if let Some(status) = str_of(change, "status") {
                if !STATES.contains(&status) {
                    return Err(PlanError(format!(
                        "A step's status is one of {}, not \"{status}\".",
                        STATES.join(", ")
                    )));
                }
                if status == DOING && step.status != DOING {
                    // Begun (again): its check counts from now.
                    step.clear_check();
                    begun_now.push(step.number);
                }
                if status == DONE
                    && step.status != DONE
                    && let Some(command) = &step.check
                    && is_command(command)
                    && step.check_result.as_deref() != Some("passed")
                {
                    let failed = step.check_result.as_deref() == Some("failed");
                    return Err(PlanError(format!(
                        "Step {} is done only once its check passed: run it with run_shell (`{command}`) since the step began{}",
                        step.number,
                        if failed {
                            "; it failed last time: fix what it says, or change the step and say why in its notes."
                        } else {
                            "."
                        }
                    )));
                }
                step.status = status.to_string();
            }
            if let Some(f) = change.get("files") {
                step.files = strings(f, MOST_FILES, 300);
            }
            if let Some(f) = change.get("symbols") {
                let named = strings(f, MOST_SYMBOLS, 200);
                if named != step.symbols {
                    step.impact = None;
                }
                step.symbols = named;
            }
            if let Some(Value::Array(d)) = change.get("depends_on") {
                let mut deps: Vec<u32> = d
                    .iter()
                    .filter_map(|x| {
                        x.as_u64().or_else(|| x.as_str().and_then(|s| s.trim_start_matches('#').parse().ok()))
                    })
                    .map(|n| n as u32)
                    .collect();
                deps.sort_unstable();
                deps.dedup();
                step.depends_on = deps;
            }
            if let Some(n) = str_of(change, "notes") {
                step.notes = if n.trim().is_empty() { None } else { Some(cut(n.trim(), 4000)) };
            }
            steps[at] = step;
        }
        // A step begun while another was doing: that one waits again (one doing at a time).
        let doing: Vec<u32> = steps.iter().filter(|s| s.status == DOING).map(|s| s.number).collect();
        if doing.len() > 1 {
            let keep = begun_now.last().copied().filter(|n| doing.contains(n)).unwrap_or(doing[doing.len() - 1]);
            for s in steps.iter_mut().filter(|s| s.status == DOING && s.number != keep) {
                s.status = TODO.into();
                mended.push(format!(
                    "step {} was doing: it is to do again, as step {keep} began (mark it done when its check passed)",
                    s.number
                ));
            }
        }
        steps.sort_by_key(|s| s.number);
        let made = Plan {
            title: if title.is_empty() && !steps.is_empty() { "Plan".into() } else { title },
            goal,
            steps,
            review: base.review.clone(),
            critique: base.critique.clone(),
        };
        made.validate()?;
        Ok((made, mended))
    }

    fn validate(&self) -> Result<(), PlanError> {
        if self.steps.len() > MOST_STEPS {
            return Err(PlanError(format!("A plan holds at most {MOST_STEPS} steps.")));
        }
        let numbers: Vec<u32> = self.steps.iter().map(|s| s.number).collect();
        for s in &self.steps {
            if let Some(bad) = s.depends_on.iter().find(|d| !numbers.contains(d) || **d == s.number) {
                return Err(PlanError(format!(
                    "Step {} waits for step {bad}, which {}.",
                    s.number,
                    if *bad == s.number { "is itself" } else { "the plan does not have" }
                )));
            }
        }
        if let Some(circle) = self.circle() {
            let path: Vec<String> = circle.iter().map(u32::to_string).collect();
            return Err(PlanError(format!("Steps {} wait for one another in a circle.", path.join(" → "))));
        }
        Ok(())
    }

    fn circle(&self) -> Option<Vec<u32>> {
        fn visit(
            n: u32,
            plan: &Plan,
            state: &mut std::collections::HashMap<u32, u8>,
            path: &mut Vec<u32>,
        ) -> Option<Vec<u32>> {
            state.insert(n, 1);
            path.push(n);
            let deps = plan.steps.iter().find(|s| s.number == n).map(|s| s.depends_on.clone()).unwrap_or_default();
            for d in deps {
                match state.get(&d).copied().unwrap_or(0) {
                    1 => {
                        let mut c: Vec<u32> = path.iter().copied().skip_while(|x| *x != d).collect();
                        c.push(d);
                        return Some(c);
                    }
                    0 => {
                        if let Some(found) = visit(d, plan, state, path) {
                            return Some(found);
                        }
                    }
                    _ => {}
                }
            }
            state.insert(n, 2);
            path.pop();
            None
        }
        let mut state = std::collections::HashMap::new();
        for s in &self.steps {
            if state.get(&s.number).copied().unwrap_or(0) == 0
                && let Some(c) = visit(s.number, self, &mut state, &mut Vec::new())
            {
                return Some(c);
            }
        }
        None
    }

    /// A command ran: the step whose check it is (the one doing, else one not done) records how it went. `None` when no
    /// step's check is that command.
    pub fn ran(&self, command: &str, passed: bool, output: &str, at: &str) -> Option<Plan> {
        let said = normal(command);
        let finds: [&dyn Fn(&Step) -> bool; 4] = [
            &|s| s.status == DOING && s.check.as_deref().is_some_and(|c| normal(c) == said),
            &|s| s.status != DONE && s.check.as_deref().is_some_and(|c| normal(c) == said),
            &|s| s.status == DOING && s.check.as_deref().is_some_and(|c| runs(command, c)),
            &|s| s.status != DONE && s.check.as_deref().is_some_and(|c| runs(command, c)),
        ];
        let number = finds.iter().find_map(|f| self.steps.iter().find(|s| f(s)).map(|s| s.number))?;
        let mut p = self.clone();
        for s in p.steps.iter_mut().filter(|s| s.number == number) {
            s.check_result = Some(if passed { "passed" } else { "failed" }.into());
            s.check_output = Some(tail(output, 4000));
            s.checked_at = Some(at.to_string());
        }
        Some(p)
    }

    /// The plan for the model to read: each step on a line, with what done means and how its check went.
    pub fn text(&self) -> String {
        if self.is_empty() {
            return "No plan.".into();
        }
        let review = match self.review.as_deref() {
            Some(PROPOSED) => " (proposed: the person reviews it before it is carried out)",
            Some(APPROVED) => " (approved)",
            _ => "",
        };
        let mut lines = vec![format!(
            "Plan: {}{review}{}",
            self.title,
            self.goal.as_ref().map(|g| format!("\nGoal: {g}")).unwrap_or_default()
        )];
        for s in &self.steps {
            let mut line = format!("{}. [{}] {}", s.number, s.status, s.title);
            if !s.depends_on.is_empty() {
                let d: Vec<String> = s.depends_on.iter().map(u32::to_string).collect();
                line.push_str(&format!(" (after {})", d.join(", ")));
            }
            if let Some(c) = &s.check {
                line.push_str(&format!(
                    "\n   check: {c}{}",
                    s.check_result.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()
                ));
            }
            if !s.files.is_empty() {
                line.push_str(&format!("\n   files: {}", s.files.join(", ")));
            }
            if !s.symbols.is_empty() {
                line.push_str(&format!("\n   symbols: {}", s.symbols.join(", ")));
            }
            if let Some(i) = &s.impact {
                line.push_str(&format!("\n   impact: {}", i.replace('\n', "\n           ")));
            }
            if let Some(n) = &s.notes {
                line.push_str(&format!("\n   notes: {}", cut(&n.replace('\n', " "), 300)));
            }
            lines.push(line);
        }
        lines.push(format!("{} of {} done.", self.done(), self.steps.len()));
        lines.join("\n")
    }

    /// Arena's shape for it (`PUT /api/code-arena/chats/{id}/plan`).
    pub fn to_arena(&self, place: &str) -> Value {
        json!({
            "title": self.title,
            "goal": self.goal,
            "place": place,
            "review": self.review,
            "steps": self.steps.iter().map(|s| json!({
                "number": s.number, "title": s.title, "status": s.status, "check": s.check, "files": s.files,
                "dependsOn": s.depends_on, "notes": s.notes, "checkResult": s.check_result,
                "checkOutput": s.check_output, "checkedAt": s.checked_at,
            })).collect::<Vec<_>>(),
        })
    }
}

fn normal(command: &str) -> String {
    command.trim().trim_matches('`').split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a command run is a step's check: as written, or as one of the commands it chains (`cd src && cargo test`,
/// `cargo test 2>&1 | tail`).
pub fn runs(ran: &str, check: &str) -> bool {
    let (r, c) = (normal(ran), normal(check));
    if r == c {
        return true;
    }
    if c.is_empty() {
        return false;
    }
    let mut parts = vec![r.as_str()];
    for sep in ["&&", "||", ";", "|"] {
        parts = parts.iter().flat_map(|p| p.split(sep)).collect();
    }
    parts.iter().any(|p| {
        // A redirection at its end (2>&1, >out.txt) is not part of the command.
        let words: Vec<&str> = p.split_whitespace().collect();
        let end = words
            .iter()
            .position(|w| w.starts_with('>') || w.starts_with("2>") || w.starts_with("1>"))
            .unwrap_or(words.len());
        words[..end].join(" ") == c
    })
}

/// Whether a check is a command: its first word a program on the PATH, or a script in the project.
pub fn is_command(check: &str, root: &std::path::Path) -> bool {
    let Some(first) = check.trim().trim_matches('`').split_whitespace().next() else { return false };
    let Some(c) = first.chars().next() else { return false };
    if !c.is_ascii_alphanumeric() && c != '.' && c != '/' {
        return false;
    }
    if first.contains('/') || first.contains('\\') {
        let p = std::path::Path::new(first);
        return if p.is_absolute() { p.exists() } else { root.join(p).exists() };
    }
    crate::shell::which(first)
}

/// What the session does when a step begins: the checkpoints keep what its writes change.
pub type OnBegin = Arc<dyn Fn(u32) + Send + Sync>;

/// What the session does when the plan changes: keep it, and send it to Arena.
pub type OnChange = Arc<dyn Fn(&Plan) + Send + Sync>;

/// The session's plan, shared by the plan tool, `run_shell` (which records checks) and the session.
#[derive(Default)]
pub struct PlanBook {
    plan: Mutex<Plan>,
    on_change: Mutex<Option<OnChange>>,
    /// A step began: the checkpoints keep what its writes change, for `/plan undo N`.
    on_begin: Mutex<Option<OnBegin>>,
}

impl std::fmt::Debug for PlanBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanBook").field("plan", &self.get()).finish()
    }
}

impl PlanBook {
    pub fn get(&self) -> Plan {
        self.plan.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// Changes it, and tells the session.
    pub fn set(&self, plan: Plan) {
        let before = self.get();
        let begun: Vec<u32> = plan
            .steps
            .iter()
            .filter(|s| s.status == DOING && before.steps.iter().all(|b| b.number != s.number || b.status != DOING))
            .map(|s| s.number)
            .collect();
        if let Ok(mut p) = self.plan.lock() {
            *p = plan.clone();
        }
        if let Some(f) = self.on_begin.lock().ok().and_then(|g| g.clone()) {
            for n in begun {
                f(n);
            }
        }
        if let Some(f) = self.on_change.lock().ok().and_then(|g| g.clone()) {
            f(&plan);
        }
    }

    /// Set from the session's store: not a change to tell.
    pub fn load(&self, plan: Plan) {
        if let Ok(mut p) = self.plan.lock() {
            *p = plan;
        }
    }

    pub fn on_change(&self, f: OnChange) {
        if let Ok(mut g) = self.on_change.lock() {
            *g = Some(f);
        }
    }

    pub fn on_begin(&self, f: OnBegin) {
        if let Ok(mut g) = self.on_begin.lock() {
            *g = Some(f);
        }
    }

    /// A command ran: a step's check, if it is one.
    pub fn ran(&self, command: &str, passed: bool, output: &str) {
        let at = chrono_now();
        if let Some(p) = self.get().ran(command, passed, output, &at) {
            self.set(p);
        }
    }
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The `plan` tool.
pub struct PlanTool;

#[async_trait::async_trait]
impl Tool for PlanTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "plan".into(),
            description: "Your working plan for a task of several steps: a title, a goal and numbered steps. Give only what changes: a step with its number changes it (or remove: true takes it out); one without a number is added at the end; clear: true starts again. Each step says what done means in check (a command whose success shows it, such as `cargo test -p parser`, or what to look at). Mark a step doing as you start it and done when it is: a step whose check is a command is done only once that command passed with run_shell since the step began. One step is doing at a time. depends_on names the steps it waits for. The plan is shown to the person, kept with the session and said again after a compaction. Call it with nothing to read the plan.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "goal": {"type": "string", "description": "What the whole plan achieves."},
                    "clear": {"type": "boolean", "description": "Start again from nothing."},
                    "steps": {"type": "array", "items": {"type": "object", "properties": {
                        "number": {"type": "integer", "description": "The step to change; leave out to add one."},
                        "title": {"type": "string"},
                        "status": {"type": "string", "enum": STATES},
                        "check": {"type": "string", "description": "What done means: a command, or what to look at."},
                        "files": {"type": "array", "items": {"type": "string"}},
                        "symbols": {"type": "array", "items": {"type": "string"}, "description": "The functions, types or classes it changes."},
                        "depends_on": {"type": "array", "items": {"type": "integer"}},
                        "notes": {"type": "string"},
                        "remove": {"type": "boolean"}
                    }}}
                }
            }),
        }
    }

    fn kind(&self) -> Kind {
        Kind::Read
    }

    fn subject(&self, args: &Value) -> String {
        args.get("title").and_then(Value::as_str).unwrap_or("").to_string()
    }

    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let book = &ctx.plan;
        let empty = args.as_object().is_none_or(|o| o.is_empty());
        if empty {
            return Output::ok(book.get().text());
        }
        let root = ctx.workspace.root().to_path_buf();
        let is_cmd = move |c: &str| is_command(c, &root);
        match book.get().apply(&args, &is_cmd) {
            Ok((mut plan, mended)) => {
                // Made in plan mode: proposed, for the person's review.
                if ctx.planning && plan.review.is_none() && !plan.is_empty() {
                    plan.review = Some(PROPOSED.into());
                }
                book.set(plan.clone());
                let mut text = plan.text();
                if !mended.is_empty() {
                    text.push_str(&format!("\n(Read so: {}.)", mended.join("; ")));
                }
                if plan.review.as_deref() == Some(PROPOSED) {
                    text.push_str("\nThe person reviews it now (approve, change a step, another way, reject): end your answer here, saying what the plan does and what you would like them to look at.");
                }
                Output::ok(text)
            }
            Err(e) => Output::error(e.0),
        }
    }
}

#[cfg(test)]
mod tests;
