//! An answer's Markdown drawn as VS Code's chat draws it: 13 px text at a line and a half, headings in their sizes,
//! inline code in the editor's font on `textPreformat.background`, fenced code in a bordered box with the theme's
//! token colours, lists with their bullets and numbers, links in `textLink.foreground`. An answer is written a piece
//! at a time: each block is a view of its own that stays while the answer grows, and only the block that changed
//! (the last) is laid out again.

use std::cell::RefCell;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::LazyLock;

use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::Rect;
use floem::reactive::{
    Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo, create_rw_signal,
};
use floem::style::{CursorStyle, Position, TextOverflow};
use floem::text::{Attrs, AttrsList, FamilyOwned, LineHeightValue, Style as Slant, TextLayout, Weight};
use floem::views::{
    Decorators, container, dyn_stack, empty, h_stack, label, rich_text, scroll, stack, text_input, tooltip, v_stack,
};
use floem::{AnyView, IntoView, View};

use crate::citations::{self, Target};
use crate::highlight::{Line, Snippet};
use crate::markdown::{Block, Text, parse};
use crate::state::Ui;
use crate::widgets::{MONO_FONT, UI_FONT};

static UI: LazyLock<Vec<FamilyOwned>> = LazyLock::new(|| FamilyOwned::parse_list(&UI_FONT).collect());
static MONO: LazyLock<Vec<FamilyOwned>> = LazyLock::new(|| FamilyOwned::parse_list(&MONO_FONT).collect());

/// An answer's text, drawn from its Markdown as it is written.
pub fn answer(ui: Ui, text: RwSignal<String>) -> impl IntoView {
    answer_quoting(ui, text, None)
}

/// A selection made in an answer's words, as it is offered to be quoted: the text selected, the view of the block
/// it is in, and where it is in that view.
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    pub text: String,
    pub view: floem::ViewId,
    pub rect: Rect,
}

/// What an answer's words tell of a selection made in them: `picked` is told each one made, and none when it is
/// gone; `held` says whether a press is on what was offered for it (its toolbar), so the selection stays though the
/// words lose the keys.
#[derive(Clone)]
pub struct Quoting {
    pub picked: Rc<dyn Fn(Option<Picked>)>,
    pub held: Rc<dyn Fn() -> bool>,
}

/// An answer's text whose selections are told to `quoting` (an answer of the chat, to reply quoting a part of it).
pub fn answer_quoting(ui: Ui, text: RwSignal<String>, quoting: Option<Quoting>) -> impl IntoView {
    let blocks = create_memo(move |_| text.with(|t| parse(t).into_iter().map(Rc::new).collect::<Vec<_>>()));
    blocks_view(ui, blocks, quoting, None)
}

/// Where a link's address leads, as a document reads it.
pub type Targeting = Box<dyn Fn(&str) -> Option<Target>>;

/// A picture drawn, by its address and its words.
pub type Picturing = Box<dyn Fn(&str, &str) -> AnyView>;

/// What a document drawn by the answers' renderer (a Markdown file's preview) does that an answer does not: where
/// its links lead and what a click on one does, how its pictures are drawn, and the view each of its blocks is.
pub struct Doc {
    /// Where a link's address leads (a path is the document's own, read from its folder); none: nowhere.
    pub target: Targeting,
    /// What a click on a link does.
    pub follow: Box<dyn Fn(&Target)>,
    /// A picture of it drawn, by its address and its words.
    pub picture: Picturing,
    /// Told each block's view as it is made, by the block's place in the document.
    pub placed: Box<dyn Fn(usize, floem::ViewId)>,
}

/// A document's blocks drawn as an answer's are: read off the window's thread by whoever has it, each block laid
/// out again only when it changes.
pub fn document(ui: Ui, blocks: Memo<Vec<Rc<Block>>>, doc: Doc) -> impl View {
    blocks_view(ui, blocks, None, Some(Rc::new(doc)))
}

fn blocks_view(ui: Ui, blocks: Memo<Vec<Rc<Block>>>, quoting: Option<Quoting>, doc: Option<Rc<Doc>>) -> impl View {
    dyn_stack(
        move || blocks.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(n, b): &(usize, Rc<Block>)| (*n, shape(b)),
        move |(n, b): (usize, Rc<Block>)| {
            let view = block(ui, n, &b, blocks, quoting.clone(), doc.clone());
            if let Some(d) = &doc {
                (d.placed)(n, view.id());
            }
            view
        },
    )
    .style(move |s| s.flex_col().width_full().min_width(0.0).apply_if(blocks.with(|b| b.is_empty()), |s| s.hide()))
}

/// A paragraph that is only pictures (and the spaces between them): their addresses and words.
fn pictures_only(t: &Text) -> Option<Vec<(String, String)>> {
    if t.pictures.is_empty() {
        return None;
    }
    let mut rest = t.text.clone().into_bytes();
    for (r, _) in &t.pictures {
        rest[r.clone()].fill(b' ');
    }
    rest.iter()
        .all(u8::is_ascii_whitespace)
        .then(|| t.pictures.iter().map(|(r, src)| (src.clone(), t.text[r.clone()].to_string())).collect())
}

/// What a block's view is made for: its kind, and what of it the view is built with. Its text is not of it, so a
/// block that grows keeps its view.
fn shape(b: &Block) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(b).hash(&mut h);
    match b {
        Block::Heading(level, _) => level.hash(&mut h),
        Block::Item { depth, marker, .. } => (depth, marker).hash(&mut h),
        Block::Code { lang, name, .. } => (lang, name).hash(&mut h),
        Block::Table { align, .. } => align.hash(&mut h),
        // A paragraph of pictures is drawn as them (in a document), so its pictures are what it is made for.
        Block::Paragraph(t) => t.pictures.iter().for_each(|(_, src)| src.hash(&mut h)),
        _ => {}
    }
    h.finish()
}

fn block(
    ui: Ui,
    n: usize,
    first: &Block,
    blocks: Memo<Vec<Rc<Block>>>,
    q: Option<Quoting>,
    doc: Option<Rc<Doc>>,
) -> AnyView {
    let t = ui.colors;
    // The block as it is now: only a change to it lays it out again.
    let now = create_memo(move |_| blocks.with(|b| b.get(n).cloned()));
    let words = move || {
        now.with(|b| match b.as_deref() {
            Some(Block::Paragraph(t) | Block::Heading(_, t) | Block::Quote(t) | Block::Item { text: t, .. }) => {
                t.clone()
            }
            _ => Text::default(),
        })
    };
    // The space over a block (none over an answer's first).
    let over = move |px: f64| if n == 0 { 0.0 } else { px };
    match first {
        Block::Paragraph(t) if doc.is_some() && pictures_only(t).is_some() => {
            let d = doc.as_deref().expect("a document");
            let shown = pictures_only(t).unwrap_or_default();
            floem::views::h_stack_from_iter(shown.iter().map(|(src, alt)| (d.picture)(src, alt)))
                .style(move |s| s.flex_wrap(floem::style::FlexWrap::Wrap).gap(4.0).width_full().margin_top(over(8.0)))
                .into_any()
        }
        Block::Paragraph(_) => prose(ui, words, 13.0, false, q, doc).style(move |s| s.margin_top(over(8.0))).into_any(),
        Block::Heading(level, _) => {
            let size = match level {
                1 => 20.0,
                2 => 16.0,
                3 => 14.0,
                _ => 13.0,
            };
            prose(ui, words, size, true, q, doc).style(move |s| s.margin_top(over(16.0))).into_any()
        }
        Block::Item { depth, marker, .. } => {
            let (depth, marker) = (*depth as f64, marker.clone());
            h_stack((
                label(move || marker.clone()).style(move |s| {
                    s.min_width(22.0)
                        .padding_right(6.0)
                        .justify_end()
                        .line_height(1.5)
                        .color(t.get().fg)
                        .selectable(false)
                }),
                prose(ui, words, 13.0, false, q, doc).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0)),
            ))
            .style(move |s| s.width_full().min_width(0.0).padding_left(depth * 20.0).margin_top(over(4.0)))
            .into_any()
        }
        Block::Quote(_) => container(prose(ui, words, 13.0, false, q, doc))
            .style(move |s| {
                s.width_full()
                    .min_width(0.0)
                    .padding_left(10.0)
                    .padding_vert(2.0)
                    .border_left(4.0)
                    .border_color(t.get().quote_border)
                    .margin_top(over(8.0))
            })
            .into_any(),
        Block::Code { lang, name, .. } => {
            let text = move || {
                now.with(|b| match b.as_deref() {
                    Some(Block::Code { text, .. }) => text.clone(),
                    _ => String::new(),
                })
            };
            if crate::diagrams::kind_of(lang).is_some() {
                let closed =
                    create_memo(move |_| now.with(|b| matches!(b.as_deref(), Some(Block::Code { closed: true, .. }))));
                return crate::diagrams::block(ui, lang, name, text, closed)
                    .style(move |s| s.margin_top(over(8.0)))
                    .into_any();
            }
            code(ui, lang, name, text).style(move |s| s.margin_top(over(8.0))).into_any()
        }
        Block::Table { .. } => table(ui, now, q, doc).style(move |s| s.margin_top(over(8.0))).into_any(),
        Block::Rule => empty()
            .style(move |s| {
                s.height(1.0).width_full().margin_top(over(12.0)).margin_bottom(4.0).background(t.get().widget_border)
            })
            .into_any(),
    }
}

/// What a block's text is laid out as: wrapped to its width, the runs of inline code (drawn on their background),
/// the places its links lead to, and the text itself (what a selection of it copies).
#[derive(Clone, Default)]
struct Laid {
    layout: TextLayout,
    marks: Vec<Range<usize>>,
    links: Vec<(Range<usize>, Target)>,
    text: String,
}

/// Where a text's links lead: its own links (to the web or the project's files), and the citations of the
/// project's files in its inline code and in its plain words. In the order they come in the text.
fn cited(words: &Text, files: &HashSet<String>) -> Vec<(Range<usize>, Target)> {
    let mut out: Vec<(Range<usize>, Target)> =
        words.links.iter().filter_map(|(r, href)| Some((r.clone(), citations::target(href, files)?))).collect();
    for (r, style) in &words.spans {
        if style.code
            && !style.link
            && let Some(mut at) = citations::parse_ref(&words.text[r.clone()])
            && let Some(path) = citations::known(files, &at.path)
        {
            at.path = path;
            out.push((r.clone(), Target::File(at)));
        }
    }
    let marked = |r: &Range<usize>| words.spans.iter().any(|(s, _)| s.start < r.end && r.start < s.end);
    for (r, at) in citations::find_refs(&words.text, files) {
        if !marked(&r) {
            out.push((r, Target::File(at)));
        }
    }
    out.sort_by_key(|(r, _)| r.start);
    out
}

/// The place in the text under a point of it (a byte offset), and whether the point is on the text.
fn offset(layout: &TextLayout, x: f64, y: f64) -> (usize, bool) {
    let hit = layout.hit_point(floem::kurbo::Point::new(x.max(0.0), y.max(0.0)));
    let start = layout.lines_range().get(hit.line).map(|r| r.start).unwrap_or(0);
    (start + hit.index, hit.is_inside)
}

/// The link at a place of the text.
fn link_at(links: &[(Range<usize>, Target)], at: usize) -> Option<&Target> {
    links.iter().find(|(r, _)| r.start <= at && at < r.end).map(|(_, to)| to)
}

/// Runs of a text drawn as backgrounds under it (its inline code's, its selection's), in a layer of their own.
fn backgrounds(
    ui: Ui,
    runs: impl Fn() -> Vec<Range<usize>> + 'static,
    laid: RwSignal<Laid>,
    colour: fn(&crate::colors::Colors) -> floem::peniko::Color,
    round: f64,
) -> impl IntoView {
    let t = ui.colors;
    dyn_stack(
        move || {
            let runs = runs();
            let rects = match runs.is_empty() {
                true => Vec::new(),
                false => laid.with(|l| rects(&l.layout, &runs, 0.0)),
            };
            rects.into_iter().enumerate().collect::<Vec<_>>()
        },
        |(n, r): &(usize, Rect)| (*n, [r.x0, r.y0, r.x1, r.y1].map(f64::to_bits)),
        move |(_, r): (usize, Rect)| {
            empty().style(move |s| {
                s.position(Position::Absolute)
                    .inset_left(r.x0)
                    .inset_top(r.y0)
                    .width(r.width())
                    .height(r.height())
                    .border_radius(round)
                    .background(colour(&t.get()))
            })
        },
    )
    .style(|s| s.position(Position::Absolute).inset(0.0))
}

/// A text laid out on one line with its runs (bold, slanted, code, links in their colour), and where its inline
/// code is.
fn lay_out(
    words: &Text,
    links: &[(Range<usize>, Target)],
    c: &crate::colors::Colors,
    size: f32,
    heading: bool,
) -> (TextLayout, Vec<Range<usize>>) {
    let base = Attrs::new()
        .family(&UI)
        .font_size(size)
        .line_height(LineHeightValue::Px(size * if heading { 1.4 } else { 1.5 }))
        .weight(if heading { *crate::widgets::SEMIBOLD } else { Weight::NORMAL })
        .color(c.fg);
    let mut attrs = AttrsList::new(base);
    let mut marks = Vec::new();
    let linked = |r: &Range<usize>| links.iter().any(|(l, _)| l == r);
    for (range, style) in &words.spans {
        let mut a = base;
        if style.bold {
            a = a.weight(Weight::BOLD);
        }
        if style.italic {
            a = a.style(Slant::Italic);
        }
        if style.code {
            a = a.family(&MONO).font_size(size - 1.0).color(c.preformat);
            marks.push(range.clone());
        }
        if style.link || linked(range) {
            a = a.color(c.link);
        }
        attrs.add_span(range.clone(), a);
    }
    let plain = |r: &Range<usize>| !words.spans.iter().any(|(s, _)| s.start < r.end && r.start < s.end);
    for (r, _) in links.iter().filter(|(r, _)| plain(r)) {
        attrs.add_span(r.clone(), base.color(c.link));
    }
    let mut layout = TextLayout::new();
    layout.set_text(&words.text, attrs);
    (layout, marks)
}

/// A block's text with its runs: wrapped to its width, the backgrounds of its inline code drawn under it. Its links
/// open with a click (a citation of a file in the editor, an address in the browser); a drag selects its text, and
/// Ctrl+C copies what is selected.
fn prose(
    ui: Ui,
    words: impl Fn() -> Text + 'static,
    size: f32,
    heading: bool,
    q: Option<Quoting>,
    doc: Option<Rc<Doc>>,
) -> impl IntoView {
    let t = ui.colors;
    let doc_links = doc.clone();
    let laid = create_rw_signal(Laid::default());
    // The width the text is wrapped to, as it was laid out last.
    let width = create_rw_signal(0.0_f64);
    // Laid out when the text, the theme or the project's files change; wrapped again only when the width does.
    let whole = create_rw_signal(Laid::default());
    create_effect(move |_| {
        let (c, words) = (t.get(), words());
        // A document's links are its own (a path read from its folder); an answer's cite the project's files too.
        let links = match &doc_links {
            Some(d) => words.links.iter().filter_map(|(r, href)| Some((r.clone(), (d.target)(href)?))).collect(),
            None => cited(&words, &citations::files(ui)),
        };
        let (layout, marks) = lay_out(&words, &links, &c, size, heading);
        whole.set(Laid { layout, marks, links, text: words.text });
    });
    create_effect(move |_| {
        let width = width.get();
        let mut l = whole.with(Laid::clone);
        if width > 0.0 && l.layout.size().width > width {
            l.layout.set_size(width as f32, f32::MAX);
        }
        laid.set(l);
    });
    // What is selected (where it began, where it is now), where the pointer went down, whether it is over a link.
    let selected = create_rw_signal(None::<(usize, usize)>);
    let pressed = create_rw_signal(None::<usize>);
    let over_link = create_rw_signal(false);
    let marks = backgrounds(ui, move || laid.with(|l| l.marks.clone()), laid, |c| c.preformat_bg, 3.0);
    let chosen = move || match selected.get() {
        Some((a, b)) if a != b => std::iter::once(a.min(b)..a.max(b)).collect(),
        _ => Vec::new(),
    };
    let selection = backgrounds(ui, chosen, laid, |c| c.selection, 0.0);
    let text = rich_text(move || laid.with(|l| l.layout.clone()))
        .on_resize(move |r| {
            if width.get_untracked() != r.width() {
                width.set(r.width());
            }
        })
        .style(|s| s.width_full().min_width(0.0));
    let view = stack((marks, selection, text));
    let id = view.id();
    // A selection told to the answer it is in (none: it is gone).
    let held = q.as_ref().map(|q| q.held.clone());
    let tell = move |chose: Option<(usize, usize)>| {
        let Some(q) = &q else { return };
        let picked = chose.filter(|(a, b)| a != b).and_then(|(a, b)| {
            let range = a.min(b)..a.max(b);
            laid.with_untracked(|l| {
                let text = l.text.get(range.clone())?.to_string();
                let rect = rects(&l.layout, &[range], 0.0).into_iter().reduce(|a, b| a.union(b))?;
                Some(Picked { text, view: id, rect })
            })
        });
        (q.picked)(picked);
    };
    let tell = Rc::new(tell);
    let (on_down, on_up, on_lost, on_all) = (tell.clone(), tell.clone(), tell.clone(), tell);
    view.keyboard_navigable()
        .on_event(EventListener::PointerDown, move |e| {
            let Event::PointerDown(p) = e else { return EventPropagation::Continue };
            if !p.button.is_primary() {
                return EventPropagation::Continue;
            }
            id.request_focus();
            id.request_active();
            let (at, _) = laid.with_untracked(|l| offset(&l.layout, p.pos.x, p.pos.y));
            pressed.set(Some(at));
            selected.set(Some((at, at)));
            on_down(None);
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::PointerMove, move |e| {
            let Event::PointerMove(p) = e else { return };
            let (at, inside) = laid.with_untracked(|l| offset(&l.layout, p.pos.x, p.pos.y));
            if let Some(from) = pressed.get_untracked() {
                if selected.get_untracked() != Some((from, at)) {
                    selected.set(Some((from, at)));
                }
            } else {
                let over = inside && laid.with_untracked(|l| link_at(&l.links, at).is_some());
                if over_link.get_untracked() != over {
                    over_link.set(over);
                }
            }
        })
        .on_event(EventListener::PointerUp, move |e| {
            let Event::PointerUp(p) = e else { return EventPropagation::Continue };
            let Some(from) = pressed.get_untracked() else { return EventPropagation::Continue };
            pressed.set(None);
            id.clear_active();
            let (at, inside) = laid.with_untracked(|l| offset(&l.layout, p.pos.x, p.pos.y));
            if at == from {
                selected.set(None);
                if let Some(to) = inside.then(|| laid.with_untracked(|l| link_at(&l.links, at).cloned())).flatten() {
                    match &doc {
                        Some(d) => (d.follow)(&to),
                        None => citations::open(ui, &to),
                    }
                }
            } else {
                on_up(Some((from, at)));
            }
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            // A press on the selection's toolbar keeps it.
            if held.as_ref().is_some_and(|held| held()) {
                return;
            }
            if selected.get_untracked().is_some() {
                selected.set(None);
                on_lost(None);
            }
        })
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            let m = k.modifiers;
            let command = m.contains(Modifiers::CONTROL) || m.contains(Modifiers::META);
            let letter = |l: &str| matches!(&k.key.logical_key, Key::Character(c) if c.eq_ignore_ascii_case(l));
            if command && letter("c") {
                if let Some((a, b)) = selected.get_untracked().filter(|(a, b)| a != b) {
                    let text = laid.with_untracked(|l| l.text.get(a.min(b)..a.max(b)).map(str::to_string));
                    if let Some(text) = text {
                        let _ = floem::Clipboard::set_contents(text);
                    }
                    return EventPropagation::Stop;
                }
            } else if command && letter("a") {
                let all = (0, laid.with_untracked(|l| l.text.len()));
                selected.set(Some(all));
                on_all(Some(all));
                return EventPropagation::Stop;
            }
            EventPropagation::Continue
        })
        .style(move |s| {
            s.width_full().min_width(0.0).cursor(if over_link.get() { CursorStyle::Pointer } else { CursorStyle::Text })
        })
}

/// Where the runs `marks` are drawn in a text wrapped to `width` (a run that goes over a line's end has a
/// rectangle on each of its lines).
fn rects(layout: &TextLayout, marks: &[Range<usize>], width: f64) -> Vec<Rect> {
    let wrapped;
    let layout = if width > 0.0 && layout.size().width > width {
        let mut l = layout.clone();
        l.set_size(width as f32, f32::MAX);
        wrapped = l;
        &wrapped
    } else {
        layout
    };
    let mut out = Vec::new();
    for run in layout.layout_runs() {
        let base = layout.lines_range().get(run.line_i).map(|r| r.start).unwrap_or(0);
        for mark in marks {
            let within = run.glyphs.iter().filter(|g| base + g.start < mark.end && base + g.end > mark.start);
            let span = within.fold(None, |span: Option<(f32, f32)>, g| {
                Some(span.map_or((g.x, g.x + g.w), |(a, b)| (a.min(g.x), b.max(g.x + g.w))))
            });
            if let Some((left, right)) = span {
                let (top, bottom) = (run.line_top as f64 + 1.0, (run.line_top + run.line_height) as f64 - 1.0);
                out.push(Rect::new(left as f64 - 2.0, top, right as f64 + 2.0, bottom));
            }
        }
    }
    out
}

/// A table's filter box is offered once it has more rows than this.
const FILTER_FROM: usize = 8;

/// The widest a table's column is drawn (its longer cells wrap within it).
const MOST_COLUMN: f64 = 320.0;

/// A number, when the whole cell reads as one (`1,204`, `35%`, `$4.20`, `-3`); else none, and the cell sorts as text.
fn cell_number(text: &str) -> Option<f64> {
    let t = text.trim();
    let t = t.strip_prefix(['$', '€', '£', '¥']).unwrap_or(t);
    let t = t.strip_suffix('%').unwrap_or(t).replace(',', "").replace('−', "-");
    let body = t.strip_prefix(['-', '+']).unwrap_or(&t);
    let (whole, power) = match body.find(['e', 'E']) {
        Some(at) => (&body[..at], Some(&body[at + 1..])),
        None => (body, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (int, frac) = whole.split_once('.').unwrap_or((whole, ""));
    let whole_ok =
        (digits(int) || int.is_empty()) && (frac.is_empty() || digits(frac)) && whole != "." && !whole.is_empty();
    let power_ok = power.is_none_or(|p| digits(p.strip_prefix(['-', '+']).unwrap_or(p)));
    (whole_ok && power_ok).then(|| t.parse::<f64>().ok()).flatten().filter(|n| n.is_finite())
}

/// Text in the order people read it: letters without their case, and the numbers in it as numbers (2 before 10).
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        n.push(c);
                        it.next();
                    }
                    n
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (m, n) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let order = m.len().cmp(&n.len()).then_with(|| m.cmp(n));
                if order.is_ne() {
                    return order;
                }
            }
            (Some(c), Some(d)) => {
                let order = c.to_lowercase().cmp(d.to_lowercase());
                if order.is_ne() {
                    return order;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// The rows (by their place) in the order of one column: numbers as numbers when both cells are, else text as
/// people read it; empty cells last whichever way it sorts.
fn sort_rows(rows: &[Vec<Text>], column: usize, desc: bool) -> Vec<usize> {
    let cell = |r: usize| rows[r].get(column).map(|c| c.text.trim()).unwrap_or("");
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        let (x, y) = (cell(a), cell(b));
        match (x.is_empty(), y.is_empty()) {
            (true, true) => return std::cmp::Ordering::Equal,
            (true, false) => return std::cmp::Ordering::Greater,
            (false, true) => return std::cmp::Ordering::Less,
            _ => {}
        }
        let o = match (cell_number(x), cell_number(y)) {
            (Some(m), Some(n)) => m.partial_cmp(&n).unwrap_or(std::cmp::Ordering::Equal),
            _ => natural(x, y),
        };
        if desc { o.reverse() } else { o }
    });
    order
}

/// A header's click: sorted up by a new column, then down, then back to the order written.
fn next_sort(now: Option<(usize, bool)>, column: usize) -> Option<(usize, bool)> {
    match now {
        Some((c, false)) if c == column => Some((column, true)),
        Some((c, true)) if c == column => None,
        _ => Some((column, false)),
    }
}

/// The rows shown: those that hold the words typed (in any cell, case aside), in the order sorted.
fn shown_rows(rows: &[Vec<Text>], sort: Option<(usize, bool)>, filter: &str) -> Vec<usize> {
    let order = match sort {
        Some((column, desc)) => sort_rows(rows, column, desc),
        None => (0..rows.len()).collect(),
    };
    let words = filter.trim().to_lowercase();
    if words.is_empty() {
        return order;
    }
    order.into_iter().filter(|&r| rows[r].iter().any(|c| c.text.to_lowercase().contains(&words))).collect()
}

/// How wide a cell's text is on one line, drawn as it is (a header's in bold).
fn measure(text: &Text, bold: bool, c: &crate::colors::Colors) -> f64 {
    lay_out(text, &[], c, 13.0, bold).0.size().width
}

/// A table as VS Code's chat draws one: its cells in bordered columns as wide as their widest text (to a limit, past
/// which they wrap), the header's text in bold. A header's click sorts by its column (up, down, then as written),
/// and a long table has a box to filter its rows. It scrolls across when it is wider than the answer.
fn table(ui: Ui, now: Memo<Option<Rc<Block>>>, q: Option<Quoting>, doc: Option<Rc<Doc>>) -> impl IntoView {
    let t = ui.colors;
    let parts = create_memo(move |_| match now.get().as_deref() {
        Some(Block::Table { head, rows, .. }) => (Rc::new(head.clone()), Rc::new(rows.clone())),
        _ => (Rc::new(Vec::new()), Rc::new(Vec::new())),
    });
    let sort = create_rw_signal(None::<(usize, bool)>);
    let filter = create_rw_signal(String::new());
    // Each text measured once (the table is read again with each piece of an answer).
    let measured = Rc::new(RefCell::new(std::collections::HashMap::<(Text, bool), f64>::new()));
    let widths = create_memo(move |_| {
        let (head, rows) = parts.get();
        let mut m = measured.borrow_mut();
        let c = t.get_untracked();
        let mut width =
            |text: &Text, bold: bool| *m.entry((text.clone(), bold)).or_insert_with(|| measure(text, bold, &c));
        (0..head.len())
            .map(|c| {
                let widest = rows.iter().filter_map(|r| r.get(c)).map(|x| width(x, false)).fold(0.0, f64::max);
                // A header has its sort mark beside it.
                let header = width(&head[c], true) + 20.0;
                (widest.max(header).min(MOST_COLUMN) + 17.0).ceil()
            })
            .collect::<Vec<f64>>()
    });
    let cell_style = move |s: floem::style::Style, column: usize| {
        let w = widths.with(|w| w.get(column).copied().unwrap_or(60.0));
        s.width(w)
            .min_width(w)
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .border_right(1.0)
            .border_bottom(1.0)
            .border_color(t.get().widget_border)
    };
    let header = dyn_stack(
        move || parts.with(|(head, _)| head.iter().map(|c| c.text.clone()).enumerate().collect::<Vec<_>>()),
        |(n, text): &(usize, String)| (*n, text.clone()),
        move |(n, text): (usize, String)| {
            let mark = move || match sort.get() {
                Some((c, false)) if c == n => "arrow-up",
                Some((c, true)) if c == n => "arrow-down",
                _ => "unfold",
            };
            h_stack((
                label(move || text.clone()).style(|s| s.font_weight(*crate::widgets::SEMIBOLD).min_width(0.0)),
                floem::views::dyn_container(mark, |m| crate::icons::icon(m, 14.0)).style(move |s| {
                    let unsorted = sort.get().is_none_or(|(c, _)| c != n);
                    s.color(t.get().icon).apply_if(unsorted, |s| s.color(t.get().icon.multiply_alpha(0.4)))
                }),
            ))
            .on_click_stop(move |_| sort.update(|s| *s = next_sort(*s, n)))
            .style(move |s| {
                cell_style(s, n).gap(4.0).items_center().line_height(1.5).cursor(CursorStyle::Pointer).selectable(false)
            })
        },
    )
    .style(move |s| s.border_left(1.0).border_top(1.0).border_color(t.get().widget_border));
    let shown = create_memo(move |_| {
        let (_, rows) = parts.get();
        let order = filter.with(|f| sort.with(|s| shown_rows(&rows, *s, f)));
        order.into_iter().map(|r| (r, Rc::new(rows[r].clone()))).collect::<Vec<_>>()
    });
    let body = dyn_stack(
        move || shown.get(),
        |(r, cells): &(usize, Rc<Vec<Text>>)| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            cells.hash(&mut h);
            (*r, h.finish())
        },
        move |(_, cells): (usize, Rc<Vec<Text>>)| {
            let row = (0..cells.len())
                .map(|c| {
                    let words = cells[c].clone();
                    container(prose(ui, move || words.clone(), 13.0, false, q.clone(), doc.clone()))
                        .style(move |s| cell_style(s, c))
                })
                .collect::<Vec<_>>();
            floem::views::h_stack_from_iter(row).style(move |s| s.border_left(1.0).border_color(t.get().widget_border))
        },
    )
    .style(|s| s.flex_col());
    let none = label(move || format!("No rows hold “{}”.", filter.get().trim())).style(move |s| {
        let hidden = filter.with(|f| f.trim().is_empty()) || shown.with(|r| !r.is_empty());
        s.padding(6.0).color(t.get().desc).apply_if(hidden, |s| s.hide())
    });
    let search = h_stack((
        crate::icons::icon("search", 14.0).style(move |s| s.color(t.get().desc)),
        text_input(filter).placeholder("Filter rows").style(move |s| {
            let c = t.get();
            let s =
                s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).font_size(12.0).border(0.0).background(c.input_bg);
            crate::widgets::steady_input(s.color(c.input_fg), c.input_bg, None)
        }),
    ))
    .style(move |s| {
        let c = t.get();
        let few = parts.with(|(_, rows)| rows.len() <= FILTER_FROM);
        s.width(224.0)
            .height(26.0)
            .gap(6.0)
            .padding_horiz(6.0)
            .margin_bottom(6.0)
            .items_center()
            .border(1.0)
            .border_radius(2.0)
            .border_color(c.widget_border)
            .background(c.input_bg)
            .apply_if(few, |s| s.hide())
    });
    v_stack((
        search,
        scroll(v_stack((header, body)).style(|s| s.flex_col())).style(|s| s.width_full().min_width(0.0)),
        none,
    ))
    .style(|s| s.width_full().min_width(0.0))
}

/// What Enter in a code block's Save box does.
#[derive(Debug, PartialEq, Eq)]
enum SaveStep {
    /// No path is typed.
    Nothing,
    /// The path is a file of the project already: it is said so first.
    Ask,
    Save,
}

/// What Enter does with the path typed: a file of the project is replaced only once it has been asked about.
fn save_step(path: &str, files: &HashSet<String>, asked: Option<&str>) -> SaveStep {
    let path = path.trim();
    if path.is_empty() {
        SaveStep::Nothing
    } else if citations::known(files, path).is_some() && asked != Some(path) {
        SaveStep::Ask
    } else {
        SaveStep::Save
    }
}

/// The file name a block is saved as when its fence names none: `snippet` with its language's extension.
fn snippet_name(lang: &str) -> String {
    let ext = match lang {
        "rust" | "rs" => "rs",
        "python" | "py" => "py",
        "typescript" | "ts" => "ts",
        "tsx" => "tsx",
        "javascript" | "js" => "js",
        "jsx" => "jsx",
        "sh" | "bash" | "shell" | "zsh" | "console" => "sh",
        "powershell" | "ps1" | "pwsh" => "ps1",
        "csharp" | "cs" | "c#" => "cs",
        "cpp" | "c++" | "cc" => "cpp",
        "c" | "h" => "c",
        "go" | "golang" => "go",
        "java" => "java",
        "kotlin" | "kt" => "kt",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "html" | "xml" | "css" | "scss" | "sql" | "md" | "lua" | "rb" | "php" | "swift" => lang,
        "markdown" => "md",
        "ruby" => "rb",
        _ => "txt",
    };
    format!("snippet.{ext}")
}

/// What a code block's bar says: the file its fence names, else its language, else `text`.
fn code_label(lang: &str, name: &str) -> String {
    [name, lang].into_iter().find(|s| !s.is_empty()).unwrap_or("text").to_string()
}

/// One of a code block's bar's actions: a 16 px codicon in a 22 px square, with its tooltip.
pub(crate) fn bar_action(
    ui: Ui,
    icon: &'static str,
    tip: impl Fn() -> String + 'static,
    on: impl Fn() + 'static,
) -> impl IntoView {
    let tip = Rc::new(tip);
    tooltip(crate::widgets::action(ui.colors, icon, on), move || {
        let tip = tip.clone();
        label(move || tip())
    })
}

/// A fenced block as VS Code's chat draws it: the editor's font at 13 px on the editor's background, in a bordered
/// box, coloured by its language's grammar and the theme's token colours. Its bar names it (the file its fence
/// names, or its language) and holds its actions: Open in the editor (for a file of the project), Wrap Lines (on;
/// off, it scrolls across with its line numbers), Save as a file (a path asked for under the bar), Copy (which says
/// Copied a moment). Each of its lines is a view of its own, so a block being written lays out its last line only.
pub(crate) fn code(ui: Ui, lang: &str, name: &str, text: impl Fn() -> String + 'static) -> impl IntoView {
    code_with(ui, lang, name, text, None)
}

/// What a block drawn as a picture (a diagram) adds to its code's box: its actions on the bar (before Copy), the
/// view under the bar (the picture, or what it says of it), and whether that picture stands in the lines' place.
pub(crate) struct Drawing {
    pub actions: AnyView,
    pub over: AnyView,
    pub drawn: Memo<bool>,
}

/// A fenced block's box (see [`code`]), with what a drawing of it adds.
pub(crate) fn code_with(
    ui: Ui,
    lang: &str,
    name: &str,
    text: impl Fn() -> String + 'static,
    drawing: Option<Drawing>,
) -> impl IntoView {
    let t = ui.colors;
    let drawn = drawing.as_ref().map(|d| d.drawn);
    let is_drawn = move || drawn.is_some_and(|d| d.get());
    let (actions, over) = match drawing {
        Some(d) => (d.actions, d.over),
        None => (empty().style(|s| s.hide()).into_any(), empty().style(|s| s.hide()).into_any()),
    };
    let text = create_memo(move |_| text());
    let wrap = create_rw_signal(true);
    let copied = create_rw_signal(false);
    let saving = create_rw_signal(false);
    // The path asked about (a file of the project already): Enter again replaces it.
    let asked = create_rw_signal(None::<String>);
    let path = create_rw_signal(if name.is_empty() { snippet_name(lang) } else { name.to_string() });
    let kept = Rc::new(RefCell::new(Snippet::new(lang)));
    let lines = dyn_stack(
        move || {
            let tokens = ui.tokens.get();
            text.with(|text| kept.borrow_mut().lines(text, &tokens))
        },
        |l: &Rc<Line>| l.id,
        move |l: Rc<Line>| {
            let base = Attrs::new()
                .family(&MONO)
                .font_size(13.0)
                .line_height(LineHeightValue::Px(18.0))
                .color(t.get_untracked().editor_fg);
            let mut attrs = AttrsList::new(base);
            for run in &l.runs {
                let mut a = base.color(run.colour);
                if run.bold {
                    a = a.weight(Weight::BOLD);
                }
                if run.italic {
                    a = a.style(Slant::Italic);
                }
                attrs.add_span(run.range.clone(), a);
            }
            let mut layout = TextLayout::new();
            layout.set_tab_width(4);
            layout.set_text(&l.text, attrs);
            rich_text(move || layout.clone()).style(move |s| match wrap.get() {
                true => s.width_full().min_width(0.0).min_height(18.0).text_overflow(TextOverflow::Wrap),
                false => s.height(18.0).text_overflow(TextOverflow::Clip),
            })
        },
    )
    .style(move |s| s.flex_col().apply_if(wrap.get(), |s| s.width_full().min_width(0.0)));
    // The lines' numbers, while they are not wrapped (a wrapped line would not stand by its number).
    let count = create_memo(move |_| text.with(|t| t.lines().count().max(1)));
    let numbers =
        label(move || (1..=count.get()).map(|i| i.to_string()).collect::<Vec<_>>().join("\n")).style(move |s| {
            s.font_family(MONO_FONT.to_string())
                .font_size(13.0)
                .line_height(18.0 / 13.0)
                .padding_right(12.0)
                .color(t.get().line_number)
                .selectable(false)
                .apply_if(wrap.get(), |s| s.hide())
        });
    let body = h_stack((numbers, scroll(lines).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0)))).style(
        move |s| s.width_full().min_width(0.0).padding_horiz(12.0).padding_vert(8.0).apply_if(is_drawn(), |s| s.hide()),
    );
    let label_text = code_label(lang, name);
    let name = name.to_string();
    let file = create_memo(move |_| {
        let files = citations::files(ui);
        (!name.is_empty()).then(|| citations::known(&files, &name)).flatten()
    });
    let copy = move || {
        let _ = floem::Clipboard::set_contents(text.get_untracked());
        copied.set(true);
        floem::action::exec_after(std::time::Duration::from_millis(1500), move |_| {
            if copied.try_get_untracked() == Some(true) {
                copied.set(false);
            }
        });
    };
    let open = container(bar_action(
        ui,
        "go-to-file",
        move || format!("Open {} in the editor", file.get().unwrap_or_default()),
        move || {
            if let Some(path) = file.get_untracked() {
                citations::open(ui, &Target::File(citations::Ref { path, ..Default::default() }));
            }
        },
    ))
    .style(move |s| s.apply_if(file.with(Option::is_none), |s| s.hide()));
    let copy_button = h_stack((
        crate::icons::icon("copy", 16.0).style(move |s| s.apply_if(copied.get(), |s| s.hide())),
        crate::icons::icon("check", 16.0).style(move |s| s.apply_if(!copied.get(), |s| s.hide())),
        label(move || if copied.get() { "Copied" } else { "Copy" }),
    ))
    .on_click_stop(move |_| copy())
    .style(move |s| {
        let c = t.get();
        s.height(22.0)
            .padding_horiz(4.0)
            .gap(4.0)
            .items_center()
            .border_radius(5.0)
            .color(c.icon)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.toolbar_hover))
    });
    let bar = h_stack((
        label(move || label_text.clone()).style(move |s| {
            s.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
                .text_overflow(TextOverflow::Ellipsis)
                .color(t.get().desc)
        }),
        open,
        actions,
        container(bar_action(
            ui,
            "word-wrap",
            move || if wrap.get() { "Do Not Wrap Lines".into() } else { "Wrap Lines".into() },
            move || wrap.update(|w| *w = !*w),
        ))
        .style(move |s| s.apply_if(is_drawn(), |s| s.hide())),
        bar_action(ui, "save", || "Save as a File...".into(), move || saving.update(|s| *s = !*s)),
        copy_button,
    ))
    .style(move |s| {
        s.width_full()
            .height(28.0)
            .padding_left(12.0)
            .padding_right(4.0)
            .gap(2.0)
            .items_center()
            .font_size(12.0)
            .border_bottom(1.0)
            .border_color(t.get().widget_border)
            .selectable(false)
    });
    let input = text_input(path);
    let input_id = input.id();
    create_effect(move |_| {
        if saving.get() {
            input_id.request_focus();
        }
    });
    let input = input
        .placeholder("Path in the project")
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            match &k.key.logical_key {
                Key::Named(NamedKey::Enter) => {
                    let to = path.get_untracked();
                    let files = citations::files(ui);
                    match asked.with_untracked(|a| save_step(&to, &files, a.as_deref())) {
                        SaveStep::Nothing => {}
                        SaveStep::Ask => asked.set(Some(to.trim().to_string())),
                        SaveStep::Save => {
                            citations::save(&to, &text.get_untracked());
                            asked.set(None);
                            saving.set(false);
                        }
                    }
                }
                Key::Named(NamedKey::Escape) => saving.set(false),
                _ => {}
            }
            // No key goes on to the workbench's own while a path is typed.
            EventPropagation::Stop
        })
        .style(move |s| {
            let c = t.get();
            let s = s
                .width_full()
                .height(24.0)
                .padding_horiz(6.0)
                .font_size(13.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(c.focus)
                .background(c.input_bg)
                .color(c.input_fg);
            crate::widgets::steady_input(s, c.input_bg, Some(c.focus))
        });
    let warning = label(move || format!("{} is there already: press Enter again to replace it.", path.get().trim()))
        .style(move |s| {
            let shown = asked.with(|a| a.as_deref() == Some(path.with(|p| p.trim().to_string()).as_str()));
            s.width_full().color(t.get().warning).apply_if(!shown, |s| s.hide())
        });
    let ask = v_stack((
        h_stack((label(|| "Save as").style(move |s| s.color(t.get().desc).selectable(false)), input))
            .style(|s| s.width_full().gap(8.0).items_center()),
        warning,
    ))
    .style(move |s| {
        s.width_full()
            .gap(8.0)
            .items_center()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .font_size(12.0)
            .border_bottom(1.0)
            .border_color(t.get().widget_border)
            .apply_if(!saving.get(), |s| s.hide())
    });
    v_stack((bar, ask, over, body)).style(move |s| {
        let c = t.get();
        s.width_full()
            .min_width(0.0)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(4.0)
            .background(c.editor_bg)
    })
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: crate::state::Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::inline;

    #[test]
    fn a_block_that_grows_keeps_its_view_and_another_kind_does_not() {
        let para = |s| Block::Paragraph(inline(s));
        assert_eq!(shape(&para("a")), shape(&para("a longer one")));
        assert_ne!(shape(&para("a")), shape(&Block::Quote(inline("a"))));
        let code = |lang: &str, text: &str| Block::Code {
            lang: lang.into(),
            name: String::new(),
            text: text.into(),
            closed: true,
        };
        assert_eq!(shape(&code("rust", "fn")), shape(&code("rust", "fn main() {}")));
        assert_ne!(shape(&code("rust", "fn")), shape(&code("sh", "fn")));
        assert_ne!(shape(&Block::Heading(1, inline("a"))), shape(&Block::Heading(2, inline("a"))));
        assert_ne!(shape(&para("![a](a.png)")), shape(&para("![a](b.png)")), "a picture changed is drawn again");
    }

    #[test]
    fn a_paragraph_of_pictures_alone_is_drawn_as_them() {
        let both = vec![("a.png".to_string(), "one".to_string()), ("b.svg".to_string(), "b.svg".to_string())];
        assert_eq!(pictures_only(&inline("![one](a.png) ![](b.svg)")), Some(both));
        assert_eq!(pictures_only(&inline("See ![one](a.png)")), None, "words beside it: a paragraph");
        assert_eq!(pictures_only(&inline("[one](a.png)")), None, "a link is no picture");
    }

    fn rows(cells: &[&[&str]]) -> Vec<Vec<Text>> {
        cells.iter().map(|r| r.iter().map(|c| inline(c)).collect()).collect()
    }

    #[test]
    fn a_tables_numbers_sort_as_numbers_its_words_as_people_read_them_and_empty_cells_last() {
        assert_eq!(cell_number("1,204"), Some(1204.0));
        assert_eq!(cell_number("35%"), Some(35.0));
        assert_eq!(cell_number("$4.20"), Some(4.2));
        assert_eq!(cell_number("−3"), Some(-3.0));
        assert_eq!(cell_number("1e3"), Some(1000.0));
        for not in ["", "v2", "1.2.3", "-", ".", "12 ms", "inf", "NaN"] {
            assert_eq!(cell_number(not), None, "{not}");
        }
        let r = rows(&[&["a10", "10"], &["a2", "9"], &["", "100"], &["B1", ""]]);
        assert_eq!(sort_rows(&r, 0, false), [1, 0, 3, 2]);
        assert_eq!(sort_rows(&r, 0, true), [3, 0, 1, 2], "reversed, the empty cell still last");
        assert_eq!(sort_rows(&r, 1, false), [1, 0, 2, 3]);
        assert_eq!(sort_rows(&r, 1, true), [2, 0, 1, 3]);
    }

    #[test]
    fn a_header_sorts_up_then_down_then_as_written_and_the_filter_keeps_the_rows_that_hold_it() {
        assert_eq!(next_sort(None, 1), Some((1, false)));
        assert_eq!(next_sort(Some((1, false)), 1), Some((1, true)));
        assert_eq!(next_sort(Some((1, true)), 1), None);
        assert_eq!(next_sort(Some((1, true)), 0), Some((0, false)));
        let r = rows(&[&["main.rs", "3"], &["Lib.rs", "1"], &["readme", "2"]]);
        assert_eq!(shown_rows(&r, None, ""), [0, 1, 2]);
        assert_eq!(shown_rows(&r, Some((1, false)), " "), [1, 2, 0]);
        assert_eq!(shown_rows(&r, Some((1, true)), "RS"), [0, 1]);
        assert!(shown_rows(&r, None, "nothing").is_empty());
    }

    #[test]
    fn a_code_block_is_named_by_its_file_else_its_language_and_saved_by_its_extension() {
        assert_eq!(code_label("ts", "src/app.ts"), "src/app.ts");
        assert_eq!(code_label("rust", ""), "rust");
        assert_eq!(code_label("", ""), "text");
        assert_eq!(snippet_name("python"), "snippet.py");
        assert_eq!(snippet_name("css"), "snippet.css");
        assert_eq!(snippet_name("brainfuck"), "snippet.txt");
        let files: HashSet<String> = ["src/app.ts".to_string()].into();
        assert_eq!(save_step("  ", &files, None), SaveStep::Nothing);
        assert_eq!(save_step("new.ts", &files, None), SaveStep::Save);
        assert_eq!(save_step("src/app.ts", &files, None), SaveStep::Ask, "a file there is not replaced unasked");
        assert_eq!(save_step("src/app.ts", &files, Some("other.ts")), SaveStep::Ask);
        assert_eq!(save_step(" src/app.ts", &files, Some("src/app.ts")), SaveStep::Save);
    }

    #[test]
    fn an_answers_links_and_the_projects_files_it_cites_lead_where_they_say() {
        let files: HashSet<String> = ["src/app.ts", "README.md"].map(String::from).into();
        let words =
            inline("Open `src/app.ts:12`, see README.md and [docs](https://x.dev), not `other.ts` or [x](#top).");
        let links: Vec<(&str, Target)> =
            cited(&words, &files).into_iter().map(|(r, to)| (&words.text[r], to)).collect();
        let file = |path: &str, line| Target::File(citations::Ref { path: path.into(), line, ..Default::default() });
        assert_eq!(
            links,
            [
                ("src/app.ts:12", file("src/app.ts", Some(12))),
                ("README.md", file("README.md", None)),
                ("docs", Target::Web("https://x.dev".into())),
            ]
        );
        // Before the files are listed, only its own links lead anywhere.
        assert_eq!(cited(&words, &HashSet::new()).len(), 1);
        let to = links.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>();
        let ranges: Vec<(Range<usize>, Target)> = cited(&words, &files);
        assert_eq!(link_at(&ranges, ranges[0].0.start), Some(&to[0]));
        assert_eq!(link_at(&ranges, ranges[0].0.end), None);
    }

    #[test]
    fn inline_codes_background_is_under_its_letters_on_each_line_it_is_on() {
        // Drawn in the one font every machine has here: the window's own icons.
        crate::icons::load();
        let family: Vec<FamilyOwned> = FamilyOwned::parse_list("codicon").collect();
        let word = crate::icons::char_of("check").to_string().repeat(4);
        let text = [word.as_str(); 3].concat();
        let mark = word.len()..word.len() * 2;
        let mut layout = TextLayout::new();
        layout.set_text(&text, AttrsList::new(Attrs::new().family(&family).font_size(16.0)));
        let whole = layout.size().width;
        let one = rects(&layout, std::slice::from_ref(&mark), 0.0);
        assert_eq!(one.len(), 1, "on one line, one rectangle");
        assert!(one[0].x0 > 0.0 && one[0].x1 < whole && one[0].width() > whole / 4.0);
        // Wrapped within the code, it is on two lines.
        let two = rects(&layout, std::slice::from_ref(&mark), whole / 2.0 + 1.0);
        assert_eq!(two.len(), 2);
        assert!(two[1].y0 > two[0].y0);
    }
}
