//! The workbench's small parts drawn as VS Code draws them: its action buttons (22 px, rounded, a hover background),
//! its buttons, its section and view titles, and the sash a part is resized by.

use floem::event::{Event, EventListener};
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::style::{CursorStyle, Style};
use floem::text::Weight;
use floem::views::{Decorators, container, empty, label};
use floem::{IntoView, View};

use crate::colors::Colors;
use crate::icons::icon;

pub mod dialog;
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
