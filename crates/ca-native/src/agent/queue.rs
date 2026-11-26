//! The messages sent while an answer is written, waiting to be read.

use serde_json::Value;

use crate::model::Kind;
use crate::state::{Send, Ui};

/// The session's messages that are a message queued, taken in. Never taken away from the
/// rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    if m["type"] == "queued" {
        ui.push(Kind::Queued(s("text")));
    }
    false
}
