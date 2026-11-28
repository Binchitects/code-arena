//! Changed files as a tree or a list, as VS Code's Source Control has them, for Source Control and for the agent's
//! Changes. Each view shows its files as `scm.defaultViewMode` says until View as Tree or View as List in its title
//! chooses for the folder; in a tree, a folder with nothing in it but one folder shares its row with it
//! (`scm.compactFolders`: `tests/unit`), folders come before the files beside them, and what is folded stays folded.
//! The choice and the folds are kept with the folder's view state (`kept`, under "scm_tree"), so they are back at
//! the next start. Collapse All folds every folder of the tree.
//!
//! The rows are laid out here (`layout`, plain and tested); each view makes its own rows of them, with its own ids.
//! What every part has (`register`, `take`) is said in `ext.rs`.

use std::collections::{BTreeMap, BTreeSet};

use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use serde_json::{Value, json};

use crate::state::{Send, Ui};

/// The views whose files are shown as a tree or a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Scm,
    Changes,
}

impl Which {
    fn id(self) -> &'static str {
        match self {
            Which::Scm => "scm",
            Which::Changes => "changes",
        }
    }
}

/// One view's shape: a tree or a list as chosen (none: as `scm.defaultViewMode` says), and its folded folders.
#[derive(Clone, Copy)]
struct Shape {
    tree: RwSignal<Option<bool>>,
    folded: RwSignal<BTreeSet<String>>,
}

impl Shape {
    fn new(ui: Ui) -> Shape {
        Shape { tree: ui.scope.create_rw_signal(None), folded: ui.scope.create_rw_signal(BTreeSet::new()) }
    }
}

/// This part's state: both views' shapes, and whether the folder's kept shapes were read.
#[derive(Clone, Copy)]
struct Trees {
    scm: Shape,
    changes: Shape,
    restored: RwSignal<bool>,
}

impl crate::ext::State for Trees {
    fn new(ui: Ui) -> Trees {
        Trees { scm: Shape::new(ui), changes: Shape::new(ui), restored: ui.scope.create_rw_signal(false) }
    }
}

fn shape(ui: Ui, w: Which) -> Shape {
    let t = ui.state::<Trees>();
    match w {
        Which::Scm => t.scm,
        Which::Changes => t.changes,
    }
}

/// Whether a choice (or none) and the default say tree.
fn tree_of(chosen: Option<bool>, default: &Value) -> bool {
    chosen.unwrap_or(default.as_str() == Some("tree"))
}

/// Whether the view shows a tree now. Followed.
pub fn as_tree(ui: Ui, w: Which) -> bool {
    let chosen = shape(ui, w).tree.get();
    tree_of(chosen, &crate::settings::setting(ui, "scm.defaultViewMode", None))
}

/// As [`as_tree`], not followed: for a row being drawn, or a command.
pub fn as_tree_now(ui: Ui, w: Which) -> bool {
    let chosen = shape(ui, w).tree.get_untracked();
    tree_of(chosen, &crate::settings::now(ui, "scm.defaultViewMode", None))
}

/// The view shown as a tree, or as a list, as chosen for the folder; nothing is set when that is the choice already.
pub fn set_tree(ui: Ui, w: Which, tree: bool) {
    let s = shape(ui, w);
    if s.tree.get_untracked() != Some(tree) {
        s.tree.set(Some(tree));
    }
}

/// Whether a folder with only one folder in it shares its row with it. Followed.
pub fn compact(ui: Ui) -> bool {
    crate::settings::setting(ui, "scm.compactFolders", None).as_bool().unwrap_or(true)
}

/// The view's folded folders, by the keys its rows give them. Followed.
pub fn folded(ui: Ui, w: Which) -> BTreeSet<String> {
    shape(ui, w).folded.get()
}

/// A folder folded, or unfolded.
pub fn toggle(ui: Ui, w: Which, key: &str) {
    shape(ui, w).folded.update(|f| {
        if !f.remove(key) {
            f.insert(key.to_string());
        }
    });
}

/// These folders folded and no others (Collapse All); nothing is set when they already are.
pub fn fold_only(ui: Ui, w: Which, keys: BTreeSet<String>) {
    let f = shape(ui, w).folded;
    if f.with_untracked(|now| *now != keys) {
        f.set(keys);
    }
}

/// A row of a tree laid out: a folder (its path, how many of the path's last names its row shows, and whether it
/// is open), or the file at that place of the paths given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Folder { path: String, span: usize, depth: usize, open: bool },
    File { index: usize, depth: usize },
}

/// The folders of a tree, each with the files right in it (by their place in the paths given).
#[derive(Default)]
struct Dir<'a> {
    dirs: BTreeMap<&'a str, Dir<'a>>,
    files: Vec<usize>,
}

fn emit(dir: &Dir, prefix: &str, depth: usize, compact: bool, open: &dyn Fn(&str) -> bool, out: &mut Vec<Entry>) {
    for (name, sub) in &dir.dirs {
        let (mut path, mut span, mut sub) = (format!("{prefix}{name}"), 1, sub);
        while compact && sub.files.is_empty() && sub.dirs.len() == 1 {
            let Some((name, only)) = sub.dirs.iter().next() else { break };
            path = format!("{path}/{name}");
            span += 1;
            sub = only;
        }
        let is_open = open(&path);
        out.push(Entry::Folder { path: path.clone(), span, depth, open: is_open });
        if is_open {
            emit(sub, &format!("{path}/"), depth + 1, compact, open, out);
        }
    }
    for &index in &dir.files {
        out.push(Entry::File { index, depth });
    }
}

/// The rows of a tree of these paths: each folder before the files beside it, the folders by name and the files in
/// the order given; with `compact`, a folder with only one folder in it on one row with it. A folder `open` says is
/// not open has its rows left out.
pub fn layout(paths: &[&str], compact: bool, open: &dyn Fn(&str) -> bool) -> Vec<Entry> {
    let mut top = Dir::default();
    for (index, path) in paths.iter().enumerate() {
        let mut at = &mut top;
        let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        parts.pop();
        for p in parts {
            at = at.dirs.entry(p).or_default();
        }
        at.files.push(index);
    }
    let mut out = Vec::new();
    emit(&top, "", 0, compact, open, &mut out);
    out
}

/// Every folder's row of a tree of these paths (what Collapse All folds).
pub fn folders(paths: &[&str], compact: bool) -> Vec<String> {
    layout(paths, compact, &|_| true)
        .into_iter()
        .filter_map(|e| match e {
            Entry::Folder { path, .. } => Some(path),
            Entry::File { .. } => None,
        })
        .collect()
}

/// Whether a path is somewhere under a folder.
pub fn under(folder: &str, path: &str) -> bool {
    path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/'))
}

/// What a folder's row says: the last `span` names of its path (`tests/unit`).
pub fn label_of(path: &str, span: usize) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    parts[parts.len().saturating_sub(span.max(1))..].join("/")
}

/// What is kept with the folder's view state (`Ui::view_state`).
pub fn kept(ui: Ui) -> Value {
    let one = |w: Which| {
        let s = shape(ui, w);
        json!({"tree": s.tree.get_untracked(), "folded": s.folded.with_untracked(|f| f.iter().cloned().collect::<Vec<_>>())})
    };
    json!({"scm": one(Which::Scm), "changes": one(Which::Changes)})
}

/// Read by what keeps the layout, so a change here is kept too.
pub fn track(ui: Ui) {
    for w in [Which::Scm, Which::Changes] {
        let s = shape(ui, w);
        s.tree.track();
        s.folded.track();
    }
}

fn restore(ui: Ui, v: &Value) {
    let t = ui.state::<Trees>();
    // The first state only (a session switched keeps the window's own).
    if t.restored.get_untracked() {
        return;
    }
    t.restored.set(true);
    for w in [Which::Scm, Which::Changes] {
        let (s, v) = (shape(ui, w), &v[w.id()]);
        if let Some(tree) = v["tree"].as_bool() {
            s.tree.set(Some(tree));
        }
        let folded: BTreeSet<String> =
            v["folded"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        if !folded.is_empty() {
            s.folded.set(folded);
        }
    }
}

/// What it adds to the window's registries, once at start: nothing; the views register their own title actions.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it. The
/// folder's kept shapes come with the first state; never taken away.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "state" {
        restore(ui, &m["view_state"]["scm_tree"]);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(paths: &[&str], compact: bool, folded: &[&str]) -> Vec<String> {
        layout(paths, compact, &|p| !folded.contains(&p))
            .into_iter()
            .map(|e| match e {
                Entry::Folder { path, span, depth, open } => {
                    format!("{depth} {}{} [{path}]", if open { "v " } else { "> " }, label_of(&path, span))
                }
                Entry::File { index, depth } => format!("{depth} {}", paths[index]),
            })
            .collect()
    }

    #[test]
    fn folders_come_first_and_a_folder_with_one_folder_shares_its_row() {
        let paths = ["README.md", "tests/unit/a.rs", "tests/unit/b.rs", "src/ui/view.rs", "src/main.rs"];
        assert_eq!(
            shown(&paths, true, &[]),
            [
                "0 v src [src]",
                "1 v ui [src/ui]",
                "2 src/ui/view.rs",
                "1 src/main.rs",
                "0 v tests/unit [tests/unit]",
                "1 tests/unit/a.rs",
                "1 tests/unit/b.rs",
                "0 README.md",
            ]
        );
        // Without compact folders each folder has its own row.
        assert_eq!(
            shown(&["tests/unit/a.rs"], false, &[]),
            ["0 v tests [tests]", "1 v unit [tests/unit]", "2 tests/unit/a.rs"]
        );
        // A chain of three, and a folder that has files is where the chain ends.
        assert_eq!(shown(&["a/b/c/d.rs", "a/x.rs"], true, &[]), ["0 v a [a]", "1 v b/c [a/b/c]", "2 a/b/c/d.rs", "1 a/x.rs"]);
    }

    #[test]
    fn a_folded_folder_keeps_its_row_and_hides_what_is_in_it() {
        let paths = ["src/ui/view.rs", "src/main.rs", "b.rs"];
        assert_eq!(shown(&paths, true, &["src"]), ["0 > src [src]", "0 b.rs"]);
        assert_eq!(shown(&paths, true, &["src/ui"]), ["0 v src [src]", "1 > ui [src/ui]", "1 src/main.rs", "0 b.rs"]);
    }

    #[test]
    fn every_folder_is_listed_for_collapse_all() {
        let paths = ["tests/unit/a.rs", "src/ui/view.rs", "src/main.rs"];
        assert_eq!(folders(&paths, true), ["src", "src/ui", "tests/unit"]);
        assert_eq!(folders(&paths, false), ["src", "src/ui", "tests", "tests/unit"]);
        assert!(folders(&["a.rs"], true).is_empty());
    }

    #[test]
    fn a_path_is_under_a_folder_only_past_a_slash() {
        assert!(under("src", "src/a.rs"));
        assert!(under("src/ui", "src/ui/deep/a.rs"));
        assert!(!under("src", "srcs/a.rs"));
        assert!(!under("src", "src"));
    }

    #[test]
    fn a_folders_row_says_the_last_names_it_spans() {
        assert_eq!(label_of("a/b/c", 2), "b/c");
        assert_eq!(label_of("a/b/c", 1), "c");
        assert_eq!(label_of("a", 3), "a");
    }

    #[test]
    fn the_choice_wins_over_the_default() {
        assert!(!tree_of(None, &json!("list")));
        assert!(tree_of(None, &json!("tree")));
        assert!(!tree_of(None, &Value::Null));
        assert!(tree_of(Some(true), &json!("list")));
        assert!(!tree_of(Some(false), &json!("tree")));
    }

    #[test]
    fn the_shapes_are_kept_and_read_back_once() {
        let ui = Ui::new();
        let send: Send = std::rc::Rc::new(|_| {});
        set_tree(ui, Which::Scm, true);
        toggle(ui, Which::Changes, "folder:src");
        let kept = kept(ui);
        assert_eq!(kept["scm"]["tree"], true);
        assert_eq!(kept["changes"]["tree"], Value::Null);
        assert_eq!(kept["changes"]["folded"], json!(["folder:src"]));

        let again = Ui::new();
        take(again, &json!({"type": "state", "view_state": {"scm_tree": kept}}), &send);
        assert!(as_tree_now(again, Which::Scm));
        assert!(!as_tree_now(again, Which::Changes), "no choice: the default, a list");
        assert!(shape(again, Which::Changes).folded.with_untracked(|f| f.contains("folder:src")));
        // A later state (a session switched) leaves the window's own shapes.
        take(again, &json!({"type": "state", "view_state": {"scm_tree": {"scm": {"tree": false}}}}), &send);
        assert!(as_tree_now(again, Which::Scm));
    }

    #[test]
    fn choosing_what_is_shown_sets_nothing() {
        let ui = Ui::new();
        set_tree(ui, Which::Scm, false);
        assert_eq!(shape(ui, Which::Scm).tree.get_untracked(), Some(false));
        set_tree(ui, Which::Scm, true);
        assert_eq!(shape(ui, Which::Scm).tree.get_untracked(), Some(true));
        let before = shape(ui, Which::Scm).tree;
        set_tree(ui, Which::Scm, true);
        assert_eq!(before.get_untracked(), Some(true));
    }
}
