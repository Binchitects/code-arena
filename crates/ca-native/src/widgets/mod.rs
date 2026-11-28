//! The workbench's small parts drawn as VS Code draws them: its action buttons (22 px, rounded, a hover background),
//! its buttons, its section and view titles, and the sash a part is resized by. Its larger shared parts are this
//! file's children: the quick pick (`quick`), the dialogs (`dialog`), the toasts and the notifications' list
//! (`toast`), the tree of many rows (`tree`) and the menu opened at a place (`popup`).

use std::cell::Cell;
use std::rc::Rc;

use floem::action::add_overlay;
use floem::event::{Event, EventListener};
use floem::keyboard::{Key, Modifiers};
use floem::kurbo::{Point, Size};
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::style::{CursorStyle, Style};
use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout, Weight};
use floem::views::{Decorators, container, empty, label};
use floem::{AnyView, IntoView, View, ViewId};

use crate::colors::Colors;
use crate::icons::icon;
use crate::state::Ui;

pub mod dialog;
pub mod form;
pub mod popup;
pub mod quick;
pub mod toast;
pub mod tree;

/// VS Code's fonts on each system, the first of them this machine has (the text drawn takes one family only).
const UI_FONTS: &[&str] =
    &["Segoe UI", "SF Pro Text", "Ubuntu Sans", "Ubuntu", "Cantarell", "Noto Sans", "DejaVu Sans"];
const MONO_FONTS: &[&str] =
    &["Consolas", "Menlo", "Droid Sans Mono", "DejaVu Sans Mono", "Ubuntu Mono", "Liberation Mono"];

fn installed(names: &[&str], or: &str) -> String {
    let fonts = floem::text::FONT_SYSTEM.lock();
    let has = |n: &str| fonts.db().faces().any(|f| f.families.iter().any(|(name, _)| name == n));
    names.iter().find(|n| has(n)).map(|n| n.to_string()).unwrap_or_else(|| or.to_string())
}

pub static UI_FONT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| installed(UI_FONTS, "sans-serif"));
pub static MONO_FONT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| installed(MONO_FONTS, "monospace"));

/// Whether this machine has an italic face of normal width: text asked in italics without one has no font to be
/// drawn with (the text system finds none and stops the window), so it is drawn upright then.
pub static ITALIC: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    let fonts = floem::text::FONT_SYSTEM.lock();
    fonts.db().faces().any(|f| f.style == floem::text::Style::Italic && f.stretch == floem::text::Stretch::Normal)
});

/// The weight VS Code gives headings (600), or bold where the UI font has no semibold face: asked for a weight its
/// family lacks, the text system picks the nearest weight of any family, so headings came out in the mono font.
pub static SEMIBOLD: std::sync::LazyLock<floem::text::Weight> = std::sync::LazyLock::new(|| {
    let ui = UI_FONT.as_str();
    let fonts = floem::text::FONT_SYSTEM.lock();
    let has = fonts
        .db()
        .faces()
        .any(|f| f.weight == floem::text::Weight::SEMIBOLD && f.families.iter().any(|(name, _)| name == ui));
    if has { floem::text::Weight::SEMIBOLD } else { floem::text::Weight::BOLD }
});

pub type Theme = RwSignal<Colors>;

/// A toolbar action: a 16 px codicon in a 22 px square, rounded, with VS Code's hover background.
pub fn action(t: Theme, name: &'static str, on: impl Fn() + 'static) -> impl IntoView {
    container(icon(name, 16.0)).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        s.width(22.0)
            .height(22.0)
            .items_center()
            .justify_center()
            .border_radius(5.0)
            .color(c.icon)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.background(c.toolbar_hover))
    })
}

/// The same, greyed out while `dim` says so (as VS Code draws a disabled action), still pressable to say why.
pub fn action_dim(
    t: Theme,
    name: &'static str,
    dim: impl Fn() -> bool + 'static,
    on: impl Fn() + 'static,
) -> impl IntoView {
    let dim = std::rc::Rc::new(dim);
    let shade = dim.clone();
    // Mixed into the side bar's background, since a glyph's own transparency is not drawn by every renderer.
    let glyph = icon(name, 16.0).style(move |s| {
        let c = t.get();
        s.color(if shade() { mix(c.icon, c.side_bg, 0.6) } else { c.icon })
    });
    container(glyph).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        s.width(22.0)
            .height(22.0)
            .items_center()
            .justify_center()
            .border_radius(5.0)
            .cursor(CursorStyle::Pointer)
            .apply_if(!dim(), |s| s.hover(|s| s.background(c.toolbar_hover)))
    })
}

/// The same, drawn only while `on` is true (the action lit, as the layout buttons when their part is shown).
pub fn action_lit(t: Theme, name: impl Fn() -> &'static str + 'static, on: impl Fn() + 'static) -> impl IntoView {
    let shown = floem::views::dyn_container(name, move |n| icon(n, 16.0));
    container(shown).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        s.width(22.0)
            .height(22.0)
            .items_center()
            .justify_center()
            .border_radius(5.0)
            .color(c.icon)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.background(c.toolbar_hover))
    })
}

/// An option of a search box (Match Case, Use Regular Expression): a 20 px square, lit while it is on.
pub fn toggle(t: Theme, name: &'static str, on: RwSignal<bool>, tip: &'static str) -> impl IntoView {
    let button = container(icon(name, 16.0)).on_click_stop(move |_| on.update(|b| *b = !*b)).style(move |s| {
        let c = t.get();
        let lit = on.get();
        s.width(20.0)
            .height(20.0)
            .items_center()
            .justify_center()
            .border(1.0)
            .border_radius(3.0)
            .font_size(12.0)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .border_color(if lit { c.focus } else { Color::TRANSPARENT })
            .background(if lit { c.focus.multiply_alpha(0.35) } else { Color::TRANSPARENT })
            .color(c.input_fg)
            .hover(|s| s.background(c.toolbar_hover))
    });
    floem::views::tooltip(button, move || label(move || tip))
}

/// A button: the primary one (`button.background`) or the secondary.
pub fn button(t: Theme, text: &'static str, primary: bool, on: impl Fn() + 'static) -> impl IntoView {
    label(move || text).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        let (bg, fg, hover) = if primary {
            (c.button_bg, c.button_fg, c.button_hover)
        } else {
            (c.button2_bg, c.button2_fg, c.button2_hover)
        };
        s.padding_horiz(14.0)
            .padding_vert(4.0)
            .border_radius(4.0)
            .font_size(13.0)
            .line_height(1.4)
            .background(bg)
            .color(fg)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(hover))
    })
}

/// VS Code's small uppercase titles (the side bar's "EXPLORER", a panel's tabs): 11 px.
pub fn caps(s: Style) -> Style {
    s.font_size(11.0).selectable(false)
}

/// A text input's own background and border kept while it is pointed at or has the keys: floem's theme turns a
/// hovered input near white (its light grey text then hard to read) and a focused one's border purple. `border` is
/// the border's colour while it has the keys, as the input has none of its own when it is None.
pub fn steady_input(s: Style, background: Color, border: Option<Color>) -> Style {
    s.hover(|s| s.background(background)).focus(|s| {
        let s = s.hover(|s| s.background(background));
        match border {
            Some(c) => s.border_color(c),
            None => s.border(0.0),
        }
    })
}

pub fn bold(s: Style) -> Style {
    s.font_weight(Weight::BOLD)
}

/// A sash: the 4 px strip between two parts that resizes them, lit in `sash.hoverBorder` while it is pointed at or
/// dragged. It follows the pointer exactly: `to` gets the size the part would have, its size when the drag began
/// and how far the pointer has gone since (across when `vertical`, else down, `sign` -1 when the part grows the
/// other way), in the window's pixels.
pub fn sash(t: Theme, vertical: bool, sign: f64, size: RwSignal<f64>, to: impl Fn(f64) + 'static) -> impl IntoView {
    let to = std::rc::Rc::new(to);
    let again = to.clone();
    let down = RwSignal::new(None::<(f64, f64)>);
    let lit = RwSignal::new(false);
    let view = empty();
    let id = view.id();
    // Where the pointer is in the window: the sash's own place in it and the pointer's in the sash.
    let at = move |p: floem::kurbo::Point| {
        let o = id.layout_rect().origin();
        if vertical { o.x + p.x } else { o.y + p.y }
    };
    view.on_event_stop(EventListener::PointerDown, move |e| {
        if let Event::PointerDown(p) = e {
            down.set(Some((at(p.pos), size.get_untracked())));
            id.request_active();
        }
    })
    .on_event_stop(EventListener::PointerMove, move |e| {
        if let (Event::PointerMove(p), Some((from, start))) = (e, down.get_untracked()) {
            to(start + sign * (at(p.pos) - from));
        }
    })
    .on_event_stop(EventListener::PointerUp, move |_| down.set(None))
    .on_double_click_stop(move |_| again(f64::NAN))
    .on_event_cont(EventListener::PointerEnter, move |_| lit.set(true))
    .on_event_cont(EventListener::PointerLeave, move |_| lit.set(false))
    .style(move |s| {
        let c = t.get();
        let on = lit.get() || down.get().is_some();
        let s = if vertical {
            s.width(4.0).height_full().margin_horiz(-2.0).cursor(CursorStyle::ColResize)
        } else {
            s.height(4.0).width_full().margin_vert(-2.0).cursor(CursorStyle::RowResize)
        };
        s.z_index(10).background(if on { c.sash } else { Color::TRANSPARENT })
    })
}

/// A part's 1 px border on one side, in its colour.
pub fn line(s: Style, colour: Color, side: char) -> Style {
    let s = s.border_color(colour);
    match side {
        'l' => s.border_left(1.0),
        'r' => s.border_right(1.0),
        't' => s.border_top(1.0),
        _ => s.border_bottom(1.0),
    }
}

/// A colour between two others: `a` at 0, `b` at 1. For a dimmed text on a known background (a text's own
/// transparency is not drawn by every renderer).
pub fn mix(a: Color, b: Color, t: f64) -> Color {
    let of = |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * t.clamp(0.0, 1.0)).round() as u8;
    Color::rgba8(of(a.r, b.r), of(a.g, b.g), of(a.b, b.b), of(a.a, b.a))
}

/// The size of the window a view is in (a view may float in a window of its own); the main window's when no view
/// is given, or it is not laid out yet.
pub fn window_size(ui: Ui, of: Option<ViewId>) -> Size {
    let mut top = of;
    while let Some(p) = top.and_then(|t| t.parent()) {
        top = Some(p);
    }
    top.and_then(|t| t.get_size()).filter(|s| s.width > 0.0 && s.height > 0.0).unwrap_or_else(|| {
        let (w, h) = ui.window.get_untracked();
        Size::new(w, h)
    })
}

/// How wide a text is drawn in the window's font at a size.
pub fn text_width(text: &str, size: f32) -> f64 {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list(&UI_FONT).collect();
    let mut layout = TextLayout::new();
    layout.set_text(text, AttrsList::new(Attrs::new().family(&family).font_size(size)));
    layout.size().width
}

/// What gives a cover its keys back (a press takes them from whatever had them).
pub type Keep = Rc<dyn Fn()>;

/// A layer over the whole of the window an event is being taken in (an overlay of the window's, so it is over
/// every part of it and has the pointer first), with `body` in it: for a menu opened at a place and for a dialog.
/// The layer has the keys while it is there, each given to `key` and none going on to the workbench's own; a press
/// that no part of the body takes is a press outside it, and calls `outside` (each solid part of the body is made
/// with `solid`, which takes the press). `size` is the window's. It is outside the workbench's own view, so it
/// sets its font and colours itself. Removed with Floem's `remove_overlay` and the id this gives.
pub fn cover(
    ui: Ui,
    size: Size,
    dim: bool,
    key: impl Fn(&Key, Modifiers) + 'static,
    outside: impl Fn() + 'static,
    body: impl FnOnce(Keep) -> AnyView + 'static,
) -> ViewId {
    let t = ui.colors;
    add_overlay(Point::ZERO, move |_| {
        let me = Rc::new(Cell::new(None::<ViewId>));
        let keep: Keep = {
            let me = me.clone();
            Rc::new(move || {
                if let Some(id) = me.get() {
                    id.request_focus();
                }
            })
        };
        let layer = container(body(keep)).keyboard_navigable();
        let id = layer.id();
        me.set(Some(id));
        id.request_focus();
        layer
            .on_event_stop(EventListener::KeyDown, move |e| {
                if let Event::KeyDown(k) = e {
                    key(&k.key.logical_key, k.modifiers);
                }
            })
            .on_event_stop(EventListener::PointerDown, move |_| outside())
            // Nothing under it is pointed at or scrolled while it is there.
            .on_event_stop(EventListener::PointerMove, |_| {})
            .on_event_stop(EventListener::PointerWheel, |_| {})
            .style(move |s| {
                let c = t.get();
                // A little short of the window: an overlay that reaches the window's edge is drawn moved.
                s.width(size.width - 6.0)
                    .height(size.height - 6.0)
                    .font_family(UI_FONT.clone())
                    .font_size(13.0)
                    .color(c.fg)
                    .apply_if(dim, |s| s.background(Color::rgba8(0, 0, 0, 0x4D)))
            })
    })
}

/// A solid part of a cover's body (a menu, a dialog's box): a press on it is not a press outside, and the cover
/// keeps its keys.
pub fn solid(part: impl IntoView + 'static, keep: Keep) -> impl IntoView {
    part.into_view().on_event_stop(EventListener::PointerDown, move |_| keep())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_between_two_is_nearer_the_one_it_is_mixed_towards() {
        let (fg, bg) = (Color::rgb8(204, 204, 204), Color::rgb8(37, 37, 38));
        assert_eq!(mix(fg, bg, 0.0), fg);
        assert_eq!(mix(fg, bg, 1.0), bg);
        assert_eq!(mix(fg, bg, 0.6), Color::rgb8(104, 104, 104));
        assert_eq!(mix(fg, bg, 7.0), bg, "no further than the other colour");
        assert_eq!(mix(Color::rgba8(0, 0, 0, 0), Color::rgba8(0, 0, 0, 255), 0.5).a, 128);
    }
}
