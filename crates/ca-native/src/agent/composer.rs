//! The input as VS Code's chat input: a box with the text and, under it, its toolbar (the pickers; send or stop).
//! And what the thread's place shows while there is no thread yet.

use floem::IntoView;
use floem::keyboard::{Key, NamedKey};
use floem::kurbo::Rect;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, create_effect, create_rw_signal};
use floem::style::TextOverflow;
use floem::views::{Decorators, TextInput, dyn_container, empty, h_stack_from_iter, label, v_stack, v_stack_from_iter};
use serde_json::json;

use super::pickers::pill;
use super::slots::{self, Slot};
use crate::icons::icon;
use crate::state::{Picker, Send, Ui};
use crate::widgets::{action, bold};

pub(super) fn welcome(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    v_stack((
        icon("comment-discussion", 48.0).style(move |s| s.color(t.get().desc).margin_bottom(12.0)),
        label(|| "Code Arena").style(move |s| bold(s).font_size(20.0).color(t.get().fg)),
        label(|| "Ask about this folder, or have the agent change it.")
            .style(move |s| s.color(t.get().desc).margin_top(6.0).text_overflow(TextOverflow::Wrap)),
    ))
    .style(|s| s.items_center().justify_center().size_full().padding(20.0))
}

/// The input as VS Code's chat input: a box with the text and, under it, its toolbar.
pub(super) fn input(ui: Ui, send: Send, field: TextInput, anchor: RwSignal<Rect>) -> impl IntoView {
    let t = ui.colors;
    let focused = create_rw_signal(false);
    let submit = {
        let send = send.clone();
        move || {
            let text = ui.draft.get_untracked().trim().to_string();
            if !text.is_empty() {
                send(json!({"type": "send", "text": text}));
                ui.draft.set(String::new());
            }
        }
    };
    let enter = submit.clone();
    let stop = send.clone();
    let send_or_stop = send.clone();
    // A turn that begins closes a picker left open.
    create_effect(move |_| {
        if ui.running.get() {
            ui.picker.set(None);
        }
    });
    let field = field
        .placeholder("Ask Code Arena")
        .on_key_down(Key::Named(NamedKey::Enter), |m| m.is_empty(), move |_| enter())
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| stop(json!({"type": "stop"})))
        .on_event_cont(floem::event::EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(floem::event::EventListener::FocusLost, move |_| focused.set(false))
        .style(move |s| {
            let c = t.get();
            s.width_full()
                .padding_horiz(2.0)
                .padding_vert(4.0)
                .border(0.0)
                .background(floem::peniko::Color::TRANSPARENT)
                .color(c.input_fg)
                .font_size(13.0)
                .cursor_color(floem::peniko::Brush::Solid(c.cursor))
        });
    // The text, with what is sent with the message over it (no gap of the box's between them: what is there brings
    // its own space, and nothing takes none).
    let text = match slots::column(ui, &send, Slot::BoxTop) {
        Some(over) => v_stack((over, field)).style(|s| s.width_full().min_width(0.0)).into_any(),
        None => field.into_any(),
    };
    let send_button = dyn_container(
        move || ui.running.get(),
        move |running| {
            let send = send_or_stop.clone();
            let submit = submit.clone();
            if running {
                action(t, "debug-stop", move || send(json!({"type": "stop"}))).into_any()
            } else {
                action(t, "send", submit).into_any()
            }
        },
    );
    // The toolbar: the pickers, what other parts add after them, the room between, what they add before the send
    // button, and it.
    let mut tools = vec![
        pill(ui, Picker::Mode, anchor).into_any(),
        pill(ui, Picker::Model, anchor).into_any(),
        pill(ui, Picker::Thinking, anchor).into_any(),
    ];
    tools.extend(slots::row(ui, &send, Slot::BoxLeft));
    tools.push(empty().style(|s| s.flex_grow(1.0_f32)).into_any());
    tools.extend(slots::row(ui, &send, Slot::BoxRight));
    tools.push(send_button.into_any());
    let toolbar = h_stack_from_iter(tools).style(|s| s.gap(2.0).items_center().width_full().min_width(0.0));
    v_stack_from_iter([text, toolbar.into_any()]).style(move |s| {
        let c = t.get();
        s.margin(12.0)
            .margin_top(6.0)
            .padding(6.0)
            .gap(4.0)
            .border(1.0)
            .border_color(if focused.get() { c.focus } else { c.input_border })
            .border_radius(6.0)
            .background(c.input_bg)
    })
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: crate::state::Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
}
