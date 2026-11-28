//! Compacting the chat, and the mark where it was compacted.
//!
//! `/compact` typed whole runs at the first Enter, as Compact Now does (`compact_now`): refused while an answer is
//! written ('Compact when the answer is done.') or while the thread has fewer than two messages ('Nothing to
//! compact yet.'). While the model summarizes, a line over the box says so ('Compacting: the model summarizes…');
//! a toast says how it ended. After the last turn it covers, the thread has a 'Chat compacted' divider whose 'Show
//! summary' shows the summary the model goes on with, drawn as an answer is; compacted again, the divider moves and
//! shows the latest. A question it covers cannot be edited or answered again (`covers`).
//!
//! The session says `compacting` before and `compacted{summary, through_q}` after (`nothing` or `error` when it did
//! not); each page of the thread has its messages `covered` and the mark `{role: 'mark', type: 'compaction',
//! after_q, summary}`.

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, dyn_container, empty, h_stack, label, v_stack};
use serde_json::{Value, json};

use super::slots::{self, Slot};
use crate::icons::icon;
use crate::model::{Item, Kind};
use crate::state::{Send, Ui};
use crate::widgets::toast;

/// What is said when it is asked for while an answer is written.
pub const WAIT: &str = "Compact when the answer is done.";
/// What is said when the thread has too little to compact.
pub const TOO_LITTLE: &str = "Nothing to compact yet.";
/// What a question it covers says of Edit and Answer again.
pub const COVERED: &str = "Compacted: the model reads this question in the summary, so it cannot be asked again";

/// The compaction's state: whether the model summarizes now, and the last question the latest one covers (0: none).
#[derive(Clone, Copy)]
pub struct Compaction {
    pub busy: RwSignal<bool>,
    pub through_q: RwSignal<u64>,
}

impl crate::ext::State for Compaction {
    fn new(ui: Ui) -> Compaction {
        Compaction { busy: ui.scope.create_rw_signal(false), through_q: ui.scope.create_rw_signal(0) }
    }
}

/// Whether the latest compaction covers question `q` (tracked: read in a style, it changes with it).
pub fn covers(ui: Ui, q: u64) -> bool {
    let through = ui.state::<Compaction>().through_q.get();
    q > 0 && q <= through
}

/// Whether the latest compaction covers question `q`, as it is now (untracked: read when pressed).
pub fn covers_now(ui: Ui, q: u64) -> bool {
    q > 0 && q <= ui.state::<Compaction>().through_q.get_untracked()
}

/// Why it cannot be compacted now, from whether an answer is written and how many messages the thread has.
pub fn refused(running: bool, messages: usize) -> Option<&'static str> {
    if running {
        Some(WAIT)
    } else if messages < 2 {
        Some(TOO_LITTLE)
    } else {
        None
    }
}

/// The thread's requests and answers, counted.
fn messages(items: &[Item]) -> usize {
    items.iter().filter(|i| matches!(i.kind, Kind::User(_) | Kind::Assistant(_))).count()
}

/// Compacts now, or says why not.
pub fn compact_now(ui: Ui, send: &Send) {
    let st = ui.state::<Compaction>();
    if st.busy.get_untracked() {
        return;
    }
    if let Some(why) = refused(ui.running.get_untracked(), ui.items.with_untracked(|l| messages(l))) {
        toast::info(ui, why);
        return;
    }
    st.busy.set(true);
    send(json!({"type": "compact"}));
}

/// Whether the box's words are the command, typed whole.
pub fn is_command(typed: &str) -> bool {
    typed.trim() == "/compact"
}

/// A notice of the session's that says what the toast and the mark say already.
fn said_by_us(text: &str) -> bool {
    ["Compacting:", "Compacted:", "Nothing older to summarize", "The model gave no summary"]
        .iter()
        .any(|p| text.starts_with(p))
}

/// Where the mark goes among the items: before the first request after the question it covers last, else at the
/// end.
pub fn place(items: &[Item], through_q: u64) -> usize {
    items.iter().position(|i| matches!(&i.kind, Kind::User(q) if q.q > through_q)).unwrap_or(items.len())
}

/// The mark of a kept page of the thread, as an item.
pub fn kept(ui: Ui, msg: &Value) -> Item {
    let after_q = msg["after_q"].as_u64().unwrap_or(0);
    Item { id: ui.id(), kind: Kind::compaction(ui.scope, after_q, msg["summary"].as_str().unwrap_or("")) }
}

/// The last question a page of the thread says is covered: by its mark, or by its requests marked `covered`.
pub fn covered_in(m: &Value) -> u64 {
    let list = m["messages"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    list.iter()
        .filter_map(|v| match v["role"].as_str() {
            Some("mark") => v["after_q"].as_u64(),
            Some("user") if v["covered"] == true => v["q"].as_u64(),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn change<T: PartialEq + 'static>(s: RwSignal<T>, v: T) {
    if s.with_untracked(|o| *o != v) {
        s.set(v);
    }
}

/// The mark drawn in the thread: a line, 'Chat compacted' with Show summary, a line; the summary under it, shown.
pub fn view(ui: Ui, _send: Send, item: &Item) -> floem::AnyView {
    let Kind::Compaction { summary, open, .. } = item.kind.clone() else {
        return empty().style(|s| s.hide()).into_any();
    };
    let t = ui.colors;
    let rule = move || empty().style(move |s| s.height(1.0).flex_grow(1.0_f32).background(t.get().panel_border));
    let toggle = label(move || if open.get() { "Hide summary" } else { "Show summary" })
        .on_click_stop(move |_| open.update(|o| *o = !*o))
        .style(move |s| {
            let t = t.get();
            s.color(t.link).cursor(CursorStyle::Pointer).hover(|s| s.color(t.link_active))
        });
    let line = h_stack((
        rule(),
        icon("agent-compact", 14.0).style(move |s| s.color(t.get().desc)),
        label(|| "Chat compacted").style(move |s| s.color(t.get().desc)),
        toggle,
        rule(),
    ))
    .style(|s| s.width_full().items_center().gap(6.0).font_size(12.0));
    let text = ui.scope.create_rw_signal(summary);
    let body = dyn_container(
        move || open.get(),
        move |shown| {
            if !shown {
                return empty().style(|s| s.hide()).into_any();
            }
            container(crate::prose::answer(ui, text))
                .style(move |s| {
                    s.width_full()
                        .margin_top(6.0)
                        .padding_left(10.0)
                        .border_left(2.0)
                        .border_color(t.get().quote_border)
                        .text_overflow(TextOverflow::Wrap)
                })
                .into_any()
        },
    )
    .style(|s| s.width_full());
    v_stack((line, body)).style(|s| s.width_full().padding_vert(4.0)).into_any()
}

/// The line over the box while the model summarizes.
fn working(ui: Ui) -> impl IntoView {
    let (st, t) = (ui.state::<Compaction>(), ui.colors);
    h_stack((icon("loading", 14.0), label(|| "Compacting: the model summarizes…"))).style(move |s| {
        s.apply_if(!st.busy.get(), |s| s.hide())
            .gap(6.0)
            .items_center()
            .font_size(12.0)
            .padding_horiz(4.0)
            .padding_bottom(4.0)
            .color(t.get().desc)
    })
}

/// What it adds to the window's registries, once at start: the line over the box, and `/compact` at Enter.
pub fn register(_ui: Ui, _send: &Send) {
    slots::add(Slot::BoxTop, "compaction.working", 5, |ui, _| working(ui).into_any());
    // Before the slash menu's own Enter, so the command typed whole runs at once.
    crate::chat_bridge::on_key("compaction.command", 5, |ui, send, key, mods| {
        let enter = matches!(key, floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter));
        if !enter || !mods.is_empty() || !ui.draft.with_untracked(|d| is_command(d)) {
            return false;
        }
        compact_now(ui, send);
        // Refused while an answer is written, the words stay in the box.
        if !ui.running.get_untracked() {
            crate::chat_bridge::put_in_box(ui, "");
        }
        true
    });
}

/// The session's messages that are this part's, taken in; its own notices taken away from the thread while it
/// compacts (the toast and the mark say them).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let st = ui.state::<Compaction>();
    match m["type"].as_str().unwrap_or("") {
        "compacting" => change(st.busy, true),
        "notice" if st.busy.get_untracked() && said_by_us(m["text"].as_str().unwrap_or("")) => return true,
        "compacted" => {
            change(st.busy, false);
            let through = m["through_q"].as_u64().unwrap_or(0);
            if let Some(summary) = m["summary"].as_str() {
                // Only steps of the turn summarized (no question covered): no divider to draw.
                if through > 0 {
                    let mark = Item { id: ui.id(), kind: Kind::compaction(ui.scope, through, summary) };
                    ui.items.update(|l| {
                        l.retain(|i| !matches!(i.kind, Kind::Compaction { .. }));
                        let at = place(l, through);
                        l.insert(at, mark);
                    });
                    change(st.through_q, through);
                }
                toast::info(ui, "Chat compacted: the model reads a summary of the earlier part from now on.");
            } else if m["nothing"] == true {
                toast::info(ui, "Nothing older to summarize.");
            } else {
                let why = m["error"].as_str().unwrap_or("no answer");
                toast::error(ui, &format!("The model gave no summary ({why}): the conversation is as it was."));
            }
        }
        "refused" if m["of"] == "compact" => change(st.busy, false),
        "turn_start" => change(st.busy, false),
        "state" => {
            change(st.busy, false);
            change(st.through_q, covered_in(m));
        }
        "older" => {
            let older = covered_in(m);
            if older > st.through_q.get_untracked() {
                st.through_q.set(older);
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Question;

    #[test]
    fn it_is_refused_while_an_answer_is_written_or_with_too_little() {
        assert_eq!(refused(true, 9), Some(WAIT));
        assert_eq!(refused(false, 1), Some(TOO_LITTLE));
        assert_eq!(refused(false, 2), None);
    }

    #[test]
    fn the_command_is_typed_whole() {
        assert!(is_command("/compact"));
        assert!(is_command("  /compact \n"));
        assert!(!is_command("/compact-at 80"));
        assert!(!is_command("/compact now"));
    }

    #[test]
    fn its_own_notices_are_known() {
        assert!(said_by_us("Compacting: the older part of the conversation is being summarized."));
        assert!(said_by_us("Compacted: the model now reads about 900 tokens."));
        assert!(!said_by_us("Retrying in 2 s."));
    }

    fn asked(q: u64) -> Item {
        Item { id: q, kind: Kind::User(Question { q, ..Question::of("why") }) }
    }

    #[test]
    fn the_mark_goes_before_the_first_question_after_what_it_covers() {
        let items = vec![asked(1), asked(2), asked(3)];
        assert_eq!(place(&items, 2), 2);
        assert_eq!(place(&items, 3), 3);
        assert_eq!(place(&[], 3), 0);
    }

    #[test]
    fn compacted_again_the_divider_moves_and_covered_questions_are_known() {
        let ui = Ui::new();
        let send: Send = std::rc::Rc::new(|_| {});
        ui.items.set(vec![asked(1), asked(2), asked(3)]);
        let marks = |ui: Ui| {
            ui.items.with_untracked(|l| {
                l.iter()
                    .enumerate()
                    .filter(|(_, i)| matches!(i.kind, Kind::Compaction { .. }))
                    .map(|(at, _)| at)
                    .collect::<Vec<_>>()
            })
        };
        take(ui, &json!({"type": "compacting"}), &send);
        assert!(ui.state::<Compaction>().busy.get_untracked());
        assert!(take(ui, &json!({"type": "notice", "text": "Compacting: the older part."}), &send));
        take(ui, &json!({"type": "compacted", "summary": "So far.", "through_q": 1}), &send);
        assert_eq!(marks(ui), [1]);
        take(ui, &json!({"type": "compacted", "summary": "Later.", "through_q": 2}), &send);
        assert_eq!(marks(ui), [2]);
        assert!(!ui.state::<Compaction>().busy.get_untracked());
        assert!(covers_now(ui, 2) && !covers_now(ui, 3) && !covers_now(ui, 0));
        // Only steps of a turn summarized: no divider.
        take(ui, &json!({"type": "compacted", "summary": "Steps.", "through_q": 0}), &send);
        assert_eq!(marks(ui), [2]);
    }

    #[test]
    fn a_page_says_what_it_covers_by_its_mark_or_its_requests() {
        let m = json!({"messages": [
            {"role": "user", "q": 1, "covered": true},
            {"role": "mark", "after_q": 2},
            {"role": "user", "q": 3}]});
        assert_eq!(covered_in(&m), 2);
        let older = json!({"messages": [{"role": "user", "q": 4, "covered": true}, {"role": "assistant"}]});
        assert_eq!(covered_in(&older), 4);
        assert_eq!(covered_in(&json!({"messages": []})), 0);
    }
}
