//! The thread as the native window holds it, and what it says of a turn's calls.

use floem::reactive::{RwSignal, Scope};

#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub name: String,
    pub subject: String,
    pub done: bool,
    pub ok: bool,
    pub text: String,
}

#[derive(Clone)]
pub struct Item {
    pub id: u64,
    pub kind: Kind,
}

#[derive(Clone)]
pub enum Kind {
    User(String),
    Queued(String),
    /// The answer: its words (streamed), its calls, and whether its calls are shown.
    Assistant {
        text: RwSignal<String>,
        tools: RwSignal<Vec<Tool>>,
        open: RwSignal<bool>,
    },
    Notice(String),
    Error(String),
    Ask {
        id: u64,
        tool: String,
        subject: String,
        answer: RwSignal<Option<String>>,
    },
}

impl Kind {
    /// An answer, its signals made in `scope`: the window's own, which lasts, not the one current when a message is
    /// taken in (that one may go while the answer is still drawn).
    pub fn assistant(scope: Scope) -> Kind {
        Kind::Assistant {
            text: scope.create_rw_signal(String::new()),
            tools: scope.create_rw_signal(Vec::new()),
            open: scope.create_rw_signal(false),
        }
    }
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
        Tool { name: name.into(), subject: subject.into(), done: true, ok: true, text: String::new() }
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
}
