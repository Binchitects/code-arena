//! The mark where the chat was compacted, with its summary. Nothing is here yet: the file is kept for the part of
//! the plan "Compaction marks and Show summary", which writes it. What every part has (`register`, `take`) is said
//! in `ext.rs`.

use serde_json::Value;

use crate::model::Item;
use crate::state::{Send, Ui};

/// The mark drawn in the thread (the item's kind is `Kind::Compaction`). Nothing until it is written.
pub fn view(_ui: Ui, _send: Send, _item: &Item) -> floem::AnyView {
    use floem::views::Decorators;
    floem::IntoView::into_any(floem::views::empty().style(|s| s.hide()))
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}
