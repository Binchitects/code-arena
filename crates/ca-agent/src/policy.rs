//! What runs without asking: the mode, the kept rules (`permissions` in settings), and what the person said
//! "always" to in this session.

use std::collections::HashSet;

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

#[async_trait::async_trait]
pub trait Approver: Send + Sync {
    /// Asks whether a call may run; `None` when nobody can be asked.
    async fn ask(&self, tool: &str, subject: &str) -> Option<Answer>;
}

/// A run that cannot ask (`-p`): what would ask is refused, with a reason the model passes on.
pub struct NoAsking;

#[async_trait::async_trait]
impl Approver for NoAsking {
    async fn ask(&self, _tool: &str, _subject: &str) -> Option<Answer> {
        None
    }
}

/// The mode and rules of one session, with what was said "always" to.
#[derive(Debug, Clone)]
pub struct Policy {
    pub mode: Mode,
    pub rules: Rules,
    always: HashSet<String>,
}

impl Policy {
    pub fn new(mode: Mode, rules: Rules) -> Self {
        Self { mode, rules, always: HashSet::new() }
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

fn always_key(tool: &str, kind: Kind, subject: &str) -> String {
    match kind {
        Kind::Edit => "edit".into(),
        Kind::Shell if tool == "run_shell" => {
            let words: Vec<&str> = subject.split_whitespace().take(2).collect();
            format!("run_shell:{}", words.join(" "))
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
