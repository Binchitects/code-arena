//! The Agent view's header, over the thread. Nothing is in it yet: it takes no room.

use floem::IntoView;
use floem::views::{Decorators, empty};

use crate::state::{Send, Ui};

pub(super) fn view(_ui: Ui, _send: Send) -> impl IntoView {
    empty().style(|s| s.hide())
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: crate::state::Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
}
