//! The Explorer's decorations, as VS Code's: a file's name in the colour of what happened to it, with its letters at
//! the row's end. The agent's changes say A or M; git's say M, U, A, R, D or C (`git.decorations.enabled`), and what
//! the repository ignores is dimmed; a file with errors or warnings is red or yellow with their count
//! (`problems.decorations.enabled`). A folder takes the colour of what is in it, and a closed one shows a dot.
//! `explorer.decorations.colors` and `explorer.decorations.badges` turn the colours and the letters off. The row's
//! tooltip says all of it.
//!
//! Everything is worked out once, in a memo of the window, when the status, the changes, the problems or one of the
//! settings is another; a row reads only its own mark, so it is drawn again only when that changes.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use floem::peniko::Color;
use floem::reactive::{Memo, SignalGet, SignalWith};
use serde_json::Value;

use crate::colors::Colors;
use crate::decor::Severity;
use crate::scm::{Group, Scm, Status};
use crate::state::{Send, Ui};

/// What colours a row, the stronger after the weaker: a folder takes the strongest of what is in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tint {
    Ignored,
    StageModified,
    Modified,
    StageDeleted,
    Deleted,
    Renamed,
    Added,
    Untracked,
    AgentModified,
    AgentAdded,
    Conflict,
    Warning,
    Error,
}

impl Tint {
    pub fn colour(self, c: &Colors) -> Color {
        match self {
            Tint::Ignored => c.scm_ignored,
            Tint::StageModified => c.scm_stage_modified,
            Tint::Modified | Tint::AgentModified => c.scm_modified,
            Tint::StageDeleted => c.scm_stage_deleted,
            Tint::Deleted => c.removed_fg,
            Tint::Renamed => c.scm_renamed,
            Tint::Added | Tint::AgentAdded => c.added_fg,
            Tint::Untracked => c.scm_untracked,
            Tint::Conflict => c.scm_conflict,
            Tint::Warning => c.list_warning_fg,
            Tint::Error => c.list_error_fg,
        }
    }
}

/// How a row is decorated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mark {
    /// The letters at a file's end: its problems' count first, then the agent's letter and git's ("2, M").
    pub badge: String,
    pub tint: Option<Tint>,
    /// A folder has something decorated in it: its row shows a dot while it is closed.
    pub dot: bool,
    /// What the tooltip says after the path, each part on its own.
    pub tips: Vec<String>,
}

/// What the decorations are made of: each part as its owner says it.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// git's files, with their letter and their colour, as the Source Control view groups them.
    pub git: Vec<(String, char, Tint, &'static str)>,
    /// What the repository ignores (a folder ignored whole ends in a slash).
    pub ignored: Vec<String>,
    /// The files the agent changed, with their letter (A, M or D).
    pub agent: Vec<(String, &'static str)>,
    /// Each problem's file and severity.
    pub problems: Vec<(String, Severity)>,
}

/// Which decorations are on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flags {
    pub git: bool,
    pub problems: bool,
    pub colors: bool,
    pub badges: bool,
}

impl Default for Flags {
    fn default() -> Self {
        Flags { git: true, problems: true, colors: true, badges: true }
    }
}

/// Every row's decoration, by path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks {
    marks: HashMap<String, Mark>,
    /// The paths ignored (a folder's without its slash): what is in them is ignored too.
    ignored: HashSet<String>,
    flags: Flags,
}

impl Marks {
    /// A row's decoration: its own, or the one of what the repository ignores.
    pub fn of(&self, path: &str) -> Mark {
        let mut m = self.marks.get(path).cloned().unwrap_or_default();
        if self.is_ignored(path) {
            m.tips.push("Ignored".into());
            if self.flags.colors && m.tint.is_none_or(|t| t < Tint::Warning) {
                m.tint = Some(Tint::Ignored);
            }
        }
        m
    }

    fn is_ignored(&self, path: &str) -> bool {
        if self.ignored.is_empty() {
            return false;
        }
        let mut at = path;
        loop {
            if self.ignored.contains(at) {
                return true;
            }
            match at.rfind('/') {
                Some(i) => at = &at[..i],
                None => return false,
            }
        }
    }
}

/// What a file has, before it is said.
#[derive(Default)]
struct Has {
    errors: usize,
    warnings: usize,
    agent: Option<&'static str>,
    git: Option<(char, Tint, &'static str)>,
}

/// What is in a folder, before it is said.
#[derive(Default)]
struct Within {
    errors: usize,
    warnings: usize,
    agent: usize,
    git: usize,
    tint: Option<Tint>,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// A count as VS Code writes it in a badge.
fn count(n: usize) -> String {
    if n > 9 { "9+".into() } else { n.to_string() }
}

/// Each row's decoration from what its owners say, with the decorations that are on.
pub fn marks(f: &Facts, flags: Flags) -> Marks {
    let mut files: HashMap<&str, Has> = HashMap::new();
    if flags.problems {
        for (p, s) in &f.problems {
            let h = files.entry(p.as_str()).or_default();
            match s {
                Severity::Error => h.errors += 1,
                Severity::Warning => h.warnings += 1,
                _ => {}
            }
        }
        files.retain(|_, h| h.errors + h.warnings > 0);
    }
    for (p, letter) in &f.agent {
        files.entry(p.as_str()).or_default().agent = Some(letter);
    }
    if flags.git {
        for (p, letter, tint, says) in &f.git {
            files.entry(p.as_str()).or_default().git = Some((*letter, *tint, says));
        }
    }
    let mut marks = HashMap::new();
    let mut folders: HashMap<String, Within> = HashMap::new();
    for (path, h) in &files {
        let mut letters: Vec<String> = Vec::new();
        let mut tips = Vec::new();
        let problems = h.errors + h.warnings;
        if problems > 0 {
            letters.push(count(problems));
            tips.push(format!("{} in this file", plural(problems, "problem", "problems")));
        }
        if let Some(a) = h.agent {
            letters.push(a.to_string());
            tips.push(
                match a {
                    "A" => "Created by the agent",
                    "D" => "Deleted by the agent",
                    _ => "Changed by the agent",
                }
                .into(),
            );
        }
        if let Some((letter, _, says)) = h.git {
            if !letters.iter().any(|l| l.len() == 1 && l.starts_with(letter)) {
                letters.push(letter.to_string());
            }
            tips.push(says.to_string());
        }
        let agent_tint = h.agent.map(|a| if a == "A" { Tint::AgentAdded } else { Tint::AgentModified });
        let tint = [
            (h.errors > 0).then_some(Tint::Error),
            (h.warnings > 0).then_some(Tint::Warning),
            agent_tint,
            h.git.map(|g| g.1),
        ]
        .into_iter()
        .flatten()
        .max();
        let mut at: &str = path;
        while let Some(i) = at.rfind('/') {
            at = &at[..i];
            let w = folders.entry(at.to_string()).or_default();
            w.errors += h.errors;
            w.warnings += h.warnings;
            w.agent += h.agent.is_some() as usize;
            w.git += h.git.is_some() as usize;
            w.tint = w.tint.max(tint);
        }
        let mark = Mark {
            badge: if flags.badges { letters.join(", ") } else { String::new() },
            tint: tint.filter(|_| flags.colors),
            dot: false,
            tips,
        };
        marks.insert(path.to_string(), mark);
    }
    for (path, w) in folders {
        let mut tips = Vec::new();
        if w.errors + w.warnings > 0 {
            let mut kinds = Vec::new();
            if w.errors > 0 {
                kinds.push(plural(w.errors, "error", "errors"));
            }
            if w.warnings > 0 {
                kinds.push(plural(w.warnings, "warning", "warnings"));
            }
            tips.push(format!("Contains {}", kinds.join(" and ")));
        }
        if w.agent > 0 {
            tips.push(format!("Contains {} the agent changed", plural(w.agent, "file", "files")));
        }
        if w.git > 0 {
            tips.push(format!("Contains {}", plural(w.git, "changed file", "changed files")));
        }
        let mark = Mark { badge: String::new(), tint: w.tint.filter(|_| flags.colors), dot: flags.badges, tips };
        marks.insert(path, mark);
    }
    let ignored = if flags.git {
        f.ignored.iter().map(|p| p.trim_end_matches('/').to_string()).filter(|p| !p.is_empty()).collect()
    } else {
        HashSet::new()
    };
    Marks { marks, ignored, flags }
}

/// git's letter, colour and words for a file of the status, as VS Code's git decorations say them; a file in more
/// than one group is said as its last (the merge's over the working tree's over the index's).
pub fn git_facts(s: &Status) -> Vec<(String, char, Tint, &'static str)> {
    let mut out = Vec::new();
    for g in [Group::Staged, Group::Changes, Group::Merge] {
        for f in s.of_group(g) {
            let letter = g.letter(f);
            let (tint, says) = match (g, letter) {
                (Group::Merge, _) => (
                    Tint::Conflict,
                    match (f.x, f.y) {
                        ('U', 'U') => "Conflict: Both Modified",
                        ('A', 'A') => "Conflict: Both Added",
                        ('D', 'D') => "Conflict: Both Deleted",
                        ('A', 'U') => "Conflict: Added By Us",
                        ('U', 'A') => "Conflict: Added By Them",
                        ('D', 'U') => "Conflict: Deleted By Us",
                        _ => "Conflict: Deleted By Them",
                    },
                ),
                (Group::Staged, 'A') => (Tint::Added, "Index Added"),
                (Group::Staged, 'D') => (Tint::StageDeleted, "Index Deleted"),
                (Group::Staged, 'R') => (Tint::Renamed, "Index Renamed"),
                (Group::Staged, 'C') => (Tint::Added, "Index Copied"),
                (Group::Staged, _) => (Tint::StageModified, "Index Modified"),
                (_, 'U') => (Tint::Untracked, "Untracked"),
                (_, 'D') => (Tint::Deleted, "Deleted"),
                (_, 'A') => (Tint::Added, "Intent to Add"),
                (_, 'R') => (Tint::Renamed, "Renamed"),
                _ => (Tint::Modified, "Modified"),
            };
            out.push((f.path.clone(), letter, tint, says));
        }
    }
    out
}

/// The Explorer's decorations, one memo per window.
#[derive(Clone, Copy)]
pub struct Decor {
    pub marks: Memo<Rc<Marks>>,
}

impl crate::ext::State for Decor {
    fn new(ui: Ui) -> Self {
        let on = move |key: &str| crate::settings::setting(ui, key, None).as_bool().unwrap_or(true);
        let marks = ui.scope.create_memo(move |_| {
            let flags = Flags {
                git: on("git.decorations.enabled"),
                problems: on("problems.decorations.enabled"),
                colors: on("explorer.decorations.colors"),
                badges: on("explorer.decorations.badges"),
            };
            let scm = ui.state::<Scm>();
            let (git, ignored) = scm.status.with(|s| match s {
                Some(s) if flags.git => (git_facts(s), s.ignored.clone()),
                _ => (Vec::new(), Vec::new()),
            });
            let agent = ui.changes.with(|cs| cs.iter().map(|c| (c.path.clone(), c.letter())).collect());
            let problems = if flags.problems { crate::problems::severities(ui) } else { Vec::new() };
            Rc::new(marks(&Facts { git, ignored, agent, problems }, flags))
        });
        Decor { marks }
    }
}

/// A row's decoration, behind a memo: the row is drawn again only when its own mark changes. Made in the row's
/// view, so it goes with it.
pub fn mark_of(ui: Ui, path: String) -> Memo<Mark> {
    let marks = ui.state::<Decor>().marks;
    floem::reactive::create_memo(move |_| marks.with(|m| m.of(&path)))
}

/// What a row's tooltip says: its path, then each of its decorations.
pub fn tooltip(path: &str, m: &Mark) -> String {
    std::iter::once(path).chain(m.tips.iter().map(String::as_str)).collect::<Vec<_>>().join(" \u{2022} ")
}

/// The colour of a row's name and letters, when it has one.
pub fn colour(ui: Ui, m: &Mark) -> Option<Color> {
    let c = ui.colors.get();
    m.tint.map(|t| t.colour(&c))
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scm::File;

    fn file(path: &str, x: char, y: char) -> File {
        File {
            path: path.into(),
            from: None,
            x,
            y,
            staged: x != ' ' && x != '?' && x != 'U',
            changed: y != ' ' && y != '?',
            untracked: x == '?',
            conflict: x == 'U' || y == 'U',
        }
    }

    fn facts(files: Vec<File>) -> Facts {
        Facts { git: git_facts(&Status { files, ..Status::default() }), ..Facts::default() }
    }

    #[test]
    fn git_letters_and_words_follow_vs_code_and_the_working_tree_is_said_over_the_index() {
        let f = facts(vec![file("a.rs", ' ', 'M'), file("n.rs", '?', '?'), file("s.rs", 'M', 'M'), file("i.rs", 'A', ' ')]);
        let m = marks(&f, Flags::default());
        assert_eq!((m.of("a.rs").badge, m.of("a.rs").tint), ("M".to_string(), Some(Tint::Modified)));
        assert_eq!((m.of("n.rs").badge, m.of("n.rs").tips), ("U".to_string(), vec!["Untracked".to_string()]));
        assert_eq!(m.of("s.rs").tips, ["Modified"]);
        assert_eq!((m.of("i.rs").badge, m.of("i.rs").tint), ("A".to_string(), Some(Tint::Added)));
        let c = marks(&facts(vec![file("c.rs", 'U', 'U')]), Flags::default()).of("c.rs");
        assert_eq!((c.badge.as_str(), c.tint, c.tips[0].as_str()), ("C", Some(Tint::Conflict), "Conflict: Both Modified"));
    }

    #[test]
    fn a_folder_takes_the_strongest_colour_in_it_and_a_dot_and_says_what_it_contains() {
        let mut f = facts(vec![file("src/ui/a.rs", ' ', 'M')]);
        f.problems = vec![("src/b.rs".into(), Severity::Error), ("src/b.rs".into(), Severity::Warning), ("src/b.rs".into(), Severity::Info)];
        f.agent = vec![("src/ui/a.rs".into(), "M")];
        let m = marks(&f, Flags::default());
        let b = m.of("src/b.rs");
        assert_eq!((b.badge.as_str(), b.tint), ("2", Some(Tint::Error)));
        assert_eq!(b.tips, ["2 problems in this file"]);
        // The agent's M and git's M are one letter; both are said.
        let a = m.of("src/ui/a.rs");
        assert_eq!((a.badge.as_str(), a.tint), ("M", Some(Tint::AgentModified)));
        assert_eq!(a.tips, ["Changed by the agent", "Modified"]);
        let src = m.of("src");
        assert_eq!((src.dot, src.tint, src.badge.as_str()), (true, Some(Tint::Error), ""));
        assert_eq!(src.tips, ["Contains 1 error and 1 warning", "Contains 1 file the agent changed", "Contains 1 changed file"]);
        assert_eq!(m.of("src/ui").tint, Some(Tint::AgentModified));
        assert_eq!(m.of("other"), Mark::default());
    }

    #[test]
    fn what_the_repository_ignores_is_dimmed_with_all_in_it_and_said() {
        let f = Facts { ignored: vec!["target/".into(), "src/run.log".into()], ..Facts::default() };
        let m = marks(&f, Flags::default());
        for p in ["target", "target/debug/x", "src/run.log"] {
            assert_eq!((m.of(p).tint, m.of(p).tips), (Some(Tint::Ignored), vec!["Ignored".to_string()]), "{p}");
        }
        assert_eq!(m.of("src").tint, None);
        assert_eq!(m.of("targets").tint, None);
        assert_eq!(marks(&f, Flags { git: false, ..Flags::default() }).of("target"), Mark::default());
    }

    #[test]
    fn the_settings_turn_each_decoration_off_and_many_problems_are_nine_plus() {
        let mut f = facts(vec![file("a.rs", ' ', 'M')]);
        f.problems = (0..12).map(|_| ("a.rs".to_string(), Severity::Warning)).collect();
        assert_eq!(marks(&f, Flags::default()).of("a.rs").badge, "9+, M");
        assert_eq!(marks(&f, Flags { problems: false, ..Flags::default() }).of("a.rs").badge, "M");
        assert_eq!(marks(&f, Flags { git: false, ..Flags::default() }).of("a.rs").tint, Some(Tint::Warning));
        let plain = marks(&f, Flags { colors: false, badges: false, ..Flags::default() });
        assert_eq!((plain.of("a.rs").badge.as_str(), plain.of("a.rs").tint), ("", None));
        assert_eq!(plain.of("a.rs").tips.len(), 2);
        assert!(marks(&facts(vec![file("d/a.rs", ' ', 'M')]), Flags { badges: false, ..Flags::default() }).of("d").dot.eq(&false));
    }

    #[test]
    fn a_tooltip_says_the_path_and_each_decoration() {
        let m = Mark { tips: vec!["1 problem in this file".into(), "Modified".into()], ..Mark::default() };
        assert_eq!(tooltip("src/a.rs", &m), "src/a.rs \u{2022} 1 problem in this file \u{2022} Modified");
        assert_eq!(tooltip("src", &Mark::default()), "src");
    }
}
