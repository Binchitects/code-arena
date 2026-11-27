//! What an edit changed, for the chat's "Changes to PATH" under the call and its answer's "N files changed". A
//! `write_file` or `edit_file` call's file is read as it begins and again as it ends, and `tool_end` carries the
//! diff between the two: `diff{path, added, removed, created, lines: [[op, old, new, text]], more}`. A file the call
//! made was not there before it (`created`, every line added). An edit a command made (`run_shell`) has no diff:
//! nothing says which files a command will write before it runs.
//!
//! The diffs of this run are kept by session and call id for a face that asks for the thread again, and at a
//! turn's end with each call's result message in the session (its `details`), for the thread read back later.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use ca_agent::Call;
use ca_tools::Output;
use serde_json::{Value, json};
use similar::{ChangeTag, TextDiff};

use crate::ide_ext::Shared;

/// The lines around a change that are shown with it, as VS Code's inline diffs show them.
const AROUND: usize = 3;

/// The most changed lines a diff lists; past them it says how many more there are.
const MOST_CHANGED: usize = 200;

/// A file bigger than this is not compared.
const MOST_BYTES: u64 = 2 * 1024 * 1024;

/// How long a comparison may take before it settles for a coarser diff.
const DEADLINE: Duration = Duration::from_millis(200);

/// The calls that write one file named by their `path`.
fn is_edit(name: &str) -> bool {
    matches!(name, "write_file" | "edit_file")
}

/// The diffs of this run, and the files of the calls that run now as they were before them.
#[derive(Default)]
pub struct Diffs(Mutex<Held>);

#[derive(Default)]
struct Held {
    /// By session and call id: the file, its path as shown, and its text before the call (none: not there).
    before: HashMap<(String, String), (PathBuf, String, Option<String>)>,
    /// By session and call id: the diff the call's end carried.
    done: HashMap<(String, String), Value>,
}

impl Diffs {
    /// The most diffs remembered: past it the oldest run's are forgotten with the rest.
    const MOST: usize = 2000;

    /// A call begins: its file as it is now is kept, when it is an edit of a file of the workspace.
    pub fn begin(sh: &Shared, call: &Call<'_>) {
        if !is_edit(call.name) {
            return;
        }
        let Some(path) = call.arguments["path"].as_str() else { return };
        let Ok(real) = sh.workspace.resolve(path) else { return };
        let shown = real
            .strip_prefix(sh.workspace.root())
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string());
        let before = match std::fs::metadata(&real) {
            Ok(m) if m.len() > MOST_BYTES => return,
            Ok(_) => match std::fs::read(&real).map(String::from_utf8) {
                Ok(Ok(text)) => Some(text),
                // Not text: not compared.
                _ => return,
            },
            Err(_) => None,
        };
        let me = sh.kept.of::<Diffs>();
        if let Ok(mut g) = me.0.lock() {
            g.before.insert((sh.session.clone(), call.id.to_string()), (real, shown, before));
        }
    }

    /// A call ended: the diff of what it wrote, when it was an edit that wrote its file. Kept for the thread.
    pub fn end(sh: &Shared, call: &Call<'_>, output: &Output) -> Option<Value> {
        let me = sh.kept.of::<Diffs>();
        let key = (sh.session.clone(), call.id.to_string());
        let (real, shown, before) = me.0.lock().ok()?.before.remove(&key)?;
        if output.is_error || !output.wrote.contains(&real) {
            return None;
        }
        let meta = std::fs::metadata(&real).ok()?;
        if meta.len() > MOST_BYTES {
            return None;
        }
        let after = String::from_utf8(std::fs::read(&real).ok()?).ok()?;
        let diff = edit_diff(&shown, before.as_deref(), &after);
        if let Ok(mut g) = me.0.lock() {
            if g.done.len() >= Self::MOST {
                g.done.clear();
            }
            g.done.insert(key, diff.clone());
        }
        Some(diff)
    }

    /// The diff a call of this run carried.
    pub fn of(sh: &Shared, session: &str, id: &str) -> Option<Value> {
        let me = sh.kept.of::<Diffs>();
        let g = me.0.lock().ok()?;
        g.done.get(&(session.to_string(), id.to_string())).cloned()
    }

    /// The turn ended: each diff of its calls is kept with the call's result in the session.
    pub fn keep(sh: &Shared, store: &Mutex<ca_session::Store>) {
        let me = sh.kept.of::<Diffs>();
        // A call the turn's stop cut off never ended: its file's text before it goes.
        if let Ok(mut g) = me.0.lock() {
            g.before.retain(|(session, _), _| *session != sh.session);
        }
        let Ok(store) = store.lock() else { return };
        let Ok(page) = store.page(&sh.session, 1, None) else { return };
        for m in &page.messages {
            let Some(id) = m.message.tool_call_id.as_deref() else { continue };
            let diff = me.0.lock().ok().and_then(|g| g.done.get(&(sh.session.clone(), id.to_string())).cloned());
            if let Some(diff) = diff {
                let mut fields = serde_json::Map::new();
                fields.insert("diff".into(), diff);
                let _ = store.merge_details(&sh.session, m.seq, &fields);
            }
        }
    }
}

/// A kept step's calls given their diffs: each from its result's details in the session, else from this run.
pub fn add_kept(
    calls: &mut [Value],
    after: &[ca_session::Stored],
    details: &HashMap<i64, Value>,
    of_run: &dyn Fn(&str) -> Option<Value>,
) {
    for c in calls {
        let id = c["id"].as_str().unwrap_or("").to_string();
        let result = after.iter().find(|m| m.message.tool_call_id.as_deref() == Some(id.as_str()));
        let kept = result.and_then(|r| details.get(&r.seq)).map(|d| d["diff"].clone()).filter(Value::is_object);
        if let Some(diff) = kept.or_else(|| of_run(&id)) {
            c["diff"] = diff;
        }
    }
}

/// What changed from `before` (none: the file was not there) to `after`, as `tool_end` carries it: the changed
/// lines with three around them, each `[op, old, new, text]` (op ' ', '-' or '+'; a line's number in the old and
/// the new text, 0 when it is not in that one), and how many changed lines more are not listed.
pub fn edit_diff(path: &str, before: Option<&str>, after: &str) -> Value {
    let old = before.unwrap_or("");
    let diff = TextDiff::configure().timeout(DEADLINE).diff_lines(old, after);
    let (mut added, mut removed, mut listed, mut more) = (0usize, 0usize, 0usize, 0usize);
    let mut lines: Vec<Value> = Vec::new();
    for group in diff.grouped_ops(AROUND) {
        for op in &group {
            for change in diff.iter_changes(op) {
                let tag = change.tag();
                match tag {
                    ChangeTag::Insert => added += 1,
                    ChangeTag::Delete => removed += 1,
                    ChangeTag::Equal => {}
                }
                if listed >= MOST_CHANGED {
                    if tag != ChangeTag::Equal {
                        more += 1;
                    }
                    continue;
                }
                if tag != ChangeTag::Equal {
                    listed += 1;
                }
                let op = match tag {
                    ChangeTag::Equal => " ",
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                };
                let number = |i: Option<usize>| i.map(|i| i + 1).unwrap_or(0);
                let text = change.value().trim_end_matches(['\n', '\r']);
                lines.push(json!([op, number(change.old_index()), number(change.new_index()), text]));
            }
        }
    }
    // The lines after the last listed change that are only its context: not shown once the list is cut.
    if more > 0 {
        while lines.last().is_some_and(|l| l[0] == " ") {
            lines.pop();
        }
    }
    json!({"path": path, "added": added, "removed": removed, "created": before.is_none(), "lines": lines,
           "more": more})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(v: &Value) -> String {
        v["lines"].as_array().expect("lines").iter().map(|l| l[0].as_str().expect("an op")).collect()
    }

    #[test]
    fn an_edit_lists_its_changed_lines_with_three_around_them_numbered_in_both_texts() {
        let before: String = (1..=12).map(|n| format!("line {n}\n")).collect();
        let after = before.replace("line 6\n", "six\n");
        let d = edit_diff("src/a.rs", Some(&before), &after);
        assert_eq!(
            (d["added"].as_u64(), d["removed"].as_u64(), d["created"].as_bool()),
            (Some(1), Some(1), Some(false))
        );
        assert_eq!(ops(&d), "   -+   ");
        assert_eq!(d["lines"][0], json!([" ", 3, 3, "line 3"]));
        assert_eq!(d["lines"][3], json!(["-", 6, 0, "line 6"]));
        assert_eq!(d["lines"][4], json!(["+", 0, 6, "six"]));
        assert_eq!(d["more"], 0);
    }

    #[test]
    fn changes_far_apart_are_two_groups_with_a_jump_in_their_numbers() {
        let before: String = (1..=30).map(|n| format!("{n}\n")).collect();
        let after = before.replace("\n2\n", "\ntwo\n").replace("\n28\n", "\ntwenty-eight\n");
        let d = edit_diff("a", Some(&before), &after);
        let news: Vec<u64> =
            d["lines"].as_array().expect("lines").iter().map(|l| l[2].as_u64().expect("a number")).collect();
        assert!(news.windows(2).any(|w| w[1] > w[0] + 1), "{news:?}");
        assert_eq!(d["added"], 2);
    }

    #[test]
    fn a_file_made_by_the_call_is_created_and_every_line_is_added() {
        let d = edit_diff("new.txt", None, "a\nb\n");
        assert_eq!(d["created"], true);
        assert_eq!(ops(&d), "++");
        assert_eq!(d["lines"][1], json!(["+", 0, 2, "b"]));
        assert_eq!(d["removed"], 0);
    }

    #[test]
    fn a_long_change_lists_its_first_lines_and_says_how_many_more() {
        let after: String = (0..250).map(|n| format!("{n}\n")).collect();
        let d = edit_diff("big", Some(""), &after);
        assert_eq!(d["lines"].as_array().expect("lines").len(), MOST_CHANGED);
        assert_eq!(d["more"], 50);
        assert_eq!(d["added"], 250);
        assert_eq!(d["created"], false);
    }

    #[test]
    fn a_kept_call_takes_its_diff_from_its_results_details_else_from_the_run() {
        let result =
            |id: &str, seq: i64| ca_session::Stored { seq, turn: 1, message: ca_llm::Message::tool(id, "done") };
        let mut calls = vec![json!({"id": "a"}), json!({"id": "b"}), json!({"id": "c"})];
        let details = HashMap::from([(5, json!({"diff": {"path": "x"}}))]);
        let run = |id: &str| (id == "b").then(|| json!({"path": "y"}));
        add_kept(&mut calls, &[result("a", 5), result("b", 6)], &details, &run);
        assert_eq!(calls[0]["diff"]["path"], "x");
        assert_eq!(calls[1]["diff"]["path"], "y");
        assert!(calls[2]["diff"].is_null());
    }
}
