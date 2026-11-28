//! The agent's question before a call that needs a yes: what it would do, how a command waits, Laya's reading of
//! it, and Allow, Always for this session (unless it asks every time), Deny.

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::TextOverflow;
use floem::views::{Decorators, h_stack, label, v_stack};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::model::Kind;
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, bold, button};

/// From this likelihood on, Laya's line is a warning (the driver asks even in yolo then).
const RISKY: f64 = 0.6;

/// Laya's reading of a command, each from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Risk {
    pub destructive: f64,
    pub outside: f64,
    pub network: f64,
}

impl Risk {
    fn of(v: &Value) -> Option<Risk> {
        let f = |k: &str| v[k].as_f64().unwrap_or(0.0);
        v.is_object().then(|| Risk { destructive: f("destructive"), outside: f("outside"), network: f("network") })
    }

    /// Whether any likelihood is 60% or more.
    pub fn asks(&self) -> bool {
        self.destructive >= RISKY || self.outside >= RISKY || self.network >= RISKY
    }

    /// "Laya: destructive 96%, outside the workspace 93%, network 3%".
    pub fn line(&self) -> String {
        let pc = |v: f64| (v.clamp(0.0, 1.0) * 100.0).round() as u32;
        format!(
            "Laya: destructive {}%, outside the workspace {}%, network {}%",
            pc(self.destructive),
            pc(self.outside),
            pc(self.network)
        )
    }
}

/// A question as the driver put it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Asked {
    pub tool: String,
    /// What the call is about, past how a command waits.
    pub subject: String,
    pub title: String,
    /// What "always" would spare; `None` when the call asks every time (and no Always is offered).
    pub always: Option<String>,
    pub risk: Option<Risk>,
    pub timeless: bool,
    pub background: bool,
}

impl Asked {
    pub fn of(m: &Value) -> Asked {
        let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
        let tool = s("tool");
        let raw = s("subject");
        let subject = ["[no time limit] ", "[in the background] "]
            .iter()
            .find_map(|p| raw.strip_prefix(p))
            .map(str::to_string)
            .unwrap_or(raw);
        let title = m["title"].as_str().map(str::to_string).unwrap_or_else(|| format!("Run {tool}?"));
        Asked {
            title,
            subject,
            always: m["always"].as_str().filter(|a| !a.is_empty()).map(str::to_string),
            risk: Risk::of(&m["risk"]),
            timeless: m["timeless"] == true,
            background: m["background"] == true,
            tool,
        }
    }

    /// How the command waits: "With no time limit", "In the background"; none for the rest.
    pub fn waits(&self) -> Option<&'static str> {
        if self.background {
            Some("In the background")
        } else if self.timeless {
            Some("With no time limit")
        } else {
            None
        }
    }
}

/// What the card says once answered.
pub fn said(answer: &str) -> &'static str {
    match answer {
        "no" => "Denied",
        "always" => "Allowed for this session",
        "yes" => "Allowed",
        "gone" => "No longer asked: the turn ended",
        _ => "",
    }
}

pub(super) fn card(ui: Ui, send: Send, id: u64, asked: Asked, answer: RwSignal<Option<String>>) -> impl IntoView {
    let t = ui.colors;
    let reply = move |a: &'static str| {
        let send = send.clone();
        move || send(json!({"type": "answer", "id": id, "answer": a}))
    };
    let open = move || answer.with(Option::is_none);
    let Asked { title, subject, always, risk, .. } = asked.clone();
    let waits = asked.waits();
    let offers_always = always.is_some();
    let always_text = always.unwrap_or_default();
    let laya = risk.map(|r| (r.line(), r.asks()));
    let (laya_text, laya_warns) = laya.clone().unwrap_or_default();
    v_stack((
        label(move || title.clone()).style(move |s| bold(s).line_height(1.4).color(t.get().fg)),
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
            icon("watch", 14.0).style(move |s| s.color(t.get().desc)),
            label(move || waits.unwrap_or_default()),
        ))
        .style(move |s| s.gap(6.0).items_center().color(t.get().desc).apply_if(waits.is_none(), |s| s.hide())),
        h_stack((
            icon(if laya_warns { "warning" } else { "shield" }, 14.0)
                .style(move |s| s.apply_if(laya_warns, |s| s.color(t.get().warning))),
            label(move || laya_text.clone()).style(|s| s.text_overflow(TextOverflow::Wrap).min_width(0.0)),
        ))
        .style(move |s| s.gap(6.0).items_center().color(t.get().desc).apply_if(laya.is_none(), |s| s.hide())),
        h_stack((
            button(t, "Allow", true, reply("yes")),
            button(t, "Always for this session", false, reply("always"))
                .style(move |s| s.apply_if(!offers_always, |s| s.hide())),
            button(t, "Deny", false, reply("no")),
        ))
        .style(move |s| s.gap(8.0).apply_if(!open(), |s| s.hide())),
        label(move || always_text.clone()).style(move |s| {
            s.font_size(12.0)
                .color(t.get().desc)
                .text_overflow(TextOverflow::Wrap)
                .apply_if(!offers_always || !open(), |s| s.hide())
        }),
        label(move || answer.with(|a| said(a.as_deref().unwrap_or(""))))
            .style(move |s| s.color(t.get().desc).apply_if(open(), |s| s.hide())),
    ))
    .style(move |s| {
        let c = t.get();
        s.gap(10.0).padding(12.0).border(1.0).border_color(c.widget_border).border_radius(6.0).background(c.widget)
    })
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are the agent's question (asked, answered), taken in. Never taken away from the rest of
/// the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "ask" => ui.push(Kind::Ask {
            id: m["id"].as_u64().unwrap_or(0),
            asked: Asked::of(m),
            answer: ui.scope.create_rw_signal(None),
        }),
        "answered" => {
            let id = m["id"].as_u64().unwrap_or(0);
            let said = m["answer"].as_str().unwrap_or("").to_string();
            ui.items.with_untracked(|l| {
                for i in l {
                    if let Kind::Ask { id: a, answer, .. } = &i.kind
                        && *a == id
                        && answer.with_untracked(|v| v.as_deref() != Some(said.as_str()))
                    {
                        answer.set(Some(said.clone()));
                    }
                }
            });
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_risky_command_says_lays_reading_and_offers_no_always() {
        let m = json!({"type": "ask", "id": 4, "tool": "run_shell", "subject": "[no time limit] rm -rf ../build",
                       "title": "Run this command?", "arguments": {"command": "rm -rf ../build"}, "always": null,
                       "risk": {"destructive": 0.96, "outside": 0.93, "network": 0.03},
                       "timeless": true, "background": false});
        let a = Asked::of(&m);
        assert_eq!((a.title.as_str(), a.subject.as_str()), ("Run this command?", "rm -rf ../build"));
        assert_eq!(a.always, None);
        assert_eq!(a.waits(), Some("With no time limit"));
        let r = a.risk.expect("a reading");
        assert_eq!(r.line(), "Laya: destructive 96%, outside the workspace 93%, network 3%");
        assert!(r.asks());
    }

    #[test]
    fn an_edit_offers_always_with_what_it_spares() {
        let m = json!({"type": "ask", "id": 5, "tool": "edit_file", "subject": "a.rs",
                       "title": "Allow edit_file to change this file?",
                       "always": "Always for this session: no more questions about file edits.", "risk": null});
        let a = Asked::of(&m);
        assert_eq!(a.always.as_deref(), Some("Always for this session: no more questions about file edits."));
        assert_eq!((a.risk, a.waits()), (None, None));
        let bg = Asked::of(&json!({"tool": "run_shell", "subject": "[in the background] npm run dev", "background": true}));
        assert_eq!((bg.subject.as_str(), bg.waits(), bg.title.as_str()), ("npm run dev", Some("In the background"), "Run run_shell?"));
        assert!(!Risk { destructive: 0.59, outside: 0.0, network: 0.1 }.asks());
        assert_eq!(said("always"), "Allowed for this session");
    }
}
