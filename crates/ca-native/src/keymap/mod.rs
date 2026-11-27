//! The keymap: the default keys and the person's (keybindings.json), each with its `when`. Nothing is here yet: the
//! file is kept for the part of the plan "VS Code keymap, when clauses, keybindings.json", which writes it. What
//! every part has (`register`, `take`) is said in `ext.rs`.

use serde_json::Value;

use crate::state::{Send, Ui};

pub mod defaults;
pub mod editor;
pub mod when;

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}
