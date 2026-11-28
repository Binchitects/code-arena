//! Replying with a quote of part of an answer, as the old Code Arena does. Words selected in a finished answer get
//! a toolbar over them (under them on an answer's first line): Reply, Ask about this, and Copy ("Copied"). A press
//! on it keeps the words selected, and it moves with the thread as it scrolls.
//!
//! - Reply: the words become the quote of the next message, a chip over the box's text ("Quoted in your reply", two
//!   lines of it, "Cut to 4,000 characters." when it was longer, an X). Backspace at the start of an empty box takes
//!   it away too. It belongs to the session it was taken in (another session's box does not show or send it), goes
//!   with the next message (`send{quote}`), and comes back when the session refuses that message.
//! - Ask about this: a field for a question about the words, and "Explain this", "Say more", "Give an example", each
//!   sent at once with the words as its quote (queued while an answer is written). What is in the box stays as it is.
//! - Chat: Reply Quoting the Selection (Ctrl+Shift+'): Reply, by the keys.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use floem::action::exec_after;
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::kurbo::Rect;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, Position, TextOverflow};
use floem::views::{Decorators, dyn_container, empty, h_stack, h_stack_from_iter, label, stack, text_input, v_stack};
use floem::{AnyView, IntoView, View, ViewId};
use serde_json::{Value, json};

use super::slots::{self, Slot};
use crate::chat_bridge;
use crate::commands::{self, command};
use crate::icons::icon;
use crate::model::{Answer, Kind};
use crate::prose::{Picked, Quoting};
use crate::state::{Send, Ui};

/// The command that replies quoting the words selected in an answer.
pub const REPLY: &str = "workbench.action.chat.replyWithQuote";

/// The longest quote (the session cuts it the same).
pub const MOST: usize = 4000;

/// The questions "Ask about this" offers, each sent at once.
pub const ASKS: [&str; 3] = ["Explain this", "Say more", "Give an example"];

/// The height of the toolbar, and the room kept between it and the words.
const BAR: f64 = 30.0;
const GAP: f64 = 4.0;

/// The quote of the next message: its words (cut to `MOST`), whether they were cut, the session it belongs to.
#[derive(Clone, Debug, PartialEq)]
pub struct Quote {
    pub text: String,
    pub cut: bool,
    pub session: String,
}

/// Words selected in a finished answer: what they are, the answer (its words' signal), where they are in the
/// answer's view, that view's width, and the view of the block they are in (the keys go back to it).
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub text: String,
    pub answer: RwSignal<String>,
    pub rect: Rect,
    pub width: f64,
    pub view: ViewId,
}

#[derive(Clone, Copy)]
pub struct Quotes {
    pub quote: RwSignal<Option<Quote>>,
    /// The session the thread is of.
    pub session: RwSignal<String>,
    /// The words selected in an answer, none when none are.
    pub picked: RwSignal<Option<Pick>>,
    /// The toolbar asks a question about them, and the question typed.
    pub asking: RwSignal<bool>,
    pub question: RwSignal<String>,
    /// Copy was pressed a moment ago.
    pub copied: RwSignal<bool>,
    /// A press is on the toolbar: the words stay selected though they lose the keys.
    held: RwSignal<bool>,
    next: RwSignal<u64>,
}

impl crate::ext::State for Quotes {
    fn new(ui: Ui) -> Quotes {
        let s = ui.scope;
        Quotes {
            quote: s.create_rw_signal(None),
            session: s.create_rw_signal(String::new()),
            picked: s.create_rw_signal(None),
            asking: s.create_rw_signal(false),
            question: s.create_rw_signal(String::new()),
            copied: s.create_rw_signal(false),
            held: s.create_rw_signal(false),
            next: s.create_rw_signal(0),
        }
    }
}

/// A quote of these words for the session: tidied, trimmed and cut to `MOST` characters; none when nothing is left.
pub fn quote_of(words: &str, session: &str) -> Option<Quote> {
    let text = words.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let cut = text.chars().count() > MOST;
    let text = if cut { text.chars().take(MOST).collect::<String>().trim_end().to_string() } else { text.to_string() };
    Some(Quote { text, cut, session: session.to_string() })
}

/// The quote of the next message, when it is this session's.
fn shown(ui: Ui) -> Option<Quote> {
    let me = ui.state::<Quotes>();
    let session = me.session.get_untracked();
    me.quote.get_untracked().filter(|q| q.session == session)
}

/// The words become the quote of the next message, the box given the keys.
pub fn reply(ui: Ui, words: &str) {
    let me = ui.state::<Quotes>();
    if let Some(q) = quote_of(words, &me.session.get_untracked()) {
        me.quote.set(Some(q));
    }
    close(ui);
    chat_bridge::focus(ui);
}

/// The toolbar gone: no words picked, no question asked.
fn close(ui: Ui) {
    let me = ui.state::<Quotes>();
    if me.picked.with_untracked(Option::is_some) {
        me.picked.set(None);
    }
    if me.asking.get_untracked() {
        me.asking.set(false);
    }
    if !me.question.with_untracked(String::is_empty) {
        me.question.set(String::new());
    }
}

/// A question about the words picked, sent now with them as its quote (queued while an answer is written); the box
/// and its own quote are left as they are.
fn ask(ui: Ui, send: &Send, question: &str) {
    let me = ui.state::<Quotes>();
    let question = question.trim();
    let Some(pick) = me.picked.get_untracked() else { return };
    if question.is_empty() {
        return;
    }
    let Some(q) = quote_of(&pick.text, "") else { return };
    let n = me.next.get_untracked();
    me.next.set(n + 1);
    send(json!({"type": "send", "text": question, "quote": q.text, "key": format!("quote-{n}")}));
    close(ui);
}

/// Whether an answer is finished: not the one being written.
fn finished(ui: Ui, answer: RwSignal<String>) -> bool {
    !ui.running.get_untracked()
        || ui.items.with_untracked(|l| {
            l.iter().rev().find_map(|i| match &i.kind {
                Kind::Assistant(a) => Some(a.text),
                _ => None,
            }) != Some(answer)
        })
}

/// A press on the toolbar: the words stay selected through it (the words lose the keys on any press), and get them
/// back unless the press is on the question's field.
fn hold(ui: Ui, back: bool) {
    let me = ui.state::<Quotes>();
    me.held.set(true);
    if back && let Some(p) = me.picked.get_untracked() {
        p.view.request_focus();
    }
    exec_after(Duration::ZERO, move |_| me.held.set(false));
}

/// An answer's view with what quotes it: `body` is given what its words tell of a selection made in them, and the
/// toolbar is drawn over it at the words selected.
pub fn around(ui: Ui, send: Send, answer: Answer, body: impl FnOnce(Quoting) -> AnyView) -> AnyView {
    let me = ui.state::<Quotes>();
    let frame: Rc<Cell<Option<ViewId>>> = Rc::new(Cell::new(None));
    let at = frame.clone();
    let words = answer.text;
    let quoting = Quoting {
        picked: Rc::new(move |p: Option<Picked>| {
            if me.held.get_untracked() {
                return;
            }
            let pick = p.filter(|_| finished(ui, words)).and_then(|p| {
                let frame = at.get()?.layout_rect();
                let o = p.view.layout_rect().origin() - frame.origin();
                Some(Pick { text: p.text, answer: words, rect: p.rect + o, width: frame.width(), view: p.view })
            });
            match pick {
                Some(p) => {
                    if me.asking.get_untracked() {
                        me.asking.set(false);
                    }
                    me.picked.set(Some(p));
                }
                // Gone from this answer: another's stays.
                None if me.picked.with_untracked(|p| p.as_ref().is_some_and(|p| p.answer == words)) => close(ui),
                None => {}
            }
        }),
        // The words stay selected while a question is asked about them (its field has the keys).
        held: Rc::new(move || me.held.get_untracked() || me.asking.get_untracked()),
    };
    let content = body(quoting);
    frame.set(Some(content.id()));
    stack((content, toolbar(ui, send, words))).style(|s| s.width_full().min_width(0.0)).into_any()
}

/// Where the toolbar is in an answer's view: over the words, under them when there is no room over them; at their
/// start, but never past the view's right edge.
pub fn place(rect: Rect, width: f64, bar_width: f64) -> (f64, f64) {
    let top = if rect.y0 >= BAR + GAP { rect.y0 - BAR - GAP } else { rect.y1 + GAP };
    let left = rect.x0.min(width - bar_width).max(0.0);
    (left, top)
}

/// The toolbar over the words picked in this answer: its buttons, or the question being asked.
fn toolbar(ui: Ui, send: Send, answer: RwSignal<String>) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Quotes>();
    // Built only for the answer the words are in, and again only when it asks or stops asking.
    let shown = create_memo(move |_| {
        let mine = me.picked.with(|p| p.as_ref().is_some_and(|p| p.answer == answer));
        mine.then(|| me.asking.get())
    });
    let width = if_asking(320.0, 236.0);
    dyn_container(
        move || shown.get(),
        move |shown| match shown {
            None => empty().into_any(),
            Some(false) => buttons(ui).into_any(),
            Some(true) => asking(ui, send.clone()).into_any(),
        },
    )
    .style(move |s| {
        let c = t.get();
        // Another answer's toolbar reads no more than whether it is shown.
        let Some(asking) = shown.get() else { return s.position(Position::Absolute).hide() };
        let at = me.picked.with(|p| p.as_ref().map(|p| (p.rect, p.width)));
        let s = s
            .position(Position::Absolute)
            .padding(3.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(c.widget_border)
            .background(c.widget)
            .color(c.fg)
            .font_size(12.0)
            .box_shadow_blur(8.0)
            .box_shadow_color(floem::peniko::Color::rgba8(0, 0, 0, 0x40))
            .z_index(5);
        match at {
            Some((rect, w)) => {
                let (left, top) = place(rect, w, width(asking));
                s.inset_left(left).inset_top(top).apply_if(asking, |s| s.width(width(true)))
            }
            _ => s.hide(),
        }
    })
}

/// The toolbar's width, as it asks a question or not.
fn if_asking(asking: f64, not: f64) -> impl Fn(bool) -> f64 + Copy {
    move |a| if a { asking } else { not }
}

/// A button of the toolbar: its icon and words; it keeps the words selected while it is pressed.
fn button(ui: Ui, name: &'static str, words: impl Fn() -> String + 'static, on: impl Fn() + 'static) -> impl IntoView {
    let t = ui.colors;
    h_stack((icon(name, 14.0), label(words)))
        .on_event_cont(EventListener::PointerDown, move |_| hold(ui, true))
        .on_click_stop(move |_| on())
        .style(move |s| {
            s.items_center()
                .gap(5.0)
                .height(22.0)
                .padding_horiz(6.0)
                .border_radius(4.0)
                .cursor(CursorStyle::Pointer)
                .selectable(false)
                .hover(|s| s.background(t.get().toolbar_hover))
        })
}

fn buttons(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Quotes>();
    let keys = commands::keys_of(REPLY);
    let reply = button(
        ui,
        "reply",
        || "Reply".into(),
        move || {
            if let Some(p) = me.picked.get_untracked() {
                reply(ui, &p.text);
            }
        },
    );
    let tip = if keys.is_empty() { "Reply quoting this".to_string() } else { format!("Reply quoting this ({keys})") };
    let reply = floem::views::tooltip(reply, move || {
        label({
            let tip = tip.clone();
            move || tip.clone()
        })
    });
    let ask_about = button(ui, "comment-discussion", || "Ask about this".into(), move || me.asking.set(true));
    let copy = button(
        ui,
        "copy",
        move || if me.copied.get() { "Copied".into() } else { "Copy".into() },
        move || {
            if let Some(p) = me.picked.get_untracked() {
                let _ = floem::Clipboard::set_contents(p.text);
                if !me.copied.get_untracked() {
                    me.copied.set(true);
                    exec_after(Duration::from_millis(1500), move |_| me.copied.set(false));
                }
            }
        },
    );
    let line = empty().style(move |s| s.width(1.0).height(18.0).margin_horiz(2.0).background(t.get().widget_border));
    h_stack((reply, ask_about, line, copy)).style(|s| s.items_center().gap(1.0))
}

/// The question about the words: a field (Enter sends it, Escape closes it) and the questions offered.
fn asking(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Quotes>();
    let input = text_input(me.question).placeholder("Ask about this…");
    let id = input.id();
    exec_after(Duration::ZERO, move |_| id.request_focus());
    let on_enter = send.clone();
    let input = input
        .on_event_cont(EventListener::PointerDown, move |_| hold(ui, false))
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            match &k.key.logical_key {
                Key::Named(NamedKey::Enter) => ask(ui, &on_enter, &me.question.get_untracked()),
                Key::Named(NamedKey::Escape) => close(ui),
                _ => return EventPropagation::Continue,
            }
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            if !me.held.get_untracked() {
                close(ui);
            }
        })
        .style(move |s| {
            let c = t.get();
            let s = s
                .flex_grow(1.0_f32)
                .min_width(0.0)
                .height(26.0)
                .padding_horiz(6.0)
                .font_size(12.0)
                .border(1.0)
                .border_radius(4.0)
                .border_color(c.focus)
                .background(c.input_bg)
                .color(c.input_fg);
            crate::widgets::steady_input(s, c.input_bg, Some(c.focus))
        });
    let on_go = send.clone();
    let go = button(ui, "send", String::new, move || ask(ui, &on_go, &me.question.get_untracked()));
    let offered = h_stack_from_iter(ASKS.iter().map(|a| {
        let send = send.clone();
        label(move || a.to_string())
            .on_event_cont(EventListener::PointerDown, move |_| hold(ui, false))
            .on_click_stop(move |_| ask(ui, &send, a))
            .style(move |s| {
                let c = t.get();
                s.height(22.0)
                    .items_center()
                    .padding_horiz(7.0)
                    .border(1.0)
                    .border_radius(4.0)
                    .border_color(c.widget_border)
                    .font_size(11.0)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .hover(|s| s.background(c.toolbar_hover))
            })
    }))
    .style(|s| s.gap(4.0).flex_wrap(floem::style::FlexWrap::Wrap));
    v_stack((h_stack((input, go)).style(|s| s.items_center().gap(4.0).width_full()), offered))
        .style(|s| s.gap(4.0).padding(2.0).width_full())
}

/// The chip over the box's text: the quote of the next message, and the X that takes it away.
fn chip(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Quotes>();
    let now = create_memo(move |_| {
        let session = me.session.get();
        me.quote.get().filter(|q| q.session == session)
    });
    let x = floem::views::tooltip(
        floem::views::container(icon("close", 14.0)).on_click_stop(move |_| me.quote.set(None)).style(move |s| {
            s.padding(3.0)
                .border_radius(4.0)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(t.get().toolbar_hover))
        }),
        || label(|| "Remove the quote"),
    );
    h_stack((
        icon("quote", 14.0).style(move |s| s.color(t.get().desc).margin_top(2.0)),
        v_stack((
            label(|| "Quoted in your reply").style(move |s| s.font_size(11.0).color(t.get().desc)),
            label(move || now.with(|q| q.as_ref().map(|q| super::question::two_lines(&q.text)).unwrap_or_default()))
                .style(move |s| s.font_size(12.0).line_height(1.4).color(t.get().fg).text_overflow(TextOverflow::Wrap)),
            label(move || format!("Cut to {} characters.", super::answer::thousands(MOST as u64))).style(move |s| {
                s.font_size(11.0)
                    .color(t.get().desc)
                    .apply_if(!now.with(|q| q.as_ref().is_some_and(|q| q.cut)), |s| s.hide())
            }),
        ))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).gap(1.0)),
        x,
    ))
    .style(move |s| {
        let c = t.get();
        s.width_full()
            .items_start()
            .gap(6.0)
            .margin_bottom(6.0)
            .padding_vert(4.0)
            .padding_left(8.0)
            .padding_right(3.0)
            .border_left(2.0)
            .border_color(c.quote_border)
            .border_radius(4.0)
            .background(c.widget)
            .apply_if(now.with(Option::is_none), |s| s.hide())
    })
}

/// What it adds to the window's registries, once at start: the chip over the box's text, its share of the next
/// message, Backspace in an empty box, and the command that replies by the keys.
pub fn register(_ui: Ui, _send: &Send) {
    slots::add(Slot::BoxTop, "quote", 10, |ui, _send| chip(ui).into_any());
    chat_bridge::sent_with(
        "quote",
        10,
        chat_bridge::with(|ui, msg| {
            if let Some(q) = shown(ui) {
                msg["quote"] = json!(q.text);
                ui.state::<Quotes>().quote.set(None);
            }
        })
        .back(|ui, msg| {
            let me = ui.state::<Quotes>();
            if let Some(words) = msg["quote"].as_str()
                && shown(ui).is_none()
                && let Some(q) = quote_of(words, &me.session.get_untracked())
            {
                me.quote.set(Some(Quote { cut: words.chars().count() >= MOST, ..q }));
            }
        }),
    );
    chat_bridge::on_key("quote", 10, |ui, _send, key, mods| {
        let backspace = matches!(key, Key::Named(NamedKey::Backspace)) && mods.is_empty();
        if backspace && shown(ui).is_some() && super::composer::text(ui).is_empty() {
            ui.state::<Quotes>().quote.set(None);
            return true;
        }
        false
    });
    commands::add(
        command(REPLY, "Chat", "Reply Quoting the Selection", |ui, _| {
            if let Some(p) = ui.state::<Quotes>().picked.get_untracked() {
                reply(ui, &p.text);
            }
        })
        .keys("Ctrl+Shift+'")
        .enabled(|ui| ui.state::<Quotes>().picked.with_untracked(Option::is_some)),
    );
}

/// The session's messages that are this part's, taken in: the session the thread is of. Never taken away from the
/// rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "state"
        && let Some(session) = m["session"].as_str()
    {
        let me = ui.state::<Quotes>();
        if me.session.with_untracked(|s| s != session) {
            me.session.set(session.to_string());
            close(ui);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use floem::keyboard::Modifiers;

    use super::*;

    fn sender() -> (Send, Rc<RefCell<Vec<Value>>>) {
        let said = Rc::new(RefCell::new(Vec::new()));
        let to = said.clone();
        (Rc::new(move |v| to.borrow_mut().push(v)), said)
    }

    fn in_session(ui: Ui, send: &Send, id: &str) {
        take(ui, &json!({"type": "state", "session": id}), send);
    }

    #[test]
    fn a_quote_is_tidied_and_cut_to_four_thousand_characters() {
        assert_eq!(quote_of("  a\r\nb  ", "s"), Some(Quote { text: "a\nb".into(), cut: false, session: "s".into() }));
        assert_eq!(quote_of(" \n ", "s"), None);
        let q = quote_of(&"é".repeat(MOST + 1), "s").expect("a quote");
        assert!(q.cut && q.text.chars().count() == MOST);
        assert!(!quote_of(&"é".repeat(MOST), "s").expect("a quote").cut);
    }

    #[test]
    fn the_toolbar_stands_over_the_words_under_them_on_the_first_line_and_inside_the_answer() {
        let words = Rect::new(40.0, 100.0, 90.0, 118.0);
        assert_eq!(place(words, 600.0, 236.0), (40.0, 100.0 - BAR - GAP));
        let first = Rect::new(40.0, 0.0, 90.0, 18.0);
        assert_eq!(place(first, 600.0, 236.0), (40.0, 18.0 + GAP));
        let right = Rect::new(500.0, 100.0, 590.0, 118.0);
        assert_eq!(place(right, 600.0, 236.0).0, 364.0);
        assert_eq!(place(right, 200.0, 236.0).0, 0.0);
    }

    #[test]
    fn a_reply_goes_with_the_next_message_of_its_session_and_comes_back_when_it_is_refused() {
        let ui = Ui::new();
        let (send, _) = sender();
        register(ui, &send);
        in_session(ui, &send, "a");
        reply(ui, " the parser reads tokens ");
        let me = ui.state::<Quotes>();
        assert_eq!(shown(ui).map(|q| q.text), Some("the parser reads tokens".into()));
        // Another session's box neither shows nor sends it; back in its own, it is there again.
        in_session(ui, &send, "b");
        let mut msg = json!({"type": "send", "text": "why"});
        chat_bridge::fill(ui, &mut msg);
        assert!(msg.get("quote").is_none() && shown(ui).is_none());
        in_session(ui, &send, "a");
        chat_bridge::fill(ui, &mut msg);
        assert_eq!(msg["quote"], "the parser reads tokens");
        assert!(me.quote.get_untracked().is_none(), "it went with the message");
        chat_bridge::came_back(ui, &msg);
        assert_eq!(shown(ui).map(|q| q.text), Some("the parser reads tokens".into()));
    }

    #[test]
    fn backspace_in_an_empty_box_takes_the_quote_away() {
        let ui = Ui::new();
        let (send, _) = sender();
        register(ui, &send);
        reply(ui, "a part");
        let backspace = Key::Named(NamedKey::Backspace);
        ui.draft.set("x".into());
        assert!(!chat_bridge::key(ui, &send, &backspace, Modifiers::empty()));
        ui.draft.set(String::new());
        assert!(!chat_bridge::key(ui, &send, &Key::Named(NamedKey::Delete), Modifiers::empty()));
        assert!(chat_bridge::key(ui, &send, &backspace, Modifiers::empty()));
        assert!(shown(ui).is_none());
        assert!(!chat_bridge::key(ui, &send, &backspace, Modifiers::empty()), "nothing left to take away");
    }

    #[test]
    fn asking_about_words_sends_them_as_the_quote_at_once_and_leaves_the_box_alone() {
        let ui = Ui::new();
        let (send, said) = sender();
        let me = ui.state::<Quotes>();
        reply(ui, "kept for the box");
        let view = floem::views::empty().id();
        let words = ui.scope.create_rw_signal(String::new());
        me.picked.set(Some(Pick { text: "the loop".into(), answer: words, rect: Rect::ZERO, width: 0.0, view }));
        me.asking.set(true);
        ask(ui, &send, "  ");
        assert!(said.borrow().is_empty());
        ask(ui, &send, ASKS[0]);
        let sent = said.borrow().clone();
        assert_eq!((&sent[0]["text"], &sent[0]["quote"]), (&json!("Explain this"), &json!("the loop")));
        assert!(sent[0]["key"].as_str().is_some_and(|k| k.starts_with("quote-")));
        assert!(me.picked.get_untracked().is_none() && !me.asking.get_untracked());
        assert_eq!(shown(ui).map(|q| q.text), Some("kept for the box".into()));
    }

    #[test]
    fn the_reply_keys_are_ctrl_shift_quote() {
        assert!(commands::parse_keys("Ctrl+Shift+'").is_some());
    }
}
