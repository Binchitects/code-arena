//! What the rest of the window asks of the Agent view's input: text put in its box, lines of a file sent with the
//! next message, a question asked at once, the keys given to it. The editor, the Explorer, Problems and the others
//! call these and know nothing of the box; the box (the composer) says how each is done, once, with `install`.
//! Until it has, each does nothing.
//!
//! ```ignore
//! chat_bridge::add_piece(ui, Piece { path: "src/cart.rs".into(), start: 12, end: 14, text });
//! chat_bridge::ask_now(ui, &send, "Explain this");
//! ```

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

/// Lines of a file sent with the next message: the file, its first and last line (from 1), and their text.
pub use crate::model::Piece;
use crate::state::{Send, Ui};

/// What the box does with a text.
pub type Text = Rc<dyn Fn(Ui, &str)>;
/// What sends a text as a message.
pub type Ask = Rc<dyn Fn(Ui, &Send, &str)>;

/// How the box does what is asked of it.
#[derive(Clone)]
pub struct Bridge {
    /// The box's text made this (and the keys given to the box).
    pub put_in_box: Text,
    /// This put before what is in the box.
    pub insert_at_start: Text,
    /// Lines of a file added to what the next message is sent with.
    pub add_piece: Rc<dyn Fn(Ui, Piece)>,
    /// This sent as a message now (queued when an answer is being written), what is in the box left alone.
    pub ask_now: Ask,
    /// The keys given to the box, its view shown.
    pub focus: Rc<dyn Fn(Ui)>,
}

thread_local! {
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
}

/// The box says how it does each (from the composer's `register`).
pub fn install(bridge: Bridge) {
    BRIDGE.with(|b| *b.borrow_mut() = Some(bridge));
}

/// Whether the box has said how.
pub fn installed() -> bool {
    BRIDGE.with(|b| b.borrow().is_some())
}

/// The bridge taken out before it is called: what it calls may install another.
fn bridge() -> Option<Bridge> {
    BRIDGE.with(|b| b.borrow().clone())
}

pub fn put_in_box(ui: Ui, text: &str) {
    if let Some(b) = bridge() {
        (b.put_in_box)(ui, text);
    }
}

pub fn insert_at_start(ui: Ui, text: &str) {
    if let Some(b) = bridge() {
        (b.insert_at_start)(ui, text);
    }
}

pub fn add_piece(ui: Ui, piece: Piece) {
    if let Some(b) = bridge() {
        (b.add_piece)(ui, piece);
    }
}

pub fn ask_now(ui: Ui, send: &Send, text: &str) {
    if let Some(b) = bridge() {
        (b.ask_now)(ui, send, text);
    }
}

pub fn focus(ui: Ui) {
    if let Some(b) = bridge() {
        (b.focus)(ui);
    }
}

#[cfg(test)]
mod tests {
    use floem::reactive::{SignalGet, SignalUpdate};
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn nothing_is_done_until_the_box_says_how_and_then_each_reaches_it() {
        let ui = Ui::new();
        let said = Rc::new(RefCell::new(Vec::<Value>::new()));
        let to = said.clone();
        let send: Send = Rc::new(move |v| to.borrow_mut().push(v));
        assert!(!installed());
        put_in_box(ui, "lost");
        ask_now(ui, &send, "lost");
        focus(ui);
        assert!(ui.draft.get_untracked().is_empty() && said.borrow().is_empty());
        let pieces = Rc::new(RefCell::new(Vec::new()));
        let got = pieces.clone();
        install(Bridge {
            put_in_box: Rc::new(|ui, text| ui.draft.set(text.to_string())),
            insert_at_start: Rc::new(|ui, text| ui.draft.update(|d| d.insert_str(0, text))),
            add_piece: Rc::new(move |_, piece| got.borrow_mut().push(piece)),
            ask_now: Rc::new(|_, send, text| send(json!({"type": "send", "text": text}))),
            focus: Rc::new(|ui| ui.message.set("focused".into())),
        });
        assert!(installed());
        put_in_box(ui, "world");
        insert_at_start(ui, "hello ");
        assert_eq!(ui.draft.get_untracked(), "hello world");
        add_piece(ui, Piece { path: "a.rs".into(), start: 1, end: 2, text: "x\ny".into() });
        assert_eq!(pieces.borrow().len(), 1);
        ask_now(ui, &send, "Explain this");
        assert_eq!(*said.borrow(), [json!({"type": "send", "text": "Explain this"})]);
        focus(ui);
        assert_eq!(ui.message.get_untracked(), "focused");
    }
}
