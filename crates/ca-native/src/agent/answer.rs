//! An answer in the thread: its calls in a line that opens, then its words.

use floem::IntoView;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::{Decorators, dyn_container, h_stack, label, v_stack};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::model::{Answer, Item, Kind, Tool, summary};
use crate::state::{Send, Ui};

pub(super) fn view(ui: Ui, answer: Answer) -> impl IntoView {
    let t = ui.colors;
    let Answer { text, tools, open, .. } = answer;
    v_stack((
        h_stack((
            dyn_container(move || open.get(), |o| icon(if o { "chevron-down" } else { "chevron-right" }, 16.0)),
            label(move || summary(&tools.get())),
        ))
        .on_click_stop(move |_| open.update(|o| *o = !*o))
        .style(move |s| {
            s.gap(4.0)
                .items_center()
                .color(t.get().desc)
                .cursor(CursorStyle::Pointer)
                .selectable(false)
                .apply_if(tools.with(|t| t.is_empty()), |s| s.hide())
        }),
        super::tools::list(ui, tools, open),
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
pub(super) fn kept(ui: Ui, msg: &Value, items: &mut Vec<Item>) {
    let text = msg["text"].as_str().unwrap_or("").to_string();
    let tools: Vec<Tool> = msg["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str())
        .map(|n| Tool { name: n.into(), done: true, ok: true, ..Tool::default() })
        .collect();
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
