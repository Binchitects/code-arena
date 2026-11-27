//! What a call's card says, worked out from the call alone: where it stands, its icon, its main argument, the
//! line its run of calls folds into, the chip of the file it read or changed, and how its arguments and its result
//! are shown. The views are in `tools.rs`.

use serde_json::Value;

use crate::model::Tool;

/// Where a call stands, as its card says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// It asks the person first.
    Waiting,
    Running,
    Done,
    Failed,
    /// The person, the mode or a hook said no.
    Denied,
    /// The turn ended before it ran.
    NotRun,
}

/// Where a call stands; `running` says whether its turn still runs (a call not ended in a turn that ended did not
/// run).
pub fn state(t: &Tool, running: bool) -> State {
    if !t.done {
        return match (running, t.status.as_str()) {
            (false, _) => State::NotRun,
            (true, "waiting") => State::Waiting,
            (true, _) => State::Running,
        };
    }
    match t.status.as_str() {
        "denied" => State::Denied,
        "skipped" => State::NotRun,
        _ if !t.ok => State::Failed,
        _ => State::Done,
    }
}

/// A time as VS Code says one: "320 ms", "1.4s", "2m 5s".
pub fn took(ms: u64) -> String {
    match ms {
        0..1000 => format!("{ms} ms"),
        1000..60_000 => format!("{:.1}s", ms as f64 / 1000.0),
        _ => format!("{}m {}s", ms / 60_000, ms / 1000 % 60),
    }
}

/// How many things a call's result holds, in its tool's word ("42 lines", "3 results"); none for a tool whose
/// result is not a list.
pub fn count(t: &Tool) -> Option<String> {
    let (one, many) = match t.name.as_str() {
        "read_file" => ("line", "lines"),
        "grep" => ("result", "results"),
        "glob" => ("file", "files"),
        "list_dir" => ("entry", "entries"),
        _ => return None,
    };
    let n = match t.name.as_str() {
        "read_file" => code_of(&t.text).0.lines().count(),
        "grep" => locations(&t.text).iter().filter(|l| l.line > 0).count(),
        _ => t.text.lines().filter(|l| !l.trim().is_empty()).count(),
    };
    Some(format!("{n}{} {}", if t.cut { "+" } else { "" }, if n == 1 { one } else { many }))
}

/// What a card's head says after its title: where it stands, and for a call that ran, its count and its time.
/// `secs` is how long a running call has run.
pub fn status(t: &Tool, state: State, secs: u64) -> String {
    match state {
        State::Waiting => "Waiting for you".into(),
        State::Running => match &t.progress {
            Some(p) if p.total > 0 => format!("{} of {} done", p.done, p.total),
            _ if secs > 0 => format!("Running · {secs}s"),
            _ => "Running".into(),
        },
        State::Denied => "Not allowed".into(),
        State::Failed => "Failed".into(),
        State::NotRun => "Not run".into(),
        State::Done => {
            let parts: Vec<String> = count(t).into_iter().chain((t.ms > 0).then(|| took(t.ms))).collect();
            parts.join(" · ")
        }
    }
}

/// The codicon of a call, by its tool.
pub fn icon_of(name: &str) -> &'static str {
    match name {
        "read_file" => "file",
        "edit_file" => "edit",
        "write_file" => "new-file",
        "grep" => "search",
        "glob" => "search-fuzzy",
        "list_dir" => "folder",
        "run_shell" | "run_steps" => "terminal",
        "command_output" => "output",
        "stop_command" => "debug-stop",
        "git" | "propose_commits" => "git-commit",
        "plan" | "goal_check" => "checklist",
        "task" => "hubot",
        "skill" => "book",
        n if n.starts_with("web") => "globe",
        _ => "tools",
    }
}

/// What a call is about, on its head: what its tool says, else its first short text argument.
pub fn main_argument(t: &Tool) -> String {
    if !t.subject.is_empty() {
        return t.subject.clone();
    }
    t.arguments
        .as_object()
        .and_then(|o| o.values().find_map(|v| v.as_str().filter(|s| !s.contains('\n') && !s.is_empty())))
        .map(|s| if s.chars().count() > 80 { s.chars().take(80).chain(['…']).collect() } else { s.to_string() })
        .unwrap_or_default()
}

/// A part of an answer's calls as drawn: one call's card, or a run of calls folded into one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    One(usize),
    /// The calls from `start`, `len` of them.
    Fold {
        start: usize,
        len: usize,
    },
}

/// An answer's calls in groups: two or more ended calls in a row (a task's excepted, which has a card of its own,
/// and an edit with its diff, shown under it) fold into a line; a call that waits or runs is drawn by itself.
pub fn groups(tools: &[Tool], running: bool) -> Vec<Group> {
    let folds = |t: &Tool| {
        t.name != "task" && !(t.ok && t.diff.is_some()) && !matches!(state(t, running), State::Waiting | State::Running)
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < tools.len() {
        let len = tools[i..].iter().take_while(|t| folds(t)).count();
        if len >= 2 {
            out.push(Group::Fold { start: i, len });
            i += len;
        } else {
            out.push(Group::One(i));
            i += 1;
        }
    }
    out
}

/// A folded run's line: "Used 5 tools · read_file ×3, grep, run_shell", its tools in the order first used.
pub fn fold_line(tools: &[Tool]) -> String {
    let mut names: Vec<(&str, usize)> = Vec::new();
    for t in tools {
        match names.iter_mut().find(|(n, _)| *n == t.name) {
            Some((_, c)) => *c += 1,
            None => names.push((&t.name, 1)),
        }
    }
    let listed: Vec<String> =
        names.iter().map(|(n, c)| if *c > 1 { format!("{n} ×{c}") } else { n.to_string() }).collect();
    format!("Used {} tool{} · {}", tools.len(), if tools.len() == 1 { "" } else { "s" }, listed.join(", "))
}

/// The file a call read or changed and its lines, for the chip under it: `src/a.ts:12-14` (the path, then the
/// first line and the last), for `read_file`, `edit_file` and `write_file` only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub path: String,
    /// From 1 (0: the whole file).
    pub first: usize,
    pub last: usize,
}

impl Chip {
    pub fn of(t: &Tool) -> Option<Chip> {
        if !matches!(t.name.as_str(), "read_file" | "edit_file" | "write_file") {
            return None;
        }
        let path = t.arguments["path"].as_str().map(str::to_string).unwrap_or_else(|| t.subject.clone());
        if path.is_empty() {
            return None;
        }
        let n = |k: &str| t.arguments[k].as_u64().map(|n| n as usize);
        let (first, last) = match t.name.as_str() {
            "read_file" => match (n("offset"), n("limit")) {
                (None, None) => (0, 0),
                (o, l) => {
                    let first = o.unwrap_or(1).max(1);
                    (first, l.map(|l| first + l.max(1) - 1).unwrap_or(first))
                }
            },
            // An edit's lines: those of its changes, as its diff numbers them.
            _ => t
                .diff
                .as_ref()
                .map(|d| {
                    let at: Vec<usize> = d.lines.iter().filter(|l| l.op != ' ').map(|l| l.new.max(l.old)).collect();
                    (at.iter().copied().min().unwrap_or(0), at.iter().copied().max().unwrap_or(0))
                })
                .unwrap_or((0, 0)),
        };
        Some(Chip { path, first, last })
    }

    pub fn text(&self) -> String {
        match (self.first, self.last) {
            (0, _) => self.path.clone(),
            (f, l) if l > f => format!("{}:{f}-{l}", self.path),
            (f, _) => format!("{}:{f}", self.path),
        }
    }
}

/// A call's arguments as "Asked with" shows them: the short ones listed (name, value), the long texts as code
/// (name, text), and the rest as JSON.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asked {
    pub short: Vec<(String, String)>,
    pub code: Vec<(String, String)>,
    pub rest: Option<String>,
}

pub fn asked(arguments: &Value) -> Asked {
    let mut out = Asked::default();
    let Some(o) = arguments.as_object() else {
        if !arguments.is_null() {
            out.rest = serde_json::to_string_pretty(arguments).ok();
        }
        return out;
    };
    let mut rest = serde_json::Map::new();
    for (k, v) in o {
        match v {
            Value::String(s) if s.contains('\n') || s.chars().count() > 120 => out.code.push((k.clone(), s.clone())),
            Value::String(s) => out.short.push((k.clone(), s.clone())),
            Value::Number(_) | Value::Bool(_) | Value::Null => out.short.push((k.clone(), v.to_string())),
            _ => {
                rest.insert(k.clone(), v.clone());
            }
        }
    }
    if !rest.is_empty() {
        out.rest = serde_json::to_string_pretty(&Value::Object(rest)).ok();
    }
    out
}

/// A place in a result that opens: a file, at a line (0: the file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub line: usize,
    pub text: String,
}

/// The places a search's result names, a line each: `path:line: text` (grep's) or a path alone (glob's); a line
/// that is neither is left out.
pub fn locations(text: &str) -> Vec<Location> {
    text.lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, ':');
            let (path, second) = (parts.next()?, parts.next());
            match second.map(|s| s.trim().parse::<usize>()) {
                Some(Ok(line)) if !path.is_empty() => {
                    Some(Location { path: path.to_string(), line, text: parts.next().unwrap_or("").trim().to_string() })
                }
                None if !path.trim().is_empty() && !path.contains(' ') => {
                    Some(Location { path: path.trim().to_string(), line: 0, text: String::new() })
                }
                _ => None,
            }
        })
        .collect()
}

/// A file's lines as `read_file` gives them, without their numbers, and the first line's number (the words after
/// the lines, such as "… 40 more lines", are left out).
pub fn code_of(text: &str) -> (String, usize) {
    let mut first = 0;
    let mut out = String::new();
    for l in text.lines() {
        let Some((n, rest)) = l.split_once('\t') else { continue };
        let Ok(n) = n.trim().parse::<usize>() else { continue };
        if first == 0 {
            first = n;
        }
        out.push_str(rest);
        out.push('\n');
    }
    (out, first)
}

/// A result's rows, when it is a list of records (a JSON array of objects): their columns, in the order first
/// seen, and each row's values as text.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub fn table(text: &str) -> Option<Table> {
    let t = text.trim_start();
    if !t.starts_with('[') {
        return None;
    }
    let rows: Vec<serde_json::Map<String, Value>> = serde_json::from_str::<Vec<Value>>(t)
        .ok()?
        .into_iter()
        .map(|v| v.as_object().cloned())
        .collect::<Option<_>>()?;
    if rows.is_empty() {
        return None;
    }
    let mut columns: Vec<String> = Vec::new();
    for r in &rows {
        for k in r.keys() {
            if !columns.contains(k) {
                columns.push(k.clone());
            }
        }
    }
    let cell = |v: Option<&Value>| match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    };
    let rows = rows.iter().map(|r| columns.iter().map(|c| cell(r.get(c))).collect()).collect();
    Some(Table { columns, rows })
}

/// Rows sorted by a column, numbers as numbers, the rest as text; `down` for the largest first.
pub fn sorted(rows: &[Vec<String>], column: usize, down: bool) -> Vec<Vec<String>> {
    let mut out = rows.to_vec();
    out.sort_by(|a, b| {
        let (x, y) = (a.get(column).map(String::as_str).unwrap_or(""), b.get(column).map(String::as_str).unwrap_or(""));
        let o = match (x.parse::<f64>(), y.parse::<f64>()) {
            (Ok(x), Ok(y)) => x.total_cmp(&y),
            _ => x.to_lowercase().cmp(&y.to_lowercase()),
        };
        if down { o.reverse() } else { o }
    });
    out
}

/// How a result is drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    /// A file's lines, highlighted by its path's language, from the given line.
    File {
        text: String,
        lang: String,
        first: usize,
    },
    Locations(Vec<Location>),
    Table(Table),
    /// What a command wrote.
    Console(String),
    Raw(String),
}

pub fn shown(t: &Tool) -> Shown {
    let ext = |p: &str| p.rsplit_once('.').map(|(_, e)| e.to_string()).unwrap_or_default();
    match t.name.as_str() {
        "read_file" if t.ok => {
            let (text, first) = code_of(&t.text);
            if first > 0 {
                let path = t.arguments["path"].as_str().unwrap_or(&t.subject);
                return Shown::File { text, lang: ext(path), first };
            }
        }
        "grep" | "glob" if t.ok => {
            let all = locations(&t.text);
            if !all.is_empty() {
                return Shown::Locations(all);
            }
        }
        "run_shell" | "command_output" | "run_steps" => return Shown::Console(t.text.clone()),
        _ => {}
    }
    match table(&t.text) {
        Some(rows) => Shown::Table(rows),
        None => Shown::Raw(t.text.clone()),
    }
}

/// The language a long argument is highlighted in: a command's the shell's, a file's content by its path.
pub fn lang_of(t: &Tool, key: &str) -> String {
    match key {
        "command" => "sh".into(),
        _ => t.arguments["path"]
            .as_str()
            .and_then(|p| p.rsplit_once('.'))
            .map(|(_, e)| e.to_string())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::Progress;

    fn call(name: &str, arguments: Value) -> Tool {
        Tool { name: name.into(), arguments, done: true, ok: true, ..Tool::default() }
    }

    #[test]
    fn a_call_says_where_it_stands_and_one_not_ended_in_a_turn_that_ended_did_not_run() {
        let waiting = Tool { status: "waiting".into(), ok: true, ..Tool::default() };
        assert_eq!(state(&waiting, true), State::Waiting);
        assert_eq!(status(&waiting, State::Waiting, 0), "Waiting for you");
        assert_eq!(state(&waiting, false), State::NotRun);
        let mut running = Tool { ok: true, ..Tool::default() };
        assert_eq!(status(&running, state(&running, true), 3), "Running · 3s");
        running.progress = Some(Progress { done: 2, total: 5, message: String::new() });
        assert_eq!(status(&running, State::Running, 3), "2 of 5 done");
        let denied = Tool { done: true, status: "denied".into(), ..Tool::default() };
        assert_eq!(status(&denied, state(&denied, true), 0), "Not allowed");
        let failed = Tool { done: true, status: "failed".into(), ..Tool::default() };
        assert_eq!(status(&failed, state(&failed, true), 0), "Failed");
        let read = Tool {
            text: "     1\ta\n     2\tb\n… 3 more lines: read on with offset 3.".into(),
            ms: 1400,
            ..call("read_file", json!({}))
        };
        assert_eq!(status(&read, state(&read, false), 0), "2 lines · 1.4s");
        assert_eq!(status(&call("plan", json!({})), State::Done, 0), "");
        assert_eq!((took(320), took(61_000)), ("320 ms".to_string(), "1m 1s".to_string()));
    }

    #[test]
    fn ended_calls_in_a_row_fold_into_a_line_but_a_task_or_one_still_running_has_its_card() {
        let mut tools: Vec<Tool> =
            ["read_file", "read_file", "grep", "read_file", "run_shell"].iter().map(|n| call(n, json!({}))).collect();
        assert_eq!(groups(&tools, false), [Group::Fold { start: 0, len: 5 }]);
        assert_eq!(fold_line(&tools), "Used 5 tools · read_file ×3, grep, run_shell");
        tools[2].name = "task".into();
        tools[4].done = false;
        assert_eq!(
            groups(&tools, true),
            [Group::Fold { start: 0, len: 2 }, Group::One(2), Group::One(3), Group::One(4)]
        );
    }

    #[test]
    fn a_read_or_an_edit_has_a_chip_of_its_file_and_lines() {
        let read = call("read_file", json!({"path": "src/a.ts", "offset": 12, "limit": 3}));
        assert_eq!(Chip::of(&read).map(|c| c.text()), Some("src/a.ts:12-14".into()));
        let whole = call("read_file", json!({"path": "src/a.ts"}));
        assert_eq!(Chip::of(&whole).map(|c| c.text()), Some("src/a.ts".into()));
        let mut edit = call("edit_file", json!({"path": "b.rs"}));
        let line = |op, old, new| crate::model::DiffLine { op, old, new, text: String::new() };
        edit.diff = Some(std::rc::Rc::new(crate::model::EditDiff {
            lines: vec![line(' ', 6, 6), line('-', 7, 0), line('+', 0, 7), line('+', 0, 8)],
            ..Default::default()
        }));
        assert_eq!(Chip::of(&edit).map(|c| c.text()), Some("b.rs:7-8".into()));
        assert_eq!(Chip::of(&call("grep", json!({"path": "x"}))), None);
    }

    #[test]
    fn arguments_are_listed_short_shown_as_code_long_and_the_rest_as_json() {
        let a = asked(&json!({"path": "a.rs", "replace_all": true, "new_string": "a\nb", "edits": [1]}));
        assert_eq!(a.short, [("path".into(), "a.rs".into()), ("replace_all".into(), "true".into())]);
        assert_eq!(a.code, [("new_string".to_string(), "a\nb".to_string())]);
        assert_eq!(a.rest.as_deref(), Some("{\n  \"edits\": [\n    1\n  ]\n}"));
        assert_eq!(asked(&Value::Null), Asked::default());
    }

    #[test]
    fn a_result_is_shown_by_what_it_holds() {
        let found = call("grep", json!({})).clone();
        let found = Tool { text: "src/a.rs:12: let x = 1;\nsrc/b.rs:3: x".into(), ..found };
        let Shown::Locations(l) = shown(&found) else { panic!("locations") };
        assert_eq!(l[0], Location { path: "src/a.rs".into(), line: 12, text: "let x = 1;".into() });
        assert_eq!(count(&found).as_deref(), Some("2 results"));
        let read = Tool { text: "    12\tfn a() {}\n".into(), ..call("read_file", json!({"path": "x.rs"})) };
        assert_eq!(shown(&read), Shown::File { text: "fn a() {}\n".into(), lang: "rs".into(), first: 12 });
        let rows =
            Tool { text: r#"[{"name": "b", "n": 10}, {"name": "a", "n": 9}]"#.into(), ..call("mcp__x", json!({})) };
        let Shown::Table(t) = shown(&rows) else { panic!("a table") };
        assert_eq!(t.columns, ["name", "n"]);
        assert_eq!(sorted(&t.rows, 1, false)[0], ["a", "9"]);
        assert_eq!(sorted(&t.rows, 0, true)[0], ["b", "10"]);
        assert!(matches!(shown(&call("run_shell", json!({}))), Shown::Console(_)));
        assert_eq!(shown(&Tool { text: "plain".into(), ..call("plan", json!({})) }), Shown::Raw("plain".into()));
        assert_eq!(main_argument(&call("x", json!({"query": "find me"}))), "find me");
    }
}
