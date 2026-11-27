//! The goal's bar over the input: what the session is working towards, and how it stands.

use floem::IntoView;
use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::{Decorators, label};
use serde_json::Value;

use crate::state::{Send, Ui};

pub(super) fn view(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    label(move || ui.goal.get().map(|g| format!("Goal  {g}")).unwrap_or_default())
        .style(move |s| super::bar(t, s).apply_if(ui.goal.get().is_none(), |s| s.hide()))
}

/// What it adds to the window's registries, once at start: its bar over the input.
pub fn register(_ui: Ui, _send: &Send) {
    super::slots::add(super::slots::Slot::Bars, "goal", 20, |ui, _| view(ui).into_any());
}

/// The session's message of its tasks, the goal's part taken in. Never taken away (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "tasks" {
        ui.goal
            .set(m["goal"]["text"].as_str().map(|g| format!("{g} · {}", m["goal"]["status"].as_str().unwrap_or(""))));
    }
    false
}
