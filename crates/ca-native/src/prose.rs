//! An answer's Markdown drawn as VS Code's chat draws it: 13 px text at a line and a half, headings in their sizes,
//! inline code in the editor's font on `textPreformat.background`, fenced code in a bordered box with the theme's
//! token colours, lists with their bullets and numbers, links in `textLink.foreground`. An answer is written a piece
//! at a time: each block is a view of its own that stays while the answer grows, and only the block that changed
//! (the last) is laid out again.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::LazyLock;

use floem::kurbo::Rect;
use floem::reactive::{
    Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo, create_rw_signal,
};
use floem::style::{Position, TextOverflow};
use floem::text::{Attrs, AttrsList, FamilyOwned, LineHeightValue, Style as Slant, TextLayout, Weight};
use floem::views::{Decorators, container, dyn_stack, empty, h_stack, label, rich_text, scroll, stack};
use floem::{AnyView, IntoView};

use crate::highlight::{Line, Snippet};
use crate::markdown::{Block, Text, parse};
use crate::state::Ui;
use crate::widgets::{MONO_FONT, UI_FONT};

static UI: LazyLock<Vec<FamilyOwned>> = LazyLock::new(|| FamilyOwned::parse_list(&UI_FONT).collect());
static MONO: LazyLock<Vec<FamilyOwned>> = LazyLock::new(|| FamilyOwned::parse_list(&MONO_FONT).collect());

/// An answer's text, drawn from its Markdown as it is written.
pub fn answer(ui: Ui, text: RwSignal<String>) -> impl IntoView {
    let blocks = create_memo(move |_| text.with(|t| parse(t).into_iter().map(Rc::new).collect::<Vec<_>>()));
    dyn_stack(
        move || blocks.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(n, b): &(usize, Rc<Block>)| (*n, shape(b)),
        move |(n, b): (usize, Rc<Block>)| block(ui, n, &b, blocks),
    )
    .style(move |s| s.flex_col().width_full().min_width(0.0).apply_if(blocks.with(|b| b.is_empty()), |s| s.hide()))
}

/// What a block's view is made for: its kind, and what of it the view is built with. Its text is not of it, so a
/// block that grows keeps its view.
fn shape(b: &Block) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(b).hash(&mut h);
    match b {
        Block::Heading(level, _) => level.hash(&mut h),
        Block::Item { depth, marker, .. } => (depth, marker).hash(&mut h),
        Block::Code { lang, .. } => lang.hash(&mut h),
        _ => {}
    }
    h.finish()
}

fn block(ui: Ui, n: usize, first: &Block, blocks: Memo<Vec<Rc<Block>>>) -> AnyView {
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
        Block::Paragraph(_) => prose(ui, words, 13.0, false).style(move |s| s.margin_top(over(8.0))).into_any(),
        Block::Heading(level, _) => {
            let size = match level {
                1 => 20.0,
                2 => 16.0,
                3 => 14.0,
                _ => 13.0,
            };
            prose(ui, words, size, true).style(move |s| s.margin_top(over(16.0))).into_any()
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
                prose(ui, words, 13.0, false).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0)),
            ))
            .style(move |s| s.width_full().min_width(0.0).padding_left(depth * 20.0).margin_top(over(4.0)))
            .into_any()
        }
        Block::Quote(_) => container(prose(ui, words, 13.0, false))
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
        Block::Code { lang, .. } => {
            let text = move || {
                now.with(|b| match b.as_deref() {
                    Some(Block::Code { text, .. }) => text.clone(),
                    _ => String::new(),
                })
            };
            code(ui, lang, text).style(move |s| s.margin_top(over(8.0))).into_any()
        }
        Block::Rule => empty()
            .style(move |s| {
                s.height(1.0).width_full().margin_top(over(12.0)).margin_bottom(4.0).background(t.get().widget_border)
            })
            .into_any(),
    }
}

/// A block's text with its runs: wrapped to its width, the backgrounds of its inline code drawn under it.
fn prose(ui: Ui, words: impl Fn() -> Text + 'static, size: f32, heading: bool) -> impl IntoView {
    let t = ui.colors;
    let laid = create_rw_signal((TextLayout::new(), Vec::<Range<usize>>::new()));
    create_effect(move |_| {
        let (c, words) = (t.get(), words());
        let base = Attrs::new()
            .family(&UI)
            .font_size(size)
            .line_height(LineHeightValue::Px(size * if heading { 1.4 } else { 1.5 }))
            .weight(if heading { Weight::SEMIBOLD } else { Weight::NORMAL })
            .color(c.fg);
        let mut attrs = AttrsList::new(base);
        let mut marks = Vec::new();
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
            if style.link {
                a = a.color(c.link);
            }
            attrs.add_span(range.clone(), a);
        }
        let mut layout = TextLayout::new();
        layout.set_text(&words.text, attrs);
        laid.set((layout, marks));
    });
    // The width the text is wrapped to, as it was laid out last.
    let width = create_rw_signal(0.0);
    let marks = dyn_stack(
        move || {
            let rects = laid.with(|(layout, marks)| match marks.is_empty() {
                true => Vec::new(),
                false => rects(layout, marks, width.get()),
            });
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
                    .border_radius(3.0)
                    .background(t.get().preformat_bg)
            })
        },
    )
    .style(|s| s.position(Position::Absolute).inset(0.0));
    let text = rich_text(move || laid.with(|l| l.0.clone()))
        .on_resize(move |r| {
            if width.get_untracked() != r.width() {
                width.set(r.width());
            }
        })
        .style(|s| s.width_full().min_width(0.0));
    stack((marks, text)).style(|s| s.width_full().min_width(0.0))
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

/// A fenced block as VS Code's chat draws it: the editor's font at 13 px on the editor's background, in a bordered
/// box that scrolls across, coloured by its language's grammar and the theme's token colours. Each of its lines is
/// a view of its own, so a block being written lays out its last line only.
fn code(ui: Ui, lang: &str, text: impl Fn() -> String + 'static) -> impl IntoView {
    let t = ui.colors;
    let kept = Rc::new(RefCell::new(Snippet::new(lang)));
    let lines = dyn_stack(
        move || {
            let (text, tokens) = (text(), ui.tokens.get());
            kept.borrow_mut().lines(&text, &tokens)
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
            rich_text(move || layout.clone()).style(|s| s.height(18.0).text_overflow(TextOverflow::Clip))
        },
    )
    .style(|s| s.flex_col());
    container(scroll(lines).style(|s| s.width_full().min_width(0.0))).style(move |s| {
        let c = t.get();
        s.width_full()
            .min_width(0.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(4.0)
            .background(c.editor_bg)
    })
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
        let code = |lang: &str, text: &str| Block::Code { lang: lang.into(), text: text.into() };
        assert_eq!(shape(&code("rust", "fn")), shape(&code("rust", "fn main() {}")));
        assert_ne!(shape(&code("rust", "fn")), shape(&code("sh", "fn")));
        assert_ne!(shape(&Block::Heading(1, inline("a"))), shape(&Block::Heading(2, inline("a"))));
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
