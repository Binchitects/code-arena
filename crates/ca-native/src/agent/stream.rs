//! The session's messages about the thread as a whole: that a turn began, and the thread the session kept, made
//! into items. What is one part's is taken in by that part (a request's by `question`, an answer's words and its
//! turn's end by `answer`, a call's by `tools`, a notice by `thread`, the agent's question by `ask`, a queued
//! message by `queue`, the bars' by each bar): see `ext.rs` for the order.

use floem::reactive::SignalUpdate;
use serde_json::Value;

use crate::model::Item;
use crate::state::{Send, Ui};

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's message that a turn began, taken in. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "turn_start" {
        ui.running.set(true);
    }
    false
}

/// The thread the session kept, as items.
pub fn thread_of(ui: Ui, m: &Value) -> Vec<Item> {
    let mut items = Vec::new();
    for msg in m["messages"].as_array().into_iter().flatten() {
        match msg["role"].as_str() {
            Some("user") => items.extend(super::question::kept(ui, msg)),
            Some("assistant") => super::answer::kept(ui, msg, &mut items, m["running"] == true),
            Some("mark") if msg["type"] == "compaction" => items.push(super::compaction::kept(ui, msg)),
            _ => {}
        }
    }
    items
}
