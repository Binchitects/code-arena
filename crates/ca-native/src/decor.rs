//! The editor's decorations, as VS Code's: what other parts mark in a file's text. A part (an owner: problems, quick
//! diff, breakpoints, inline completion…) says all its marks for a file at once with `set` (or `clear`), and they are
//! shown in every editor of that file:
//!
//! - a glyph in the glyph margin, the 18 px column left of the line numbers (a codicon, its title, and what a click
//!   on it does);
//! - a line's background, the whole width;
//! - a range's look: its text colour, its background, a squiggle by severity (VS Code's error, warning, info
//!   colours; a hint is a thin underline), an underline, a line through it; with its hover text;
//! - text that is not in the file (phantom text): inline (as an inlay hint), at the end of a line (as an error
//!   lens), or ghost text (as an inline completion, the caret before it);
//! - a mark in the overview ruler, the right edge (a squiggle's severity puts one there by itself).
//!
//! The pointer resting on a range shows its hover (on a glyph: its title) after VS Code's 300 ms, under the line; it
//! goes when the pointer moves off, the editor scrolls or the caret moves.
//!
//! Lines and columns count from 0; a column is a byte offset in the line, as the editor's own. Marks are not moved
//! by edits: an owner says them again when its view of the file changes, and marks past the text are left out when
//! drawn. All owners' marks for a file are merged once when one owner changes them (and again when the theme
//! changes); the editor reads the merged lines while it lays out the lines on screen, so the syntax is not parsed
//! again for them.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use floem::context::PaintCx;
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::Modifiers;
use floem::peniko::Color;
use floem::peniko::kurbo::{Point, Rect};
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use floem::text::{Attrs, AttrsList, FamilyOwned, LineHeightValue, TextLayout};
use floem::views::Decorators;
use floem::views::editor::command::{Command, CommandExecuted};
use floem::views::editor::core::buffer::rope_text::{RopeText, RopeTextVal};
use floem::views::editor::core::cursor::CursorAffinity;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::mode::Mode;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::core::xi_rope::Rope;
use floem::views::editor::id::EditorId;
use floem::views::editor::layout::{LineExtraStyle, TextLayoutLine};
use floem::views::editor::phantom_text::{PhantomText, PhantomTextKind, PhantomTextLine};
use floem::views::editor::text::{Document, DocumentPhantom, PreeditData};
use floem::views::editor::text_document::TextDocument;
use floem::views::editor::view::{ScreenLines, ScreenLinesBase};
use floem::views::editor::{Editor, EditorStyle};
use floem::{IntoView, View, ViewId};
use floem_renderer::Renderer;
use serde_json::Value;

use crate::colors::Colors;
use crate::state::{Send, Ui};

/// What a click on a glyph does, given the line (from 0).
pub type Click = Rc<dyn Fn(Ui, &Send, usize)>;

/// How bad a problem is, as VS Code's markers say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// A colour: one of the theme's (taken again when the theme changes) or a fixed one.
#[derive(Clone, Copy)]
pub enum Tint {
    Theme(fn(&Colors) -> Color),
    Fixed(Color),
}

impl Tint {
    pub fn of(self, c: &Colors) -> Color {
        match self {
            Tint::Theme(f) => f(c),
            Tint::Fixed(color) => color,
        }
    }
}

/// Where text that is not in the file is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phantom {
    /// Inside the line at its column, as an inlay hint (`editorInlayHint.foreground`).
    Inline,
    /// After the line's end, as an error lens: in its severity's colour, or as a description.
    EndOfLine(Option<Severity>),
    /// At its column, as an inline completion is (`editorGhostText.foreground`), the caret before it.
    Ghost,
}

/// A range's look; what is none is left as it is.
#[derive(Clone, Copy, Default)]
pub struct Look {
    pub fg: Option<Tint>,
    pub bg: Option<Tint>,
    pub squiggle: Option<Severity>,
    pub underline: Option<Tint>,
    pub strike: bool,
}

/// One decoration an owner gives.
#[derive(Clone)]
pub enum Mark {
    /// A codicon in the glyph margin; `title` is what it is.
    Glyph { line: usize, icon: &'static str, tint: Tint, title: String, click: Option<Click> },
    /// A line's background, the whole width.
    Line { line: usize, bg: Tint },
    /// A range (from, to: line and column; `to` not in it) drawn with a look, with what its hover says.
    Range { from: (usize, usize), to: (usize, usize), look: Look, hover: Option<String> },
    /// Text that is not in the file, at a line and column.
    Text { line: usize, col: usize, text: String, at: Phantom },
    /// A mark in the overview ruler.
    Ruler { line: usize, tint: Tint },
}

impl Mark {
    /// A problem's squiggle over a range, with its message as the hover (and its mark in the overview ruler).
    pub fn squiggle(from: (usize, usize), to: (usize, usize), severity: Severity, message: &str) -> Mark {
        let look = Look { squiggle: Some(severity), ..Look::default() };
        Mark::Range { from, to, look, hover: Some(message.to_string()).filter(|m| !m.is_empty()) }
    }
}

/// A glyph as drawn.
#[derive(Clone)]
pub struct Glyph {
    pub icon: &'static str,
    pub color: Color,
    pub title: String,
    pub click: Option<Click>,
}

/// A piece of a range on one line (`end` none: to the line's end), its colours taken from the theme.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: Option<usize>,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub wave: Option<Color>,
    pub under: Option<Color>,
    pub strike: Option<Color>,
    pub hover: Option<String>,
}

/// What is on one line, all owners' together.
#[derive(Clone, Default)]
pub struct LineMarks {
    /// The first owner's glyph (owners in the order of their names).
    pub glyph: Option<Glyph>,
    pub bg: Option<Color>,
    pub spans: Vec<Span>,
    /// Column, text, where, colour; by column.
    pub texts: Vec<(usize, String, Phantom, Color)>,
}

/// A file's marks merged: by line, and the overview ruler's (line, colour) in the order of the lines.
#[derive(Clone, Default)]
pub struct Merged {
    pub lines: BTreeMap<usize, LineMarks>,
    pub ruler: Vec<(usize, Color)>,
}

/// A see-through colour laid on the editor's background: text is drawn without its colour's alpha (and a thin fill
/// with it comes out near black).
fn on_editor(color: Color, c: &Colors) -> Color {
    let (a, bg) = (color.a as f64 / 255.0, c.editor_bg);
    let mix = |f: u8, b: u8| (f as f64 * a + b as f64 * (1.0 - a)).round() as u8;
    Color::rgb8(mix(color.r, bg.r), mix(color.g, bg.g), mix(color.b, bg.b))
}

fn squiggle_color(s: Severity, c: &Colors) -> Color {
    match s {
        Severity::Error => c.problem_error,
        Severity::Warning => c.warning,
        Severity::Info => c.problem_info,
        Severity::Hint => c.problem_hint,
    }
}

fn ruler_color(s: Severity, c: &Colors) -> Option<Color> {
    match s {
        Severity::Error => Some(c.ruler_error),
        Severity::Warning => Some(c.ruler_warning),
        Severity::Info => Some(c.ruler_info),
        Severity::Hint => None,
    }
}

/// The owners' marks as one file's lines, with the theme's colours.
pub fn merge<'a>(owners: impl IntoIterator<Item = &'a [Mark]>, c: &Colors) -> Merged {
    let mut m = Merged::default();
    let mut ruler = Vec::new();
    for marks in owners {
        for mark in marks {
            match mark {
                Mark::Glyph { line, icon, tint, title, click } => {
                    let at = m.lines.entry(*line).or_default();
                    if at.glyph.is_none() {
                        at.glyph = Some(Glyph { icon, color: tint.of(c), title: title.clone(), click: click.clone() });
                    }
                }
                Mark::Line { line, bg } => m.lines.entry(*line).or_default().bg = Some(bg.of(c)),
                Mark::Range { from, to, look, hover } => {
                    let (from, to) = if to < from { (*to, *from) } else { (*from, *to) };
                    let hint = look.squiggle.filter(|s| *s == Severity::Hint);
                    let span = Span {
                        start: 0,
                        end: None,
                        fg: look.fg.map(|t| on_editor(t.of(c), c)),
                        bg: look.bg.map(|t| t.of(c)),
                        wave: look.squiggle.filter(|s| *s != Severity::Hint).map(|s| squiggle_color(s, c)),
                        under: look.underline.map(|t| t.of(c)).or(hint.map(|s| squiggle_color(s, c))),
                        strike: look.strike.then(|| look.fg.map(|t| t.of(c)).unwrap_or(c.editor_fg)),
                        hover: hover.clone(),
                    };
                    for line in from.0..=to.0 {
                        let start = if line == from.0 { from.1 } else { 0 };
                        let end = (line == to.0).then_some(to.1);
                        // A range that ends at the start of a line is not on it.
                        if line > from.0 && end == Some(0) {
                            continue;
                        }
                        m.lines.entry(line).or_default().spans.push(Span { start, end, ..span.clone() });
                    }
                    if let Some(color) = look.squiggle.and_then(|s| ruler_color(s, c)) {
                        ruler.push((from.0, color));
                    }
                }
                Mark::Text { line, col, text, at } => {
                    let color = match at {
                        Phantom::Inline => c.inlay_fg,
                        Phantom::EndOfLine(Some(s)) => squiggle_color(*s, c),
                        Phantom::EndOfLine(None) => c.desc,
                        Phantom::Ghost => c.ghost_text,
                    };
                    m.lines.entry(*line).or_default().texts.push((*col, text.clone(), *at, on_editor(color, c)));
                }
                Mark::Ruler { line, tint } => ruler.push((*line, tint.of(c))),
            }
        }
    }
    for marks in m.lines.values_mut() {
        marks.texts.sort_by_key(|t| t.0);
    }
    ruler.sort_by_key(|r| r.0);
    ruler.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    m.ruler = ruler;
    m
}

/// A file's marks: each owner's, and them merged.
struct File {
    owners: BTreeMap<String, Rc<[Mark]>>,
    merged: Rc<Merged>,
}

thread_local! {
    static FILES: RefCell<HashMap<String, File>> = RefCell::new(HashMap::new());
}

/// Counts every change of the marks, for what draws them outside the editor's text (the margin, the ruler).
#[derive(Clone, Copy)]
struct Decor {
    rev: RwSignal<u64>,
}

impl crate::ext::State for Decor {
    fn new(ui: Ui) -> Self {
        Decor { rev: ui.scope.create_rw_signal(0) }
    }
}

/// The hover shown: what the ranges under the pointer say, and where in the window (under their line).
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    texts: Vec<String>,
    at: Point,
}

#[derive(Clone, Copy)]
struct Hover {
    shown: RwSignal<Option<Shown>>,
    /// The pointer's latest stop: a hover waits a moment and shows only if the pointer has not moved on.
    asked: RwSignal<u64>,
}

impl crate::ext::State for Hover {
    fn new(ui: Ui) -> Self {
        Hover { shown: ui.scope.create_rw_signal(None), asked: ui.scope.create_rw_signal(0) }
    }
}

impl Hover {
    fn hide(self) {
        self.asked.update(|n| *n += 1);
        if self.shown.with_untracked(Option::is_some) {
            self.shown.set(None);
        }
    }

    /// VS Code's hover delay (`editor.hover.delay`).
    fn ask(self, wanted: Shown) {
        if self.shown.with_untracked(|s| s.as_ref().is_some_and(|s| s.texts == wanted.texts)) {
            return;
        }
        self.asked.update(|n| *n += 1);
        let n = self.asked.get_untracked();
        floem::action::exec_after(std::time::Duration::from_millis(300), move |_| {
            if self.asked.try_get_untracked() == Some(n) {
                self.shown.set(Some(wanted));
            }
        });
    }
}

/// The hover under a point over an editor (`local` in the editor's own view, `win` in the window): the glyph's title
/// in the glyph margin, the ranges' hovers over the text; none there: hidden.
fn hover_over(ui: Ui, editor: &Editor, path: &str, local: Point, win: Point) {
    let hover = ui.state::<Hover>();
    let (o, v) = (editor.window_origin.get_untracked(), editor.viewport.get_untracked());
    let at = Point::new(win.x - o.x + v.x0, win.y - o.y + v.y0);
    let ((line, col), inside) = editor.line_col_of_point(Mode::Insert, at);
    let texts = match of(path) {
        None => Vec::new(),
        Some(m) if local.x < MARGIN => line_at(editor, local.y)
            .and_then(|line| m.lines.get(&line).and_then(|l| l.glyph.as_ref()))
            .map(|g| vec![g.title.clone()])
            .filter(|t| !t[0].is_empty())
            .unwrap_or_default(),
        Some(_) if win.x >= o.x && inside => hover_at(path, line, col),
        Some(_) => Vec::new(),
    };
    if texts.is_empty() {
        return hover.hide();
    }
    let height = editor.line_height(line) as f64;
    let below = o.y + ((at.y / height).floor() + 1.0) * height - v.y0;
    hover.ask(Shown { texts, at: Point::new(win.x, below + 2.0) });
}

/// The hover's box, as VS Code's: its colours, its border, at most 500 px wide.
fn hover_view(ui: Ui) -> floem::AnyView {
    let hover = ui.state::<Hover>();
    let t = ui.colors;
    floem::views::dyn_container(
        move || hover.shown.get().map(|s| s.texts),
        move |texts| {
            let c = t.get_untracked();
            let lines = texts.unwrap_or_default().into_iter().map(|text| floem::views::label(move || text.clone()));
            floem::views::v_stack_from_iter(lines)
                .style(move |s| {
                    s.padding_vert(4.0)
                        .padding_horiz(8.0)
                        .row_gap(4.0)
                        .max_width(500.0)
                        .font_size(13.0)
                        .color(c.hover_fg)
                        .background(c.hover_bg)
                        .border(1.0)
                        .border_color(c.hover_border)
                        .border_radius(3.0)
                })
                .into_any()
        },
    )
    .style(move |s| match hover.shown.get() {
        None => s.hide(),
        Some(h) => s.absolute().inset_left(h.at.x).inset_top(h.at.y),
    })
    .into_any()
}

/// An owner's marks for a file, in place of those it gave before (none: it has none there now).
pub fn set(ui: Ui, owner: &str, path: &str, marks: Vec<Mark>) {
    let c = ui.colors.get_untracked();
    let changed = FILES.with(|f| {
        let mut files = f.borrow_mut();
        if marks.is_empty() && !files.get(path).is_some_and(|file| file.owners.contains_key(owner)) {
            return false;
        }
        let file = files
            .entry(path.to_string())
            .or_insert_with(|| File { owners: BTreeMap::new(), merged: Rc::new(Merged::default()) });
        if marks.is_empty() {
            file.owners.remove(owner);
        } else {
            file.owners.insert(owner.to_string(), marks.into());
        }
        file.merged = Rc::new(merge(file.owners.values().map(|m| &m[..]), &c));
        if file.owners.is_empty() {
            files.remove(path);
        }
        true
    });
    if changed {
        redraw(ui, path);
        ui.state::<Decor>().rev.update(|r| *r += 1);
    }
}

/// An owner's marks for a file taken away.
pub fn clear(ui: Ui, owner: &str, path: &str) {
    set(ui, owner, path, Vec::new());
}

/// A file's marks, merged (none: it has none).
pub fn of(path: &str) -> Option<Rc<Merged>> {
    if path.is_empty() {
        return None;
    }
    FILES.with(|f| f.borrow().get(path).map(|file| file.merged.clone()))
}

/// What the hovers of the ranges at a line and column say, in the owners' order.
pub fn hover_at(path: &str, line: usize, col: usize) -> Vec<String> {
    let Some(m) = of(path) else { return Vec::new() };
    let Some(marks) = m.lines.get(&line) else { return Vec::new() };
    marks
        .spans
        .iter()
        .filter(|s| s.start <= col && s.end.is_none_or(|e| col < e))
        .filter_map(|s| s.hover.clone())
        .collect()
}

/// The editor of the file lays out its lines again (the text's revision is what its layouts are kept by).
fn redraw(ui: Ui, path: &str) {
    if let Some(tab) = ui.tab(path) {
        tab.doc.cache_rev().update(|r| *r += 1);
    }
}

/// The text that is not in the file on a line `len` bytes long, by column, as the editor shows it.
pub fn phantoms(path: &str, line: usize, len: usize) -> Vec<PhantomText> {
    let Some(m) = of(path) else { return Vec::new() };
    let Some(marks) = m.lines.get(&line) else { return Vec::new() };
    let mut texts: Vec<PhantomText> = marks
        .texts
        .iter()
        .map(|(col, text, at, color)| {
            let (kind, col, text, affinity) = match at {
                Phantom::Inline => (PhantomTextKind::InlayHint, (*col).min(len), text.clone(), None),
                // VS Code leaves a gap after the line's text.
                Phantom::EndOfLine(_) => (PhantomTextKind::Diagnostic, len, format!("    {text}"), None),
                Phantom::Ghost => {
                    (PhantomTextKind::Completion, (*col).min(len), text.clone(), Some(CursorAffinity::Backward))
                }
            };
            PhantomText { kind, col, affinity, text, font_size: None, fg: Some(*color), bg: None, under_line: None }
        })
        .collect();
    texts.sort_by_key(|t| (t.col, t.kind == PhantomTextKind::Diagnostic));
    texts
}

/// A line's phantom text: the document's own (a placeholder, what an input method is composing) and the marks'.
pub fn phantom_line(doc: &TextDocument, path: &str, edid: EditorId, es: &EditorStyle, line: usize) -> PhantomTextLine {
    let mut p = doc.phantom_text(edid, es, line);
    let more = phantoms(path, line, doc.rope_text().line_end_col(line, true));
    if !more.is_empty() {
        p.text.extend(more);
        // The editor places them in this order.
        p.text.sort_by_key(|t| (t.col, t.kind == PhantomTextKind::Diagnostic));
    }
    p
}

/// A line's syntax runs moved past the phantom text before them, then the ranges' text colours: the attrs index the
/// line as drawn, with its phantom text in it.
pub fn attrs(
    path: &str,
    line: usize,
    phantom: &PhantomTextLine,
    runs: Vec<(std::ops::Range<usize>, Attrs)>,
    default: Attrs,
    list: &mut AttrsList,
) {
    let at = |col: usize| if phantom.text.is_empty() { col } else { phantom.col_at(col) };
    for (range, a) in runs {
        list.add_span(at(range.start)..at(range.end), a);
    }
    let Some(m) = of(path) else { return };
    let Some(marks) = m.lines.get(&line) else { return };
    for s in &marks.spans {
        if let Some(fg) = s.fg {
            let end = s.end.unwrap_or(usize::MAX / 2);
            list.add_span(at(s.start)..at(end.max(s.start)), default.color(fg));
        }
    }
}

/// A line's backgrounds, squiggles, underlines and lines through, where its text (`len` bytes) is laid out.
pub fn layout(path: &str, line: usize, len: usize, line_height: f64, l: &mut TextLayoutLine) {
    let Some(m) = of(path) else { return };
    let Some(marks) = m.lines.get(&line) else { return };
    if let Some(bg) = marks.bg {
        l.extra_style.push(LineExtraStyle {
            x: 0.0,
            y: 0.0,
            width: None,
            height: line_height,
            bg_color: Some(bg),
            under_line: None,
            wave_line: None,
        });
    }
    // Columns past the line's own text (marks said before an edit) stop at its end.
    let shown = |col: usize| l.phantom_text.col_at(col.min(len));
    for s in &marks.spans {
        let x0 = l.text.hit_position(shown(s.start)).point.x;
        let mut x1 = l.text.hit_position(shown(s.end.unwrap_or(len))).point.x;
        // A range with no width (a problem at a line's end) still shows a little.
        if x1 - x0 < 1.0 {
            x1 = x0 + 6.0;
        }
        let piece = |height: f64| LineExtraStyle {
            x: x0,
            y: 0.0,
            width: Some(x1 - x0),
            height,
            bg_color: None,
            under_line: None,
            wave_line: None,
        };
        if let Some(bg) = s.bg {
            l.extra_style.push(LineExtraStyle { bg_color: Some(bg), ..piece(line_height) });
        }
        if let Some(wave) = s.wave {
            l.extra_style.push(LineExtraStyle { wave_line: Some(wave), ..piece(line_height - 4.0) });
        }
        if let Some(under) = s.under {
            l.extra_style.push(LineExtraStyle { under_line: Some(under), ..piece(line_height - 2.0) });
        }
        if let Some(strike) = s.strike {
            l.extra_style.push(LineExtraStyle { under_line: Some(strike), ..piece((line_height / 2.0).round()) });
        }
    }
}

/// A file's text with the marks' phantom text: what the editor is given in place of the document itself.
pub struct Doc {
    inner: Rc<TextDocument>,
    path: String,
}

/// The document an editor of a file is drawn from.
pub fn doc(inner: Rc<TextDocument>, path: &str) -> Rc<Doc> {
    Rc::new(Doc { inner, path: path.to_string() })
}

impl Document for Doc {
    fn text(&self) -> Rope {
        self.inner.text()
    }

    fn rope_text(&self) -> RopeTextVal {
        self.inner.rope_text()
    }

    fn cache_rev(&self) -> RwSignal<u64> {
        self.inner.cache_rev()
    }

    fn find_unmatched(&self, offset: usize, previous: bool, ch: char) -> usize {
        self.inner.find_unmatched(offset, previous, ch)
    }

    fn find_matching_pair(&self, offset: usize) -> usize {
        self.inner.find_matching_pair(offset)
    }

    fn preedit(&self) -> PreeditData {
        self.inner.preedit()
    }

    fn preedit_phantom(&self, under_line: Option<Color>, line: usize) -> Option<PhantomText> {
        self.inner.preedit_phantom(under_line, line)
    }

    fn compute_screen_lines(&self, editor: &Editor, base: RwSignal<ScreenLinesBase>) -> ScreenLines {
        // With folds, the rows the fold map puts the lines on (`editor::folding`).
        crate::editor::folding::screen_lines(editor, base, &self.path)
            .unwrap_or_else(|| self.inner.compute_screen_lines(editor, base))
    }

    fn run_command(&self, ed: &Editor, cmd: &Command, count: Option<usize>, modifiers: Modifiers) -> CommandExecuted {
        self.inner.run_command(ed, cmd, count, modifiers)
    }

    fn receive_char(&self, ed: &Editor, c: &str) {
        self.inner.receive_char(ed, c)
    }

    fn edit_single(&self, selection: Selection, content: &str, edit_type: EditType) {
        self.inner.edit_single(selection, content, edit_type)
    }

    fn edit(&self, iter: &mut dyn Iterator<Item = (Selection, &str)>, edit_type: EditType) {
        self.inner.edit(iter, edit_type)
    }
}

impl DocumentPhantom for Doc {
    fn phantom_text(&self, edid: EditorId, styling: &EditorStyle, line: usize) -> PhantomTextLine {
        phantom_line(&self.inner, &self.path, edid, styling, line)
    }

    fn has_multiline_phantom(&self, edid: EditorId, styling: &EditorStyle) -> bool {
        self.inner.has_multiline_phantom(edid, styling)
    }
}

/// The glyph margin's width, VS Code's.
pub const MARGIN: f64 = 18.0;
/// The overview ruler's width, VS Code's (that of the scroll bar it is drawn in).
pub const RULER: f64 = 14.0;

/// An editor of a file with its glyph margin (left, over the gutter's padding) and its overview ruler (right, under
/// the editor, which is drawn without a background so the ruler shows through and its scroll bar is still dragged):
/// the pointer goes to the topmost view only. Only painted again when the lines on screen, the scroll, the marks or
/// the theme change.
pub fn around<V: IntoView + 'static>(ui: Ui, view: V, editor: &Editor, path: &str) -> impl IntoView + use<V> {
    let margin = Margin { id: ViewId::new(), ui, editor: editor.clone(), path: path.to_string(), ruler: false };
    let ruler = Margin { id: ViewId::new(), ui, editor: editor.clone(), path: path.to_string(), ruler: true };
    let (screen, viewport, rev, colors) = (editor.screen_lines, editor.viewport, ui.state::<Decor>().rev, ui.colors);
    for id in [margin.id, ruler.id] {
        floem::reactive::create_effect(move |_| {
            screen.track();
            viewport.track();
            rev.track();
            colors.track();
            id.request_paint();
        });
    }
    // Scrolled, or the caret moved (a key pressed): the hover goes, as VS Code's.
    let (hover, cursor) = (ui.state::<Hover>(), editor.cursor);
    floem::reactive::create_effect(move |_| {
        viewport.track();
        cursor.track();
        hover.hide();
    });
    let origin = floem::reactive::create_rw_signal(Point::ZERO);
    let (hovered, hover_path) = (editor.clone(), path.to_string());
    let shown = editor.clone();
    let path = path.to_string();
    let margin = margin
        .on_event(EventListener::PointerDown, move |e| {
            let Event::PointerDown(p) = e else { return EventPropagation::Continue };
            let Some(line) = line_at(&shown, p.pos.y) else { return EventPropagation::Continue };
            let glyph = of(&path).and_then(|m| m.lines.get(&line).and_then(|l| l.glyph.clone()));
            match (glyph.and_then(|g| g.click), ui.out.get_untracked()) {
                (Some(click), Some(send)) => {
                    click(ui, &send, line);
                    EventPropagation::Stop
                }
                _ => EventPropagation::Continue,
            }
        })
        .style(|s| s.absolute().inset_left(0.0).inset_top(0.0).width(MARGIN).height_full());
    let ruler = ruler.style(|s| s.absolute().inset_right(0.0).inset_top(0.0).width(RULER).height_full());
    floem::views::stack((ruler, view.into_view().style(|s| s.size_full()), margin))
        .on_move(move |p| origin.set(p))
        .on_event_cont(EventListener::PointerMove, move |e| {
            if let Event::PointerMove(p) = e {
                hover_over(ui, &hovered, &hover_path, p.pos, p.pos + origin.get_untracked().to_vec2());
            }
        })
        .on_event_cont(EventListener::PointerLeave, move |_| hover.hide())
        // The editor gone (its tab closed or another shown): its hover with it.
        .on_cleanup(move || hover.hide())
        .style(|s| s.size_full())
}

/// The line drawn at a height in the editor's view (none: below the last).
fn line_at(editor: &Editor, y: f64) -> Option<usize> {
    let top = editor.viewport.get_untracked().y0;
    let height = editor.line_height(0) as f64;
    editor
        .screen_lines
        .with_untracked(|s| s.iter_lines_y().find(|(_, at)| (at - top..at - top + height).contains(&y)).map(|l| l.0))
}

/// The glyph margin or the overview ruler of an editor, painted from the file's marks.
struct Margin {
    id: ViewId,
    ui: Ui,
    editor: Editor,
    path: String,
    ruler: bool,
}

impl View for Margin {
    fn id(&self) -> ViewId {
        self.id
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let Some(size) = self.id.get_size() else { return };
        let Some(m) = of(&self.path) else { return };
        let c = self.ui.colors.get_untracked();
        if self.ruler {
            // VS Code's ruler: its left border, and each mark where its line is in the whole file.
            if m.ruler.is_empty() {
                return;
            }
            cx.fill(&Rect::new(0.0, 0.0, 1.0, size.height), on_editor(c.ruler_border, &c), 0.0);
            let lines = (self.editor.last_line() + 1) as f64;
            for (line, color) in &m.ruler {
                let y = ((*line as f64 / lines) * size.height).min(size.height - 3.0).max(0.0);
                cx.fill(&Rect::new(1.0, y, size.width, y + 3.0), *color, 0.0);
            }
            return;
        }
        let top = self.editor.viewport.get_untracked().y0;
        let height = self.editor.line_height(0) as f64;
        let family: Vec<FamilyOwned> = FamilyOwned::parse_list("codicon").collect();
        self.editor.screen_lines.with_untracked(|s| {
            for (line, y) in s.iter_lines_y() {
                let Some(g) = m.lines.get(&line).and_then(|l| l.glyph.as_ref()) else { continue };
                let attrs =
                    Attrs::new().family(&family).font_size(16.0).line_height(LineHeightValue::Px(16.0)).color(g.color);
                let mut layout = TextLayout::new();
                layout.set_text(&crate::icons::char_of(g.icon).to_string(), AttrsList::new(attrs));
                cx.draw_text(&layout, Point::new(1.0, y - top + (height - 16.0) / 2.0));
            }
        });
    }
}

/// What it adds to the window's registries, once at start: the hover's layer; and every file's marks take the theme's colours again when
/// it changes.
pub fn register(ui: Ui, _send: &Send) {
    crate::layers::add(crate::layers::Place::Over, "editor.hover", 20, |ui, _| hover_view(ui));
    let colors = ui.colors;
    ui.scope.create_effect(move |_| {
        let c = colors.get();
        let paths = FILES.with(|f| {
            let mut files = f.borrow_mut();
            for file in files.values_mut() {
                file.merged = Rc::new(merge(file.owners.values().map(|m| &m[..]), &c));
            }
            files.keys().cloned().collect::<Vec<_>>()
        });
        for path in paths {
            redraw(ui, &path);
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

    fn colors() -> Colors {
        Ui::new().colors.get_untracked()
    }

    #[test]
    fn a_range_over_lines_is_cut_into_each_lines_piece_and_a_squiggle_marks_the_ruler() {
        let c = colors();
        let marks = [Mark::squiggle((1, 4), (3, 2), Severity::Error, "expected `;`")];
        let m = merge([&marks[..]], &c);
        let spans: Vec<_> = (1..=3).map(|l| (m.lines[&l].spans[0].start, m.lines[&l].spans[0].end)).collect();
        assert_eq!(spans, [(4, None), (0, None), (0, Some(2))]);
        assert_eq!(m.lines[&1].spans[0].wave, Some(c.problem_error));
        assert_eq!(m.ruler, [(1, c.ruler_error)]);
        // Ending at a line's start: not on that line.
        let m = merge([&[Mark::squiggle((0, 0), (1, 0), Severity::Warning, "")][..]], &c);
        assert!(!m.lines.contains_key(&1) && m.lines[&0].spans[0].hover.is_none());
        assert_eq!(m.lines[&0].spans[0].wave, Some(c.warning));
        // A hint is an underline, and not in the ruler.
        let m = merge([&[Mark::squiggle((2, 1), (2, 5), Severity::Hint, "unused")][..]], &c);
        assert_eq!((m.lines[&2].spans[0].wave, m.lines[&2].spans[0].under), (None, Some(c.problem_hint)));
        assert!(m.ruler.is_empty());
    }

    #[test]
    fn the_first_owners_glyph_is_the_lines_and_the_theme_colours_it() {
        let c = colors();
        let first = [Mark::Glyph {
            line: 2,
            icon: "circle-filled",
            tint: Tint::Theme(|c| c.problem_error),
            title: "Breakpoint".into(),
            click: None,
        }];
        let second = [
            Mark::Glyph {
                line: 2,
                icon: "lightbulb",
                tint: Tint::Fixed(Color::WHITE),
                title: "Fix".into(),
                click: None,
            },
            Mark::Line { line: 2, bg: Tint::Fixed(Color::BLACK) },
            Mark::Ruler { line: 7, tint: Tint::Fixed(Color::WHITE) },
            Mark::Ruler { line: 7, tint: Tint::Fixed(Color::WHITE) },
        ];
        let m = merge([&first[..], &second[..]], &c);
        let g = m.lines[&2].glyph.as_ref().expect("a glyph");
        assert_eq!((g.icon, g.color, g.title.as_str()), ("circle-filled", c.problem_error, "Breakpoint"));
        assert_eq!(m.lines[&2].bg, Some(Color::BLACK));
        assert_eq!(m.ruler, [(7, Color::WHITE)]);
        // Ghost text is see-through: laid on the editor's background, as the text is drawn without alpha.
        let m = merge([&[Mark::Text { line: 0, col: 0, text: "x".into(), at: Phantom::Ghost }][..]], &c);
        let ghost = m.lines[&0].texts[0].3;
        assert_eq!(ghost.a, 255);
        assert!(ghost.r > c.editor_bg.r && ghost.r < c.ghost_text.r);
    }

    #[test]
    fn phantom_text_is_where_it_is_said_by_column_and_hovers_are_found_by_column() {
        let ui = Ui::new();
        let path = "decor-test/a.rs";
        set(
            ui,
            "test",
            path,
            vec![
                Mark::Text { line: 0, col: 8, text: "ghost".into(), at: Phantom::Ghost },
                Mark::Text { line: 0, col: 3, text: ": i32".into(), at: Phantom::Inline },
                Mark::Text { line: 0, col: 0, text: "1 error".into(), at: Phantom::EndOfLine(Some(Severity::Error)) },
                Mark::squiggle((0, 2), (0, 5), Severity::Error, "bad"),
            ],
        );
        let p = phantoms(path, 0, 6);
        let got: Vec<_> = p.iter().map(|t| (t.col, t.text.as_str(), t.kind)).collect();
        assert_eq!(
            got,
            [
                (3, ": i32", PhantomTextKind::InlayHint),
                // Past the line's end: at its end.
                (6, "ghost", PhantomTextKind::Completion),
                (6, "    1 error", PhantomTextKind::Diagnostic),
            ]
        );
        assert_eq!(p[1].affinity, Some(CursorAffinity::Backward));
        assert_eq!(hover_at(path, 0, 2), ["bad"]);
        assert!(hover_at(path, 0, 5).is_empty() && hover_at(path, 1, 2).is_empty());
        clear(ui, "test", path);
        assert!(of(path).is_none() && phantoms(path, 0, 6).is_empty());
    }

    #[test]
    fn an_open_files_editor_lays_out_again_when_its_marks_change_and_the_theme_colours_them_again() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        register(ui, &send);
        let path = "decor-test/c.rs";
        ui.take(&serde_json::json!({"type": "file", "path": path, "text": "fn a() {}\n", "modified": 1}), &send);
        let rev = ui.tab(path).expect("open").doc.cache_rev();
        let at = rev.get_untracked();
        let glyph = Mark::Glyph {
            line: 0,
            icon: "circle-filled",
            tint: Tint::Theme(|c| c.problem_error),
            title: String::new(),
            click: None,
        };
        set(ui, "test", path, vec![glyph]);
        assert_eq!(rev.get_untracked(), at + 1);
        // Another theme: the mark takes its colour, and the editor lays out again.
        let mut c = ui.colors.get_untracked();
        c.problem_error = Color::rgb8(1, 2, 3);
        ui.colors.set(c);
        assert_eq!(of(path).expect("marks").lines[&0].glyph.as_ref().map(|g| g.color), Some(Color::rgb8(1, 2, 3)));
        assert_eq!(rev.get_untracked(), at + 2);
        clear(ui, "test", path);
        // Nothing left to take away: nothing is laid out again.
        clear(ui, "test", path);
        assert_eq!(rev.get_untracked(), at + 3);
    }

    #[test]
    fn the_syntax_runs_move_past_phantom_text_and_a_ranges_colour_is_laid_on() {
        let ui = Ui::new();
        let path = "decor-test/b.rs";
        let look = Look { fg: Some(Tint::Fixed(Color::WHITE)), ..Look::default() };
        set(ui, "test", path, vec![Mark::Range { from: (0, 0), to: (0, 2), look, hover: None }]);
        let hint = PhantomText {
            kind: PhantomTextKind::InlayHint,
            col: 1,
            affinity: None,
            text: "ab".into(),
            font_size: None,
            fg: None,
            bg: None,
            under_line: None,
        };
        let phantom = PhantomTextLine { text: [hint].into_iter().collect() };
        let default = Attrs::new();
        let mut list = AttrsList::new(default);
        attrs(path, 0, &phantom, vec![(2..4, default.color(Color::BLACK))], default, &mut list);
        // The run at 2..4 is drawn at 4..6, past the two bytes put in at 1; the range 0..2 is drawn at 0..4.
        // Only the colour is read back (a span's font size is not kept in the list).
        let color = |a: Attrs| format!("{a:?}").split(", family").next().unwrap_or_default().to_string();
        let span = |i: usize| color(list.get_span(i));
        let (black, white) = (color(default.color(Color::BLACK)), color(default.color(Color::WHITE)));
        assert!(span(4) == black && span(5) == black && span(6) == color(default));
        assert!(span(0) == white && span(3) == white);
        clear(ui, "test", path);
    }
}
