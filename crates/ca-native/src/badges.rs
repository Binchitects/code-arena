//! The activity bar's badges: a count on a view's icon (problems, changed files, tasks at work), as VS Code draws
//! them. The part that knows the count sets it, and the activity bar draws it while it is more than none:
//!
//! ```ignore
//! badges::set(ui, View::Problems, errors);
//! ```
//!
//! A count is a signal of the window's own (made in its long-lived scope, kept with the window), so setting it draws
//! the one badge again and nothing else; it is set only when it changes.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::collections::HashMap;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};

use crate::ext::State;
use crate::layout::{VIEWS, View};
use crate::state::Ui;

/// Every view's count.
#[derive(Clone, Copy)]
struct Badges(RwSignal<HashMap<View, RwSignal<usize>>>);

impl State for Badges {
    fn new(ui: Ui) -> Badges {
        Badges(ui.scope.create_rw_signal(VIEWS.into_iter().map(|v| (v, ui.scope.create_rw_signal(0))).collect()))
    }
}

/// A view's count, to read where it is drawn.
pub fn count(ui: Ui, v: View) -> RwSignal<usize> {
    let all = ui.state::<Badges>().0;
    if let Some(s) = all.with_untracked(|m| m.get(&v).copied()) {
        return s;
    }
    let s = ui.scope.create_rw_signal(0);
    all.update(|m| {
        m.insert(v, s);
    });
    s
}

/// Sets a view's count (none: no badge).
pub fn set(ui: Ui, v: View, n: usize) {
    let s = count(ui, v);
    if s.get_untracked() != n {
        s.set(n);
    }
}

/// A count as the badge writes it: VS Code's "99+" past 99.
pub fn text(n: usize) -> String {
    if n > 99 { "99+".to_string() } else { n.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_views_count_is_kept_with_its_window_and_written_as_vs_code_writes_it() {
        let ui = Ui::new();
        assert_eq!(count(ui, View::Changes).get_untracked(), 0);
        set(ui, View::Changes, 3);
        set(ui, View::Changes, 3);
        assert_eq!(count(ui, View::Changes).get_untracked(), 3);
        assert_eq!(count(ui, View::Files).get_untracked(), 0);
        // Another window's counts are its own.
        assert_eq!(count(Ui::new(), View::Changes).get_untracked(), 0);
        assert_eq!(text(7), "7");
        assert_eq!(text(99), "99");
        assert_eq!(text(100), "99+");
    }
}
