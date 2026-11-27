//! An answer's calls, listed while its line is open: each with how it went and what it gave.

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::style::TextOverflow;
use floem::views::{Decorators, dyn_stack, h_stack, label, v_stack};
use serde_json::Value;

use crate::icons::icon;
use crate::model::{Kind, Tool};
use crate::state::{Send, Ui};
use crate::widgets::MONO_FONT;

pub(super) fn list(ui: Ui, tools: RwSignal<Vec<Tool>>, open: RwSignal<bool>) -> impl IntoView {
    let t = ui.colors;
    dyn_stack(
        move || if open.get() { tools.get().into_iter().enumerate().collect::<Vec<_>>() } else { vec![] },
        |(n, t): &(usize, Tool)| (*n, t.done),
        move |(_, tool): (usize, Tool)| {
            let failed = !tool.ok;
            let out = tool.text.clone();
            v_stack((
                h_stack((
                    icon(
                        if !tool.done {
                            "loading"
                        } else if failed {
                            "error"
                        } else {
                            "check"
                        },
                        16.0,
                    )
                    .style(move |s| {
                        let c = t.get();
                        s.color(if failed { c.error } else { c.desc })
                    }),
                    label(move || format!("{}  {}", tool.name, tool.subject))
                        .style(move |s| s.font_family(MONO_FONT.clone()).font_size(12.0).color(t.get().desc)),
                ))
                .style(|s| s.gap(6.0).items_center()),
                label(move || out.clone()).style(move |s| {
                    let c = t.get();
                    s.font_family(MONO_FONT.clone())
                        .font_size(12.0)
                        .padding(8.0)
                        .border_radius(4.0)
                        .background(c.code)
                        .text_overflow(TextOverflow::Wrap)
                        .apply_if(tool.text.is_empty(), |s| s.hide())
                }),
            ))
            .style(|s| s.gap(4.0).padding_left(20.0))
        },
    )
    .style(|s| s.flex_col().gap(6.0))
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are a call's (begun, ended), taken in. Never taken away from the rest of the window
/// (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "tool_start" => {
            if let Kind::Assistant(answer) = ui.current().kind {
                answer.tools.update(|t| t.push(Tool { ok: true, text: String::new(), ..Tool::from_json(m, false) }));
            }
        }
        "tool_end" => {
            if let Kind::Assistant(answer) = ui.current().kind {
                answer.tools.update(|ts| {
                    // The call as it ended: what the session says of it now, over what it said when it began.
                    let ended = Tool { ok: m["ok"] == true, ..Tool::from_json(m, true) };
                    // The call it ends: the one of its id, when the session numbers its calls; else the last one
                    // of its name and subject that has not ended.
                    let same = |t: &Tool| {
                        if ended.id.is_empty() {
                            !t.done && t.name == ended.name && t.subject == ended.subject
                        } else {
                            t.id == ended.id
                        }
                    };
                    match ts.iter_mut().rev().find(|t| same(t)) {
                        Some(t) => {
                            let arguments = std::mem::take(&mut t.arguments);
                            *t = ended;
                            if t.arguments.is_null() {
                                t.arguments = arguments;
                            }
                        }
                        None => ts.push(ended),
                    }
                });
            }
        }
        _ => {}
    }
    false
}
