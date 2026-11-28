//! Staging, unstaging, reverting and accepting chosen lines, as VS Code's Stage Selected Ranges: the changes the
//! caret or the selection of the modified side touches, each cut down to the lines it touches (VS Code's
//! `intersectDiffWithRange`), put into the other text and nothing else.
//!
//! - Stage Selected Ranges (Ctrl+K Ctrl+Alt+S): in a compare of the index with the file, the index gets those
//!   lines; the driver writes them with `git_stage_text`.
//! - Unstage Selected Ranges (Ctrl+K Ctrl+N, Ctrl+K Ctrl+Alt+N): in a compare of HEAD with the index, the index
//!   gets HEAD's lines back there.
//! - Revert Selected Ranges (Ctrl+K Ctrl+R): in a compare whose modified side is the file in its editor, those
//!   lines get the original's back, as one edit (one undo).
//! - Accept the Agent's Selected Changes (Ctrl+K Ctrl+Alt+A): in a compare of the file before the agent, that text
//!   gets the agent's lines there (`changes_accept_text`), so they no longer show as the agent's.
//!
//! The lines are compared again for this (line ends' spaces counted), off the window's thread. The commands and
//! their buttons in the compare's toolbar are here; the sides are `compare::Source::{Index, Head}`.

use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use floem::reactive::SignalGet;
use floem::views::editor::Editor;
use floem::views::editor::core::cursor::CursorMode;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;
use serde_json::{Value, json};

use super::align::{self, Hunk};
use super::{Entry, Source};
use crate::commands::{self, command};
use crate::state::{Send, Ui};
use crate::widgets::toast;

pub const STAGE: &str = "git.stageSelectedRanges";
pub const UNSTAGE: &str = "git.unstageSelectedRanges";
pub const REVERT: &str = "git.revertSelectedRanges";
pub const ACCEPT: &str = "codeArena.acceptSelectedChanges";

/// What is said when the caret and the selection touch no change.
pub const NONE_TOUCHED: &str = "Put the cursor in a change, or select the lines of the changes.";

/// The longest the lines are compared for a command.
const MOST: Duration = Duration::from_secs(3);

/// Lines of the modified side a selection covers, the first and the last (from 0).
pub type Lines = (usize, usize);

/// The lines a selection covers, from its two ends' lines and the column of its end: a selection that ends at the
/// start of a line does not take that line.
pub fn lines_of(start: (usize, usize), end: (usize, usize)) -> Lines {
    let ((a, _), (b, col)) = if start <= end { (start, end) } else { (end, start) };
    if b > a && col == 0 { (a, b - 1) } else { (a, b) }
}

/// A hunk cut down to the lines `first..=last` of the modified side touch, as VS Code's: where both sides have as
/// many lines, line by line; lines put in, those of them; otherwise the original's lines with the touched ones. A
/// hunk that only took lines away is touched from the line above it or the line where they were.
fn narrow(h: &Hunk, (first, last): Lines) -> Option<Hunk> {
    if h.right.is_empty() {
        let at = h.right.start;
        return (first <= at && last + 1 >= at).then(|| h.clone());
    }
    let (from, to) = (first.max(h.right.start), (last + 1).min(h.right.end));
    if from >= to {
        return None;
    }
    let left = if h.left.len() == h.right.len() {
        let delta = from - h.right.start;
        h.left.start + delta..h.left.start + delta + (to - from)
    } else {
        h.left.clone()
    };
    Some(Hunk { left, right: from..to })
}

/// The hunks the selections touch, each cut down to what they touch (several selections in one hunk: from the
/// first line any touches to the last), in order.
pub fn touched(hunks: &[Hunk], sels: &[Lines]) -> Vec<Hunk> {
    hunks
        .iter()
        .filter_map(|h| {
            let pieces: Vec<Hunk> = sels.iter().filter_map(|s| narrow(h, *s)).collect();
            let first = pieces.iter().map(|p| p.right.start).min()?;
            let last = pieces.iter().map(|p| p.right.end).max()?;
            if h.right.is_empty() { Some(h.clone()) } else { narrow(h, (first, last - 1)) }
        })
        .collect()
}

/// A hunk read the other way: what goes back.
pub fn mirror(h: &Hunk) -> Hunk {
    Hunk { left: h.right.clone(), right: h.left.clone() }
}

/// `base` with each hunk's lines (its `left`) given `other`'s (its `right`), the rest as it was. A line brought in
/// takes `base`'s line ends (`\r\n` or `\n`).
pub fn put(base: &str, other: &str, hunks: &[Hunk]) -> String {
    let (a, b) = (align::lines(base), align::lines(other));
    let crlf = base.find('\n').map(|i| base[..i].ends_with('\r'));
    let mut hunks: Vec<&Hunk> = hunks.iter().collect();
    hunks.sort_by_key(|h| h.left.start);
    let mut out: Vec<String> = Vec::with_capacity(a.len());
    let mut i = 0;
    for h in hunks {
        if h.left.start < i || h.left.end > a.len() || h.right.end > b.len() {
            continue;
        }
        out.extend(a[i..h.left.start].iter().map(|l| l.to_string()));
        for (n, l) in b[h.right.clone()].iter().enumerate() {
            // The last line has no line end after it.
            let ended = h.right.start + n + 1 < b.len();
            out.push(match crlf {
                Some(false) if ended => l.strip_suffix('\r').unwrap_or(l).to_string(),
                Some(true) if ended && !l.ends_with('\r') => format!("{l}\r"),
                _ => l.to_string(),
            });
        }
        i = h.left.end;
    }
    out.extend(a[i.min(a.len())..].iter().map(|l| l.to_string()));
    out.join("\n")
}

/// What a command does with the changes it touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// The original gets the modified side's lines (stage, accept); the original's new text is the answer.
    Take,
    /// The modified side gets the original's lines back (unstage, revert); the modified side's new text is it.
    GiveBack,
}

/// The new text a command makes from the two sides and the modified side's selections; none when they touch no
/// change. The lines are compared again with their ends' spaces counted.
pub fn work(left: &str, right: &str, sels: &[Lines], how: How) -> Option<String> {
    let c = align::compare(left, right, false, Some(MOST));
    let chosen = touched(&c.hunks, sels);
    if chosen.is_empty() {
        return None;
    }
    Some(match how {
        How::Take => put(left, right, &chosen),
        How::GiveBack => put(right, left, &chosen.iter().map(mirror).collect::<Vec<_>>()),
    })
}

/// The one edit that makes `old` into `new`: the bytes of `old` replaced and the text put there (what both begin
/// and end with left alone).
pub fn one_edit<'a>(old: &str, new: &'a str) -> (Range<usize>, &'a str) {
    let mut head = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(head) || !new.is_char_boundary(head) {
        head -= 1;
    }
    let most = old.len().min(new.len()) - head;
    let mut tail = old.bytes().rev().zip(new.bytes().rev()).take(most).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(old.len() - tail) || !new.is_char_boundary(new.len() - tail) {
        tail -= 1;
    }
    (head..old.len() - tail, &new[head..new.len() - tail])
}

// ==== In the window ====

/// What a command is, by the compare's sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Stage,
    Unstage,
    Revert,
    Accept,
}

/// The file a command acts on in a compare of these sides, when it can.
pub fn file_for(act: Act, left: &Source, right: &Source, right_editable: bool) -> Option<String> {
    match (act, left, right) {
        (Act::Stage, Source::Index(p), Source::Live(q) | Source::File(q)) if p == q => Some(p.clone()),
        (Act::Unstage, Source::Head(_), Source::Index(p)) => Some(p.clone()),
        (Act::Revert, _, Source::Live(p)) if right_editable => Some(p.clone()),
        (Act::Accept, Source::Before(p), Source::Live(q) | Source::File(q)) if p == q => Some(p.clone()),
        _ => None,
    }
}

fn can(ui: Ui, act: Act) -> bool {
    super::shown(ui).is_some_and(|e| file_for(act, &e.left.source, &e.right.source, e.right.editable).is_some())
}

/// The lines each selection of an editor covers.
fn selections(ed: &Editor) -> Vec<Lines> {
    let at = |o: usize| ed.offset_to_line_col(o);
    match ed.cursor.get_untracked().mode {
        CursorMode::Insert(sel) => sel.regions().iter().map(|r| lines_of(at(r.start), at(r.end))).collect(),
        CursorMode::Normal(o) => vec![lines_of(at(o), at(o))],
        CursorMode::Visual { start, end, .. } => vec![lines_of(at(start), at(end))],
    }
}

/// Whether both of a compare's sides read from git have come (a side still empty would stage only the lines
/// chosen).
fn git_sides_here(e: &Entry) -> bool {
    [&e.left, &e.right].iter().all(|h| !matches!(h.source, Source::Index(_) | Source::Head(_)) || super::has_come(h))
}

/// A command run on the compare shown.
pub fn act(ui: Ui, send: &Send, what: Act) {
    let Some(e) = super::shown(ui) else { return };
    let Some(file) = file_for(what, &e.left.source, &e.right.source, e.right.editable) else { return };
    let Some(right_editor) = e.editors.borrow().as_ref().map(|(_, r)| r.clone()) else {
        toast::info(ui, "Show the compare side by side to choose its lines.");
        return;
    };
    if !git_sides_here(&e) {
        toast::info(ui, "The file's sides are still being read from git.");
        return;
    }
    let sels = selections(&right_editor);
    let (left, right) = (e.left.doc.text().to_string(), e.right.doc.text().to_string());
    let how = if matches!(what, Act::Stage | Act::Accept) { How::Take } else { How::GiveBack };
    let (send, entry, taken) = (send.clone(), e.clone(), right.clone());
    let done = floem::ext_event::create_ext_action(ui.scope, move |text: Option<String>| {
        let Some(text) = text else {
            toast::info(ui, NONE_TOUCHED);
            return;
        };
        finish(ui, &send, &entry, what, &file, &taken, text);
    });
    std::thread::spawn(move || done(work(&left, &right, &sels, how)));
}

/// A command's new text used: sent to git or the agent's checkpoints, or put in the file's editor.
fn finish(ui: Ui, send: &Send, e: &Rc<Entry>, what: Act, file: &str, taken: &str, text: String) {
    match what {
        Act::Stage => {
            super::set_side(&e.left, &text);
            send(json!({"type": "git_stage_text", "repo": "", "path": file, "text": text, "key": super::GIT_KEY}));
        }
        Act::Unstage => {
            super::set_side(&e.right, &text);
            send(json!({"type": "git_stage_text", "repo": "", "path": file, "text": text, "key": super::GIT_KEY}));
        }
        Act::Accept => {
            send(json!({"type": "changes_accept_text", "path": file, "text": text}));
        }
        Act::Revert => {
            let now = e.right.doc.text().to_string();
            // Typed in meanwhile: the lines chosen may be others now.
            if now != taken {
                toast::info(ui, "The file changed meanwhile; try again.");
                return;
            }
            let (range, with) = one_edit(&now, &text);
            e.right.doc.edit_single(Selection::region(range.start, range.end), with, EditType::Other);
        }
    }
}

/// What it adds to the window's registries, once at start: the four commands and their keys.
pub fn register(_ui: Ui, _send: &Send) {
    for (id, category, title, keys, what) in [
        (STAGE, "Git", "Stage Selected Ranges", &["Ctrl+K Ctrl+Alt+S"][..], Act::Stage),
        (UNSTAGE, "Git", "Unstage Selected Ranges", &["Ctrl+K Ctrl+N", "Ctrl+K Ctrl+Alt+N"][..], Act::Unstage),
        (REVERT, "Git", "Revert Selected Ranges", &["Ctrl+K Ctrl+R"][..], Act::Revert),
        (ACCEPT, "Changes", "Accept the Agent's Selected Changes", &["Ctrl+K Ctrl+Alt+A"][..], Act::Accept),
    ] {
        let mut c = command(id, category, title, move |ui, send| act(ui, send, what)).when(move |ui| can(ui, what));
        for k in keys {
            c = c.keys(k);
        }
        commands::add(c);
    }
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(left: Range<usize>, right: Range<usize>) -> Hunk {
        Hunk { left, right }
    }

    #[test]
    fn a_selection_ending_at_a_lines_start_does_not_take_that_line() {
        assert_eq!(lines_of((2, 3), (2, 3)), (2, 2));
        assert_eq!(lines_of((4, 0), (2, 1)), (2, 3));
        assert_eq!(lines_of((2, 1), (4, 2)), (2, 4));
    }

    #[test]
    fn the_changes_touched_are_cut_down_to_the_lines_touched_as_vs_codes() {
        // Two lines changed line by line: the second alone.
        assert_eq!(touched(&[h(3..5, 3..5)], &[(4, 4)]), vec![h(4..5, 4..5)]);
        // Three lines put in: the middle one.
        assert_eq!(touched(&[h(2..2, 2..5)], &[(3, 3)]), vec![h(2..2, 3..4)]);
        // One line made two: all of the original's with the line touched.
        assert_eq!(touched(&[h(1..2, 1..3)], &[(2, 9)]), vec![h(1..2, 2..3)]);
        // Lines taken away above line 4: touched from line 3 or 4, not 2 or 5.
        let gone = h(4..6, 4..4);
        assert_eq!(touched(&[gone.clone()], &[(3, 3)]), vec![gone.clone()]);
        assert_eq!(touched(&[gone.clone()], &[(4, 4)]), vec![gone.clone()]);
        assert!(touched(&[gone.clone()], &[(2, 2)]).is_empty());
        assert!(touched(&[gone], &[(5, 5)]).is_empty());
        // Nothing touched, and two selections in one hunk.
        assert!(touched(&[h(0..1, 0..1)], &[(3, 4)]).is_empty());
        assert_eq!(touched(&[h(0..4, 0..4)], &[(0, 0), (2, 2)]), vec![h(0..3, 0..3)]);
    }

    #[test]
    fn staging_the_touched_lines_puts_only_them_in_the_index() {
        let index = "a\nb\nc\nd\n";
        let file = "a\nB\nc\nD\ne\n";
        // The caret on B: only B is staged.
        assert_eq!(work(index, file, &[(1, 1)], How::Take).as_deref(), Some("a\nB\nc\nd\n"));
        // All of it: the file.
        assert_eq!(work(index, file, &[(0, 5)], How::Take).as_deref(), Some(file));
        // A line that is the same: nothing.
        assert_eq!(work(index, file, &[(0, 0)], How::Take), None);
        // The last line put in at the end, with no line end.
        assert_eq!(work("a", "a\nb", &[(1, 1)], How::Take).as_deref(), Some("a\nb"));
        // Lines taken away at the start, staged from the first line.
        assert_eq!(work("x\ny\na\n", "a\n", &[(0, 0)], How::Take).as_deref(), Some("a\n"));
    }

    #[test]
    fn unstaging_and_reverting_give_the_original_lines_back() {
        let head = "a\nb\nc\n";
        let index = "a\nB\nc\nd\n";
        assert_eq!(work(head, index, &[(3, 3)], How::GiveBack).as_deref(), Some("a\nB\nc\n"));
        assert_eq!(work(head, index, &[(1, 1)], How::GiveBack).as_deref(), Some("a\nb\nc\nd\n"));
        assert_eq!(work(head, index, &[(0, 4)], How::GiveBack).as_deref(), Some(head));
    }

    #[test]
    fn lines_brought_in_take_the_line_ends_of_the_text_they_go_into() {
        assert_eq!(work("a\nb\n", "a\r\nB\r\n", &[(1, 1)], How::Take).as_deref(), Some("a\nB\n"));
        assert_eq!(work("a\nB\n", "a\r\nb\r\n", &[(1, 1)], How::GiveBack).as_deref(), Some("a\r\nB\r\n"));
    }

    #[test]
    fn one_edit_changes_only_what_differs() {
        assert_eq!(one_edit("abcdef", "abXYef"), (2..4, "XY"));
        assert_eq!(one_edit("aaa", "aaaa"), (3..3, "a"));
        assert_eq!(one_edit("añb", "aéb"), (1..3, "é"));
        assert_eq!(one_edit("same", "same"), (4..4, ""));
    }

    #[test]
    fn each_command_acts_on_the_compares_it_belongs_to() {
        let (index, live, head) =
            (Source::Index("a.rs".into()), Source::Live("a.rs".into()), Source::Head("a.rs".into()));
        assert_eq!(file_for(Act::Stage, &index, &live, true).as_deref(), Some("a.rs"));
        assert_eq!(file_for(Act::Unstage, &head, &index, false).as_deref(), Some("a.rs"));
        assert_eq!(file_for(Act::Stage, &head, &index, false), None);
        assert_eq!(file_for(Act::Revert, &index, &live, true).as_deref(), Some("a.rs"));
        assert_eq!(file_for(Act::Revert, &index, &Source::File("a.rs".into()), false), None);
        let before = Source::Before("a.rs".into());
        assert_eq!(file_for(Act::Accept, &before, &live, true).as_deref(), Some("a.rs"));
        assert_eq!(file_for(Act::Accept, &index, &live, true), None);
    }
}
