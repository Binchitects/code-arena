//! The thread: the requests, the answers, the notices and the agent's questions, one after the other, followed to
//! its end as it grows.

use floem::IntoView;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::style::TextOverflow;
use floem::views::{Decorators, dyn_container, dyn_stack, h_stack, label, scroll};
use serde_json::Value;

use crate::icons::icon;
use crate::model::{Item, Kind};
use crate::state::{Send, Ui};

pub(super) fn thread(ui: Ui, send: Send) -> impl IntoView {
    let follow = create_rw_signal(0.0f32);
    create_effect(move |_| {
        ui.items.with(|l| l.len());
        follow.set(100.0);
    });
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
                scroll(
                    dyn_stack(move || ui.items.get(), |i: &Item| i.id, move |i: Item| item(ui, i, send.clone()))
                        .style(|s| s.flex_col().width_full().padding_horiz(16.0).padding_vert(12.0).gap(16.0)),
                )
                .scroll_to_percent(move || follow.get())
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
            let own = super::question::bubble(ui, question.text).into_any();
            around_item(ui, &send, ItemSlot::QuestionTop, own, ItemSlot::QuestionFoot, &i)
        }
        Kind::Assistant(answer) => {
            let own = super::answer::view(ui, answer, send.clone()).into_any();
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
        if !(t.starts_with("› ") && t.ends_with("(sent while it worked)")) {
            ui.push(Kind::Notice(t));
        }
    }
    false
}
