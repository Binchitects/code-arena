//! An answer in the thread: its calls, each a card (`tools.rs`), then its words.

use floem::IntoView;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{Decorators, v_stack};
use serde_json::{Value, json};

use crate::model::{Answer, Item, Kind};
use crate::state::{Send, Ui};

pub(super) fn view(ui: Ui, answer: Answer, send: Send) -> impl IntoView {
    let Answer { text, tools, .. } = answer;
    v_stack((
        super::tools::list(ui, send, tools).style(move |s| s.apply_if(tools.with(|t| t.is_empty()), |s| s.hide())),
        crate::prose::answer(ui, text),
    ))
    .style(|s| s.gap(8.0).width_full())
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are an answer's (its words as they come, its turn's end), taken in. Never taken away
/// from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "text" => {
            if let Kind::Assistant(answer) = ui.current().kind {
                answer.text.update(|t| t.push_str(m["delta"].as_str().unwrap_or("")));
            }
        }
        "turn_end" => {
            ui.running.set(false);
            send(json!({"type": "changes"}));
            ui.items.with_untracked(|l| {
                for i in l {
                    if let Kind::Ask { answer, .. } = &i.kind
                        && answer.get_untracked().is_none()
                    {
                        answer.set(Some("gone".into()));
                    }
                }
            });
            if m["ok"] != true {
                if m["stopped"] == true {
                    ui.push(Kind::Notice("Stopped.".into()));
                } else {
                    ui.push(Kind::Error(m["error"].as_str().unwrap_or("The turn failed.").to_string()));
                }
            }
        }
        _ => {}
    }
    false
}

/// An answer's step the session kept, put after the items before it: into the answer it carries on, or a new one.
pub(super) fn kept(ui: Ui, msg: &Value, items: &mut Vec<Item>, running: bool) {
    let text = msg["text"].as_str().unwrap_or("").to_string();
    let tools = super::tools::kept(msg, running);
    if !matches!(items.last(), Some(Item { kind: Kind::Assistant(_), .. })) {
        items.push(Item { id: ui.id(), kind: Kind::assistant(ui.scope) });
    }
    if let Some(Item { kind: Kind::Assistant(Answer { text: t, tools: ts, .. }), .. }) = items.last() {
        if !text.is_empty() {
            t.update(|x| {
                if !x.is_empty() {
                    x.push_str("\n\n");
                }
                x.push_str(&text)
            });
        }
        ts.update(|x| x.extend(tools));
    }
}
