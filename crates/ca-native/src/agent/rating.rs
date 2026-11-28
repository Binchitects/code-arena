//! An answer's rating, as the old Code Arena's web chat has it under an answer of a chat kept in Arena: a thumb up
//! ("Good answer") and one down ("Bad answer"), either kept at once, filled while it is the one given. The thumb
//! also opens a box under the answer for more: for a down vote a reason, for either some words and whether the
//! admins may read the chat; "Send" tells it, "Take back" takes the rating away. A thumb given is shown again when
//! the thread is read again (the driver keeps it on the answer, `rate` and `rated`).

use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::CursorStyle;
use floem::views::{
    Decorators, container, dyn_container, empty, h_stack, h_stack_from_iter, label, text_input, tooltip, v_stack,
};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use super::slots::{self, ItemSlot};
use crate::icons::icon;
use crate::keymap::{Focus, TakesKeys};
use crate::model::{Answer, Item, Kind};
use crate::state::{Send, Ui};
use crate::widgets::button;

/// The reasons a down vote may give, as Arena names them and its web chat says them.
pub const REASONS: &[(&str, &str)] = &[
    ("wrong", "Wrong"),
    ("incomplete", "Incomplete"),
    ("too_long", "Too long"),
    ("unsafe", "Unsafe"),
    ("ignored_instructions", "Ignored instructions"),
    ("other", "Other"),
];

/// Whether this session's answers can be rated (it is kept with a chat in Arena), as `state` says.
#[derive(Clone, Copy)]
struct Rating {
    can: RwSignal<bool>,
}

impl crate::ext::State for Rating {
    fn new(ui: Ui) -> Self {
        Rating { can: ui.scope.create_rw_signal(false) }
    }
}

/// The box under an answer: the thumb it is open for (none: closed), the reason chosen, the words and whether the
/// chat is shared.
#[derive(Clone, Copy)]
struct Told {
    side: RwSignal<Option<bool>>,
    reason: RwSignal<Option<&'static str>>,
    comment: RwSignal<String>,
    share: RwSignal<bool>,
}

/// What it adds to the window's registries, once at start: the thumbs under an answer.
pub fn register(_ui: Ui, _send: &Send) {
    slots::add_item(ItemSlot::AnswerFoot, "rating", 30, |ui, send, item| match &item.kind {
        Kind::Assistant(answer) => thumbs(ui, send, item.id, *answer).into_any(),
        _ => empty().into_any(),
    });
}

/// The session's messages that are this part's, taken in: whether answers can be rated (`state`), and a rating
/// as the driver kept it (`rated`). Never taken away from the parts after it.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "state" => put(ui.state::<Rating>().can, m["rating"] == true),
        "rated" => {
            let (q, up) = (m["q"].as_u64().unwrap_or(0), m["up"].as_bool());
            if let Some(a) = ui.items.with_untracked(|l| answer_of(l, q)) {
                put(a.rating, up);
            }
        }
        _ => {}
    }
    false
}

/// Sets a signal only to a new value: what draws from it is drawn again only when it changed.
fn put<T: PartialEq + 'static>(s: RwSignal<T>, v: T) {
    if s.with_untracked(|now| *now != v) {
        s.set(v);
    }
}

/// The number of the question an item of the thread answers: the person's request before it (0: none known). A
/// message read at a step of the answer (taken in) or a note of Code Arena's is no question of its own.
pub fn question_of(items: &[Item], id: u64) -> u64 {
    let Some(at) = items.iter().position(|i| i.id == id) else { return 0 };
    items[..at]
        .iter()
        .rev()
        .find_map(|i| match &i.kind {
            Kind::User(q) if !q.taken_in && !q.note => Some(q.q),
            _ => None,
        })
        .unwrap_or(0)
}

/// The answer of question `q` the driver keeps its rating on: the last of the answers after it.
fn answer_of(items: &[Item], q: u64) -> Option<Answer> {
    let mut now = 0;
    let mut found = None;
    for i in items {
        match &i.kind {
            Kind::User(x) if !x.taken_in && !x.note => now = x.q,
            Kind::Assistant(a) if q > 0 && now == q => found = Some(*a),
            _ => {}
        }
    }
    found
}

/// The rating of question `q`'s answer for the driver: up or down with what the box says (`None`: taken back).
pub fn request(q: u64, up: Option<bool>, reason: Option<&str>, comment: &str, share: bool) -> Value {
    let mut m = json!({"type": "rate", "q": q, "key": format!("rate:{q}")});
    match up {
        None => m["clear"] = json!(true),
        Some(up) => {
            m["up"] = json!(up);
            m["share"] = json!(share);
            if let Some(r) = reason.filter(|_| !up) {
                m["reason"] = json!(r);
            }
            if !comment.trim().is_empty() {
                m["comment"] = json!(comment.trim());
            }
        }
    }
    m
}

/// A thumb's tooltip: what it does, or, given, what more it offers.
pub fn tip(up: bool, given: bool) -> &'static str {
    match (up, given) {
        (true, false) => "Good answer",
        (false, false) => "Bad answer",
        (true, true) => "You liked this answer: say what was good, or take it back",
        (false, true) => "You rated this answer down: say why, or take it back",
    }
}

/// The thumbs under a finished answer, and the box one opens. Hidden while the session's answers cannot be rated
/// and while the answer is written.
fn thumbs(ui: Ui, send: Send, id: u64, answer: Answer) -> impl IntoView {
    let t = ui.colors;
    let can = ui.state::<Rating>().can;
    let Answer { text, usage, rating, .. } = answer;
    let shown = create_memo(move |_| {
        can.get()
            && text.with(|x| !x.is_empty())
            && (usage.with(Option::is_some) || rating.with(Option::is_some) || !ui.running.get())
    });
    let told = Told {
        side: ui.scope.create_rw_signal(None),
        reason: ui.scope.create_rw_signal(None),
        comment: ui.scope.create_rw_signal(String::new()),
        share: ui.scope.create_rw_signal(false),
    };
    let q_of = move || ui.items.with_untracked(|l| question_of(l, id));
    let thumb = |up: bool| {
        let send = send.clone();
        let given = create_memo(move |_| rating.get() == Some(up));
        let name = match up {
            true => ("thumbsup", "thumbsup-filled"),
            false => ("thumbsdown", "thumbsdown-filled"),
        };
        let b = container(dyn_container(move || given.get(), move |g| icon(if g { name.1 } else { name.0 }, 14.0)))
            .on_click_stop(move |_| {
                let q = q_of();
                if q == 0 {
                    return;
                }
                if told.side.get_untracked() == Some(up) {
                    told.side.set(None);
                    return;
                }
                // Kept at once; what the box says is more.
                if rating.get_untracked() != Some(up) {
                    rating.set(Some(up));
                    put(told.reason, None);
                    put(told.comment, String::new());
                    put(told.share, false);
                    send(request(q, Some(up), None, "", false));
                }
                told.side.set(Some(up));
            })
            .style(move |s| {
                let c = t.get();
                s.width(22.0)
                    .height(22.0)
                    .items_center()
                    .justify_center()
                    .border_radius(5.0)
                    .color(c.icon)
                    .cursor(CursorStyle::Pointer)
                    .hover(|s| s.background(c.toolbar_hover))
                    .apply_if(told.side.get() == Some(up), |s| s.background(c.toolbar_hover))
            });
        tooltip(b, move || label(move || tip(up, given.get())))
    };
    let row = h_stack((thumb(true), thumb(false))).style(|s| s.gap(2.0).items_center());
    let open = create_memo(move |_| told.side.get());
    let more = dyn_container(
        move || open.get(),
        move |side| match side {
            Some(up) => boxed(ui, send.clone(), told, up, rating, q_of).into_any(),
            None => empty().into_any(),
        },
    );
    v_stack((row, more)).style(move |s| s.margin_top(2.0).gap(4.0).width_full().apply_if(!shown.get(), |s| s.hide()))
}

/// The box a thumb opens: what was wrong (a reason) or good, some words, whether the admins may read the chat, and
/// "Take back" and "Send".
fn boxed(
    ui: Ui,
    send: Send,
    told: Told,
    up: bool,
    rating: RwSignal<Option<bool>>,
    q_of: impl Fn() -> u64 + Copy + 'static,
) -> AnyView {
    let t = ui.colors;
    let go = {
        let send = send.clone();
        move || {
            let q = q_of();
            let words = told.comment.get_untracked();
            send(request(q, Some(up), told.reason.get_untracked(), &words, told.share.get_untracked()));
            told.side.set(None);
        }
    };
    let back = move || {
        put(rating, None);
        send(request(q_of(), None, None, "", false));
        told.side.set(None);
    };
    let title = label(move || if up { "What was good" } else { "What was wrong" })
        .style(|s| crate::widgets::bold(s.font_size(12.0)));
    let reasons = h_stack_from_iter(REASONS.iter().map(|&(value, words)| {
        let on = create_memo(move |_| told.reason.get() == Some(value));
        label(move || words)
            .on_click_stop(move |_| told.reason.set(if on.get_untracked() { None } else { Some(value) }))
            .style(move |s| {
                let c = t.get();
                s.height(22.0)
                    .items_center()
                    .padding_horiz(8.0)
                    .border(1.0)
                    .border_radius(11.0)
                    .border_color(if on.get() { c.focus } else { c.widget_border })
                    .font_size(11.0)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .apply_if(on.get(), |s| s.background(c.toolbar_hover))
                    .hover(|s| s.background(c.toolbar_hover))
            })
    }))
    .style(move |s| s.gap(4.0).flex_wrap(floem::style::FlexWrap::Wrap).apply_if(up, |s| s.hide()));
    let on_enter = go.clone();
    let input = text_input(told.comment)
        .placeholder("Tell more (optional)")
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            match &k.key.logical_key {
                Key::Named(NamedKey::Enter) => on_enter(),
                Key::Named(NamedKey::Escape) => told.side.set(None),
                _ => return EventPropagation::Continue,
            }
            EventPropagation::Stop
        })
        .takes_keys(Focus::Input)
        .style(move |s| {
            let c = t.get();
            let s = s
                .width_full()
                .min_width(0.0)
                .height(26.0)
                .padding_horiz(6.0)
                .font_size(12.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(c.widget_border)
                .background(c.input_bg)
                .color(c.input_fg)
                .focus(|s| s.border_color(c.focus));
            crate::widgets::steady_input(s, c.input_bg, Some(c.focus))
        });
    let share = told.share;
    let mark = container(icon("check", 14.0).style(move |s| s.apply_if(!share.get(), |s| s.hide()))).style(move |s| {
        let c = t.get();
        s.width(18.0)
            .height(18.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .justify_center()
            .border(1.0)
            .border_radius(3.0)
            .border_color(c.checkbox_border)
            .background(c.checkbox_bg)
            .color(c.checkbox_fg)
    });
    let hint = if up { "went well" } else { "went wrong" };
    let said = v_stack((
        label(|| "Share this chat with the admins").style(|s| s.font_size(12.0)),
        label(move || {
            format!("They can then read it, to see what {hint}. Otherwise they see only its title, the model and your words.")
        })
        .style(move |s| s.font_size(11.0).color(t.get().desc)),
    ))
    .style(|s| s.gap(2.0).min_width(0.0).flex_shrink(1.0_f32));
    let tick = h_stack((mark, said))
        .on_click_stop(move |_| share.update(|o| *o = !*o))
        .style(|s| s.gap(8.0).items_start().cursor(CursorStyle::Pointer));
    let buttons = h_stack((
        button(t, "Take back", false, back),
        empty().style(|s| s.flex_grow(1.0_f32)),
        button(t, "Send", true, go),
    ))
    .style(|s| s.items_center().width_full());
    v_stack((title, reasons, input, tick, buttons))
        .style(move |s| {
            let c = t.get();
            s.gap(8.0)
                .padding(10.0)
                .width_full()
                .max_width(420.0)
                .border(1.0)
                .border_radius(4.0)
                .border_color(c.widget_border)
                .background(c.widget)
                .color(c.fg)
        })
        .into_any()
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    fn items(ui: Ui) -> Vec<Item> {
        ui.items.get_untracked()
    }

    #[test]
    fn an_answer_is_of_the_question_before_it_and_its_rating_goes_on_the_last() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let take = |m: Value| ui.take(&m, &send);
        take(json!({"type": "user", "text": "hi", "q": 1}));
        take(json!({"type": "text", "delta": "Hello."}));
        take(json!({"type": "user", "text": "and the tests?", "q": 2}));
        take(json!({"type": "text", "delta": "They pass."}));
        let l = items(ui);
        let answers: Vec<u64> =
            l.iter().filter(|i| matches!(i.kind, Kind::Assistant(_))).map(|i| question_of(&l, i.id)).collect();
        assert_eq!(answers, [1, 2]);
        assert!(answer_of(&l, 2).is_some_and(|a| a.text.get_untracked() == "They pass."));
        assert!(answer_of(&l, 3).is_none() && answer_of(&l, 0).is_none());
    }

    #[test]
    fn a_rating_the_driver_says_and_whether_answers_can_be_rated_are_taken_in() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let take = |m: Value| ui.take(&m, &send);
        take(json!({"type": "user", "text": "hi", "q": 1}));
        take(json!({"type": "text", "delta": "Hello."}));
        take(json!({"type": "rated", "q": 1, "up": false, "key": "rate:1"}));
        let a = answer_of(&items(ui), 1).expect("the answer");
        assert_eq!(a.rating.get_untracked(), Some(false));
        take(json!({"type": "rated", "q": 1, "up": null}));
        assert_eq!(a.rating.get_untracked(), None);
        assert!(!ui.state::<Rating>().can.get_untracked());
        take(json!({"type": "state", "messages": [], "rating": true}));
        assert!(ui.state::<Rating>().can.get_untracked());
    }

    #[test]
    fn a_thread_read_again_shows_the_rating_kept_on_its_answer() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let msgs = json!([
            {"role": "user", "text": "hi", "q": 1, "turn": 1},
            {"role": "assistant", "text": "", "tools": ["read_file"], "turn": 1},
            {"role": "assistant", "text": "Hello.", "turn": 1, "rating": true},
        ]);
        ui.take(&json!({"type": "state", "messages": msgs, "rating": true}), &send);
        assert_eq!(answer_of(&items(ui), 1).map(|a| a.rating.get_untracked()), Some(Some(true)));
    }

    #[test]
    fn the_request_says_what_the_box_says_and_a_rating_taken_back_says_only_that() {
        assert_eq!(
            request(2, Some(false), Some("wrong"), "  not so ", true),
            json!({"type": "rate", "q": 2, "key": "rate:2", "up": false, "share": true, "reason": "wrong",
                "comment": "not so"})
        );
        assert_eq!(
            request(2, Some(true), Some("wrong"), " ", false),
            json!({"type": "rate", "q": 2, "key": "rate:2", "up": true, "share": false})
        );
        assert_eq!(request(2, None, None, "", false), json!({"type": "rate", "q": 2, "key": "rate:2", "clear": true}));
        assert_eq!((tip(true, false), tip(false, false)), ("Good answer", "Bad answer"));
    }
}
