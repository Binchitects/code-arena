//! The places of the Agent view where its parts draw without editing each other's files: around the input's box,
//! and with a request or an answer in the thread. A part adds what it draws from its `register` (see `ext.rs`):
//!
//! ```ignore
//! slots::add(Slot::BoxTop, "pieces", 20, |ui, send| chips(ui, send).into_any());
//! slots::add_item(ItemSlot::AnswerFoot, "rating", 30, |ui, send, item| thumbs(ui, send, item).into_any());
//! ```
//!
//! Around the box (`Slot`), from the top down:
//!
//! - `Bars`: rows between the thread and the box, each a bar of its own (`agent::bar` is their look). The orders in
//!   use: the plan 10, the goal 20, the commands with no time limit 30; loops 40, the queue's note 50.
//! - `BoxTop`: inside the box, over its text: what is sent with the next message. A quote 10, pieces of files 20,
//!   attachments 30.
//! - `BoxLeft`: in the box's toolbar, after the pickers. The tools picker 10.
//! - `BoxRight`: in the toolbar, before the send button. The context ring 10, the paperclip 20.
//! - `BoxFoot`: under the box, outside it: a line of hints.
//!
//! With an item of the thread (`ItemSlot`); what is drawn is given the item (its `kind` has the request's details
//! or the answer's signals, see `model.rs`):
//!
//! - `QuestionTop`: over a request's bubble, at the right: its quote 10, "Sent while it worked" 20.
//! - `QuestionFoot`: under the bubble, at the right: the chips of what was sent with it 10, its attachments 20, its
//!   actions (copy, edit, rewind) 30.
//! - `AnswerTop`: over an answer's calls and words: its thinking 10.
//! - `AnswerFoot`: under an answer's words: "N files changed" 10, its footer (copy, model, time, tokens) 20, its
//!   rating 30, its actions 40.
//!
//! The conventions:
//!
//! - What is added hides itself (`s.hide()`, on its own signals) when it has nothing to show, and brings its own
//!   space (a margin): a place adds none, so a place with nothing shown takes no room.
//! - A place with nothing added is not built at all: the view is as it was before there were places.
//! - The places are filled at start, from `register`; what is added later is not drawn. One added again under the
//!   same id takes the place of the first.
//! - Within a place: by `order` (the lower first), then as added.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use floem::views::{Decorators, h_stack_from_iter, v_stack_from_iter};
use floem::{AnyView, IntoView};

use crate::model::Item;
use crate::state::{Send, Ui};

/// A place around the input's box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Bars,
    BoxTop,
    BoxLeft,
    BoxRight,
    BoxFoot,
}

/// A place with an item of the thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemSlot {
    QuestionTop,
    QuestionFoot,
    AnswerTop,
    AnswerFoot,
}

type Draw = Rc<dyn Fn(Ui, Send) -> AnyView>;
type DrawItem = Rc<dyn Fn(Ui, Send, &Item) -> AnyView>;

struct Added<P, D> {
    place: P,
    id: &'static str,
    order: i32,
    draw: D,
}

thread_local! {
    static AROUND: RefCell<Vec<Added<Slot, Draw>>> = const { RefCell::new(Vec::new()) };
    static WITH_ITEM: RefCell<Vec<Added<ItemSlot, DrawItem>>> = const { RefCell::new(Vec::new()) };
}

fn put<P: PartialEq, D>(list: &mut Vec<Added<P, D>>, new: Added<P, D>) {
    match list.iter().position(|old| old.place == new.place && old.id == new.id) {
        Some(at) => list[at] = new,
        None => list.push(new),
    }
}

/// What a place holds, in the order drawn: its ids and what draws each.
fn held<P: PartialEq + Copy, D: Clone>(list: &[Added<P, D>], place: P) -> Vec<(&'static str, D)> {
    let mut mine: Vec<&Added<P, D>> = list.iter().filter(|a| a.place == place).collect();
    // A stable sort: as added, within an order.
    mine.sort_by_key(|a| a.order);
    mine.into_iter().map(|a| (a.id, a.draw.clone())).collect()
}

/// Adds what is drawn in a place around the box; one of the same id there takes the first's place.
pub fn add(slot: Slot, id: &'static str, order: i32, draw: impl Fn(Ui, Send) -> AnyView + 'static) {
    AROUND.with(|a| put(&mut a.borrow_mut(), Added { place: slot, id, order, draw: Rc::new(draw) }));
}

/// Adds what is drawn in a place with an item of the thread; one of the same id there takes the first's place.
pub fn add_item(slot: ItemSlot, id: &'static str, order: i32, draw: impl Fn(Ui, Send, &Item) -> AnyView + 'static) {
    WITH_ITEM.with(|a| put(&mut a.borrow_mut(), Added { place: slot, id, order, draw: Rc::new(draw) }));
}

/// The ids of what a place around the box holds, in the order drawn.
pub fn ids(slot: Slot) -> Vec<&'static str> {
    AROUND.with(|a| held(&a.borrow(), slot)).into_iter().map(|(id, _)| id).collect()
}

/// The ids of what a place with an item holds, in the order drawn.
pub fn item_ids(slot: ItemSlot) -> Vec<&'static str> {
    WITH_ITEM.with(|a| held(&a.borrow(), slot)).into_iter().map(|(id, _)| id).collect()
}

/// What a place around the box holds, drawn (none: nothing was added to it).
pub fn views(ui: Ui, send: &Send, slot: Slot) -> Vec<AnyView> {
    // Taken out of the registry first: what draws may look into it.
    let all = AROUND.with(|a| held(&a.borrow(), slot));
    all.into_iter().map(|(_, draw)| draw(ui, send.clone())).collect()
}

/// What a place with an item holds, drawn for that item (none: nothing was added to it).
pub fn item_views(ui: Ui, send: &Send, slot: ItemSlot, item: &Item) -> Vec<AnyView> {
    let all = WITH_ITEM.with(|a| held(&a.borrow(), slot));
    all.into_iter().map(|(_, draw)| draw(ui, send.clone(), item)).collect()
}

/// A place around the box as a column of what it holds, the whole width; none when nothing was added to it.
pub fn column(ui: Ui, send: &Send, slot: Slot) -> Option<AnyView> {
    let all = views(ui, send, slot);
    (!all.is_empty()).then(|| v_stack_from_iter(all).style(|s| s.width_full().min_width(0.0)).into_any())
}

/// A place around the box as a row of what it holds; none when nothing was added to it.
pub fn row(ui: Ui, send: &Send, slot: Slot) -> Option<AnyView> {
    let all = views(ui, send, slot);
    (!all.is_empty()).then(|| h_stack_from_iter(all).style(|s| s.items_center().gap(2.0)).into_any())
}

/// A place with an item as a column of what it holds, the whole width (a request's at the right); none when
/// nothing was added to it.
pub fn item_column(ui: Ui, send: &Send, slot: ItemSlot, item: &Item) -> Option<AnyView> {
    let all = item_views(ui, send, slot, item);
    let right = matches!(slot, ItemSlot::QuestionTop | ItemSlot::QuestionFoot);
    (!all.is_empty()).then(|| {
        v_stack_from_iter(all)
            .style(move |s| s.width_full().min_width(0.0).apply_if(right, |s| s.items_end()))
            .into_any()
    })
}

/// An item of the thread with what its places hold over and under it. With nothing added to either place, the
/// item's own view as it is.
pub fn around_item(ui: Ui, send: &Send, top: ItemSlot, own: AnyView, foot: ItemSlot, item: &Item) -> AnyView {
    let over = item_column(ui, send, top, item);
    let under = item_column(ui, send, foot, item);
    if over.is_none() && under.is_none() {
        return own;
    }
    let parts: Vec<AnyView> = over.into_iter().chain([own]).chain(under).collect();
    v_stack_from_iter(parts).style(|s| s.width_full().min_width(0.0)).into_any()
}

#[cfg(test)]
mod tests {
    use floem::views::empty;

    use super::*;
    use crate::model::{Kind, Question};

    #[test]
    fn a_place_holds_what_was_added_by_order_and_one_added_again_takes_the_firsts_place() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let nothing = |_: Ui, _: Send| empty().into_any();
        // Nothing added: nothing built.
        assert!(column(ui, &send, Slot::BoxFoot).is_none() && row(ui, &send, Slot::BoxFoot).is_none());
        add(Slot::BoxTop, "test.pieces", 20, nothing);
        add(Slot::BoxTop, "test.quote", 10, nothing);
        add(Slot::BoxTop, "test.files", 20, nothing);
        add(Slot::BoxRight, "test.ring", 10, nothing);
        let mine = |slot| ids(slot).into_iter().filter(|i| i.starts_with("test.")).collect::<Vec<_>>();
        assert_eq!(mine(Slot::BoxTop), ["test.quote", "test.pieces", "test.files"]);
        assert_eq!(mine(Slot::BoxRight), ["test.ring"]);
        add(Slot::BoxTop, "test.quote", 30, nothing);
        assert_eq!(mine(Slot::BoxTop), ["test.pieces", "test.files", "test.quote"]);
        assert!(column(ui, &send, Slot::BoxTop).is_some());
        assert_eq!(views(ui, &send, Slot::BoxTop).len(), ids(Slot::BoxTop).len());
    }

    #[test]
    fn what_is_drawn_with_an_item_is_given_the_item() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let seen = Rc::new(RefCell::new(Vec::new()));
        let to = seen.clone();
        add_item(ItemSlot::QuestionFoot, "test.actions", 30, move |_, _, item| {
            if let Kind::User(q) = &item.kind {
                to.borrow_mut().push((item.id, q.text.clone()));
            }
            empty().into_any()
        });
        let item = Item { id: 7, kind: Kind::User(Question::of("hello")) };
        let own = empty().into_any();
        let _ = around_item(ui, &send, ItemSlot::QuestionTop, own, ItemSlot::QuestionFoot, &item);
        assert_eq!(*seen.borrow(), [(7, "hello".to_string())]);
        assert_eq!(item_ids(ItemSlot::QuestionFoot), ["test.actions"]);
        assert!(item_column(ui, &send, ItemSlot::AnswerTop, &item).is_none());
    }
}
