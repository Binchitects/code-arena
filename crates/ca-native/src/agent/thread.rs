//! The thread: the requests, the answers, the notices and the agent's questions, one after the other, followed to
//! its end as it grows.

use std::cell::Cell;
use std::rc::Rc;

use floem::{IntoView, View};
use floem::reactive::{SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::style::TextOverflow;
use floem::views::{Decorators, dyn_container, dyn_stack, h_stack, label, scroll};
use serde_json::Value;

use crate::icons::icon;
use crate::model::{Item, Kind};
use crate::state::{Send, Ui};

pub(super) fn thread(ui: Ui, send: Send) -> impl IntoView {
    // Each new item scrolls the thread to its end; the last answer growing (its words, its thinking, its calls,
    // its footer) does too while the end is in view, so a person who scrolled up to read is left where they are.
    let follow = create_rw_signal(0u64);
    let at_end = Rc::new(Cell::new(true));
    create_effect(move |_| {
        ui.items.with(|l| l.len());
        follow.update(|n| *n += 1);
    });
    {
        let at_end = at_end.clone();
        create_effect(move |_| {
            let last = ui.items.with(|l| l.last().map(|i| i.kind.clone()));
            let Some(Kind::Assistant(a)) = last else { return };
            a.text.with(|t| t.len());
            a.reasoning.with(|r| r.len());
            a.tools.with(|t| t.len());
            a.usage.track();
            if at_end.get() {
                follow.update(|n| *n += 1);
            }
        });
    }
    // Whether there is a thread at all, told only when that changes: the thread is built once, not again at each
    // message (Floem's dyn_container builds again whenever what it reads is set, and building it again under an
    // answer being drawn drops what the answer's blocks read).
    let empty = floem::reactive::create_memo(move |_| ui.items.with(|l| l.is_empty()));
    dyn_container(
        move || empty.get(),
        move |empty| {
            if empty {
                super::composer::welcome(ui).into_any()
            } else {
                let send = send.clone();
                let items =
                    dyn_stack(move || ui.items.get(), |i: &Item| i.id, move |i: Item| item(ui, i, send.clone()))
                        .style(|s| s.flex_col().width_full().padding_horiz(16.0).padding_vert(12.0).gap(16.0));
                let (content, at_end) = (items.id(), at_end.clone());
                scroll(items)
                    .on_scroll(move |seen| {
                        let tall = content.get_layout().map(|l| l.size.height as f64).unwrap_or(0.0);
                        at_end.set(seen.y1 >= tall - 24.0);
                    })
                    .scroll_to_percent(move || {
                        follow.track();
                        100.0
                    })
                .style(|s| s.size_full())
                .into_any()
            }
        },
    )
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full())
}

fn item(ui: Ui, i: Item, send: Send) -> impl IntoView {
    use super::slots::{ItemSlot, around_item};
    let t = ui.colors;
    match i.kind.clone() {
        Kind::User(question) | Kind::Queued { question, .. } => {
            let own = super::question::bubble(ui, question);
            around_item(ui, &send, ItemSlot::QuestionTop, own, ItemSlot::QuestionFoot, &i)
        }
        Kind::Assistant(answer) => {
            let own = super::question::flashable(ui, i.id, super::answer::view(ui, answer, send.clone()).into_any());
            around_item(ui, &send, ItemSlot::AnswerTop, own, ItemSlot::AnswerFoot, &i)
        }
        Kind::Compaction { .. } => super::compaction::view(ui, send, &i),
        Kind::Notice(text) => label(move || text.clone())
            .style(move |s| s.color(t.get().desc).font_size(12.0).text_overflow(TextOverflow::Wrap))
            .into_any(),
        Kind::Error(text) => h_stack((
            icon("error", 16.0).style(move |s| s.color(t.get().error)),
            label(move || text.clone()).style(move |s| s.color(t.get().error).text_overflow(TextOverflow::Wrap)),
        ))
        .style(|s| s.gap(6.0))
        .into_any(),
        Kind::Ask { id, tool, subject, answer } => super::ask::card(ui, send, id, tool, subject, answer).into_any(),
    }
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are a notice, taken in. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    if m["type"] == "notice" {
        let t = s("text");
        // A request taken in mid-turn is drawn as its bubble; a skipped thinking by its block ("Answering now…"),
        // whose answer then goes on in the same item.
        let bubble = t.starts_with("› ") && t.ends_with("(sent while it worked)");
        if !bubble && !t.starts_with("Thinking skipped") {
            ui.push(Kind::Notice(t));
        }
    }
    false
}
