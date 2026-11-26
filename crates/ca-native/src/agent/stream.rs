//! The session's messages about the thread: each goes to the part of the Agent view it belongs to (a request's to
//! `question`, an answer's words and its turn's end to `answer`, a call's to `tools`, a notice to `thread`, the
//! agent's question to `ask`, a queued message to `queue`, the bars' to each bar), in that order; and the thread the
//! session kept, made into items.

use floem::reactive::SignalUpdate;
use serde_json::Value;

use crate::model::Item;
use crate::state::{Send, Ui};

/// The parts of the Agent view that take messages, in the order they are given one.
const PARTS: &[fn(Ui, &Value, &Send) -> bool] = &[
    super::question::take,
    super::queue::take,
    super::answer::take,
    super::tools::take,
    super::thread::take,
    super::ask::take,
    super::plan_bar::take,
    super::goal_bar::take,
    super::jobs_bar::take,
];

/// One message from the session, given to the Agent view's parts; true when one of them took it away.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    if m["type"] == "turn_start" {
        ui.running.set(true);
    }
    PARTS.iter().any(|part| part(ui, m, send))
}

/// The thread the session kept, as items.
pub fn thread_of(ui: Ui, m: &Value) -> Vec<Item> {
    let mut items = Vec::new();
    for msg in m["messages"].as_array().into_iter().flatten() {
        match msg["role"].as_str() {
            Some("user") => items.extend(super::question::kept(ui, msg)),
            Some("assistant") => super::answer::kept(ui, msg, &mut items),
            _ => {}
        }
    }
    items
}
