//! The agent's question before a call that needs a yes: the call, and Allow, Always, Deny.

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::TextOverflow;
use floem::views::{Decorators, h_stack, label, v_stack};
use serde_json::{Value, json};

use crate::model::Kind;
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, bold, button};

pub(super) fn card(
    ui: Ui,
    send: Send,
    id: u64,
    tool: String,
    subject: String,
    answer: RwSignal<Option<String>>,
) -> impl IntoView {
    let t = ui.colors;
    let reply = move |a: &'static str| {
        let send = send.clone();
        move || send(json!({"type": "answer", "id": id, "answer": a}))
    };
    v_stack((
        label(move || format!("Run {tool}?")).style(move |s| bold(s).color(t.get().fg)),
        label(move || subject.clone()).style(move |s| {
            let c = t.get();
            s.font_family(MONO_FONT.clone())
                .font_size(12.0)
                .padding(8.0)
                .border_radius(4.0)
                .background(c.code)
                .text_overflow(TextOverflow::Wrap)
        }),
        h_stack((
            button(t, "Allow", true, reply("yes")),
            button(t, "Always, this session", false, reply("always")),
            button(t, "Deny", false, reply("no")),
        ))
        .style(move |s| s.gap(8.0).apply_if(answer.get().is_some(), |s| s.hide())),
        label(move || match answer.get().as_deref() {
            Some("no") => "Denied".to_string(),
            Some("always") => "Allowed for this session".to_string(),
            Some("yes") => "Allowed".to_string(),
            Some("gone") => "No longer asked: the turn ended".to_string(),
            _ => String::new(),
        })
        .style(move |s| s.color(t.get().desc).apply_if(answer.get().is_none(), |s| s.hide())),
    ))
    .style(move |s| {
        let c = t.get();
        s.gap(10.0).padding(12.0).border(1.0).border_color(c.widget_border).border_radius(6.0).background(c.widget)
    })
}

/// The session's messages that are the agent's question (asked, answered), taken in. Never taken away from the
/// rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    match m["type"].as_str().unwrap_or("") {
        "ask" => ui.push(Kind::Ask {
            id: m["id"].as_u64().unwrap_or(0),
            tool: s("tool"),
            subject: s("subject"),
            answer: ui.scope.create_rw_signal(None),
        }),
        "answered" => {
            let id = m["id"].as_u64().unwrap_or(0);
            ui.items.with_untracked(|l| {
                for i in l {
                    if let Kind::Ask { id: a, answer, .. } = &i.kind
                        && *a == id
                    {
                        answer.set(Some(s("answer")));
                    }
                }
            });
        }
        _ => {}
    }
    false
}
