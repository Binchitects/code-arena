//! The input's pickers, as VS Code's chat input has them: the mode, the model and how much it thinks, each a pill
//! opening a menu over itself.

use std::rc::Rc;

use floem::event::EventListener;
use floem::keyboard::{Key, NamedKey};
use floem::kurbo::Rect;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, create_effect};
use floem::style::{CursorStyle, Position, TextOverflow};
use floem::views::{Decorators, dyn_stack, empty, h_stack, label, scroll, stack};
use floem::{IntoView, View, ViewId};
use serde_json::json;

use crate::icons::icon;
use crate::state::{Picker, Send, Ui};

/// A menu's least width, as VS Code's.
const MENU_WIDTH: f64 = 160.0;

/// The modes the session takes (what runs without asking), and how much it may think.
const MODES: &[&str] = &["ask", "auto-edit", "plan", "yolo"];
const THINKING: &[&str] = &["default", "off", "low", "medium", "high", "xhigh"];

/// A name with its first letter a capital, as a menu shows it.
fn capital(s: &str) -> String {
    let mut rest = s.chars();
    rest.next().map(|c| c.to_uppercase().chain(rest).collect()).unwrap_or_default()
}

/// What a picker has in force.
fn current(ui: Ui, which: Picker) -> String {
    match which {
        Picker::Mode => ui.mode.get(),
        Picker::Model => ui.model.get(),
        Picker::Thinking => ui.thinking.get(),
    }
}

/// What a picker offers. The model in force is listed even when the session did not name it among its models.
fn options(ui: Ui, which: Picker) -> Vec<String> {
    match which {
        Picker::Mode => MODES.iter().map(|m| m.to_string()).collect(),
        Picker::Thinking => THINKING.iter().map(|m| m.to_string()).collect(),
        Picker::Model => {
            let (mut all, now) = (ui.models.get(), ui.model.get());
            if !now.is_empty() && !all.contains(&now) {
                all.insert(0, now);
            }
            all
        }
    }
}

/// An option as the menu names it: a model by its id, the others with a capital.
fn named(which: Picker, v: &str) -> String {
    if which == Picker::Model { v.to_string() } else { capital(v) }
}

/// What tells the session a picker's choice.
fn chosen(which: Picker, v: &str) -> serde_json::Value {
    match which {
        Picker::Mode => json!({"type": "mode", "mode": v}),
        Picker::Model => json!({"type": "model", "model": v}),
        Picker::Thinking => json!({"type": "thinking", "level": v}),
    }
}

/// One of the input's pickers, as VS Code's chat input has them: what is in force and a chevron, opening its menu
/// over itself. While a turn runs the session takes none of the three, so they are dimmed and do not open.
pub(super) fn pill(ui: Ui, which: Picker, anchor: RwSignal<Rect>) -> impl IntoView {
    let t = ui.colors;
    let view = h_stack((
        label(move || {
            let now = current(ui, which);
            match which {
                Picker::Mode if now.is_empty() => "Agent".to_string(),
                Picker::Thinking if now.is_empty() => "Thinking".to_string(),
                Picker::Thinking => format!("Thinking: {now}"),
                _ => named(which, &now),
            }
        })
        .style(|s| s.min_width(0.0).text_overflow(TextOverflow::Ellipsis)),
        icon("chevron-down", 12.0),
    ));
    let id = view.id();
    view.on_event_stop(EventListener::PointerDown, move |_| {
        if ui.running.get_untracked() {
            return;
        }
        let now = current(ui, which);
        ui.picker_at.set(options(ui, which).iter().position(|o| *o == now).unwrap_or(0));
        anchor.set(id.layout_rect());
        ui.picker.set(Some(which));
    })
    .style(move |s| {
        let c = t.get();
        let s = s
            .gap(2.0)
            .items_center()
            .min_width(0.0)
            .padding_horiz(6.0)
            .height(22.0)
            .border_radius(5.0)
            .font_size(12.0)
            .selectable(false)
            .apply_if(which != Picker::Model, |s| s.flex_shrink(0.0_f32));
        if ui.running.get() {
            s.color(c.desc.multiply_alpha(0.5))
        } else {
            let lit = ui.picker.get() == Some(which);
            s.color(c.desc)
                .cursor(CursorStyle::Pointer)
                .apply_if(lit, |s| s.background(c.toolbar_hover))
                .hover(|s| s.background(c.toolbar_hover))
        }
    })
}

/// The open picker's menu, as VS Code's: 26 px items, a check by the one in force, in a layer over the whole view
/// (so every item is within the view's bounds and takes the pointer), placed over the pill that opened it. The
/// menu takes the keys while it is open (the arrows, Enter, Escape) and closes on a press anywhere else: in the
/// view, on the layer; elsewhere in the window, by losing the keys. `frame` is the view, whose place in the window
/// is the layer's (a hidden layer has none), and `back` the input, which the keys go back to.
pub(super) fn menu(ui: Ui, send: Send, anchor: RwSignal<Rect>, frame: ViewId, back: ViewId) -> impl IntoView {
    let t = ui.colors;
    let close = move || {
        ui.picker.set(None);
        back.request_focus();
    };
    let choose: Rc<dyn Fn(Picker, String)> = Rc::new(move |which, v| {
        close();
        send(chosen(which, &v));
    });
    let items = {
        let choose = choose.clone();
        dyn_stack(
            move || {
                let which = ui.picker.get();
                let all = which.map(|w| options(ui, w)).unwrap_or_default();
                all.into_iter().enumerate().filter_map(|(n, v)| Some((n, which?, v))).collect::<Vec<_>>()
            },
            |item: &(usize, Picker, String)| item.clone(),
            move |(n, which, v): (usize, Picker, String)| {
                let (choose, name, value) = (choose.clone(), named(which, &v), v.clone());
                h_stack((
                    icon("check", 16.0)
                        .style(move |s| s.apply_if(current(ui, which) != value, |s| s.color(Color::TRANSPARENT))),
                    label(move || name.clone()),
                ))
                // On the press: a press takes the keys from the menu, and so would close it before a click.
                .on_event_stop(EventListener::PointerDown, move |_| choose(which, v.clone()))
                .on_event_cont(EventListener::PointerEnter, move |_| ui.picker_at.set(n))
                .style(move |s| {
                    let c = t.get();
                    s.height(26.0)
                        .items_center()
                        .gap(6.0)
                        .padding_left(6.0)
                        .padding_right(20.0)
                        .margin_horiz(4.0)
                        .border_radius(4.0)
                        .color(c.menu_fg)
                        .cursor(CursorStyle::Pointer)
                        .apply_if(ui.picker_at.get() == n, |s| s.background(c.menu_selected).color(c.menu_selected_fg))
                })
            },
        )
        .style(|s| s.flex_col().min_width(MENU_WIDTH).padding_vert(4.0))
    };
    let step = move |by: isize| {
        let n = ui.picker.get_untracked().map(|w| options(ui, w).len()).unwrap_or(0) as isize;
        if n > 0 {
            ui.picker_at.update(|at| *at = (*at as isize + by).rem_euclid(n) as usize);
        }
    };
    let behind = empty().on_event_stop(EventListener::PointerDown, move |_| close()).style(|s| s.size_full());
    let list = scroll(items)
        .keyboard_navigable()
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| close())
        .on_key_down(Key::Named(NamedKey::ArrowDown), |_| true, move |_| step(1))
        .on_key_down(Key::Named(NamedKey::ArrowUp), |_| true, move |_| step(-1))
        .on_key_down(
            Key::Named(NamedKey::Enter),
            |_| true,
            move |_| {
                if let Some(which) = ui.picker.get_untracked()
                    && let Some(v) = options(ui, which).get(ui.picker_at.get_untracked())
                {
                    choose(which, v.clone());
                }
            },
        )
        .on_event_cont(EventListener::FocusLost, move |_| ui.picker.set(None))
        .style(move |s| {
            let c = t.get();
            // Over the pill, its left at the pill's, and within the view on every side.
            let (pill, view) = (anchor.get(), frame.layout_rect());
            let left = (pill.x0 - view.x0).min(view.width() - MENU_WIDTH - 12.0).max(4.0);
            s.position(Position::Absolute)
                .inset_left(left)
                .inset_bottom(view.y1 - pill.y0 + 4.0)
                .max_height((pill.y0 - view.y0 - 12.0).clamp(60.0, 320.0))
                .border(1.0)
                .border_color(c.menu_border)
                .border_radius(6.0)
                .background(c.menu_bg)
                .box_shadow_blur(8.0)
                .box_shadow_color(c.shadow)
                .font_size(13.0)
                .selectable(false)
        });
    let id = list.id();
    create_effect(move |_| {
        if ui.picker.get().is_some() {
            id.request_focus();
        }
    });
    stack((behind, list)).style(move |s| {
        s.position(Position::Absolute).inset(0.0).z_index(100).apply_if(ui.picker.get().is_none(), |s| s.hide())
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

    #[test]
    fn a_menus_names_begin_with_a_capital() {
        assert_eq!(capital("auto-edit"), "Auto-edit");
        assert_eq!(capital("xhigh"), "Xhigh");
        assert_eq!(capital(""), "");
    }
}
