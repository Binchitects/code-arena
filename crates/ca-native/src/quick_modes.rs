//! Quick Open's other modes: a line (:), a symbol (@), a symbol of the project (#), and the ones parts add. Nothing
//! is here yet: the file is kept for the parts of the plan "Quick Open modes and palette recents" and "Go to Symbol
//! in Workspace", which writes it. What every part has (`register`, `take`) is said in `ext.rs`.

use serde_json::Value;

use crate::state::{Send, Ui};

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}
