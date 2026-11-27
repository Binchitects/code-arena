//! The messages sent while an answer is written, waiting to be read.

use serde_json::Value;

use crate::model::{Kind, Question};
use crate::state::{Send, Ui};

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are a message queued, taken in. Never taken away from the
/// rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "queued" {
        ui.push(Kind::Queued { id: m["id"].clone(), question: Question::from_json(m) });
    }
    false
}
