//! The commands running with no time limit, each a bar over the input with a button that stops it.

use floem::IntoView;
use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::{Decorators, dyn_stack, h_stack, label};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, action};

pub(super) fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    dyn_stack(
        move || ui.jobs.get(),
        |(id, _): &(u64, String)| *id,
        move |(id, cmd): (u64, String)| {
            let send = send.clone();
            h_stack((
                icon("terminal", 16.0),
                label(move || cmd.clone()).style(|s| s.flex_grow(1.0_f32).font_family(MONO_FONT.clone())),
                action(t, "debug-stop", move || send(json!({"type": "stop_job", "id": id}))),
            ))
            .style(move |s| super::bar(t, s).gap(6.0).items_center())
        },
    )
    .style(|s| s.flex_col().gap(6.0))
}

/// The session's message of its tasks, the running commands' part taken in. Never taken away (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "tasks" {
        ui.jobs.set(
            m["jobs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|j| j["running"] == true)
                .map(|j| (j["id"].as_u64().unwrap_or(0), j["command"].as_str().unwrap_or("").to_string()))
                .collect(),
        );
    }
    false
}
