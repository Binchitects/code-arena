//! The Explorer, as VS Code's: the folder's section (its name in capitals, 22 px), then the tree in 22 px rows, each
//! level 8 px further in with its twistie, a folder opening on a click, a file opening in the editor. Its file
//! operations are VS Code's too: New File and New Folder typed in a row of the tree, a row renamed in place (F2), a
//! file or a folder deleted after a question (Delete), a row's menu on a right-click, and the arrows to go through
//! the tree. The session does the work on disk and says what it did; the tree follows what it says.
//!
//! Several rows are selected as in VS Code (Ctrl+click, Shift+click, Ctrl+A), and what acts on a row acts on all
//! of them: Delete, Cut (Ctrl+X), Copy (Ctrl+C) and Paste (Ctrl+V) into the folder selected. Rows are dragged onto
//! a folder (or the tree's empty part, the top) to move them, Ctrl held to copy them, and files dropped from the
//! system are copied in. The file the editor shows is selected in the tree (`explorer.autoReveal`), its folders
//! opened. A folder being listed shows "…" under it, one that could not be listed says why, a link is marked ↪.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use floem::action::exec_after;
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::{Point, Rect};
use floem::peniko::{Brush, Color};
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, dyn_stack, empty, h_stack, label, scroll, text_input, tooltip, v_stack};
use floem::{AnyView, IntoView, View};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::menus::Menu;
use crate::state::{Entry, Send, Ui};
use crate::widgets::{action, bold, caps, dialog, line, popup, toast};

/// VS Code's row in a tree.
const ROW: f64 = 22.0;

/// What the tree's inline input is for.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// A new file or a new folder, in this folder (the project's top: empty).
    New { parent: String, dir: bool },
    /// The row of this path, renamed.
    Rename { path: String },
}

/// A move through the tree with the keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Enter,
}

#[derive(Clone, Debug, PartialEq)]
struct Row {
    depth: usize,
    path: String,
    name: String,
    dir: bool,
    open: bool,
    /// The row is the inline input: a row being renamed, or the one a new name is typed in (its path is then its
    /// folder's and a slash, which no file's is).
    input: bool,
    /// The entry is a link.
    link: bool,
    /// A line under an open folder instead of an entry: "…" while it is listed, or why it could not be (its path
    /// is then its folder's, a slash and a character no file's name has).
    note: Option<String>,
}

/// The Explorer's own state, one per window.
#[derive(Clone, Copy)]
pub struct Explorer {
    /// The rows selected with the focused one (`ui.selected`): a selection of several while it holds that one.
    picked: RwSignal<HashSet<String>>,
    /// Where a Shift+click's range begins.
    anchor: RwSignal<String>,
    /// What Copy or Cut took.
    clip: RwSignal<Option<Clip>>,
    /// The entries that are links, and the folders that could not be listed, with why.
    links: RwSignal<HashSet<String>>,
    errors: RwSignal<HashMap<String, String>>,
    /// The rows being dragged.
    drag: RwSignal<Option<Drag>>,
    /// The folder they would go in, told only when it is another.
    over: Memo<Option<String>>,
    /// How far down the row to keep in sight is.
    reveal: RwSignal<f64>,
    /// A file to show once its folders are listed, and whether the tree scrolls to it.
    pending: RwSignal<Option<(String, bool)>>,
    /// What this window asked to be copied (where to) or deleted: only that is selected, or offered back, after.
    asked: RwSignal<HashSet<String>>,
    /// The list of rows, whose place in the window says which row the pointer is over.
    list: RwSignal<Option<floem::ViewId>>,
}

/// The rows Copy or Cut took.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    paths: Vec<String>,
    cut: bool,
}

/// Rows dragged: which, and the folder under the pointer (the top: empty; none: nowhere they may go).
#[derive(Clone, Debug, PartialEq)]
struct Drag {
    paths: Vec<String>,
    over: Option<String>,
}

impl crate::ext::State for Explorer {
    fn new(ui: Ui) -> Self {
        let drag = ui.scope.create_rw_signal(None::<Drag>);
        Explorer {
            picked: ui.scope.create_rw_signal(HashSet::new()),
            anchor: ui.scope.create_rw_signal(String::new()),
            clip: ui.scope.create_rw_signal(None),
            links: ui.scope.create_rw_signal(HashSet::new()),
            errors: ui.scope.create_rw_signal(HashMap::new()),
            drag,
            over: ui.scope.create_memo(move |_| drag.with(|d| d.as_ref().and_then(|d| d.over.clone()))),
            reveal: ui.scope.create_rw_signal(0.0),
            pending: ui.scope.create_rw_signal(None),
            asked: ui.scope.create_rw_signal(HashSet::new()),
            list: ui.scope.create_rw_signal(None),
        }
    }
}

fn me(ui: Ui) -> Explorer {
    ui.state::<Explorer>()
}

/// A signal set only to another value: what reads it is not run again for nothing.
fn put<T: PartialEq + 'static>(s: RwSignal<T>, v: T) {
    if s.with_untracked(|now| *now != v) {
        s.set(v);
    }
}

/// The path is the folder or inside it (everything is inside the top).
fn inside(path: &str, folder: &str) -> bool {
    folder.is_empty() || path == folder || path.strip_prefix(folder).is_some_and(|r| r.starts_with('/'))
}

/// The rows from one to another, both in, in the tree's order (only the second when the first is not shown).
fn range(order: &[String], from: &str, to: &str) -> Vec<String> {
    let at = |p: &str| order.iter().position(|o| o == p);
    match (at(from), at(to)) {
        (Some(a), Some(b)) => order[a.min(b)..=a.max(b)].to_vec(),
        _ => vec![to.to_string()],
    }
}

/// The paths, without those inside a folder among them: a folder moved takes what is in it.
fn outermost(paths: &[String]) -> Vec<String> {
    paths.iter().filter(|p| !paths.iter().any(|q| q != *p && inside(p, q))).cloned().collect()
}

/// What the session is asked so the rows go in a folder: moved (renamed), or copied (it names the copy when the
/// name is taken). A row already there is not moved, and a folder never goes into itself.
fn moves(paths: &[String], folder: &str, copy: bool) -> Vec<Value> {
    outermost(paths)
        .into_iter()
        .filter(|p| !(folder == p || (!folder.is_empty() && inside(folder, p))))
        .filter(|p| copy || parent_of(p) != folder)
        .map(|p| {
            let to = join(folder, name_of(&p));
            if copy {
                json!({"type": "copy", "path": p, "to": to})
            } else {
                json!({"type": "rename", "path": p, "to": to})
            }
        })
        .collect()
}

/// The icon of an entry, by its kind as the old Code Arena's own icons have it: code, text, JSON, a shell script, a
/// picture, settings; a folder open or closed.
fn icon_of(name: &str, dir: bool, open: bool) -> &'static str {
    if dir {
        return if open { "folder-opened" } else { "folder" };
    }
    let lower = name.to_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "go" | "c" | "h" | "cpp" | "hpp" | "cc" | "cs"
        | "java" | "kt" | "swift" | "rb" | "php" | "lua" | "html" | "css" | "scss" | "vue" | "svelte" | "sql"
        | "dart" | "scala" | "zig" => "file-code",
        // The braces, as VS Code's file icons draw JSON.
        "json" | "jsonc" | "json5" => "symbol-namespace",
        "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" => "terminal",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" | "avif" => "file-media",
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "env" | "lock" | "editorconfig" | "gitignore"
        | "gitattributes" | "npmrc" | "xml" => "settings-gear",
        "zip" | "gz" | "tar" | "tgz" | "7z" | "rar" => "file-zip",
        _ if lower == "dockerfile" || lower == "makefile" || lower.starts_with(".env") => "settings-gear",
        _ => "file-text",
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn name_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, n)| n)
}

/// The rows shown: each folder's entries under it while it is open, and the inline input where it belongs (a new
/// folder before its folder's entries, a new file before its folder's files).
/// A folder still being listed, or one that could not be, has a line of its own under it.
fn rows_of(tree: &HashMap<String, Vec<Entry>>, expanded: &HashSet<String>, edit: Option<&Edit>, x: &Listed) -> Vec<Row> {
    fn walk(
        tree: &HashMap<String, Vec<Entry>>,
        expanded: &HashSet<String>,
        edit: Option<&Edit>,
        x: &Listed,
        dir: &str,
        depth: usize,
        out: &mut Vec<Row>,
    ) {
        let new = match edit {
            Some(Edit::New { parent, dir: d }) if parent == dir => Some(*d),
            _ => None,
        };
        let row = |path: String, name: String, dir: bool| Row {
            depth,
            path,
            name,
            dir,
            open: false,
            input: false,
            link: false,
            note: None,
        };
        let input = |d: bool| Row { input: true, ..row(format!("{dir}/"), String::new(), d) };
        let note = match (tree.contains_key(dir), x.errors.get(dir)) {
            (_, Some(e)) => Some(e.clone()),
            (false, None) => Some("\u{2026}".to_string()),
            (true, None) => None,
        };
        if let Some(note) = note {
            if let Some(d) = new {
                out.push(input(d));
            }
            out.push(Row { note: Some(note), ..row(format!("{dir}/\u{1}"), String::new(), false) });
            return;
        }
        let mut placed = false;
        for e in tree.get(dir).map(Vec::as_slice).unwrap_or_default() {
            if let Some(d) = new
                && !placed
                && (d || !e.dir)
            {
                out.push(input(d));
                placed = true;
            }
            let path = join(dir, &e.name);
            let open = e.dir && expanded.contains(&path);
            let renamed = matches!(edit, Some(Edit::Rename { path: p }) if *p == path);
            let link = x.links.contains(&path);
            out.push(Row { open, input: renamed, link, ..row(path.clone(), e.name.clone(), e.dir) });
            if open {
                walk(tree, expanded, edit, x, &path, depth + 1, out);
            }
        }
        if let Some(d) = new
            && !placed
        {
            out.push(input(d));
        }
    }
    let mut out = Vec::new();
    walk(tree, expanded, edit, x, "", 0, &mut out);
    out
}

/// What the listings said besides their entries.
#[derive(Default)]
struct Listed {
    links: HashSet<String>,
    errors: HashMap<String, String>,
}

fn listed(ui: Ui, follow: bool) -> Listed {
    let x = me(ui);
    if follow {
        Listed { links: x.links.get(), errors: x.errors.get() }
    } else {
        Listed { links: x.links.get_untracked(), errors: x.errors.get_untracked() }
    }
}

/// The rows drawn. What is typed in the input and which row is selected are not read here, so neither makes the
/// tree again.
fn rows(ui: Ui) -> Vec<Row> {
    let l = listed(ui, true);
    ui.tree.with(|t| ui.expanded.with(|x| ui.explorer_edit.with(|e| rows_of(t, x, e.as_ref(), &l))))
}

/// The rows as they are now (the notes among them, so a row's place is where it is drawn), for the keys.
fn rows_now(ui: Ui) -> Vec<Row> {
    let l = listed(ui, false);
    ui.tree.with_untracked(|t| ui.expanded.with_untracked(|x| rows_of(t, x, None, &l)))
}

/// The entries shown, in the tree's order: what a range or Ctrl+A selects.
fn order(ui: Ui) -> Vec<String> {
    rows_now(ui).into_iter().filter(|r| r.note.is_none()).map(|r| r.path).collect()
}

/// The rows selected, in the tree's order: the focused one and those selected with it. What acts on "the selected
/// rows" (Delete, Copy, Cut, a drag, the commands of other parts such as Compare Selected) acts on these.
pub fn selection(ui: Ui) -> Vec<String> {
    let sel = ui.selected.get_untracked();
    let picked = me(ui).picked.get_untracked();
    if sel.is_empty() {
        return Vec::new();
    }
    if picked.len() < 2 || !picked.contains(&sel) {
        return vec![sel];
    }
    let mut out: Vec<String> = order(ui).into_iter().filter(|p| picked.contains(p)).collect();
    // Those in a folder closed since: after, by name.
    let mut rest: Vec<String> = picked.iter().filter(|p| !out.contains(p)).cloned().collect();
    rest.sort();
    out.extend(rest);
    out
}

/// One row selected, alone.
fn select_one(ui: Ui, path: &str) {
    let x = me(ui);
    if x.picked.with_untracked(|p| !p.is_empty()) {
        x.picked.set(HashSet::new());
    }
    put(x.anchor, path.to_string());
    put(ui.selected, path.to_string());
}

/// A click on a row, as VS Code takes it: alone, a folder opens or closes and a file opens; with Ctrl (Cmd on
/// macOS) the row is added to the selection or taken out of it; with Shift the rows from the last one clicked to
/// it are the selection.
fn click(ui: Ui, send: &Send, path: &str, dir: bool, m: Modifiers) {
    let x = me(ui);
    let sel = ui.selected.get_untracked();
    if m.contains(Modifiers::SHIFT) {
        let from = x.anchor.get_untracked();
        let from = if from.is_empty() { sel } else { from };
        put(x.picked, range(&order(ui), &from, path).into_iter().collect());
        put(ui.selected, path.to_string());
        return;
    }
    if m.intersects(Modifiers::CONTROL | Modifiers::META) {
        let mut picked = x.picked.get_untracked();
        if !picked.contains(&sel) {
            picked = [sel].into_iter().filter(|s| !s.is_empty()).collect();
        }
        let focus = if picked.remove(path) {
            // Taken out: the focus goes to the last row still selected.
            order(ui).into_iter().rev().find(|p| picked.contains(p)).unwrap_or_default()
        } else {
            picked.insert(path.to_string());
            path.to_string()
        };
        put(x.picked, picked);
        put(x.anchor, path.to_string());
        put(ui.selected, focus);
        return;
    }
    select_one(ui, path);
    if dir {
        ui.toggle_folder(path, send);
    } else {
        ui.open(path, send);
    }
}

/// Ctrl+A in the tree: every row shown.
fn select_all(ui: Ui) {
    let all = order(ui);
    let Some(first) = all.first().cloned() else { return };
    if ui.selected.with_untracked(|s| !all.contains(s)) {
        ui.selected.set(first);
    }
    put(me(ui).picked, all.into_iter().collect());
}

/// Copy or Cut: the rows selected (a folder's, not again what is in it).
fn take_rows(ui: Ui, cut: bool) {
    let paths = outermost(&selection(ui));
    if !paths.is_empty() {
        put(me(ui).clip, Some(Clip { paths, cut }));
    }
}

/// The folder a paste or a drop goes in: the folder of the row (a file's is the one it is in), or the top.
fn folder_of(ui: Ui, path: &str) -> String {
    if path.is_empty() || is_dir(ui, path) { path.to_string() } else { parent_of(path).to_string() }
}

/// What Copy or Cut took, in the folder selected: copies (named "name copy.ext" when the name is there), or moved
/// there (their tabs follow them). Cut rows are pasted once.
fn paste(ui: Ui, send: &Send) {
    let x = me(ui);
    let Some(clip) = x.clip.get_untracked() else { return };
    let folder = folder_of_selected(ui);
    if let Some(p) = clip.paths.iter().find(|p| !folder.is_empty() && inside(&folder, p)) {
        toast::error(ui, &format!("Cannot paste '{}' into itself.", name_of(p)));
        return;
    }
    go(ui, send, &folder, moves(&clip.paths, &folder, !clip.cut));
    if clip.cut {
        x.clip.set(None);
    }
}

/// The session asked to move or copy rows into a folder, which is opened so they are seen there.
fn go(ui: Ui, send: &Send, folder: &str, asks: Vec<Value>) {
    if asks.is_empty() {
        return;
    }
    if !folder.is_empty() && !ui.expanded.with_untracked(|e| e.contains(folder)) {
        ui.toggle_folder(folder, send);
    }
    let x = me(ui);
    for m in asks {
        if m["type"] == "copy" {
            let to = m["to"].as_str().unwrap_or("").to_string();
            x.asked.update(|a| {
                a.insert(to);
            });
        }
        send(m);
    }
}

/// Rows dropped on a folder: moved, after VS Code's question while `explorer.confirmDragAndDrop` is on (its box
/// turns it off), or copied with Ctrl held (Alt on macOS), which asks nothing.
fn drop_on(ui: Ui, send: &Send, paths: &[String], folder: &str, copy: bool) {
    let asks = moves(paths, folder, copy);
    if asks.is_empty() {
        return;
    }
    let confirm = crate::settings::now(ui, "explorer.confirmDragAndDrop", None).as_bool() != Some(false);
    if copy || !confirm {
        go(ui, send, folder, asks);
        return;
    }
    let into = if folder.is_empty() { project_name(ui) } else { name_of(folder).to_string() };
    let question = match asks.as_slice() {
        [one] => format!("Are you sure you want to move '{}' into '{into}'?", name_of(one["path"].as_str().unwrap_or(""))),
        many => format!("Are you sure you want to move the following {} files into '{into}'?", many.len()),
    };
    let detail = names(asks.iter().filter_map(|m| m["path"].as_str()));
    let (send, folder) = (send.clone(), folder.to_string());
    let mut asked = dialog::ask(&question).button("Move").checkbox("Do not ask me again", false);
    if asks.len() > 1 {
        asked = asked.detail(&detail);
    }
    if let Some(tree) = ui.explorer_tree.get_untracked() {
        asked = asked.back(tree);
    }
    asked.open(ui, move |answer| {
        if !answer.yes() {
            return;
        }
        if answer.checked {
            send(json!({"type": "set_setting", "key": "explorer.confirmDragAndDrop", "value": false}));
        }
        go(ui, &send, &folder, asks.clone());
    });
}

/// The names of the paths, one a line, as VS Code lists them under a question about several (ten at most).
fn names<'a>(paths: impl Iterator<Item = &'a str>) -> String {
    let all: Vec<&str> = paths.map(name_of).collect();
    let mut lines: Vec<String> = all.iter().take(10).map(|n| n.to_string()).collect();
    match all.len().saturating_sub(10) {
        0 => {}
        1 => lines.push("...1 additional file not shown".into()),
        n => lines.push(format!("...{n} additional files not shown")),
    }
    lines.join("\n")
}

fn project_name(ui: Ui) -> String {
    ui.folder.get_untracked().rsplit('/').next().unwrap_or("").to_string()
}

/// A file shown in the tree: its folders opened (and listed when they were not), it selected, and the tree scrolled
/// to it once it is among the rows (at once, or when its folder's listing comes).
pub fn reveal(ui: Ui, send: &Send, path: &str, scroll: bool) {
    if path.is_empty() {
        return;
    }
    let mut at = String::new();
    let mut closed = Vec::new();
    for part in parent_of(path).split('/').filter(|p| !p.is_empty()) {
        at = join(&at, part);
        if !ui.expanded.with_untracked(|e| e.contains(&at)) {
            closed.push(at.clone());
        }
    }
    if !closed.is_empty() {
        ui.expanded.update(|e| e.extend(closed.iter().cloned()));
        for c in closed.iter().filter(|c| ui.tree.with_untracked(|t| !t.contains_key(*c))) {
            send(json!({"type": "ls", "path": c}));
        }
    }
    if !selection(ui).iter().any(|s| s == path) {
        select_one(ui, path);
    }
    put(me(ui).pending, Some((path.to_string(), scroll)));
    scroll_to_pending(ui);
}

/// The file waiting to be shown, scrolled to once it is a row.
fn scroll_to_pending(ui: Ui) {
    let x = me(ui);
    let Some((path, scroll)) = x.pending.get_untracked() else { return };
    let Some(i) = rows_now(ui).iter().position(|r| r.path == path) else { return };
    x.pending.set(None);
    if scroll {
        x.reveal.set(i as f64 * ROW);
    }
}

fn is_dir(ui: Ui, path: &str) -> bool {
    ui.tree.with_untracked(|t| {
        t.get(parent_of(path)).is_some_and(|es| es.iter().any(|e| e.dir && e.name == name_of(path)))
    })
}

/// The folder a new file goes in, as VS Code chooses it: the folder selected, or the one the selected file is in,
/// or the project's top.
fn folder_of_selected(ui: Ui) -> String {
    let sel = ui.selected.get_untracked();
    if sel.is_empty() || is_dir(ui, &sel) { sel } else { parent_of(&sel).to_string() }
}

/// What is wrong with a name typed, in VS Code's words; none when it can be asked of the session. A name may hold
/// folders on the way ("a/b.rs"), as VS Code's may.
fn problem(tree: &HashMap<String, Vec<Entry>>, edit: &Edit, text: &str) -> Option<String> {
    let name = text.trim();
    if name.is_empty() {
        return None;
    }
    if name.contains(['\\', ':']) || name.split('/').any(|p| p.is_empty() || p == "." || p == ".." || p == ".git") {
        return Some(format!("The name {name} is not valid as a file or folder name. Please choose a different name."));
    }
    let (parent, old) = match edit {
        Edit::New { parent, .. } => (parent.as_str(), None),
        Edit::Rename { path } => (parent_of(path), Some(name_of(path))),
    };
    if Some(name) != old && tree.get(parent).is_some_and(|es| es.iter().any(|e| e.name == name)) {
        return Some(format!(
            "A file or folder {name} already exists at this location. Please choose a different name."
        ));
    }
    None
}

/// The input shown for a new file or folder in `parent`, which is opened (and the folders it is in) so the input
/// is seen.
pub fn begin_new(ui: Ui, send: &Send, parent: &str, dir: bool) {
    let mut at = String::new();
    for part in parent.split('/').filter(|p| !p.is_empty()) {
        at = join(&at, part);
        if !ui.expanded.with_untracked(|e| e.contains(&at)) {
            ui.toggle_folder(&at, send);
        }
    }
    ui.explorer_text.set(String::new());
    ui.explorer_edit.set(Some(Edit::New { parent: parent.to_string(), dir }));
}

/// A row turned into the input, its name in it.
pub fn begin_rename(ui: Ui, path: &str) {
    if path.is_empty() {
        return;
    }
    ui.explorer_text.set(name_of(path).to_string());
    ui.explorer_edit.set(Some(Edit::Rename { path: path.to_string() }));
}

/// Enter in the input: the session is asked to make or to move it. A name that is not valid stays in the input;
/// none typed, or the name it had, is the same as Escape.
pub fn commit(ui: Ui, send: &Send) {
    let Some(edit) = ui.explorer_edit.get_untracked() else { return };
    let text = ui.explorer_text.get_untracked();
    let name = text.trim();
    if !name.is_empty() && ui.tree.with_untracked(|t| problem(t, &edit, name)).is_some() {
        return;
    }
    match &edit {
        _ if name.is_empty() => {}
        Edit::New { parent, dir } => {
            let path = join(parent, name);
            // Opened (a folder: selected) when the session says it is made.
            ui.explorer_asked.set(Some(path.clone()));
            send(json!({"type": "create", "path": path, "dir": dir}));
        }
        Edit::Rename { path } if name != name_of(path) => {
            send(json!({"type": "rename", "path": path, "to": join(parent_of(path), name)}));
        }
        Edit::Rename { .. } => {}
    }
    ui.explorer_edit.set(None);
}

/// A key in the tree: Up and Down move the selection, Left closes a folder (or goes to the folder a row is in),
/// Right opens one (or goes into it), Enter opens a file (or a folder). Says which row is selected after.
pub fn navigate(ui: Ui, send: &Send, nav: Nav) -> Option<usize> {
    // The notes are passed over; the place said is the row's among all that are drawn.
    let all = rows_now(ui);
    let drawn = |path: &str| all.iter().position(|r| r.path == path);
    let rows: Vec<&Row> = all.iter().filter(|r| r.note.is_none()).collect();
    let last = rows.len().checked_sub(1)?;
    let sel = ui.selected.get_untracked();
    let Some(i) = rows.iter().position(|r| r.path == sel) else {
        // Nothing selected (or a row no longer shown): the first row, or the last for Up.
        let i = if nav == Nav::Up { last } else { 0 };
        select_one(ui, &rows[i].path);
        return drawn(&rows[i].path);
    };
    let r = &rows[i];
    let to = match nav {
        Nav::Up => i.saturating_sub(1),
        Nav::Down => (i + 1).min(last),
        Nav::Left if r.dir && r.open => {
            ui.toggle_folder(&r.path, send);
            i
        }
        Nav::Left => rows[..i].iter().rposition(|p| p.depth < r.depth).unwrap_or(i),
        Nav::Right if r.dir && !r.open => {
            ui.toggle_folder(&r.path, send);
            i
        }
        Nav::Right if r.dir && i < last && rows[i + 1].depth > r.depth => i + 1,
        Nav::Right => i,
        Nav::Enter if r.dir => {
            ui.toggle_folder(&r.path, send);
            i
        }
        Nav::Enter => {
            ui.open(&r.path, send);
            i
        }
    };
    // The arrows move a selection of one, as VS Code's do.
    select_one(ui, &rows[to].path);
    drawn(&rows[to].path)
}

/// The tree listed again from the session: its top and each open folder.
fn refresh(ui: Ui, send: &Send) {
    ui.tree.update(|t| t.clear());
    me(ui).errors.update(|e| e.clear());
    send(json!({"type": "ls", "path": ""}));
    for p in ui.expanded.get_untracked() {
        send(json!({"type": "ls", "path": p}));
    }
}

/// VS Code's Open in Integrated Terminal: a new terminal, its shell started in the folder.
fn open_terminal(ui: Ui, send: &Send, folder: &str) {
    let (inner, cwd) = (send.clone(), folder.to_string());
    let with: Send = Rc::new(move |mut m: Value| {
        if m["type"] == "term_open" {
            m["cwd"] = json!(cwd);
        }
        inner(m)
    });
    crate::commands::run(ui, &with, crate::workbench::NEW_TERMINAL);
}

fn focus_tree(ui: Ui) {
    if let Some(id) = ui.explorer_tree.get_untracked() {
        id.request_focus();
    }
}

fn tip(v: impl IntoView + 'static, text: &'static str) -> impl IntoView {
    tooltip(v, move || label(move || text))
}

/// The Explorer title's actions, as VS Code's: New File, New Folder, Refresh, Collapse All.
// Not in the title yet: the title is drawn with the dock, which puts them there.
pub fn actions(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let (file, folder, again) = (send.clone(), send.clone(), send);
    h_stack((
        tip(action(t, "new-file", move || begin_new(ui, &file, &folder_of_selected(ui), false)), "New File..."),
        tip(action(t, "new-folder", move || begin_new(ui, &folder, &folder_of_selected(ui), true)), "New Folder..."),
        tip(action(t, "refresh", move || refresh(ui, &again)), "Refresh Explorer"),
        tip(action(t, "collapse-all", move || ui.expanded.update(|e| e.clear())), "Collapse Folders in Explorer"),
    ))
    .style(|s| s.gap(2.0))
}

/// VS Code's menu for a row (or for the tree's empty part: `path` empty), where the pointer is in the window: the
/// Explorer's own rows, then the ones the window's parts registered for the place (`menus.rs`), which act on the
/// row selected.
fn open_menu(ui: Ui, send: &Send, path: String, dir: bool, at: (f64, f64)) {
    let Some(tree) = ui.explorer_tree.get_untracked() else { return };
    let folder = if dir { path.clone() } else { parent_of(&path).to_string() };
    let new = |dir: bool| {
        let (send, folder) = (send.clone(), folder.clone());
        move || begin_new(ui, &send, &folder, dir)
    };
    let mut items: Vec<popup::Item> =
        vec![popup::entry("New File...", new(false)).into(), popup::entry("New Folder...", new(true)).into()];
    let terminal = {
        let (send, folder) = (send.clone(), folder.clone());
        move || open_terminal(ui, &send, &folder)
    };
    items.push(popup::entry("Open in Integrated Terminal", terminal).into());
    let pasted = send.clone();
    let paste_row = popup::entry("Paste", move || paste(ui, &pasted))
        .keys("Ctrl+V")
        .enabled(me(ui).clip.with_untracked(Option::is_some));
    items.push(popup::line());
    if path.is_empty() {
        items.push(paste_row.into());
    } else {
        let copy = |text: String| {
            move || {
                if floem::Clipboard::set_contents(text.clone()).is_err() {
                    toast::error(ui, "The path could not be copied.");
                }
            }
        };
        let (renamed, asked) = (path.clone(), send.clone());
        let several = selection(ui).len() > 1;
        items.extend([
            popup::entry("Cut", move || take_rows(ui, true)).keys("Ctrl+X").into(),
            popup::entry("Copy", move || take_rows(ui, false)).keys("Ctrl+C").into(),
            paste_row.into(),
            popup::line(),
            popup::entry("Copy Path", copy(join(&ui.folder.get_untracked(), &path))).into(),
            popup::entry("Copy Relative Path", copy(path.clone())).into(),
            popup::line(),
            popup::entry("Rename...", move || begin_rename(ui, &renamed)).keys("F2").enabled(!several).into(),
            popup::entry("Delete", move || ask_delete(ui, &asked, selection(ui))).keys("Delete").into(),
        ]);
    }
    let place = if path.is_empty() { Menu::ExplorerEmpty } else { Menu::ExplorerRow };
    let more = popup::of_rows(ui, send, crate::menus::rows(ui, place));
    if !more.is_empty() {
        items.push(popup::line());
        items.extend(more);
    }
    popup::open(ui, popup::At::point(tree, at.0, at.1).back(tree), items);
}

/// VS Code's question before a delete, as its dialog: the question, what becomes of the file, the two buttons.
/// What is deleted is not gone: the session keeps it among its backups.
fn ask_delete(ui: Ui, send: &Send, paths: Vec<String>) {
    let paths = outermost(&paths);
    let (question, detail) = match paths.as_slice() {
        [] => return,
        [path] if is_dir(ui, path) => (
            format!("Are you sure you want to delete '{}' and its contents?", name_of(path)),
            "You can restore this folder from the backups.".to_string(),
        ),
        [path] => {
            (format!("Are you sure you want to delete '{}'?", name_of(path)), "You can restore this file from the backups.".into())
        }
        many => {
            let question = if many.iter().any(|p| is_dir(ui, p)) {
                format!("Are you sure you want to delete the following {} files/directories and their contents?", many.len())
            } else {
                format!("Are you sure you want to delete the following {} files?", many.len())
            };
            let list = names(many.iter().map(String::as_str));
            (question, format!("{list}\n\nYou can restore them from the backups."))
        }
    };
    let send = send.clone();
    let mut asked = dialog::ask(&question).detail(&detail).button("Move to Backups");
    if let Some(tree) = ui.explorer_tree.get_untracked() {
        asked = asked.back(tree);
    }
    asked.open(ui, move |answer| {
        if !answer.yes() {
            return;
        }
        // One deleted alone is offered back in a notification.
        if let [one] = paths.as_slice() {
            me(ui).asked.update(|a| {
                a.insert(one.clone());
            });
        }
        for path in &paths {
            send(json!({"type": "delete", "path": path}));
        }
    });
}

type OpenMenu = Rc<dyn Fn(String, bool)>;

/// The inline input, in a row of the tree: Enter asks the session, Escape or the focus lost gives it up. What is
/// wrong with the name is said under it, as VS Code says it.
fn input_row(ui: Ui, send: Send, r: Row) -> impl IntoView {
    let t = ui.colors;
    let mine = ui.explorer_edit.get_untracked();
    let wrong = move || {
        ui.explorer_edit.with(|e| {
            e.as_ref().and_then(|e| ui.tree.with(|tree| ui.explorer_text.with(|text| problem(tree, e, text))))
        })
    };
    let input = text_input(ui.explorer_text)
        .on_event(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(k) = e
                && k.key.logical_key == Key::Named(NamedKey::Enter)
            {
                commit(ui, &send);
                if ui.explorer_edit.with_untracked(|e| e.is_none()) {
                    focus_tree(ui);
                }
            }
            // No key goes on to the workbench's own while a name is typed.
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            // Escape, or a press elsewhere: given up (unless another input has been opened since).
            if ui.explorer_edit.get_untracked() == mine {
                ui.explorer_edit.set(None);
            }
        })
        .style(move |s| {
            let c = t.get();
            s.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
                .height(20.0)
                .padding_horiz(3.0)
                .border(0.0)
                .font_size(13.0)
                .background(Color::TRANSPARENT)
                .color(c.input_fg)
                .cursor_color(Brush::Solid(c.cursor))
        });
    let id = input.id();
    // Once the row is in the tree, the input has the keys.
    exec_after(Duration::ZERO, move |_| id.request_focus());
    let twistie =
        if r.dir { icon("chevron-right", 16.0).into_any() } else { empty().style(|s| s.width(16.0)).into_any() };
    let indent = 8.0 * r.depth as f64;
    let field = container(input).style(move |s| {
        let c = t.get();
        s.flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_width(0.0)
            .height(ROW)
            .margin_left(4.0)
            .items_center()
            .border(1.0)
            .border_color(if wrong().is_some() { c.input_error } else { c.focus })
            .background(c.input_bg)
    });
    v_stack((
        h_stack((empty().style(move |s| s.width(indent)), twistie, field)).style(move |s| {
            s.height(ROW).width_full().items_center().padding_left(8.0).padding_right(12.0).color(t.get().side_fg)
        }),
        label(move || wrong().unwrap_or_default()).style(move |s| {
            let c = t.get();
            s.margin_left(indent + 28.0)
                .margin_right(12.0)
                .padding_horiz(4.0)
                .padding_vert(3.0)
                .font_size(12.0)
                .line_height(1.4)
                .border(1.0)
                .border_color(c.input_error)
                .background(c.input_error_bg)
                .color(c.fg)
                .text_overflow(TextOverflow::Wrap)
                .apply_if(wrong().is_none(), |s| s.hide())
        }),
    ))
    .style(|s| s.width_full())
}

fn row_view(ui: Ui, send: Send, r: Row, focused: RwSignal<bool>, menu: OpenMenu) -> AnyView {
    if r.input {
        return input_row(ui, send, r).into_any();
    }
    let t = ui.colors;
    let x = me(ui);
    let indent = 8.0 * r.depth as f64;
    if let Some(note) = r.note.clone() {
        // "…" while the folder is listed; why it could not be, in the error's colour.
        let error = note != "\u{2026}";
        return label(move || note.clone())
            .style(move |s| {
                let c = t.get();
                s.height(ROW)
                    .width_full()
                    .items_center()
                    .padding_left(8.0 + indent + 16.0 + 4.0)
                    .padding_right(12.0)
                    .font_size(13.0)
                    .color(if error { c.error } else { c.desc })
                    .text_ellipsis()
                    .selectable(false)
            })
            .into_any();
    }
    let (path, shown, target) = (r.path.clone(), r.path.clone(), r.path.clone());
    let name = if r.link { format!("{} \u{21aa}", r.name) } else { r.name.clone() };
    let row = h_stack((
        empty().style(move |s| s.width(indent)),
        if r.dir {
            icon(if r.open { "chevron-down" } else { "chevron-right" }, 16.0).into_any()
        } else {
            empty().style(|s| s.width(16.0)).into_any()
        },
        icon(icon_of(&r.name, r.dir, r.open), 16.0).style(|s| s.margin_left(2.0)),
        label(move || name.clone()).style(|s| s.padding_left(4.0).text_ellipsis()),
    ));
    // A press, then a move past 5 px, is a drag of the rows selected; let go without one, a click.
    let start = create_rw_signal(None::<Point>);
    let id = row.id();
    let at = move |p: Point| {
        let o = id.layout_rect().origin();
        Point::new(o.x + p.x, o.y + p.y)
    };
    let dragged = path.clone();
    row.on_event(EventListener::PointerDown, move |e| {
        if let Event::PointerDown(p) = e
            && p.button.is_primary()
        {
            start.set(Some(at(p.pos)));
            id.request_active();
        }
        EventPropagation::Continue
    })
    .on_event_stop(EventListener::PointerMove, move |e| {
        let (Event::PointerMove(p), Some(from)) = (e, start.get_untracked()) else { return };
        let now = at(p.pos);
        if x.drag.with_untracked(Option::is_none) {
            if (now - from).hypot() < 5.0 {
                return;
            }
            // The row dragged is among the rows selected, or it alone is.
            if !selection(ui).contains(&dragged) {
                select_one(ui, &dragged);
            }
            x.drag.set(Some(Drag { paths: outermost(&selection(ui)), over: None }));
        }
        let over = x.drag.with_untracked(|d| d.as_ref().and_then(|d| aim(ui, &d.paths, now)));
        if x.drag.with_untracked(|d| d.as_ref().is_some_and(|d| d.over != over)) {
            x.drag.update(|d| {
                if let Some(d) = d {
                    d.over = over;
                }
            });
        }
    })
    .on_event_stop(EventListener::PointerUp, move |e| {
        let Event::PointerUp(p) = e else { return };
        let was = start.get_untracked();
        start.set(None);
        match x.drag.get_untracked() {
            Some(d) => {
                x.drag.set(None);
                let copy = if cfg!(target_os = "macos") {
                    p.modifiers.contains(Modifiers::ALT)
                } else {
                    p.modifiers.contains(Modifiers::CONTROL)
                };
                if let Some(folder) = d.over {
                    drop_on(ui, &send, &d.paths, &folder, copy);
                }
            }
            None if was.is_some() => {
                focus_tree(ui);
                click(ui, &send, &path, r.dir, p.modifiers);
            }
            None => {}
        }
    })
    // The click was taken when the press ended: it is not the tree's empty part's.
    .on_click_stop(|_| {})
    .on_secondary_click_stop(move |_| {
        // A row among those selected keeps them; another is selected alone.
        if !selection(ui).contains(&target) {
            select_one(ui, &target);
        }
        menu(target.clone(), r.dir);
    })
    .style(move |s| {
        let c = t.get();
        let selected = ui.selected.with(|sel| {
            *sel == shown || x.picked.with(|p| p.len() > 1 && p.contains(sel) && p.contains(&shown))
        });
        let cut = x.clip.with(|k| k.as_ref().is_some_and(|k| k.cut && k.paths.contains(&shown)));
        let dropped = x.over.with(|o| o.as_ref().is_some_and(|f| !f.is_empty() && inside(&shown, f)));
        let s = s
            .height(ROW)
            .width_full()
            .items_center()
            .padding_left(8.0)
            .font_size(13.0)
            .color(c.side_fg)
            .cursor(CursorStyle::Pointer)
            .selectable(false);
        // VS Code's selection: lit while the tree has the keys, dimmed while it has not.
        let s = match (selected, focused.get()) {
            (true, true) => s.background(c.list_selected).color(c.list_selected_fg),
            (true, false) => s.background(c.list_inactive),
            _ => s.hover(|s| s.background(c.list_hover)),
        };
        // A row cut is faded until it is pasted; the folder rows would be dropped in is shaded with all in it.
        s.apply_if(cut, |s| s.color(c.desc)).apply_if(dropped, |s| s.background(c.drop_bg))
    })
    .into_any()
}

/// The folder rows dragged to a point in the window would go in: the folder under it (a file's: the one it is in),
/// the top below the rows; none outside the tree or where they may not go (into themselves).
fn aim(ui: Ui, paths: &[String], at: Point) -> Option<String> {
    let x = me(ui);
    let tree = ui.explorer_tree.get_untracked()?.layout_rect();
    if !tree.contains(at) {
        return None;
    }
    let top = x.list.get_untracked()?.layout_rect().y0;
    let rows = rows_now(ui);
    let i = ((at.y - top) / ROW).floor();
    let folder = drop_folder(&rows, i, |p| is_dir(ui, p));
    (!moves(paths, &folder, true).is_empty()).then_some(folder)
}

/// The folder of the row at a place in the list: a folder's own, a file's or a note's the one it is in, the top
/// past the last row.
fn drop_folder(rows: &[Row], i: f64, dir: impl Fn(&str) -> bool) -> String {
    if i < 0.0 {
        return String::new();
    }
    match rows.get(i as usize) {
        Some(r) if r.note.is_some() => r.path.trim_end_matches('\u{1}').trim_end_matches('/').to_string(),
        Some(r) if r.dir || dir(&r.path) => r.path.clone(),
        Some(r) => parent_of(&r.path).to_string(),
        None => String::new(),
    }
}

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    // The tree has the keys; the row to keep in sight after a key (how far down it is); and where the pointer is
    // in the window, for a menu opened by a right-click.
    let focused = create_rw_signal(false);
    let x = me(ui);
    let reveal = x.reveal;
    let pointer = create_rw_signal((0.0_f64, 0.0_f64));
    let menu: OpenMenu = {
        let send = send.clone();
        Rc::new(move |path, dir| open_menu(ui, &send, path, dir, pointer.get_untracked()))
    };
    let header = h_stack((
        icon("chevron-down", 16.0),
        label(move || ui.folder.get().rsplit('/').next().unwrap_or("").to_uppercase()).style(|s| bold(caps(s))),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.section_border, 'b')
            .height(22.0)
            .items_center()
            .gap(2.0)
            .padding_left(2.0)
            .background(c.section_bg)
            .color(c.section_fg)
            .selectable(false)
    });
    let list = {
        let (send, menu) = (send.clone(), menu.clone());
        dyn_stack(
            move || rows(ui),
            |r: &Row| (r.path.clone(), r.open, r.input, r.link, r.note.clone()),
            move |r: Row| row_view(ui, send.clone(), r, focused, menu.clone()),
        )
        .style(|s| s.flex_col().width_full())
    };
    x.list.set(Some(list.id()));
    let dropped = send.clone();
    let tree = scroll(list)
        .ensure_visible(move || {
            let y = reveal.get();
            Rect::new(0.0, y, 1.0, y + ROW)
        })
        .keyboard_navigable()
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            let m = k.modifiers;
            // Ctrl (Cmd on macOS) with A, C, X or V: the selection's, as in VS Code's Explorer.
            if m.intersects(Modifiers::CONTROL | Modifiers::META) && !m.intersects(Modifiers::SHIFT | Modifiers::ALT) {
                let Key::Character(c) = &k.key.logical_key else { return EventPropagation::Continue };
                match c.to_lowercase().as_str() {
                    "a" => select_all(ui),
                    "c" => take_rows(ui, false),
                    "x" => take_rows(ui, true),
                    "v" => paste(ui, &send),
                    _ => return EventPropagation::Continue,
                }
                return EventPropagation::Stop;
            }
            // Shift with Up or Down: the selection grows from where it began.
            if m == Modifiers::SHIFT
                && let Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown) = k.key.logical_key
            {
                let from = x.anchor.get_untracked();
                let nav = if k.key.logical_key == Key::Named(NamedKey::ArrowUp) { Nav::Up } else { Nav::Down };
                if let Some(i) = navigate(ui, &send, nav) {
                    reveal.set(i as f64 * ROW);
                }
                let to = ui.selected.get_untracked();
                let from = if from.is_empty() { to.clone() } else { from };
                put(x.picked, range(&order(ui), &from, &to).into_iter().collect());
                put(x.anchor, from);
                return EventPropagation::Stop;
            }
            if !m.is_empty() {
                return EventPropagation::Continue;
            }
            let nav = match &k.key.logical_key {
                Key::Named(NamedKey::ArrowUp) => Nav::Up,
                Key::Named(NamedKey::ArrowDown) => Nav::Down,
                Key::Named(NamedKey::ArrowLeft) => Nav::Left,
                Key::Named(NamedKey::ArrowRight) => Nav::Right,
                Key::Named(NamedKey::Enter) => Nav::Enter,
                Key::Named(NamedKey::F2) => {
                    begin_rename(ui, &ui.selected.get_untracked());
                    return EventPropagation::Stop;
                }
                Key::Named(NamedKey::Delete) => {
                    ask_delete(ui, &send, selection(ui));
                    return EventPropagation::Stop;
                }
                _ => return EventPropagation::Continue,
            };
            if let Some(i) = navigate(ui, &send, nav) {
                reveal.set(i as f64 * ROW);
            }
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| focused.set(false))
        // The tree's empty part: a click selects nothing (a new file then goes in the project's top), a right-click
        // opens the menu for the top.
        .on_click_stop(move |_| select_one(ui, ""))
        .on_secondary_click_stop(move |_| {
            // The menu's rows that the window's parts registered act on what is selected: here, the top.
            select_one(ui, "");
            menu(String::new(), true)
        })
        // Files dropped from the system: copied into the folder under the pointer (one there of that name is
        // replaced only after a question).
        .on_event_stop(EventListener::DroppedFile, move |e| {
            let Event::DroppedFile(f) = e else { return };
            let Some(name) = f.path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return };
            let o = ui.explorer_tree.get_untracked().map(|t| t.layout_rect().origin()).unwrap_or_default();
            let top = x.list.get_untracked().map(|l| l.layout_rect().y0).unwrap_or_default();
            let folder = drop_folder(&rows_now(ui), ((o.y + f.pos.y - top) / ROW).floor(), |p| is_dir(ui, p));
            import(ui, &dropped, &f.path.to_string_lossy(), &join(&folder, &name), false);
        })
        .style(move |s| {
            // Rows dragged to the top: the whole tree is shaded.
            let top = x.over.with(|o| o.as_deref() == Some(""));
            s.flex_grow(1.0_f32).flex_basis(0.0).width_full().apply_if(top, |s| s.background(t.get().drop_bg))
        });
    ui.explorer_tree.set(Some(tree.id()));
    let whole = v_stack((header, tree));
    let id = whole.id();
    whole
        .on_event_cont(EventListener::PointerMove, move |e| {
            if let Event::PointerMove(p) = e {
                // The event's point is this view's own: with the view's place in the window, the window's.
                let o = id.layout_rect().origin();
                pointer.set((o.x + p.pos.x, o.y + p.pos.y));
            }
        })
        .style(|s| s.size_full().min_height(0.0))
}

/// What the session could not do, said in the status bar as before, and as the old Code Arena says it: a
/// notification in the corner, kept in the bell's list.
fn said_wrong(ui: Ui, what: &str) {
    ui.message.set(what.to_string());
    toast::error_of(what).source("Explorer").show_in(ui);
}

/// A file or a folder from the system copied into the project at `to`; asked again, after VS Code's question,
/// to replace one there.
fn import(ui: Ui, send: &Send, from: &str, to: &str, replace: bool) {
    me(ui).asked.update(|a| {
        a.insert(to.to_string());
    });
    send(json!({"type": "import", "from": from, "to": to, "replace": replace}));
}

/// The rows' paths followed when a file or a folder is moved, and those inside one deleted dropped.
fn follow(ui: Ui, from: &str, to: Option<&str>) {
    let x = me(ui);
    let now = |p: &str| match to {
        Some(to) => Some(crate::state::moved_path(p, from, to).unwrap_or_else(|| p.to_string())),
        None => (!inside(p, from)).then(|| p.to_string()),
    };
    if x.picked.with_untracked(|p| p.iter().any(|p| inside(p, from))) {
        x.picked.update(|p| *p = p.iter().filter_map(|p| now(p)).collect());
    }
    if x.clip.with_untracked(|c| c.as_ref().is_some_and(|c| c.paths.iter().any(|p| inside(p, from)))) {
        x.clip.update(|c| {
            if let Some(k) = c {
                k.paths = k.paths.iter().filter_map(|p| now(p)).collect();
            }
            if c.as_ref().is_some_and(|k| k.paths.is_empty()) {
                *c = None;
            }
        });
    }
}

/// What it adds to the window's registries, once at start: its commands, and the file the editor shows followed in
/// the tree (`explorer.autoReveal`).
pub fn register(ui: Ui, send: &Send) {
    use crate::commands::{add, command};
    add(command("filesExplorer.cut", "File", "Cut", |ui, _| take_rows(ui, true)));
    add(command("filesExplorer.copy", "File", "Copy", |ui, _| take_rows(ui, false)));
    add(command("filesExplorer.paste", "File", "Paste", paste).when(|ui| me(ui).clip.with_untracked(Option::is_some)));
    add(command("workbench.files.action.showActiveFileInExplorer", "File", "Reveal Active File in Explorer View", |ui, send| {
        let Some(path) = ui.active.get_untracked().and_then(|a| ui.tab(&a)).filter(|t| t.is_file()).map(|t| t.path) else {
            return;
        };
        crate::commands::run(ui, send, crate::workbench::SHOW_EXPLORER);
        reveal(ui, send, &path, true);
    }));
    add(command("openInIntegratedTerminal", "File", "Open in Integrated Terminal", |ui, send| {
        let folder = folder_of(ui, &ui.selected.get_untracked());
        open_terminal(ui, send, &folder);
    }));
    let send = send.clone();
    ui.scope.create_effect(move |_| {
        let Some(active) = ui.active.get() else { return };
        let Some(path) = ui.tab(&active).filter(|t| t.is_file()).map(|t| t.path) else { return };
        let how = crate::settings::now(ui, "explorer.autoReveal", None);
        if how == json!(false) {
            return;
        }
        reveal(ui, &send, &path, how != json!("focusNoScroll"));
    });
}

/// The session's messages that are the Explorer's, taken in. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    let x = me(ui);
    match m["type"].as_str().unwrap_or("") {
        "ls" => {
            let path = s("path");
            if let Some(e) = m["error"].as_str() {
                // Said where its entries would be.
                if x.errors.with_untracked(|es| es.get(&path).map(String::as_str) != Some(e)) {
                    x.errors.update(|es| {
                        es.insert(path, e.to_string());
                    });
                }
                return false;
            }
            let all = m["entries"].as_array().map(Vec::as_slice).unwrap_or_default();
            let entries = all
                .iter()
                .map(|e| Entry { name: e["name"].as_str().unwrap_or("").into(), dir: e["dir"] == true })
                .collect();
            let links: HashSet<String> =
                all.iter().filter(|e| e["link"] == true).filter_map(|e| e["name"].as_str()).map(|n| join(&path, n)).collect();
            let gone_or_new = x.links.with_untracked(|l| {
                l.iter().any(|p| parent_of(p) == path && !links.contains(p)) || links.iter().any(|p| !l.contains(p))
            });
            if gone_or_new {
                x.links.update(|l| {
                    l.retain(|p| parent_of(p) != path);
                    l.extend(links);
                });
            }
            if x.errors.with_untracked(|e| e.contains_key(&path)) {
                x.errors.update(|e| {
                    e.remove(&path);
                });
            }
            ui.tree.update(|t| {
                t.insert(path, entries);
            });
            scroll_to_pending(ui);
        }
        // The Explorer's own. Every face is told, so what another face moved or deleted is followed here too;
        // but a file is opened only in the window that asked for it.
        "created" => {
            let path = s("path");
            if !ui.explorer_asked.with_untracked(|a| a.as_deref() == Some(path.as_str())) {
                return false;
            }
            ui.explorer_asked.set(None);
            match m["error"].as_str() {
                Some(e) => said_wrong(ui, &format!("Could not create {path}: {e}")),
                None => {
                    ui.message.set(String::new());
                    if m["dir"] == true {
                        ui.selected.set(path);
                    } else {
                        ui.open(&path, send);
                    }
                }
            }
        }
        "renamed" => {
            let (path, to) = (s("path"), s("to"));
            match m["error"].as_str() {
                Some(e) => said_wrong(ui, &format!("Could not rename {path}: {e}")),
                None => {
                    ui.message.set(String::new());
                    follow(ui, &path, Some(&to));
                    ui.moved(&path, &to);
                }
            }
        }
        "deleted" => {
            let path = s("path");
            match m["error"].as_str() {
                Some(e) => said_wrong(ui, &format!("Could not delete {path}: {e}")),
                None => {
                    follow(ui, &path, None);
                    ui.gone(&path);
                    let kept = m["kept"].as_str().map(str::to_string);
                    ui.message.set(match &kept {
                        Some(kept) => format!("Deleted {path}: it is kept in {kept}"),
                        None => String::new(),
                    });
                    // Deleted from this window: it can be put back from the notification.
                    let mine = x.asked.with_untracked(|a| a.contains(&path));
                    if let (true, Some(kept)) = (mine, kept) {
                        x.asked.update(|a| {
                            a.remove(&path);
                        });
                        let (back, to) = (kept.clone(), path.clone());
                        toast::info_of(&format!("Deleted {path}: it is kept in {kept}"))
                            .source("Explorer")
                            .action("Restore", move |ui, send| {
                                me(ui).asked.update(|a| {
                                    a.insert(to.clone());
                                });
                                send(json!({"type": "restore_deleted", "kept": back, "to": to}));
                            })
                            .show_in(ui);
                    }
                }
            }
        }
        // A copy this window asked for is selected where it went.
        "copied" => {
            let (path, asked) = (s("path"), s("asked"));
            let mine = x.asked.with_untracked(|a| a.contains(&asked));
            if mine {
                x.asked.update(|a| {
                    a.remove(&asked);
                });
            }
            match m["error"].as_str() {
                Some(e) if mine => said_wrong(ui, &format!("Could not copy {path}: {e}")),
                None if mine => select_one(ui, &s("to")),
                _ => {}
            }
        }
        "imported" => {
            let (from, to) = (s("from"), s("to"));
            if !x.asked.with_untracked(|a| a.contains(&to)) {
                return false;
            }
            x.asked.update(|a| {
                a.remove(&to);
            });
            match m["error"].as_str() {
                Some("exists") => {
                    let send = send.clone();
                    let mut asked = dialog::ask(&format!(
                        "A file or folder with the name '{}' already exists in the destination folder. Do you want to replace it?",
                        name_of(&to)
                    ))
                    .detail("What is there now is kept in the backups.")
                    .button("Replace");
                    if let Some(tree) = ui.explorer_tree.get_untracked() {
                        asked = asked.back(tree);
                    }
                    asked.open(ui, move |answer| {
                        if answer.yes() {
                            import(ui, &send, &from, &to, true);
                        }
                    });
                }
                Some(e) => said_wrong(ui, &format!("Could not copy {from}: {e}")),
                None => select_one(ui, &to),
            }
        }
        "restored" => {
            let to = s("to");
            if !x.asked.with_untracked(|a| a.contains(&to)) {
                return false;
            }
            x.asked.update(|a| {
                a.remove(&to);
            });
            match m["error"].as_str() {
                Some(e) => said_wrong(ui, &format!("Could not restore {to}: {e}")),
                None => {
                    ui.message.set(format!("Restored {to}"));
                    reveal(ui, send, &to, true);
                }
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json::Value;

    use super::*;

    fn sender() -> (Send, Rc<RefCell<Vec<Value>>>) {
        let asked = Rc::new(RefCell::new(Vec::new()));
        let to = asked.clone();
        (Rc::new(move |v: Value| to.borrow_mut().push(v)), asked)
    }

    /// src (open: in (closed), a.rs, b.rs), docs (closed), z.txt.
    fn project() -> Ui {
        let ui = Ui::new();
        let e = |name: &str, dir: bool| Entry { name: name.into(), dir };
        ui.tree.set(
            [
                (String::new(), vec![e("docs", true), e("src", true), e("z.txt", false)]),
                ("src".to_string(), vec![e("in", true), e("a.rs", false), e("b.rs", false)]),
                ("src/in".to_string(), vec![e("deep.rs", false)]),
            ]
            .into(),
        );
        ui.expanded.set(["src".to_string()].into());
        ui
    }

    fn shown(ui: Ui) -> Vec<String> {
        rows(ui).into_iter().map(|r| if r.input { format!("[{}]", r.path) } else { r.path }).collect()
    }

    #[test]
    fn what_the_session_could_not_do_is_a_notification_kept_in_the_bells_list() {
        let ui = project();
        let (send, _) = sender();
        assert_eq!(toast::count(ui), 0);
        ui.take(
            &json!({"type": "renamed", "path": "src/a.rs", "to": "src/b.rs", "error": "it is there already"}),
            &send,
        );
        assert_eq!(ui.message.get_untracked(), "Could not rename src/a.rs: it is there already");
        assert_eq!(toast::count(ui), 1);
        ui.take(&json!({"type": "deleted", "path": "z.txt", "error": "permission denied"}), &send);
        assert_eq!(toast::count(ui), 2);
        // What went well says nothing in the corner.
        ui.take(&json!({"type": "renamed", "path": "src/a.rs", "to": "src/c.rs"}), &send);
        ui.take(&json!({"type": "deleted", "path": "z.txt", "kept": ".code-arena/backups/1"}), &send);
        assert_eq!(toast::count(ui), 2);
        assert_eq!(ui.message.get_untracked(), "Deleted z.txt: it is kept in .code-arena/backups/1");
    }

    #[test]
    fn the_input_is_a_row_where_the_new_entry_will_be_or_the_row_renamed() {
        let ui = project();
        let (send, asked) = sender();
        assert_eq!(shown(ui), ["docs", "src", "src/in", "src/a.rs", "src/b.rs", "z.txt"]);
        // A new file: after its folder's folders; a new folder: before them; one level in.
        begin_new(ui, &send, "src", false);
        assert_eq!(shown(ui), ["docs", "src", "src/in", "[src/]", "src/a.rs", "src/b.rs", "z.txt"]);
        assert_eq!(rows(ui)[3].depth, 1);
        begin_new(ui, &send, "src", true);
        assert_eq!(shown(ui), ["docs", "src", "[src/]", "src/in", "src/a.rs", "src/b.rs", "z.txt"]);
        begin_new(ui, &send, "", false);
        assert_eq!(shown(ui), ["docs", "src", "src/in", "src/a.rs", "src/b.rs", "[/]", "z.txt"]);
        assert!(asked.borrow().is_empty(), "the folders were open and listed already");
        // In a closed folder inside a closed one: both are opened, and the one not listed yet is asked for.
        ui.expanded.set(HashSet::new());
        ui.tree.update(|t| {
            t.remove("src/in");
        });
        begin_new(ui, &send, "src/in", false);
        // The folder not listed yet says so under the input.
        assert_eq!(shown(ui), ["docs", "src", "src/in", "[src/in/]", "src/in/\u{1}", "src/a.rs", "src/b.rs", "z.txt"]);
        assert_eq!(*asked.borrow(), [json!({"type": "ls", "path": "src/in"})]);
        // A row renamed is the input itself, with its name in it.
        begin_rename(ui, "src/a.rs");
        assert_eq!(shown(ui), ["docs", "src", "src/in", "src/in/\u{1}", "[src/a.rs]", "src/b.rs", "z.txt"]);
        assert_eq!(ui.explorer_text.get_untracked(), "a.rs");
    }

    #[test]
    fn enter_asks_the_session_and_a_name_that_is_not_valid_stays_in_the_input() {
        let ui = project();
        let (send, asked) = sender();
        begin_new(ui, &send, "src", false);
        ui.explorer_text.set("a.rs".into());
        commit(ui, &send);
        assert!(ui.explorer_edit.get_untracked().is_some() && asked.borrow().is_empty(), "it is there already");
        for bad in ["..", "a/../b", "/a", "a/", "a\\b", ".git/x"] {
            ui.explorer_text.set(bad.into());
            commit(ui, &send);
            assert!(ui.explorer_edit.get_untracked().is_some() && asked.borrow().is_empty(), "{bad}");
        }
        ui.explorer_text.set(" new.rs ".into());
        commit(ui, &send);
        assert_eq!(*asked.borrow(), [json!({"type": "create", "path": "src/new.rs", "dir": false})]);
        assert!(ui.explorer_edit.get_untracked().is_none());
        assert_eq!(ui.explorer_asked.get_untracked().as_deref(), Some("src/new.rs"));
        asked.borrow_mut().clear();
        // A folder, at the top, with a folder on the way.
        begin_new(ui, &send, "", true);
        ui.explorer_text.set("x/y".into());
        commit(ui, &send);
        assert_eq!(*asked.borrow(), [json!({"type": "create", "path": "x/y", "dir": true})]);
        asked.borrow_mut().clear();
        // Renamed: asked with the folder it is in; its own name or no name asks nothing.
        begin_rename(ui, "src/a.rs");
        commit(ui, &send);
        begin_rename(ui, "src/a.rs");
        ui.explorer_text.set(String::new());
        commit(ui, &send);
        assert!(asked.borrow().is_empty() && ui.explorer_edit.get_untracked().is_none());
        begin_rename(ui, "src/a.rs");
        ui.explorer_text.set("b.rs".into());
        commit(ui, &send);
        assert!(ui.explorer_edit.get_untracked().is_some(), "another's name");
        ui.explorer_text.set("c.rs".into());
        commit(ui, &send);
        assert_eq!(*asked.borrow(), [json!({"type": "rename", "path": "src/a.rs", "to": "src/c.rs"})]);
    }

    #[test]
    fn what_is_wrong_with_a_name_is_said_as_vs_code_says_it() {
        let ui = project();
        let new = Edit::New { parent: "src".into(), dir: false };
        let said = |edit: &Edit, text: &str| ui.tree.with_untracked(|t| problem(t, edit, text));
        assert_eq!(said(&new, ""), None);
        assert_eq!(said(&new, "new.rs"), None);
        assert_eq!(
            said(&new, "a.rs").as_deref(),
            Some("A file or folder a.rs already exists at this location. Please choose a different name.")
        );
        assert_eq!(
            said(&new, "..").as_deref(),
            Some("The name .. is not valid as a file or folder name. Please choose a different name.")
        );
        assert_eq!(said(&Edit::Rename { path: "src/a.rs".into() }, "a.rs"), None);
    }

    #[test]
    fn the_arrows_go_through_the_tree_and_open_and_close_its_folders() {
        let ui = project();
        let (send, asked) = sender();
        let go = |nav| {
            navigate(ui, &send, nav);
            ui.selected.get_untracked()
        };
        assert_eq!(go(Nav::Down), "docs", "nothing was selected: the first row");
        assert_eq!(go(Nav::Up), "docs", "no row before the first");
        assert_eq!(go(Nav::Down), "src");
        // Right on an open folder goes into it; on a closed one opens it (listed when it was not yet).
        assert_eq!(go(Nav::Right), "src/in");
        ui.tree.update(|t| {
            t.remove("src/in");
        });
        assert_eq!(go(Nav::Right), "src/in");
        assert!(ui.expanded.with_untracked(|e| e.contains("src/in")));
        assert_eq!(*asked.borrow(), [json!({"type": "ls", "path": "src/in"})]);
        // Left closes it, then goes to the folder it is in.
        assert_eq!(go(Nav::Left), "src/in");
        assert!(!ui.expanded.with_untracked(|e| e.contains("src/in")));
        assert_eq!(go(Nav::Left), "src");
        assert_eq!(go(Nav::Down), "src/in");
        assert_eq!(go(Nav::Down), "src/a.rs");
        assert_eq!(go(Nav::Left), "src", "a file: the folder it is in");
        // Enter opens a file in the editor, and a folder like a click.
        ui.selected.set("z.txt".into());
        asked.borrow_mut().clear();
        assert_eq!(navigate(ui, &send, Nav::Enter), Some(5));
        assert_eq!(*asked.borrow(), [json!({"type": "open", "path": "z.txt"})]);
        assert_eq!(go(Nav::Down), "z.txt", "no row after the last");
        ui.selected.set("src".into());
        navigate(ui, &send, Nav::Enter);
        assert!(!ui.expanded.with_untracked(|e| e.contains("src")));
        // A selection that is not among the rows (in a closed folder): Up starts from the last row.
        ui.selected.set("src/a.rs".into());
        assert_eq!(go(Nav::Up), "z.txt");
        assert_eq!(navigate(Ui::new(), &send, Nav::Down), None, "an empty tree");
    }

    #[test]
    fn a_new_file_goes_in_the_folder_selected_or_the_one_the_selected_file_is_in() {
        let ui = project();
        assert_eq!(folder_of_selected(ui), "");
        ui.selected.set("src".into());
        assert_eq!(folder_of_selected(ui), "src");
        ui.selected.set("src/a.rs".into());
        assert_eq!(folder_of_selected(ui), "src");
        ui.selected.set("z.txt".into());
        assert_eq!(folder_of_selected(ui), "");
    }

    #[test]
    fn rows_are_selected_with_ctrl_shift_and_ctrl_a_and_an_arrow_selects_one_again() {
        let ui = project();
        let (send, asked) = sender();
        let none = Modifiers::empty();
        click(ui, &send, "src/a.rs", false, none);
        assert_eq!(selection(ui), ["src/a.rs"]);
        assert_eq!(*asked.borrow(), [json!({"type": "open", "path": "src/a.rs"})]);
        // Ctrl adds a row (and opens nothing); Ctrl again takes it out.
        click(ui, &send, "z.txt", false, Modifiers::CONTROL);
        assert_eq!(selection(ui), ["src/a.rs", "z.txt"]);
        click(ui, &send, "docs", true, Modifiers::CONTROL);
        assert_eq!(selection(ui), ["docs", "src/a.rs", "z.txt"], "in the tree's order");
        click(ui, &send, "docs", true, Modifiers::CONTROL);
        assert_eq!(selection(ui), ["src/a.rs", "z.txt"]);
        assert_eq!(asked.borrow().len(), 1);
        // Shift: from the last row clicked.
        click(ui, &send, "src/in", true, none);
        click(ui, &send, "src/b.rs", false, Modifiers::SHIFT);
        assert_eq!(selection(ui), ["src/in", "src/in/deep.rs", "src/a.rs", "src/b.rs"], "the click opened src/in");
        click(ui, &send, "src", true, Modifiers::SHIFT);
        assert_eq!(selection(ui), ["src", "src/in"]);
        select_all(ui);
        assert_eq!(selection(ui).len(), 7);
        // An arrow, or the session selecting another row, leaves one.
        navigate(ui, &send, Nav::Down);
        assert_eq!(selection(ui).len(), 1);
        select_all(ui);
        ui.selected.set("elsewhere".into());
        assert_eq!(selection(ui), ["elsewhere"]);
    }

    #[test]
    fn rows_go_in_a_folder_moved_or_copied_but_never_into_themselves() {
        let p = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(outermost(&p(&["src", "src/a.rs", "srcx", "z"])), p(&["src", "srcx", "z"]));
        assert_eq!(
            moves(&p(&["src/a.rs", "z.txt"]), "docs", false),
            [
                json!({"type": "rename", "path": "src/a.rs", "to": "docs/a.rs"}),
                json!({"type": "rename", "path": "z.txt", "to": "docs/z.txt"})
            ]
        );
        assert!(moves(&p(&["src/a.rs"]), "src", false).is_empty(), "already there");
        assert_eq!(moves(&p(&["src/a.rs"]), "src", true), [json!({"type": "copy", "path": "src/a.rs", "to": "src/a.rs"})]);
        assert!(moves(&p(&["src"]), "src/in", true).is_empty() && moves(&p(&["src"]), "src", false).is_empty());
        assert_eq!(moves(&p(&["src/in"]), "", false), [json!({"type": "rename", "path": "src/in", "to": "in"})]);
        assert_eq!(range(&p(&["a", "b", "c", "d"]), "c", "a"), p(&["a", "b", "c"]));
        assert_eq!(range(&p(&["a", "b"]), "gone", "b"), p(&["b"]));
    }

    #[test]
    fn copy_cut_and_paste_ask_the_session_and_follow_what_it_did() {
        let ui = project();
        let (send, asked) = sender();
        click(ui, &send, "src/a.rs", false, Modifiers::empty());
        click(ui, &send, "z.txt", false, Modifiers::CONTROL);
        take_rows(ui, false);
        asked.borrow_mut().clear();
        ui.selected.set("docs".into());
        paste(ui, &send);
        let got = asked.borrow().clone();
        assert!(got.contains(&json!({"type": "copy", "path": "src/a.rs", "to": "docs/a.rs"})), "{got:?}");
        assert!(got.contains(&json!({"type": "ls", "path": "docs"})), "the folder is opened: {got:?}");
        // The copy this window asked for is selected where it went; another window's is not.
        ui.take(&json!({"type": "copied", "path": "src/a.rs", "asked": "docs/a.rs", "to": "docs/a.rs"}), &send);
        assert_eq!(ui.selected.get_untracked(), "docs/a.rs");
        ui.take(&json!({"type": "copied", "path": "x", "asked": "y", "to": "y"}), &send);
        assert_eq!(ui.selected.get_untracked(), "docs/a.rs");
        // Cut: moved, once; what is cut follows a rename made meanwhile.
        click(ui, &send, "src/b.rs", false, Modifiers::empty());
        take_rows(ui, true);
        ui.take(&json!({"type": "renamed", "path": "src", "to": "lib"}), &send);
        ui.selected.set(String::new());
        asked.borrow_mut().clear();
        paste(ui, &send);
        assert_eq!(*asked.borrow(), [json!({"type": "rename", "path": "lib/b.rs", "to": "b.rs"})]);
        assert!(me(ui).clip.get_untracked().is_none());
        // A folder is not pasted into itself.
        ui.selected.set("lib".into());
        take_rows(ui, false);
        ui.selected.set("lib/in".into());
        ui.tree.update(|t| {
            t.insert("lib".into(), vec![Entry { name: "in".into(), dir: true }]);
        });
        asked.borrow_mut().clear();
        paste(ui, &send);
        assert!(asked.borrow().is_empty());
    }

    #[test]
    fn a_folder_being_listed_or_not_listed_says_so_and_a_link_is_marked() {
        let ui = project();
        let (send, _) = sender();
        ui.expanded.update(|e| {
            e.insert("docs".into());
        });
        assert_eq!(rows(ui)[1].note.as_deref(), Some("\u{2026}"));
        ui.take(&json!({"type": "ls", "path": "docs", "error": "not a folder that can be read"}), &send);
        assert_eq!(rows(ui)[1].note.as_deref(), Some("not a folder that can be read"));
        ui.take(&json!({"type": "ls", "path": "docs", "entries": [{"name": "up", "dir": true, "link": true}]}), &send);
        let r = &rows(ui)[1];
        assert_eq!((r.path.as_str(), r.link, r.note.is_none()), ("docs/up", true, true));
        // The arrows pass the notes over.
        ui.expanded.update(|e| {
            e.insert("docs/up".into());
        });
        ui.selected.set("docs/up".into());
        assert_eq!(navigate(ui, &send, Nav::Down), Some(3));
        assert_eq!(ui.selected.get_untracked(), "src");
    }

    #[test]
    fn a_drop_goes_in_the_folder_under_the_pointer() {
        let ui = project();
        let rows = rows_now(ui);
        let dir = |_: &str| false;
        assert_eq!(drop_folder(&rows, 1.0, dir), "src");
        assert_eq!(drop_folder(&rows, 3.0, dir), "src", "a file: the folder it is in");
        assert_eq!(drop_folder(&rows, 5.0, dir), "");
        assert_eq!(drop_folder(&rows, 40.0, dir), "", "below the rows: the top");
        ui.expanded.update(|e| {
            e.insert("docs".into());
        });
        assert_eq!(drop_folder(&rows_now(ui), 1.0, dir), "docs", "the note of a folder being listed");
    }

    #[test]
    fn a_file_is_revealed_its_folders_opened_and_listed() {
        let ui = project();
        let (send, asked) = sender();
        ui.tree.update(|t| {
            t.insert("docs".into(), vec![Entry { name: "guide".into(), dir: true }]);
        });
        reveal(ui, &send, "docs/guide/a.md", true);
        assert!(ui.expanded.with_untracked(|e| e.contains("docs") && e.contains("docs/guide")));
        assert_eq!(*asked.borrow(), [json!({"type": "ls", "path": "docs/guide"})]);
        assert_eq!(ui.selected.get_untracked(), "docs/guide/a.md");
        assert!(me(ui).pending.get_untracked().is_some(), "not a row yet");
        ui.take(&json!({"type": "ls", "path": "docs/guide", "entries": [{"name": "a.md", "dir": false}]}), &send);
        assert!(me(ui).pending.get_untracked().is_none());
        assert_eq!(me(ui).reveal.get_untracked(), 2.0 * ROW);
    }

    #[test]
    fn what_this_window_deleted_can_be_restored_from_its_notification() {
        let ui = project();
        let (send, _) = sender();
        me(ui).asked.update(|a| {
            a.insert("z.txt".into());
        });
        ui.take(&json!({"type": "deleted", "path": "z.txt", "kept": "/k/1/z.txt"}), &send);
        assert_eq!(toast::count(ui), 1);
        ui.take(&json!({"type": "deleted", "path": "src/a.rs", "kept": "/k/2/src/a.rs"}), &send);
        assert_eq!(toast::count(ui), 1, "another window's delete");
    }

    #[test]
    fn each_kind_of_file_has_its_icon() {
        assert_eq!(icon_of("main.rs", false, false), "file-code");
        assert_eq!(icon_of("package.json", false, false), "symbol-namespace");
        assert_eq!(icon_of("run.sh", false, false), "terminal");
        assert_eq!(icon_of("logo.PNG", false, false), "file-media");
        assert_eq!(icon_of("Cargo.toml", false, false), "settings-gear");
        assert_eq!(icon_of("README.md", false, false), "file-text");
        assert_eq!(icon_of("src", true, true), "folder-opened");
        for name in ["file-code", "symbol-namespace", "terminal", "file-media", "settings-gear", "file-text", "folder"] {
            assert!(crate::icons::known(name), "{name}");
        }
    }
}
