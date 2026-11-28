//! Folding, as VS Code's (`editor.folding`): a region is a line and the lines under it that are indented further
//! (VS Code's indentation strategy; in Python, YAML and the other off-side languages without the empty lines after
//! it), or the lines from a `#region` marker to its `#endregion`. A folded region shows its first line only, with
//! VS Code's `⋯` after it and its background (`editor.foldBackground`); its chevron is in the gutter, right of the
//! line numbers (a folded one always, the others while the pointer is over the gutter, VS Code's "mouseover"), and a
//! click on it folds or unfolds. The keys are VS Code's: Fold (Ctrl+Shift+[), Unfold (Ctrl+Shift+]), Toggle Fold
//! (Ctrl+K Ctrl+L), Fold All (Ctrl+K Ctrl+0), Unfold All (Ctrl+K Ctrl+J), Fold and Unfold Recursively (Ctrl+K
//! Ctrl+[ and Ctrl+K Ctrl+]), Fold Level 1 to 7 (Ctrl+K Ctrl+1…7).
//!
//! Floem's editor has no folding: its lines are one under the other, a line at its index times the line height.
//! The file's editor is therefore drawn here (`view`): the lines on screen are worked out from the fold map (the
//! document's `compute_screen_lines`, which `decor::Doc` hands to `screen_lines`), and so are the height scrolled
//! over and where the caret is scrolled to. The caret never rests in a folded line: Up and Down go over the fold,
//! and a caret put there another way (a search's hit, Go to Line) unfolds it. An edit moves the folds after it and
//! unfolds the one it is in. The folds are kept per file while the window is open, for every editor of it.
//!
//! Other parts that place lines by height (the minimap, sticky scroll) read `map(path)` (`row_of_line`,
//! `line_of_row`). Folds apply while the editor does not wrap lines (it does not: as VS Code's default).
//!
//! What every part has (`register`, `take`) is said in `ext.rs`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use floem::context::PaintCx;
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::{Point, Rect, Vec2};
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::CursorStyle;
use floem::text::{Attrs, AttrsList, FamilyOwned, LineHeightValue, TextLayout};
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::buffer::rope_text::{RopeText, RopeTextVal};
use floem::views::editor::core::cursor::CursorMode;
use floem::views::editor::core::xi_rope::Rope;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::keypress::press::KeyPress;
use floem::views::editor::text::{Document, WrapMethod};
use floem::views::editor::text_document::TextDocument;
use floem::views::editor::view::{
    LineInfo, LineRegion, ScreenLines, ScreenLinesBase, cursor_caret, editor_gutter, editor_view,
};
use floem::views::editor::visual_line::RVLine;
use floem::views::{Decorators, container, scroll, stack};
use floem::{IntoView, View, ViewId};
use floem_renderer::Renderer;
use serde_json::Value;

use crate::commands::{self, command};
use crate::decor::{Mark, Phantom, Tint};
use crate::state::{Send, Ui};

// ==== Regions ====

/// A line's indentation in columns, a tab to the next tab stop; none for a line that is empty or only spaces (VS
/// Code's `computeIndentLevel`).
pub fn indent_of(line: &str, tab: usize) -> Option<usize> {
    let tab = tab.max(1);
    let mut at = 0;
    for c in line.chars() {
        match c {
            ' ' => at += 1,
            '\t' => at += tab - at % tab,
            '\r' | '\n' => return None,
            _ => return Some(at),
        }
    }
    None
}

/// Whether a line is a region marker, after its comment's opener (`//`, `#`, `/*`, `<!--`, `--`, `'`, `;`):
/// `#region` (or `region`, `#pragma region`) starts one, `#endregion` ends one.
pub fn marker(line: &str) -> Option<bool> {
    let mut t = line.trim_start();
    for opener in ["<!--", "//", "/*", "--", "#pragma", "#", "'", ";"] {
        if let Some(rest) = t.strip_prefix(opener) {
            t = rest.trim_start();
            break;
        }
    }
    let t = t.strip_prefix('#').unwrap_or(t);
    let word = |w: &str| t.strip_prefix(w).is_some_and(|r| !r.starts_with(|c: char| c.is_alphanumeric() || c == '_'));
    if word("endregion") {
        Some(false)
    } else if word("region") {
        Some(true)
    } else {
        None
    }
}

/// Whether a language folds as VS Code's off-side ones do (its blocks by indentation alone): the empty lines after
/// a region are not in it.
pub fn off_side(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "py" | "pyi"
            | "pyw"
            | "yaml"
            | "yml"
            | "coffee"
            | "pug"
            | "jade"
            | "haml"
            | "nim"
            | "fs"
            | "fsx"
            | "fsi"
            | "sass"
            | "styl"
            | "slim"
    )
}

/// A text's regions, (first line, last line) from 0, sorted by first line, an outer one before those in it: VS
/// Code's indentation regions (`computeRanges`, read from the last line up) and its marker regions, a marker's
/// taking the place of an indentation region on the same line.
pub fn regions(lines: &[&str], tab: usize, off_side: bool) -> Vec<(usize, usize)> {
    struct Open {
        indent: isize,
        end_above: usize,
    }
    let n = lines.len();
    let mut found = Vec::new();
    let mut open = vec![Open { indent: -1, end_above: n }];
    for line in (0..n).rev() {
        let Some(indent) = indent_of(lines[line], tab) else {
            if off_side {
                // The empty lines go with the block after them: they end the one before above them.
                open.last_mut().expect("the bottom stays").end_above = line;
            }
            continue;
        };
        let indent = indent as isize;
        if open.last().expect("the bottom stays").indent > indent {
            while open.last().expect("the bottom stays").indent > indent {
                open.pop();
            }
            let end = open.last().expect("the bottom stays").end_above.saturating_sub(1);
            if end > line {
                found.push((line, end));
            }
        }
        let last = open.last_mut().expect("the bottom stays");
        if last.indent == indent {
            last.end_above = line;
        } else {
            open.push(Open { indent, end_above: line });
        }
    }
    let mut starts = Vec::new();
    let mut marked = HashMap::new();
    for (i, l) in lines.iter().enumerate() {
        match marker(l) {
            Some(true) => starts.push(i),
            Some(false) => {
                if let Some(s) = starts.pop()
                    && i > s
                {
                    marked.insert(s, i);
                }
            }
            None => {}
        }
    }
    found.retain(|(s, _)| !marked.contains_key(s));
    found.extend(marked);
    found.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    found
}

/// How deep each region is (1: in none other), for the regions as `regions` sorts them.
pub fn levels(regions: &[(usize, usize)]) -> Vec<usize> {
    let mut open: Vec<usize> = Vec::new();
    regions
        .iter()
        .map(|&(s, e)| {
            while open.last().is_some_and(|&end| end < s) {
                open.pop();
            }
            open.push(e);
            open.len()
        })
        .collect()
}

/// The regions a line is in (its first line counts), the innermost first.
fn around(regions: &[(usize, usize)], line: usize) -> Vec<(usize, usize)> {
    let mut found: Vec<_> = regions.iter().copied().filter(|&(s, e)| s <= line && line <= e).collect();
    found.reverse();
    found
}

// ==== What a command changes ====

/// Fold: the innermost region at a line that is not folded yet.
pub fn fold(folds: &mut Vec<(usize, usize)>, regions: &[(usize, usize)], line: usize) {
    if let Some(r) = around(regions, line).into_iter().find(|r| !folds.iter().any(|f| f.0 == r.0)) {
        folds.push(r);
    }
}

/// Unfold: the folded region at a line, the innermost.
pub fn unfold(folds: &mut Vec<(usize, usize)>, line: usize) {
    let inner = folds.iter().copied().filter(|&(s, e)| s <= line && line <= e).max_by_key(|f| f.0);
    if let Some(inner) = inner {
        folds.retain(|f| *f != inner);
    }
}

/// Toggle Fold: the region starting at the line unfolded when it is folded; else Fold.
pub fn toggle(folds: &mut Vec<(usize, usize)>, regions: &[(usize, usize)], line: usize) {
    if folds.iter().any(|f| f.0 == line) {
        folds.retain(|f| f.0 != line);
    } else {
        fold(folds, regions, line);
    }
}

/// Fold or unfold Recursively: the innermost region at a line not yet so, with every region in it.
pub fn recursively(folds: &mut Vec<(usize, usize)>, regions: &[(usize, usize)], line: usize, folded: bool) {
    let Some(&(s, e)) = around(regions, line).first() else { return };
    folds.retain(|f| f.0 < s || f.0 > e);
    if folded {
        folds.extend(regions.iter().copied().filter(|&(a, b)| a >= s && b <= e));
    }
}

/// Fold Level N: every region that deep, but those the caret is in.
pub fn fold_level(folds: &mut Vec<(usize, usize)>, regions: &[(usize, usize)], level: usize, caret: usize) {
    for (r, l) in regions.iter().zip(levels(regions)) {
        if l == level && !(r.0 <= caret && caret <= r.1) && !folds.iter().any(|f| f.0 == r.0) {
            folds.push(*r);
        }
    }
}

/// The folds after an edit that put lines `from..=to_new` where `from..=to_old` were: those after it moved by the
/// lines it added or took away, those it touched unfolded (an edit on a folded region's first line that adds no
/// line keeps it).
pub fn shift(folds: &mut Vec<(usize, usize)>, from: usize, to_old: usize, to_new: usize) {
    let by = to_new as isize - to_old as isize;
    folds.retain_mut(|f| {
        if f.1 < from {
            true
        } else if f.0 > to_old {
            f.0 = (f.0 as isize + by) as usize;
            f.1 = (f.1 as isize + by) as usize;
            true
        } else {
            f.0 == from && to_old == from && by == 0
        }
    });
}

// ==== The fold map ====

/// Which lines the folds hide, and where each line that is not hidden is drawn (its row, from 0); with the empty
/// rows (fillers) another part puts above some lines (the compare editor, for the lines of its two sides to stand
/// beside each other).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoldMap {
    /// Each run of hidden lines (first, last), in order and apart, with how many lines are hidden before it.
    hidden: Vec<(usize, usize, usize)>,
    /// Each run of empty rows: the line it is above, how many, the row it begins on, and how many empty rows are
    /// above it; in the order of their lines.
    fillers: Vec<(usize, usize, usize, usize)>,
}

impl FoldMap {
    /// The map of folds, (first line, last line) of each region: its lines after the first are hidden.
    pub fn new(folds: &[(usize, usize)]) -> FoldMap {
        let mut runs: Vec<(usize, usize)> = folds.iter().filter(|f| f.1 > f.0).map(|f| (f.0 + 1, f.1)).collect();
        runs.sort();
        let mut hidden: Vec<(usize, usize, usize)> = Vec::new();
        let mut before = 0;
        for (a, b) in runs {
            match hidden.last_mut() {
                Some(last) if a <= last.1 + 1 => {
                    if b > last.1 {
                        before += b - last.1;
                        last.1 = b;
                    }
                }
                _ => {
                    hidden.push((a, b, before));
                    before += b - a + 1;
                }
            }
        }
        FoldMap { hidden, fillers: Vec::new() }
    }

    /// The map with empty rows above lines: (the line, how many rows); the line may be the line count (under the
    /// last). A hidden line's rows are above the line its fold starts on.
    pub fn with_fillers(mut self, fillers: &[(usize, usize)]) -> FoldMap {
        let mut runs: Vec<(usize, usize)> = fillers.iter().copied().filter(|f| f.1 > 0).collect();
        runs.sort();
        runs.dedup_by(|b, a| {
            let same = a.0 == b.0;
            if same {
                a.1 += b.1;
            }
            same
        });
        self.fillers.clear();
        let mut before = 0;
        for (line, count) in runs {
            let row = self.folded_row(line) + before;
            self.fillers.push((line, count, row, before));
            before += count;
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.hidden.is_empty() && self.fillers.is_empty()
    }

    /// How many empty rows there are.
    pub fn filled(&self) -> usize {
        self.fillers.last().map_or(0, |f| f.1 + f.3)
    }

    /// The empty rows, each its first row and how many.
    pub fn filler_rows(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.fillers.iter().map(|f| (f.2, f.1))
    }

    /// How many lines are hidden.
    pub fn count(&self) -> usize {
        self.hidden.last().map_or(0, |h| h.2 + h.1 - h.0 + 1)
    }

    /// The run of hidden lines a line is in.
    pub fn run_of(&self, line: usize) -> Option<(usize, usize)> {
        let i = self.hidden.partition_point(|h| h.0 <= line);
        self.hidden[..i].last().filter(|h| line <= h.1).map(|h| (h.0, h.1))
    }

    pub fn is_hidden(&self, line: usize) -> bool {
        self.run_of(line).is_some()
    }

    /// The row a line is drawn on; a hidden line's is that of the line its fold starts on.
    pub fn row_of_line(&self, line: usize) -> usize {
        let i = self.fillers.partition_point(|f| f.0 <= line);
        self.folded_row(line) + self.fillers[..i].last().map_or(0, |f| f.1 + f.3)
    }

    /// The row a line is drawn on by the folds alone.
    fn folded_row(&self, line: usize) -> usize {
        let i = self.hidden.partition_point(|h| h.0 <= line);
        match self.hidden[..i].last() {
            None => line,
            Some(h) if line <= h.1 => h.0 - 1 - h.2,
            Some(h) => line - (h.2 + h.1 - h.0 + 1),
        }
    }

    /// The line drawn on a row; on an empty row, the line under it.
    pub fn line_of_row(&self, row: usize) -> usize {
        let i = self.fillers.partition_point(|f| f.2 <= row);
        match self.fillers[..i].last() {
            Some(f) if row < f.2 + f.1 => f.0,
            Some(f) => self.folded_line(row - f.1 - f.3),
            None => self.folded_line(row),
        }
    }

    /// The line drawn on a row; none on an empty row.
    pub fn line_at_row(&self, row: usize) -> Option<usize> {
        let i = self.fillers.partition_point(|f| f.2 <= row);
        match self.fillers[..i].last() {
            Some(f) if row < f.2 + f.1 => None,
            _ => Some(self.line_of_row(row)),
        }
    }

    /// The line drawn on a row by the folds alone.
    fn folded_line(&self, row: usize) -> usize {
        let i = self.hidden.partition_point(|h| h.0 - h.2 <= row);
        row + self.hidden[..i].last().map_or(0, |h| h.2 + h.1 - h.0 + 1)
    }

    /// How many rows a text of so many lines is drawn on.
    pub fn rows(&self, lines: usize) -> usize {
        lines.saturating_sub(self.count()) + self.filled()
    }
}

// ==== The folds of each file ====

struct File {
    folds: Vec<(usize, usize)>,
    /// The empty rows another part puts above lines (`set_fillers`).
    fillers: Vec<(usize, usize)>,
    map: Rc<FoldMap>,
    /// Set when the folds change: the editors of the file draw their lines again.
    rev: RwSignal<u64>,
    /// The text as the folds know it, to tell which lines an edit changed.
    text: Rope,
    /// The document whose edits move the folds (its address).
    doc: usize,
    /// The regions, with the document's `cache_rev` they were found at.
    regions: Option<(u64, Regions)>,
}

/// A file's regions, each its first and last line (from 0).
type Regions = Rc<Vec<(usize, usize)>>;

thread_local! {
    static FILES: RefCell<HashMap<String, File>> = RefCell::new(HashMap::new());
}

/// A file's fold map (none: nothing folded).
pub fn map(path: &str) -> Option<Rc<FoldMap>> {
    FILES.with(|f| f.borrow().get(path).map(|f| f.map.clone()).filter(|m| !m.is_empty()))
}

/// A file's folds, (first line, last line) of each folded region.
pub fn folds(path: &str) -> Vec<(usize, usize)> {
    FILES.with(|f| f.borrow().get(path).map(|f| f.folds.clone()).unwrap_or_default())
}

/// A file's folds known from its document: its edits move them from then on. A document other than the one
/// known (the file opened again) starts with none.
pub fn attach(ui: Ui, path: &str, doc: &Rc<TextDocument>) {
    let id = Rc::as_ptr(doc) as usize;
    let known = FILES.with(|f| f.borrow().get(path).map(|f| f.doc));
    if known == Some(id) {
        return;
    }
    let file = File {
        folds: Vec::new(),
        fillers: Vec::new(),
        map: Rc::new(FoldMap::default()),
        rev: ui.scope.create_rw_signal(0u64),
        text: doc.text(),
        doc: id,
        regions: None,
    };
    FILES.with(|f| f.borrow_mut().insert(path.to_string(), file));
    let (path, weak) = (path.to_string(), Rc::downgrade(doc));
    doc.add_on_update(move |u| {
        let Some(doc) = weak.upgrade() else { return };
        edited(ui, &path, Rc::as_ptr(&doc) as usize, u.deltas(), doc.text());
    });
}

/// The folds moved by edits of a file's document.
fn edited<'a>(
    ui: Ui,
    path: &str,
    doc: usize,
    deltas: impl Iterator<Item = &'a floem::views::editor::core::xi_rope::RopeDelta>,
    now: Rope,
) {
    let changed = FILES.with(|f| {
        let mut files = f.borrow_mut();
        let file = files.get_mut(path).filter(|f| f.doc == doc)?;
        let before = file.folds.clone();
        for delta in deltas {
            let (iv, len) = delta.summary();
            let old = &file.text;
            let from = old.line_of_offset(iv.start.min(old.len()));
            let to_old = old.line_of_offset(iv.end.min(old.len()));
            let new = delta.apply(old);
            let to_new = new.line_of_offset((iv.start + len).min(new.len()));
            shift(&mut file.folds, from, to_old, to_new);
            file.text = new;
        }
        // Not the text as the document has it (an edit not told): nothing is known of where the folds are.
        if file.text.len() != now.len() {
            file.folds.clear();
        }
        file.text = now;
        (file.folds != before).then(|| {
            file.map = Rc::new(FoldMap::new(&file.folds).with_fillers(&file.fillers));
        })
    });
    if changed.is_some() {
        // Told after the edit, not while the document is being changed.
        let path = path.to_string();
        floem::action::exec_after(std::time::Duration::ZERO, move |_| told(ui, &path));
    }
}

/// A file's folds changed: its editors draw them, and its folded regions' first lines are marked.
fn told(ui: Ui, path: &str) {
    let Some((rev, folds, text)) =
        FILES.with(|f| f.borrow().get(path).map(|f| (f.rev, f.folds.clone(), f.text.clone())))
    else {
        return;
    };
    if rev.try_get_untracked().is_some() {
        rev.update(|r| *r += 1);
    }
    let text = RopeTextVal::new(text);
    let last = text.last_line();
    let marks = folds
        .iter()
        .filter(|f| f.0 <= last)
        .flat_map(|&(s, _)| {
            [
                Mark::Line { line: s, bg: Tint::Theme(|c| c.fold_bg) },
                Mark::Text { line: s, col: text.line_end_col(s, true), text: " ⋯".into(), at: Phantom::Inline },
            ]
        })
        .collect::<Vec<_>>();
    if marks.is_empty() {
        crate::decor::clear(ui, "folding", path)
    } else {
        crate::decor::set(ui, "folding", path, marks)
    }
}

/// The empty rows above lines of an editor's text, (the line, how many rows), in place of those said before: its
/// editors draw them. Nothing before the text is `attach`ed under that path.
pub fn set_fillers(path: &str, fillers: Vec<(usize, usize)>) {
    let rev = FILES.with(|files| {
        let mut files = files.borrow_mut();
        let file = files.get_mut(path).filter(|f| f.fillers != fillers)?;
        file.fillers = fillers;
        file.map = Rc::new(FoldMap::new(&file.folds).with_fillers(&file.fillers));
        Some(file.rev)
    });
    if let Some(rev) = rev
        && rev.try_get_untracked().is_some()
    {
        rev.update(|r| *r += 1);
    }
}

/// A file's folds changed by a command.
fn change(ui: Ui, path: &str, f: impl FnOnce(&mut Vec<(usize, usize)>)) {
    let changed = FILES.with(|files| {
        let mut files = files.borrow_mut();
        let file = files.get_mut(path)?;
        let before = file.folds.clone();
        f(&mut file.folds);
        file.folds.sort();
        file.folds.dedup_by_key(|f| f.0);
        (file.folds != before).then(|| file.map = Rc::new(FoldMap::new(&file.folds).with_fillers(&file.fillers)))
    });
    if changed.is_some() {
        told(ui, path);
    }
}

/// A file's regions, found again when its text changed since.
pub(crate) fn regions_of(path: &str, editor: &Editor) -> Rc<Vec<(usize, usize)>> {
    let rev = editor.doc().cache_rev().get_untracked();
    if let Some(r) = FILES.with(|f| f.borrow().get(path).and_then(|f| f.regions.clone())).filter(|r| r.0 == rev) {
        return r.1;
    }
    let text = editor.doc().text().to_string();
    let lines: Vec<&str> = text.split('\n').collect();
    let found = Rc::new(regions(&lines, 4, off_side(path)));
    FILES.with(|f| {
        if let Some(file) = f.borrow_mut().get_mut(path) {
            file.regions = Some((rev, found.clone()));
        }
    });
    found
}

/// Whether an editor's lines are one row each (its folds apply then).
fn unwrapped(editor: &Editor) -> bool {
    editor.es.with_untracked(|s| s.wrap_method()) == WrapMethod::None
}

/// The fold map an editor of a file draws with, followed (none: nothing folded, or its lines wrap).
pub(crate) fn followed(path: &str, editor: &Editor) -> Option<Rc<FoldMap>> {
    let rev = FILES.with(|f| f.borrow().get(path).map(|f| f.rev))?;
    rev.track();
    map(path).filter(|_| unwrapped(editor))
}

/// The lines on screen of an editor of a file with folds: the rows of the viewport, each the line the fold map
/// puts there (none: nothing folded, Floem's own then).
pub fn screen_lines(editor: &Editor, base: RwSignal<ScreenLinesBase>, path: &str) -> Option<ScreenLines> {
    let map = followed(path, editor)?;
    let height = f64::from(editor.line_height(0));
    let (y0, y1) = base.with_untracked(|b| (b.active_viewport.y0, b.active_viewport.y1));
    editor.doc().cache_rev().track();
    let last = editor.last_line();
    let mut lines = Vec::new();
    let mut info = HashMap::new();
    for row in (y0 / height).floor() as usize..(y1 / height).ceil() as usize {
        let Some(line) = map.line_at_row(row) else { continue };
        if line > last {
            break;
        }
        // Its layout as of the text now (it is laid out again when the text changed).
        editor.text_layout_trigger(line, false);
        let Some(vline_info) = editor.iter_rvlines(false, RVLine::new(line, 0)).next() else { break };
        let y = row as f64 * height - y0;
        lines.push(vline_info.rvline);
        info.insert(vline_info.rvline, LineInfo { y, vline_y: y, vline_info });
    }
    Some(ScreenLines { lines: Rc::new(lines), info: Rc::new(info), diff_sections: None, base })
}

// ==== The caret ====

/// The caret kept out of folded lines: Up or Down from beside a fold goes over it; put in one another way, the fold
/// opens.
fn keep_caret(ui: Ui, path: &str, editor: &Editor, before: Option<usize>) -> usize {
    let offset = editor.cursor.with(|c| c.offset());
    let (line, col) = editor.offset_to_line_col(offset);
    let Some(map) = map(path).filter(|_| unwrapped(editor)) else { return line };
    let Some((first, last)) = map.run_of(line) else { return line };
    let header = first - 1;
    let to = match before {
        Some(b) if b == header && line > b => Some(if last < editor.last_line() { last + 1 } else { header }),
        Some(b) if b == last + 1 && line < b => Some(header),
        _ => None,
    };
    match to {
        Some(to) => {
            let at = editor.offset_of_line_col(to, col.min(editor.line_end_col(to, true)));
            let selecting = editor.cursor.with_untracked(|c| match &c.mode {
                CursorMode::Insert(s) => !s.is_caret(),
                _ => false,
            });
            editor.cursor.update(|c| c.set_offset(at, selecting, false));
            to
        }
        None => {
            change(ui, path, |f| f.retain(|&(s, e)| !(s < line && line <= e)));
            line
        }
    }
}

// ==== The editor ====

/// The width of the gutter's part right of the line numbers, where the chevrons are (`gutter_right_padding`).
pub const CHEVRONS: f64 = 22.0;

/// A file's editor, as Floem's own (its gutter, then its text in a scroll view), with the folds: its height is
/// that of the rows drawn, the caret is scrolled to the row it is on, and the gutter has the chevrons.
pub fn view<K: Fn(RwSignal<Editor>, &KeyPress, Modifiers) -> CommandExecuted + 'static>(
    ui: Ui,
    editor: Editor,
    doc: &Rc<TextDocument>,
    path: &str,
    keys: K,
) -> impl IntoView + use<K> {
    attach(ui, path, doc);
    let sig = floem::reactive::create_rw_signal(editor.clone());
    let gutter_width = floem::reactive::create_rw_signal(0.0);
    let hovered = floem::reactive::create_rw_signal(false);
    {
        let (path, e) = (path.to_string(), editor.clone());
        create_effect(move |before: Option<usize>| keep_caret(ui, &path, &e, before));
    }
    let gutter = editor_gutter(sig).on_resize(move |r| {
        if gutter_width.get_untracked() != r.width() {
            gutter_width.set(r.width());
        }
    });
    let strip = Chevrons { id: ViewId::new(), ui, editor: editor.clone(), path: path.to_string(), hovered };
    let id = strip.id;
    let (screen, viewport, colors, cache) = (editor.screen_lines, editor.viewport, ui.colors, editor.doc().cache_rev());
    let rev = FILES.with(|f| f.borrow().get(path).map(|f| f.rev));
    create_effect(move |_| {
        screen.track();
        viewport.track();
        hovered.track();
        colors.track();
        cache.track();
        if let Some(rev) = rev {
            rev.track();
        }
        id.request_paint();
    });
    let (shown, clicked) = (editor.clone(), path.to_string());
    let strip = strip
        .on_event(EventListener::PointerDown, move |e| {
            let Event::PointerDown(p) = e else { return EventPropagation::Continue };
            let Some(line) = line_at(&shown, p.pos.y) else { return EventPropagation::Continue };
            if !is_header(&clicked, &shown, line) {
                return EventPropagation::Continue;
            }
            let regions = regions_of(&clicked, &shown);
            change(ui, &clicked, |f| toggle(f, &regions, line));
            EventPropagation::Stop
        })
        .style(move |s| {
            let on = crate::settings::flag(ui, "editor.folding", None);
            s.absolute()
                .inset_top(0.0)
                .inset_left(gutter_width.get() - CHEVRONS)
                .width(CHEVRONS)
                .height_full()
                .apply_if(!on, |s| s.hide())
        });
    let e = editor.clone();
    let bars = crate::scm::quick_diff::bars(ui, &editor, path, gutter_width);
    stack((gutter, bars, content(sig, path, keys), strip))
        .on_event_cont(EventListener::PointerMove, move |e| {
            if let Event::PointerMove(p) = e {
                let over = p.pos.x < gutter_width.get_untracked();
                if hovered.get_untracked() != over {
                    hovered.set(over);
                }
            }
        })
        .on_event_cont(EventListener::PointerLeave, move |_| {
            if hovered.get_untracked() {
                hovered.set(false);
            }
        })
        .on_cleanup(move || e.cx.get().dispose())
        .style(|s| s.size_full())
}

/// The editor's text in its scroll view: Floem's `editor_content`, with the size and the caret's place from the
/// folds.
fn content<K: Fn(RwSignal<Editor>, &KeyPress, Modifiers) -> CommandExecuted + 'static>(
    sig: RwSignal<Editor>,
    path: &str,
    keys: K,
) -> impl IntoView + use<K> {
    let ed = sig.get_untracked();
    let (cursor, scroll_delta, scroll_to, window_origin, viewport) =
        (ed.cursor, ed.scroll_delta, ed.scroll_to, ed.window_origin, ed.viewport);
    let text = editor_view(sig, |_| true).style(|s| s.absolute().cursor(CursorStyle::Text));
    let id = text.id();
    ed.editor_view_id.set(Some(id));
    let text = crate::keymap::tracked(text, crate::keymap::Focus::Editor)
        .on_event_cont(EventListener::FocusGained, move |_| sig.with_untracked(|e| e.editor_view_focused.notify()))
        .on_event_cont(EventListener::FocusLost, move |_| sig.with_untracked(|e| e.editor_view_focus_lost.notify()))
        .on_event_cont(EventListener::PointerDown, move |event| {
            if let Event::PointerDown(p) = event {
                id.request_active();
                id.request_focus();
                sig.get_untracked().pointer_down(p);
            }
        })
        .on_event_cont(EventListener::PointerMove, move |event| {
            if let Event::PointerMove(p) = event {
                sig.get_untracked().pointer_move(p);
            }
        })
        .on_event_cont(EventListener::PointerUp, move |event| {
            if let Event::PointerUp(p) = event {
                sig.get_untracked().pointer_up(p);
            }
        })
        .on_event_stop(EventListener::KeyDown, move |event| {
            let Event::KeyDown(key_event) = event else { return };
            let key_text = key_event.key.text.clone();
            let Ok(press) = KeyPress::try_from(key_event) else { return };
            // A key a command took (a chord's second key, Ctrl+K D) is not typed too.
            if keys(sig, &press, key_event.modifiers) == CommandExecuted::Yes {
                return;
            }
            let mut mods = key_event.modifiers;
            mods.set(Modifiers::SHIFT, false);
            mods.set(Modifiers::ALTGR, false);
            if mods.is_empty() {
                match press.key {
                    KeyInput::Keyboard(Key::Character(c), _) => sig.get_untracked().receive_char(&c),
                    KeyInput::Keyboard(Key::Named(NamedKey::Space), _) => sig.get_untracked().receive_char(" "),
                    KeyInput::Keyboard(Key::Unidentified(_), _) => {
                        if let Some(text) = key_text {
                            sig.get_untracked().receive_char(&text);
                        }
                    }
                    _ => {}
                }
            }
        });
    // The size scrolled over: as wide as the longest line laid out, as high as the rows drawn, and at least the
    // scroll view's own size. The editor's viewport alone cannot give that size: Floem clips it to the text view,
    // which is as wide as this container, so the current line's highlight stopped at the longest line's end.
    let (size_path, screen) = (path.to_string(), ed.screen_lines);
    let size = create_memo(move |_| {
        screen.track();
        let vp = viewport.get();
        let e = sig.get_untracked();
        let height = f64::from(e.line_height(0));
        let rows = match followed(&size_path, &e) {
            Some(map) => map.rows(e.last_line() + 1),
            None => e.last_vline().get() + 1,
        } as f64
            * height;
        let beyond =
            if e.es.with_untracked(|s| s.scroll_beyond_last_line()) { vp.height().min(rows) - height } else { 0.0 };
        // Wider by what a minimap covers at the right, so a long line's end can be scrolled out from under it.
        let room = super::minimap::room(&e);
        ((e.max_line_width() + room).max(vp.width()), rows.max(vp.height()) + beyond.max(0.0))
    });
    let caret_path = path.to_string();
    scroll(container(text).style(move |s| {
        let (w, h) = size.get();
        // Out of the flow, as Floem's own editor puts its text: in it, its size made the scroll view as wide as
        // the longest line, so a long line was cut at the group's edge with nothing to scroll.
        s.absolute().width(w).height(h).min_size_full()
    }))
    .on_move(move |p| window_origin.set(p))
    .scroll_to(move || scroll_to.get().map(Vec2::to_point))
    .scroll_delta(move || scroll_delta.get())
    .ensure_visible(move || {
        let editor = sig.get_untracked();
        let cursor = cursor.get();
        editor.doc().cache_rev().track();
        let LineRegion { x, width, rvline } =
            cursor_caret(&editor, cursor.offset(), !cursor.is_insert(), cursor.affinity);
        let height = f64::from(editor.line_height(0));
        let row = match map(&caret_path).filter(|_| unwrapped(&editor)) {
            Some(map) => map.row_of_line(rvline.line),
            None => editor.vline_of_rvline(rvline).get(),
        };
        let rect = Rect::from_origin_size((x, row as f64 * height), (width, height)).inflate(10.0, 1.0);
        // Kept left of a minimap at the right.
        let rect = Rect { x1: rect.x1 + super::minimap::room(&editor), ..rect };
        let vp = viewport.get_untracked();
        let near = [(vp.y0 - rect.y0), (vp.y1 - rect.y0), (vp.y0 - rect.y1), (vp.y1 - rect.y1)].map(f64::abs);
        let smallest = near.iter().copied().fold(f64::MAX, f64::min);
        let biggest = near.iter().copied().fold(0.0, f64::max);
        if biggest > vp.height() && smallest > vp.height() / 2.0 {
            rect.inflate(0.0, vp.height() / 2.0)
        } else {
            let around = editor.es.with(|s| s.cursor_surrounding_lines()) as f64 * height;
            Rect::new(rect.x0, rect.y0 - around, rect.x1, rect.y1 + around)
        }
    })
    .style(|s| s.size_pct(100.0, 100.0))
}

/// The caret shown in the middle of an editor (Go to Line, a search's hit): at its row with the folds.
pub fn center(editor: &Editor, path: &str) {
    let Some(map) = map(path).filter(|_| unwrapped(editor)) else {
        editor.center_window();
        return;
    };
    let line = editor.offset_to_line_col(editor.cursor.with_untracked(|c| c.offset())).0;
    let height = f64::from(editor.line_height(0));
    let vp = editor.viewport.get_untracked();
    let top = (map.row_of_line(line) as f64 * height - vp.height() / 2.0 + height / 2.0).max(0.0);
    editor.scroll_to.set(Some(Vec2::new(vp.x0, top)));
}

/// The line drawn at a height in the editor's view (none: below the last).
fn line_at(editor: &Editor, y: f64) -> Option<usize> {
    let top = editor.viewport.get_untracked().y0;
    let height = f64::from(editor.line_height(0));
    editor
        .screen_lines
        .with_untracked(|s| s.iter_lines_y().find(|(_, at)| (at - top..at - top + height).contains(&y)).map(|l| l.0))
}

/// Whether a line starts a region of a file (a folded one does).
fn is_header(path: &str, editor: &Editor, line: usize) -> bool {
    FILES.with(|f| f.borrow().get(path).is_some_and(|f| f.folds.iter().any(|x| x.0 == line)))
        || regions_of(path, editor).binary_search_by(|r| r.0.cmp(&line)).is_ok()
}

/// The gutter's chevrons, right of the line numbers: a folded region's (right) always, the others' (down) while the
/// pointer is over the gutter.
struct Chevrons {
    id: ViewId,
    ui: Ui,
    editor: Editor,
    path: String,
    hovered: RwSignal<bool>,
}

impl View for Chevrons {
    fn id(&self) -> ViewId {
        self.id
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let folded: Vec<usize> = folds(&self.path).iter().map(|f| f.0).collect();
        let hovered = self.hovered.get_untracked();
        if folded.is_empty() && !hovered {
            return;
        }
        let regions = if hovered { regions_of(&self.path, &self.editor) } else { Rc::new(Vec::new()) };
        let c = self.ui.colors.get_untracked();
        let top = self.editor.viewport.get_untracked().y0;
        let height = f64::from(self.editor.line_height(0));
        let family: Vec<FamilyOwned> = FamilyOwned::parse_list("codicon").collect();
        let attrs = Attrs::new()
            .family(&family)
            .font_size(16.0)
            .line_height(LineHeightValue::Px(16.0))
            .color(c.folding_control);
        self.editor.screen_lines.with_untracked(|s| {
            for (line, y) in s.iter_lines_y() {
                let icon = if folded.contains(&line) {
                    "chevron-right"
                } else if regions.binary_search_by(|r| r.0.cmp(&line)).is_ok() {
                    "chevron-down"
                } else {
                    continue;
                };
                let mut layout = TextLayout::new();
                layout.set_text(&crate::icons::char_of(icon).to_string(), AttrsList::new(attrs));
                cx.draw_text(&layout, Point::new((CHEVRONS - 16.0) / 2.0, y - top + (height - 16.0) / 2.0));
            }
        });
    }
}

// ==== The commands ====

/// The editor on screen with the keys, its file and its caret's line.
fn live(ui: Ui) -> Option<(String, Editor, usize)> {
    let (path, editor) = ui.live.get_untracked()?;
    let line = editor.offset_to_line_col(editor.cursor.with_untracked(|c| c.offset())).0;
    Some((path, editor, line))
}

/// A command on the folds of the editor on screen, with its regions and its caret's line; the caret then on the
/// line its fold starts on when it was folded away.
fn on_live(ui: Ui, f: impl FnOnce(&mut Vec<(usize, usize)>, &[(usize, usize)], usize)) {
    let Some((path, editor, line)) = live(ui) else { return };
    let regions = regions_of(&path, &editor);
    change(ui, &path, |folds| f(folds, &regions, line));
    if let Some(map) = map(&path)
        && map.is_hidden(line)
    {
        let to = map.line_of_row(map.row_of_line(line));
        editor.cursor.update(|c| c.set_offset(editor.offset_of_line(to), false, false));
    }
}

/// What it adds to the window's registries, once at start: the commands and their keys; and when folding is turned
/// off, every fold opened.
pub fn register(ui: Ui, _send: &Send) {
    let on = |ui: Ui| crate::settings::flag(ui, "editor.folding", None) && ui.live.with_untracked(|l| l.is_some());
    let add = |id: &str, title: &str, keys: &[&str], run: fn(Ui)| {
        let mut c = command(id, "", title, move |ui, _| run(ui)).enabled(on);
        for k in keys {
            c = c.keys(k);
        }
        commands::add(c);
    };
    add("editor.fold", "Fold", &["Ctrl+Shift+["], |ui| on_live(ui, fold));
    add("editor.unfold", "Unfold", &["Ctrl+Shift+]"], |ui| on_live(ui, |f, _, l| unfold(f, l)));
    add("editor.toggleFold", "Toggle Fold", &["Ctrl+K Ctrl+L"], |ui| on_live(ui, toggle));
    add("editor.foldRecursively", "Fold Recursively", &["Ctrl+K Ctrl+["], |ui| {
        on_live(ui, |f, r, l| recursively(f, r, l, true))
    });
    add("editor.unfoldRecursively", "Unfold Recursively", &["Ctrl+K Ctrl+]"], |ui| {
        on_live(ui, |f, r, l| recursively(f, r, l, false))
    });
    add("editor.foldAll", "Fold All", &["Ctrl+K Ctrl+0"], |ui| on_live(ui, |f, r, _| *f = r.to_vec()));
    add("editor.unfoldAll", "Unfold All", &["Ctrl+K Ctrl+J"], |ui| on_live(ui, |f, _, _| f.clear()));
    let levels: [fn(Ui); 7] = [
        |ui| on_live(ui, |f, r, l| fold_level(f, r, 1, l)),
        |ui| on_live(ui, |f, r, l| fold_level(f, r, 2, l)),
        |ui| on_live(ui, |f, r, l| fold_level(f, r, 3, l)),
        |ui| on_live(ui, |f, r, l| fold_level(f, r, 4, l)),
        |ui| on_live(ui, |f, r, l| fold_level(f, r, 5, l)),
        |ui| on_live(ui, |f, r, l| fold_level(f, r, 6, l)),
        |ui| on_live(ui, |f, r, l| fold_level(f, r, 7, l)),
    ];
    for (n, run) in levels.into_iter().enumerate() {
        let n = n + 1;
        add(&format!("editor.foldLevel{n}"), &format!("Fold Level {n}"), &[&format!("Ctrl+K Ctrl+{n}")], run);
    }
    ui.scope.create_effect(move |_| {
        if !crate::settings::flag(ui, "editor.folding", None) {
            let paths: Vec<String> = FILES
                .with(|f| f.borrow().iter().filter(|(_, f)| !f.folds.is_empty()).map(|(p, _)| p.clone()).collect());
            for p in paths {
                floem::reactive::untrack(|| change(ui, &p, |f| f.clear()));
            }
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

    fn lines(text: &str) -> Vec<&str> {
        text.split('\n').collect()
    }

    #[test]
    fn indentation_counts_tabs_to_the_next_stop_and_skips_empty_lines() {
        assert_eq!(indent_of("    a", 4), Some(4));
        assert_eq!(indent_of("\ta", 4), Some(4));
        assert_eq!(indent_of("  \ta", 4), Some(4));
        assert_eq!(indent_of("a", 4), Some(0));
        assert_eq!(indent_of("   ", 4), None);
        assert_eq!(indent_of("", 4), None);
    }

    #[test]
    fn regions_are_vs_codes_by_indentation() {
        let text = "fn a() {\n    if x {\n        y();\n    }\n\n    z();\n}\nfn b() {}";
        // The closing brace is not in its region (it is as indented as the first line); an empty line is.
        assert_eq!(regions(&lines(text), 4, false), vec![(0, 5), (1, 2)]);
        assert_eq!(levels(&regions(&lines(text), 4, false)), vec![1, 2]);
        // Off-side: the empty lines after a block are not in it.
        let py = "def f():\n    a\n\n    b\n\n\nc = 1\n";
        assert_eq!(regions(&lines(py), 4, true), vec![(0, 3)]);
        assert_eq!(regions(&lines(py), 4, false), vec![(0, 5)]);
        // A last region runs to the end of the text.
        assert_eq!(regions(&lines("a:\n  b\n  c"), 4, true), vec![(0, 2)]);
        // One line under it is enough, none is not.
        assert_eq!(regions(&lines("a\n  b\nc\nd"), 4, false), vec![(0, 1)]);
        assert!(regions(&lines("a\nb\n"), 4, false).is_empty());
    }

    #[test]
    fn markers_make_regions_with_their_end_line() {
        assert_eq!(marker("// #region Setup"), Some(true));
        assert_eq!(marker("  //#endregion"), Some(false));
        assert_eq!(marker("#region"), Some(true));
        assert_eq!(marker("#pragma region x"), Some(true));
        assert_eq!(marker("<!-- #region -->"), Some(true));
        assert_eq!(marker("// regional"), None);
        assert_eq!(marker("let region = 1;"), None);
        let text = "// #region a\nx\n// #region b\ny\n// #endregion\n// #endregion\nz";
        assert_eq!(regions(&lines(text), 4, false), vec![(0, 5), (2, 4)]);
    }

    #[test]
    fn the_map_puts_each_line_shown_on_its_row() {
        // Lines 2..=4 and 7..=8 hidden (folds 1..=4, a fold in it 2..=3, and 6..=8).
        let map = FoldMap::new(&[(1, 4), (2, 3), (6, 8)]);
        assert_eq!(map.count(), 5);
        assert_eq!(map.rows(12), 7);
        let shown: Vec<usize> = (0..7).map(|r| map.line_of_row(r)).collect();
        assert_eq!(shown, vec![0, 1, 5, 6, 9, 10, 11]);
        for (row, &line) in shown.iter().enumerate() {
            assert_eq!(map.row_of_line(line), row);
            assert!(!map.is_hidden(line));
        }
        assert!(map.is_hidden(3) && map.is_hidden(8));
        assert_eq!(map.row_of_line(3), 1);
        assert_eq!(map.run_of(7), Some((7, 8)));
        assert_eq!(map.run_of(5), None);
        // A fold right after another keeps its first line; folds over each other make one run.
        assert_eq!(FoldMap::new(&[(0, 2), (3, 5)]).run_of(3), None);
        assert_eq!(FoldMap::new(&[(0, 3), (1, 5)]).run_of(2), Some((1, 5)));
        assert_eq!(FoldMap::new(&[(0, 3), (1, 5)]).count(), 5);
        assert!(FoldMap::new(&[]).is_empty());
        assert_eq!(FoldMap::new(&[]).line_of_row(4), 4);
    }

    #[test]
    fn empty_rows_put_the_lines_under_them_lower() {
        let map = FoldMap::default().with_fillers(&[(2, 2), (5, 1), (2, 1), (9, 0)]);
        let rows: Vec<Option<usize>> = (0..10).map(|r| map.line_at_row(r)).collect();
        assert_eq!(rows, [Some(0), Some(1), None, None, None, Some(2), Some(3), Some(4), None, Some(5)]);
        for line in 0..7 {
            assert_eq!(map.line_at_row(map.row_of_line(line)), Some(line));
        }
        assert_eq!(map.line_of_row(3), 2, "an empty row's line is the one under it");
        assert_eq!(map.rows(7), 11);
        assert_eq!(map.filler_rows().collect::<Vec<_>>(), [(2, 3), (8, 1)]);
        // With a fold above them.
        let map = FoldMap::new(&[(0, 2)]).with_fillers(&[(4, 2)]);
        let rows: Vec<Option<usize>> = (0..5).map(|r| map.line_at_row(r)).collect();
        assert_eq!(rows, [Some(0), Some(3), None, None, Some(4)]);
        assert_eq!(map.row_of_line(5), 5);
    }

    #[test]
    fn the_commands_fold_as_vs_codes() {
        let regions = vec![(0, 6), (1, 3), (4, 5)];
        let mut folds = Vec::new();
        fold(&mut folds, &regions, 2);
        assert_eq!(folds, vec![(1, 3)]);
        // Again: the region around it.
        fold(&mut folds, &regions, 2);
        assert_eq!(folds, vec![(1, 3), (0, 6)]);
        unfold(&mut folds, 0);
        assert_eq!(folds, vec![(1, 3)]);
        toggle(&mut folds, &regions, 1);
        assert!(folds.is_empty());
        toggle(&mut folds, &regions, 4);
        assert_eq!(folds, vec![(4, 5)]);
        recursively(&mut folds, &regions, 0, true);
        folds.sort();
        assert_eq!(folds, vec![(0, 6), (1, 3), (4, 5)]);
        recursively(&mut folds, &regions, 0, false);
        assert!(folds.is_empty());
        fold_level(&mut folds, &regions, 2, 2);
        assert_eq!(folds, vec![(4, 5)]);
    }

    #[test]
    fn an_edit_moves_the_folds_after_it_and_opens_the_one_it_is_in() {
        let mut folds = vec![(2, 4), (8, 9)];
        // A line added on line 0.
        shift(&mut folds, 0, 0, 1);
        assert_eq!(folds, vec![(3, 5), (9, 10)]);
        // Typed on a folded region's first line.
        shift(&mut folds, 3, 3, 3);
        assert_eq!(folds, vec![(3, 5), (9, 10)]);
        // Two lines taken away after the first fold, before the second.
        shift(&mut folds, 6, 8, 6);
        assert_eq!(folds, vec![(3, 5), (7, 8)]);
        // In the second's hidden lines: it opens.
        shift(&mut folds, 8, 8, 8);
        assert_eq!(folds, vec![(3, 5)]);
    }

    #[test]
    fn a_documents_edits_move_its_files_folds() {
        use floem::views::editor::core::editor::EditType;
        use floem::views::editor::core::selection::Selection;
        let ui = Ui::new();
        let doc = Rc::new(TextDocument::new(ui.scope, "a {\n  b\n  c\n}\nd {\n  e\n}\n"));
        attach(ui, "f.rs", &doc);
        change(ui, "f.rs", |f| f.push((4, 5)));
        assert_eq!(map("f.rs").map(|m| m.rows(8)), Some(7));
        doc.edit_single(Selection::caret(0), "x\n", EditType::InsertChars);
        assert_eq!(folds("f.rs"), vec![(5, 6)]);
        // In the fold: it opens.
        let at = doc.text().offset_of_line(6);
        doc.edit_single(Selection::caret(at), "y", EditType::InsertChars);
        assert!(folds("f.rs").is_empty() && map("f.rs").is_none());
    }
}
