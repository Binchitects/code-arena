//! The editor's editing commands, VS Code's: Toggle Line Comment (Ctrl+/) and Block Comment (Shift+Alt+A) with each
//! language's tokens, Move Line Up/Down (Alt+Up/Down), Copy Line Up/Down (Shift+Alt+Up/Down), Delete Line
//! (Ctrl+Shift+K), Insert Line Below/Above (Ctrl+Enter, Ctrl+Shift+Enter), Indent/Outdent (Ctrl+], Ctrl+[, Tab and
//! Shift+Tab on lines), Add Cursor Above/Below (Ctrl+Alt+Up/Down), Add Selection to Next Find Match (Ctrl+D), Select
//! All Occurrences (Ctrl+Shift+L), Add Cursors to Line Ends (Shift+Alt+I), Go to Bracket (Ctrl+Shift+\), Expand and
//! Shrink Selection (Shift+Alt+Right/Left); brackets and quotes closed as they are opened (and typed over, and
//! deleted together), as `editor.autoClosingBrackets` and `editor.autoClosingQuotes` say; Enter keeping the line's
//! indent, and indenting after an opened bracket, as `editor.autoIndent` says. The Edit and Selection menus and the
//! editor's right-click menu are filled with them.
//!
//! What each does to the text is worked out by a plain function over the text and its selections (the first half of
//! this file, tested without a window), which gives the edits and the selections after them; the edits are made in
//! one go, one undo step.
//!
//! The keys: an editor asks `key` before its own keymap, and the workbench asks `editor_only` before it runs a
//! command by a key, so these keys do nothing where no editor has the keys (the chat's box, the Explorer).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use floem::event::Event;
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::Editor;
use floem::views::editor::command::Command as EdCommand;
use floem::views::editor::core::buffer::rope_text::{RopeText, RopeTextRef};
use floem::views::editor::core::command::{EditCommand, MultiSelectionCommand};
use floem::views::editor::core::cursor::CursorMode;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::mode::Mode;
use floem::views::editor::core::selection::{SelRegion, Selection};
use floem::views::editor::core::xi_rope::Rope;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::keypress::press::KeyPress;
use serde_json::Value;

use crate::commands::{self, Chord, command};
use crate::menus::{self, EDIT, Menu, SELECTION, item};
use crate::state::{Send, Ui};

// ---- What a command does to a text, worked out. ----

/// A selection: where it was begun and where its caret is (the same for a caret alone), as offsets in the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sel {
    pub anchor: usize,
    pub head: usize,
}

impl Sel {
    pub fn caret(at: usize) -> Sel {
        Sel { anchor: at, head: at }
    }

    pub fn new(anchor: usize, head: usize) -> Sel {
        Sel { anchor, head }
    }

    pub fn min(self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn max(self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn is_caret(self) -> bool {
        self.anchor == self.head
    }
}

/// What a command does: the parts of the text replaced (from, to, by; in the text as it was, in order, none
/// overlapping) and the selections after (in the text as it is then).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub edits: Vec<(usize, usize, String)>,
    pub sels: Vec<Sel>,
}

#[cfg(test)]
impl Change {
    /// The text after the edits.
    pub fn applied(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (from, to, by) in self.edits.iter().rev() {
            out.replace_range(*from..*to, by);
        }
        out
    }
}

/// A language's comment tokens, VS Code's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tokens {
    pub line: Option<&'static str>,
    pub block: Option<(&'static str, &'static str)>,
}

/// The comment tokens of a file, by its name (VS Code's languages' `comments`); none for plain text.
pub fn tokens_of(path: &str) -> Tokens {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_ascii_lowercase();
    let ext = name.rsplit_once('.').map_or(name.as_str(), |(_, e)| e);
    let c = (Some("//"), Some(("/*", "*/")));
    let (line, block) = match ext {
        "rs" | "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" | "c" | "h" | "cc" | "cpp" | "cxx"
        | "hpp" | "cs" | "java" | "go" | "kt" | "kts" | "swift" | "scala" | "dart" | "php" | "json" | "jsonc"
        | "scss" | "less" | "proto" | "zig" | "groovy" | "gradle" => c,
        "css" => (None, Some(("/*", "*/"))),
        "py" | "pyi" => (Some("#"), Some(("\"\"\"", "\"\"\""))),
        "rb" => (Some("#"), Some(("=begin", "=end"))),
        "ps1" | "psm1" => (Some("#"), Some(("<#", "#>"))),
        "sh" | "bash" | "zsh" | "fish" | "yaml" | "yml" | "toml" | "r" | "pl" | "makefile" | "dockerfile" | "mk"
        | "cmake" | "conf" | "gitignore" | "env" | "tf" => (Some("#"), None),
        "ini" => (Some(";"), None),
        "sql" => (Some("--"), Some(("/*", "*/"))),
        "lua" => (Some("--"), Some(("--[[", "]]"))),
        "hs" => (Some("--"), Some(("{-", "-}"))),
        "html" | "htm" | "xml" | "svg" | "md" | "markdown" | "vue" | "xaml" | "csproj" => (None, Some(("<!--", "-->"))),
        "bat" | "cmd" => (Some("@REM"), None),
        "vb" => (Some("'"), None),
        _ => (None, None),
    };
    Tokens { line, block }
}

/// Which of the line comment commands: VS Code's Toggle, Add (Ctrl+K Ctrl+C) and Remove (Ctrl+K Ctrl+U).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comment {
    Toggle,
    Add,
    Remove,
}

/// The text's line ending (`\r\n` when it has one, else `\n`).
fn ending(t: &RopeTextRef) -> &'static str {
    if t.line_content(0).ends_with("\r\n") { "\r\n" } else { "\n" }
}

/// A line's text, without its ending.
fn line_text(t: &RopeTextRef, line: usize) -> String {
    t.line_content(line).trim_end_matches(['\n', '\r']).to_string()
}

/// Where a line's text ends (before its ending).
fn line_end(t: &RopeTextRef, line: usize) -> usize {
    t.offset_of_line(line) + line_text(t, line).len()
}

fn indent_len(s: &str) -> usize {
    s.len() - s.trim_start_matches([' ', '\t']).len()
}

/// The lines the selections are on, as runs (first, last) in order, runs that touch made one. A selection ending
/// at the start of a line is not on that line (as VS Code has it: whole lines selected are those lines).
fn blocks(t: &RopeTextRef, sels: &[Sel]) -> Vec<(usize, usize)> {
    let mut v: Vec<(usize, usize)> = sels
        .iter()
        .map(|s| {
            let a = t.line_of_offset(s.min());
            let mut b = t.line_of_offset(s.max());
            if b > a && t.offset_of_line(b) == s.max() {
                b -= 1;
            }
            (a, b)
        })
        .collect();
    v.sort();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 + 1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// An offset of the text as it was, in the text after the edits. One where text is put stays before it unless
/// `after`; one in a part replaced goes to its start (or its end, `after`).
fn map(edits: &[(usize, usize, String)], off: usize, after: bool) -> usize {
    let mut shift: isize = 0;
    for (s, e, t) in edits {
        let (s, e) = (*s, *e);
        if off > e || (off == e && (s < e || after)) {
            shift += t.len() as isize - (e - s) as isize;
        } else if off > s {
            return (s as isize + shift) as usize + if after { t.len() } else { 0 };
        } else {
            break;
        }
    }
    (off as isize + shift) as usize
}

/// A selection in the text after the edits: a caret goes after what is put where it is; a selection keeps its
/// start before it and its end after it (so what is put at its edges is in it).
fn map_sel(edits: &[(usize, usize, String)], s: Sel) -> Sel {
    if s.is_caret() {
        return Sel::caret(map(edits, s.head, true));
    }
    let (lo, hi) = (map(edits, s.min(), false), map(edits, s.max(), true));
    if s.anchor <= s.head { Sel::new(lo, hi) } else { Sel::new(hi, lo) }
}

fn mapped(edits: Vec<(usize, usize, String)>, sels: &[Sel]) -> Option<Change> {
    if edits.is_empty() {
        return None;
    }
    let sels = sels.iter().map(|s| map_sel(&edits, *s)).collect();
    Some(Change { edits, sels })
}

/// Edits each with where its caret goes, inside what it puts: the carets after them all.
fn with_carets(parts: Vec<(usize, usize, String, usize)>) -> Change {
    let mut shift: isize = 0;
    let mut change = Change::default();
    for (s, e, t, at) in parts {
        change.sels.push(Sel::caret((s as isize + shift) as usize + at));
        shift += t.len() as isize - (e - s) as isize;
        change.edits.push((s, e, t));
    }
    change
}

/// Toggle (or Add, or Remove) Line Comment: each line of the selections (blank ones left be, unless all are)
/// gets the token at the run's least indent, or has it taken away (with the space after it) when every line has it.
/// A language with no line comment has its lines' text put in a block comment instead.
pub fn line_comment(text: &Rope, sels: &[Sel], tokens: Tokens, how: Comment) -> Option<Change> {
    let t = RopeTextRef::new(text);
    let Some(token) = tokens.line else {
        if how == Comment::Remove {
            return None;
        }
        // The lines' text, as VS Code puts it in a block comment where there is no line comment.
        let whole: Vec<Sel> = blocks(&t, sels)
            .into_iter()
            .map(|(a, b)| {
                let first = line_text(&t, a);
                Sel::new(t.offset_of_line(a) + indent_len(&first), line_end(&t, b))
            })
            .collect();
        let all = sels.iter().all(|s| s.is_caret());
        return block_comment(text, if all { &whole } else { sels }, tokens);
    };
    let runs = blocks(&t, sels);
    let lines_of = |(a, b): (usize, usize)| -> Vec<(usize, String)> {
        let all: Vec<(usize, String)> = (a..=b).map(|l| (l, line_text(&t, l))).collect();
        let full: Vec<(usize, String)> = all.iter().filter(|(_, s)| !s.trim().is_empty()).cloned().collect();
        if full.is_empty() { all } else { full }
    };
    let runs: Vec<Vec<(usize, String)>> = runs.into_iter().map(lines_of).collect();
    let commented = runs.iter().flatten().all(|(_, s)| s.trim_start().starts_with(token));
    let remove = match how {
        Comment::Toggle => commented,
        Comment::Add => false,
        Comment::Remove => true,
    };
    let mut edits = Vec::new();
    for run in &runs {
        let least = run.iter().map(|(_, s)| indent_len(s)).min().unwrap_or(0);
        for (l, s) in run {
            let start = t.offset_of_line(*l);
            if remove {
                let at = indent_len(s);
                if s[at..].starts_with(token) {
                    let space = usize::from(s[at + token.len()..].starts_with(' '));
                    edits.push((start + at, start + at + token.len() + space, String::new()));
                }
            } else {
                edits.push((start + least, start + least, format!("{token} ")));
            }
        }
    }
    mapped(edits, sels)
}

/// Toggle Block Comment: a selection in the tokens has them taken away (with a space inside each), else is put in
/// them; a caret alone gets the two tokens with itself between.
pub fn block_comment(text: &Rope, sels: &[Sel], tokens: Tokens) -> Option<Change> {
    let t = RopeTextRef::new(text);
    let (open, close) = tokens.block?;
    let mut sorted = sels.to_vec();
    sorted.sort_by_key(|s| s.min());
    sorted.dedup_by_key(|s| (s.min(), s.max()));
    let mut edits = Vec::new();
    for s in &sorted {
        let (lo, hi) = (s.min(), s.max());
        if s.is_caret() {
            edits.push((lo, lo, format!("{open}  {close}")));
            continue;
        }
        let inner = t.slice_to_cow(lo..hi);
        let trimmed = inner.trim();
        if trimmed.len() >= open.len() + close.len() && trimmed.starts_with(open) && trimmed.ends_with(close) {
            let from = lo + (inner.len() - inner.trim_start().len());
            let to = hi - (inner.len() - inner.trim_end().len());
            let body = &text.slice_to_cow(from + open.len()..to - close.len());
            let a = usize::from(body.starts_with(' '));
            let b = usize::from(body.len() > a && body.ends_with(' '));
            edits.push((from, from + open.len() + a, String::new()));
            edits.push((to - close.len() - b, to, String::new()));
        } else {
            edits.push((lo, lo, format!("{open} ")));
            edits.push((hi, hi, format!(" {close}")));
        }
    }
    let mut change = Change { sels: Vec::new(), edits };
    for s in sels {
        if s.is_caret() {
            change.sels.push(Sel::caret(map(&change.edits, s.head, false) + open.len() + 1));
        } else {
            let (lo, hi) = (map(&change.edits, s.min(), true), map(&change.edits, s.max(), false));
            change.sels.push(if s.anchor <= s.head { Sel::new(lo, hi) } else { Sel::new(hi, lo) });
        }
    }
    Some(change)
}

/// Move Line Up / Down: the selections' lines go past the line above (or below), the selections with them.
pub fn move_lines(text: &Rope, sels: &[Sel], down: bool) -> Option<Change> {
    let t = RopeTextRef::new(text);
    let runs = blocks(&t, sels);
    let last = t.last_line();
    if runs.is_empty() || (!down && runs[0].0 == 0) || (down && runs.last()?.1 >= last) {
        return None;
    }
    let nl = ending(&t);
    let mut edits = Vec::new();
    let mut shifts = Vec::new();
    for &(a, b) in &runs {
        let moved: Vec<String> = (a..=b).map(|l| line_text(&t, l)).collect();
        let moved = moved.join(nl);
        if down {
            let other = line_text(&t, b + 1);
            edits.push((t.offset_of_line(a), line_end(&t, b + 1), format!("{other}{nl}{moved}")));
            shifts.push(((a, b), (other.len() + nl.len()) as isize));
        } else {
            let other = line_text(&t, a - 1);
            edits.push((t.offset_of_line(a - 1), line_end(&t, b), format!("{moved}{nl}{other}")));
            shifts.push(((a, b), -((other.len() + nl.len()) as isize)));
        }
    }
    let sels = sels
        .iter()
        .map(|s| {
            let line = t.line_of_offset(s.min());
            let by = shifts.iter().find(|((a, b), _)| (*a..=*b).contains(&line)).map_or(0, |(_, by)| *by);
            let at = |o: usize| (o as isize + by) as usize;
            Sel::new(at(s.anchor), at(s.head))
        })
        .collect();
    Some(Change { edits, sels })
}

/// Copy Line Up / Down: the selections' lines put again above (or below) them; the selections go to the copy
/// below (Down) or stay on the one above (Up).
pub fn copy_lines(text: &Rope, sels: &[Sel], down: bool) -> Option<Change> {
    let t = RopeTextRef::new(text);
    let runs = blocks(&t, sels);
    let nl = ending(&t);
    let mut edits = Vec::new();
    // For each run: its lines, how much is put before it, how much it puts.
    let mut put = Vec::new();
    let mut before = 0;
    for &(a, b) in &runs {
        let lines: Vec<String> = (a..=b).map(|l| line_text(&t, l)).collect();
        let copy = lines.join(nl);
        let len = copy.len() + nl.len();
        if down {
            edits.push((line_end(&t, b), line_end(&t, b), format!("{nl}{copy}")));
        } else {
            let at = t.offset_of_line(a);
            edits.push((at, at, format!("{copy}{nl}")));
        }
        put.push(((a, b), before, len));
        before += len;
    }
    let sels = sels
        .iter()
        .map(|s| {
            let line = t.line_of_offset(s.min());
            let (_, before, len) = put.iter().find(|((a, b), _, _)| (*a..=*b).contains(&line)).copied()?;
            let by = before + if down { len } else { 0 };
            Some(Sel::new(s.anchor + by, s.head + by))
        })
        .collect::<Option<Vec<Sel>>>()?;
    Some(Change { edits, sels })
}

/// Delete Line: the selections' lines gone, a caret left on the line after them (the one before, at the end of the
/// text) at the column it was.
pub fn delete_lines(text: &Rope, sels: &[Sel]) -> Option<Change> {
    let t = RopeTextRef::new(text);
    let runs = blocks(&t, sels);
    let last = t.last_line();
    let mut edits = Vec::new();
    let mut sels_after = Vec::new();
    let mut removed = 0;
    for &(a, b) in &runs {
        let col = sels
            .iter()
            .find(|s| (a..=b).contains(&t.line_of_offset(s.head)))
            .map_or(0, |s| s.head - t.offset_of_line(t.line_of_offset(s.head)));
        let (from, to, line_after, start_after) = if b < last {
            let from = t.offset_of_line(a);
            (from, t.offset_of_line(b + 1), b + 1, from)
        } else if a > 0 {
            let from = line_end(&t, a - 1);
            (from, t.len(), a - 1, t.offset_of_line(a - 1))
        } else {
            (0, t.len(), usize::MAX, 0)
        };
        let room = if line_after == usize::MAX { 0 } else { line_text(&t, line_after).len() };
        sels_after.push(Sel::caret(start_after - removed + col.min(room)));
        removed += to - from;
        edits.push((from, to, String::new()));
    }
    (!edits.is_empty()).then_some(Change { edits, sels: sels_after })
}

/// Whether a line asks for the next one to be indented one more: it ends with an opened bracket (or, in Python, a
/// colon).
fn opens(before: &str, colon: bool) -> bool {
    let s = before.trim_end();
    s.ends_with(['{', '[', '(']) || (colon && s.ends_with(':'))
}

/// Insert Line Below (Ctrl+Enter) / Above (Ctrl+Shift+Enter): an empty line under (over) each caret's line, with its
/// indent (one more under a line that opens a bracket), the caret on it.
pub fn insert_line(text: &Rope, sels: &[Sel], below: bool, unit: &str, colon: bool) -> Option<Change> {
    let t = RopeTextRef::new(text);
    let nl = ending(&t);
    let mut lines: Vec<usize> = sels.iter().map(|s| t.line_of_offset(if below { s.max() } else { s.min() })).collect();
    lines.sort();
    lines.dedup();
    let parts = lines
        .into_iter()
        .map(|l| {
            let s = line_text(&t, l);
            let mut indent = s[..indent_len(&s)].to_string();
            if below {
                if opens(&s, colon) {
                    indent.push_str(unit);
                }
                let at = line_end(&t, l);
                let len = nl.len() + indent.len();
                (at, at, format!("{nl}{indent}"), len)
            } else {
                let at = t.offset_of_line(l);
                let len = indent.len();
                (at, at, format!("{indent}{nl}"), len)
            }
        })
        .collect();
    Some(with_carets(parts))
}

/// Indent Line (Ctrl+], Tab on lines): one indent more at the start of each of the selections' lines (empty
/// lines left be, when there are several).
pub fn indent(text: &Rope, sels: &[Sel], unit: &str) -> Option<Change> {
    let t = RopeTextRef::new(text);
    let mut edits = Vec::new();
    for (a, b) in blocks(&t, sels) {
        for l in a..=b {
            if a < b && line_text(&t, l).is_empty() {
                continue;
            }
            let at = t.offset_of_line(l);
            edits.push((at, at, unit.to_string()));
        }
    }
    mapped(edits, sels)
}

/// Outdent Line (Ctrl+[, Shift+Tab): one indent less at the start of each of the selections' lines (a tab, or the
/// spaces back to the tab stop before).
pub fn outdent(text: &Rope, sels: &[Sel], tab_size: usize) -> Option<Change> {
    let t = RopeTextRef::new(text);
    let size = tab_size.max(1);
    let mut edits = Vec::new();
    for (a, b) in blocks(&t, sels) {
        for l in a..=b {
            let s = line_text(&t, l);
            let at = t.offset_of_line(l);
            let n = if s.starts_with('\t') {
                1
            } else {
                let spaces = s.len() - s.trim_start_matches(' ').len();
                let n = if spaces.is_multiple_of(size) { spaces.min(size) } else { spaces % size };
                // A tab after the spaces back to the tab stop.
                if n == 0 && s[spaces..].starts_with('\t') { spaces + 1 } else { n }
            };
            if n > 0 {
                edits.push((at, at + n, String::new()));
            }
        }
    }
    mapped(edits, sels)
}

/// Every place a text is in another (from, to), none overlapping.
fn matches(hay: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    hay.match_indices(needle).map(|(at, m)| (at, at + m.len())).collect()
}

/// The word a caret is in (or at the end of), if any.
fn word_at(t: &RopeTextRef, at: usize) -> Option<(usize, usize)> {
    let (a, b) = t.select_word(at);
    let w = t.slice_to_cow(a..b);
    (b > a && w.chars().all(|c| c.is_alphanumeric() || c == '_')).then_some((a, b))
}

/// Add Selection to Next Find Match (Ctrl+D): carets alone become the words they are in; else the text of the
/// selection added last is found again after it (round past the end), and that place is selected too. The
/// selections and the one added (to be shown).
pub fn next_occurrence(text: &Rope, sels: &[Sel], last: Sel) -> Option<(Vec<Sel>, Sel)> {
    let t = RopeTextRef::new(text);
    if sels.iter().all(|s| s.is_caret()) {
        let words: Vec<Sel> = sels.iter().filter_map(|s| word_at(&t, s.head)).map(|(a, b)| Sel::new(a, b)).collect();
        let shown = *words.last()?;
        return Some((words, shown));
    }
    let needle = t.slice_to_cow(last.min()..last.max()).to_string();
    let hay = t.slice_to_cow(0..t.len());
    let all = matches(&hay, &needle);
    let taken = |m: &(usize, usize)| sels.iter().any(|s| s.min() == m.0 && s.max() == m.1);
    let next = all.iter().find(|m| m.0 >= last.max() && !taken(m)).or_else(|| all.iter().find(|m| !taken(m)))?;
    let added = Sel::new(next.0, next.1);
    let mut out = sels.to_vec();
    out.push(added);
    Some((out, added))
}

/// Select All Occurrences of Find Match (Ctrl+Shift+L): every place the selection's text (or the caret's word) is.
pub fn all_occurrences(text: &Rope, last: Sel) -> Option<Vec<Sel>> {
    let t = RopeTextRef::new(text);
    let (a, b) = if last.is_caret() { word_at(&t, last.head)? } else { (last.min(), last.max()) };
    let needle = t.slice_to_cow(a..b).to_string();
    let hay = t.slice_to_cow(0..t.len());
    let found: Vec<Sel> = matches(&hay, &needle).into_iter().map(|(a, b)| Sel::new(a, b)).collect();
    (!found.is_empty()).then_some(found)
}

const PAIRS: [(u8, u8); 3] = [(b'(', b')'), (b'[', b']'), (b'{', b'}')];

/// The bracket matching the one at `at` (scanning the text's bytes), if any.
fn match_of(b: &[u8], at: usize) -> Option<usize> {
    let c = *b.get(at)?;
    if let Some(&(open, close)) = PAIRS.iter().find(|p| p.0 == c) {
        let mut depth = 0usize;
        for (i, &x) in b.iter().enumerate().skip(at) {
            if x == open {
                depth += 1;
            } else if x == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    } else if let Some(&(open, close)) = PAIRS.iter().find(|p| p.1 == c) {
        let mut depth = 0usize;
        for i in (0..=at).rev() {
            if b[i] == close {
                depth += 1;
            } else if b[i] == open {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// The opened bracket before `at` that is not closed before it, if any.
fn enclosing(b: &[u8], at: usize) -> Option<usize> {
    let mut depth = [0usize; 3];
    for i in (0..at.min(b.len())).rev() {
        for (k, (open, close)) in PAIRS.iter().enumerate() {
            if b[i] == *close {
                depth[k] += 1;
            } else if b[i] == *open {
                if depth[k] == 0 {
                    return Some(i);
                }
                depth[k] -= 1;
            }
        }
    }
    None
}

fn is_bracket(c: u8) -> bool {
    PAIRS.iter().any(|p| p.0 == c || p.1 == c)
}

/// Go to Bracket (Ctrl+Shift+\): from a bracket (the one after the caret, else the one before) to the one matching
/// it; elsewhere to the closing bracket of the brackets the caret is in.
pub fn bracket_jump(text: &str, at: usize) -> Option<usize> {
    let b = text.as_bytes();
    if b.get(at).copied().is_some_and(is_bracket) {
        return match_of(b, at);
    }
    if at > 0 && is_bracket(b[at - 1]) {
        return match_of(b, at - 1);
    }
    match_of(b, enclosing(b, at)?)
}

/// Expand Selection (Shift+Alt+Right): the least of what holds the selection and is more: its word, the inside of
/// the quotes or brackets it is in, those with the quotes or brackets, its line's text, its whole lines, the text.
pub fn expand(rope: &Rope, text: &str, s: Sel) -> Sel {
    let (lo, hi) = (s.min(), s.max());
    let t = RopeTextRef::new(rope);
    let b = text.as_bytes();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    if let Some(w) = word_at(&t, lo) {
        spans.push(w);
    }
    // The quotes on the selection's line.
    let line = t.line_of_offset(lo);
    if line == t.line_of_offset(hi) {
        let (start, s) = (t.offset_of_line(line), line_text(&t, line));
        for q in ['"', '\'', '`'] {
            let at: Vec<usize> = s.match_indices(q).map(|(i, _)| start + i).collect();
            for p in at.chunks(2).filter(|p| p.len() == 2) {
                spans.push((p[0] + 1, p[1]));
                spans.push((p[0], p[1] + 1));
            }
        }
        let indent = indent_len(&s);
        spans.push((start + indent, start + s.len()));
    }
    // The brackets round it, inside and with them, going out.
    let mut from = lo;
    for _ in 0..64 {
        let Some(open) = enclosing(b, from) else { break };
        match match_of(b, open) {
            Some(close) if close >= hi => {
                spans.push((open + 1, close));
                spans.push((open, close + 1));
            }
            _ => {}
        }
        from = open;
    }
    let (first, last) = (t.line_of_offset(lo), t.line_of_offset(hi));
    spans.push((t.offset_of_line(first), t.offset_of_line(last + 1)));
    spans.push((0, text.len()));
    spans
        .into_iter()
        .filter(|&(a, z)| a <= lo && z >= hi && z - a > hi - lo)
        .min_by_key(|&(a, z)| z - a)
        .map_or(s, |(a, z)| Sel::new(a, z))
}

/// What `editor.autoClosingBrackets` and `editor.autoClosingQuotes` say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    Always,
    LanguageDefined,
    BeforeWhitespace,
    Never,
}

impl Close {
    pub fn of(word: &str) -> Close {
        match word {
            "always" => Close::Always,
            "beforeWhitespace" => Close::BeforeWhitespace,
            "never" => Close::Never,
            _ => Close::LanguageDefined,
        }
    }

    /// Whether a pair is closed before this character (none: the text's end).
    fn before(self, next: Option<char>) -> bool {
        match self {
            Close::Always => true,
            Close::Never => false,
            Close::BeforeWhitespace => next.is_none_or(char::is_whitespace),
            // VS Code's `autoCloseBefore`.
            Close::LanguageDefined => next.is_none_or(|c| c.is_whitespace() || ";:.,=}])>".contains(c)),
        }
    }
}

/// The quotes a language closes: its own (Rust's and Markdown's `'` are no quotes; JavaScript's, Go's and the
/// shells' `` ` `` is one).
pub fn quotes_of(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" | "md" | "markdown" | "txt" | "" => "\"",
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" | "go" | "sh" | "bash" | "zsh" => "\"'`",
        _ => "\"'",
    }
}

fn closer(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' | '\'' | '`' => Some(c),
        _ => None,
    }
}

fn char_at(t: &RopeTextRef, at: usize) -> Option<char> {
    t.slice_to_cow(at..t.next_grapheme_offset(at, 1, t.len())).chars().next()
}

fn char_before(t: &RopeTextRef, at: usize) -> Option<char> {
    t.slice_to_cow(t.prev_grapheme_offset(at, 1, 0)..at).chars().next_back()
}

/// A character typed, as VS Code closes pairs: a closer typed before the same closer goes over it; an opened
/// bracket (or quote) is closed where the setting says (a quote not after a word); selections are put in the pair.
/// None: the character is put in as it is.
pub fn typed(text: &Rope, sels: &[Sel], c: char, brackets: Close, quotes: Close, quote_chars: &str) -> Option<Change> {
    let t = RopeTextRef::new(text);
    let quote = quote_chars.contains(c);
    let rule = if quote { quotes } else { brackets };
    // Never: not closed, nor typed over.
    if sels.is_empty() || rule == Close::Never {
        return None;
    }
    let carets = sels.iter().all(|s| s.is_caret());
    // Typed over.
    if carets && (")]}".contains(c) || quote) && sels.iter().all(|s| char_at(&t, s.head) == Some(c)) {
        let sels = sels.iter().map(|s| Sel::caret(s.head + c.len_utf8())).collect();
        return Some(Change { edits: Vec::new(), sels });
    }
    let close = closer(c).filter(|_| quote || "([{".contains(c))?;
    let mut sorted = sels.to_vec();
    sorted.sort_by_key(|s| s.min());
    if !carets {
        // Put round the selections.
        let mut edits = Vec::new();
        for s in &sorted {
            edits.push((s.min(), s.min(), c.to_string()));
            edits.push((s.max(), s.max(), close.to_string()));
        }
        let sels = sels
            .iter()
            .map(|s| {
                let (lo, hi) = (map(&edits, s.min(), true), map(&edits, s.max(), false));
                if s.anchor <= s.head { Sel::new(lo, hi) } else { Sel::new(hi, lo) }
            })
            .collect();
        return Some(Change { edits, sels });
    }
    let closes = sorted.iter().all(|s| {
        let (prev, next) = (char_before(&t, s.head), char_at(&t, s.head));
        rule.before(next) && (!quote || prev.is_none_or(|p| !(p.is_alphanumeric() || p == '_' || p == c)))
    });
    if !closes {
        return None;
    }
    let parts = sorted.iter().map(|s| (s.head, s.head, format!("{c}{close}"), c.len_utf8())).collect();
    Some(with_carets(parts))
}

/// Backspace between a pair (`(|)`, `"|"`): both gone. None: Backspace as it is.
pub fn delete_pair(text: &Rope, sels: &[Sel]) -> Option<Change> {
    let t = RopeTextRef::new(text);
    if sels.is_empty() || !sels.iter().all(|s| s.is_caret()) {
        return None;
    }
    let pair = |s: &Sel| {
        let (p, n) = (char_before(&t, s.head)?, char_at(&t, s.head)?);
        (closer(p) == Some(n)).then_some((s.head - p.len_utf8(), s.head + n.len_utf8()))
    };
    let mut spans: Vec<(usize, usize)> = sels.iter().map(pair).collect::<Option<Vec<_>>>()?;
    spans.sort();
    spans.dedup();
    Some(with_carets(spans.into_iter().map(|(a, z)| (a, z, String::new(), 0)).collect()))
}

/// Enter, as `editor.autoIndent` says: `none` a new line alone; else with the line's indent, one more after an
/// opened bracket, and between a pair (`{|}`) the closer on a line of its own under the caret's.
pub fn newline(text: &Rope, sels: &[Sel], unit: &str, auto: &str, colon: bool) -> Option<Change> {
    let t = RopeTextRef::new(text);
    let nl = ending(&t);
    let mut sorted = sels.to_vec();
    sorted.sort_by_key(|s| s.min());
    sorted.dedup_by_key(|s| (s.min(), s.max()));
    let parts = sorted
        .into_iter()
        .map(|s| {
            let (lo, hi) = (s.min(), s.max());
            if auto == "none" {
                return (lo, hi, nl.to_string(), nl.len());
            }
            let line = t.line_of_offset(lo);
            let start = t.offset_of_line(line);
            let before = t.slice_to_cow(start..lo).to_string();
            let mut indent = before[..indent_len(&before)].to_string();
            let after = line_text(&t, t.line_of_offset(hi));
            let after = &after[(hi - t.offset_of_line(t.line_of_offset(hi))).min(after.len())..];
            if auto != "keep" && opens(&before, colon) {
                let outer = indent.clone();
                indent.push_str(unit);
                let last = before.trim_end().chars().next_back().and_then(closer);
                if last.is_some() && after.trim_start().starts_with(last.unwrap_or(' ')) {
                    let at = nl.len() + indent.len();
                    return (lo, hi, format!("{nl}{indent}{nl}{outer}"), at);
                }
            }
            (lo, hi, format!("{nl}{indent}"), nl.len() + indent.len())
        })
        .collect();
    Some(with_carets(parts))
}

// ---- The window's. ----

/// Expand Selection's steps in an editor (its id): the selections gone from, and those it left.
type Grown = (u64, Vec<Vec<Sel>>, Vec<Sel>);

thread_local! {
    static UI: Cell<Option<Ui>> = const { Cell::new(None) };
    /// The keys of these commands: an editor takes them, and the workbench leaves them be.
    static KEYS: RefCell<HashSet<Vec<Chord>>> = RefCell::new(HashSet::new());
    /// Expand Selection's steps: the editor's id, the selections it went from (to go back to with Shrink), and
    /// those it left (another selection since: the steps are forgotten).
    static GROWN: RefCell<Option<Grown>> = const { RefCell::new(None) };
}

/// The editor of the focused group (or the person's file shown).
fn editor_of(ui: Ui) -> Option<(String, Editor)> {
    ui.live.with_untracked(|l| l.clone())
}

fn sels_of(e: &Editor) -> (Vec<Sel>, Sel) {
    e.cursor.with_untracked(|c| match &c.mode {
        CursorMode::Insert(sel) => {
            let all: Vec<Sel> = sel.regions().iter().map(|r| Sel::new(r.start, r.end)).collect();
            let last = sel.last_inserted().map_or(Sel::caret(c.offset()), |r| Sel::new(r.start, r.end));
            (all, last)
        }
        _ => (vec![Sel::caret(c.offset())], Sel::caret(c.offset())),
    })
}

fn selection(sels: &[Sel]) -> Selection {
    let mut s = Selection::new();
    for x in sels {
        s.add_region(SelRegion::new(x.anchor, x.head, None));
    }
    s
}

/// The selections set (the last given the one added last, as Ctrl+D goes on from it).
fn set_sels(e: &Editor, sels: &[Sel]) {
    let s = selection(sels);
    e.cursor.update(|c| c.set_insert(s));
}

/// A change made in the editor: its edits in one go (one undo step), then its selections.
fn apply(e: &Editor, change: Change, kind: EditType) {
    if !change.edits.is_empty() {
        if e.read_only.get_untracked() {
            return;
        }
        let edits: Vec<(Selection, &str)> =
            change.edits.iter().map(|(a, z, by)| (Selection::region(*a, *z), by.as_str())).collect();
        e.doc().edit(&mut edits.into_iter(), kind);
    }
    set_sels(e, &change.sels);
}

/// A command of Floem's own editor run in it.
fn floem_command(e: &Editor, cmd: EdCommand) {
    e.doc().run_command(e, &cmd, Some(1), Modifiers::empty());
}

/// VS Code's language id of a file, for its settings.
fn lang(ui: Ui, path: &str) -> Option<&'static str> {
    Some(crate::language::now(ui, path))
}

/// One indent: a tab, or `editor.tabSize` spaces (`editor.insertSpaces`).
fn unit(ui: Ui, path: &str) -> (String, usize) {
    let l = lang(ui, path);
    let size = crate::settings::now(ui, "editor.tabSize", l).as_u64().unwrap_or(4).clamp(1, 32) as usize;
    let spaces = crate::settings::now(ui, "editor.insertSpaces", l).as_bool().unwrap_or(true);
    (if spaces { " ".repeat(size) } else { "\t".into() }, size)
}

fn colon(ui: Ui, path: &str) -> bool {
    matches!(lang(ui, path), Some("python"))
}

/// A command's work on the editor: what it changes, worked out from the text and selections.
fn edit_with(ui: Ui, kind: EditType, work: impl FnOnce(&str, &Rope, &[Sel]) -> Option<Change>) {
    let Some((path, e)) = editor_of(ui) else { return };
    let (sels, _) = sels_of(&e);
    let text = e.doc().text();
    if let Some(change) = work(&path, &text, &sels) {
        apply(&e, change, kind);
    }
}

/// The command ids, VS Code's.
pub const COMMENT_LINE: &str = "editor.action.commentLine";
pub const ADD_COMMENT: &str = "editor.action.addCommentLine";
pub const REMOVE_COMMENT: &str = "editor.action.removeCommentLine";
pub const BLOCK_COMMENT: &str = "editor.action.blockComment";
pub const MOVE_UP: &str = "editor.action.moveLinesUpAction";
pub const MOVE_DOWN: &str = "editor.action.moveLinesDownAction";
pub const COPY_UP: &str = "editor.action.copyLinesUpAction";
pub const COPY_DOWN: &str = "editor.action.copyLinesDownAction";
pub const DELETE_LINES: &str = "editor.action.deleteLines";
pub const LINE_BELOW: &str = "editor.action.insertLineAfter";
pub const LINE_ABOVE: &str = "editor.action.insertLineBefore";
pub const INDENT: &str = "editor.action.indentLines";
pub const OUTDENT: &str = "editor.action.outdentLines";
pub const CURSOR_ABOVE: &str = "editor.action.insertCursorAbove";
pub const CURSOR_BELOW: &str = "editor.action.insertCursorBelow";
pub const LINE_ENDS: &str = "editor.action.insertCursorAtEndOfEachLineSelected";
pub const NEXT_MATCH: &str = "editor.action.addSelectionToNextFindMatch";
pub const ALL_MATCHES: &str = "editor.action.selectHighlights";
pub const JUMP_BRACKET: &str = "editor.action.jumpToBracket";
pub const EXPAND: &str = "editor.action.smartSelect.expand";
pub const SHRINK: &str = "editor.action.smartSelect.shrink";
pub const SELECT_ALL: &str = "editor.action.selectAll";
pub const LINE_SELECT: &str = "expandLineSelection";
pub const UNDO: &str = "undo";
pub const REDO: &str = "redo";
pub const CUT: &str = "editor.action.clipboardCutAction";
pub const COPY: &str = "editor.action.clipboardCopyAction";
pub const PASTE: &str = "editor.action.clipboardPasteAction";

fn comment(ui: Ui, how: Comment) {
    edit_with(ui, EditType::ToggleComment, |p, t, s| line_comment(t, s, tokens_of(p), how));
}

fn add_next(ui: Ui) {
    let Some((_, e)) = editor_of(ui) else { return };
    let (sels, last) = sels_of(&e);
    if let Some((mut all, added)) = next_occurrence(&e.doc().text(), &sels, last) {
        // The one added last, last: Ctrl+D goes on from it.
        all.retain(|s| *s != added);
        all.push(added);
        set_sels(&e, &all);
    }
}

fn grow(ui: Ui, out: bool) {
    let Some((_, e)) = editor_of(ui) else { return };
    let (sels, _) = sels_of(&e);
    let id = e.id().to_raw();
    let kept = GROWN.with(|g| g.borrow().clone()).filter(|(i, _, now)| *i == id && *now == sels);
    if out {
        let rope = e.doc().text();
        let text = rope.slice_to_cow(0..rope.len());
        let wider: Vec<Sel> = sels.iter().map(|s| expand(&rope, &text, *s)).collect();
        if wider == sels {
            return;
        }
        let mut steps = kept.map(|(_, s, _)| s).unwrap_or_default();
        steps.push(sels);
        GROWN.with(|g| *g.borrow_mut() = Some((id, steps, wider.clone())));
        set_sels(&e, &wider);
    } else if let Some((_, mut steps, _)) = kept
        && let Some(back) = steps.pop()
    {
        GROWN.with(|g| *g.borrow_mut() = Some((id, steps, back.clone())));
        set_sels(&e, &back);
    }
}

fn jump(ui: Ui) {
    let Some((_, e)) = editor_of(ui) else { return };
    let (sels, _) = sels_of(&e);
    let text = e.doc().text();
    let text = text.slice_to_cow(0..text.len());
    let moved: Vec<Sel> = sels.iter().map(|s| Sel::caret(bracket_jump(&text, s.head).unwrap_or(s.head))).collect();
    set_sels(&e, &moved);
}

fn with_floem(ui: Ui, cmd: EdCommand) {
    if let Some((_, e)) = editor_of(ui) {
        floem_command(&e, cmd);
    }
}

/// Each command: its id, title, keys, and what it does.
fn all_commands() -> Vec<commands::Command> {
    use EditCommand as E;
    use MultiSelectionCommand as M;
    let c = |id: &str, title: &str, run: fn(Ui)| command(id, "", title, move |ui, _| run(ui));
    vec![
        c(COMMENT_LINE, "Toggle Line Comment", |ui| comment(ui, Comment::Toggle)).keys("Ctrl+/"),
        c(ADD_COMMENT, "Add Line Comment", |ui| comment(ui, Comment::Add)).keys("Ctrl+K Ctrl+C"),
        c(REMOVE_COMMENT, "Remove Line Comment", |ui| comment(ui, Comment::Remove)).keys("Ctrl+K Ctrl+U"),
        c(BLOCK_COMMENT, "Toggle Block Comment", |ui| {
            edit_with(ui, EditType::ToggleComment, |p, t, s| block_comment(t, s, tokens_of(p)))
        })
        .keys("Shift+Alt+A"),
        c(MOVE_UP, "Move Line Up", |ui| edit_with(ui, EditType::MoveLine, |_, t, s| move_lines(t, s, false)))
            .keys("Alt+Up"),
        c(MOVE_DOWN, "Move Line Down", |ui| edit_with(ui, EditType::MoveLine, |_, t, s| move_lines(t, s, true)))
            .keys("Alt+Down"),
        c(COPY_UP, "Copy Line Up", |ui| edit_with(ui, EditType::Other, |_, t, s| copy_lines(t, s, false)))
            .keys("Shift+Alt+Up"),
        c(COPY_DOWN, "Copy Line Down", |ui| edit_with(ui, EditType::Other, |_, t, s| copy_lines(t, s, true)))
            .keys("Shift+Alt+Down"),
        c(DELETE_LINES, "Delete Line", |ui| edit_with(ui, EditType::Delete, |_, t, s| delete_lines(t, s)))
            .keys("Ctrl+Shift+K"),
        c(LINE_BELOW, "Insert Line Below", |ui| {
            edit_with(ui, EditType::InsertNewline, |p, t, s| insert_line(t, s, true, &unit(ui, p).0, colon(ui, p)))
        })
        .keys("Ctrl+Enter"),
        c(LINE_ABOVE, "Insert Line Above", |ui| {
            edit_with(ui, EditType::InsertNewline, |p, t, s| insert_line(t, s, false, &unit(ui, p).0, colon(ui, p)))
        })
        .keys("Ctrl+Shift+Enter"),
        c(INDENT, "Indent Line", |ui| edit_with(ui, EditType::Indent, |p, t, s| indent(t, s, &unit(ui, p).0)))
            .keys("Ctrl+]"),
        c(OUTDENT, "Outdent Line", |ui| edit_with(ui, EditType::Outdent, |p, t, s| outdent(t, s, unit(ui, p).1)))
            .keys("Ctrl+["),
        c(CURSOR_ABOVE, "Add Cursor Above", |ui| with_floem(ui, EdCommand::MultiSelection(M::InsertCursorAbove)))
            .keys("Ctrl+Alt+Up"),
        c(CURSOR_BELOW, "Add Cursor Below", |ui| with_floem(ui, EdCommand::MultiSelection(M::InsertCursorBelow)))
            .keys("Ctrl+Alt+Down"),
        c(LINE_ENDS, "Add Cursors to Line Ends", |ui| {
            with_floem(ui, EdCommand::MultiSelection(M::InsertCursorEndOfLine))
        })
        .keys("Shift+Alt+I"),
        c(NEXT_MATCH, "Add Selection to Next Find Match", add_next).keys("Ctrl+D"),
        c(ALL_MATCHES, "Select All Occurrences of Find Match", |ui| {
            let Some((_, e)) = editor_of(ui) else { return };
            let (_, last) = sels_of(&e);
            if let Some(all) = all_occurrences(&e.doc().text(), last) {
                set_sels(&e, &all);
            }
        })
        .keys("Ctrl+Shift+L"),
        c(JUMP_BRACKET, "Go to Bracket", jump).keys("Ctrl+Shift+\\"),
        c(EXPAND, "Expand Selection", |ui| grow(ui, true)).keys("Shift+Alt+Right"),
        c(SHRINK, "Shrink Selection", |ui| grow(ui, false)).keys("Shift+Alt+Left"),
        c(SELECT_ALL, "Select All", |ui| with_floem(ui, EdCommand::MultiSelection(M::SelectAll))).keys("Ctrl+A"),
        c(LINE_SELECT, "Expand Line Selection", |ui| with_floem(ui, EdCommand::MultiSelection(M::SelectCurrentLine)))
            .keys("Ctrl+L"),
        c(UNDO, "Undo", |ui| with_floem(ui, EdCommand::Edit(E::Undo))).keys("Ctrl+Z"),
        c(REDO, "Redo", |ui| with_floem(ui, EdCommand::Edit(E::Redo))).keys("Ctrl+Y").keys("Ctrl+Shift+Z"),
        c(CUT, "Cut", |ui| with_floem(ui, EdCommand::Edit(E::ClipboardCut))).keys("Ctrl+X"),
        c(COPY, "Copy", |ui| with_floem(ui, EdCommand::Edit(E::ClipboardCopy))).keys("Ctrl+C"),
        c(PASTE, "Paste", |ui| with_floem(ui, EdCommand::Edit(E::ClipboardPaste))).keys("Ctrl+V"),
    ]
}

/// What it adds to the window's registries, once at start: the commands, with their keys, and their places in the
/// Edit and Selection menus and the editor's right-click menu.
pub fn register(ui: Ui, _send: &Send) {
    UI.set(Some(ui));
    let has_editor = |ui: Ui| ui.live.with_untracked(|l| l.is_some());
    KEYS.with(|k| {
        let mut k = k.borrow_mut();
        for c in all_commands() {
            for keys in &c.keys {
                if let Some(chords) = commands::parse_keys(keys) {
                    k.insert(chords);
                }
            }
            commands::add(c.when(has_editor));
        }
    });
    let bar = Menu::TitleBar;
    let menu = |m: Menu, id: &str, group: &str, order: i32| menus::add(m, item(id).group(group).order(order));
    // Edit: as VS Code's (Undo and Redo, the clipboard, Find, the comments).
    menus::add(bar(EDIT), item(UNDO).title("Undo").group("1_undo").order(1));
    menus::add(bar(EDIT), item(REDO).title("Redo").group("1_undo").order(2));
    menu(bar(EDIT), CUT, "2_ccp", 1);
    menu(bar(EDIT), COPY, "2_ccp", 2);
    menu(bar(EDIT), PASTE, "2_ccp", 3);
    menu(bar(EDIT), COMMENT_LINE, "5_insert", 1);
    menu(bar(EDIT), BLOCK_COMMENT, "5_insert", 2);
    // Selection: as VS Code's.
    menus::add(bar(SELECTION), item(SELECT_ALL).title("Select All").group("1_basic").order(1));
    menu(bar(SELECTION), EXPAND, "1_basic", 2);
    menu(bar(SELECTION), SHRINK, "1_basic", 3);
    menu(bar(SELECTION), COPY_UP, "2_line", 1);
    menu(bar(SELECTION), COPY_DOWN, "2_line", 2);
    menu(bar(SELECTION), MOVE_UP, "2_line", 3);
    menu(bar(SELECTION), MOVE_DOWN, "2_line", 4);
    menu(bar(SELECTION), CURSOR_ABOVE, "3_multi", 1);
    menu(bar(SELECTION), CURSOR_BELOW, "3_multi", 2);
    menu(bar(SELECTION), LINE_ENDS, "3_multi", 3);
    menus::add(bar(SELECTION), item(NEXT_MATCH).title("Add Next Occurrence").group("3_multi").order(4));
    menus::add(bar(SELECTION), item(ALL_MATCHES).title("Select All Occurrences").group("3_multi").order(5));
    // The editor's right-click: the clipboard, then the palette.
    menu(Menu::EditorContext, CUT, "9_cutcopypaste", 1);
    menu(Menu::EditorContext, COPY, "9_cutcopypaste", 2);
    menu(Menu::EditorContext, PASTE, "9_cutcopypaste", 3);
    menus::add(
        Menu::EditorContext,
        item(crate::workbench::SHOW_COMMANDS).title("Command Palette...").group("z_commands"),
    );
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

/// Whether a key (after the first key of a chord begun, if one is) is one of these commands': the workbench leaves
/// it to the editor that has the keys, and does nothing with it elsewhere.
pub fn editor_only(key: &Key, m: Modifiers) -> bool {
    let Some(chord) = Chord::of(key, m) else { return false };
    let keys = match commands::waiting() {
        Some(first) => vec![first, chord],
        None => vec![chord],
    };
    KEYS.with(|k| k.borrow().contains(&keys))
}

/// A key pressed in an editor, before its own keymap: Tab and Shift+Tab on lines, Enter, Backspace between a pair,
/// and these commands' keys. True when it took the key.
pub fn key(ui: Ui, press: &KeyPress, mods: Modifiers) -> bool {
    let KeyInput::Keyboard(key, _) = &press.key else { return false };
    let plain = !mods.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::META);
    let shift = mods.contains(Modifiers::SHIFT);
    match key {
        Key::Named(NamedKey::Tab) if plain => {
            let Some((path, e)) = editor_of(ui) else { return false };
            let (sels, _) = sels_of(&e);
            let text = e.doc().text();
            let t = RopeTextRef::new(&text);
            let lines = sels.iter().any(|s| t.line_of_offset(s.min()) != t.line_of_offset(s.max()));
            if shift {
                if let Some(c) = outdent(&text, &sels, unit(ui, &path).1) {
                    apply(&e, c, EditType::Outdent);
                }
                return true;
            }
            if lines {
                if let Some(c) = indent(&text, &sels, &unit(ui, &path).0) {
                    apply(&e, c, EditType::Indent);
                }
                return true;
            }
            false
        }
        Key::Named(NamedKey::Enter) if plain && !shift => {
            let Some((path, e)) = editor_of(ui) else { return false };
            if e.read_only.get_untracked() {
                return false;
            }
            let auto =
                crate::settings::now(ui, "editor.autoIndent", lang(ui, &path)).as_str().unwrap_or("full").to_string();
            edit_with(ui, EditType::InsertNewline, |p, t, s| newline(t, s, &unit(ui, p).0, &auto, colon(ui, p)));
            true
        }
        Key::Named(NamedKey::Backspace) if mods.is_empty() => {
            let Some((path, e)) = editor_of(ui) else { return false };
            let l = lang(ui, &path);
            let off = crate::settings::now(ui, "editor.autoClosingBrackets", l).as_str() == Some("never");
            if off || e.read_only.get_untracked() {
                return false;
            }
            let (sels, _) = sels_of(&e);
            match delete_pair(&e.doc().text(), &sels) {
                Some(c) => {
                    apply(&e, c, EditType::Delete);
                    true
                }
                None => false,
            }
        }
        // Taken when a command ran (else Floem's own keymap has it).
        _ if editor_only(key, mods) => {
            let mut ran = false;
            super::tell(ui, |send| ran = commands::press(ui, send, key, mods) != commands::Pressed::Free);
            ran
        }
        _ => false,
    }
}

/// A character typed in a file's editor (`decor::Doc`): pairs closed and typed over. False: it is put in as it is.
pub fn receive_char(ed: &Editor, path: &str, c: &str) -> bool {
    let Some(ui) = UI.get() else { return false };
    let mut chars = c.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else { return false };
    if ed.read_only.get_untracked() || ed.cursor.with_untracked(|c| c.get_mode()) != Mode::Insert {
        return false;
    }
    let l = lang(ui, path);
    let brackets = Close::of(crate::settings::now(ui, "editor.autoClosingBrackets", l).as_str().unwrap_or(""));
    let quotes = Close::of(crate::settings::now(ui, "editor.autoClosingQuotes", l).as_str().unwrap_or(""));
    let (sels, _) = sels_of(ed);
    match typed(&ed.doc().text(), &sels, ch, brackets, quotes, quotes_of(path)) {
        Some(change) => {
            apply(ed, change, EditType::InsertChars);
            true
        }
        None => false,
    }
}

/// The editor's right-click menu, where it was clicked (a press of the pointer's other button).
pub fn context_menu(ui: Ui, view: floem::ViewId, e: &Event) {
    if !matches!(e, Event::PointerDown(p) if p.button.is_secondary()) {
        return;
    }
    // The keys go back to the editor's text once the menu is closed.
    let back = editor_of(ui).and_then(|(_, ed)| ed.editor_view_id.get_untracked()).unwrap_or(view);
    super::tell(ui, |send| {
        crate::widgets::popup::open_menu(
            ui,
            send,
            crate::widgets::popup::At::pointer(view, e).back(back),
            Menu::EditorContext,
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rope {
        Rope::from(s)
    }

    const RUST: Tokens = Tokens { line: Some("//"), block: Some(("/*", "*/")) };

    fn run(text: &str, change: Option<Change>) -> (String, Vec<Sel>) {
        let c = change.expect("a change");
        (c.applied(text), c.sels)
    }

    #[test]
    fn a_line_comment_goes_on_at_the_least_indent_and_comes_off_again() {
        let text = "fn a() {\n    let x = 1;\n\n    x\n}\n";
        // The two lines of the body, selected whole (to the start of the line after).
        let sels = [Sel::new(9, 31)];
        let (on, s) = run(text, line_comment(&r(text), &sels, RUST, Comment::Toggle));
        assert_eq!(on, "fn a() {\n    // let x = 1;\n\n    // x\n}\n");
        assert_eq!(s, vec![Sel::new(9, 37)]);
        let (off, _) = run(&on, line_comment(&r(&on), &s, RUST, Comment::Toggle));
        assert_eq!(off, text);
        // A caret alone: its line, the caret after the token.
        let (one, s) = run("x\n", line_comment(&r("x\n"), &[Sel::caret(0)], RUST, Comment::Toggle));
        assert_eq!((one.as_str(), s), ("// x\n", vec![Sel::caret(3)]));
        // Some lines commented, some not: Toggle comments them all; Remove takes off what there is.
        let mixed = "// a\nb\n";
        let all = [Sel::new(0, 7)];
        assert_eq!(run(mixed, line_comment(&r(mixed), &all, RUST, Comment::Toggle)).0, "// // a\n// b\n");
        assert_eq!(run(mixed, line_comment(&r(mixed), &all, RUST, Comment::Remove)).0, "a\nb\n");
        // Python's token; HTML's lines in a block comment.
        let py = Tokens { line: Some("#"), block: None };
        assert_eq!(run("x = 1", line_comment(&r("x = 1"), &[Sel::caret(2)], py, Comment::Toggle)).0, "# x = 1");
        let html = tokens_of("a.html");
        assert_eq!(
            run("  <b/>", line_comment(&r("  <b/>"), &[Sel::caret(3)], html, Comment::Toggle)).0,
            "  <!-- <b/> -->"
        );
    }

    #[test]
    fn a_block_comment_wraps_the_selection_and_unwraps_it() {
        let text = "let x = 1;";
        let (on, s) = run(text, block_comment(&r(text), &[Sel::new(8, 9)], RUST));
        assert_eq!((on.as_str(), s.clone()), ("let x = /* 1 */;", vec![Sel::new(11, 12)]));
        let all = [Sel::new(8, 15)];
        assert_eq!(run(&on, block_comment(&r(&on), &all, RUST)).0, text);
        let (caret, s) = run("ab", block_comment(&r("ab"), &[Sel::caret(1)], RUST));
        assert_eq!((caret.as_str(), s), ("a/*  */b", vec![Sel::caret(4)]));
        assert!(block_comment(&r("x"), &[Sel::caret(0)], tokens_of("a.sh")).is_none());
    }

    #[test]
    fn lines_move_and_are_copied_and_deleted_with_their_carets() {
        let text = "a\nbb\nccc";
        let (up, s) = run(text, move_lines(&r(text), &[Sel::caret(3)], false));
        assert_eq!((up.as_str(), s), ("bb\na\nccc", vec![Sel::caret(1)]));
        let (down, s) = run(text, move_lines(&r(text), &[Sel::caret(3)], true));
        assert_eq!((down.as_str(), s), ("a\nccc\nbb", vec![Sel::caret(7)]));
        assert!(move_lines(&r(text), &[Sel::caret(0)], false).is_none());
        assert!(move_lines(&r(text), &[Sel::caret(6)], true).is_none());
        // Two lines selected move together.
        assert_eq!(run(text, move_lines(&r(text), &[Sel::new(0, 3)], true)).0, "ccc\na\nbb");
        let (copy, s) = run(text, copy_lines(&r(text), &[Sel::caret(3)], true));
        assert_eq!((copy.as_str(), s), ("a\nbb\nbb\nccc", vec![Sel::caret(6)]));
        let (copy, s) = run(text, copy_lines(&r(text), &[Sel::caret(3)], false));
        assert_eq!((copy.as_str(), s), ("a\nbb\nbb\nccc", vec![Sel::caret(3)]));
        let (gone, s) = run(text, delete_lines(&r(text), &[Sel::caret(3)]));
        assert_eq!((gone.as_str(), s), ("a\nccc", vec![Sel::caret(3)]));
        let (gone, s) = run(text, delete_lines(&r(text), &[Sel::caret(8)]));
        assert_eq!((gone.as_str(), s), ("a\nbb", vec![Sel::caret(4)]));
        // A file with Windows' line ends keeps them.
        let crlf = "a\r\nb";
        assert_eq!(run(crlf, move_lines(&r(crlf), &[Sel::caret(0)], true)).0, "b\r\na");
    }

    #[test]
    fn lines_are_inserted_indented_and_indented_and_outdented() {
        let text = "  if x {\n  }";
        let (below, s) = run(text, insert_line(&r(text), &[Sel::caret(3)], true, "    ", false));
        assert_eq!((below.as_str(), s), ("  if x {\n      \n  }", vec![Sel::caret(15)]));
        let (above, s) = run(text, insert_line(&r(text), &[Sel::caret(3)], false, "    ", false));
        assert_eq!((above.as_str(), s), ("  \n  if x {\n  }", vec![Sel::caret(2)]));
        let lines = "a\n\nb\n";
        let (more, s) = run(lines, indent(&r(lines), &[Sel::new(0, 4)], "\t"));
        assert_eq!((more.as_str(), s.clone()), ("\ta\n\n\tb\n", vec![Sel::new(0, 6)]));
        assert_eq!(run(&more, outdent(&r(&more), &s, 4)).0, lines);
        // Spaces back to the tab stop before.
        assert_eq!(run("      x", outdent(&r("      x"), &[Sel::caret(7)], 4)).0, "    x");
        assert!(outdent(&r("x"), &[Sel::caret(0)], 4).is_none());
    }

    #[test]
    fn next_and_all_occurrences_are_selected() {
        let text = "foo bar foo baz foo";
        let (words, shown) = next_occurrence(&r(text), &[Sel::caret(1)], Sel::caret(1)).expect("the word");
        assert_eq!((words.clone(), shown), (vec![Sel::new(0, 3)], Sel::new(0, 3)));
        let (two, added) = next_occurrence(&r(text), &words, shown).expect("the next");
        assert_eq!(added, Sel::new(8, 11));
        let (three, added) = next_occurrence(&r(text), &two, added).expect("the next");
        assert_eq!(added, Sel::new(16, 19));
        // Round past the end, every one taken: none more.
        assert!(next_occurrence(&r(text), &three, added).is_none());
        let from_last = next_occurrence(&r(text), &[Sel::new(16, 19)], Sel::new(16, 19)).expect("round");
        assert_eq!(from_last.1, Sel::new(0, 3));
        let all = all_occurrences(&r(text), Sel::caret(17)).expect("all");
        assert_eq!(all, vec![Sel::new(0, 3), Sel::new(8, 11), Sel::new(16, 19)]);
        assert!(all_occurrences(&r("  "), Sel::caret(1)).is_none());
    }

    #[test]
    fn brackets_are_gone_to_and_selections_grow() {
        let text = "f(a, [b]) { x }";
        assert_eq!(bracket_jump(text, 1), Some(8));
        assert_eq!(bracket_jump(text, 8), Some(1));
        assert_eq!(bracket_jump(text, 2), Some(8));
        assert_eq!(bracket_jump(text, 6), Some(7));
        assert_eq!(bracket_jump(text, 13), Some(14));
        assert_eq!(bracket_jump("abc", 1), None);
        let s = expand(&r(text), text, Sel::caret(6));
        assert_eq!(s, Sel::new(6, 7));
        let s = expand(&r(text), text, s);
        assert_eq!(s, Sel::new(5, 8));
        let s = expand(&r(text), text, s);
        assert_eq!(s, Sel::new(2, 8));
        let s = expand(&r(text), text, s);
        assert_eq!(s, Sel::new(1, 9));
        assert_eq!(expand(&r(text), text, s), Sel::new(0, text.len()));
        assert_eq!(expand(&r("say \"hi there\""), "say \"hi there\"", Sel::caret(6)), Sel::new(5, 7));
        assert_eq!(expand(&r("say \"hi there\""), "say \"hi there\"", Sel::new(5, 7)), Sel::new(5, 13));
    }

    #[test]
    fn pairs_are_closed_typed_over_and_deleted_together() {
        let lang = Close::LanguageDefined;
        let (t, s) = run("", typed(&r(""), &[Sel::caret(0)], '(', lang, lang, "\"'"));
        assert_eq!((t.as_str(), s.clone()), ("()", vec![Sel::caret(1)]));
        let over = typed(&r(&t), &s, ')', lang, lang, "\"'").expect("over");
        assert!(over.edits.is_empty() && over.sels == vec![Sel::caret(2)]);
        // Not before a word; a quote not after one.
        assert!(typed(&r("x"), &[Sel::caret(0)], '(', lang, lang, "\"'").is_none());
        assert!(typed(&r("it"), &[Sel::caret(2)], '\'', lang, lang, "\"'").is_none());
        assert!(typed(&r("x"), &[Sel::caret(0)], '(', Close::Never, lang, "\"'").is_none());
        assert_eq!(run("x", typed(&r("x"), &[Sel::caret(0)], '(', Close::Always, lang, "\"'")).0, "()x");
        // Rust's ' is no quote.
        assert!(typed(&r(""), &[Sel::caret(0)], '\'', lang, lang, quotes_of("a.rs")).is_none());
        // A selection put in the pair.
        let (t, s) = run("ab", typed(&r("ab"), &[Sel::new(0, 2)], '"', lang, lang, "\"'"));
        assert_eq!((t.as_str(), s), ("\"ab\"", vec![Sel::new(1, 3)]));
        let (t, s) = run("()", delete_pair(&r("()"), &[Sel::caret(1)]));
        assert_eq!((t.as_str(), s), ("", vec![Sel::caret(0)]));
        assert!(delete_pair(&r("(x"), &[Sel::caret(1)]).is_none());
    }

    #[test]
    fn enter_keeps_the_indent_and_opens_a_pair() {
        let text = "    a {}";
        let (t, s) = run(text, newline(&r(text), &[Sel::caret(7)], "    ", "full", false));
        assert_eq!((t.as_str(), s), ("    a {\n        \n    }", vec![Sel::caret(16)]));
        let (t, s) = run("  x", newline(&r("  x"), &[Sel::caret(3)], "  ", "full", false));
        assert_eq!((t.as_str(), s), ("  x\n  ", vec![Sel::caret(6)]));
        assert_eq!(run("  x", newline(&r("  x"), &[Sel::caret(3)], "  ", "none", false)).0, "  x\n");
        assert_eq!(run("if a:", newline(&r("if a:"), &[Sel::caret(5)], "    ", "full", true)).0, "if a:\n    ");
    }

    #[test]
    fn the_comment_tokens_are_the_languages() {
        assert_eq!(tokens_of("src/a.rs"), RUST);
        assert_eq!(tokens_of("a.py").line, Some("#"));
        assert_eq!(tokens_of("Dockerfile").line, Some("#"));
        assert_eq!(tokens_of("a.css"), Tokens { line: None, block: Some(("/*", "*/")) });
        assert_eq!(tokens_of("notes.txt"), Tokens { line: None, block: None });
    }

    #[test]
    fn a_character_of_several_bytes_is_never_cut() {
        let lang = Close::LanguageDefined;
        assert!(delete_pair(&r("éabc"), &[Sel::caret(2)]).is_none());
        assert!(delete_pair(&r("é"), &[Sel::caret(2)]).is_none());
        assert!(typed(&r("aéé"), &[Sel::caret(0)], '(', lang, lang, "\"").is_none());
        assert_eq!(run("é", typed(&r("é"), &[Sel::caret(2)], '(', lang, lang, "\"")).0, "é()");
        let long = "é".repeat(3000);
        assert!(newline(&r(&long), &[Sel::caret(long.len())], "  ", "full", false).is_some());
        assert!(typed(&r("é)"), &[Sel::caret(2)], ')', Close::Never, lang, "\"").is_none());
    }

    #[test]
    fn the_keys_parse_as_bindings() {
        for keys in ["Ctrl+/", "Ctrl+]", "Ctrl+[", "Ctrl+Shift+\\", "Shift+Alt+A", "Ctrl+K Ctrl+C"] {
            assert!(commands::parse_keys(keys).is_some(), "{keys}");
        }
        let slash = Chord::of(&Key::Character("/".into()), Modifiers::CONTROL).expect("a chord");
        assert_eq!(commands::parse_keys("Ctrl+/"), Some(vec![slash]));
        let bar = Chord::of(&Key::Character("|".into()), Modifiers::CONTROL | Modifiers::SHIFT).expect("a chord");
        assert_eq!(commands::parse_keys("Ctrl+Shift+\\"), Some(vec![bar]));
    }
}
