//! The Source Control view, as VS Code's: the commit box (its message, then the Commit button with Commit (Amend)
//! beside it), then the repository's changes in three groups, Merge Changes, Staged Changes and Changes, each with
//! its count. A file's row has its name, its folder dimmed and its letter in git's colours (a deleted file's name
//! struck through); while the pointer is on a row or a group its actions show: Open File, Stage, Unstage, Discard.
//! A click opens what changed: the working tree against the index, or (staged) the index against HEAD, in a tab of
//! its own; a file in a merge's conflict opens as it is. The view's icon has the count of changed files, and the
//! status bar the branch.
//!
//! What the driver says of the repository (`git_status`) is asked when the window has the session, again after a
//! file is saved, made, moved or deleted, after a turn, and when Refresh is pressed; the driver also tells it by
//! itself when git's files change (a commit made in a terminal). What is shown is set only when it changed, so a
//! status told again as it was costs nothing. What is discarded is asked about first; a file git does not track is
//! deleted as the Explorer deletes, kept aside.
//!
//! What every part has (`register`, `take`) is said in `ext.rs`; what a view has (`READY`, `view`, `actions`), in
//! `layout.rs`.

use std::collections::HashMap;
use std::rc::Rc;

use floem::keyboard::{Key, NamedKey};
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo, create_rw_signal};
use floem::style::CursorStyle;
use floem::views::{Decorators, container, dyn_container, empty, h_stack, label, stack, text_input, v_stack};
use floem::{IntoView, View as _};
use serde_json::{Value, json};

use crate::colors::Colors;
use crate::diff::Line;
use crate::icons::icon;
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::widgets::tree as wt;
use crate::widgets::{dialog, popup, toast};

pub mod branches;
pub mod history;
pub mod quick_diff;
pub mod repos;
pub mod tree;

/// Whether the view is in the window.
pub const READY: bool = true;

/// The tab kind of a file's changes: `scm:w:<path>` (the working tree against the index) or `scm:i:<path>` (the
/// index against HEAD).
const KIND: &str = "scm";

/// One changed file, as the driver says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct File {
    pub path: String,
    pub from: Option<String>,
    pub x: char,
    pub y: char,
    pub staged: bool,
    pub changed: bool,
    pub untracked: bool,
    pub conflict: bool,
}

/// The repository's state, as the driver says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// Where git's top is; none: the folder has no repository.
    pub root: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub files: Vec<File>,
    pub total: usize,
    pub error: Option<String>,
}

impl Status {
    pub fn of(m: &Value) -> Status {
        let s = |v: &Value| v.as_str().map(str::to_string);
        let letter = |v: &Value| v.as_str().and_then(|t| t.chars().next()).unwrap_or(' ');
        let files: Vec<File> = m["files"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| File {
                path: f["path"].as_str().unwrap_or("").to_string(),
                from: s(&f["from"]),
                x: letter(&f["x"]),
                y: letter(&f["y"]),
                staged: f["staged"].as_bool().unwrap_or(false),
                changed: f["changed"].as_bool().unwrap_or(false),
                untracked: f["untracked"].as_bool().unwrap_or(false),
                conflict: f["conflict"].as_bool().unwrap_or(false),
            })
            .collect();
        Status {
            root: s(&m["root"]),
            branch: s(&m["branch"]),
            upstream: s(&m["upstream"]),
            ahead: m["ahead"].as_u64().unwrap_or(0),
            behind: m["behind"].as_u64().unwrap_or(0),
            total: m["total"].as_u64().map(|n| n as usize).unwrap_or(files.len()),
            files,
            error: s(&m["error"]).filter(|e| !e.is_empty()),
        }
    }

    /// The files of a group, in the order git gave them.
    pub fn of_group(&self, g: Group) -> Vec<&File> {
        self.files.iter().filter(|f| g.has(f)).collect()
    }
}

/// The groups of changes, in the order they are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Merge,
    Staged,
    Changes,
}

pub const GROUPS: [Group; 3] = [Group::Merge, Group::Staged, Group::Changes];

impl Group {
    pub fn id(self) -> &'static str {
        match self {
            Group::Merge => "merge",
            Group::Staged => "staged",
            Group::Changes => "changes",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Group::Merge => "Merge Changes",
            Group::Staged => "Staged Changes",
            Group::Changes => "Changes",
        }
    }

    fn of_id(id: &str) -> Option<Group> {
        GROUPS.into_iter().find(|g| g.id() == id)
    }

    /// Whether a file is in it (one changed again after it was staged is in Staged and in Changes).
    pub fn has(self, f: &File) -> bool {
        match self {
            Group::Merge => f.conflict,
            Group::Staged => f.staged,
            Group::Changes => f.changed || f.untracked,
        }
    }

    /// VS Code's letter for a file in it.
    pub fn letter(self, f: &File) -> char {
        match self {
            Group::Merge => match (f.x, f.y) {
                ('U', 'U') => 'C',
                ('A', _) | (_, 'A') => 'A',
                _ => 'D',
            },
            Group::Staged => match f.x {
                'T' => 'M',
                c => c,
            },
            Group::Changes if f.untracked => 'U',
            Group::Changes => match f.y {
                'T' => 'M',
                c => c,
            },
        }
    }
}

/// git's colour for a letter.
fn letter_colour(c: &Colors, letter: char) -> Color {
    match letter {
        'A' => c.added_fg,
        'D' => c.removed_fg,
        'U' => c.scm_untracked,
        'R' => c.scm_renamed,
        'C' => c.scm_conflict,
        _ => c.scm_modified,
    }
}

/// A row's id: a group's (`g:changes`), or a file's in a group (`f:changes:src/a.rs`).
fn group_row(g: Group) -> String {
    format!("g:{}", g.id())
}

fn file_row(g: Group, path: &str) -> String {
    format!("f:{}:{path}", g.id())
}

/// What a row's id names.
#[derive(Debug, PartialEq)]
enum Row<'a> {
    Group(Group),
    File(Group, &'a str),
}

fn row_of(id: &str) -> Option<Row<'_>> {
    if let Some(g) = id.strip_prefix("g:") {
        return Group::of_id(g).map(Row::Group);
    }
    let (g, path) = id.strip_prefix("f:")?.split_once(':')?;
    Group::of_id(g).map(|g| Row::File(g, path))
}

/// The rows shown: each group that has files (Changes also when it has none, as VS Code's), its files under it
/// unless it is folded.
pub fn rows(s: &Status, folded: &[Group]) -> im::Vector<wt::Node> {
    let mut out = im::Vector::new();
    for g in GROUPS {
        let files = s.of_group(g);
        if files.is_empty() && g != Group::Changes {
            continue;
        }
        let open = !folded.contains(&g);
        let mut head = wt::folder(&group_row(g), 0, open);
        head.stamp = files.len() as u64;
        out.push_back(head);
        if open {
            for f in files {
                let mut n = wt::leaf(&file_row(g, &f.path), 1);
                n.stamp = u64::from(g.letter(f)) << 8 | u64::from(f.from.is_some());
                out.push_back(n);
            }
        }
    }
    out
}

/// The paths git is given for files; with `from`, a renamed file's old path too (to unstage both halves: it is
/// neither in the index nor on disk, so staging it would fail).
fn paths_of(files: &[&File], from: bool) -> Vec<String> {
    let mut out = Vec::new();
    for f in files {
        out.push(f.path.clone());
        if let (true, Some(old)) = (from, &f.from) {
            out.push(old.clone());
        }
    }
    out
}

/// What a changes tab compares: the side before and the side after, each `(rev, working, path)` as `git_show`
/// takes it; `None`: the side has no file (added, deleted).
type Side = Option<(String, bool, String)>;

fn sides(s: &Status, rest: &str) -> Option<(Side, Side)> {
    let (how, path) = rest.split_once(':')?;
    let f = s.files.iter().find(|f| f.path == path);
    let at = |rev: &str, working: bool, p: &str| Some((rev.to_string(), working, p.to_string()));
    match how {
        // The working tree against the index: an untracked or newly added file has no side before.
        "w" => {
            let before = if f.is_some_and(|f| f.untracked) { None } else { at("", false, path) };
            let after = if f.is_some_and(|f| f.y == 'D') { None } else { at("", true, path) };
            Some((before, after))
        }
        // The index against HEAD: an added file has no HEAD side, a deleted one no index side.
        "i" => {
            let old = f.and_then(|f| f.from.clone()).unwrap_or_else(|| path.to_string());
            let before = if f.is_some_and(|f| f.x == 'A') { None } else { at("HEAD", false, &old) };
            let after = if f.is_some_and(|f| f.x == 'D') { None } else { at("", false, path) };
            Some((before, after))
        }
        _ => None,
    }
}

/// A changes tab's title, as VS Code's: `a.rs (Working Tree)`, `a.rs (Index)`.
pub fn title_of(rest: &str) -> String {
    let (how, path) = rest.split_once(':').unwrap_or(("w", rest));
    let name = path.rsplit('/').next().unwrap_or(path);
    format!("{name} ({})", if how == "i" { "Index" } else { "Working Tree" })
}

fn shown_key(rev: &str, working: bool, path: &str) -> String {
    format!("{rev}|{working}|{path}")
}

/// A file's side as the driver sent it.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    Text(Rc<str>),
    Missing,
    Binary,
    TooLarge,
}

impl Shown {
    fn of(m: &Value) -> Shown {
        if m["too_large"].as_bool() == Some(true) {
            Shown::TooLarge
        } else if m["binary"].as_bool() == Some(true) {
            Shown::Binary
        } else if m["exists"].as_bool() == Some(true) {
            Shown::Text(m["text"].as_str().unwrap_or("").into())
        } else {
            Shown::Missing
        }
    }
}

/// A changes tab's content, once both sides have come: its lines (worked out off the window's thread), or why it
/// has none.
#[derive(Clone)]
pub enum Compared {
    Lines(Rc<Vec<Line>>),
    Note(&'static str),
}

impl PartialEq for Compared {
    fn eq(&self, o: &Compared) -> bool {
        match (self, o) {
            (Compared::Lines(a), Compared::Lines(b)) => Rc::ptr_eq(a, b),
            (Compared::Note(a), Compared::Note(b)) => a == b,
            _ => false,
        }
    }
}

/// The view's state, one per window.
#[derive(Clone, Copy)]
pub struct Scm {
    pub status: RwSignal<Option<Status>>,
    pub message: RwSignal<String>,
    folded: RwSignal<Vec<Group>>,
    tree: wt::State,
    /// A commit is being made.
    busy: RwSignal<bool>,
    /// Each file's side asked for, by `shown_key`.
    shown: RwSignal<HashMap<String, Shown>>,
    /// Each changes tab's content, by the tab's path.
    compared: RwSignal<HashMap<String, Compared>>,
    /// How many times a status was asked for a moment later (only the last is sent).
    asks: RwSignal<u64>,
    /// The view drawn, where the title's menus open.
    root: RwSignal<Option<floem::ViewId>>,
}

impl crate::ext::State for Scm {
    fn new(ui: Ui) -> Scm {
        Scm {
            status: ui.scope.create_rw_signal(None),
            message: ui.scope.create_rw_signal(String::new()),
            folded: ui.scope.create_rw_signal(Vec::new()),
            tree: wt::State::new(ui),
            busy: ui.scope.create_rw_signal(false),
            shown: ui.scope.create_rw_signal(HashMap::new()),
            compared: ui.scope.create_rw_signal(HashMap::new()),
            asks: ui.scope.create_rw_signal(0),
            root: ui.scope.create_rw_signal(None),
        }
    }
}

fn state(ui: Ui) -> Scm {
    ui.state::<Scm>()
}

/// The repository's status asked for now.
fn refresh(send: &Send) {
    send(json!({"type": "git_status", "repo": ""}));
}

/// The status asked for a moment from now: several reasons at once ask once.
fn refresh_soon(ui: Ui, send: &Send) {
    let me = state(ui);
    me.asks.update(|n| *n += 1);
    let n = me.asks.get_untracked();
    let send = send.clone();
    floem::action::exec_after(std::time::Duration::from_millis(300), move |_| {
        if me.asks.get_untracked() == n {
            refresh(&send);
        }
    });
}

fn files_of(ui: Ui, g: Group) -> Vec<File> {
    state(ui)
        .status
        .with_untracked(|s| s.as_ref().map(|s| s.of_group(g).into_iter().cloned().collect()).unwrap_or_default())
}

fn stage(send: &Send, files: &[File]) {
    if !files.is_empty() {
        send(json!({"type": "git_stage", "repo": "", "paths": paths_of(&files.iter().collect::<Vec<_>>(), false)}));
    }
}

fn unstage(send: &Send, files: &[File]) {
    if !files.is_empty() {
        send(json!({"type": "git_unstage", "repo": "", "paths": paths_of(&files.iter().collect::<Vec<_>>(), true)}));
    }
}

fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The question asked before changes are discarded: its message, detail and button.
pub fn discard_question(files: &[File]) -> (String, &'static str, String) {
    match files {
        [f] if f.untracked => (
            format!("Are you sure you want to delete {}?", name_of(&f.path)),
            "It is kept aside with the files the Explorer deletes, where it can be got back.",
            "Delete File".to_string(),
        ),
        [f] => (
            format!("Are you sure you want to discard changes in {}?", name_of(&f.path)),
            "This cannot be undone.",
            "Discard File".to_string(),
        ),
        many => (
            format!("Are you sure you want to discard ALL changes in {} files?", many.len()),
            "This cannot be undone. Files git does not track are kept aside.",
            format!("Discard All {} Files", many.len()),
        ),
    }
}

fn discard(ui: Ui, send: &Send, files: Vec<File>) {
    if files.is_empty() {
        return;
    }
    let (message, detail, button) = discard_question(&files);
    let send = send.clone();
    let mut ask = dialog::ask(&message).detail(detail).kind(dialog::Kind::Warning).button(&button).destructive();
    if let Some(back) = state(ui).tree.view() {
        ask = ask.back(back);
    }
    ask.open(ui, move |a| {
        if a.yes() {
            send(
                json!({"type": "git_discard", "repo": "", "paths": paths_of(&files.iter().collect::<Vec<_>>(), false)}),
            );
        }
    });
}

/// The commit asked of the driver: what is staged, or every change when nothing is. False: it was not asked (one
/// is being made, there is nothing to commit, no message).
pub(crate) fn commit(ui: Ui, send: &Send, amend: bool) -> bool {
    let me = state(ui);
    if me.busy.get_untracked() {
        return false;
    }
    let (staged, any) = me.status.with_untracked(|s| {
        s.as_ref().map(|s| (s.of_group(Group::Staged).len(), !s.files.is_empty())).unwrap_or((0, false))
    });
    if staged == 0 && !any && !amend {
        toast::info(ui, "There are no changes to commit.");
        return false;
    }
    let message = me.message.get_untracked();
    if message.trim().is_empty() && !amend {
        toast::warning(ui, "Type a message to commit.");
        return false;
    }
    me.busy.set(true);
    send(json!({"type": "git_commit", "repo": "", "message": message, "all": staged == 0 && !amend, "amend": amend}));
    true
}

/// The view drawn, where the title's menus open.
pub(crate) fn view_id(ui: Ui) -> Option<floem::ViewId> {
    state(ui).root.get_untracked()
}

/// A file's row opened: its changes in a tab, or the file itself (one git does not track yet, one in conflict).
fn open_row(ui: Ui, send: &Send, id: &str) {
    let Some(Row::File(g, path)) = row_of(id) else { return };
    let f =
        state(ui).status.with_untracked(|s| s.as_ref().and_then(|s| s.files.iter().find(|f| f.path == path).cloned()));
    let Some(f) = f else { return };
    match g {
        Group::Merge => ui.open(path, send),
        Group::Changes if f.untracked => ui.open(path, send),
        Group::Changes => crate::editor::kinds::open(ui, &format!("{KIND}:w:{path}")),
        Group::Staged => crate::editor::kinds::open(ui, &format!("{KIND}:i:{path}")),
    }
}

/// A changes tab's sides asked of the driver.
fn ask_sides(ui: Ui, send: &Send, rest: &str) {
    let Some((before, after)) = state(ui).status.with_untracked(|s| s.as_ref().and_then(|s| sides(s, rest))) else {
        return;
    };
    for (rev, working, path) in [before, after].into_iter().flatten() {
        send(json!({"type": "git_show", "repo": "", "path": path, "rev": rev, "working": working}));
    }
    compare(ui, rest);
}

/// A changes tab's lines worked out, off the window's thread, once both its sides have come.
fn compare(ui: Ui, rest: &str) {
    let me = state(ui);
    let Some((before, after)) = me.status.with_untracked(|s| s.as_ref().and_then(|s| sides(s, rest))) else { return };
    let side = |s: &Side| -> Option<Shown> {
        match s {
            None => Some(Shown::Missing),
            Some((rev, working, path)) => me.shown.with_untracked(|m| m.get(&shown_key(rev, *working, path)).cloned()),
        }
    };
    let (Some(a), Some(b)) = (side(&before), side(&after)) else { return };
    let tab = format!("{KIND}:{rest}");
    let text = |s: &Shown| match s {
        Shown::Text(t) => Ok(t.to_string()),
        Shown::Missing => Ok(String::new()),
        Shown::Binary => Err("The file is not shown because it is binary."),
        Shown::TooLarge => Err("The file is not shown because it is too large."),
    };
    let (a, b) = match (text(&a), text(&b)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            if me.compared.with_untracked(|c| c.get(&tab) != Some(&Compared::Note(e))) {
                me.compared.update(|c| {
                    c.insert(tab, Compared::Note(e));
                });
            }
            return;
        }
    };
    // Set only when the lines are others: the tab is drawn again (and scrolled to its top) only then.
    let done = floem::ext_event::create_ext_action(ui.scope, move |lines: Vec<Line>| {
        let same = me.compared.with_untracked(|c| matches!(c.get(&tab), Some(Compared::Lines(was)) if **was == lines));
        if !same {
            me.compared.update(|c| {
                c.insert(tab, Compared::Lines(Rc::new(lines)));
            });
        }
    });
    std::thread::spawn(move || done(crate::diff::lines(&a, &b)));
}

/// What a changes tab draws.
fn changes_tab(ui: Ui, send: Send, path: String) -> floem::AnyView {
    let t = ui.colors;
    let me = state(ui);
    let rest = path.strip_prefix(&format!("{KIND}:")).unwrap_or("").to_string();
    ask_sides(ui, &send, &rest);
    let key = path.clone();
    let now = create_memo(move |_| me.compared.with(|c| c.get(&key).cloned()));
    dyn_container(
        move || now.get(),
        move |c| match c {
            Some(Compared::Lines(lines)) if lines.is_empty() => note(ui, "No changes."),
            Some(Compared::Lines(lines)) => crate::changes::diff_view(ui, lines).into_any(),
            Some(Compared::Note(n)) => note(ui, n),
            None => empty().into_any(),
        },
    )
    .style(move |s| s.size_full().background(t.get().editor_bg))
    .into_any()
}

fn note(ui: Ui, text: &'static str) -> floem::AnyView {
    let t = ui.colors;
    label(move || text).style(move |s| s.padding(20.0).color(t.get().desc)).into_any()
}

/// The count at a group's right, as VS Code's.
fn count(t: crate::widgets::Theme, n: usize) -> impl IntoView {
    label(move || n.to_string()).style(move |s| {
        let c = t.get();
        s.font_size(11.0)
            .min_width(18.0)
            .height(18.0)
            .padding_horiz(5.0)
            .border_radius(9.0)
            .justify_center()
            .items_center()
            .margin_right(10.0)
            .background(c.scm_count_bg)
            .color(c.scm_count_fg)
    })
}

/// A row's content: a group's title, or a file's icon, name (struck through when deleted) and folder.
fn row_content(ui: Ui, node: &wt::Node) -> floem::AnyView {
    let t = ui.colors;
    let me = state(ui);
    match row_of(&node.id) {
        Some(Row::Group(g)) => {
            label(move || g.title()).style(|s| s.width_full().font_bold().text_ellipsis().min_width(0.0)).into_any()
        }
        Some(Row::File(g, path)) => {
            let f =
                me.status.with_untracked(|s| s.as_ref().and_then(|s| s.files.iter().find(|f| f.path == path).cloned()));
            let Some(f) = f else { return empty().into_any() };
            let letter = g.letter(&f);
            let deleted = letter == 'D';
            let name = name_of(&f.path).to_string();
            let dir = match (f.path.rsplit_once('/'), &f.from) {
                (_, Some(from)) => format!("{from} →"),
                (Some((d, _)), None) => d.to_string(),
                (None, None) => String::new(),
            };
            let name_view = stack((
                label(move || name.clone()).style(move |s| {
                    let c = t.get();
                    s.color(letter_colour(&c, letter))
                }),
                // VS Code strikes a deleted file's name through.
                empty().style(move |s| {
                    let c = t.get();
                    s.absolute()
                        .inset_left(0.0)
                        .inset_right(0.0)
                        .inset_top(9.0)
                        .height(1.0)
                        .background(letter_colour(&c, letter))
                        .apply_if(!deleted, |s| s.hide())
                }),
            ))
            .style(|s| s.flex_shrink(0.0_f32));
            h_stack((
                icon("file", 16.0).style(move |s| s.color(t.get().icon).flex_shrink(0.0_f32)),
                name_view,
                label(move || dir.clone()).style(move |s| {
                    s.font_size(12.0).color(t.get().desc).text_ellipsis().min_width(0.0).flex_shrink(1.0_f32)
                }),
            ))
            .style(|s| s.width_full().min_width(0.0).items_center().gap(6.0))
            .into_any()
        }
        None => empty().into_any(),
    }
}

/// What a row shows at its right, right of its actions: a group's count, a file's letter. In one column whatever
/// the names' widths, and still while the actions are shown, as VS Code's.
fn row_decoration(ui: Ui, node: &wt::Node) -> floem::AnyView {
    let t = ui.colors;
    let me = state(ui);
    match row_of(&node.id) {
        Some(Row::Group(g)) => {
            let n = me.status.with_untracked(|s| s.as_ref().map(|s| s.of_group(g).len()).unwrap_or(0));
            container(count(t, n)).style(|s| s.margin_right(8.0)).into_any()
        }
        Some(Row::File(g, path)) => {
            let f =
                me.status.with_untracked(|s| s.as_ref().and_then(|s| s.files.iter().find(|f| f.path == path).cloned()));
            let Some(f) = f else { return empty().into_any() };
            let letter = g.letter(&f);
            label(move || letter.to_string())
                .style(move |s| {
                    let c = t.get();
                    s.font_size(12.0).width(12.0).margin_right(10.0).color(letter_colour(&c, letter))
                })
                .into_any()
        }
        None => empty().into_any(),
    }
}

/// A row's actions, as VS Code's.
fn row_actions(ui: Ui, send: &Send, node: &wt::Node) -> Vec<wt::Action> {
    let s = send.clone();
    match row_of(&node.id) {
        Some(Row::Group(g)) => {
            let files = move || files_of(ui, g);
            match g {
                Group::Merge => vec![wt::act("add", "Stage All Merge Changes", move || stage(&s, &files()))],
                Group::Staged => vec![wt::act("remove", "Unstage All Changes", move || unstage(&s, &files()))],
                Group::Changes => {
                    let d = s.clone();
                    vec![
                        wt::act("discard", "Discard All Changes", move || discard(ui, &d, files())),
                        wt::act("add", "Stage All Changes", move || stage(&s, &files())),
                    ]
                }
            }
        }
        Some(Row::File(g, path)) => {
            let path = path.to_string();
            let file = {
                let path = path.clone();
                move || {
                    state(ui).status.with_untracked(|st| {
                        st.as_ref()
                            .and_then(|st| st.files.iter().find(|f| f.path == path).cloned())
                            .into_iter()
                            .collect::<Vec<_>>()
                    })
                }
            };
            let open = {
                let (s, path) = (s.clone(), path.clone());
                wt::act("go-to-file", "Open File", move || ui.open(&path, &s))
            };
            match g {
                Group::Merge => vec![open, wt::act("add", "Stage Changes", move || stage(&s, &file()))],
                Group::Staged => vec![open, wt::act("remove", "Unstage Changes", move || unstage(&s, &file()))],
                Group::Changes => {
                    let (d, f2) = (s.clone(), file.clone());
                    vec![
                        open,
                        wt::act("discard", "Discard Changes", move || discard(ui, &d, f2())),
                        wt::act("add", "Stage Changes", move || stage(&s, &file())),
                    ]
                }
            }
        }
        None => Vec::new(),
    }
}

/// The message box. Its placeholder names the branch, so it is made again when the branch is another.
fn message_box(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let me = state(ui);
    let placeholder = create_memo(move |_| match me.status.with(|s| s.as_ref().and_then(|s| s.branch.clone())) {
        Some(b) => format!("Message (Ctrl+Enter commits on {b})"),
        None => "Message (Ctrl+Enter commits)".to_string(),
    });
    dyn_container(
        move || placeholder.get(),
        move |words| {
            let focused = create_rw_signal(false);
            let go = send.clone();
            // The border and padding sit on the box that holds both the input and its placeholder.
            // floem paints its own placeholder past the box's edge, so the words are a label under the input that
            // ends in an ellipsis, hidden once something is typed.
            let hint =
                container(label(move || words.clone()).style(|s| s.text_ellipsis().min_width(0.0))).style(move |s| {
                    s.absolute()
                        .inset(0.0)
                        .padding_horiz(6.0)
                        .items_center()
                        .color(t.get().desc)
                        .apply_if(me.message.with(|m| !m.is_empty()), |s| s.hide())
                });
            let input = text_input(me.message)
                .on_key_down(Key::Named(NamedKey::Enter), |m| m.control(), move |_| {
                    commit(ui, &go, false);
                })
                .on_event_cont(floem::event::EventListener::FocusGained, move |_| focused.set(true))
                .on_event_cont(floem::event::EventListener::FocusLost, move |_| focused.set(false))
                .style(move |s| {
                    let c = t.get();
                    // A width in percent: with none, floem gives the input a fixed width of so many characters.
                    let s = s
                        .width_full()
                        .min_width(0.0)
                        .border(0.0)
                        .padding(0.0)
                        .background(Color::TRANSPARENT)
                        .color(c.input_fg)
                        .cursor_color(floem::peniko::Brush::Solid(c.cursor));
                    crate::widgets::steady_input(s, Color::TRANSPARENT, None)
                });
            stack((hint, input))
                .style(move |s| {
                    let c = t.get();
                    s.width_full()
                        .min_width(0.0)
                        .height(26.0)
                        .items_center()
                        .padding_horiz(6.0)
                        .border(1.0)
                        .border_radius(2.0)
                        .border_color(if focused.get() { c.focus } else { c.input_border })
                        .background(c.input_bg)
                })
                .into_any()
        },
    )
    .style(|s| s.width_full().min_width(0.0))
}

/// The commit box: the message, then the Commit button and its menu (Commit, Commit (Amend)).
fn commit_box(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let me = state(ui);
    let words = create_memo(move |_| {
        let n = me.status.with(|s| s.as_ref().map(|s| s.of_group(Group::Staged).len()).unwrap_or(0));
        if me.busy.get() {
            "Committing…".to_string()
        } else if n > 0 {
            format!("Commit {n} staged")
        } else {
            "Commit all".to_string()
        }
    });
    let c1 = send.clone();
    let main = h_stack((icon("check", 16.0), label(move || words.get())))
        .on_click_stop(move |_| {
            commit(ui, &c1, false);
        })
        .style(move |s| {
            let c = t.get();
            s.flex_grow(1.0_f32)
                .height_full()
                .gap(6.0)
                .items_center()
                .justify_center()
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.button_hover))
        });
    let more = container(icon("chevron-down", 16.0));
    let more_id = more.id();
    let c2 = send.clone();
    let more = more
        .on_click_stop(move |_| {
            let (a, b) = (c2.clone(), c2.clone());
            let mut rows: Vec<popup::Item> = vec![
                popup::entry("Commit", move || {
                    commit(ui, &a, false);
                })
                .into(),
                popup::entry("Commit (Amend)", move || {
                    commit(ui, &b, true);
                })
                .into(),
            ];
            rows.extend(branches::commit_rows(ui, &c2));
            popup::open(ui, popup::At::under(more_id), rows);
        })
        .style(move |s| {
            let c = t.get();
            s.width(26.0)
                .height_full()
                .items_center()
                .justify_center()
                .cursor(CursorStyle::Pointer)
                .border_left(1.0)
                .border_color(c.button_fg.multiply_alpha(0.4))
                .hover(|s| s.background(c.button_hover))
        });
    let button = h_stack((main, more)).style(move |s| {
        let c = t.get();
        s.width_full()
            .height(26.0)
            .border_radius(2.0)
            .background(c.button_bg)
            .color(c.button_fg)
            .font_size(13.0)
            .selectable(false)
            .apply_if(me.busy.get(), |s| s.color(c.button_fg.multiply_alpha(0.6)))
    });
    v_stack((message_box(ui, send.clone()), button, branches::sync_button(ui, send))).style(|s| s.width_full().gap(6.0).padding_horiz(12.0).padding_vert(6.0))
}

/// What the view says instead of its changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Empty {
    /// No status has come yet.
    Waiting,
    /// The folder has no repository (Initialize Repository is offered).
    NoRepo,
    /// Git could not say (it is not installed, the repository is broken): what it said.
    Failed,
    /// Nothing changed.
    Clean,
    /// There are changes: the groups are shown.
    Changes,
}

pub fn empty_of(s: Option<&Status>) -> Empty {
    match s {
        None => Empty::Waiting,
        Some(s) if s.error.is_some() && (s.root.is_none() || s.files.is_empty()) => Empty::Failed,
        Some(s) if s.root.is_none() => Empty::NoRepo,
        Some(s) if s.files.is_empty() => Empty::Clean,
        Some(_) => Empty::Changes,
    }
}

/// The view's own content.
pub fn view(ui: Ui, send: Send) -> floem::AnyView {
    let t = ui.colors;
    let me = state(ui);
    let what = create_memo(move |_| me.status.with(|s| empty_of(s.as_ref())));
    let rows_now =
        create_memo(move |_| me.status.with(|s| s.as_ref().map(|s| rows(s, &me.folded.get())).unwrap_or_default()));
    let (acting, opened) = (send.clone(), send.clone());
    let changes = wt::tree(ui, me.tree, move || rows_now.get())
        .content(move |n| row_content(ui, n))
        .decoration(move |n| row_decoration(ui, n))
        .actions(move |n| row_actions(ui, &acting, n))
        .multi()
        .on_toggle(move |id| {
            if let Some(Row::Group(g)) = row_of(id) {
                me.folded.update(|f| match f.iter().position(|x| *x == g) {
                    Some(at) => {
                        f.remove(at);
                    }
                    None => f.push(g),
                });
            }
        })
        .on_open(move |id| open_row(ui, &opened, id))
        .view()
        .style(move |s| {
            s.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_height(0.0)
                .width_full()
                .apply_if(what.get() != Empty::Changes, |s| s.hide())
        });
    let init = send.clone();
    let words = dyn_container(
        move || what.get(),
        move |w| {
            let say = move |text: String| {
                label(move || text.clone())
                    .style(move |s| s.padding_horiz(20.0).padding_vert(6.0).line_height(1.4).color(t.get().desc))
            };
            match w {
                Empty::NoRepo => {
                    let init = init.clone();
                    v_stack((
                        say("The folder currently open doesn't have a git repository. You can initialize a \
                             repository which will enable source control features powered by git."
                            .to_string()),
                        container(crate::widgets::button(t, "Initialize Repository", true, move || {
                            init(json!({"type": "git_init", "repo": ""}))
                        }))
                        .style(|s| s.padding_horiz(20.0)),
                    ))
                    .style(|s| s.width_full().gap(6.0))
                    .into_any()
                }
                Empty::Failed => {
                    say(me.status.with_untracked(|s| s.as_ref().and_then(|s| s.error.clone()).unwrap_or_default()))
                        .into_any()
                }
                Empty::Clean => say("There are no changes.".to_string()).into_any(),
                Empty::Waiting | Empty::Changes => empty().into_any(),
            }
        },
    )
    .style(|s| s.width_full());
    let boxed = commit_box(ui, send)
        .style(move |s| s.apply_if(!matches!(what.get(), Empty::Clean | Empty::Changes), |s| s.hide()));
    let root = v_stack((boxed, words, changes)).style(|s| s.size_full().min_height(0.0));
    me.root.set(Some(root.id()));
    root.into_any()
}

/// The actions in its title: they are the registry's (`menus::add` under the view's title), so none here.
pub fn actions(_ui: Ui, _send: Send) -> Option<floem::AnyView> {
    None
}

/// Whether the folder has a repository.
fn has_repo(ui: Ui) -> bool {
    state(ui).status.with(|s| s.as_ref().is_some_and(|s| s.root.is_some()))
}

/// Its commands, its title's actions and its kind of tab.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{Menu, item};
    add(command("git.commit", "Git", "Commit", |ui, send| {
        commit(ui, send, false);
    })
    .when(has_repo));
    add(command("git.commitAmend", "Git", "Commit (Amend)", |ui, send| {
        commit(ui, send, true);
    })
    .when(has_repo));
    add(command("git.refresh", "Git", "Refresh", |_, send| refresh(send)));
    add(command("git.stageAll", "Git", "Stage All Changes", |ui, send| stage(send, &files_of(ui, Group::Changes)))
        .when(has_repo));
    add(command("git.unstageAll", "Git", "Unstage All Changes", |ui, send| {
        unstage(send, &files_of(ui, Group::Staged))
    })
    .when(has_repo));
    add(command("git.cleanAll", "Git", "Discard All Changes", |ui, send| {
        discard(ui, send, files_of(ui, Group::Changes))
    })
    .when(has_repo));
    add(command("git.init", "Git", "Initialize Repository", |_, send| send(json!({"type": "git_init", "repo": ""})))
        .when(|ui| !has_repo(ui)));
    let title = Menu::ViewTitle(View::Scm);
    crate::menus::add(title, item("git.commit").title("Commit").icon("check").group("navigation").order(1));
    crate::menus::add(title, item("git.refresh").title("Refresh").icon("refresh").group("navigation").order(2));
    crate::editor::kinds::add(
        crate::editor::kinds::kind(
            KIND,
            |_ui, rest| title_of(rest),
            |ui, send, tab| changes_tab(ui, send, tab.path.clone()),
        )
        .icon("git-compare"),
    );
}

/// The paths of the open changes tabs (what is after `scm:`).
fn open_tabs(ui: Ui) -> Vec<String> {
    ui.tabs.with_untracked(|tabs| tabs.iter().filter(|t| t.kind == Some(KIND)).map(|t| t.file.clone()).collect())
}

/// The driver's status, taken in: set only where it changed. Its replies to a commit and to a file's side, too;
/// and the status asked again when files change. Never taken away (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let me = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "git_status" if m["repo"].as_str().unwrap_or("").is_empty() => {
            let s = Status::of(m);
            if let Some(b) = &s.branch
                && ui.branch.with_untracked(|was| was != b)
            {
                ui.branch.set(b.clone());
            }
            crate::badges::set(ui, View::Scm, s.total);
            if me.status.with_untracked(|was| was.as_ref() != Some(&s)) {
                me.status.set(Some(s));
                // What the open changes tabs compare may be another now.
                for rest in open_tabs(ui) {
                    ask_sides(ui, send, &rest);
                }
            }
        }
        "git_committed" => {
            me.busy.set(false);
            match m["error"].as_str() {
                Some(e) => {
                    toast::error(ui, e);
                }
                None => {
                    me.message.set(String::new());
                    toast::info(ui, &format!("Committed {}", m["hash"].as_str().unwrap_or("")));
                }
            }
        }
        "git_failed" => {
            toast::error(ui, m["error"].as_str().unwrap_or("git failed"));
        }
        // The index's text asked by quick diff is its own (`quick_diff.rs`).
        "git_shown" if m["key"] != quick_diff::OWNER => {
            let path = m["path"].as_str().unwrap_or("");
            let key = shown_key(m["rev"].as_str().unwrap_or(""), m["working"].as_bool().unwrap_or(false), path);
            let shown = Shown::of(m);
            if me.shown.with_untracked(|s| s.get(&key) != Some(&shown)) {
                me.shown.update(|s| {
                    s.insert(key, shown);
                });
                for rest in open_tabs(ui) {
                    compare(ui, &rest);
                }
            }
        }
        "state" | "saved" | "created" | "renamed" | "deleted" | "stats" | "turn_end" => refresh_soon(ui, send),
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, x: char, y: char) -> File {
        let untracked = x == '?';
        let conflict = matches!((x, y), ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D'));
        File {
            path: path.into(),
            from: None,
            x,
            y,
            staged: !untracked && !conflict && x != ' ',
            changed: !untracked && !conflict && y != ' ',
            untracked,
            conflict,
        }
    }

    fn status(files: Vec<File>) -> Status {
        Status { root: Some("/r".into()), branch: Some("main".into()), total: files.len(), files, ..Status::default() }
    }

    fn ids(rows: &im::Vector<wt::Node>) -> Vec<String> {
        rows.iter().map(|n| n.id.clone()).collect()
    }

    #[test]
    fn the_drivers_status_is_read_whole() {
        let m = json!({"type": "git_status", "repo": "", "root": "/r", "branch": "main", "upstream": "origin/main",
            "ahead": 1, "behind": 2, "total": 1, "files": [{"path": "b.rs", "from": "a.rs", "x": "R", "y": " ",
            "staged": true, "changed": false, "untracked": false, "conflict": false}]});
        let s = Status::of(&m);
        assert_eq!(
            (s.branch.as_deref(), s.upstream.as_deref(), s.ahead, s.behind),
            (Some("main"), Some("origin/main"), 1, 2)
        );
        let renamed =
            File { path: "b.rs".into(), from: Some("a.rs".into()), x: 'R', y: ' ', staged: true, ..File::default() };
        assert_eq!(s.files[0], renamed);
        assert_eq!(empty_of(Some(&s)), Empty::Changes);
        assert_eq!(empty_of(Some(&Status::of(&json!({"root": null})))), Empty::NoRepo);
        assert_eq!(empty_of(Some(&Status::of(&json!({"root": null, "error": "git was not found"})))), Empty::Failed);
        assert_eq!(empty_of(Some(&status(vec![]))), Empty::Clean);
        assert_eq!(empty_of(None), Empty::Waiting);
    }

    #[test]
    fn the_groups_have_each_file_where_git_has_it_with_vs_codes_letters() {
        let s = status(vec![
            file("a.rs", 'M', 'M'),
            file("n.txt", '?', '?'),
            file("c.rs", 'U', 'U'),
            file("d.rs", ' ', 'D'),
            file("e.rs", 'A', ' '),
        ]);
        assert_eq!(
            ids(&rows(&s, &[])),
            [
                "g:merge",
                "f:merge:c.rs",
                "g:staged",
                "f:staged:a.rs",
                "f:staged:e.rs",
                "g:changes",
                "f:changes:a.rs",
                "f:changes:n.txt",
                "f:changes:d.rs"
            ]
        );
        let letters: Vec<char> =
            GROUPS.iter().flat_map(|g| s.of_group(*g).into_iter().map(|f| g.letter(f)).collect::<Vec<_>>()).collect();
        assert_eq!(letters, ['C', 'M', 'A', 'M', 'U', 'D']);
        // A folded group keeps its row; a group with nothing in it is left out, but Changes is always there.
        let folded = ids(&rows(&s, &[Group::Staged]));
        assert!(folded.contains(&"g:staged".to_string()) && !folded.iter().any(|i| i.starts_with("f:staged")));
        assert_eq!(ids(&rows(&status(vec![]), &[])), ["g:changes"]);
        // A row whose letter changes is drawn again.
        let before = rows(&status(vec![file("a.rs", ' ', 'M')]), &[]);
        let after = rows(&status(vec![file("a.rs", ' ', 'D')]), &[]);
        assert_ne!(before[1], after[1]);
        assert_eq!(row_of("f:changes:src/a:b.rs"), Some(Row::File(Group::Changes, "src/a:b.rs")));
    }

    #[test]
    fn a_changes_tab_compares_the_sides_vs_code_does() {
        let mut renamed = file("new.rs", 'R', ' ');
        renamed.from = Some("old.rs".into());
        let s = status(vec![
            file("a.rs", 'M', 'M'),
            file("n.txt", '?', '?'),
            file("d.rs", ' ', 'D'),
            file("e.rs", 'A', ' '),
            renamed,
        ]);
        let at = |rev: &str, w: bool, p: &str| Some((rev.to_string(), w, p.to_string()));
        assert_eq!(sides(&s, "w:a.rs"), Some((at("", false, "a.rs"), at("", true, "a.rs"))));
        assert_eq!(sides(&s, "i:a.rs"), Some((at("HEAD", false, "a.rs"), at("", false, "a.rs"))));
        assert_eq!(sides(&s, "w:d.rs"), Some((at("", false, "d.rs"), None)));
        assert_eq!(sides(&s, "i:e.rs"), Some((None, at("", false, "e.rs"))));
        assert_eq!(sides(&s, "i:new.rs"), Some((at("HEAD", false, "old.rs"), at("", false, "new.rs"))));
        assert_eq!(sides(&s, "w:n.txt"), Some((None, at("", true, "n.txt"))));
        assert_eq!(title_of("w:src/a.rs"), "a.rs (Working Tree)");
        assert_eq!(title_of("i:a.rs"), "a.rs (Index)");
        assert_eq!(paths_of(&[&s.files[4]], true), ["new.rs", "old.rs"], "both halves of a rename are unstaged");
    }

    #[test]
    fn discarding_asks_first_in_vs_codes_words() {
        let (m, d, b) = discard_question(&[file("src/a.rs", ' ', 'M')]);
        assert_eq!(
            (m.as_str(), d, b.as_str()),
            ("Are you sure you want to discard changes in a.rs?", "This cannot be undone.", "Discard File")
        );
        let (m, _, b) = discard_question(&[file("n.txt", '?', '?')]);
        assert_eq!((m.as_str(), b.as_str()), ("Are you sure you want to delete n.txt?", "Delete File"));
        let (m, _, b) = discard_question(&[file("a", ' ', 'M'), file("b", ' ', 'M')]);
        assert_eq!(
            (m.as_str(), b.as_str()),
            ("Are you sure you want to discard ALL changes in 2 files?", "Discard All 2 Files")
        );
    }

    #[test]
    fn a_status_sets_the_branch_and_the_badge_and_is_set_again_only_when_it_changed() {
        let ui = Ui::new();
        crate::layout::also_ready(&[View::Scm]);
        let none: Send = Rc::new(|_| {});
        register(ui, &none);
        assert!(crate::commands::find("git.commit").is_some() && crate::commands::find("git.init").is_some());
        let _ = view(ui, none.clone());
        let m = json!({"type": "git_status", "repo": "", "root": "/r", "branch": "dev", "total": 1,
            "files": [{"path": "a.rs", "x": " ", "y": "M", "changed": true}]});
        take(ui, &m, &none);
        assert_eq!(ui.branch.get_untracked(), "dev");
        assert_eq!(crate::badges::count(ui, View::Scm).get_untracked(), 1);
        let sets = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = sets.clone();
        floem::reactive::create_effect(move |_| {
            state(ui).status.with(|_| ());
            seen.set(seen.get() + 1);
        });
        take(ui, &m, &none);
        assert_eq!(sets.get(), 1, "the same status is not set again");
        take(ui, &json!({"type": "git_status", "repo": "", "root": "/r", "branch": "dev", "files": []}), &none);
        assert_eq!((sets.get(), crate::badges::count(ui, View::Scm).get_untracked()), (2, 0));
        // A commit that failed keeps the message; one made clears it, and each is said.
        let me = state(ui);
        me.message.set("Fix the cart".into());
        let toasts = toast::count(ui);
        take(ui, &json!({"type": "git_committed", "repo": "", "error": "Author identity unknown"}), &none);
        assert_eq!((me.message.get_untracked().as_str(), toast::count(ui)), ("Fix the cart", toasts + 1));
        take(ui, &json!({"type": "git_committed", "repo": "", "hash": "abc1234"}), &none);
        assert_eq!((me.message.get_untracked().as_str(), toast::count(ui)), ("", toasts + 2));
        crate::layout::also_ready(&[]);
    }
}
