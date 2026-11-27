//! The messages sent while an answer is written, waiting to be read: dashed bubbles at the thread's end, each
//! changed in place (Enter keeps the words, Escape leaves them) or taken back until the answer reads it, and all
//! of them taken back at once with Clear all.
//!
//! The session numbers each (`queued{id}`) and says when it is read: at the answer's next step (`took_in{id}`: the
//! bubble becomes the question it is, marked "Sent while it worked"), or as a turn of its own after the answer's
//! end (`user{late, queued: [id]}`: the question takes the bubble's place). What waits is said again whenever it
//! changes (`queue{items}`), so a window that comes in late, or another window, shows the same.
//!
//! One that came too late for the answer (and a command, which waits whole for its end) is answered by a turn of
//! its own, begun with its question said again (`user` with `late`): the question takes the place of the bubble
//! that waited. Several that waited are said again as one question, their words one under the other. A session
//! that does not number them is followed by their words.

use std::time::Duration;

use floem::action::exec_after;
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::kurbo::Stroke;
use floem::peniko::Brush;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, dyn_container, empty, h_stack, label, text_input, tooltip, v_stack};
use floem::{AnyView, IntoView, View};
use serde_json::{Value, json};

use crate::ext::State;
use crate::icons::icon;
use crate::model::{Item, Kind, Question};
use crate::state::{Send, Ui};

/// What a queued message says when the session could not change or take it back: the answer read it first.
pub const NOT_CHANGED: &str = "The message could not be changed.";

/// The note over the messages that wait.
pub const NOTE: &str = "The answer reads them at its next step; a command waits for the answer to end.";

/// The queue's state in a window: the bubble being changed (its item's number) and its words, and the message the
/// session could not change (its number as the session gave it).
#[derive(Clone, Copy)]
struct Queue {
    editing: RwSignal<Option<u64>>,
    draft: RwSignal<String>,
    failed: RwSignal<Option<u64>>,
}

impl State for Queue {
    fn new(ui: Ui) -> Queue {
        Queue {
            editing: ui.scope.create_rw_signal(None),
            draft: ui.scope.create_rw_signal(String::new()),
            failed: ui.scope.create_rw_signal(None),
        }
    }
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are a message queued, taken in: one queued, one read by the answer, what waits now,
/// one that could not be changed, and the question of a turn that says again what waited. Never taken away from
/// the rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let q = ui.state::<Queue>();
    match m["type"].as_str().unwrap_or("") {
        "queued" => {
            ui.push(Kind::Queued { id: m["id"].clone(), question: Question::from_json(m), later: m["later"] == true })
        }
        "took_in" => taken_in(ui, m),
        "queue" => {
            let items = m["items"].as_array().cloned().unwrap_or_default();
            now_waiting(ui, &items);
            if q.failed.get_untracked().is_some_and(|f| m["id"].as_u64() == Some(f)) {
                q.failed.set(None);
            }
        }
        "queue_error" => {
            q.editing.set(None);
            let id = m["id"].as_u64();
            if id.is_some_and(|id| at(ui, id).is_some()) {
                q.failed.set(id);
            } else {
                crate::widgets::toast::error(ui, NOT_CHANGED);
            }
        }
        // The question itself is in the thread already (`question.rs` is given the message first).
        "user" if m["late"] == true => {
            let numbered: Vec<u64> =
                m["queued"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
            let gone = ui.items.with_untracked(|l| {
                if numbered.is_empty() {
                    said_again(l, m["text"].as_str().unwrap_or(""))
                } else {
                    l.iter().filter(|i| queued_id(i).is_some_and(|id| numbered.contains(&id))).map(|i| i.id).collect()
                }
            });
            if !gone.is_empty() {
                ui.items.update(|l| l.retain(|i| !gone.contains(&i.id)));
            }
        }
        "state" if q.editing.get_untracked().is_some() => q.editing.set(None),
        _ => {}
    }
    false
}

/// A queued item's number as the session gave it (none: not numbered, or not queued).
fn queued_id(i: &Item) -> Option<u64> {
    match &i.kind {
        Kind::Queued { id, .. } => id.as_u64(),
        _ => None,
    }
}

/// Where the bubble the session numbered so is in the thread.
fn at(ui: Ui, id: u64) -> Option<usize> {
    ui.items.with_untracked(|l| l.iter().position(|i| queued_id(i) == Some(id)))
}

/// Read by the answer at its next step: the bubble that waited becomes the request it is, under a new number (so
/// it is drawn as one), marked so. One no bubble waited for (sent from another window before this one came) is
/// added.
fn taken_in(ui: Ui, m: &Value) {
    let id = ui.id();
    let waiting = m["id"].as_u64().and_then(|n| at(ui, n));
    ui.items.update(|l| {
        let mut question = match waiting.map(|w| &l[w].kind) {
            Some(Kind::Queued { question, .. }) => question.clone(),
            _ => Question::from_json(m),
        };
        question.taken_in = true;
        question.q = m["q"].as_u64().unwrap_or(question.q);
        let item = Item { id, kind: Kind::User(question) };
        match waiting {
            Some(w) => l[w] = item,
            None => l.push(item),
        }
    });
}

/// What waits now, as the session says it: a numbered bubble it does not list is gone (taken back, here or in
/// another window), and one whose words changed is drawn again with them. A bubble the session did not number is
/// left as it is.
fn now_waiting(ui: Ui, items: &[Value]) {
    let words = |id: u64| items.iter().find(|i| i["id"].as_u64() == Some(id)).map(text_of);
    let changes = ui.items.with_untracked(|l| {
        l.iter().any(|i| match (&i.kind, queued_id(i)) {
            (Kind::Queued { question, .. }, Some(id)) => words(id).is_none_or(|w| w != question.text),
            _ => false,
        })
    });
    if !changes {
        return;
    }
    let fresh: Vec<u64> = (0..items.len()).map(|_| ui.id()).collect();
    let mut fresh = fresh.into_iter();
    ui.items.update(|l| {
        l.retain(|i| queued_id(i).is_none_or(|id| words(id).is_some()));
        for i in l.iter_mut() {
            let Some(id) = queued_id(i) else { continue };
            let Some(w) = words(id) else { continue };
            if let Kind::Queued { question, .. } = &mut i.kind
                && question.text != w
            {
                (question.text, question.typed) = (w, String::new());
                i.id = fresh.next().unwrap_or(i.id);
            }
        }
    });
}

fn text_of(v: &Value) -> String {
    v["text"].as_str().unwrap_or("").to_string()
}

/// The queued messages a late question says again, by their items' numbers: the last run of them, in the order
/// they were sent, whose words one under the other are the question's. None when no run is.
fn said_again(items: &[Item], text: &str) -> Vec<u64> {
    let queued: Vec<(u64, &str)> = items
        .iter()
        .filter_map(|i| match &i.kind {
            Kind::Queued { question, .. } => Some((i.id, question.text.as_str())),
            _ => None,
        })
        .collect();
    for end in (0..queued.len()).rev() {
        // From the run's last message back to its first: the question ends with each in turn.
        let mut rest = text;
        for start in (0..=end).rev() {
            let Some(before) = rest.strip_suffix(queued[start].1) else { break };
            if before.is_empty() {
                return queued[start..=end].iter().map(|(id, _)| *id).collect();
            }
            let Some(before) = before.strip_suffix('\n') else { break };
            rest = before;
        }
    }
    Vec::new()
}

/// What a waiting message says of itself: when it is read, and what goes with it.
fn status(later: bool, question: &Question) -> String {
    let mut s = String::from(if later { "Waits for the answer to end" } else { "Read at the next step" });
    match question.files.len() {
        0 => {}
        1 => s.push_str(" · 1 file"),
        n => s.push_str(&format!(" · {n} files")),
    }
    s
}

/// A queued message's bubble: dashed, at the right like the person's requests, with when it is read, its quote
/// and files, and (while the session can still change it) Edit and Cancel; the first of them has the note and
/// Clear all over it.
pub fn bubble(ui: Ui, send: Send, item: &Item) -> AnyView {
    let Kind::Queued { id, question, later } = item.kind.clone() else { return empty().into_any() };
    let (t, q, me) = (ui.colors, ui.state::<Queue>(), item.id);
    let number = id.as_u64();
    let hovered = ui.scope.create_rw_signal(false);
    let first = create_memo(move |_| {
        ui.items.with(|l| l.iter().find(|i| matches!(i.kind, Kind::Queued { .. })).map(|i| i.id)) == Some(me)
    });
    let editing = create_memo(move |_| q.editing.get() == Some(me));
    let failed = create_memo(move |_| number.is_some() && q.failed.get() == number);
    let words = question.text.clone();
    let no_quote = question.quote.is_empty();
    let send_clear = send.clone();
    let head = h_stack((
        label(|| NOTE).style(move |s| {
            s.font_size(11.0).color(t.get().desc).flex_grow(1.0_f32).min_width(0.0).text_overflow(TextOverflow::Wrap)
        }),
        label(|| "Clear all").on_click_stop(move |_| send_clear(json!({"type": "queue_clear"}))).style(move |s| {
            let c = t.get();
            s.font_size(11.0).color(c.link).cursor(CursorStyle::Pointer)
        }),
    ))
    .style(move |s| s.gap(8.0).items_center().width_full().apply_if(!first.get() || number.is_none(), |s| s.hide()));
    let tool = move |name: &'static str, tip: &'static str, run: Box<dyn Fn()>| {
        let button = icon(name, 16.0).on_click_stop(move |_| run()).style(move |s| {
            let c = t.get();
            s.padding(2.0)
                .width(20.0)
                .height(20.0)
                .border_radius(4.0)
                .color(c.fg)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.toolbar_hover))
        });
        tooltip(button, move || label(move || tip))
    };
    let edit_words = words.clone();
    let send_cancel = send.clone();
    let tools = h_stack((
        tool(
            "edit",
            "Edit",
            Box::new(move || {
                q.failed.set(None);
                q.draft.set(edit_words.clone());
                q.editing.set(Some(me));
            }),
        ),
        tool(
            "close",
            "Cancel",
            Box::new(move || {
                if let Some(n) = number {
                    send_cancel(json!({"type": "queue_cancel", "id": n}));
                }
            }),
        ),
    ))
    .style(move |s| {
        s.gap(2.0).apply_if(number.is_none() || editing.get() || !(hovered.get() || failed.get()), |s| s.hide())
    });
    let top = h_stack((
        icon("watch", 12.0).style(move |s| s.color(t.get().desc)),
        icon("quote", 12.0).style(move |s| s.color(t.get().desc).apply_if(no_quote, |s| s.hide())),
        label({
            let said = status(later, &question);
            move || said.clone()
        })
        .style(move |s| s.font_size(11.0).color(t.get().desc).min_width(0.0).text_ellipsis()),
        empty().style(|s| s.flex_grow(1.0_f32).min_width(8.0)),
        tools,
    ))
    .style(|s| s.gap(4.0).items_center().width_full().min_height(20.0));
    let body = dyn_container(
        move || editing.get(),
        move |on| {
            if on {
                field(ui, send.clone(), me, number).into_any()
            } else {
                let w = words.clone();
                label(move || w.clone())
                    .style(move |s| s.color(t.get().fg).line_height(1.5).text_overflow(TextOverflow::Wrap))
                    .into_any()
            }
        },
    )
    .style(|s| s.width_full());
    let error = label(|| NOT_CHANGED)
        .style(move |s| s.font_size(11.0).color(t.get().error).apply_if(!failed.get(), |s| s.hide()));
    let inside = v_stack((top, body, error))
        .on_event_cont(EventListener::PointerEnter, move |_| hovered.set(true))
        .on_event_cont(EventListener::PointerLeave, move |_| hovered.set(false))
        .style(move |s| {
            let c = t.get();
            s.padding_horiz(12.0)
                .padding_vert(6.0)
                .border_radius(8.0)
                .border(Stroke::new(1.0).with_dashes(0.0, [4.0, 3.0]))
                .border_color(if editing.get() { c.focus } else { c.widget_border })
                .gap(2.0)
                .min_width(200.0)
                .max_width_pct(85.0)
        });
    v_stack((
        head,
        h_stack((empty().style(|s| s.flex_grow(1.0_f32).min_width(40.0)), inside)).style(|s| s.width_full()),
    ))
    .style(|s| s.width_full().gap(6.0))
    .into_any()
}

/// The words of a queued message being changed: Enter has the session keep them, Escape leaves them as they were.
fn field(ui: Ui, send: Send, me: u64, number: Option<u64>) -> impl IntoView {
    let (t, q) = (ui.colors, ui.state::<Queue>());
    let input = text_input(q.draft)
        .on_event(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(k) = e {
                match &k.key.logical_key {
                    Key::Named(NamedKey::Enter) => {
                        let text = q.draft.get_untracked().trim().to_string();
                        if let Some(n) = number
                            && !text.is_empty()
                        {
                            send(json!({"type": "queue_edit", "id": n, "text": text}));
                        }
                        q.editing.set(None);
                    }
                    Key::Named(NamedKey::Escape) => q.editing.set(None),
                    _ => {}
                }
            }
            // No key goes on to the workbench's own while the words are changed.
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            if q.editing.get_untracked() == Some(me) {
                q.editing.set(None);
            }
        })
        .style(move |s| {
            let c = t.get();
            s.width_full()
                .min_width(0.0)
                .padding_horiz(4.0)
                .padding_vert(2.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(c.focus)
                .background(c.input_bg)
                .color(c.input_fg)
                .cursor_color(Brush::Solid(c.cursor))
        });
    let id = input.id();
    exec_after(Duration::ZERO, move |_| id.request_focus());
    input
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use serde_json::json;

    use super::*;

    /// The thread in short: each request and each queued message by its words.
    fn thread(ui: Ui) -> Vec<String> {
        ui.items.with_untracked(|l| {
            l.iter()
                .filter_map(|i| match &i.kind {
                    Kind::User(q) => Some(format!("user {}{}", q.text, if q.taken_in { " (taken in)" } else { "" })),
                    Kind::Queued { question, later, .. } => {
                        Some(format!("queued {}{}", question.text, if *later { " (later)" } else { "" }))
                    }
                    Kind::Assistant(_) => Some("answer".into()),
                    _ => None,
                })
                .collect()
        })
    }

    #[test]
    fn a_question_that_waited_takes_the_place_of_its_queued_bubble() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let take = |m: Value| ui.take(&m, &send);
        take(json!({"type": "user", "text": "first", "q": 1}));
        take(json!({"type": "text", "delta": "An answer"}));
        // Read by the turn at its next step: its bubble stays where it was sent.
        take(json!({"type": "queued", "text": "and the tests"}));
        take(json!({"type": "text", "delta": " goes on."}));
        // A command waits whole for the answer's end; two more come too late for the turn.
        take(json!({"type": "queued", "text": "/explain it", "later": true}));
        take(json!({"type": "queued", "text": "then commit"}));
        take(json!({"type": "queued", "text": "and push"}));
        assert_eq!(
            thread(ui),
            [
                "user first",
                "answer",
                "queued and the tests",
                "answer",
                "queued /explain it (later)",
                "queued then commit",
                "queued and push"
            ]
        );
        take(json!({"type": "turn_end", "ok": true}));
        // The command's own turn: its question in the bubble's place, the others still waiting.
        take(json!({"type": "user", "text": "/explain it", "q": 2, "late": true}));
        take(json!({"type": "text", "delta": "Explained."}));
        take(json!({"type": "turn_end", "ok": true}));
        // The two that came too late, said again as one question.
        take(json!({"type": "user", "text": "then commit\nand push", "q": 3, "late": true}));
        assert_eq!(
            thread(ui),
            [
                "user first",
                "answer",
                "queued and the tests",
                "answer",
                "user /explain it",
                "answer",
                "user then commit\nand push"
            ]
        );
        // A question no bubble waited for (another window's, or its words changed since) removes none, and a
        // question typed as usual never does.
        take(json!({"type": "user", "text": "something else", "q": 4, "late": true}));
        take(json!({"type": "user", "text": "and the tests", "q": 5}));
        assert_eq!(thread(ui).iter().filter(|t| t.starts_with("queued")).count(), 1);
    }

    #[test]
    fn numbered_messages_are_changed_taken_back_and_taken_in_by_their_numbers() {
        let ui = Ui::new();
        let sent = Rc::new(std::cell::RefCell::new(Vec::new()));
        let to = sent.clone();
        let send: Send = Rc::new(move |m| to.borrow_mut().push(m));
        let take = |m: Value| ui.take(&m, &send);
        take(json!({"type": "user", "text": "first", "q": 1}));
        take(json!({"type": "text", "delta": "An answer"}));
        take(json!({"type": "queued", "id": 1, "text": "same words"}));
        take(json!({"type": "queued", "id": 2, "text": "same words"}));
        take(json!({"type": "queued", "id": 3, "text": "/explain it", "later": true}));
        take(json!({"type": "queued", "id": 4, "text": "drop this"}));
        let before: Vec<u64> = ui.items.with_untracked(|l| l.iter().map(|i| i.id).collect());
        // Changed in another window, one taken back: what waits now.
        take(json!({"type": "queue", "id": 2, "items": [
            {"id": 1, "text": "same words"}, {"id": 2, "text": "other words"}, {"id": 3, "text": "/explain it"}
        ]}));
        assert_eq!(
            thread(ui),
            ["user first", "answer", "queued same words", "queued other words", "queued /explain it (later)"]
        );
        let after: Vec<u64> = ui.items.with_untracked(|l| l.iter().map(|i| i.id).collect());
        assert_eq!(before[..3], after[..3], "what did not change is not drawn again");
        assert_ne!(before[3], after[3], "what changed is");
        // The second is read at the next step: it, not the first of these words, becomes the question.
        take(json!({"type": "took_in", "id": 2, "q": 1, "text": "other words"}));
        take(json!({"type": "notice", "text": "› other words  (sent while it worked)", "took_in": 2}));
        assert_eq!(
            thread(ui),
            ["user first", "answer", "queued same words", "user other words (taken in)", "queued /explain it (later)"]
        );
        // Too late to change: said in its bubble; one gone already, by a toast.
        take(json!({"type": "queue_error", "id": 1}));
        assert_eq!(ui.state::<Queue>().failed.get_untracked(), Some(1));
        take(json!({"type": "queue", "items": [{"id": 1, "text": "same words"}, {"id": 3, "text": "/explain it"}]}));
        take(json!({"type": "turn_end", "ok": true}));
        // The command's turn, then the one left as its own: by their numbers, though the words differ.
        take(json!({"type": "user", "text": "/explain it", "q": 2, "late": true, "queued": [3]}));
        take(json!({"type": "user", "text": "same words, typed again", "q": 3, "late": true, "queued": [1]}));
        assert_eq!(thread(ui).iter().filter(|t| t.starts_with("queued")).count(), 0, "{:?}", thread(ui));
        assert!(!sent.borrow().iter().any(|m| m["type"].as_str().is_some_and(|t| t.starts_with("queue_"))));
    }

    #[test]
    fn what_a_waiting_message_says_of_itself() {
        let mut q = Question::of("look");
        assert_eq!(status(false, &q), "Read at the next step");
        q.files = vec![Default::default(), Default::default()];
        assert_eq!(status(true, &q), "Waits for the answer to end · 2 files");
    }

    #[test]
    fn the_run_of_queued_messages_a_question_says_again() {
        let item = |id: u64, text: &str| Item {
            id,
            kind: Kind::Queued { id: Value::Null, question: Question::of(text), later: false },
        };
        let items = [item(1, "a"), item(2, "b\nc"), item(3, "a"), item(4, "d")];
        assert_eq!(said_again(&items, "d"), [4]);
        assert_eq!(said_again(&items, "a"), [3], "the last one of these words");
        assert_eq!(said_again(&items, "b\nc\na\nd"), [2, 3, 4]);
        assert_eq!(said_again(&items, "a\nb\nc"), [1, 2]);
        assert_eq!(said_again(&items, "c\na"), Vec::<u64>::new(), "not a part of a message");
        assert_eq!(said_again(&items, "a\nd\n"), Vec::<u64>::new());
        assert_eq!(said_again(&[], "a"), Vec::<u64>::new());
    }
}
