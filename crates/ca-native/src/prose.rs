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

use floem::kurbo::Rect;
use floem::reactive::{
    Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo, create_rw_signal,
};
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, Modifiers, NamedKey};
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
        Block::Code { lang, name, .. } => (lang, name).hash(&mut h),
        Block::Table { align, .. } => align.hash(&mut h),
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
        Block::Code { lang, name, .. } => {
            let text = move || {
                now.with(|b| match b.as_deref() {
                    Some(Block::Code { text, .. }) => text.clone(),
                    _ => String::new(),
                })
            };
            code(ui, lang, name, text).style(move |s| s.margin_top(over(8.0))).into_any()
        }
        Block::Table { .. } => empty().into_any(),
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

/// A block's text with its runs: wrapped to its width, the backgrounds of its inline code drawn under it. Its links
/// open with a click (a citation of a file in the editor, an address in the browser); a drag selects its text, and
/// Ctrl+C copies what is selected.
fn prose(ui: Ui, words: impl Fn() -> Text + 'static, size: f32, heading: bool) -> impl IntoView {
    let t = ui.colors;
    let laid = create_rw_signal(Laid::default());
    // The width the text is wrapped to, as it was laid out last.
    let width = create_rw_signal(0.0_f64);
    create_effect(move |_| {
        let (c, words, files, width) = (t.get(), words(), citations::files(ui), width.get());
        let links = cited(&words, &files);
        let base = Attrs::new()
            .family(&UI)
            .font_size(size)
            .line_height(LineHeightValue::Px(size * if heading { 1.4 } else { 1.5 }))
            .weight(if heading { Weight::SEMIBOLD } else { Weight::NORMAL })
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
        if width > 0.0 && layout.size().width > width {
            layout.set_size(width as f32, f32::MAX);
        }
        laid.set(Laid { layout, marks, links, text: words.text });
    });
    // What is selected (where it began, where it is now), where the pointer went down, whether it is over a link.
    let selected = create_rw_signal(None::<(usize, usize)>);
    let pressed = create_rw_signal(None::<usize>);
    let over_link = create_rw_signal(false);
    let marks = backgrounds(ui, move || laid.with(|l| l.marks.clone()), laid, |c| c.preformat_bg, 3.0);
    let chosen = move || match selected.get() {
        Some((a, b)) if a != b => vec![a.min(b)..a.max(b)],
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
                    citations::open(ui, &to);
                }
            }
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            if selected.get_untracked().is_some() {
                selected.set(None);
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
                selected.set(Some((0, laid.with_untracked(|l| l.text.len()))));
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
fn bar_action(ui: Ui, icon: &'static str, tip: impl Fn() -> String + 'static, on: impl Fn() + 'static) -> impl IntoView {
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
fn code(ui: Ui, lang: &str, name: &str, text: impl Fn() -> String + 'static) -> impl IntoView {
    let t = ui.colors;
    let text = create_memo(move |_| text());
    let wrap = create_rw_signal(true);
    let copied = create_rw_signal(false);
    let saving = create_rw_signal(false);
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
    let numbers = label(move || {
        let n = text.with(|t| t.lines().count().max(1));
        (1..=n).map(|i| i.to_string()).collect::<Vec<_>>().join("\n")
    })
    .style(move |s| {
        s.font_family(MONO_FONT.to_string())
            .font_size(13.0)
            .line_height(18.0 / 13.0)
            .padding_right(12.0)
            .color(t.get().line_number)
            .selectable(false)
            .apply_if(wrap.get(), |s| s.hide())
    });
    let body = h_stack((numbers, scroll(lines).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0))))
        .style(|s| s.width_full().min_width(0.0).padding_horiz(12.0).padding_vert(8.0));
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
            s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).text_overflow(TextOverflow::Ellipsis).color(t.get().desc)
        }),
        open,
        bar_action(
            ui,
            "word-wrap",
            move || if wrap.get() { "Do Not Wrap Lines".into() } else { "Wrap Lines".into() },
            move || wrap.update(|w| *w = !*w),
        ),
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
                    let to = path.get_untracked().trim().to_string();
                    if !to.is_empty() {
                        citations::save(&to, &text.get_untracked());
                        saving.set(false);
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
            s.width_full()
                .height(24.0)
                .padding_horiz(6.0)
                .font_size(13.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(c.focus)
                .background(c.input_bg)
                .color(c.input_fg)
        });
    let ask = h_stack((
        label(|| "Save as").style(move |s| s.color(t.get().desc).selectable(false)),
        input,
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
    v_stack((bar, ask, body)).style(move |s| {
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
        let code = |lang: &str, text: &str| Block::Code { lang: lang.into(), name: String::new(), text: text.into() };
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
