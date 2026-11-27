//! What the window's parts draw over the whole window, and in rows under its title bar, without editing the
//! workbench: a dialog, the toasts, the Help overlay, a menu opened at the pointer; a banner. A part adds its layer
//! from its `register` (see `ext.rs`), and the workbench draws them all, once, when the window is built:
//!
//! ```ignore
//! layers::add(Place::Over, "dialog", 30, |ui, send| dialog_host(ui, send).into_any());
//! layers::add(Place::Banner, "no-model", 10, |ui, send| banner(ui, send).into_any());
//! ```
//!
//! The conventions:
//!
//! - `Place::Over`: over the window's own layers (the title bar's menus, a view's menu, the dock's guide, the quick
//!   input), under the strips the window is resized by. A layer places itself (`s.absolute()` and its insets) and
//!   is hidden (`s.hide()`, on its own signal) while it shows nothing: one that fills the window and is not hidden
//!   takes every click from what is under it. One that is small and always there (the toasts' corner) sizes itself
//!   to what it shows. The orders in use, the lower drawn first (so under the higher): the Help overlay 10, a peek
//!   or a hover 20, dialogs 30, a menu opened at a place 40, the toasts 50, the "Opening…" cover 60. The window's
//!   own menus have z-index 100; a layer that must be over them says so in its style.
//! - `Place::Banner`: a row the window's whole width between the title bar and the rest, the lower order nearer the
//!   title bar. A banner hides itself while it has nothing to say, and then takes no room.
//! - The layers are built once, at start, from what was added in `register`: one added later is not drawn. One
//!   added again under the same id takes the place of the first.
//! - A layer's keys: the view that has the focus has the keys. A layer that opens asks for them
//!   (`id.request_focus()`), and gives them back to what had them when it closes.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use floem::AnyView;

use crate::state::{Send, Ui};

/// Where a layer is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    /// Over the window.
    Over,
    /// A row under the title bar.
    Banner,
}

struct Layer {
    place: Place,
    id: &'static str,
    order: i32,
    draw: Rc<dyn Fn(Ui, Send) -> AnyView>,
}

thread_local! {
    static LAYERS: RefCell<Vec<Layer>> = const { RefCell::new(Vec::new()) };
}

/// Adds a layer; one of the same id in the same place takes the first's place.
pub fn add(place: Place, id: &'static str, order: i32, draw: impl Fn(Ui, Send) -> AnyView + 'static) {
    let layer = Layer { place, id, order, draw: Rc::new(draw) };
    LAYERS.with(|l| {
        let mut layers = l.borrow_mut();
        match layers.iter().position(|old| old.place == place && old.id == id) {
            Some(at) => layers[at] = layer,
            None => layers.push(layer),
        }
    });
}

/// A place's layers in the order drawn (by their orders; equal ones as added): their ids and what draws each.
fn held(place: Place) -> Vec<(&'static str, Rc<dyn Fn(Ui, Send) -> AnyView>)> {
    LAYERS.with(|l| {
        let layers = l.borrow();
        let mut mine: Vec<&Layer> = layers.iter().filter(|layer| layer.place == place).collect();
        mine.sort_by_key(|layer| layer.order);
        mine.into_iter().map(|layer| (layer.id, layer.draw.clone())).collect()
    })
}

/// The ids of a place's layers, in the order drawn.
pub fn ids(place: Place) -> Vec<&'static str> {
    held(place).into_iter().map(|(id, _)| id).collect()
}

/// A place's layers, drawn (the workbench's, once).
pub fn views(ui: Ui, send: &Send, place: Place) -> Vec<AnyView> {
    held(place).into_iter().map(|(_, draw)| draw(ui, send.clone())).collect()
}

#[cfg(test)]
mod tests {
    use floem::IntoView;
    use floem::views::empty;

    use super::*;

    #[test]
    fn a_places_layers_come_by_their_order_and_one_added_again_takes_the_firsts_place() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let nothing = |_: Ui, _: Send| empty().into_any();
        assert!(views(ui, &send, Place::Banner).is_empty());
        add(Place::Over, "test.toasts", 50, nothing);
        add(Place::Over, "test.dialog", 30, nothing);
        add(Place::Over, "test.menu", 30, nothing);
        add(Place::Banner, "test.no-model", 10, nothing);
        assert_eq!(ids(Place::Over), ["test.dialog", "test.menu", "test.toasts"]);
        assert_eq!(ids(Place::Banner), ["test.no-model"]);
        add(Place::Over, "test.dialog", 60, nothing);
        assert_eq!(ids(Place::Over), ["test.menu", "test.toasts", "test.dialog"]);
        assert_eq!(views(ui, &send, Place::Over).len(), 3);
    }
}
