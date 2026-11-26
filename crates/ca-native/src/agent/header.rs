//! The Agent view's header, over the thread. Nothing is in it yet: it takes no room.

use floem::IntoView;
use floem::views::{Decorators, empty};

use crate::state::{Send, Ui};

pub(super) fn view(_ui: Ui, _send: Send) -> impl IntoView {
    empty().style(|s| s.hide())
}
