//! What runs without asking: the mode, the kept rules (`permissions` in settings), and what the person said
//! "always" to in this session.

use std::collections::HashSet;
use std::sync::Arc;

use ca_tools::Kind;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Reads run; edits and commands ask.
    Ask,
    /// Reads and edits run; commands ask.
    AutoEdit,
    /// Reads run; anything that changes something is refused.
    Plan,
    /// Everything runs: for a folder you can throw away.
    Yolo,
}

impl Mode {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "ask" => Mode::Ask,
            "auto-edit" | "autoedit" | "auto" => Mode::AutoEdit,
            "plan" => Mode::Plan,
            "yolo" => Mode::Yolo,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::AutoEdit => "auto-edit",
            Mode::Plan => "plan",
            Mode::Yolo => "yolo",
        }
    }
}

/// A kept rule: a tool's name, with a pattern in brackets matched against its subject (`*` is anything):
/// `run_shell(npm test*)`, `edit_file`, `read_file(.env*)`, `mcp__tickets__*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    tool: String,
    pattern: Option<String>,
}

impl Rule {
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        match t.split_once('(') {
            Some((tool, rest)) => {
                let pattern = rest.strip_suffix(')')?;
                Some(Rule { tool: tool.trim().to_string(), pattern: Some(pattern.to_string()) })
            }
            None => Some(Rule { tool: t.to_string(), pattern: None }),
        }
    }

    fn matches(&self, tool: &str, subject: &str) -> bool {
        if !wildcard(&self.tool, tool) {
            return false;
        }
        let Some(p) = &self.pattern else { return true };
        if tool == "run_shell" {
            // How it waits (`[no time limit] `, `[in the background] `) is no part of the command a rule names.
            let subject = match subject.strip_prefix('[').and_then(|r| r.split_once("] ")) {
                Some((_, rest)) => rest,
                None => subject,
            };
            // A command is matched whole and part by part: `run_shell(rm *)` refuses `make && rm -rf build` too.
            wildcard(p, subject.trim()) || command_parts(subject).iter().any(|part| wildcard(p, part))
        } else {
            wildcard(p, subject)
        }
    }
}

/// The parts of a command line, split at `&&`, `||`, `;`, `|` and line breaks (outside quotes).
pub fn command_parts(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) if c == q => {
                quote = None;
                cur.push(c);
            }
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                cur.push(c);
            }
            None if c == ';' || c == '\n' || c == '|' || (c == '&' && chars.get(i + 1) == Some(&'&')) => {
                if c == '&' || (c == '|' && chars.get(i + 1) == Some(&'|')) {
                    i += 1;
                }
                parts.push(std::mem::take(&mut cur));
            }
            None => cur.push(c),
        }
        i += 1;
    }
    parts.push(cur);
    parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// `*` matches anything (also nothing); everything else is itself.
fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[derive(Debug, Clone, Default)]
pub struct Rules {
    allow: Vec<Rule>,
    deny: Vec<Rule>,
}

impl Rules {
    /// A rule that refuses, for this session.
    pub fn deny(&mut self, rule: &str) {
        if let Some(r) = Rule::parse(rule) {
            self.deny.push(r);
        }
    }

    /// The rules as written: (allow, deny).
    pub fn written(&self) -> (Vec<String>, Vec<String>) {
        let w = |r: &Rule| match &r.pattern {
            Some(p) => format!("{}({p})", r.tool),
            None => r.tool.clone(),
        };
        (self.allow.iter().map(w).collect(), self.deny.iter().map(w).collect())
    }

    /// A rule taken away, for this session, from both lists, as it is written (`run_shell(npm test*)`): whether
    /// there was one.
    pub fn forget(&mut self, rule: &str) -> bool {
        let Some(gone) = Rule::parse(rule) else { return false };
        let same = |r: &Rule| r.tool == gone.tool && r.pattern == gone.pattern;
        let before = self.allow.len() + self.deny.len();
        self.allow.retain(|r| !same(r));
        self.deny.retain(|r| !same(r));
        self.allow.len() + self.deny.len() != before
    }

    /// A rule kept for this session only (a trusted server's tools: `mcp__name__*`).
    pub fn allow(&mut self, rule: &str) {
        if let Some(r) = Rule::parse(rule) {
            self.allow.push(r);
        }
    }

    /// From the `permissions` setting: `{"allow": [...], "deny": [...]}`.
    pub fn from_setting(v: &Value) -> Self {
        let list = |k: &str| -> Vec<Rule> {
            v.get(k)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| r.as_str().and_then(Rule::parse))
                .collect()
        };
        Rules { allow: list("allow"), deny: list("deny") }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Run,
    Ask,
    Refuse(String),
}

/// What the person said to a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    /// Yes, and for the rest of this session.
    Always,
}

/// The question before a call: what it is, how it is put to the person, and what "always" would spare them.
#[derive(Debug, Clone, Copy)]
pub struct Question<'a> {
    pub tool: &'a str,
    /// As the tool names it; a command keeps its `[no time limit] ` or `[in the background] `.
    pub subject: &'a str,
    /// "Run this command?", "Allow TOOL to change this file?", "Allow TOOL to run with these arguments?".
    pub title: &'a str,
    pub arguments: &'a Value,
    /// "Always for this session: no more questions …"; `None` when the call asks every time.
    pub always: Option<&'a str>,
    /// Laya's reading of a command, when there was one.
    pub risk: Option<Risk>,
    /// A command with no time limit.
    pub timeless: bool,
    /// A command left running in the background.
    pub background: bool,
}

#[async_trait::async_trait]
pub trait Approver: Send + Sync {
    /// Asks whether a call may run; `None` when nobody can be asked.
    async fn ask(&self, q: &Question<'_>) -> Option<Answer>;
}

/// A run that cannot ask (`-p`): what would ask is refused, with a reason the model passes on.
pub struct NoAsking;

#[async_trait::async_trait]
impl Approver for NoAsking {
    async fn ask(&self, _q: &Question<'_>) -> Option<Answer> {
        None
    }
}

/// Laya's reading of a command, each from 0 to 1: how likely it destroys something, reaches outside the
/// workspace, and uses the network.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Risk {
    #[serde(default)]
    pub destructive: f64,
    #[serde(default)]
    pub outside: f64,
    #[serde(default)]
    pub network: f64,
}

/// From this likelihood on, a command asks whatever the mode and the rules say.
pub const RISKY: f64 = 0.6;

impl Risk {
    /// Whether it asks even in yolo mode and past a kept allow rule.
    pub fn asks(&self) -> bool {
        self.destructive >= RISKY || self.outside >= RISKY || self.network >= RISKY
    }

    /// "Laya: destructive 96%, outside the workspace 93%, network 3%".
    pub fn line(&self) -> String {
        let pc = |v: f64| (v.clamp(0.0, 1.0) * 100.0).round() as u32;
        format!(
            "Laya: destructive {}%, outside the workspace {}%, network {}%",
            pc(self.destructive),
            pc(self.outside),
            pc(self.network)
        )
    }
}

/// Laya, which reads a command before it runs (Arena's, when the server has it).
#[async_trait::async_trait]
pub trait CommandCheck: Send + Sync {
    /// `None` when there is no reading: no Laya on the server, no answer in time, or no server.
    async fn check(&self, command: &str) -> Option<Risk>;
}

/// A decision with Laya's reading: a command that would run asks when any likelihood is 60% or more. A question
/// or a refusal is never taken away.
pub fn weigh(decision: Decision, risk: Option<&Risk>) -> Decision {
    match decision {
        Decision::Run if risk.is_some_and(Risk::asks) => Decision::Ask,
        d => d,
    }
}

/// How a command waits, from its subject: (no time limit, in the background), and the command itself.
pub fn waits(subject: &str) -> (bool, bool, &str) {
    if let Some(rest) = subject.strip_prefix("[no time limit] ") {
        (true, false, rest)
    } else if let Some(rest) = subject.strip_prefix("[in the background] ") {
        (false, true, rest)
    } else {
        (false, false, subject)
    }
}

/// The question's first line, by what the call does.
pub fn title(tool: &str, kind: Kind) -> String {
    match (tool, kind) {
        ("run_shell", _) => "Run this command?".into(),
        ("stop_command", _) => "Stop this command?".into(),
        (_, Kind::Edit) => format!("Allow {tool} to change this file?"),
        _ => format!("Allow {tool} to run with these arguments?"),
    }
}

/// Laya, as a session's policy holds it.
#[derive(Clone)]
pub struct Laya(pub Arc<dyn CommandCheck>);

impl std::fmt::Debug for Laya {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Laya")
    }
}

/// The mode and rules of one session, with what was said "always" to, and Laya when the server has it.
#[derive(Debug, Clone)]
pub struct Policy {
    pub mode: Mode,
    pub rules: Rules,
    always: HashSet<String>,
    pub laya: Option<Laya>,
    /// How each tool's calls ask, as the person set it in Configure Tools: "ask" asks every call in every mode,
    /// "allow" runs a call that changes something unasked (Laya still makes a risky command ask).
    pub approvals: crate::tool_settings::Approvals,
}

impl Policy {
    pub fn new(mode: Mode, rules: Rules) -> Self {
        Self { mode, rules, always: HashSet::new(), laya: None, approvals: Default::default() }
    }

    /// Laya's reading of a command; none for other tools, or with no Laya.
    pub async fn risk(&self, tool: &str, subject: &str) -> Option<Risk> {
        let laya = self.laya.as_ref().filter(|_| tool == "run_shell")?;
        laya.0.check(waits(subject).2).await
    }

    pub fn decide(&self, tool: &str, kind: Kind, subject: &str) -> Decision {
        if let Some(r) = self.rules.deny.iter().find(|r| r.matches(tool, subject)) {
            let shown = match &r.pattern {
                Some(p) => format!("{}({p})", r.tool),
                None => r.tool.clone(),
            };
            return Decision::Refuse(format!(
                "a kept rule refuses it ({shown}): find another way, or ask the person to change the rule"
            ));
        }
        if self.mode == Mode::Plan && kind != Kind::Read {
            return Decision::Refuse(
                "plan mode: nothing is changed until the plan is approved. Investigate, then propose the plan".into(),
            );
        }
        // Stopping a command the person may be relying on asks every time, in every mode.
        if tool == "stop_command" {
            return Decision::Ask;
        }
        match self.approvals.get(tool) {
            Some(crate::tool_settings::Approval::Ask) => return Decision::Ask,
            Some(crate::tool_settings::Approval::Allow) if kind != Kind::Read => return Decision::Run,
            _ => {}
        }
        if kind == Kind::Read || self.mode == Mode::Yolo {
            return Decision::Run;
        }
        if self.rules.allow.iter().any(|r| r.matches(tool, subject))
            || self.always.contains(&always_key(tool, kind, subject))
        {
            return Decision::Run;
        }
        match (self.mode, kind) {
            (Mode::AutoEdit, Kind::Edit) => Decision::Run,
            _ => Decision::Ask,
        }
    }

    /// "Always", for this session: every file edit; a command by its first two words (`npm test …`).
    pub fn remember(&mut self, tool: &str, kind: Kind, subject: &str) {
        if tool == "stop_command" {
            return;
        }
        self.always.insert(always_key(tool, kind, subject));
    }
}

/// What "always" would spare the person, in the words of the question: "Always for this session: no more
/// questions about file edits."; `None` for a call that asks every time.
pub fn always_text(tool: &str, kind: Kind, subject: &str) -> Option<String> {
    if tool == "stop_command" {
        return None;
    }
    let what = match kind {
        Kind::Edit => "about file edits".to_string(),
        Kind::Shell if tool == "run_shell" => {
            let how = match waits(subject) {
                (true, _, _) => " with no time limit",
                (_, true, _) => " in the background",
                _ => "",
            };
            format!("for commands that start with \"{}\"{how}", first_words(subject))
        }
        _ => format!("for {tool}"),
    };
    Some(format!("Always for this session: no more questions {what}."))
}

/// A command's first two words, past how it waits.
fn first_words(subject: &str) -> String {
    let (_, _, command) = waits(subject);
    command.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

fn always_key(tool: &str, kind: Kind, subject: &str) -> String {
    match kind {
        Kind::Edit => "edit".into(),
        // How it waits stays in the key: "always" for one way of waiting does not cover another.
        Kind::Shell if tool == "run_shell" => {
            let how = &subject[..subject.len() - waits(subject).2.len()];
            format!("run_shell:{how}{}", first_words(subject))
        }
        _ => tool.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modes_say_what_asks() {
        let p = |m| Policy::new(m, Rules::default());
        assert_eq!(p(Mode::Ask).decide("read_file", Kind::Read, "a"), Decision::Run);
        assert_eq!(p(Mode::Ask).decide("edit_file", Kind::Edit, "a"), Decision::Ask);
        assert_eq!(p(Mode::AutoEdit).decide("edit_file", Kind::Edit, "a"), Decision::Run);
        assert_eq!(p(Mode::AutoEdit).decide("run_shell", Kind::Shell, "ls"), Decision::Ask);
        assert!(matches!(p(Mode::Plan).decide("edit_file", Kind::Edit, "a"), Decision::Refuse(_)));
        assert_eq!(p(Mode::Yolo).decide("run_shell", Kind::Shell, "rm -rf x"), Decision::Run);
    }

    #[test]
    fn rules_deny_in_every_mode_and_match_command_parts() {
        let rules = Rules::from_setting(
            &json!({"allow": ["run_shell(npm test*)"], "deny": ["run_shell(rm *)", "read_file(.env*)"]}),
        );
        let p = Policy::new(Mode::Yolo, rules.clone());
        assert!(matches!(p.decide("run_shell", Kind::Shell, "make && rm -rf build"), Decision::Refuse(_)));
        assert!(matches!(p.decide("read_file", Kind::Read, ".env.local"), Decision::Refuse(_)));
        let p = Policy::new(Mode::Ask, rules);
        assert_eq!(p.decide("run_shell", Kind::Shell, "npm test -- --watch=false"), Decision::Run);
        assert_eq!(p.decide("run_shell", Kind::Shell, "npm install"), Decision::Ask);
    }

    #[test]
    fn a_deny_rule_holds_for_commands_with_no_time_limit_too() {
        let rules = Rules::from_setting(&json!({"deny": ["run_shell(rm *)"]}));
        let p = Policy::new(Mode::Yolo, rules);
        assert!(matches!(
            p.decide("run_shell", Kind::Shell, "[no time limit] make && rm -rf build"),
            Decision::Refuse(_)
        ));
        assert!(matches!(p.decide("run_shell", Kind::Shell, "[in the background] rm -rf /tmp/x"), Decision::Refuse(_)));
        assert_eq!(p.decide("stop_command", Kind::Shell, "job 1"), Decision::Ask, "stopping asks even in yolo");
    }

    #[test]
    fn always_covers_a_commands_first_two_words() {
        let mut p = Policy::new(Mode::Ask, Rules::default());
        p.remember("run_shell", Kind::Shell, "npm test src/a");
        assert_eq!(p.decide("run_shell", Kind::Shell, "npm test src/b"), Decision::Run);
        assert_eq!(p.decide("run_shell", Kind::Shell, "npm publish"), Decision::Ask);
        p.remember("edit_file", Kind::Edit, "a.rs");
        assert_eq!(p.decide("write_file", Kind::Edit, "b.rs"), Decision::Run);
    }

    #[test]
    fn a_tools_own_approval_holds_whatever_the_mode() {
        use crate::tool_settings::Approval;
        let mut p = Policy::new(Mode::Yolo, Rules::default());
        p.approvals.insert("write_file".into(), Approval::Ask);
        assert_eq!(p.decide("write_file", Kind::Edit, "a.txt"), Decision::Ask, "ask holds in yolo");
        p.mode = Mode::Ask;
        p.approvals.insert("run_shell".into(), Approval::Allow);
        assert_eq!(p.decide("run_shell", Kind::Shell, "ls"), Decision::Run);
        p.approvals.insert("stop_command".into(), Approval::Allow);
        assert_eq!(p.decide("stop_command", Kind::Shell, "1"), Decision::Ask, "stopping a command asks every time");
        p.rules.deny("run_shell(rm *)");
        assert!(
            matches!(p.decide("run_shell", Kind::Shell, "rm -rf x"), Decision::Refuse(_)),
            "a deny rule still refuses"
        );
        p.mode = Mode::Plan;
        assert!(matches!(p.decide("run_shell", Kind::Shell, "ls"), Decision::Refuse(_)), "plan mode still refuses");
    }

    #[test]
    fn sixty_percent_asks_even_in_yolo() {
        let p = Policy::new(Mode::Yolo, Rules::from_setting(&json!({"allow": ["run_shell(rm *)"]})));
        let risky = Risk { destructive: 0.6, outside: 0.1, network: 0.0 };
        let calm = Risk { destructive: 0.59, outside: 0.2, network: 0.03 };
        let run = p.decide("run_shell", Kind::Shell, "rm -rf ../x");
        assert_eq!(weigh(run.clone(), Some(&risky)), Decision::Ask);
        assert_eq!(weigh(run.clone(), Some(&calm)), Decision::Run);
        assert_eq!(weigh(run, None), Decision::Run, "no Laya: as before");
        let refused = Decision::Refuse("no".into());
        assert_eq!(weigh(refused.clone(), Some(&risky)), refused, "a refusal stays one");
        assert_eq!(weigh(Decision::Ask, Some(&calm)), Decision::Ask, "a question is never taken away");
    }

    #[test]
    fn laya_says_each_likelihood() {
        let r = Risk { destructive: 0.96, outside: 0.931, network: 0.03 };
        assert_eq!(r.line(), "Laya: destructive 96%, outside the workspace 93%, network 3%");
        assert!(r.asks());
        let r: Risk = serde_json::from_value(json!({"destructive": 0.1, "network": 0.7})).expect("a reading");
        assert!(r.asks());
    }

    #[test]
    fn always_says_what_it_spares_and_covers_one_way_of_waiting() {
        let mut p = Policy::new(Mode::Ask, Rules::default());
        p.remember("run_shell", Kind::Shell, "[no time limit] npm test");
        assert_eq!(p.decide("run_shell", Kind::Shell, "[no time limit] npm test -- a"), Decision::Run);
        assert_eq!(p.decide("run_shell", Kind::Shell, "npm test -- a"), Decision::Ask);
        assert_eq!(p.decide("run_shell", Kind::Shell, "[no time limit] cargo build"), Decision::Ask);
        assert_eq!(
            always_text("run_shell", Kind::Shell, "[in the background] npm run dev").as_deref(),
            Some(
                "Always for this session: no more questions for commands that start with \"npm run\" in the background."
            )
        );
        assert_eq!(
            always_text("run_shell", Kind::Shell, "npm test").as_deref(),
            Some("Always for this session: no more questions for commands that start with \"npm test\".")
        );
        assert_eq!(
            always_text("edit_file", Kind::Edit, "a.rs").as_deref(),
            Some("Always for this session: no more questions about file edits.")
        );
        assert_eq!(always_text("stop_command", Kind::Shell, "job 1"), None);
        assert_eq!(waits("[no time limit] make"), (true, false, "make"));
        assert_eq!(waits("[in the background] npm run dev"), (false, true, "npm run dev"));
        assert_eq!(title("run_shell", Kind::Shell), "Run this command?");
    }

    #[test]
    fn command_parts_respect_quotes() {
        assert_eq!(command_parts("a && b || c; d | e"), vec!["a", "b", "c", "d", "e"]);
        assert_eq!(command_parts("echo 'a && b' && ls"), vec!["echo 'a && b'", "ls"]);
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("mcp__tickets__*", "mcp__tickets__open"));
        assert!(wildcard("*", ""));
        assert!(wildcard("a*b*c", "aXbYc"));
        assert!(!wildcard("a*b", "ac"));
    }
}
