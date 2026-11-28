//! The lines of files sent with the next message: a chip each over the chat's box ("cart.rs:12-30", "(unsaved)"
//! when the file has changes not saved, X to leave it out), and a dashed "+ cart.rs:12" that offers the editor's
//! selection (or its caret's line). Eight at most; the same lines added again take their chip's place.
//!
//! The editor's "Code Arena: Add to chat" (Ctrl+L) adds its selection; "Explain this", "Fix this" and "Complete
//! this" ask about it at once (queued while an answer is written), the chips waiting in the box left alone. The
//! Explorer's "Add to Chat" puts the row's `@path` in the box.
//!
//! What goes to the session: `send{text, context: [{path, start, end, text?}]}` (the text only of an unsaved
//! file's lines; the driver reads the rest from the file, with three lines around). Lines sent with no words go as
//! "Look at these lines."; a message the session refuses gives its pieces back to the box. The person's own files
//! (settings, keybindings) and tabs that are not a file's text are never offered.

use std::cell::RefCell;

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, FlexWrap};
use floem::views::editor::core::buffer::rope_text::{RopeText, RopeTextRef};
use floem::views::editor::core::cursor::CursorMode;
use floem::views::{Decorators, dyn_stack, h_stack, label, tooltip};
use serde_json::{Value, json};

use super::slots::{self, Slot};
use crate::chat_bridge::{self, Piece};
use crate::commands::{self, command};
use crate::icons::icon;
use crate::menus::{self, Menu, item};
use crate::state::{Send, Ui};

pub const ADD: &str = "codeArena.addToChat";
pub const EXPLAIN: &str = "codeArena.explainThis";
pub const FIX: &str = "codeArena.fixThis";
pub const COMPLETE: &str = "codeArena.completeThis";
const ADD_ROW: &str = "codeArena.addFileToChat";

/// The most pieces sent with a message.
pub const MOST: usize = 8;
/// The words of a message sent with lines and nothing typed.
pub const LOOK: &str = "Look at these lines.";

/// Lines of a file waiting in the box: the piece, and whether its file had changes not saved when it was added
/// (then its text is sent, as the person sees it).
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub id: u64,
    pub piece: Piece,
    pub unsaved: bool,
}

/// This part's state: the chips over the box, and the number the next one is given.
#[derive(Clone, Copy)]
pub struct Pieces {
    list: RwSignal<Vec<Chip>>,
    next: RwSignal<u64>,
}

impl crate::ext::State for Pieces {
    fn new(ui: Ui) -> Pieces {
        Pieces { list: ui.scope.create_rw_signal(Vec::new()), next: ui.scope.create_rw_signal(0) }
    }
}

thread_local! {
    /// The piece an Explain, Fix or Complete is sent with, in place of the chips, while it is sent.
    static ONCE: RefCell<Option<Chip>> = const { RefCell::new(None) };
}

/// A chip added to the list: in the place of one of the same file whose lines it shares, else at the end, the
/// oldest left out past `MOST`.
pub fn put(list: &mut Vec<Chip>, chip: Chip) {
    let (path, start, end) = (chip.piece.path.clone(), chip.piece.start, chip.piece.end);
    let same = |c: &Chip| c.piece.path == path && c.piece.start <= end && start <= c.piece.end;
    match list.iter().position(same) {
        Some(at) => {
            list[at] = chip;
            // Another of the same lines further on goes.
            let mut seen = false;
            let kept = std::mem::take(list);
            for c in kept {
                if same(&c) {
                    if seen {
                        continue;
                    }
                    seen = true;
                }
                list.push(c);
            }
        }
        None => list.push(chip),
    }
    while list.len() > MOST {
        list.remove(0);
    }
}

/// A chip as it reads: "src/cart.rs:12-30", "src/cart.rs:7", "(unsaved)" after it.
pub fn name(chip: &Chip) -> String {
    let p = &chip.piece;
    let lines = if p.end > p.start { format!("{}-{}", p.start, p.end) } else { p.start.to_string() };
    let unsaved = if chip.unsaved { " (unsaved)" } else { "" };
    format!("{}:{lines}{unsaved}", p.path)
}

/// A piece as the session is sent it.
pub fn context_of(chip: &Chip) -> Value {
    let p = &chip.piece;
    let mut v = json!({"path": p.path, "start": p.start, "end": p.end});
    if chip.unsaved {
        v["text"] = json!(p.text);
    }
    v
}

/// The message on its way out: the piece of an Explain (or the chips, taken out of the box) in its `context`,
/// and words when it has none.
fn fill(ui: Ui, msg: &mut Value) {
    let me = ui.state::<Pieces>();
    let chips = match ONCE.with(|o| o.borrow().clone()) {
        Some(once) => vec![once],
        None => {
            let chips = me.list.get_untracked();
            if !chips.is_empty() {
                me.list.set(Vec::new());
            }
            chips
        }
    };
    if chips.is_empty() {
        return;
    }
    msg["context"] = Value::Array(chips.iter().map(context_of).collect());
    if msg["text"].as_str().is_none_or(|t| t.trim().is_empty()) {
        msg["text"] = json!(LOOK);
    }
}

/// A message the session refused: its pieces back over the box.
fn back(ui: Ui, msg: &Value) {
    let Some(sent) = msg["context"].as_array().filter(|c| !c.is_empty()) else { return };
    for v in sent {
        let piece = Piece::from_json(v);
        let unsaved = v["text"].is_string();
        add(ui, piece, unsaved);
    }
}

/// Lines added over the box.
pub fn add(ui: Ui, piece: Piece, unsaved: bool) {
    if piece.path.is_empty() || piece.start == 0 {
        return;
    }
    let me = ui.state::<Pieces>();
    let id = me.next.get_untracked();
    me.next.set(id + 1);
    me.list.update(|l| put(l, Chip { id, piece, unsaved }));
}

/// The lines of the editor shown: its selection's (a selection that ends at a line's start leaves that line out),
/// else its caret's line; none for a tab that is not a file of the folder's text (the person's own files, a diff, a
/// preview).
pub fn current(ui: Ui) -> Option<Chip> {
    let (path, e) = ui.live.get_untracked()?;
    let tab = ui.tab(&path)?;
    if tab.kind.is_some() || tab.diff.is_some() || tab.file.is_empty() {
        return None;
    }
    let (from, to) = e.cursor.with_untracked(|c| match &c.mode {
        CursorMode::Insert(sel) => {
            let r = sel.last_inserted().map_or((c.offset(), c.offset()), |r| (r.min(), r.max()));
            (r.0, r.1)
        }
        _ => (c.offset(), c.offset()),
    });
    let rope = e.doc().text();
    let t = RopeTextRef::new(&rope);
    let (a, b) = lines_of(&t, from, to);
    let unsaved = tab.dirty.get_untracked();
    let text = if unsaved {
        let end = t.offset_of_line(b + 1).max(t.offset_of_line(b));
        let end = if b + 1 > t.last_line() { t.len() } else { end };
        rope.slice_to_cow(t.offset_of_line(a)..end).trim_end_matches(['\n', '\r']).to_string()
    } else {
        String::new()
    };
    Some(Chip { id: 0, piece: Piece { path: tab.file.clone(), start: a + 1, end: b + 1, text }, unsaved })
}

/// The lines (from 0) a selection from one offset to another takes.
fn lines_of(t: &RopeTextRef, from: usize, to: usize) -> (usize, usize) {
    let a = t.line_of_offset(from);
    let mut b = t.line_of_offset(to);
    if b > a && t.offset_of_line(b) == to {
        b -= 1;
    }
    (a, b)
}

/// The editor's lines over the box, the keys there.
fn add_current(ui: Ui) {
    if let Some(chip) = current(ui) {
        add(ui, chip.piece, chip.unsaved);
        chat_bridge::focus(ui);
    }
}

/// The editor's lines asked about at once, with these words: the chips in the box stay for the next message.
pub fn ask(ui: Ui, send: &Send, words: &str) {
    let Some(chip) = current(ui) else { return };
    ONCE.with(|o| *o.borrow_mut() = Some(chip));
    chat_bridge::ask_now(ui, send, words);
    ONCE.with(|o| *o.borrow_mut() = None);
    ui.reveal(crate::layout::View::Agent);
}

/// The chips over the box, and the offer of the editor's lines; nothing while there are neither.
fn chips(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Pieces>();
    // What the editor offers, followed: its editor, its caret and selection, its file's changes.
    let offer = create_memo(move |_| {
        let live = ui.live.get();
        if let Some((path, e)) = &live {
            e.cursor.track();
            if let Some(tab) = ui.tab(path) {
                tab.dirty.track();
            }
        }
        let chip = current(ui)?;
        let taken = me.list.with(|l| {
            l.iter().any(|c| {
                c.piece.path == chip.piece.path && (c.piece.start, c.piece.end) == (chip.piece.start, chip.piece.end)
            })
        });
        (!taken).then(|| (chip.piece.path.clone(), chip.piece.start, chip.piece.end, chip.unsaved))
    });
    let open = send.clone();
    let row = dyn_stack(move || me.list.get(), |c| c.id, move |c| chip_view(ui, &open, c).into_any())
        .style(|s| s.gap(4.0).flex_wrap(FlexWrap::Wrap).min_width(0.0));
    let offered = label(move || {
        offer.get().map_or(String::new(), |(path, start, end, unsaved)| {
            let chip = Chip { id: 0, piece: Piece { path, start, end, text: String::new() }, unsaved };
            format!("+ {}", name(&chip))
        })
    })
    .on_click_stop(move |_| add_current(ui))
    .style(move |s| {
        let c = t.get();
        s.font_size(12.0)
            .padding_horiz(6.0)
            .padding_vert(1.0)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(4.0)
            .color(c.desc)
            .cursor(CursorStyle::Pointer)
            .text_ellipsis()
            .max_width_full()
            .hover(|s| s.color(c.fg).background(c.list_hover))
            .apply_if(offer.with(Option::is_none), |s| s.hide())
    });
    let offered = tooltip(offered, || label(|| "Add the editor's lines to what is sent"));
    let empty = create_memo(move |_| me.list.with(Vec::is_empty) && offer.with(Option::is_none));
    h_stack((row, offered)).style(move |s| {
        s.gap(4.0)
            .flex_wrap(FlexWrap::Wrap)
            .items_center()
            .width_full()
            .min_width(0.0)
            .padding_horiz(4.0)
            .padding_top(4.0)
            .apply_if(empty.get(), |s| s.hide())
    })
}

/// One chip: its file's icon and name (a click opens its lines), and X.
fn chip_view(ui: Ui, send: &Send, chip: Chip) -> impl IntoView {
    let t = ui.colors;
    let words = name(&chip);
    let (path, start, id) = (chip.piece.path.clone(), chip.piece.start, chip.id);
    let send = send.clone();
    let out = format!("Leave out {path}");
    let close = icon("close", 12.0)
        .on_click_stop(move |_| ui.state::<Pieces>().list.update(|l| l.retain(|c| c.id != id)))
        .style(move |s| {
            let c = t.get();
            s.border_radius(3.0).cursor(CursorStyle::Pointer).hover(|s| s.background(c.toolbar_hover))
        });
    let close = tooltip(close, move || {
        label({
            let out = out.clone();
            move || out.clone()
        })
    });
    h_stack((
        icon("file", 14.0),
        label(move || words.clone())
            .on_click_stop(move |_| ui.open_at(&path, start, &send))
            .style(|s| s.font_size(12.0).text_ellipsis().min_width(0.0).cursor(CursorStyle::Pointer)),
        close,
    ))
    .style(move |s| {
        let c = t.get();
        s.gap(4.0)
            .items_center()
            .padding_left(6.0)
            .padding_right(2.0)
            .padding_vert(1.0)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(4.0)
            .max_width_full()
    })
}

/// What it adds to the window's registries, once at start: the chips over the box and their share of the next
/// message, the editor's commands (with their keys and places in its right-click menu), and the Explorer's.
pub fn register(ui: Ui, _send: &Send) {
    chat_bridge::pieces_by(|ui, piece| {
        let unsaved = !piece.text.is_empty();
        add(ui, piece, unsaved);
    });
    chat_bridge::sent_with(
        "pieces",
        20,
        chat_bridge::with(fill).more(|ui| ui.state::<Pieces>().list.with(|l| !l.is_empty())).back(back),
    );
    slots::add(Slot::BoxTop, "pieces", 20, |ui, send| chips(ui, send).into_any());
    let offered = |ui: Ui| current(ui).is_some();
    commands::add(command(ADD, "Code Arena", "Add to chat", |ui, _| add_current(ui)).keys("Ctrl+L").when(offered));
    let asks = [(EXPLAIN, "Explain this"), (FIX, "Fix this"), (COMPLETE, "Complete this")];
    for (id, words) in asks {
        commands::add(command(id, "Code Arena", words, move |ui, send| ask(ui, send, words)).when(offered));
    }
    for (order, id) in [ADD, EXPLAIN, FIX, COMPLETE].into_iter().enumerate() {
        let title = commands::find(id).map(|c| c.label()).unwrap_or_default();
        menus::add(Menu::EditorContext, item(id).title(&title).group("1_chat").order(order as i32));
    }
    // The Explorer's row: its path as a mention before what is typed.
    let row = |ui: Ui| !ui.selected.get_untracked().is_empty();
    commands::add(
        command(ADD_ROW, "Code Arena", "Add File to Chat", |ui, _| {
            let path = ui.selected.get_untracked();
            if !path.is_empty() {
                chat_bridge::insert_at_start(ui, &super::mentions::mention(&path, "", true));
                chat_bridge::focus(ui);
            }
        })
        .when(row)
        .hidden(),
    );
    menus::add(Menu::ExplorerRow, item(ADD_ROW).title("Add to Chat").group("3_chat"));
    let _ = ui;
}

/// The session's messages that are this part's, taken in: none (what is sent comes back through the box).
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(id: u64, path: &str, start: usize, end: usize) -> Chip {
        Chip { id, piece: Piece { path: path.into(), start, end, text: String::new() }, unsaved: false }
    }

    #[test]
    fn the_same_lines_take_their_chips_place_and_the_oldest_go_past_eight() {
        let mut list = Vec::new();
        put(&mut list, chip(0, "a.rs", 1, 5));
        put(&mut list, chip(1, "b.rs", 1, 5));
        put(&mut list, chip(2, "a.rs", 4, 9));
        assert_eq!(list.iter().map(|c| c.id).collect::<Vec<_>>(), [2, 1]);
        put(&mut list, chip(3, "a.rs", 20, 20));
        assert_eq!(list.len(), 3);
        for id in 4..12 {
            put(&mut list, chip(id, "c.rs", id as usize * 10, id as usize * 10));
        }
        assert_eq!(list.len(), MOST);
        assert_eq!(list[0].id, 4, "the oldest went");
    }

    #[test]
    fn a_chip_reads_as_its_file_and_lines_and_is_sent_with_its_text_only_unsaved() {
        let mut c = chip(0, "src/cart.rs", 12, 30);
        assert_eq!(name(&c), "src/cart.rs:12-30");
        assert_eq!(context_of(&c), json!({"path": "src/cart.rs", "start": 12, "end": 30}));
        c.piece.end = 12;
        c.unsaved = true;
        c.piece.text = "let x = 1;".into();
        assert_eq!(name(&c), "src/cart.rs:12 (unsaved)");
        assert_eq!(context_of(&c)["text"], "let x = 1;");
    }

    #[test]
    fn the_chips_go_with_a_message_and_come_back_when_it_is_refused_but_an_explain_leaves_them() {
        let ui = Ui::new();
        let me = ui.state::<Pieces>();
        add(ui, Piece { path: "a.rs".into(), start: 3, end: 4, text: String::new() }, false);
        add(ui, Piece { path: "b.rs".into(), start: 7, end: 7, text: "x".into() }, true);
        // Asked about other lines at once: the chips stay.
        ONCE.with(|o| *o.borrow_mut() = Some(chip(9, "c.rs", 1, 2)));
        let mut msg = json!({"type": "send", "text": "Explain this"});
        fill(ui, &mut msg);
        ONCE.with(|o| *o.borrow_mut() = None);
        assert_eq!(msg["context"], json!([{"path": "c.rs", "start": 1, "end": 2}]));
        assert_eq!(msg["text"], "Explain this");
        assert_eq!(me.list.with_untracked(Vec::len), 2);
        // Sent with no words: they go, and say so.
        let mut msg = json!({"type": "send", "text": ""});
        fill(ui, &mut msg);
        assert_eq!(msg["text"], LOOK);
        assert_eq!(
            msg["context"],
            json!([{"path": "a.rs", "start": 3, "end": 4}, {"path": "b.rs", "start": 7, "end": 7, "text": "x"}])
        );
        assert!(me.list.with_untracked(Vec::is_empty));
        // Refused: they come back as they were.
        back(ui, &msg);
        let names: Vec<String> = me.list.with_untracked(|l| l.iter().map(name).collect());
        assert_eq!(names, ["a.rs:3-4", "b.rs:7 (unsaved)"]);
        // Nothing waiting: the message is as it was.
        me.list.set(Vec::new());
        let mut msg = json!({"type": "send", "text": "hi"});
        fill(ui, &mut msg);
        assert_eq!(msg, json!({"type": "send", "text": "hi"}));
    }

    #[test]
    fn ctrl_l_in_an_editor_adds_its_line_and_the_persons_own_files_are_not_offered() {
        use std::rc::Rc;

        use floem::keyboard::{Key, Modifiers};
        use floem::views::editor::text_document::TextDocument;

        crate::icons::load();
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        crate::editor::editing::register(ui, &send);
        register(ui, &send);
        let ed = floem::views::text_editor("one\ntwo\nthree\n");
        let tab = |path: &str, kind: Option<&'static str>| crate::state::Tab {
            path: path.into(),
            file: path.into(),
            colours: crate::highlight::Colours::new(Rc::new(TextDocument::new(ui.scope, "")), ""),
            doc: Rc::new(TextDocument::new(ui.scope, "")),
            dirty: ui.scope.create_rw_signal(false),
            diff: None,
            kind,
        };
        ui.tabs.set(vec![tab("src/a.rs", None), tab("user:settings.json", Some("user"))]);
        ui.live.set(Some(("src/a.rs".into(), ed.editor().clone())));
        let me = ui.state::<Pieces>();
        commands::press(ui, &send, &Key::Character("l".into()), Modifiers::CONTROL);
        let names: Vec<String> = me.list.with_untracked(|l| l.iter().map(name).collect());
        assert_eq!(names, ["src/a.rs:1"]);
        ui.live.set(Some(("user:settings.json".into(), ed.editor().clone())));
        assert!(current(ui).is_none());
        ui.live.set(None);
        assert!(current(ui).is_none());
    }

    #[test]
    fn a_selection_ending_at_a_lines_start_leaves_that_line_out() {
        let rope = floem::views::editor::core::xi_rope::Rope::from("one\ntwo\nthree\n");
        let t = RopeTextRef::new(&rope);
        assert_eq!(lines_of(&t, 0, 8), (0, 1));
        assert_eq!(lines_of(&t, 0, 9), (0, 2));
        assert_eq!(lines_of(&t, 5, 5), (1, 1));
    }
}
