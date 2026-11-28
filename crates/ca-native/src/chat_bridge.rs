//! What the rest of the window asks of the Agent view's input: text put in its box, lines of a file sent with the
//! next message, a question asked at once, the keys given to it. The editor, the Explorer, Problems and the others
//! call these and know nothing of the box; the box (the composer) says how each is done, once, with `install`.
//! Until it has, each does nothing.
//!
//! ```ignore
//! chat_bridge::add_piece(ui, Piece { path: "src/cart.rs".into(), start: 12, end: 14, text });
//! chat_bridge::ask_now(ui, &send, "Explain this");
//! ```
//!
//! And what the chat's own parts ask of the box without editing its file, each added from a part's `register` (one
//! added again under the same id takes the first's place):
//!
//! - `on_key`: a key pressed in the box is offered to the part before the box acts on it (a menu open over the box
//!   takes the arrows, Enter, Tab and Escape so). True: the key was the part's, and the box does nothing with it.
//! - `sent_with`: a part's share of the box's next message. Its `fill` is given the message on its way out and adds
//!   its fields (`context`, `quote`, `attachments`), taking them out of the box; its `more` says there is something
//!   to send though no word is typed (the send button is lit, and `fill` then gives the message its words); its
//!   `back` is given a message the session refused, to put back what `fill` took; its `ready` false holds the
//!   message back (a file still on its way up: the send button is dim, Enter sends nothing).
//! - `on_drop`: the part given files dropped on the box from the system (`dropped` reaches it).
//! - `pieces_by`: the part that keeps the lines of files sent with the next message (`add_piece` reaches it).
//! - `caret`, `place`, `replace`: where the caret is in the box's text and in the window, where the box is, and a
//!   part of its text made another (a name completed). The box's text itself is `ui.draft`.
//!
//! ```ignore
//! chat_bridge::on_key("slash", 10, |ui, _, key, _| menu_key(ui, key));
//! chat_bridge::sent_with("quote", 10, chat_bridge::with(|ui, msg| msg["quote"] = taken(ui)).back(put_back));
//! ```

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use floem::keyboard::{Key, Modifiers};
use floem::kurbo::Rect;
use serde_json::Value;

/// Lines of a file sent with the next message: the file, its first and last line (from 1), and their text.
pub use crate::model::Piece;
use crate::state::{Send, Ui};

/// What the box does with a text.
pub type Text = Rc<dyn Fn(Ui, &str)>;
/// What sends a text as a message.
pub type Ask = Rc<dyn Fn(Ui, &Send, &str)>;
/// What adds lines of a file to what the next message is sent with.
pub type AddPiece = Rc<dyn Fn(Ui, Piece)>;
/// What makes a part of the box's text (from, to, in bytes) another.
pub type Replace = Rc<dyn Fn(Ui, usize, usize, &str)>;
/// What fills a message on its way out with a part's share of it.
pub type Fill = Rc<dyn Fn(Ui, &mut Value)>;
/// What is given a message the session refused.
pub type Back = Rc<dyn Fn(Ui, &Value)>;

/// How the box does what is asked of it.
#[derive(Clone)]
pub struct Bridge {
    /// The box's text made this (and the keys given to the box).
    pub put_in_box: Text,
    /// This put before what is in the box.
    pub insert_at_start: Text,
    /// Lines of a file added to what the next message is sent with.
    pub add_piece: AddPiece,
    /// This sent as a message now (queued when an answer is being written), what is in the box left alone.
    pub ask_now: Ask,
    /// The keys given to the box, its view shown.
    pub focus: Rc<dyn Fn(Ui)>,
    /// Where the caret is; none while the box is not drawn.
    pub caret: Rc<dyn Fn(Ui) -> Option<Caret>>,
    /// Where the box is in the window; none while it is not drawn.
    pub place: Rc<dyn Fn(Ui) -> Option<Rect>>,
    /// A part of the box's text (from, to, in bytes) made this, the caret put after it.
    pub replace: Replace,
}

/// The caret in the box: where it is in the text (in bytes), and the place of its row in the window (a menu is put
/// over or under it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Caret {
    pub offset: usize,
    pub rect: Rect,
}

/// What is offered a key pressed in the box; true: it took it.
pub type OnKey = Rc<dyn Fn(Ui, &Send, &Key, Modifiers) -> bool>;

/// A part's share of the box's next message.
#[derive(Clone)]
pub struct With {
    /// Whether there is something of the part's to send, though no word is typed.
    pub more: Rc<dyn Fn(Ui) -> bool>,
    /// The message on its way out: the part's fields added to it, and taken out of the box.
    pub fill: Fill,
    /// A message the session refused: what `fill` took is put back.
    pub back: Back,
    /// Whether the part lets the message go now (false: a file still on its way up). Read where it is followed.
    pub ready: Rc<dyn Fn(Ui) -> bool>,
}

/// A part's share of the box's next message: what fills it. `more` and `back` are added to it when it has them.
pub fn with(fill: impl Fn(Ui, &mut Value) + 'static) -> With {
    With { more: Rc::new(|_| false), fill: Rc::new(fill), back: Rc::new(|_, _| {}), ready: Rc::new(|_| true) }
}

impl With {
    pub fn more(mut self, test: impl Fn(Ui) -> bool + 'static) -> With {
        self.more = Rc::new(test);
        self
    }

    pub fn back(mut self, back: impl Fn(Ui, &Value) + 'static) -> With {
        self.back = Rc::new(back);
        self
    }

    pub fn ready(mut self, test: impl Fn(Ui) -> bool + 'static) -> With {
        self.ready = Rc::new(test);
        self
    }
}

/// What a part added under its id, and its order among the others.
struct Added<T> {
    id: &'static str,
    order: i32,
    what: T,
}

type Kept<T> = RefCell<Vec<Added<T>>>;

/// What is given a file dropped on the box.
type OnDrop = Rc<dyn Fn(Ui, &Send, &std::path::Path)>;

thread_local! {
    static DROP: RefCell<Option<OnDrop>> = const { RefCell::new(None) };
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
    static KEYS: Kept<OnKey> = const { RefCell::new(Vec::new()) };
    static WITH: Kept<With> = const { RefCell::new(Vec::new()) };
    static PIECES: RefCell<Option<AddPiece>> = const { RefCell::new(None) };
    static ASKING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn put<T>(list: &mut Vec<Added<T>>, new: Added<T>) {
    match list.iter().position(|old| old.id == new.id) {
        Some(at) => list[at] = new,
        None => list.push(new),
    }
    // A stable sort: as added, within an order.
    list.sort_by_key(|a| a.order);
}

/// What was added, taken out before any of it is called: what is called may add more.
fn all<T: Clone>(list: &'static std::thread::LocalKey<Kept<T>>) -> Vec<T> {
    list.with(|l| l.borrow().iter().map(|a| a.what.clone()).collect())
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

/// The part that keeps the lines of files sent with the next message says how one is added.
pub fn pieces_by(add: impl Fn(Ui, Piece) + 'static) {
    PIECES.with(|p| *p.borrow_mut() = Some(Rc::new(add)));
}

pub fn add_piece(ui: Ui, piece: Piece) {
    // The part that keeps them, when there is one; else the box's own way.
    if let Some(add) = PIECES.with(|p| p.borrow().clone()) {
        add(ui, piece);
    } else if let Some(b) = bridge() {
        (b.add_piece)(ui, piece);
    }
}

/// A question asked at once. While it is sent, `asking_now` holds: what waits in the box for the next message (the
/// lines of files) stays there.
pub fn ask_now(ui: Ui, send: &Send, text: &str) {
    if let Some(b) = bridge() {
        ASKING.with(|a| a.set(true));
        (b.ask_now)(ui, send, text);
        ASKING.with(|a| a.set(false));
    }
}

/// Whether the message being filled is a question asked at once (`ask_now`), not the box's.
pub fn asking_now() -> bool {
    ASKING.with(std::cell::Cell::get)
}

pub fn focus(ui: Ui) {
    if let Some(b) = bridge() {
        (b.focus)(ui);
    }
}

pub fn caret(ui: Ui) -> Option<Caret> {
    bridge().and_then(|b| (b.caret)(ui))
}

pub fn place(ui: Ui) -> Option<Rect> {
    bridge().and_then(|b| (b.place)(ui))
}

pub fn replace(ui: Ui, from: usize, to: usize, text: &str) {
    if let Some(b) = bridge() {
        (b.replace)(ui, from, to, text);
    }
}

/// Adds what is offered each key pressed in the box before the box acts on it, the lower order first.
pub fn on_key(id: &'static str, order: i32, key: impl Fn(Ui, &Send, &Key, Modifiers) -> bool + 'static) {
    KEYS.with(|k| put(&mut k.borrow_mut(), Added { id, order, what: Rc::new(key) as OnKey }));
}

/// A key pressed in the box, offered to each part in turn; true when one took it.
pub fn key(ui: Ui, send: &Send, key: &Key, mods: Modifiers) -> bool {
    all(&KEYS).into_iter().any(|k| k(ui, send, key, mods))
}

/// Adds a part's share of the box's next message, the lower order filling first.
pub fn sent_with(id: &'static str, order: i32, what: With) {
    WITH.with(|w| put(&mut w.borrow_mut(), Added { id, order, what }));
}

/// Whether a part has something to send though no word is typed. Read where it is followed (the send button's
/// look): the signals a part's `more` reads tell it.
pub fn more(ui: Ui) -> bool {
    all(&WITH).into_iter().any(|w| (w.more)(ui))
}

/// Whether every part lets the box's message go now: while one does not, the send button is dim and Enter sends
/// nothing.
pub fn ready(ui: Ui) -> bool {
    all(&WITH).into_iter().all(|w| (w.ready)(ui))
}

/// Adds what is given files dropped on the box from the system (one added again takes the first's place).
pub fn on_drop(drop: impl Fn(Ui, &Send, &std::path::Path) + 'static) {
    DROP.with(|d| *d.borrow_mut() = Some(Rc::new(drop)));
}

/// A file dropped on the box from the system, given to the part that takes them; false when none does.
pub fn dropped(ui: Ui, send: &Send, path: &std::path::Path) -> bool {
    let Some(drop) = DROP.with(|d| d.borrow().clone()) else { return false };
    drop(ui, send, path);
    true
}

/// The box's message on its way out, given to each part to fill.
pub fn fill(ui: Ui, msg: &mut Value) {
    for w in all(&WITH) {
        (w.fill)(ui, msg);
    }
}

/// A message the session refused, given to each part to put back what it took.
pub fn came_back(ui: Ui, msg: &Value) {
    for w in all(&WITH) {
        (w.back)(ui, msg);
    }
}

#[cfg(test)]
mod tests {
    use floem::keyboard::NamedKey;
    use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
    use serde_json::json;

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
        replace(ui, 0, 0, "lost");
        assert!(ui.draft.get_untracked().is_empty() && said.borrow().is_empty());
        assert!(caret(ui).is_none() && place(ui).is_none());
        let pieces = Rc::new(RefCell::new(Vec::new()));
        let got = pieces.clone();
        install(Bridge {
            put_in_box: Rc::new(|ui, text| ui.draft.set(text.to_string())),
            insert_at_start: Rc::new(|ui, text| ui.draft.update(|d| d.insert_str(0, text))),
            add_piece: Rc::new(move |_, piece| got.borrow_mut().push(piece)),
            ask_now: Rc::new(|_, send, text| send(json!({"type": "send", "text": text}))),
            focus: Rc::new(|ui| ui.message.set("focused".into())),
            caret: Rc::new(|ui| Some(Caret { offset: ui.draft.with_untracked(String::len), rect: Rect::ZERO })),
            place: Rc::new(|_| Some(Rect::new(0.0, 0.0, 10.0, 10.0))),
            replace: Rc::new(|ui, from, to, text| ui.draft.update(|d| d.replace_range(from..to, text))),
        });
        assert!(installed());
        put_in_box(ui, "world");
        insert_at_start(ui, "hello ");
        assert_eq!(ui.draft.get_untracked(), "hello world");
        assert_eq!(caret(ui).map(|c| c.offset), Some(11));
        assert_eq!(place(ui).map(|r| r.width()), Some(10.0));
        replace(ui, 0, 5, "bye");
        assert_eq!(ui.draft.get_untracked(), "bye world");
        add_piece(ui, Piece { path: "a.rs".into(), start: 1, end: 2, text: "x\ny".into() });
        assert_eq!(pieces.borrow().len(), 1);
        ask_now(ui, &send, "Explain this");
        assert_eq!(*said.borrow(), [json!({"type": "send", "text": "Explain this"})]);
        focus(ui);
        assert_eq!(ui.message.get_untracked(), "focused");
        // The part that keeps the pieces has them from then on, not the box.
        pieces_by(|ui, piece| ui.message.set(piece.path));
        add_piece(ui, Piece { path: "b.rs".into(), start: 1, end: 1, text: String::new() });
        assert_eq!((pieces.borrow().len(), ui.message.get_untracked()), (1, "b.rs".to_string()));
    }

    #[test]
    fn a_key_is_offered_by_order_until_a_part_takes_it() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let enter = Key::Named(NamedKey::Enter);
        assert!(!key(ui, &send, &enter, Modifiers::empty()));
        on_key("test.late", 20, |ui, _, _, _| {
            ui.message.update(|m| m.push('b'));
            true
        });
        on_key("test.first", 10, |ui, _, key, _| {
            ui.message.update(|m| m.push('a'));
            matches!(key, Key::Named(NamedKey::Escape))
        });
        // The lower order is asked first; the one after it only when the first left the key.
        assert!(key(ui, &send, &enter, Modifiers::empty()));
        assert_eq!(ui.message.get_untracked(), "ab");
        assert!(key(ui, &send, &Key::Named(NamedKey::Escape), Modifiers::empty()));
        assert_eq!(ui.message.get_untracked(), "aba");
        // One added again under the same id takes the first's place.
        on_key("test.late", 20, |_, _, _, _| false);
        assert!(!key(ui, &send, &enter, Modifiers::empty()));
    }

    #[test]
    fn a_parts_share_of_a_message_is_filled_in_and_put_back_when_it_is_refused() {
        let ui = Ui::new();
        assert!(!more(ui));
        let mut msg = json!({"type": "send", "text": ""});
        fill(ui, &mut msg);
        assert_eq!(msg, json!({"type": "send", "text": ""}));
        // A part with a quote waiting: something to send with no word typed, and words for it then.
        ui.message.set("the quote".into());
        sent_with(
            "test.quote",
            10,
            with(|ui, msg| {
                msg["quote"] = json!(ui.message.get_untracked());
                if msg["text"] == "" {
                    msg["text"] = json!("Look at this.");
                }
                ui.message.set(String::new());
            })
            .more(|ui| !ui.message.get_untracked().is_empty())
            .back(|ui, msg| ui.message.set(msg["quote"].as_str().unwrap_or("").to_string())),
        );
        assert!(more(ui));
        fill(ui, &mut msg);
        assert_eq!(msg, json!({"type": "send", "text": "Look at this.", "quote": "the quote"}));
        assert!(!more(ui) && ui.message.get_untracked().is_empty());
        came_back(ui, &msg);
        assert_eq!(ui.message.get_untracked(), "the quote");
    }
}
