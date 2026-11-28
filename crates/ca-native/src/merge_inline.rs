//! A file's merge conflicts in its editor, as VS Code's built-in Merge Conflict extension shows them: the lines
//! between `<<<<<<<`, `|||||||`, `=======` and `>>>>>>>` read as conflicts, the current change green, the incoming
//! blue and the common ancestor grey, each marker line saying which it is, with marks in the overview ruler. After the
//! `<<<<<<<` line its CodeLens: Accept Current Change | Accept Incoming Change | Accept Both Changes (| Resolve in
//! Merge Editor, when the merge editor is there), each one edit undone at once. The Merge Conflict commands accept the
//! conflict at the caret or all of them, and go to the next or previous one. The file's text is read again (off the
//! window's thread) 100 ms after an edit; `merge-conflict.decorators.enabled` and `merge-conflict.codeLens.enabled`
//! turn the colours and the lens off. What every part has (`register`, `take`) is said in `ext.rs`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use floem::IntoView;
use floem::event::{Event, EventListener};
use floem::kurbo::Point;
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_rw_signal};
use floem::views::editor::Editor;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;
use floem::views::{Decorators, container};
use serde_json::Value;

use crate::commands::{self, command};
use crate::decor::{Mark, Phantom, Tint};
use crate::state::{Send, Ui};

/// The owner of the marks.
const OWNER: &str = "merge-conflict";
/// How long after an edit the text is read again, VS Code's.
const AFTER_EDIT: Duration = Duration::from_millis(100);
/// The command the lens's last entry runs: the 3-way merge editor's, shown only when it is registered.
pub const MERGE_EDITOR: &str = "git.openMergeEditor";
/// What the commands say when the file has none.
pub const NONE: &str = "There is no merge conflict in this file.";
/// What accepting one says when the caret is in none, VS Code's.
pub const NOT_IN_ONE: &str = "Editor cursor is not within a merge conflict";

/// One conflict: the lines of its markers (from 0). `base` is the `|||||||` line of a diff3 conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub start: usize,
    pub base: Option<usize>,
    pub split: usize,
    pub end: usize,
}

/// Which side a conflict is resolved with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Current,
    Incoming,
    Both,
}

/// A line's text without its line end.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l))
}

/// The conflicts of a text, in order. A `<<<<<<<` before the last one ended starts again there; one that never ends
/// is not one; a `|||||||` or `=======` out of place is text.
pub fn parse(text: &str) -> Vec<Conflict> {
    let mut found = Vec::new();
    let mut open: Option<(usize, Option<usize>, Option<usize>)> = None;
    for (i, l) in lines(text).enumerate() {
        if l.starts_with("<<<<<<<") {
            open = Some((i, None, None));
        } else if let Some((start, base, split)) = open.as_mut() {
            if split.is_none() && base.is_none() && l.starts_with("|||||||") {
                *base = Some(i);
            } else if split.is_none() && l.starts_with("=======") {
                *split = Some(i);
            } else if let Some(split) = *split
                && l.starts_with(">>>>>>>")
            {
                found.push(Conflict { start: *start, base: *base, split, end: i });
                open = None;
            }
        }
    }
    found
}

/// Where each line begins, and the text's end after them.
fn starts(text: &str) -> Vec<usize> {
    let mut at = vec![0];
    at.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    if *at.last().unwrap_or(&0) != text.len() {
        at.push(text.len());
    }
    at
}

/// The edit that resolves a conflict: from, to (offsets; the marker lines with their line ends) and the lines kept.
pub fn resolve(text: &str, c: &Conflict, choice: Choice) -> (usize, usize, String) {
    let at = starts(text);
    let offset = |line: usize| at.get(line).copied().unwrap_or(text.len());
    let span = |from: usize, to: usize| &text[offset(from)..offset(to)];
    let current = span(c.start + 1, c.base.unwrap_or(c.split));
    let incoming = span(c.split + 1, c.end);
    let kept = match choice {
        Choice::Current => current.to_string(),
        Choice::Incoming => incoming.to_string(),
        Choice::Both => {
            // Both, the incoming on the lines after the current even when the current has no line end of its own.
            let mut both = current.to_string();
            if !both.is_empty() && !both.ends_with('\n') && !incoming.is_empty() {
                both.push('\n');
            }
            both + incoming
        }
    };
    (offset(c.start), offset(c.end + 1), kept)
}

/// The conflict a line is in.
pub fn at(conflicts: &[Conflict], line: usize) -> Option<Conflict> {
    conflicts.iter().copied().find(|c| (c.start..=c.end).contains(&line))
}

/// The next (or previous) conflict's first line from a line, round the file's ends.
pub fn next(conflicts: &[Conflict], line: usize, forward: bool) -> Option<usize> {
    let starts = conflicts.iter().map(|c| c.start);
    if forward {
        starts.clone().find(|s| *s > line).or_else(|| conflicts.first().map(|c| c.start))
    } else {
        starts.rev().find(|s| *s < line).or_else(|| conflicts.last().map(|c| c.start))
    }
}

/// The lens's entries, VS Code's words; the merge editor's only when it is there.
fn entries(editor: bool) -> Vec<(&'static str, Pick)> {
    let mut e = vec![
        ("Accept Current Change", Pick::Accept(Choice::Current)),
        ("Accept Incoming Change", Pick::Accept(Choice::Incoming)),
        ("Accept Both Changes", Pick::Accept(Choice::Both)),
    ];
    if editor {
        e.push(("Resolve in Merge Editor", Pick::Editor));
    }
    e
}

/// What a lens entry does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Accept(Choice),
    Editor,
}

/// Between the lens's entries, VS Code's.
const BETWEEN: &str = " | ";
/// Before the lens, after what the marker line says.
const GAP: &str = "     ";

/// The lens's text after the `<<<<<<<` line.
fn lens_text(editor: bool) -> String {
    let words: Vec<&str> = entries(editor).iter().map(|e| e.0).collect();
    format!("{GAP}{}", words.join(BETWEEN))
}

/// The lens entry at a byte of a line's text drawn with its phantom text (none: not on an entry).
pub fn pick_at(drawn: &str, index: usize, editor: bool) -> Option<Pick> {
    let lens = lens_text(editor);
    let mut from = drawn.rfind(&lens)? + GAP.len();
    for (word, pick) in entries(editor) {
        if (from..from + word.len()).contains(&index) {
            return Some(pick);
        }
        from += word.len() + BETWEEN.len();
    }
    None
}

/// What the settings show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shows {
    pub colours: bool,
    pub lens: bool,
    pub editor: bool,
}

/// The marks of a file's conflicts.
pub fn marks(conflicts: &[Conflict], shows: Shows) -> Vec<Mark> {
    type Pick = fn(&crate::colors::Colors) -> floem::peniko::Color;
    let mut out = Vec::new();
    let label = |out: &mut Vec<Mark>, line: usize, text: String| {
        out.push(Mark::Text { line, col: 0, text, at: Phantom::Lens });
    };
    for c in conflicts {
        if shows.colours {
            let base = c.base.unwrap_or(c.split);
            let mut paint = |from: usize, to: usize, bg: Pick, ruler: Pick| {
                for line in from..to {
                    out.push(Mark::Line { line, bg: Tint::Theme(bg) });
                    out.push(Mark::Ruler { line, tint: Tint::Theme(ruler) });
                }
            };
            paint(c.start, c.start + 1, |c| c.merge_current_header, |c| c.ruler_current);
            paint(c.start + 1, base, |c| c.merge_current_content, |c| c.ruler_current);
            if let Some(b) = c.base {
                paint(b, b + 1, |c| c.merge_common_header, |c| c.ruler_common);
                paint(b + 1, c.split, |c| c.merge_common_content, |c| c.ruler_common);
            }
            paint(c.split + 1, c.end, |c| c.merge_incoming_content, |c| c.ruler_incoming);
            paint(c.end, c.end + 1, |c| c.merge_incoming_header, |c| c.ruler_incoming);
        }
        let mut head = if shows.colours { " (Current Change)".to_string() } else { String::new() };
        if shows.lens {
            head += &lens_text(shows.editor);
        }
        if !head.is_empty() {
            label(&mut out, c.start, head);
        }
        if shows.colours {
            if let Some(b) = c.base {
                label(&mut out, b, " (Common Ancestor)".into());
            }
            label(&mut out, c.end, " (Incoming Change)".into());
        }
    }
    out
}

// ==== Following the open files ====

/// An open file's conflicts as last read, and what was given for them.
struct File {
    doc: usize,
    round: u64,
    conflicts: Rc<Vec<Conflict>>,
    given: Option<(Rc<Vec<Conflict>>, Shows)>,
}

thread_local! {
    static FILES: RefCell<HashMap<String, File>> = RefCell::new(HashMap::new());
}

/// A file's conflicts as last read (none when it has none or is not open).
pub fn of(path: &str) -> Rc<Vec<Conflict>> {
    FILES.with(|f| f.borrow().get(path).map(|f| f.conflicts.clone())).unwrap_or_default()
}

/// What the settings show now.
#[derive(Clone, Copy)]
struct Merge {
    shows: RwSignal<Shows>,
}

impl crate::ext::State for Merge {
    fn new(ui: Ui) -> Self {
        Merge { shows: ui.scope.create_rw_signal(Shows { colours: true, lens: true, editor: false }) }
    }
}

/// A file's conflicts read again a moment after an edit made where the editor with the keys is not its own (the
/// merge editor's buttons).
pub fn edited(ui: Ui, path: &str) {
    soon(ui, path, AFTER_EDIT);
}

/// A file read again after `wait`, unless it is asked again meanwhile.
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
    floem::action::exec_after(wait, move |_| read(ui, path, round));
}

/// A file's conflicts read off the window's thread; its marks given when they are others.
fn read(ui: Ui, path: String, round: u64) {
    let current = move |p: &str| FILES.with(|f| f.borrow().get(p).is_some_and(|f| f.round == round));
    if !current(&path) {
        return;
    }
    let Some(tab) = ui.tab(&path).filter(|t| t.is_file()) else { return };
    let text = tab.doc.text();
    let at = path.clone();
    let done = floem::ext_event::create_ext_action(ui.scope, move |found: Vec<Conflict>| {
        if current(&at) {
            FILES.with(|f| {
                if let Some(file) = f.borrow_mut().get_mut(&at) {
                    file.conflicts = Rc::new(found);
                }
            });
            give(ui, &at);
        }
    });
    std::thread::spawn(move || {
        let text = text.to_string();
        done(if text.contains("<<<<<<<") { parse(&text) } else { Vec::new() })
    });
}

/// A file's marks given for its conflicts and the settings, only when they are others than those given.
fn give(ui: Ui, path: &str) {
    let mut shows = ui.state::<Merge>().shows.get_untracked();
    shows.editor = commands::find(MERGE_EDITOR).is_some();
    let Some((conflicts, before)) =
        FILES.with(|f| f.borrow().get(path).map(|f| (f.conflicts.clone(), f.given.clone())))
    else {
        return;
    };
    let same = match &before {
        Some((c, s)) => **c == *conflicts && (*s == shows || conflicts.is_empty()),
        None => conflicts.is_empty(),
    };
    if same {
        return;
    }
    if conflicts.is_empty() {
        crate::decor::clear(ui, OWNER, path);
    } else {
        crate::decor::set(ui, OWNER, path, marks(&conflicts, shows));
    }
    FILES.with(|f| {
        if let Some(file) = f.borrow_mut().get_mut(path) {
            file.given = Some((conflicts, shows));
        }
    });
}

/// The open file tabs followed: one opened (or its text taken from the disk) is read, one closed loses its marks.
fn follow_tabs(ui: Ui) {
    let open: Vec<(String, usize)> = ui.tabs.with(|tabs| {
        tabs.iter()
            .filter(|t| t.is_file())
            .map(|t| (t.path.clone(), Rc::as_ptr(&t.doc) as *const () as usize))
            .collect()
    });
    let (fresh, gone) = FILES.with(|f| {
        let mut files = f.borrow_mut();
        let gone: Vec<String> = files.keys().filter(|p| !open.iter().any(|(o, _)| o == *p)).cloned().collect();
        for p in &gone {
            files.remove(p);
        }
        let mut fresh = Vec::new();
        for (path, doc) in &open {
            if files.get(path).is_none_or(|f| f.doc != *doc) {
                let given = files.get(path).and_then(|f| f.given.clone());
                files.insert(path.clone(), File { doc: *doc, round: 0, conflicts: Rc::default(), given });
                fresh.push(path.clone());
            }
        }
        (fresh, gone)
    });
    for p in &gone {
        crate::decor::clear(ui, OWNER, p);
    }
    for p in &fresh {
        soon(ui, p, Duration::ZERO);
    }
}

// ==== Resolving ====

/// The editor with the keys, and its file, when it is a file's.
fn live(ui: Ui) -> Option<(String, Editor)> {
    ui.live.get_untracked().filter(|(p, _)| ui.tab(p).is_some_and(|t| t.is_file()))
}

/// Conflicts resolved in an editor, all in one edit (one undo), read from its text as it is now.
fn accept(e: &Editor, pick: impl Fn(&[Conflict]) -> Vec<Conflict>, choice: Choice) -> bool {
    let text = e.doc().text().to_string();
    let chosen = pick(&parse(&text));
    if chosen.is_empty() {
        return false;
    }
    if e.read_only.get_untracked() {
        return true;
    }
    let edits: Vec<(usize, usize, String)> = chosen.iter().map(|c| resolve(&text, c, choice)).collect();
    let caret = caret_after(e.cursor.with_untracked(|c| c.offset()), &edits);
    let mut each = edits.iter().map(|(a, z, by)| (Selection::region(*a, *z), by.as_str()));
    e.doc().edit(&mut each, EditType::Other);
    e.cursor.update(|c| c.set_insert(Selection::caret(caret)));
    true
}

/// Where a caret is after edits (in order, apart): moved by what changed before it, and to the start of a region it
/// was in.
pub fn caret_after(caret: usize, edits: &[(usize, usize, String)]) -> usize {
    let mut shift: isize = 0;
    for (a, z, by) in edits {
        if caret < *a {
            break;
        }
        if caret < *z {
            return a.saturating_add_signed(shift);
        }
        shift += by.len() as isize - (z - a) as isize;
    }
    caret.saturating_add_signed(shift)
}

/// The conflict at the caret accepted (said when there is none).
fn accept_here(ui: Ui, choice: Choice) {
    let Some((_, e)) = live(ui) else { return };
    let line = e.offset_to_line_col(e.cursor.with_untracked(|c| c.offset())).0;
    if !accept(&e, |all| at(all, line).into_iter().collect(), choice) {
        crate::widgets::toast::info(ui, NOT_IN_ONE);
    }
}

/// Every conflict of the file accepted.
fn accept_all(ui: Ui, choice: Choice) {
    let Some((_, e)) = live(ui) else { return };
    if !accept(&e, |all| all.to_vec(), choice) {
        crate::widgets::toast::info(ui, NONE);
    }
}

/// The caret put on the next or previous conflict.
fn go(ui: Ui, forward: bool) {
    let Some((path, e)) = live(ui) else { return };
    let conflicts = parse(&e.doc().text().to_string());
    let line = e.offset_to_line_col(e.cursor.with_untracked(|c| c.offset())).0;
    let Some(to) = next(&conflicts, line, forward) else {
        crate::widgets::toast::info(ui, NONE);
        return;
    };
    let at = e.offset_of_line(to);
    e.cursor.update(|c| c.set_insert(Selection::caret(at)));
    crate::editor::folding::center(&e, &path);
}

/// The editor of a file with its lens's clicks: a click on an entry after a `<<<<<<<` line does what it says.
pub fn follow<V: IntoView + 'static>(ui: Ui, view: V, editor: &Editor, path: &str) -> impl IntoView + use<V> {
    let origin = create_rw_signal(Point::ZERO);
    let (e, path) = (editor.clone(), path.to_string());
    container(view)
        .on_move(move |p| origin.set(p))
        .on_event_cont(EventListener::PointerDown, move |ev| {
            let Event::PointerDown(p) = ev else { return };
            if !p.button.is_primary() || of(&path).is_empty() {
                return;
            }
            let win = p.pos + origin.get_untracked().to_vec2();
            let Some((line, pick)) = lens_under(&e, &path, win) else { return };
            match pick {
                Pick::Accept(choice) => {
                    accept(&e, |all| all.iter().copied().filter(|c| c.start == line).collect(), choice);
                }
                Pick::Editor => {
                    if let Some(send) = ui.out.get_untracked() {
                        commands::run(ui, &send, MERGE_EDITOR);
                    }
                }
            }
        })
        .style(|s| s.size_full())
}

/// The lens entry under a point of the window over an editor, and its line.
fn lens_under(e: &Editor, path: &str, win: Point) -> Option<(usize, Pick)> {
    let (o, v) = (e.window_origin.get_untracked(), e.viewport.get_untracked());
    if win.x < o.x {
        return None;
    }
    let point = Point::new(win.x - o.x + v.x0, win.y - o.y + v.y0);
    let (line, index) = e.line_col_of_point_with_phantom(point);
    if !of(path).iter().any(|c| c.start == line) {
        return None;
    }
    let layout = e.text_layout(line);
    // A point right of the line's text is on none (the hit is put on its last byte).
    if point.x > layout.text.size().width {
        return None;
    }
    let drawn = layout.text.lines().first().map(|l| l.text().to_string())?;
    pick_at(&drawn, index, commands::find(MERGE_EDITOR).is_some()).map(|p| (line, p))
}

/// Whether a file's editor has the keys.
fn in_file(ui: Ui) -> bool {
    live(ui).is_some()
}

/// What it adds to the window's registries, once at start: the Merge Conflict commands; the open tabs and their edits
/// followed; and the two settings.
pub fn register(ui: Ui, _send: &Send) {
    for (id, title, choice) in [
        ("merge-conflict.accept.current", "Accept Current", Choice::Current),
        ("merge-conflict.accept.incoming", "Accept Incoming", Choice::Incoming),
        ("merge-conflict.accept.both", "Accept Both", Choice::Both),
    ] {
        commands::add(command(id, "Merge Conflict", title, move |ui, _| accept_here(ui, choice)).when(in_file));
    }
    for (id, title, choice) in [
        ("merge-conflict.accept.all-current", "Accept All Current", Choice::Current),
        ("merge-conflict.accept.all-incoming", "Accept All Incoming", Choice::Incoming),
        ("merge-conflict.accept.all-both", "Accept All Both", Choice::Both),
    ] {
        commands::add(command(id, "Merge Conflict", title, move |ui, _| accept_all(ui, choice)).when(in_file));
    }
    for (id, title, forward) in
        [("merge-conflict.next", "Next Conflict", true), ("merge-conflict.previous", "Previous Conflict", false)]
    {
        commands::add(command(id, "Merge Conflict", title, move |ui, _| go(ui, forward)).when(in_file));
    }
    ui.scope.create_effect(move |_| follow_tabs(ui));
    // A key typed: the editor's file read again once it has been still for a moment.
    let edited = ui.edited;
    ui.scope.create_effect(move |first: Option<()>| {
        edited.track();
        if first.is_some()
            && let Some((path, _)) = ui.live.get_untracked()
        {
            soon(ui, &path, AFTER_EDIT);
        }
    });
    let merge = ui.state::<Merge>();
    ui.scope.create_effect(move |_| {
        let colours = crate::settings::flag(ui, "merge-conflict.decorators.enabled", None);
        let lens = crate::settings::flag(ui, "merge-conflict.codeLens.enabled", None);
        let now = Shows { colours, lens, editor: false };
        if merge.shows.get_untracked() == now {
            return;
        }
        merge.shows.set(now);
        let paths: Vec<String> = FILES.with(|f| f.borrow().keys().cloned().collect());
        for p in paths {
            give(ui, &p);
        }
    });
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = "a\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> topic\nb\n";
    const DIFF3: &str = "<<<<<<< HEAD\nmine\n||||||| base\nold\n=======\ntheirs\n>>>>>>> topic\n";

    fn applied(text: &str, choice: Choice) -> String {
        let c = parse(text)[0];
        let (a, z, by) = resolve(text, &c, choice);
        format!("{}{}{}", &text[..a], by, &text[z..])
    }

    #[test]
    fn conflicts_are_read_with_and_without_a_common_ancestor() {
        assert_eq!(parse(TWO), vec![Conflict { start: 1, base: None, split: 3, end: 5 }]);
        assert_eq!(parse(DIFF3), vec![Conflict { start: 0, base: Some(2), split: 4, end: 6 }]);
        assert_eq!(parse("no markers\n"), vec![]);
    }

    #[test]
    fn broken_markers_are_text() {
        // Never ended, a second start before the first ended, and an end before any split.
        assert_eq!(parse("<<<<<<< a\nx\n=======\ny\n"), vec![]);
        let restarted = "<<<<<<< a\nx\n<<<<<<< b\ny\n=======\nz\n>>>>>>> c\n";
        assert_eq!(parse(restarted), vec![Conflict { start: 2, base: None, split: 4, end: 6 }]);
        assert_eq!(parse("<<<<<<< a\n>>>>>>> c\n"), vec![]);
        // A ======= inside the incoming change is its text.
        let twice = "<<<<<<< a\nx\n=======\ny\n=======\n>>>>>>> c\n";
        assert_eq!(parse(twice), vec![Conflict { start: 0, base: None, split: 2, end: 5 }]);
    }

    #[test]
    fn each_side_or_both_are_kept() {
        assert_eq!(applied(TWO, Choice::Current), "a\nmine\nb\n");
        assert_eq!(applied(TWO, Choice::Incoming), "a\ntheirs\nb\n");
        assert_eq!(applied(TWO, Choice::Both), "a\nmine\ntheirs\nb\n");
        assert_eq!(applied(DIFF3, Choice::Current), "mine\n");
        assert_eq!(applied(DIFF3, Choice::Both), "mine\ntheirs\n");
    }

    #[test]
    fn crlf_and_a_last_line_with_no_line_end() {
        let crlf = "<<<<<<< HEAD\r\nmine\r\n=======\r\ntheirs\r\n>>>>>>> topic\r\nb\r\n";
        assert_eq!(parse(crlf), vec![Conflict { start: 0, base: None, split: 2, end: 4 }]);
        assert_eq!(applied(crlf, Choice::Incoming), "theirs\r\nb\r\n");
        let open_end = "<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> topic";
        assert_eq!(applied(open_end, Choice::Both), "mine\ntheirs\n");
        let empty = "<<<<<<< HEAD\n=======\ntheirs\n>>>>>>> topic\n";
        assert_eq!(applied(empty, Choice::Current), "");
        assert_eq!(applied(empty, Choice::Both), "theirs\n");
    }

    #[test]
    fn the_caret_follows_the_edits() {
        let edits = vec![(2, 10, "ab".to_string()), (20, 30, String::new())];
        assert_eq!(caret_after(1, &edits), 1);
        assert_eq!(caret_after(5, &edits), 2);
        assert_eq!(caret_after(12, &edits), 6);
        assert_eq!(caret_after(25, &edits), 14);
        assert_eq!(caret_after(31, &edits), 15);
    }

    #[test]
    fn the_conflict_at_a_line_and_the_next_round_the_ends() {
        let all = parse(&format!("{TWO}{TWO}"));
        assert_eq!(all.iter().map(|c| c.start).collect::<Vec<_>>(), vec![1, 8]);
        assert_eq!(at(&all, 3).map(|c| c.start), Some(1));
        assert_eq!(at(&all, 6), None);
        assert_eq!(next(&all, 0, true), Some(1));
        assert_eq!(next(&all, 1, true), Some(8));
        assert_eq!(next(&all, 9, true), Some(1));
        assert_eq!(next(&all, 9, false), Some(8));
        assert_eq!(next(&all, 8, false), Some(1));
        assert_eq!(next(&all, 0, false), Some(8));
        assert_eq!(next(&[], 0, true), None);
    }

    #[test]
    fn a_click_on_the_lens_finds_its_entry() {
        let drawn = format!("<<<<<<< HEAD (Current Change){}", lens_text(false));
        let first = drawn.find("Accept Current").expect("in the lens");
        assert_eq!(pick_at(&drawn, first, false), Some(Pick::Accept(Choice::Current)));
        let both = drawn.find("Both Changes").expect("in the lens");
        assert_eq!(pick_at(&drawn, both, false), Some(Pick::Accept(Choice::Both)));
        let bar = drawn.find(" | ").expect("in the lens");
        assert_eq!(pick_at(&drawn, bar + 1, false), None);
        assert_eq!(pick_at(&drawn, 3, false), None);
        let with = format!("<<<<<<< HEAD{}", lens_text(true));
        assert_eq!(pick_at(&with, with.find("Resolve").expect("in the lens"), true), Some(Pick::Editor));
    }

    #[test]
    fn the_marks_follow_the_settings() {
        let c = parse(DIFF3);
        let on = Shows { colours: true, lens: true, editor: false };
        let m = marks(&c, on);
        let lines = m.iter().filter(|m| matches!(m, Mark::Line { .. })).count();
        // Every line but the =======.
        assert_eq!(lines, 6);
        let texts: Vec<(usize, String)> = m
            .iter()
            .filter_map(|m| match m {
                Mark::Text { line, text, .. } => Some((*line, text.clone())),
                _ => None,
            })
            .collect();
        assert!(texts[0].1.starts_with(" (Current Change)     Accept Current Change | "));
        assert_eq!(texts[1], (2, " (Common Ancestor)".into()));
        assert_eq!(texts[2], (6, " (Incoming Change)".into()));
        let lens_only = marks(&c, Shows { colours: false, ..on });
        assert_eq!(lens_only.len(), 1);
        assert!(marks(&c, Shows { colours: false, lens: false, editor: false }).is_empty());
    }
}
