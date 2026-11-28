//! The three-way merge editor of a file in conflict, as VS Code's: the incoming side and the current side above,
//! each in a read-only editor of its own with its branch and commit ('feature · 1a2b3c4 Shout two'), and the
//! result under them, which is the file itself (its tab's text: edited, saved with Ctrl+S, its tab shows the
//! change). Each conflict's lines are coloured in the two sides (`merge.incoming…` and `merge.current…`), the one
//! being looked at more strongly; Accept Incoming, Accept Current and Accept Combination put a side's lines (or both)
//! in the result's place of that conflict, as the check in a side's glyph margin does. The result's header says
//! 'N of M conflicts remaining', with Previous and Next (the sides follow) and Reset (the result as it was when the
//! editor opened). Complete Merge saves the file and stages it ('a.rs is merged and staged.') and closes the editor;
//! while conflicts remain it asks first. Narrower than 700 px the three are one column.
//!
//! A merge editor is a tab of the kind `merge` (`editor::kinds`), its path `merge:` and the file's path; its title
//! 'Merging a.rs', with '(rebase)', '(cherry-pick)' or '(revert)' when that is under way. What it shows of the file
//! is asked of the driver (`git_merge_info`); a file that is not text is not opened, and one git no longer has in
//! conflict says so above the three. The result's conflicts are found again, off the window's thread, 100 ms after
//! the file's last edit, and so is where each is in the two sides.
//!
//! It opens with `git.openMergeEditor` (the conflict lens's 'Resolve in Merge Editor', the palette), from a file
//! row of Merge Changes in Source Control, and from such a row's click when `git.mergeEditor` is on.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use floem::views::{Decorators, empty, h_stack, label, v_stack};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::decor::{Mark, Tint};
use crate::editor::{folding, kinds};
use crate::merge_inline::{self, Choice, Conflict};
use crate::state::{Send, Tab, Ui};
use crate::widgets::toast;

/// The kind of tab a merge editor is.
pub const KIND: &str = "merge";

/// Its command: the file shown opened in the merge editor.
pub const OPEN: &str = merge_inline::MERGE_EDITOR;

/// The owner of the two sides' decorations.
const OWNER: &str = "merge-editor";

/// Narrower than this, the three editors are one column.
const STACK_BELOW: f64 = 700.0;

/// How long after the result's last edit its conflicts are found again.
const AFTER_EDIT: Duration = Duration::from_millis(100);

/// One side of the merge, as the driver names it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Side {
    pub name: String,
    pub commit: String,
    pub short: String,
    pub subject: String,
}

impl Side {
    fn of(v: &Value) -> Side {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        Side { name: s("name"), commit: s("commit"), short: s("short"), subject: s("subject") }
    }

    /// What a side's header says of it: 'feature · 1a2b3c4 Shout two'.
    pub fn describe(&self) -> String {
        let commit = [self.short.as_str(), self.subject.as_str()]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        [self.name.as_str(), commit.as_str()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" · ")
    }
}

/// What the driver says of a file in conflict (`git_merge_info`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    /// Whether git's index still has the file in conflict.
    pub unmerged: bool,
    /// What is under way: merge, rebase, cherry-pick or revert.
    pub kind: String,
    pub current: Side,
    pub incoming: Side,
    pub current_text: String,
    pub incoming_text: String,
    /// A side is not text, or too large to show.
    pub binary: bool,
    pub error: Option<String>,
}

impl Info {
    pub fn of(m: &Value) -> Info {
        let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
        Info {
            unmerged: m["unmerged"] == true,
            kind: s("kind"),
            current: Side::of(&m["current"]),
            incoming: Side::of(&m["incoming"]),
            current_text: s("current_text"),
            incoming_text: s("incoming_text"),
            binary: m["binary"] == true || m["too_large"] == true,
            error: m["error"].as_str().map(str::to_string),
        }
    }
}

/// A merge editor's title: 'Merging a.rs', and what is under way when it is not a merge.
pub fn title_of(file: &str, kind: &str) -> String {
    let name = file.rsplit('/').next().unwrap_or(file);
    match kind {
        "rebase" | "cherry-pick" | "revert" => format!("Merging {name} ({kind})"),
        _ => format!("Merging {name}"),
    }
}

/// What the result's header says of its conflicts: those left of those it had.
pub fn remaining(left: usize, had: usize) -> String {
    match left {
        0 => "No conflicts remaining".into(),
        n if had.max(n) == 1 => "1 of 1 conflict remaining".into(),
        n => format!("{n} of {} conflicts remaining", had.max(n)),
    }
}

/// A text's lines, without their line ends.
fn lines(text: &str) -> Vec<&str> {
    let mut all: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    if text.ends_with('\n') {
        all.pop();
    }
    all
}

/// What a side has in a conflict: its lines, and the line before the conflict (none: it is the text's first).
pub struct Block<'a> {
    pub lines: Vec<&'a str>,
    pub before: Option<&'a str>,
}

/// Each conflict's lines of one side (`incoming`: the side after `=======`, else the one before it).
pub fn blocks<'a>(text: &'a str, conflicts: &[Conflict], incoming: bool) -> Vec<Block<'a>> {
    let all = lines(text);
    let span = |from: usize, to: usize| all.get(from..to.min(all.len())).map(<[&str]>::to_vec).unwrap_or_default();
    conflicts
        .iter()
        .map(|c| Block {
            lines: if incoming { span(c.split + 1, c.end) } else { span(c.start + 1, c.base.unwrap_or(c.split)) },
            before: c.start.checked_sub(1).and_then(|l| all.get(l).copied()),
        })
        .collect()
}

/// Where each conflict's block is in a side's text, as lines, in order: the first place after the last block found
/// whose lines are the block's and whose line before is the conflict's, else the first whose lines are the block's.
/// An empty block is the empty range after the line before it. A block not found is the empty range where the last
/// one ended.
pub fn locate(side: &str, blocks: &[Block]) -> Vec<Range<usize>> {
    let all = lines(side);
    let mut from = 0;
    let mut found = Vec::with_capacity(blocks.len());
    for b in blocks {
        let n = b.lines.len();
        let fits = |i: usize| all.get(i..i + n).is_some_and(|w| w == b.lines.as_slice());
        let after = |i: usize| match b.before {
            Some(line) => i > 0 && all.get(i - 1) == Some(&line),
            None => i == 0,
        };
        let at = if n == 0 {
            match b.before {
                Some(line) => (from..all.len()).find(|&i| all[i] == line).map(|i| i + 1),
                None => Some(from),
            }
        } else {
            (from..all.len()).find(|&i| fits(i) && after(i)).or_else(|| (from..all.len()).find(|&i| fits(i)))
        };
        let range = match at {
            Some(i) => i..i + n,
            None => from..from,
        };
        from = range.end;
        found.push(range);
    }
    found
}

/// The edit that resolves a conflict with both sides: from, to and the lines kept, the incoming first or the
/// current first.
pub fn combine(text: &str, c: &Conflict, incoming_first: bool) -> (usize, usize, String) {
    let (from, to, current) = merge_inline::resolve(text, c, Choice::Current);
    let incoming = merge_inline::resolve(text, c, Choice::Incoming).2;
    let (a, b) = if incoming_first { (incoming, current) } else { (current, incoming) };
    let mut both = a;
    if !both.is_empty() && !both.ends_with('\n') && !b.is_empty() {
        both.push('\n');
    }
    (from, to, both + &b)
}

/// What is found of the result: its conflicts, and where each is in the incoming side and in the current one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub conflicts: Vec<Conflict>,
    pub incoming: Vec<Range<usize>>,
    pub current: Vec<Range<usize>>,
}

/// The result's conflicts found, and where they are in the two sides.
pub fn find(result: &str, incoming: &str, current: &str) -> Found {
    let conflicts = merge_inline::parse(result);
    Found {
        incoming: locate(incoming, &blocks(result, &conflicts, true)),
        current: locate(current, &blocks(result, &conflicts, false)),
        conflicts,
    }
}

/// One of the two read-only sides: its text, its syntax colours, the key its marks are kept under.
pub struct Pane {
    pub doc: Rc<TextDocument>,
    pub colours: Rc<crate::highlight::Colours>,
    pub key: String,
}

/// The three editors on screen.
#[derive(Clone)]
struct Editors {
    incoming: Editor,
    current: Editor,
    result: Editor,
}

/// A merge editor open in a tab.
pub struct Entry {
    pub path: String,
    pub file: String,
    pub info: RwSignal<Option<Rc<Info>>>,
    pub incoming: Pane,
    pub current: Pane,
    /// What was found of the result last.
    pub found: RwSignal<Rc<Found>>,
    /// The most conflicts the result had since it opened.
    pub had: RwSignal<usize>,
    /// The conflict looked at, by its place.
    pub at: RwSignal<usize>,
    /// The editor's width, by which its three are one column.
    pub width: RwSignal<f64>,
    /// The result's text when the editor opened, for Reset.
    start: RefCell<Option<String>>,
    editors: RefCell<Option<Editors>>,
    /// The result's document an edit is followed in (by its address).
    followed: Cell<usize>,
    /// The finds asked for (an older one's answer is dropped), and the edits waited after.
    asked: Cell<u64>,
    edits: Cell<u64>,
    /// Complete Merge waits for the file's save: whether it completes with conflicts.
    completing: Cell<Option<bool>>,
}

thread_local! {
    static ENTRIES: RefCell<HashMap<String, Rc<Entry>>> = RefCell::new(HashMap::new());
    /// The files asked of the driver to be opened, for the merge editor shown once each comes.
    static WAITING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// A merge editor's tab path, by its file.
pub fn path_of(file: &str) -> String {
    format!("{KIND}:{file}")
}

/// A merge editor open, by its tab's path.
pub fn entry(path: &str) -> Option<Rc<Entry>> {
    ENTRIES.with(|o| o.borrow().get(path).cloned())
}

/// A merge editor whose tab is still open; one whose tab was closed is forgotten.
fn living(ui: Ui, path: &str) -> Option<Rc<Entry>> {
    let e = entry(path)?;
    if ui.tab(path).is_some() {
        return Some(e);
    }
    forget(ui, path);
    None
}

/// A merge editor forgotten, the marks of its sides with it.
fn forget(ui: Ui, path: &str) {
    let Some(e) = ENTRIES.with(|o| o.borrow_mut().remove(path)) else { return };
    for pane in [&e.incoming, &e.current] {
        crate::decor::clear(ui, OWNER, &pane.key);
    }
}

/// The merge editor in the tab shown.
pub fn shown(ui: Ui) -> Option<Rc<Entry>> {
    ui.active.get_untracked().and_then(|p| entry(&p))
}

/// The result's document: the file's open tab's.
fn result_doc(ui: Ui, e: &Entry) -> Option<Rc<TextDocument>> {
    ui.tab(&e.file).filter(|t| t.is_file()).map(|t| t.doc)
}

/// A file opened in the merge editor, shown (the one already open, when it is). The file is opened in a tab too
/// when it is not yet, as its text is the result.
pub fn open(ui: Ui, send: &Send, file: &str) {
    let path = path_of(file);
    if ui.tab(&path).is_none() {
        forget(ui, &path);
    }
    if entry(&path).is_none() {
        let pane = |side: &str| {
            let key = format!("{path}#{side}/{file}");
            let doc = Rc::new(TextDocument::new(ui.scope, ""));
            let colours = crate::highlight::Colours::new(doc.clone(), &key);
            folding::attach(ui, &key, &doc);
            Pane { doc, colours, key }
        };
        let e = Rc::new(Entry {
            path: path.clone(),
            file: file.to_string(),
            info: ui.scope.create_rw_signal(None),
            incoming: pane("incoming"),
            current: pane("current"),
            found: ui.scope.create_rw_signal(Rc::new(Found::default())),
            had: ui.scope.create_rw_signal(0),
            at: ui.scope.create_rw_signal(0),
            width: ui.scope.create_rw_signal(0.0),
            start: RefCell::new(None),
            editors: RefCell::new(None),
            followed: Cell::new(0),
            asked: Cell::new(0),
            edits: Cell::new(0),
            completing: Cell::new(None),
        });
        ENTRIES.with(|o| o.borrow_mut().insert(path.clone(), e));
        let (repo, rel) = crate::scm::repos::split(ui, file);
        send(json!({"type": "git_merge_info", "repo": repo, "path": rel}));
    }
    if ui.tab(file).is_some_and(|t| t.is_file()) {
        if let Some(e) = entry(&path) {
            follow(ui, &e);
        }
        kinds::open(ui, &path);
    } else {
        WAITING.with(|w| w.borrow_mut().insert(file.to_string()));
        send(json!({"type": "open", "path": file}));
    }
}

/// The result's edits followed: its conflicts found again a moment after each. Once per document (an update's
/// listener cannot be taken off; it does nothing once the editor is closed).
fn follow(ui: Ui, e: &Rc<Entry>) {
    let Some(doc) = result_doc(ui, e) else { return };
    let at = Rc::as_ptr(&doc) as usize;
    if e.followed.get() == at {
        return;
    }
    e.followed.set(at);
    if e.start.borrow().is_none() {
        *e.start.borrow_mut() = Some(doc.text().to_string());
    }
    let path = e.path.clone();
    doc.add_on_update(move |_| {
        if let Some(e) = entry(&path)
            && e.followed.get() == at
        {
            soon(ui, &e);
            // The file's conflict marks too, when the editor with the keys is one of the sides.
            merge_inline::edited(ui, &e.file);
        }
    });
    run(ui, e);
}

/// The result's conflicts found again once its edits have stopped for a moment.
fn soon(ui: Ui, e: &Rc<Entry>) {
    let n = e.edits.get() + 1;
    e.edits.set(n);
    let path = e.path.clone();
    floem::action::exec_after(AFTER_EDIT, move |_| {
        if let Some(e) = living(ui, &path)
            && e.edits.get() == n
        {
            run(ui, &e);
        }
    });
}

/// The result's conflicts found, off the window's thread.
fn run(ui: Ui, e: &Rc<Entry>) {
    let Some(doc) = result_doc(ui, e) else { return };
    let n = e.asked.get() + 1;
    e.asked.set(n);
    let result = doc.text().to_string();
    let (incoming, current) = (e.incoming.doc.text().to_string(), e.current.doc.text().to_string());
    let path = e.path.clone();
    let done = floem::ext_event::create_ext_action(ui.scope, move |f: Found| {
        if let Some(e) = entry(&path)
            && e.asked.get() == n
        {
            apply(ui, &e, f);
        }
    });
    std::thread::spawn(move || done(find(&result, &incoming, &current)));
}

/// What was found of the result taken in: the count, the conflict looked at kept in range, the sides' marks.
fn apply(ui: Ui, e: &Rc<Entry>, f: Found) {
    let n = f.conflicts.len();
    if e.had.get_untracked() < n {
        e.had.set(n);
    }
    let at = e.at.get_untracked().min(n.saturating_sub(1));
    if e.at.get_untracked() != at {
        e.at.set(at);
    }
    if *e.found.get_untracked() != f {
        e.found.set(Rc::new(f));
    }
    mark(ui, e);
}

/// The sides' marks: each conflict's lines coloured (the one looked at more strongly), with a check in the glyph
/// margin that accepts it, and its place in the overview ruler.
fn mark(ui: Ui, e: &Rc<Entry>) {
    let f = e.found.get_untracked();
    let at = e.at.get_untracked();
    let sides: [(&Pane, &Vec<Range<usize>>, Choice); 2] =
        [(&e.incoming, &f.incoming, Choice::Incoming), (&e.current, &f.current, Choice::Current)];
    for (pane, ranges, choice) in sides {
        let last = pane.doc.rope_text().num_lines().saturating_sub(1);
        let mut marks = Vec::new();
        for (i, r) in ranges.iter().enumerate() {
            let incoming = choice == Choice::Incoming;
            let bg: fn(&crate::colors::Colors) -> floem::peniko::Color = match (incoming, i == at) {
                (true, true) => |c| c.merge_incoming_header,
                (true, false) => |c| c.merge_incoming_content,
                (false, true) => |c| c.merge_current_header,
                (false, false) => |c| c.merge_current_content,
            };
            marks.extend(r.clone().map(|line| Mark::Line { line, bg: Tint::Theme(bg) }));
            let line = r.start.min(last);
            marks.push(Mark::Ruler {
                line,
                tint: Tint::Theme(if incoming { |c| c.ruler_incoming } else { |c| c.ruler_current }),
            });
            let path = e.path.clone();
            marks.push(Mark::Glyph {
                line,
                icon: "check",
                tint: Tint::Theme(|c| c.icon),
                title: if incoming { "Accept Incoming".into() } else { "Accept Current".into() },
                click: Some(Rc::new(move |ui, _send, _line| accept(ui, &path, i, choice, false))),
            });
        }
        crate::decor::set(ui, OWNER, &pane.key, marks);
        // A side is not a tab of its own, so the decorations do not lay its lines out again themselves.
        pane.doc.cache_rev().update(|r| *r += 1);
    }
}

/// A side's text put in the result's place of a conflict (by its place among the result's conflicts now), or both
/// sides (`both`: the side named first).
pub fn accept(ui: Ui, path: &str, index: usize, choice: Choice, both: bool) {
    let Some(e) = entry(path) else { return };
    let Some(doc) = result_doc(ui, &e) else { return };
    let text = doc.text().to_string();
    let conflicts = merge_inline::parse(&text);
    let Some(c) = conflicts.get(index) else { return };
    let (from, to, kept) =
        if both { combine(&text, c, choice == Choice::Incoming) } else { merge_inline::resolve(&text, c, choice) };
    doc.edit_single(Selection::region(from, to), &kept, EditType::Other);
    if e.at.get_untracked() != index {
        e.at.set(index);
    }
    // Found again at once: the next conflict is looked at where this one was.
    run(ui, &e);
}

/// The conflict looked at accepted from a side.
fn accept_shown(ui: Ui, path: &str, choice: Choice, both: bool) {
    let Some(e) = entry(path) else { return };
    accept(ui, path, e.at.get_untracked(), choice, both);
}

/// The next conflict looked at (or the previous), round the ends: the three editors' carets put on it.
pub fn go(ui: Ui, path: &str, forward: bool) {
    let Some(e) = entry(path) else { return };
    let Some(doc) = result_doc(ui, &e) else { return };
    let conflicts = merge_inline::parse(&doc.text().to_string());
    if conflicts.is_empty() {
        return;
    }
    let editors = e.editors.borrow().clone();
    let caret_line =
        editors.as_ref().map(|ed| ed.result.offset_to_line_col(ed.result.cursor.with_untracked(|c| c.offset())).0);
    let index = match caret_line.and_then(|l| merge_inline::next(&conflicts, l, forward)) {
        Some(start) => conflicts.iter().position(|c| c.start == start).unwrap_or(0),
        None => {
            let n = conflicts.len();
            let at = e.at.get_untracked().min(n - 1);
            if forward { (at + 1) % n } else { (at + n - 1) % n }
        }
    };
    show_conflict(ui, &e, index, &conflicts);
}

/// A conflict looked at: marked, and each editor's caret put on it, centred.
fn show_conflict(ui: Ui, e: &Rc<Entry>, index: usize, conflicts: &[Conflict]) {
    if e.at.get_untracked() != index {
        e.at.set(index);
        mark(ui, e);
    }
    let Some(ed) = e.editors.borrow().clone() else { return };
    let f = e.found.get_untracked();
    let put = |editor: &Editor, line: usize, key: &str| {
        let at = editor.offset_of_line(line.min(editor.last_line()));
        editor.cursor.update(|c| c.set_insert(Selection::caret(at)));
        folding::center(editor, key);
    };
    if let Some(c) = conflicts.get(index) {
        put(&ed.result, c.start, &e.file);
    }
    if let Some(r) = f.incoming.get(index) {
        put(&ed.incoming, r.start, &e.incoming.key);
    }
    if let Some(r) = f.current.get(index) {
        put(&ed.current, r.start, &e.current.key);
    }
}

/// Reset: the result's text as it was when the merge editor opened.
pub fn reset(ui: Ui, path: &str) {
    let Some(e) = entry(path) else { return };
    let (Some(doc), Some(start)) = (result_doc(ui, &e), e.start.borrow().clone()) else { return };
    let len = doc.text().len();
    if doc.text().to_string() != start {
        doc.edit_single(Selection::region(0, len), &start, EditType::Other);
    }
}

/// Complete Merge: asked first while conflicts remain; then the file saved and staged.
pub fn complete(ui: Ui, send: &Send, path: &str) {
    let Some(e) = entry(path) else { return };
    let Some(doc) = result_doc(ui, &e) else { return };
    let left = merge_inline::parse(&doc.text().to_string()).len();
    if left == 0 {
        finish(ui, send, &e, false);
        return;
    }
    let name = e.file.rsplit('/').next().unwrap_or(&e.file).to_string();
    let (send, e) = (send.clone(), e.clone());
    crate::widgets::dialog::ask(&format!("Do you want to complete the merge of {name}?"))
        .detail("The file contains unhandled conflicts.")
        .button("Complete with Conflicts")
        .open(ui, move |a| {
            if a.yes() {
                finish(ui, &send, &e, true);
            }
        });
}

/// The file saved (when it has changes) and then staged by the driver.
/// A file's merge completed by git, in the repository it is in.
fn complete_in_git(ui: Ui, send: &Send, file: &str, anyway: bool) {
    let (repo, rel) = crate::scm::repos::split(ui, file);
    send(json!({"type": "git_merge_complete", "repo": repo, "path": rel, "anyway": anyway}));
}

fn finish(ui: Ui, send: &Send, e: &Rc<Entry>, anyway: bool) {
    if ui.tab(&e.file).is_some_and(|t| t.dirty.get_untracked()) {
        e.completing.set(Some(anyway));
        ui.save_tab(&e.file, send);
    } else {
        complete_in_git(ui, send, &e.file, anyway);
    }
}

/// The file a command acts on: the editor's that has the keys (its tab, or a merge editor's file).
fn file_of(ui: Ui) -> Option<String> {
    let live = ui.live.with_untracked(|l| l.as_ref().map(|(p, _)| p.clone()));
    let path = live.or_else(|| ui.active.get_untracked())?;
    if let Some(e) = entry(&path) {
        return Some(e.file.clone());
    }
    ui.tab(&path).filter(|t| t.is_file()).map(|t| t.path)
}

/// What a merge editor's tab draws: the banner (when git says the file is merged), the two sides and the result.
fn view(ui: Ui, send: Send, tab: &Tab) -> AnyView {
    let Some(e) = living(ui, &tab.path) else {
        return label(|| "This merge editor is no longer open.")
            .style(move |s| s.padding(20.0).color(ui.colors.get().desc))
            .into_any();
    };
    follow(ui, &e);
    let theme = crate::highlight::theme_of(&ui.theme_json());
    e.incoming.colours.set_theme(theme.clone());
    e.current.colours.set_theme(theme);
    let Some(result) = ui.tab(&e.file).filter(|t| t.is_file()) else {
        return label(|| "The file is being opened…")
            .style(move |s| s.padding(20.0).color(ui.colors.get().desc))
            .into_any();
    };
    let narrow = {
        let width = e.width;
        create_memo(move |_| width.get() > 0.0 && width.get() < STACK_BELOW)
    };
    let t = ui.colors;
    let info = e.info;
    let said = create_memo(move |_| {
        info.with(|i| match i.as_deref() {
            Some(Info { error: Some(err), .. }) => Some(format!("Git could not say what is in conflict: {err}")),
            Some(Info { unmerged: false, .. }) => {
                Some("Git has already merged this file: it is not in conflict.".into())
            }
            _ => None,
        })
    });
    let banner = label(move || said.get().unwrap_or_default()).style(move |s| {
        let c = t.get();
        s.width_full()
            .padding_horiz(10.0)
            .padding_vert(4.0)
            .font_size(12.0)
            .background(c.merge_common_content)
            .color(c.fg)
            .apply_if(said.get().is_none(), |s| s.hide())
    });
    // Built once: a layout built again would leave the group holding the editors it replaced.
    let body = layout(ui, &send, &e, result, narrow)
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
    let (width, cleared) = (e.width, e.clone());
    v_stack((banner, body))
        .on_resize(move |r| {
            if (width.get_untracked() - r.width()).abs() > 0.5 {
                width.set(r.width());
            }
        })
        .on_cleanup(move || *cleared.editors.borrow_mut() = None)
        .style(move |s| s.size_full().background(t.get().editor_bg))
        .into_any()
}

/// The three editors: the sides beside each other above the result, or, while `narrow`, one column.
fn layout(ui: Ui, send: &Send, e: &Rc<Entry>, result: Tab, narrow: floem::reactive::Memo<bool>) -> AnyView {
    let t = ui.colors;
    let (incoming, iv) = side(ui, e, true);
    let (current, cv) = side(ui, e, false);
    let (res, rv) = result_view(ui, send, e, &result);
    // The editor the window's commands act on: the result, or the one clicked. The result is the file's own text, so
    // it is the file's (the Merge Conflict commands act on it, and its conflicts are read again as it is typed in).
    ui.live.set(Some((e.file.clone(), res.clone())));
    for (ed, path) in [(&incoming, &e.path), (&current, &e.path), (&res, &e.file)] {
        let (focus, me, path) = (ed.editor_view_focused, ed.clone(), path.clone());
        create_effect(move |first: Option<()>| {
            focus.track();
            if first.is_some() && ui.live.with_untracked(|l| l.as_ref().is_none_or(|(_, ed)| ed.id() != me.id())) {
                ui.live.set(Some((path.clone(), me.clone())));
            }
        });
    }
    // The conflict the result's caret is in is the one looked at.
    {
        let (cursor, here, e) = (res.cursor, res.clone(), e.clone());
        create_effect(move |_| {
            let offset = cursor.with(|c| c.offset());
            let line = here.offset_to_line_col(offset).0;
            let found = e.found.get_untracked();
            if let Some(i) = found.conflicts.iter().position(|c| (c.start..=c.end).contains(&line))
                && e.at.get_untracked() != i
            {
                e.at.set(i);
                mark(ui, &e);
            }
        });
    }
    *e.editors.borrow_mut() = Some(Editors { incoming, current, result: res });
    // The rule between the two sides lies across them while they are one column.
    let rule = move |between: bool| {
        empty().style(move |s| {
            let across = !between || narrow.get();
            let s = if across { s.height(1.0).width_full() } else { s.width(1.0).height_full() };
            s.background(t.get().group_border)
        })
    };
    let grow = |s: floem::style::Style| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).min_width(0.0);
    let top = h_stack((iv.style(grow), rule(true), cv.style(grow))).style(move |s| {
        let s = s.width_full().flex_basis(0.0).min_height(0.0);
        if narrow.get() { s.flex_col().flex_grow(2.0_f32) } else { s.flex_row().flex_grow(1.0_f32) }
    });
    v_stack((top, rule(false), rv.style(grow).style(|s| s.width_full()))).style(|s| s.size_full()).into_any()
}

/// A small action in words, as a CodeLens's or a header's: 'Accept Incoming'.
fn word(ui: Ui, text: &'static str, on: impl Fn() + 'static) -> impl IntoView {
    let t = ui.colors;
    label(move || text).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        s.padding_horiz(6.0)
            .padding_vert(1.0)
            .border_radius(3.0)
            .font_size(12.0)
            .color(c.link)
            .cursor(floem::style::CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.toolbar_hover))
    })
}

/// A header over one of the three: its name, what it is, and its actions.
fn header(ui: Ui, title: &'static str, what: impl Fn() -> String + 'static, actions: AnyView) -> impl IntoView {
    let t = ui.colors;
    h_stack((
        label(move || title).style(|s| crate::widgets::bold(s.font_size(12.0))),
        label(what)
            .style(move |s| s.margin_left(8.0).font_size(12.0).color(t.get().desc).text_ellipsis().min_width(0.0)),
        empty().style(|s| s.flex_grow(1.0_f32)),
        actions,
    ))
    .style(move |s| {
        let c = t.get();
        s.width_full()
            .height(28.0)
            .items_center()
            .gap(2.0)
            .padding_horiz(10.0)
            .background(c.editor_bg)
            .color(c.fg)
            .border_bottom(1.0)
            .border_color(c.group_border)
    })
}

/// One side: its header (its branch and commit, Accept and Accept Combination) over its read-only editor.
fn side(ui: Ui, e: &Rc<Entry>, incoming: bool) -> (Editor, AnyView) {
    let pane = if incoming { &e.incoming } else { &e.current };
    let (editor, ed) = editor_of(ui, pane.doc.clone(), pane.colours.clone(), &pane.key, true);
    let ed = crate::decor::around(ui, ed, &editor, &pane.key);
    let info = e.info;
    let what = move || {
        info.with(|i| i.as_ref().map(|i| if incoming { i.incoming.describe() } else { i.current.describe() }))
            .unwrap_or_default()
    };
    let choice = if incoming { Choice::Incoming } else { Choice::Current };
    let (p1, p2) = (e.path.clone(), e.path.clone());
    let actions = h_stack((
        word(ui, if incoming { "Accept Incoming" } else { "Accept Current" }, move || {
            accept_shown(ui, &p1, choice, false)
        }),
        word(ui, "Accept Combination", move || accept_shown(ui, &p2, choice, true)),
    ))
    .into_any();
    let view = v_stack((
        header(ui, if incoming { "Incoming" } else { "Current" }, what, actions),
        ed.into_any().style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full()),
    ))
    .into_any();
    (editor, view)
}

/// The result: its header (the conflicts left, Previous, Next, Reset, Complete Merge) over the file's own editor.
fn result_view(ui: Ui, send: &Send, e: &Rc<Entry>, tab: &Tab) -> (Editor, AnyView) {
    let (editor, ed) = editor_of(ui, tab.doc.clone(), tab.colours.clone(), &tab.path, false);
    // Drawn with its language's grammar, as the file's own tab is (it may not have been shown yet).
    {
        let (colours, here, path) = (tab.colours.clone(), editor.clone(), tab.path.clone());
        create_effect(move |_| {
            if colours.set_syntax(crate::language::syntax(ui, &path)) {
                here.update_styling(colours.clone());
            }
        });
    }
    let ed = crate::merge_inline::follow(ui, ed, &editor, &tab.path);
    let ed = crate::decor::around(ui, ed, &editor, &tab.path);
    let (found, had, file) = (e.found, e.had, e.file.clone());
    let what = move || format!("{file} · {}", remaining(found.with(|f| f.conflicts.len()), had.get()));
    let t = ui.colors;
    let (p1, p2, p3, p4) = (e.path.clone(), e.path.clone(), e.path.clone(), e.path.clone());
    let send = send.clone();
    let actions = h_stack((
        floem::views::tooltip(crate::widgets::action(t, "arrow-up", move || go(ui, &p1, false)), || {
            label(|| "Previous Conflict")
        }),
        floem::views::tooltip(crate::widgets::action(t, "arrow-down", move || go(ui, &p2, true)), || {
            label(|| "Next Conflict")
        }),
        word(ui, "Reset", move || reset(ui, &p3)),
        crate::widgets::button(t, "Complete Merge", true, move || complete(ui, &send, &p4))
            .style(|s| s.margin_left(6.0).padding_vert(2.0).font_size(12.0)),
    ))
    .style(|s| s.items_center().gap(2.0))
    .into_any();
    let view = v_stack((
        header(ui, "Result", what, actions),
        ed.into_any().style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full()),
    ))
    .into_any();
    (editor, view)
}

/// An editor of one of the three, with the window's keys, under its key's folds and decorations.
fn editor_of(
    ui: Ui,
    doc: Rc<TextDocument>,
    colours: Rc<crate::highlight::Colours>,
    key: &str,
    read_only: bool,
) -> (Editor, impl IntoView + use<>) {
    let shown = crate::decor::doc(doc.clone(), key);
    let editor = Editor::new(floem::reactive::Scope::current(), shown, colours, false);
    editor.read_only.set(read_only);
    let map = Rc::new(KeypressMap::default());
    let ed = folding::view(ui, editor.clone(), &doc, key, move |editor, press, mods| {
        if crate::commands::waiting().is_some()
            && let KeyInput::Keyboard(key, _) = &press.key
        {
            let mut ran = false;
            crate::editor::tell(ui, |send| {
                ran = crate::commands::press(ui, send, key, mods) != crate::commands::Pressed::Free;
            });
            if ran {
                return CommandExecuted::Yes;
            }
        }
        if crate::editor::keys::editor_key(ui, press, mods) {
            return CommandExecuted::Yes;
        }
        match crate::editor::keys::default_key(&map, editor, press, mods) {
            CommandExecuted::No => {
                if let KeyInput::Keyboard(key, _) = &press.key
                    && crate::keys_hook::editor(ui, key, mods)
                {
                    crate::editor::tell(ui, |send| crate::workbench::key(ui, send, key, mods));
                }
                CommandExecuted::No
            }
            done => done,
        }
    })
    .style(move |s| crate::editor::editor_look(s, &ui.colors.get(), true));
    (editor, ed)
}

/// What it adds to the window's registries, once at start: its kind of tab, its command, and the merge editor shown
/// once the file it waited for is open.
pub fn register(_ui: Ui, _send: &Send) {
    kinds::add(
        kinds::kind(
            KIND,
            |_ui, rest| {
                let kind = entry(&path_of(rest))
                    .and_then(|e| e.info.with(|i| i.as_ref().map(|i| i.kind.clone())))
                    .unwrap_or_default();
                title_of(rest, &kind)
            },
            view,
        )
        .icon("git-merge")
        .save(|ui, send, tab| {
            if let Some(e) = entry(&tab.path) {
                ui.save_tab(&e.file, send);
            }
        }),
    );
    crate::commands::add(
        crate::commands::command(OPEN, "Git", "Resolve in Merge Editor", |ui, send| {
            if let Some(file) = file_of(ui) {
                open(ui, send, &file);
            }
        })
        .when(|ui| file_of(ui).is_some()),
    );
    crate::commands::add(
        crate::commands::command(
            "merge.goToNextUnhandledConflict",
            "Merge Editor",
            "Go to Next Unhandled Conflict",
            |ui, _| {
                if let Some(e) = shown(ui) {
                    go(ui, &e.path, true);
                }
            },
        )
        .when(|ui| shown(ui).is_some()),
    );
    crate::commands::add(
        crate::commands::command(
            "merge.goToPreviousUnhandledConflict",
            "Merge Editor",
            "Go to Previous Unhandled Conflict",
            |ui, _| {
                if let Some(e) = shown(ui) {
                    go(ui, &e.path, false);
                }
            },
        )
        .when(|ui| shown(ui).is_some()),
    );
    crate::commands::add(
        crate::commands::command("mergeEditor.acceptMerge", "Merge Editor", "Complete Merge", |ui, send| {
            if let Some(e) = shown(ui) {
                complete(ui, send, &e.path);
            }
        })
        .when(|ui| shown(ui).is_some()),
    );
    crate::commands::add(
        crate::commands::command("merge.resetResult", "Merge Editor", "Reset Result", |ui, _| {
            if let Some(e) = shown(ui) {
                reset(ui, &e.path);
            }
        })
        .when(|ui| shown(ui).is_some()),
    );
    crate::ext::after("merge_editor", |ui, m, _send| {
        if m["type"] != "file" || !m["error"].is_null() {
            return;
        }
        let file = m["path"].as_str().unwrap_or("");
        if WAITING.with(|w| w.borrow_mut().remove(file))
            && let Some(e) = entry(&path_of(file))
        {
            follow(ui, &e);
            kinds::open(ui, &e.path);
        }
    });
}

/// The session's messages that are this part's: what git says of a file in a merge editor, Complete Merge's save
/// and its answer (taken away: Source Control would say a failure again).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    // git names the file from its repository's top; the window, from the project's.
    let path = &crate::scm::repos::full(m["repo"].as_str().unwrap_or(""), m["path"].as_str().unwrap_or(""));
    let name = path.rsplit('/').next().unwrap_or(path);
    match m["type"].as_str().unwrap_or("") {
        "git_merge_info" => {
            // Its tab is made only once the file has come: until then, the one waiting for it.
            let waiting = WAITING.with(|w| w.borrow().contains(path));
            let merge = path_of(path);
            let Some(e) = (if waiting { entry(&merge) } else { living(ui, &merge) }) else { return false };
            let info = Info::of(m);
            if info.binary {
                toast::warning(ui, &format!("{name} is not a text file: the merge editor opens only text."));
                WAITING.with(|w| w.borrow_mut().remove(path));
                if ui.tab(&e.path).is_some() {
                    ui.close_now(&e.path);
                }
                forget(ui, &e.path);
                return true;
            }
            for (pane, text) in [(&e.incoming, &info.incoming_text), (&e.current, &info.current_text)] {
                if pane.doc.text().to_string() != *text {
                    let len = pane.doc.text().len();
                    pane.doc.edit_single(Selection::region(0, len), text, EditType::Other);
                }
            }
            e.info.set(Some(Rc::new(info)));
            run(ui, &e);
            true
        }
        "saved" => {
            let waiting =
                ENTRIES.with(|o| o.borrow().get(&path_of(path)).cloned()).filter(|e| e.completing.get().is_some());
            if let Some(e) = waiting {
                let anyway = e.completing.take().unwrap_or(false);
                if m["error"].is_null() {
                    complete_in_git(ui, send, path, anyway);
                }
            }
            false
        }
        "git_done" if m["of"] == "git_merge_complete" => {
            toast::info(ui, &format!("{name} is merged and staged."));
            let merge = path_of(path);
            if ui.tab(&merge).is_some() {
                ui.close_now(&merge);
            }
            forget(ui, &merge);
            true
        }
        "git_failed" if m["of"] == "git_merge_complete" => {
            match m["error"].as_str() {
                Some("markers") => {
                    toast::warning(ui, &format!("{name} still has conflict markers: it was not staged."))
                }
                Some(err) => toast::error(ui, &format!("Could not stage {name}: {err}")),
                None => toast::error(ui, &format!("Could not stage {name}.")),
            };
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESULT: &str =
        "one\n<<<<<<< HEAD\n2\n=======\nTWO\n>>>>>>> feature\nthree\n<<<<<<< HEAD\nx\n=======\n>>>>>>> feature\nend\n";

    #[test]
    fn a_sides_header_names_its_branch_and_commit_and_the_title_what_is_under_way() {
        let s =
            Side { name: "feature".into(), commit: "abc".into(), short: "1a2b3c4".into(), subject: "Shout two".into() };
        assert_eq!(s.describe(), "feature · 1a2b3c4 Shout two");
        assert_eq!(Side { name: "main".into(), ..Side::default() }.describe(), "main");
        assert_eq!(title_of("src/a.rs", "merge"), "Merging a.rs");
        assert_eq!(title_of("a.rs", "rebase"), "Merging a.rs (rebase)");
        assert_eq!(title_of("a.rs", "cherry-pick"), "Merging a.rs (cherry-pick)");
        assert_eq!(remaining(2, 3), "2 of 3 conflicts remaining");
        assert_eq!(remaining(1, 1), "1 of 1 conflict remaining");
        assert_eq!(remaining(0, 3), "No conflicts remaining");
        assert_eq!(remaining(4, 2), "4 of 4 conflicts remaining");
    }

    #[test]
    fn each_conflict_is_found_in_both_sides_even_when_a_side_has_no_lines_or_repeats_a_block() {
        let f = find(RESULT, "one\nTWO\nthree\nend\n", "one\n2\nthree\nx\nend\n");
        assert_eq!(f.conflicts.len(), 2);
        assert_eq!(f.incoming, vec![1..2, 3..3], "the second conflict's incoming side is empty: after 'three'");
        assert_eq!(f.current, vec![1..2, 3..4]);
        // The same block twice: the second conflict's is the one after the first's.
        let twice = "<<<<<<< a\nx\n=======\ny\n>>>>>>> b\nmid\n<<<<<<< a\nx\n=======\nz\n>>>>>>> b\n";
        let f = find(twice, "y\nmid\nz\n", "x\nmid\nx\n");
        assert_eq!(f.current, vec![0..1, 2..3]);
        assert_eq!(f.incoming, vec![0..1, 2..3]);
        // A block that is also earlier in the side, not after the conflict's line before: the one after it is taken.
        let f = find("a\n}\nb\n<<<<<<< x\n}\n=======\nq\n>>>>>>> y\n", "a\n}\nb\nq\n", "a\n}\nb\n}\n");
        assert_eq!(f.current, vec![3..4]);
        // Not found: where the last one ended.
        assert_eq!(locate("a\nb\n", &[Block { lines: vec!["zzz"], before: None }]), vec![0..0]);
    }

    #[test]
    fn a_combination_puts_the_side_it_was_taken_from_first() {
        let c = merge_inline::parse(RESULT)[0];
        let (from, to, kept) = combine(RESULT, &c, true);
        assert_eq!(&RESULT[from..to], "<<<<<<< HEAD\n2\n=======\nTWO\n>>>>>>> feature\n");
        assert_eq!(kept, "TWO\n2\n");
        assert_eq!(combine(RESULT, &c, false).2, "2\nTWO\n");
    }

    #[test]
    fn the_merge_editor_takes_the_sides_accepts_into_the_file_and_completes_after_the_save() {
        let ui = Ui::new();
        let said = Rc::new(RefCell::new(Vec::<Value>::new()));
        let kept = said.clone();
        let send: Send = Rc::new(move |v: Value| kept.borrow_mut().push(v));
        crate::editor::disk::register(ui, &send);
        register(ui, &send);
        // The file not open yet: it is asked for, and the merge editor is shown once it comes.
        open(ui, &send, "a.txt");
        assert_eq!(said.borrow()[0], json!({"type": "git_merge_info", "repo": "", "path": "a.txt"}));
        assert_eq!(said.borrow()[1], json!({"type": "open", "path": "a.txt"}));
        // What git says may come before the file does.
        let info = json!({"type": "git_merge_info", "repo": "", "path": "a.txt", "unmerged": true, "kind": "rebase",
            "current": {"name": "main", "commit": "c", "short": "1111111", "subject": "Number two"},
            "incoming": {"name": "feature", "commit": "i", "short": "2222222", "subject": "Shout two"},
            "base_text": "one\ntwo\nthree\nend\n", "current_text": "one\n2\nthree\nx\nend\n",
            "incoming_text": "one\nTWO\nthree\nend\n", "binary": false});
        assert!(take(ui, &info, &send), "the answer is the merge editor's");
        ui.take(&json!({"type": "file", "path": "a.txt", "text": RESULT, "modified": 1}), &send);
        assert_eq!(ui.active.get_untracked().as_deref(), Some("merge:a.txt"));
        let e = entry("merge:a.txt").expect("open");
        assert_eq!(e.incoming.doc.text().to_string(), "one\nTWO\nthree\nend\n");
        let tab = ui.tab("merge:a.txt").expect("its tab");
        assert_eq!(kinds::title(ui, &tab), "Merging a.txt (rebase)");
        apply(ui, &e, find(RESULT, &e.incoming.doc.text().to_string(), &e.current.doc.text().to_string()));
        assert_eq!(e.had.get_untracked(), 2);
        // Accept Incoming on the first conflict: the file's own text changes.
        accept(ui, "merge:a.txt", 0, Choice::Incoming, false);
        let file = ui.tab("a.txt").expect("the file");
        assert!(file.text().starts_with("one\nTWO\nthree\n<<<<<<<"), "{}", file.text());
        assert!(file.dirty.get_untracked());
        // Reset puts back the text it opened with.
        reset(ui, "merge:a.txt");
        assert_eq!(file.text(), RESULT);
        // Completed with the conflicts resolved: saved first, staged once saved, then closed.
        accept(ui, "merge:a.txt", 0, Choice::Current, false);
        accept(ui, "merge:a.txt", 0, Choice::Current, true);
        assert_eq!(file.text(), "one\n2\nthree\nx\nend\n");
        // Ctrl+S in the merge editor saves the file.
        said.borrow_mut().clear();
        ui.save(&send);
        assert_eq!(
            (said.borrow()[0]["type"].as_str(), said.borrow()[0]["path"].as_str()),
            (Some("save"), Some("a.txt"))
        );
        take(ui, &json!({"type": "saved", "path": "a.txt", "modified": 2}), &send);
        assert!(said.borrow().iter().all(|m| m["type"] != "git_merge_complete"), "a plain save stages nothing");
        file.doc.edit_single(Selection::region(0, 0), "", EditType::Other);
        file.dirty.set(true);
        said.borrow_mut().clear();
        complete(ui, &send, "merge:a.txt");
        assert_eq!(said.borrow()[0]["type"], "save", "{:?}", said.borrow());
        said.borrow_mut().clear();
        take(ui, &json!({"type": "saved", "path": "a.txt", "modified": 2}), &send);
        assert_eq!(
            *said.borrow(),
            [json!({"type": "git_merge_complete", "repo": "", "path": "a.txt", "anyway": false})]
        );
        assert!(take(ui, &json!({"type": "git_done", "repo": "", "of": "git_merge_complete", "path": "a.txt"}), &send));
        assert!(ui.tab("merge:a.txt").is_none(), "closed once staged");
        assert!(entry("merge:a.txt").is_none());
    }

    #[test]
    fn a_file_that_is_not_text_is_not_opened() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        register(ui, &send);
        ui.take(&json!({"type": "file", "path": "b.png", "text": "x", "modified": 1}), &send);
        open(ui, &send, "b.png");
        assert!(ui.tab("merge:b.png").is_some());
        assert!(take(
            ui,
            &json!({"type": "git_merge_info", "repo": "", "path": "b.png", "unmerged": true, "binary": true}),
            &send
        ));
        assert!(ui.tab("merge:b.png").is_none());
    }
}
