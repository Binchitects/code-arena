//! The Changes view, as VS Code's Source Control list: each file the agent changed with its folder dimmed and its
//! letter (M, A, D) in VS Code's colours; a click opens its changes as an inline diff, the removed lines red, the
//! added green, each with its numbers before and after.

use std::rc::Rc;

use floem::IntoView;
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalWith};
use floem::style::CursorStyle;
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, dyn_stack, empty, h_stack, label, scroll, v_stack, virtual_stack,
};
use serde_json::json;

use crate::diff::{Line, Side};
use crate::state::{Change, Send, Ui};
use crate::widgets::MONO_FONT;

/// VS Code's colours for a changed file's letter (gitDecoration's).
fn letter_colour(letter: &str, dark: bool) -> Color {
    match (letter, dark) {
        ("A", true) => Color::rgb8(0x73, 0xC9, 0x91),
        ("A", false) => Color::rgb8(0x58, 0x7C, 0x0C),
        ("D", true) => Color::rgb8(0xC7, 0x4E, 0x39),
        ("D", false) => Color::rgb8(0xAD, 0x0B, 0x0B),
        (_, true) => Color::rgb8(0xE2, 0xC0, 0x8D),
        (_, false) => Color::rgb8(0x89, 0x5B, 0x03),
    }
}

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    send(json!({"type": "changes"}));
    let list = dyn_stack(
        move || ui.changes.get(),
        |c: &Change| (c.path.clone(), c.after.clone()),
        move |c: Change| {
            let (dir, name) = match c.path.rsplit_once('/') {
                Some((d, n)) => (d.to_string(), n.to_string()),
                None => (String::new(), c.path.clone()),
            };
            let letter = c.letter();
            let shown = c.clone();
            h_stack((
                label(move || name.clone()).style(move |s| {
                    let c = t.get();
                    s.color(letter_colour(letter, c.dark)).apply_if(letter == "D", |s| s.color(c.desc))
                }),
                label(move || dir.clone()).style(move |s| s.font_size(12.0).color(t.get().desc).text_ellipsis()),
                empty().style(|s| s.flex_grow(1.0_f32)),
                label(move || letter).style(move |s| s.font_size(12.0).color(letter_colour(letter, t.get().dark))),
            ))
            .on_click_stop(move |_| ui.open_changes(&shown))
            .style(move |s| {
                let c = t.get();
                s.height(22.0)
                    .width_full()
                    .items_center()
                    .gap(6.0)
                    .padding_left(20.0)
                    .padding_right(14.0)
                    .font_size(13.0)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .hover(|s| s.background(c.list_hover))
            })
        },
    )
    .style(|s| s.flex_col().width_full());
    v_stack((
        label(|| "The agent has changed no file in this session.").style(move |s| {
            s.padding_horiz(20.0)
                .padding_vert(6.0)
                .color(t.get().desc)
                .apply_if(ui.changes.with(|c| !c.is_empty()), |s| s.hide())
        }),
        scroll(list).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full()),
    ))
    .style(|s| s.size_full().min_height(0.0))
}

/// A file's changes, as VS Code's inline diff.
pub fn diff_view(ui: Ui, lines: Rc<Vec<Line>>) -> impl IntoView {
    let t = ui.colors;
    let all: im::Vector<(usize, Line)> = lines.iter().cloned().enumerate().collect();
    let number = move |n: Option<usize>| {
        label(move || n.map(|n| n.to_string()).unwrap_or_default())
            .style(move |s| s.width(44.0).justify_end().padding_right(8.0).color(t.get().line_number))
    };
    scroll(
        virtual_stack(
            VirtualDirection::Vertical,
            VirtualItemSize::Fixed(Box::new(|| 19.0)),
            move || all.clone(),
            |(i, _): &(usize, Line)| *i,
            move |(_, l): (usize, Line)| {
                let side = l.side;
                let mark = match side {
                    Side::Same => " ",
                    Side::Removed => "-",
                    Side::Added => "+",
                };
                h_stack((
                    number(l.old),
                    number(l.new),
                    label(move || mark).style(|s| s.width(16.0)),
                    label(move || l.text.replace('\t', "    ")),
                ))
                .style(move |s| {
                    let c = t.get();
                    let bg = match (side, c.dark) {
                        (Side::Removed, _) => Color::rgba8(0xFF, 0x00, 0x00, 0x33),
                        (Side::Added, true) => Color::rgba8(0x9B, 0xB9, 0x55, 0x33),
                        (Side::Added, false) => Color::rgba8(0x9C, 0xCC, 0x2C, 0x40),
                        (Side::Same, _) => Color::TRANSPARENT,
                    };
                    s.height(19.0).min_width_full().items_center().background(bg).color(c.editor_fg)
                })
            },
        )
        .style(|s| s.flex_col().min_width_full()),
    )
    .style(move |s| {
        let c = t.get();
        s.size_full().font_family(MONO_FONT.clone()).font_size(14.0).background(c.editor_bg).selectable(false)
    })
}
