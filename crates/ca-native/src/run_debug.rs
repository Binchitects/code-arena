//! The Run and Debug view. Nothing is here yet: the file is kept for the part of the plan "Run and Debug view and
//! toolbar", which writes it. What every part has (`register`, `take`) is said in `ext.rs`; what a view has
//! (`READY`, `view`, `actions`), in `layout.rs`.

use serde_json::Value;

use crate::state::{Send, Ui};

/// Whether the view is in the window. False until the view is written.
pub const READY: bool = false;

/// The view's own content.
pub fn view(_ui: Ui, _send: Send) -> floem::AnyView {
    floem::IntoView::into_any(floem::views::empty())
}

/// The actions in its title (none: it has no action of its own).
pub fn actions(_ui: Ui, _send: Send) -> Option<floem::AnyView> {
    None
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}
