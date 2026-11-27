//! The compare editor: two texts side by side or inline, their changes aligned. Nothing is here yet: the file is
//! kept for the part of the plan "Side-by-side compare editor", which writes it. What every part has (`register`,
//! `take`) is said in `ext.rs`.

use serde_json::Value;

use crate::state::{Send, Ui};

pub mod align;
pub mod ranges;
pub mod toolbar;

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}
