//! The minimap and the sticky scroll of a file's editor, as VS Code's.
//!
//! The minimap (`editor.minimap.enabled`) is a strip beside the text, left of the overview ruler (or at the editor's
//! left with `editor.minimap.side` "left"), with each line drawn small in its own colours: each run of characters
//! that are not white space a block, a minimap line 3 px high (2 px with `editor.minimap.renderCharacters`), as
//! Monaco's block renderer draws them. It is at most 120 columns wide and has VS Code's 8 px gutter at its left, where
//! the overview ruler's marks (find, problems, quick diff, conflicts) are drawn on their lines. While the pointer is
//! over it, the slider shows the lines on screen; a click scrolls there, the slider is dragged, and the wheel scrolls
//! the editor. A file longer than the minimap scrolls it as Monaco does (the slider moves from the top to the bottom
//! as the editor scrolls from its first line to its last). The text under a right-side minimap can be scrolled out:
//! the editor is made that much wider (`room`).
//!
//! Each line's blocks are worked out once and kept (`Colours::minimap`) until an edit reaches the line; a paint works
//! out new ones for 4 ms at most, draws the ones it kept from before the edit for the rest, and paints again a frame
//! later until all are new.
//!
//! The sticky scroll (`editor.stickyScroll.enabled`) pins the first lines of the scopes the top line is in at the top
//! of the editor, at most five, with their line numbers, the outer first: the outline's symbols (a function, a type,
//! a heading) when the Outline has read the file, else every indented region. A scope's end pushes its line up as it
//! comes near. A click on a pinned line puts the caret on it and scrolls it to where it is pinned; the pointer over
//! one gives it `editorStickyScrollHover.background`.
//!
//! What every part has (`register`, `take`) is said in `ext.rs`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use floem::context::PaintCx;
use floem::event::{Event, EventListener, EventPropagation};
use floem::kurbo::{Point, Rect, Vec2};
use floem::peniko::Color;
use floem::reactive::{
    Memo, RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_memo, create_rw_signal,
};
use floem::text::{Attrs, AttrsList, LineHeightValue, TextLayout};
use floem::views::Decorators;
use floem::views::editor::Editor;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::cursor::CursorMode;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::id::EditorId;
use floem::views::editor::text::Styling;
use floem::{AnyView, IntoView, View, ViewId};
use floem_renderer::Renderer;
use serde_json::Value;

use crate::highlight::Colours;
use crate::state::{Send, Ui};

/// The most columns the minimap draws (VS Code's `editor.minimap.maxColumn`).
pub const COLUMNS: usize = 120;
/// The minimap's gutter, left of its text, where the marks of lines are drawn (Monaco's `MINIMAP_GUTTER_WIDTH`).
pub const GUTTER: f64 = 8.0;
/// The most lines the sticky scroll pins (VS Code's `editor.stickyScroll.maxLineCount`).
pub const PINNED: usize = 5;
/// How long a paint works out new minimap lines before it draws the kept ones.
const BUDGET: Duration = Duration::from_millis(4);

// ==== The minimap's sums ====

/// Where the minimap is at a scroll, as Monaco's `MinimapLayout`: the row drawn at its top (with the part of it
/// scrolled over), the slider's top and height, how far the slider moves for a pixel scrolled, and whether there
/// is a slider at all (the editor scrolls).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub first: f64,
    pub slider_top: f64,
    pub slider_height: f64,
    pub ratio: f64,
    pub needed: bool,
}

/// The minimap's frame, as Monaco works it out: `rows` minimap lines of `mini` px, beside an editor of `line` px
/// lines scrolled `top` of `scroll_height`, both `height` high; `beyond` when the editor scrolls past its last line.
pub fn frame(rows: usize, line: f64, mini: f64, height: f64, top: f64, scroll_height: f64, beyond: bool) -> Frame {
    let (line, mini) = (line.max(1.0), mini.max(1.0));
    let lines = rows as f64;
    let shown = height / line;
    let slider_height = (shown * mini).floor().max(1.0);
    let below = if beyond { (shown - 1.0).max(0.0) } else { 0.0 };
    let most = if below > 0.0 {
        // Dragged to the bottom, the slider has the last line at its top.
        (lines + below - shown - 1.0) * mini
    } else {
        (lines * mini - slider_height).max(0.0)
    };
    let most = most.min(height - slider_height).max(0.0);
    let ratio = if scroll_height > height { most / (scroll_height - height) } else { 0.0 };
    let slider_top = top * ratio;
    let fits = (height / mini).floor() >= lines + below;
    let first = if fits { 0.0 } else { (top / line - slider_top / mini).max(0.0) };
    Frame { first, slider_top, slider_height, ratio, needed: most > 0.0 }
}

impl Frame {
    /// Where the editor is scrolled to for the slider dragged `dy` from where it was when the editor was at `top`.
    pub fn dragged(&self, top: f64, dy: f64) -> f64 {
        if self.ratio > 0.0 { (top + dy / self.ratio).max(0.0) } else { top }
    }

    /// Where the editor is scrolled to for a click at `y`: the slider's middle there, as Monaco's.
    pub fn clicked(&self, y: f64) -> f64 {
        if self.ratio > 0.0 { ((y - self.slider_height / 2.0) / self.ratio).max(0.0) } else { 0.0 }
    }

    /// Whether `y` is on the slider.
    pub fn on_slider(&self, y: f64) -> bool {
        (self.slider_top..self.slider_top + self.slider_height).contains(&y)
    }
}

/// The minimap's width beside text `width` wide in characters `char_width` wide, as Monaco's proportional size: a
/// part of the width as one character of the minimap is to one of the text and the gutter, at most 120 columns.
pub fn width(width: f64, char_width: f64) -> f64 {
    let room = ((width - crate::decor::RULER - 2.0).max(0.0) / (char_width.max(1.0) + 1.0)).floor() + GUTTER;
    room.min(COLUMNS as f64)
}

/// A colour laid over another at `alpha`, as Monaco's minimap lays its characters on its background.
fn blend(fg: Color, bg: Color, alpha: f64) -> Color {
    let mix = |f: u8, b: u8| (f64::from(b) + (f64::from(f) - f64::from(b)) * alpha).round() as u8;
    Color::rgb8(mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b))
}

// ==== The sticky scroll's sums ====

/// A line pinned at the top of the editor: the line, and its top from the editor's top (above its place while the
/// end of its scope pushes it up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pin {
    pub line: usize,
    pub top: f64,
}

/// The scopes the sticky scroll pins, (first line, last line) by their first line: with the outline's symbols
/// (their lines from 0, their depth), those regions that start at a symbol's line, and for a symbol with no region
/// (a heading) the lines to the next symbol as deep or less; with none, every region.
pub fn scopes(regions: &[(usize, usize)], symbols: Option<&[(usize, usize)]>, last: usize) -> Vec<(usize, usize)> {
    let Some(symbols) = symbols.filter(|s| !s.is_empty()) else { return regions.to_vec() };
    let mut out: Vec<(usize, usize)> = symbols
        .iter()
        .enumerate()
        .filter_map(|(i, &(line, depth))| {
            if let Some(r) = regions.iter().find(|r| r.0 == line) {
                return Some(*r);
            }
            let end = symbols[i + 1..].iter().find(|s| s.1 <= depth).map_or(last, |s| s.0.saturating_sub(1));
            (end > line).then_some((line, end))
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The lines pinned, the outer first: each the start of a scope the line just under the ones pinned before is in
/// (and starts above it), at most `most`. `line_at(y)` is the line drawn `y` below the editor's top; `bottom_of`
/// is how far below the top a line on screen ends, by which a scope that ends pushes its line up.
pub fn pinned(
    scopes: &[(usize, usize)],
    line_height: f64,
    most: usize,
    line_at: impl Fn(f64) -> Option<usize>,
    bottom_of: impl Fn(usize) -> Option<f64>,
) -> Vec<Pin> {
    let mut out: Vec<Pin> = Vec::new();
    while out.len() < most {
        let k = out.len();
        let Some(probe) = line_at(k as f64 * line_height) else { break };
        let chain: Vec<(usize, usize)> = scopes.iter().copied().filter(|s| s.0 < probe && probe <= s.1).collect();
        // The scopes pinned are still the outer ones of this line (else the line under them is in another).
        if chain.len() <= k || chain.iter().zip(&out).any(|(s, p)| s.0 != p.line) {
            break;
        }
        let (start, end) = chain[k];
        let mut top = k as f64 * line_height;
        if let Some(bottom) = bottom_of(end) {
            top = top.min(bottom - line_height);
        }
        out.push(Pin { line: start, top });
    }
    out
}

/// A region that ends right before its closing bracket (at its first line's indentation) ends on it, as a symbol's
/// range does: its first line stays pinned while the bracket is under it.
fn with_closer(
    text: &floem::views::editor::core::buffer::rope_text::RopeTextVal,
    (start, end): (usize, usize),
) -> (usize, usize) {
    if end + 1 > text.last_line() {
        return (start, end);
    }
    let first = text.line_content(start);
    let next = text.line_content(end + 1);
    let indent = |s: &str| s.len() - s.trim_start().len();
    let closes = next.trim_start().starts_with(['}', ')', ']']);
    if closes && indent(&first) == indent(&next) { (start, end + 1) } else { (start, end) }
}

/// Symbols by their line (from 0) and depth.
type Lines = Rc<Vec<(usize, usize)>>;

// ==== The room under the minimap ====

thread_local! {
    /// The width the minimap covers at the right of each editor's text.
    static ROOM: RefCell<HashMap<EditorId, RwSignal<f64>>> = RefCell::new(HashMap::new());
}

fn room_of(editor: &Editor) -> RwSignal<f64> {
    let id = editor.id();
    if let Some(s) = ROOM.with(|r| r.borrow().get(&id).copied()).filter(|s| s.try_get_untracked().is_some()) {
        return s;
    }
    let s = editor.cx.get().create_rw_signal(0.0);
    ROOM.with(|r| {
        let mut r = r.borrow_mut();
        // Those of editors gone (a compare's, which has no minimap) are let go.
        r.retain(|_, s| s.try_get_untracked().is_some());
        r.insert(id, s);
    });
    s
}

/// The width a right-side minimap covers at the right of an editor's text (0 without one), followed: the text is
/// made that much wider, so its end can be scrolled out from under it, and the caret is kept left of it.
pub fn room(editor: &Editor) -> f64 {
    room_of(editor).try_get().unwrap_or(0.0)
}

// ==== The views ====

/// The rows the editor draws (by the folds) and the minimap's rows, and the height scrolled over.
fn rows(editor: &Editor, path: &str) -> (Option<Rc<crate::editor::folding::FoldMap>>, usize, f64) {
    let map = crate::editor::folding::followed(path, editor);
    let lines = editor.last_line() + 1;
    let line = f64::from(editor.line_height(0));
    let drawn = match &map {
        Some(m) => m.rows(lines),
        None => editor.last_vline().get() + 1,
    } as f64
        * line;
    let vp = editor.viewport.get_untracked();
    let beyond = if beyond(editor) { (vp.height().min(drawn) - line).max(0.0) } else { 0.0 };
    let rows = map.as_ref().map_or(lines, |m| m.rows(lines));
    (map, rows, drawn.max(vp.height()) + beyond)
}

fn beyond(editor: &Editor) -> bool {
    editor.es.with_untracked(|s| s.scroll_beyond_last_line())
}

/// An editor with its minimap and its sticky scroll over it (`view` is the editor with its margin and ruler,
/// `decor::around`).
pub fn around(ui: Ui, view: impl IntoView + 'static, editor: &Editor, colours: Rc<Colours>, path: &str) -> AnyView {
    // The settings, followed for the file's language.
    let file = path.to_string();
    let lang = create_memo(move |_| crate::language::id(ui, &file));
    let on = create_memo(move |_| crate::settings::flag(ui, "editor.minimap.enabled", Some(lang.get())));
    let chars = create_memo(move |_| crate::settings::flag(ui, "editor.minimap.renderCharacters", Some(lang.get())));
    let left = create_memo(move |_| crate::settings::text(ui, "editor.minimap.side", Some(lang.get())) == "left");
    let sticky_on = create_memo(move |_| crate::settings::flag(ui, "editor.stickyScroll.enabled", Some(lang.get())));
    // Its width, by the width of the text beside it.
    let (viewport, cw_colours, eid) = (editor.viewport, colours.clone(), editor.id());
    let char_width = create_memo(move |_| {
        viewport.track();
        let family = cw_colours.font_family(eid, 0);
        let attrs = Attrs::new().family(&family).font_size(cw_colours.font_size(eid, 0) as f32);
        let mut layout = TextLayout::new();
        layout.set_text("0000000000", AttrsList::new(attrs));
        layout.size().width / 10.0
    });
    let total = create_rw_signal(0.0);
    let wide = create_memo(move |_| if on.get() { width(total.get(), char_width.get()) } else { 0.0 });
    let room = room_of(editor);
    create_effect(move |_| {
        let r = if left.get() { 0.0 } else { wide.get() };
        if room.get_untracked() != r {
            room.set(r);
        }
    });
    let map = minimap(ui, editor, colours.clone(), path, chars, on).style(move |s| {
        let w = wide.get();
        let s = s.absolute().inset_top(0.0).width(w).height_full();
        let s = if left.get() { s.inset_left(0.0) } else { s.inset_right(crate::decor::RULER) };
        s.apply_if(!on.get(), |s| s.hide())
    });
    let pins = sticky(ui, editor, colours.clone(), path, sticky_on);
    let (pinned, shown) = (pins.pins, pins.height);
    let pins = pins.view.style(move |s| {
        let (w, l) = (wide.get(), left.get());
        s.absolute()
            .inset_top(0.0)
            .inset_left(if l { w } else { 0.0 })
            .inset_right(crate::decor::RULER + if l { 0.0 } else { w })
            .height(shown.get())
            .apply_if(pinned.with(Vec::is_empty), |s| s.hide())
    });
    let id = editor.id();
    let body = floem::views::container(view)
        .style(move |s| s.size_full().padding_left(if left.get() { wide.get() } else { 0.0 }));
    floem::views::stack((body, pins, map))
        .on_resize(move |r| {
            if total.get_untracked() != r.width() {
                total.set(r.width());
            }
        })
        .on_cleanup(move || {
            ROOM.with(|r| r.borrow_mut().remove(&id));
        })
        .style(|s| s.size_full())
        .into_any()
}

/// The minimap's view: painted from the kept blocks of the lines it shows.
struct Map {
    id: ViewId,
    ui: Ui,
    editor: Editor,
    colours: Rc<Colours>,
    path: String,
    chars: Memo<bool>,
    hovered: RwSignal<bool>,
    drag: RwSignal<Option<(f64, f64)>>,
    /// A paint a frame later is asked for (lines were drawn from before an edit).
    again: Rc<Cell<bool>>,
}

/// The minimap line's height, and the height of a block in it: Monaco's 3 px (2 px with the characters).
fn heights(chars: bool) -> (f64, f64) {
    if chars { (2.0, 2.0) } else { (3.0, 2.0) }
}

fn frame_of(
    editor: &Editor,
    path: &str,
    chars: bool,
    height: f64,
) -> (Frame, Option<Rc<crate::editor::folding::FoldMap>>, usize) {
    let (map, rows, scroll_height) = rows(editor, path);
    let vp = editor.viewport.get_untracked();
    let f =
        frame(rows, f64::from(editor.line_height(0)), heights(chars).0, height, vp.y0, scroll_height, beyond(editor));
    (f, map, rows)
}

fn scroll_to(editor: &Editor, top: f64) {
    let x = editor.viewport.get_untracked().x0;
    editor.scroll_to.set(Some(Vec2::new(x, top.max(0.0))));
}

fn minimap(
    ui: Ui,
    editor: &Editor,
    colours: Rc<Colours>,
    path: &str,
    chars: Memo<bool>,
    on: Memo<bool>,
) -> impl IntoView {
    let (hovered, drag) = (create_rw_signal(false), create_rw_signal(None::<(f64, f64)>));
    let view = Map {
        id: ViewId::new(),
        ui,
        editor: editor.clone(),
        colours,
        path: path.to_string(),
        chars,
        hovered,
        drag,
        again: Rc::new(Cell::new(false)),
    };
    let id = view.id;
    let (screen, viewport, cursor, cache, colors) =
        (editor.screen_lines, editor.viewport, editor.cursor, editor.doc().cache_rev(), ui.colors);
    let rev = crate::decor::rev(ui);
    create_effect(move |_| {
        if !on.get() {
            return;
        }
        screen.track();
        viewport.track();
        cursor.track();
        cache.track();
        colors.track();
        rev.track();
        hovered.track();
        drag.track();
        chars.track();
        id.request_paint();
    });
    let (e, p) = (editor.clone(), path.to_string());
    // The frame now, while there is a slider (the editor scrolls).
    let at = move || -> Option<Frame> {
        let height = id.get_size()?.height;
        Some(frame_of(&e, &p, chars.get_untracked(), height).0).filter(|f| f.needed)
    };
    let (down, moved, wheel) = (editor.clone(), editor.clone(), editor.clone());
    let at_down = at.clone();
    view.on_event(EventListener::PointerDown, move |e| {
        let Event::PointerDown(p) = e else { return EventPropagation::Continue };
        let Some(f) = at_down() else { return EventPropagation::Stop };
        let top = if f.on_slider(p.pos.y) {
            down.viewport.get_untracked().y0
        } else {
            let top = f.clicked(p.pos.y);
            scroll_to(&down, top);
            top
        };
        drag.set(Some((p.pos.y, top)));
        id.request_active();
        EventPropagation::Stop
    })
    .on_event(EventListener::PointerMove, move |e| {
        let Event::PointerMove(p) = e else { return EventPropagation::Continue };
        if let Some((y, top)) = drag.get_untracked()
            && let Some(f) = at()
        {
            scroll_to(&moved, f.dragged(top, p.pos.y - y));
        }
        EventPropagation::Stop
    })
    .on_event_stop(EventListener::PointerUp, move |_| {
        if drag.get_untracked().is_some() {
            drag.set(None);
        }
    })
    .on_event_stop(EventListener::PointerWheel, move |e| {
        if let Event::PointerWheel(w) = e {
            wheel.scroll_delta.set(w.delta);
        }
    })
    .on_event_cont(EventListener::PointerEnter, move |_| hovered.set(true))
    .on_event_cont(EventListener::PointerLeave, move |_| hovered.set(false))
}

impl View for Map {
    fn id(&self) -> ViewId {
        self.id
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let Some(size) = self.id.get_size() else { return };
        let c = self.ui.colors.get_untracked();
        let bg = c.minimap_bg;
        cx.fill(&Rect::new(0.0, 0.0, size.width, size.height), bg, 0.0);
        let chars = self.chars.get_untracked();
        let (mini, block) = heights(chars);
        let (f, map, rows) = frame_of(&self.editor, &self.path, chars, size.height);
        let row_of = |line: usize| map.as_ref().map_or(line, |m| m.row_of_line(line));
        let start = f.first.floor() as usize;
        let end = (start + (size.height / mini).ceil() as usize + 1).min(rows);
        let y_of = |row: usize| (row as f64 - f.first) * mini;
        // The selections, across the minimap's text.
        let selected: Vec<(usize, usize)> = self.editor.cursor.with_untracked(|cur| match &cur.mode {
            CursorMode::Insert(sel) => sel
                .regions()
                .iter()
                .filter(|r| !r.is_caret())
                .map(|r| (self.editor.line_of_offset(r.min()), self.editor.line_of_offset(r.max())))
                .collect(),
            _ => Vec::new(),
        });
        for (from, to) in selected {
            let (a, b) = (row_of(from).max(start), row_of(to).min(end));
            if a <= b {
                cx.fill(&Rect::new(GUTTER, y_of(a), size.width, y_of(b) + mini), c.minimap_selection, 0.0);
            }
        }
        let alpha = if chars { 0.6 } else { 0.5 };
        let deadline = Instant::now() + BUDGET;
        let mut behind = false;
        for row in start..end {
            let line = match &map {
                Some(m) => match m.line_at_row(row) {
                    Some(l) => l,
                    None => continue,
                },
                None => row,
            };
            let (blocks, fresh) = self.colours.minimap(line, COLUMNS, Instant::now() < deadline);
            behind |= !fresh;
            let y = y_of(row);
            for b in blocks.iter().flat_map(|b| b.iter()) {
                let x = GUTTER + f64::from(b.from);
                if x >= size.width {
                    break;
                }
                let to = (GUTTER + f64::from(b.to)).min(size.width);
                cx.fill(&Rect::new(x, y, to, y + block), blend(b.colour, bg, alpha), 0.0);
            }
        }
        // The marks of the overview ruler, in the gutter.
        if let Some(m) = crate::decor::of(&self.path) {
            for (line, colour) in &m.ruler {
                let row = row_of(*line);
                if (start..end).contains(&row) {
                    let y = y_of(row);
                    cx.fill(&Rect::new(2.0, y, 6.0, y + mini), *colour, 0.0);
                }
            }
        }
        let dragging = self.drag.get_untracked().is_some();
        if f.needed && (dragging || self.hovered.get_untracked()) {
            let colour = if dragging { c.minimap_slider_active } else { c.minimap_slider_hover };
            cx.fill(&Rect::new(0.0, f.slider_top, size.width, f.slider_top + f.slider_height), colour, 0.0);
        }
        if behind && !self.again.get() {
            self.again.set(true);
            let (id, again) = (self.id, self.again.clone());
            floem::action::exec_after(Duration::from_millis(16), move |_| {
                again.set(false);
                id.request_paint();
            });
        }
    }
}

/// The sticky scroll's view and what it shows.
struct Sticky {
    view: AnyView,
    pins: Memo<Vec<Pin>>,
    height: Memo<f64>,
}

/// The lines of the scopes the top line is in, pinned over the editor's top.
fn sticky(ui: Ui, editor: &Editor, colours: Rc<Colours>, path: &str, on: Memo<bool>) -> Sticky {
    // The file's regions, found again a moment after the last edit (not on every key).
    let regions = create_rw_signal(crate::editor::folding::regions_of(path, editor));
    {
        let (cache, e, p) = (editor.doc().cache_rev(), editor.clone(), path.to_string());
        let n = Rc::new(Cell::new(0u64));
        create_effect(move |first: Option<()>| {
            cache.track();
            if first.is_none() {
                return;
            }
            n.set(n.get() + 1);
            let (mine, n, e, p) = (n.get(), n.clone(), e.clone(), p.clone());
            floem::action::exec_after(Duration::from_millis(300), move |_| {
                if n.get() != mine || e.cursor.try_get_untracked().is_none() {
                    return;
                }
                let found = crate::editor::folding::regions_of(&p, &e);
                if regions.with_untracked(|r| **r != *found) {
                    regions.set(found);
                }
            });
        });
    }
    // The outline's symbols of the file, the last ones read while it was the Outline's.
    let at = path.to_string();
    let symbols = create_memo(move |before: Option<&Option<Lines>>| {
        ui.symbols.with(|(p, s)| {
            if *p == at {
                Some(Rc::new(s.iter().map(|s| (s.line.saturating_sub(1), s.depth)).collect()))
            } else {
                before.cloned().flatten()
            }
        })
    });
    // The scopes, worked out again only when the regions or the symbols change, not on every scroll.
    let ends = editor.clone();
    let found = create_memo(move |_| {
        let text = ends.rope_text();
        regions.with(|r| {
            let r: Vec<_> = r.iter().map(|r| with_closer(&text, *r)).collect();
            symbols.with(|s| Rc::new(scopes(&r, s.as_deref().map(|s| &s[..]), ends.last_line())))
        })
    });
    let (e, viewport, screen) = (editor.clone(), editor.viewport, editor.screen_lines);
    let pins = create_memo(move |_| {
        if !on.get() {
            return Vec::new();
        }
        let vp = viewport.get();
        let line_height = f64::from(e.line_height(0));
        let found = found.get();
        screen.with(|s| {
            let lines: Vec<(usize, f64)> = s.iter_lines_y().map(|(l, y)| (l, y - vp.y0)).collect();
            pinned(
                &found,
                line_height,
                PINNED,
                |y| lines.iter().find(|(_, at)| (*at..*at + line_height).contains(&y)).map(|l| l.0),
                |line| lines.iter().filter(|l| l.0 == line).map(|l| l.1 + line_height).reduce(f64::max),
            )
        })
    });
    let line_height = {
        let e = editor.clone();
        create_memo(move |_| {
            viewport.track();
            f64::from(e.line_height(0))
        })
    };
    let height = create_memo(move |_| pins.with(|p| p.iter().map(|p| p.top).fold(0.0, f64::max)) + line_height.get());
    let hovered = create_rw_signal(None::<usize>);
    let origin = create_rw_signal(Point::ZERO);
    let view = Pins { id: ViewId::new(), ui, editor: editor.clone(), colours, pins, hovered, origin };
    let id = view.id;
    let (colors, cache, window) = (ui.colors, editor.doc().cache_rev(), editor.window_origin);
    create_effect(move |_| {
        pins.track();
        hovered.track();
        colors.track();
        cache.track();
        window.track();
        viewport.track();
        id.request_paint();
    });
    let index = move |y: f64| {
        let h = line_height.get_untracked();
        pins.with_untracked(|p| p.iter().rposition(|p| (p.top..p.top + h).contains(&y)))
    };
    let (go, wheel, p) = (editor.clone(), editor.clone(), path.to_string());
    let view = view
        .on_move(move |p| origin.set(p))
        .on_event_cont(EventListener::PointerMove, move |e| {
            if let Event::PointerMove(m) = e {
                let i = index(m.pos.y);
                if hovered.get_untracked() != i {
                    hovered.set(i);
                }
            }
        })
        .on_event_cont(EventListener::PointerLeave, move |_| {
            if hovered.get_untracked().is_some() {
                hovered.set(None);
            }
        })
        .on_event(EventListener::PointerDown, move |e| {
            let Event::PointerDown(m) = e else { return EventPropagation::Continue };
            let Some(i) = index(m.pos.y) else { return EventPropagation::Stop };
            let Some(pin) = pins.with_untracked(|p| p.get(i).copied()) else { return EventPropagation::Stop };
            reveal(&go, &p, pin.line, i);
            EventPropagation::Stop
        })
        .on_event_stop(EventListener::PointerWheel, move |e| {
            if let Event::PointerWheel(w) = e {
                wheel.scroll_delta.set(w.delta);
            }
        })
        .into_any();
    Sticky { view, pins, height }
}

/// A pinned line clicked: the caret at its first character, and the line scrolled to where it was pinned (under the
/// `above` lines pinned over it).
fn reveal(editor: &Editor, path: &str, line: usize, above: usize) {
    let text = editor.rope_text().line_content(line).to_string();
    let at = editor.offset_of_line(line) + (text.len() - text.trim_start().len());
    editor.cursor.update(|c| c.set_insert(Selection::caret(at)));
    let height = f64::from(editor.line_height(0));
    let row = crate::editor::folding::map(path).map_or(line, |m| m.row_of_line(line));
    scroll_to(editor, (row as f64 - above as f64) * height);
    if let Some(id) = editor.editor_view_id.get_untracked() {
        id.request_focus();
    }
}

/// The sticky scroll's lines: each line's own layout as the editor draws it (its colours), its number in the
/// gutter, and the shadow under them.
struct Pins {
    id: ViewId,
    ui: Ui,
    editor: Editor,
    colours: Rc<Colours>,
    pins: Memo<Vec<Pin>>,
    hovered: RwSignal<Option<usize>>,
    origin: RwSignal<Point>,
}

impl View for Pins {
    fn id(&self) -> ViewId {
        self.id
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let Some(size) = self.id.get_size() else { return };
        let pins = self.pins.get_untracked();
        if pins.is_empty() {
            return;
        }
        let c = self.ui.colors.get_untracked();
        let e = &self.editor;
        let height = f64::from(e.line_height(0));
        let bottom = size.height;
        cx.fill(&Rect::new(0.0, 0.0, size.width, bottom), c.sticky_bg, 0.0);
        let hovered = self.hovered.get_untracked();
        let gutter = (e.window_origin.get_untracked().x - self.origin.get_untracked().x).max(0.0);
        let x0 = e.viewport.get_untracked().x0;
        let eid = e.id();
        let family = self.colours.font_family(eid, 0);
        let attrs = Attrs::new()
            .family(&family)
            .font_size(self.colours.font_size(eid, 0) as f32)
            .line_height(LineHeightValue::Px(height as f32))
            .color(c.line_number);
        for (i, pin) in pins.iter().enumerate() {
            let top = pin.top;
            if hovered == Some(i) {
                cx.fill(&Rect::new(0.0, top, size.width, top + height), c.sticky_hover, 0.0);
            } else if i > 0 && pin.top < pins[i - 1].top + height {
                // Pushed up under the one above: its background covers the line it slid over.
                cx.fill(&Rect::new(0.0, top, size.width, top + height), c.sticky_bg, 0.0);
            }
            let mut number = TextLayout::new();
            number.set_text(&(pin.line + 1).to_string(), AttrsList::new(attrs));
            let w = number.size().width;
            cx.save();
            cx.clip(&Rect::new(0.0, 0.0, size.width, bottom));
            cx.draw_text(&number, Point::new(gutter - crate::editor::folding::CHEVRONS - w, top));
            cx.restore();
            let layout = e.text_layout(pin.line);
            cx.save();
            cx.clip(&Rect::new(gutter, top.max(0.0), size.width, (top + height).min(bottom)));
            cx.draw_text(&layout.text, Point::new(gutter - x0, top));
            cx.restore();
        }
        // VS Code's shadow under the lines pinned.
        for (k, a) in [0.45, 0.25, 0.1].iter().enumerate() {
            let y = bottom - 3.0 + k as f64;
            cx.fill(&Rect::new(0.0, y, size.width, y + 1.0), c.sticky_shadow.multiply_alpha(*a), 0.0);
        }
    }
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_file_fits_and_its_slider_moves_with_the_scroll() {
        // 50 lines of 19 px in a 400 px editor, minimap lines of 3 px: all fit, nothing scrolls the minimap.
        let f = frame(50, 19.0, 3.0, 400.0, 0.0, 950.0, false);
        assert_eq!((f.first, f.slider_top), (0.0, 0.0));
        assert_eq!(f.slider_height, (400.0_f64 / 19.0 * 3.0).floor());
        assert!(f.needed);
        let end = frame(50, 19.0, 3.0, 400.0, 550.0, 950.0, false);
        // Scrolled to the bottom: the slider's bottom is the last minimap line's.
        assert!((end.slider_top + end.slider_height - 150.0).abs() < 1e-9);
        assert_eq!(end.clicked(end.slider_top + end.slider_height / 2.0).round(), 550.0);
    }

    #[test]
    fn a_long_file_scrolls_the_minimap_with_the_slider_on_the_top_line() {
        // 2000 lines: the minimap shows a window of them, the editor's top line under the slider's top.
        let (top, scroll_height) = (19.0 * 1000.0, 19.0 * 2000.0);
        let f = frame(2000, 19.0, 3.0, 400.0, top, scroll_height, false);
        assert!(f.first > 0.0);
        assert!(((1000.0 - f.first) * 3.0 - f.slider_top).abs() < 1e-9);
        let most = 400.0 - f.slider_height;
        let last = frame(2000, 19.0, 3.0, 400.0, scroll_height - 400.0, scroll_height, false);
        assert!((last.slider_top - most).abs() < 1e-9, "at the bottom the slider is at the minimap's bottom");
        // A drag of the slider's whole way scrolls the whole file.
        assert!((f.dragged(0.0, most) - (scroll_height - 400.0)).abs() < 1e-6);
    }

    #[test]
    fn a_file_that_does_not_scroll_has_no_slider() {
        let f = frame(10, 19.0, 3.0, 400.0, 0.0, 400.0, false);
        assert!(!f.needed);
        assert_eq!(f.dragged(0.0, 50.0), 0.0);
    }

    #[test]
    fn the_width_is_monacos_proportional_one_at_most_120() {
        assert_eq!(width(2000.0, 8.0), 120.0);
        // (1000 - 14 - 2) / (8 + 1) = 109 columns, and the gutter.
        assert_eq!(width(1000.0, 8.0), 109.0 + GUTTER);
        assert_eq!(width(0.0, 8.0), GUTTER);
    }

    #[test]
    fn the_scopes_are_the_symbols_regions_and_a_headings_lines() {
        let regions = [(0, 9), (1, 3), (5, 8)];
        assert_eq!(scopes(&regions, None, 20), regions.to_vec());
        // Symbols at 0 and 5 (a region each), a heading at 12 to the next as deep at 15.
        let symbols = [(0, 0), (5, 1), (12, 0), (15, 0)];
        assert_eq!(scopes(&regions, Some(&symbols), 20), vec![(0, 9), (5, 8), (12, 14), (15, 20)]);
    }

    /// The lines of an editor scrolled to `top` (a line of 20 px), as the screen's lines say them.
    fn screen(top: f64) -> (impl Fn(f64) -> Option<usize>, impl Fn(usize) -> Option<f64>) {
        let first = (top / 20.0).floor() as usize;
        let lines: Vec<(usize, f64)> = (first..first + 30).map(|l| (l, l as f64 * 20.0 - top)).collect();
        let l2 = lines.clone();
        (
            move |y: f64| lines.iter().find(|(_, at)| (*at..*at + 20.0).contains(&y)).map(|l| l.0),
            move |line: usize| l2.iter().find(|l| l.0 == line).map(|l| l.1 + 20.0),
        )
    }

    #[test]
    fn the_scopes_the_top_line_is_in_are_pinned_the_outer_first() {
        let scopes = [(0, 50), (2, 20), (4, 10), (30, 40)];
        let (at, bottom) = screen(0.0);
        assert!(pinned(&scopes, 20.0, 5, at, bottom).is_empty(), "at the top nothing is above");
        let (at, bottom) = screen(100.0);
        let pins = pinned(&scopes, 20.0, 5, at, bottom);
        assert_eq!(pins.iter().map(|p| p.line).collect::<Vec<_>>(), [0, 2, 4]);
        assert_eq!(pins.iter().map(|p| p.top).collect::<Vec<_>>(), [0.0, 20.0, 40.0]);
        let (at, bottom) = screen(700.0);
        assert_eq!(pinned(&scopes, 20.0, 5, at, bottom).iter().map(|p| p.line).collect::<Vec<_>>(), [0, 30]);
        let (at, bottom) = screen(100.0);
        assert_eq!(pinned(&scopes, 20.0, 1, at, bottom).len(), 1, "at most as many as asked");
    }

    #[test]
    fn a_scope_that_ends_pushes_its_line_up() {
        // Scrolled so that line 10, the inner scope's last, ends 50 px below the top: the third pinned line, which
        // would be at 40 to 60, is pushed up to 30.
        let scopes = [(0, 50), (2, 20), (4, 10)];
        let (at, bottom) = screen(170.0);
        let pins = pinned(&scopes, 20.0, 5, at, bottom);
        assert_eq!(pins.iter().map(|p| p.line).collect::<Vec<_>>(), [0, 2, 4]);
        assert_eq!(pins[2].top, 30.0);
        assert_eq!(pins[1].top, 20.0);
    }

    #[test]
    fn blocks_are_drawn_lighter_on_the_background() {
        let (fg, bg) = (Color::rgb8(200, 100, 0), Color::rgb8(0, 0, 0));
        assert_eq!(blend(fg, bg, 0.5), Color::rgb8(100, 50, 0));
    }
}
