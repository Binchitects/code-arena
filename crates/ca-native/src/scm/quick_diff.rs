//! Quick diff, as VS Code's: beside the line numbers of a file's editor, a bar on each line changed since git's
//! index (green where lines were added, blue where they were changed) and a small red triangle where lines were
//! taken away, worked out again while typing; the same marks in the overview ruler. `scm.diffDecorations` says
//! where they are drawn (all, gutter, overview, minimap, none). Go to Next and Previous Change (Alt+F5,
//! Shift+Alt+F5 outside a compare; Alt+F3, Shift+Alt+F3) put the caret on the next or previous change.
//!
//! The index's text of each open file is asked of the session (`git_show`, keyed `quick_diff`) when the file is
//! opened and again after a save, a stage or a commit (every `git_status`); the lines are compared with a Myers
//! diff off the window's thread, 200 ms after the last key, line ends not counted. Past a million steps of work
//! the lines between the first and last that differ are said as one change.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use floem::context::PaintCx;
use floem::kurbo::{BezPath, Point, Rect};
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use floem::views::Decorators;
use floem::views::editor::Editor;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;
use floem::{IntoView, View, ViewId};
use floem_renderer::Renderer;
use serde_json::{Value, json};

use crate::commands::{self, command};
use crate::decor::{Mark, Tint};
use crate::state::{Send, Ui};

/// The name its marks and its `git_show` requests go by.
pub const OWNER: &str = "quick_diff";

/// The most steps of work the diff takes before it says the lines that differ as one change (VS Code's).
pub const BUDGET: usize = 1_000_000;

/// How long after the last key the lines are compared again.
const TYPING: Duration = Duration::from_millis(200);

/// What became of lines since the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Added,
    Modified,
    /// Lines taken away just above `line`.
    Deleted,
}

/// A run of changed lines of the file as it is now: from `line` (from 0), `count` of them (for Deleted, the count
/// of lines taken away).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub kind: Kind,
    pub line: usize,
    pub count: usize,
}

/// The changes from `before` (the index) to `after` (the editor), in the order of their lines. Lines are compared
/// without their ends (`\n`, `\r\n`).
pub fn changes(before: &str, after: &str) -> Vec<Change> {
    changes_within(before, after, BUDGET)
}

fn changes_within(before: &str, after: &str, budget: usize) -> Vec<Change> {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    // The lines the same at the start and at the end are not looked at again: typing compares only what is between.
    let start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let end = a[start..].iter().rev().zip(b[start..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[start..a.len() - end], &b[start..b.len() - end]);
    let hunks = match myers(a, b, budget) {
        Some(same) => hunks(&same, a.len(), b.len()),
        None => vec![(0, a.len(), 0, b.len())],
    };
    hunks
        .into_iter()
        .filter(|h| h.1 + h.3 > 0)
        .map(|(_, old, at, new)| {
            let line = start + at;
            match (old, new) {
                (0, n) => Change { kind: Kind::Added, line, count: n },
                (o, 0) => Change { kind: Kind::Deleted, line, count: o },
                (_, n) => Change { kind: Kind::Modified, line, count: n },
            }
        })
        .collect()
}

/// The lines both have, as (line in `a`, line in `b`) in order, by Myers' O(ND) diff; none when it takes more than
/// `budget` steps.
fn myers(a: &[&str], b: &[&str], budget: usize) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = n + m;
    let off = max + 1;
    let mut v = vec![0isize; 2 * max as usize + 3];
    // Each round's furthest places, for diagonals -d..=d, to walk back through.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut work = 0usize;
    let mut last = None;
    'rounds: for d in 0..=max {
        let mut k = -d;
        while k <= d {
            let at = |k: isize| (k + off) as usize;
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) { v[at(k + 1)] } else { v[at(k - 1)] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
                work += 1;
            }
            v[at(k)] = x;
            work += 1;
            if x >= n && y >= m {
                last = Some(d);
                break 'rounds;
            }
            k += 2;
        }
        trace.push(v[(off - d) as usize..=(off + d) as usize].to_vec());
        if work > budget {
            return None;
        }
    }
    let last = last?;
    let (mut x, mut y) = (n, m);
    let mut same = Vec::new();
    for d in (1..=last).rev() {
        let before = &trace[d as usize - 1];
        let was = |k: isize| before[(k + d - 1) as usize];
        let k = x - y;
        let from = if k == -d || (k != d && was(k - 1) < was(k + 1)) { k + 1 } else { k - 1 };
        let (px, py) = (was(from), was(from) - from);
        // The one edit (down from k+1, or right from k-1), then the run of lines both have.
        let (mx, my) = if from == k + 1 { (px, px - k) } else { (px + 1, px + 1 - k) };
        while x > mx && y > my {
            x -= 1;
            y -= 1;
            same.push((x as usize, y as usize));
        }
        (x, y) = (px, py);
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        same.push((x as usize, y as usize));
    }
    same.reverse();
    Some(same)
}

/// The runs between the lines both have: (line in a, lines of a, line in b, lines of b).
fn hunks(same: &[(usize, usize)], n: usize, m: usize) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let (mut x, mut y) = (0, 0);
    for &(i, j) in same.iter().chain(std::iter::once(&(n, m))) {
        if i > x || j > y {
            out.push((x, i - x, y, j - y));
        }
        (x, y) = (i + 1, j + 1);
    }
    out
}

/// The line after a change's last (a deletion's triangle is drawn on the line above it, or on the first line).
fn end(c: &Change) -> usize {
    if c.kind == Kind::Deleted { c.line + 1 } else { c.line + c.count }
}

/// The line a change is gone to (a deletion: the line above where the lines were, as VS Code's).
fn target(c: &Change) -> usize {
    if c.kind == Kind::Deleted { c.line.saturating_sub(1) } else { c.line }
}

/// The line of the change after the caret's line (`forward`) or before it, round past the ends.
pub fn next_line(changes: &[Change], caret: usize, forward: bool) -> Option<usize> {
    let lines: Vec<usize> = changes.iter().map(target).collect();
    if forward {
        lines.iter().find(|l| **l > caret).or(lines.first()).copied()
    } else {
        lines.iter().rev().find(|l| **l < caret).or(lines.last()).copied()
    }
}

/// Where the marks are drawn, as `scm.diffDecorations` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shows {
    pub gutter: bool,
    pub ruler: bool,
}

impl Shows {
    pub fn of(value: &str) -> Shows {
        match value {
            "gutter" => Shows { gutter: true, ruler: false },
            "overview" => Shows { gutter: false, ruler: true },
            // The minimap is not drawn yet; "minimap" and "none" show nothing.
            "minimap" | "none" => Shows { gutter: false, ruler: false },
            _ => Shows { gutter: true, ruler: true },
        }
    }
}

/// The overview ruler's marks of the changes: (line, kind), a big run's lines sampled (the ruler is a few hundred
/// pixels high), at most about 2000 in all.
pub fn ruler_of(changes: &[Change]) -> Vec<(usize, Kind)> {
    let mut out = Vec::new();
    for c in changes {
        if c.kind == Kind::Deleted {
            out.push((target(c), c.kind));
            continue;
        }
        let step = c.count.div_ceil(64).max(1);
        out.extend((c.line..c.line + c.count).step_by(step).map(|l| (l, c.kind)));
    }
    if out.len() > 2000 {
        let step = out.len().div_ceil(2000);
        out = out.into_iter().step_by(step).collect();
    }
    out
}

// ==== The files ====

/// An open file's index text and its changes.
struct File {
    /// The index's text (none: the file is not in it, it is binary or too large, or it has not come yet).
    base: Option<Arc<str>>,
    changes: Rc<Vec<Change>>,
    /// The ruler's marks given to `decor`.
    ruler: Vec<(usize, Kind)>,
    /// Counts the comparisons asked, so only the last one's answer is taken.
    round: u64,
    /// The document it was asked for (a file opened again has another).
    doc: usize,
}

thread_local! {
    static FILES: RefCell<HashMap<String, File>> = RefCell::new(HashMap::new());
}

/// A file's changes now (none when there are none or it is not open).
pub fn of(path: &str) -> Rc<Vec<Change>> {
    FILES.with(|f| f.borrow().get(path).map(|f| f.changes.clone())).unwrap_or_default()
}

/// Counts every change of the files' changes (and of where they are drawn), for the gutter's bars.
#[derive(Clone, Copy)]
struct Quick {
    rev: RwSignal<u64>,
    shows: RwSignal<Shows>,
}

impl crate::ext::State for Quick {
    fn new(ui: Ui) -> Self {
        Quick { rev: ui.scope.create_rw_signal(0), shows: ui.scope.create_rw_signal(Shows::of("all")) }
    }
}

/// The index's text of a file asked of the session.
fn ask(send: &Send, path: &str) {
    send(json!({"type": "git_show", "repo": "", "path": path, "rev": "", "working": false, "key": OWNER}));
}

/// Every open file's index text asked again.
fn ask_all(send: &Send) {
    let paths: Vec<String> = FILES.with(|f| f.borrow().keys().cloned().collect());
    for p in paths {
        ask(send, &p);
    }
}

/// A file's lines compared again after `wait`, unless it is asked again meanwhile.
fn soon(ui: Ui, path: &str, wait: Duration) {
    let Some(round) = FILES.with(|f| {
        f.borrow_mut().get_mut(path).map(|f| {
            f.round += 1;
            f.round
        })
    }) else {
        return;
    };
    let path = path.to_string();
    floem::action::exec_after(wait, move |_| {
        if FILES.with(|f| f.borrow().get(&path).is_some_and(|f| f.round == round)) {
            compare(ui, path, round);
        }
    });
}

/// A file's lines compared with its index text off the window's thread; its marks set when they are others.
fn compare(ui: Ui, path: String, round: u64) {
    let Some(base) = FILES.with(|f| f.borrow().get(&path).map(|f| f.base.clone())) else { return };
    let Some(base) = base else {
        apply(ui, &path, Vec::new());
        return;
    };
    let Some(tab) = ui.tab(&path).filter(|t| t.is_file()) else { return };
    let text = tab.doc.text();
    let at = path.clone();
    let done = floem::ext_event::create_ext_action(ui.scope, move |found: Vec<Change>| {
        if FILES.with(|f| f.borrow().get(&at).is_some_and(|f| f.round == round)) {
            apply(ui, &at, found);
        }
    });
    std::thread::spawn(move || done(changes(&base, &text.to_string())));
}

/// A file's changes taken in: the gutter drawn again and the ruler's marks given, each only when they are others.
fn apply(ui: Ui, path: &str, found: Vec<Change>) {
    let shows = ui.state::<Quick>().shows.get_untracked();
    let ruler = if shows.ruler { ruler_of(&found) } else { Vec::new() };
    let (bars, marks) = FILES.with(|f| {
        let mut files = f.borrow_mut();
        let Some(file) = files.get_mut(path) else { return (false, false) };
        let bars = *file.changes != found;
        if bars {
            file.changes = Rc::new(found);
        }
        let marks = file.ruler != ruler;
        if marks {
            file.ruler = ruler.clone();
        }
        (bars, marks)
    });
    if marks {
        set_ruler(ui, path, &ruler);
    }
    if bars {
        ui.state::<Quick>().rev.update(|r| *r += 1);
    }
}

fn set_ruler(ui: Ui, path: &str, ruler: &[(usize, Kind)]) {
    let marks = ruler.iter().map(|(line, kind)| Mark::Ruler { line: *line, tint: Tint::Theme(ruler_tint(*kind)) });
    crate::decor::set(ui, OWNER, path, marks.collect());
}

fn ruler_tint(kind: Kind) -> fn(&crate::colors::Colors) -> floem::peniko::Color {
    match kind {
        Kind::Added => |c| c.ruler_added,
        Kind::Modified => |c| c.ruler_modified,
        Kind::Deleted => |c| c.ruler_deleted,
    }
}

/// The open file tabs followed: one opened has its index text asked, one closed has its marks taken away.
fn follow_tabs(ui: Ui, send: &Send) {
    let open: Vec<(String, usize)> = ui.tabs.with(|tabs| {
        tabs.iter().filter(|t| t.is_file()).map(|t| (t.path.clone(), Rc::as_ptr(&t.doc) as *const () as usize)).collect()
    });
    let (asked, gone) = FILES.with(|f| {
        let mut files = f.borrow_mut();
        let gone: Vec<String> =
            files.keys().filter(|p| !open.iter().any(|(o, _)| o == *p)).cloned().collect::<Vec<_>>();
        for p in &gone {
            files.remove(p);
        }
        let mut asked = Vec::new();
        for (path, doc) in &open {
            if files.get(path).is_none_or(|f| f.doc != *doc) {
                let file = File { base: None, changes: Rc::default(), ruler: Vec::new(), round: 0, doc: *doc };
                files.insert(path.clone(), file);
                asked.push(path.clone());
            }
        }
        (asked, gone)
    });
    for p in &gone {
        crate::decor::clear(ui, OWNER, p);
    }
    for p in &asked {
        crate::decor::clear(ui, OWNER, p);
        ask(send, p);
    }
    if !gone.is_empty() || !asked.is_empty() {
        ui.state::<Quick>().rev.update(|r| *r += 1);
    }
}

// ==== The gutter ====

/// The bars' strip, laid over the editor's gutter right of the line numbers (where its left is, from the gutter's
/// width): drawn again only when the lines on screen, the scroll, the changes or the theme change.
pub fn bars(ui: Ui, editor: &Editor, path: &str, gutter: RwSignal<f64>) -> impl IntoView + use<> {
    let strip = Bars { id: ViewId::new(), ui, editor: editor.clone(), path: path.to_string() };
    let id = strip.id;
    let (screen, viewport, colors, rev) = (editor.screen_lines, editor.viewport, ui.colors, ui.state::<Quick>().rev);
    ui.scope.create_effect(move |_| {
        screen.track();
        viewport.track();
        colors.track();
        rev.track();
        id.request_paint();
    });
    strip.style(move |s| s.absolute().inset_top(0.0).inset_left(gutter.get() - crate::editor::folding::CHEVRONS).width(WIDTH).height_full())
}

/// The strip's width; the bar is 3 px of it, as VS Code's.
const WIDTH: f64 = 6.0;

struct Bars {
    id: ViewId,
    ui: Ui,
    editor: Editor,
    path: String,
}

impl View for Bars {
    fn id(&self) -> ViewId {
        self.id
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        if !self.ui.state::<Quick>().shows.get_untracked().gutter {
            return;
        }
        let changes = of(&self.path);
        if changes.is_empty() {
            return;
        }
        let c = self.ui.colors.get_untracked();
        let top = self.editor.viewport.get_untracked().y0;
        let row = f64::from(self.editor.line_height(0));
        let lines: Vec<(usize, f64)> = self.editor.screen_lines.with_untracked(|s| s.iter_lines_y().collect());
        for (i, &(line, y)) in lines.iter().enumerate() {
            // A wrapped line's bar is as high as its rows.
            let height = lines.get(i + 1).filter(|n| n.0 == line + 1).map_or(row, |n| n.1 - y);
            let y = y - top;
            // The changes in order of their lines: from the first that does not end above this line.
            let at = changes.partition_point(|c| end(c) <= line);
            for ch in &changes[at..] {
                if ch.line > line + 1 {
                    break;
                }
                match ch.kind {
                    Kind::Added | Kind::Modified if (ch.line..ch.line + ch.count).contains(&line) => {
                        let color = if ch.kind == Kind::Added { c.gutter_added } else { c.gutter_modified };
                        cx.fill(&Rect::new(1.0, y, 4.0, y + height), color, 0.0);
                    }
                    // A triangle on the edge where the lines were: under the line above, or over the first line.
                    Kind::Deleted if ch.line == line + 1 || (ch.line == 0 && line == 0) => {
                        let edge = if ch.line == 0 { y } else { y + height };
                        let mut tri = BezPath::new();
                        tri.move_to(Point::new(1.0, edge - 4.0));
                        tri.line_to(Point::new(5.0, edge));
                        tri.line_to(Point::new(1.0, edge + 4.0));
                        tri.close_path();
                        cx.fill(&tri, c.gutter_deleted, 0.0);
                    }
                    _ => {}
                }
            }
        }
    }
}

// ==== The commands ====

/// The caret put on the next or previous change of the file in the editor with the keys.
fn go(ui: Ui, forward: bool) {
    let Some((path, editor)) = ui.live.get_untracked() else { return };
    let caret = editor.offset_to_line_col(editor.cursor.with_untracked(|c| c.offset())).0;
    let Some(line) = next_line(&of(&path), caret, forward) else { return };
    let at = editor.offset_of_line(line.min(editor.last_line()));
    editor.cursor.update(|c| c.set_insert(Selection::caret(at)));
    crate::editor::folding::center(&editor, &path);
}

/// Whether a file's editor has the keys (not a compare, whose own Next Change has Alt+F5).
fn in_file(ui: Ui) -> bool {
    crate::compare::shown(ui).is_none()
        && ui.live.with_untracked(|l| l.as_ref().is_some_and(|(p, _)| ui.tab(p).is_some_and(|t| t.is_file())))
}

/// What it adds to the window's registries, once at start: the commands and their keys; the open tabs and the edits
/// followed; and `scm.diffDecorations`.
pub fn register(ui: Ui, send: &Send) {
    for (id, title, keys, forward) in [
        ("workbench.action.editor.nextChange", "Go to Next Change", "Alt+F5", true),
        ("workbench.action.editor.previousChange", "Go to Previous Change", "Shift+Alt+F5", false),
        ("editor.action.dirtydiff.next", "Show Next Change", "Alt+F3", true),
        ("editor.action.dirtydiff.previous", "Show Previous Change", "Shift+Alt+F3", false),
    ] {
        commands::add(command(id, "", title, move |ui, _| go(ui, forward)).keys(keys).when(in_file));
    }
    let send = send.clone();
    ui.scope.create_effect(move |_| follow_tabs(ui, &send));
    // A key typed: the editor's file compared again once typing stops.
    let edited = ui.edited;
    ui.scope.create_effect(move |first: Option<()>| {
        edited.track();
        if first.is_some()
            && let Some((path, _)) = ui.live.get_untracked()
        {
            soon(ui, &path, TYPING);
        }
    });
    let quick = ui.state::<Quick>();
    ui.scope.create_effect(move |_| {
        let shows = Shows::of(crate::settings::setting(ui, "scm.diffDecorations", None).as_str().unwrap_or("all"));
        if quick.shows.get_untracked() == shows {
            return;
        }
        quick.shows.set(shows);
        let files: Vec<(String, Rc<Vec<Change>>)> =
            FILES.with(|f| f.borrow().iter().map(|(p, f)| (p.clone(), f.changes.clone())).collect());
        for (path, changes) in files {
            // Given again as they are, so the ruler's marks follow the setting.
            FILES.with(|f| {
                if let Some(file) = f.borrow_mut().get_mut(&path) {
                    file.changes = Rc::default();
                }
            });
            apply(ui, &path, (*changes).clone());
        }
        quick.rev.update(|r| *r += 1);
    });
}

/// The session's messages that are this part's, taken in: the index's text (taken away from the parts after it), and
/// the file's lines compared again when it is saved, staged or committed, or changed on disk.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "git_shown" if m["key"] == OWNER => {
            let path = m["path"].as_str().unwrap_or("");
            let base: Option<Arc<str>> =
                if m["exists"].as_bool().unwrap_or(false) { m["text"].as_str().map(Arc::from) } else { None };
            let changed = FILES.with(|f| {
                let mut files = f.borrow_mut();
                let Some(file) = files.get_mut(path) else { return false };
                if file.base == base {
                    return false;
                }
                file.base = base;
                true
            });
            if changed {
                soon(ui, path, Duration::ZERO);
            }
            return true;
        }
        "git_status" if m["repo"].as_str().unwrap_or("").is_empty() => ask_all(send),
        "git_committed" => ask_all(send),
        "saved" => {
            if let Some(path) = m["path"].as_str() {
                ask(send, path);
            }
        }
        // Read again from disk (an agent's edit): its lines compared again once the tab has the text.
        "file" => {
            if let Some(path) = m["path"].as_str() {
                soon(ui, path, Duration::from_millis(50));
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(kind: Kind, line: usize, count: usize) -> Change {
        Change { kind, line, count }
    }

    #[test]
    fn added_modified_and_deleted_lines() {
        assert_eq!(changes("a\nb\nc\n", "a\nb\nc\n"), vec![]);
        assert_eq!(changes("a\nc\n", "a\nb\nc\n"), vec![ch(Kind::Added, 1, 1)]);
        assert_eq!(changes("a\nb\nc\n", "a\nB\nc\n"), vec![ch(Kind::Modified, 1, 1)]);
        assert_eq!(changes("a\nb\nc\n", "a\nc\n"), vec![ch(Kind::Deleted, 1, 1)]);
        assert_eq!(
            changes("a\nb\nc\nd\ne\n", "x\na\nc\nD\ne\nf\n"),
            vec![ch(Kind::Added, 0, 1), ch(Kind::Deleted, 2, 1), ch(Kind::Modified, 3, 1), ch(Kind::Added, 5, 1)]
        );
    }

    #[test]
    fn at_the_start_and_the_end() {
        assert_eq!(changes("a\nb\n", "b\n"), vec![ch(Kind::Deleted, 0, 1)]);
        assert_eq!(changes("a\nb\n", "a\n"), vec![ch(Kind::Deleted, 1, 1)]);
        assert_eq!(changes("", "a\nb\n"), vec![ch(Kind::Added, 0, 2)]);
        assert_eq!(changes("a\nb\n", ""), vec![ch(Kind::Deleted, 0, 2)]);
    }

    #[test]
    fn line_ends_are_not_counted() {
        assert_eq!(changes("a\r\nb\r\nc\r\n", "a\nb\nc\n"), vec![]);
        assert_eq!(changes("a\r\nb\r\n", "a\nB\n"), vec![ch(Kind::Modified, 1, 1)]);
    }

    #[test]
    fn past_the_budget_one_change() {
        let before = "same\na\nb\nc\nd\nend\n";
        let after = "same\n1\nb\n2\nd\n3\n4\nend\n";
        assert_eq!(changes_within(before, after, 2), vec![ch(Kind::Modified, 1, 6)]);
        assert_eq!(changes_within(before, after, BUDGET).len(), 3);
    }

    #[test]
    fn myers_finds_the_fewest_edits() {
        let a = ["a", "b", "c", "a", "b", "b", "a"];
        let b = ["c", "b", "a", "b", "a", "c"];
        let same = myers(&a, &b, BUDGET).expect("within the budget");
        // The longest run both have is four lines long (D = 13 - 2·4 = 5).
        assert_eq!(same.len(), 4);
        assert!(same.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1));
        assert!(same.iter().all(|&(i, j)| a[i] == b[j]));
    }

    #[test]
    fn next_and_previous_round_the_ends() {
        let c = [ch(Kind::Added, 2, 1), ch(Kind::Deleted, 6, 1), ch(Kind::Modified, 9, 2)];
        assert_eq!(next_line(&c, 0, true), Some(2));
        assert_eq!(next_line(&c, 2, true), Some(5));
        assert_eq!(next_line(&c, 9, true), Some(2));
        assert_eq!(next_line(&c, 9, false), Some(5));
        assert_eq!(next_line(&c, 2, false), Some(9));
        assert_eq!(next_line(&[], 3, true), None);
    }

    #[test]
    fn where_the_setting_draws() {
        assert_eq!(Shows::of("all"), Shows { gutter: true, ruler: true });
        assert_eq!(Shows::of("gutter"), Shows { gutter: true, ruler: false });
        assert_eq!(Shows::of("overview"), Shows { gutter: false, ruler: true });
        assert_eq!(Shows::of("none"), Shows { gutter: false, ruler: false });
    }

    #[test]
    fn the_ruler_samples_big_runs() {
        assert_eq!(ruler_of(&[ch(Kind::Modified, 3, 2)]), vec![(3, Kind::Modified), (4, Kind::Modified)]);
        assert_eq!(ruler_of(&[ch(Kind::Added, 0, 100_000)]).len(), 64);
        assert_eq!(ruler_of(&[ch(Kind::Deleted, 0, 5)]), vec![(0, Kind::Deleted)]);
    }
}
