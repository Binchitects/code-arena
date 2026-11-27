//! A request in the thread: its words in a bubble on the right.

use floem::IntoView;
use floem::reactive::SignalGet;
use floem::style::TextOverflow;
use floem::views::{Decorators, empty, h_stack, label};
use serde_json::Value;

use crate::model::{Item, Kind, Question};
use crate::state::{Send, Ui};

pub(super) fn bubble(ui: Ui, text: String) -> impl IntoView {
    let t = ui.colors;
    h_stack((
        empty().style(|s| s.flex_grow(1.0_f32).min_width(40.0)),
        label(move || text.clone()).style(move |s| {
            let c = t.get();
            s.padding_horiz(12.0)
                .padding_vert(8.0)
                .border_radius(8.0)
                .background(c.chat_request)
                .color(c.fg)
                .line_height(1.5)
                .text_overflow(TextOverflow::Wrap)
                .max_width_pct(85.0)
        }),
    ))
    .style(|s| s.width_full())
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are a request's, taken in. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "user" {
        ui.push(Kind::User(Question::from_json(m)));
    }
    false
}

/// A request the session kept, as an item (none for a note of Code Arena's own).
pub(super) fn kept(ui: Ui, msg: &Value) -> Option<Item> {
    let question = Question::from_json(msg);
    (!question.text.starts_with("(Code Arena")).then(|| Item { id: ui.id(), kind: Kind::User(question) })
}
