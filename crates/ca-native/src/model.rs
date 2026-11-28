//! The thread as the native window holds it, and what it says of a turn's calls.
//!
//! Every field the thread's parts are to show is here already, so a part fills and draws its own and none changes
//! the shape of an item: a request's details (`Question`), an answer's (`Answer`: its signals, one a part), a
//! call's (`Tool`), a queued message's number, the mark of a compaction. What the session does not say yet is
//! empty (`Default`): each is read from a message's JSON with `from_json`, which takes what is there.

// The parts to come read and fill what the window's first parts do not yet.
#![allow(dead_code)]

use std::rc::Rc;

use floem::reactive::{RwSignal, Scope};
use serde_json::Value;

/// Lines of a file: sent with a message, or what a call was about. The file, its first and last line (from 1; 0:
/// the whole file), and their text when it is kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Piece {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Piece {
    pub fn from_json(v: &Value) -> Piece {
        Piece {
            path: text_of(v, "path"),
            start: v["start"].as_u64().unwrap_or(0) as usize,
            end: v["end"].as_u64().unwrap_or(0) as usize,
            text: text_of(v, "text"),
        }
    }
}

/// A file attached to a message: Arena's id for it, its name and size, what it is ("image", "document"…), whether
/// it was cut to fit, and a small picture of it (a data URL) when it has one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub kind: String,
    pub cut: bool,
    pub thumb: String,
}

impl Attachment {
    pub fn from_json(v: &Value) -> Attachment {
        Attachment {
            id: text_of(v, "id"),
            name: text_of(v, "name"),
            size: v["size"].as_u64().unwrap_or(0),
            kind: text_of(v, "kind"),
            cut: v["cut"] == true,
            thumb: text_of(v, "thumb"),
        }
    }
}

/// A request: its words and what was sent with them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Question {
    /// Its words as the model was given them.
    pub text: String,
    /// What the person typed, apart from what was attached with @ (empty: the same as `text`).
    pub typed: String,
    /// Its number among the session's questions, from 1 (0: not known, a request the session has not numbered).
    pub q: u64,
    /// The part of an answer it replies to (empty: none).
    pub quote: String,
    /// The lines of files sent with it.
    pub files: Vec<Piece>,
    /// Sent while an answer was written, and read at its next step.
    pub taken_in: bool,
    /// Sent while an answer was written, too late for it (or a command, kept whole for its end): the question of a
    /// turn of its own, which the thread showed as queued when it was sent.
    pub late: bool,
    /// Said by the session itself as its next turn (a plan's review, a goal's next turn), not by the person.
    pub carried: bool,
    /// A note of Code Arena's own, not the person's words.
    pub note: bool,
    /// The skill it ran (empty: none).
    pub skill: String,
    pub attachments: Vec<Attachment>,
}

impl Question {
    /// A request of these words and nothing else.
    pub fn of(text: impl Into<String>) -> Question {
        Question { text: text.into(), ..Question::default() }
    }

    /// A request as the session says it (a `user` message, a kept message, a queued one): what is there is taken.
    pub fn from_json(v: &Value) -> Question {
        let list = |k: &str| v[k].as_array().cloned().unwrap_or_default();
        Question {
            text: text_of(v, "text"),
            typed: text_of(v, "typed"),
            q: v["q"].as_u64().unwrap_or(0),
            quote: text_of(v, "quote"),
            files: list("files").iter().map(Piece::from_json).collect(),
            taken_in: v["taken_in"] == true,
            late: v["late"] == true,
            carried: v["carried"] == true,
            note: v["note"] == true,
            skill: text_of(v, "skill"),
            attachments: list("attachments").iter().map(Attachment::from_json).collect(),
        }
    }
}

/// A line of an edit's diff: what became of it (' ' as it was, '+' added, '-' removed), its number in the old text
/// and in the new one (0: it is not in that one), and its text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiffLine {
    pub op: char,
    pub old: usize,
    pub new: usize,
    pub text: String,
}

/// What an edit changed in a file: how many lines it added and removed, whether it made the file, the changed
/// lines with those around them, and how many changed lines more are not listed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditDiff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub created: bool,
    pub lines: Vec<DiffLine>,
    pub more: usize,
}

impl EditDiff {
    /// As the session says it: `{path, added, removed, created, lines: [[op, old, new, text]], more}`.
    pub fn from_json(v: &Value) -> Option<EditDiff> {
        v.is_object().then(|| EditDiff {
            path: text_of(v, "path"),
            added: v["added"].as_u64().unwrap_or(0) as usize,
            removed: v["removed"].as_u64().unwrap_or(0) as usize,
            created: v["created"] == true,
            lines: v["lines"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|l| DiffLine {
                    op: l[0].as_str().and_then(|o| o.chars().next()).unwrap_or(' '),
                    old: l[1].as_u64().unwrap_or(0) as usize,
                    new: l[2].as_u64().unwrap_or(0) as usize,
                    text: l[3].as_str().unwrap_or("").to_string(),
                })
                .collect(),
            more: v["more"].as_u64().unwrap_or(0) as usize,
        })
    }
}

/// How far a call has got, where it says: so many done of so many, and its words.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub message: String,
}

/// A call of a tool in an answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tool {
    pub name: String,
    pub subject: String,
    pub done: bool,
    pub ok: bool,
    pub text: String,
    /// The call's id, as the session gave it (empty: a call the session did not number).
    pub id: String,
    /// What it was asked with (null: not said).
    pub arguments: Value,
    /// Where it stands, in the session's word ("waiting", "running", "denied", "failed", "skipped"; empty: not
    /// said, read `done` and `ok`).
    pub status: String,
    /// How long it took, in milliseconds (0: not said).
    pub ms: u64,
    /// Its result was cut to fit.
    pub cut: bool,
    /// What it changed in a file, when it is an edit.
    pub diff: Option<Rc<EditDiff>>,
    pub progress: Option<Progress>,
    /// When it began to run in this window (none: not running, or read back from the session).
    pub began: Option<std::time::Instant>,
    /// The files it made, as `.code-arena/media/NAME` (the session finds them in its whole result, else they are
    /// read from the result shown).
    pub media: Vec<String>,
}

impl Tool {
    /// A call as the session says it (`tool_start`, `tool_end`, a kept call): what is there is taken.
    pub fn from_json(v: &Value, done: bool) -> Tool {
        Tool {
            name: text_of(v, "name"),
            subject: text_of(v, "subject"),
            done,
            ok: v["ok"] != false,
            text: text_of(v, "text"),
            id: text_of(v, "id"),
            arguments: v["arguments"].clone(),
            status: text_of(v, "status"),
            ms: v["ms"].as_u64().unwrap_or(0),
            cut: v["cut"] == true,
            diff: EditDiff::from_json(&v["diff"]).map(Rc::new),
            progress: None,
            began: None,
            media: match v["media"].as_array() {
                Some(all) => all.iter().filter_map(|m| m.as_str().map(String::from)).collect(),
                None => crate::agent::media::found(v["text"].as_str().unwrap_or("")),
            },
        }
    }
}

/// How long the model thought before it answered, and whether its thinking was cut short.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Thought {
    pub ms: u64,
    pub cut_short: bool,
}

/// What a turn used: the tokens read (and how many of them from the cache) and written, how long it took, and its
/// cost when it is known.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub prompt: u64,
    pub cached: u64,
    pub completion: u64,
    pub ms: u64,
    pub cost: Option<f64>,
}

impl Usage {
    /// As the session says it: `{prompt, cached, completion, ms, cost?}`; none when it says none.
    pub fn from_json(v: &Value) -> Option<Usage> {
        v.is_object().then(|| Usage {
            prompt: v["prompt"].as_u64().unwrap_or(0),
            cached: v["cached"].as_u64().unwrap_or(0),
            completion: v["completion"].as_u64().unwrap_or(0),
            ms: v["ms"].as_u64().unwrap_or(0),
            cost: v["cost"].as_f64(),
        })
    }
}

/// An answer: each of its parts a signal, so a part that changes (a word streamed, a call ended) draws that part
/// again and nothing else.
#[derive(Clone, Copy)]
pub struct Answer {
    /// Its words (streamed), its calls, and whether its calls are shown.
    pub text: RwSignal<String>,
    pub tools: RwSignal<Vec<Tool>>,
    pub open: RwSignal<bool>,
    /// The question it answers, by its number (0: not known).
    pub q: RwSignal<u64>,
    /// The model's thinking, as it comes.
    pub reasoning: RwSignal<String>,
    /// How long it thought (none: it thinks still, or did not).
    pub thought: RwSignal<Option<Thought>>,
    /// What its turn used (none until the turn ends).
    pub usage: RwSignal<Option<Usage>>,
    /// The model that wrote it (empty: not said).
    pub model: RwSignal<String>,
    /// The person's rating of it: up (true), down (false), none.
    pub rating: RwSignal<Option<bool>>,
    /// What Code Arena said while it was written (a step skipped, the context compacted), shown with it.
    pub notices: RwSignal<Vec<String>>,
}

#[derive(Clone)]
pub struct Item {
    pub id: u64,
    pub kind: Kind,
}

#[derive(Clone)]
pub enum Kind {
    User(Question),
    /// A message sent while an answer is written, waiting to be read: its number as the session gave it (echoed
    /// as it is to change or cancel it; null: the session did not number it), the message, and whether it waits
    /// whole for the answer's end (a command), not read by the turn at its next step.
    Queued {
        id: Value,
        question: Question,
        later: bool,
    },
    Assistant(Answer),
    Notice(String),
    Error(String),
    Ask {
        id: u64,
        tool: String,
        subject: String,
        answer: RwSignal<Option<String>>,
    },
    /// Where the chat was compacted: after which question, the summary the model goes on with, and whether the
    /// summary is shown.
    Compaction {
        after_q: u64,
        summary: String,
        open: RwSignal<bool>,
    },
}

impl Kind {
    /// An answer, its signals made in `scope`: the window's own, which lasts, not the one current when a message is
    /// taken in (that one may go while the answer is still drawn).
    pub fn assistant(scope: Scope) -> Kind {
        Kind::Assistant(Answer {
            text: scope.create_rw_signal(String::new()),
            tools: scope.create_rw_signal(Vec::new()),
            open: scope.create_rw_signal(false),
            q: scope.create_rw_signal(0),
            reasoning: scope.create_rw_signal(String::new()),
            thought: scope.create_rw_signal(None),
            usage: scope.create_rw_signal(None),
            model: scope.create_rw_signal(String::new()),
            rating: scope.create_rw_signal(None),
            notices: scope.create_rw_signal(Vec::new()),
        })
    }

    /// The mark of a compaction, its signal made in `scope` (the window's own).
    pub fn compaction(scope: Scope, after_q: u64, summary: impl Into<String>) -> Kind {
        Kind::Compaction { after_q, summary: summary.into(), open: scope.create_rw_signal(false) }
    }
}

/// A text of a message's JSON (none: an empty one).
fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_string()
}

/// A turn's calls in a line, as the page says them: "Read 4 files, ran 2 commands, edited cart.rs".
pub fn summary(tools: &[Tool]) -> String {
    let count = |names: &[&str]| tools.iter().filter(|t| names.contains(&t.name.as_str())).count();
    let plural = |n: usize, one: &str, many: &str| if n == 1 { one.to_string() } else { format!("{n} {many}") };
    let mut parts = Vec::new();
    let read = count(&["read_file"]);
    if read > 0 {
        parts.push(format!("read {}", plural(read, "1 file", "files")));
    }
    let searched = count(&["grep", "glob", "list_dir", "search_sessions", "session_search"]);
    if searched > 0 {
        parts.push(format!("searched {}", plural(searched, "once", "times")));
    }
    let ran = count(&["run_shell"]);
    if ran > 0 {
        parts.push(format!("ran {}", plural(ran, "1 command", "commands")));
    }
    let mut edited: Vec<&str> = tools
        .iter()
        .filter(|t| t.name == "edit_file" || t.name == "write_file")
        .map(|t| t.subject.rsplit('/').next().unwrap_or(&t.subject))
        .collect();
    edited.dedup();
    if !edited.is_empty() {
        parts.push(if edited.len() > 2 {
            format!("edited {} files", edited.len())
        } else {
            format!("edited {}", edited.join(", "))
        });
    }
    let known = [
        "read_file",
        "grep",
        "glob",
        "list_dir",
        "search_sessions",
        "session_search",
        "run_shell",
        "edit_file",
        "write_file",
    ];
    let other = tools.iter().filter(|t| !known.contains(&t.name.as_str())).count();
    if other > 0 {
        parts.push(format!("{} other call{}", other, if other == 1 { "" } else { "s" }));
    }
    let failed = tools.iter().filter(|t| t.done && !t.ok).count();
    let mut line = parts.join(", ");
    if let Some(first) = line.get(..1) {
        line = first.to_uppercase() + &line[1..];
    }
    if failed > 0 {
        line.push_str(&format!(" ({failed} failed)"));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(name: &str, subject: &str) -> Tool {
        Tool { name: name.into(), subject: subject.into(), done: true, ok: true, ..Tool::default() }
    }

    #[test]
    fn a_turns_calls_in_a_line() {
        assert_eq!(
            summary(&[t("read_file", "a"), t("read_file", "b"), t("run_shell", "ls")]),
            "Read files, ran 1 command".replace("files", "2 files")
        );
        assert_eq!(summary(&[t("edit_file", "src/cart.rs")]), "Edited cart.rs");
        let mut failed = t("run_shell", "x");
        failed.ok = false;
        assert_eq!(summary(&[failed, t("task", "")]), "Ran 1 command, 1 other call (1 failed)");
    }

    #[test]
    fn a_request_and_a_call_are_read_from_what_the_session_says_and_what_it_does_not_say_is_empty() {
        use serde_json::json;
        // As the session says a request today: its words only.
        assert_eq!(Question::from_json(&json!({"type": "user", "text": "hello"})), Question::of("hello"));
        let q = Question::from_json(&json!({
            "text": "@src/a.rs explain", "typed": "explain", "q": 3, "quote": "the part", "taken_in": true,
            "skill": "review", "files": [{"path": "src/a.rs", "start": 2, "end": 4}],
            "attachments": [{"id": "f1", "name": "shot.png", "size": 2048, "kind": "image", "cut": false}],
        }));
        assert_eq!(
            (q.q, q.typed.as_str(), q.quote.as_str(), q.taken_in, q.note),
            (3, "explain", "the part", true, false)
        );
        assert_eq!(q.files, [Piece { path: "src/a.rs".into(), start: 2, end: 4, text: String::new() }]);
        assert_eq!(q.attachments[0].name, "shot.png");
        assert_eq!(q.skill, "review");
        // A request that waited for the answer's end, and one the session said itself.
        let late = Question::from_json(&json!({"type": "user", "text": "/explain it", "q": 2, "late": true}));
        assert!(late.late && !late.carried && !q.late);
        assert!(Question::from_json(&json!({"type": "user", "text": "go on", "q": 3, "carried": true})).carried);
        // A call as it is said today, and with all it is to carry.
        let plain = Tool::from_json(&json!({"type": "tool_start", "name": "run_shell", "subject": "ls"}), false);
        assert_eq!(plain, Tool { name: "run_shell".into(), subject: "ls".into(), ok: true, ..Tool::default() });
        let full = Tool::from_json(
            &json!({
                "id": "call_1", "name": "edit_file", "subject": "a.rs", "ok": true, "text": "done", "ms": 12,
                "cut": true, "status": "running", "arguments": {"path": "a.rs"},
                "diff": {"path": "a.rs", "added": 1, "removed": 1, "created": false, "more": 2,
                         "lines": [["-", 3, 0, "old"], ["+", 0, 3, "new"], [" ", 4, 4, "same"]]},
            }),
            true,
        );
        assert_eq!((full.id.as_str(), full.ms, full.cut, full.done), ("call_1", 12, true, true));
        assert_eq!(full.arguments["path"], "a.rs");
        let diff = full.diff.expect("a diff");
        assert_eq!((diff.added, diff.removed, diff.more, diff.lines.len()), (1, 1, 2, 3));
        assert_eq!(diff.lines[1], DiffLine { op: '+', old: 0, new: 3, text: "new".into() });
        assert_eq!(EditDiff::from_json(&json!(null)), None);
        assert_eq!(Usage::from_json(&json!(null)), None);
        let used = Usage::from_json(&json!({"prompt": 8600, "cached": 6278, "completion": 1200, "ms": 12000}));
        assert_eq!(used, Some(Usage { prompt: 8600, cached: 6278, completion: 1200, ms: 12000, cost: None }));
    }
}
