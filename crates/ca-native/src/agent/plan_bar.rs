//! The plan's bar over the input: its title and how many of its steps are done.

use floem::IntoView;
use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::{Decorators, label};
use serde_json::Value;

use crate::state::{Send, Ui};

pub(super) fn view(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    label(move || ui.plan.get().map(|(p, d, n)| format!("Plan  {p} · {d} of {n} done")).unwrap_or_default())
        .style(move |s| super::bar(t, s).apply_if(ui.plan.get().is_none(), |s| s.hide()))
}

/// What it adds to the window's registries, once at start: its bar over the input.
pub fn register(_ui: Ui, _send: &Send) {
    super::slots::add(super::slots::Slot::Bars, "plan", 10, |ui, _| view(ui).into_any());
}

/// The session's message of its tasks, the plan's part taken in. Never taken away (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "tasks" {
        ui.plan.set(m["plan"]["steps"].as_array().map(|steps| {
            let done = steps.iter().filter(|st| st["status"] == "done").count();
            (m["plan"]["title"].as_str().unwrap_or("").to_string(), done, steps.len())
        }));
    }
    false
}
