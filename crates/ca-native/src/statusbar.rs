//! The status bar's items: a part of the window adds its own from its `register` (see `ext.rs`), and the bar draws
//! them, the left ones from the left and the right ones after them, each side by its priorities (the lower first,
//! so the higher is nearer the window's right edge):
//!
//! ```ignore
//! statusbar::add("status.problems", Side::Left, 120, |ui, _send| {
//!     let t = ui.colors;
//!     h_stack((icon("error", 16.0), label(move || …))).style(move |s| statusbar::item(t, s)).into_any()
//! });
//! ```
//!
//! The priorities in use, so a new item finds its place. Left: remote 90, branch 100, sync 110, problems 120, agent
//! changes 130, jobs 140, project 150, terminal 155, chord 160, the window's message 170. Right: Ln/Col 200,
//! indentation 210, encoding 220, end of line 230, language 240, formatter 250, language server 260, Complete 270,
//! context 280, last turn 285, mode 290, model 291, MCP 292, connection 297, version 298, bell 299.
//!
//! An item hides itself when it has nothing to show (its own style, on its own signals). The bar is built once, at
//! start, from the items there are then: add from `register`, not later. An item added again under the same id takes
//! the place of the first.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use floem::AnyView;
use floem::reactive::SignalGet;
use floem::style::Style;

use crate::state::{Send, Ui};
use crate::widgets::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone)]
pub struct Item {
    pub id: &'static str,
    pub side: Side,
    pub priority: u32,
    pub view: Rc<dyn Fn(Ui, Send) -> AnyView>,
}

thread_local! {
    static ITEMS: RefCell<Vec<Item>> = const { RefCell::new(Vec::new()) };
}

/// Adds an item to the bar; one of the same id takes the first's place.
pub fn add(id: &'static str, side: Side, priority: u32, view: impl Fn(Ui, Send) -> AnyView + 'static) {
    let item = Item { id, side, priority, view: Rc::new(view) };
    ITEMS.with(|i| {
        let mut items = i.borrow_mut();
        match items.iter().position(|old| old.id == id) {
            Some(at) => items[at] = item,
            None => items.push(item),
        }
    });
}

/// A side's items, in the order they are drawn (by priority; equal ones as added).
pub fn items(side: Side) -> Vec<Item> {
    let mut all: Vec<Item> = ITEMS.with(|i| i.borrow().iter().filter(|i| i.side == side).cloned().collect());
    all.sort_by_key(|i| i.priority);
    all
}

/// An item's look, as VS Code's: the bar's height, 5 px within and 2 px between, lit under the pointer.
pub fn item(t: Theme, s: Style) -> Style {
    let c = t.get();
    s.height_full().items_center().gap(4.0).padding_horiz(5.0).margin_horiz(2.0).hover(|s| s.background(c.status_hover))
}

#[cfg(test)]
mod tests {
    use floem::IntoView;
    use floem::views::empty;

    use super::*;

    #[test]
    fn a_sides_items_come_by_priority_and_one_added_again_takes_the_firsts_place() {
        let view = |_: Ui, _: Send| empty().into_any();
        add("test.b", Side::Left, 120, view);
        add("test.a", Side::Left, 100, view);
        add("test.right", Side::Right, 10, view);
        add("test.c", Side::Left, 120, view);
        let ids = |side| items(side).into_iter().map(|i| i.id).filter(|i| i.starts_with("test.")).collect::<Vec<_>>();
        assert_eq!(ids(Side::Left), ["test.a", "test.b", "test.c"]);
        assert_eq!(ids(Side::Right), ["test.right"]);
        add("test.b", Side::Left, 300, view);
        assert_eq!(ids(Side::Left), ["test.a", "test.c", "test.b"]);
    }
}
