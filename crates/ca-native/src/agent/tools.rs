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

/// The session's messages that are a call's (begun, ended), taken in. Never taken away from the
/// rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    match m["type"].as_str().unwrap_or("") {
        "tool_start" => {
            if let Kind::Assistant { tools, .. } = ui.current().kind {
                tools.update(|t| {
                    t.push(Tool { name: s("name"), subject: s("subject"), done: false, ok: true, text: String::new() })
                });
            }
        }
        "tool_end" => {
            if let Kind::Assistant { tools, .. } = ui.current().kind {
                tools.update(|ts| {
                    let (name, subject) = (s("name"), s("subject"));
                    match ts.iter_mut().rev().find(|t| !t.done && t.name == name && t.subject == subject) {
                        Some(t) => {
                            t.done = true;
                            t.ok = m["ok"] == true;
                            t.text = s("text");
                        }
                        None => ts.push(Tool { name, subject, done: true, ok: m["ok"] == true, text: s("text") }),
                    }
                });
            }
        }
        _ => {}
    }
    false
}
