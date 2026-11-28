//! The Ports view, beside the Terminal in the panel. In a Code Studio workspace on Arena's servers the Ports panel
//! lists what the workspace's programs listen on and forwards them (Arena's own page has it); here, and in Code Studio
//! through the relay (`code-arena ide --connect`), the folder is on this machine, so its ports are its own, at
//! localhost, and the view says so. What every part has (`register`, `take`) is said in `ext.rs`; what a view has
//! (`READY`, `view`, `actions`), in `layout.rs`.

use floem::reactive::SignalGet;
use floem::style::TextOverflow;
use floem::views::{Decorators, container, label};
use floem::{AnyView, IntoView};
use serde_json::Value;

use crate::state::{Send, Ui};

/// Whether the view is in the window.
pub const READY: bool = true;

/// What the view says: the old Code Arena's words for a folder on this machine.
pub const HERE: &str = "Here the folder is on this machine: what its programs listen on is at localhost already.";

/// The view's own content: the one sentence, wrapped, in the description's colour.
pub fn view(ui: Ui, _send: Send) -> AnyView {
    let t = ui.colors;
    container(label(|| HERE).style(move |s| {
        s.font_size(13.0).line_height(1.4).color(t.get().desc).text_overflow(TextOverflow::Wrap).min_width(0.0)
    }))
    .style(|s| s.size_full().min_height(0.0).padding_horiz(20.0).padding_vert(10.0))
    .into_any()
}

/// The actions in its title (none: there is nothing here to forward).
pub fn actions(_ui: Ui, _send: Send) -> Option<AnyView> {
    None
}

/// What it adds to the window's registries, once at start (nothing: the view's command is the layout's).
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's (none).
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use crate::layout::View;

    #[test]
    fn the_view_is_in_the_panel_with_its_words() {
        assert!(View::Ports.written());
        assert!(super::HERE.starts_with("Here the folder is on this machine"));
    }
}
