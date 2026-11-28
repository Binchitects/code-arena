//! The session's messages about the thread as a whole: that a turn began, and the thread the session kept, made
//! into items. What is one part's is taken in by that part (a request's by `question`, an answer's words and its
//! turn's end by `answer`, a call's by `tools`, a notice by `thread`, the agent's question by `ask`, a queued
//! message by `queue`, the bars' by each bar): see `ext.rs` for the order.
//!
//! Also what the chat's own commands and the turns' ends ask of the window: `ui_action{open, text?}` opens a view
//! (`/tasks`, `/context`, `/projects`, `/loop`, `/goal TEXT`, `/resume`); `status_line{text}` (and `status_line`
//! in `state`) is the person's `statusLine` command's line, an item of the status bar; a `notice` with `toast` is
//! what the session's background work said meanwhile, a toast as well as its line in the thread.

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Decorators, label};
use serde_json::Value;

use crate::layout::View;
use crate::model::Item;
use crate::state::{Send, Ui};
use crate::statusbar;

/// The status line's text; empty: the item is hidden.
#[derive(Clone, Copy)]
struct Line {
    text: RwSignal<String>,
}

impl crate::ext::State for Line {
    fn new(ui: Ui) -> Line {
        Line { text: ui.scope.create_rw_signal(String::new()) }
    }
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    // Between the chord's message (160) and the window's (170): the person's own line, as VS Code puts an
    // extension's item on the left.
    statusbar::add("status.statusLine", statusbar::Side::Left, 165, |ui, _send| {
        let t = ui.colors;
        let text = ui.state::<Line>().text;
        label(move || text.get())
            .style(move |s| statusbar::item(t, s).apply_if(text.with(String::is_empty), |s| s.hide()))
            .into_any()
    });
}

/// The session's message that a turn began, taken in, and what the commands ask of the window. Never taken away
/// from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "turn_start" => ui.running.set(true),
        "ui_action" => open(ui, send, m["open"].as_str().unwrap_or(""), m["text"].as_str().unwrap_or("")),
        "status_line" => line(ui, m["text"].as_str().unwrap_or("")),
        "state" => {
            if let Some(t) = m["status_line"].as_str() {
                line(ui, t);
            }
        }
        // Not again for a turn said again to a face that came late.
        "notice" if m["toast"] == true && m["replay"] != true => {
            let text = m["text"].as_str().unwrap_or("");
            if m["problem"] == true {
                crate::widgets::toast::warning(ui, text);
            } else {
                crate::widgets::toast::info(ui, text);
            }
        }
        _ => {}
    }
    false
}

/// The status line's text, set only when it is another.
fn line(ui: Ui, text: &str) {
    let l = ui.state::<Line>();
    if l.text.with_untracked(|t| t != text) {
        l.text.set(text.to_string());
    }
}

/// Which view a command opens, by the name `ui_action` gives it.
#[derive(Debug, PartialEq)]
enum Opens {
    View(View),
    Command(&'static str),
    Goal,
}

fn opens(name: &str) -> Option<Opens> {
    Some(match name {
        "tasks" => Opens::View(View::Tasks),
        "projects" => Opens::View(View::Projects),
        "sessions" => Opens::View(View::Sessions),
        "context" => Opens::Command("codeArena.agent.showContext"),
        "loops" => Opens::Command(crate::loops::LOOPS),
        "goal" => Opens::Goal,
        _ => return None,
    })
}

/// `ui_action{open, text}`: the view opened, the goal's form with the text typed after `/goal`.
fn open(ui: Ui, send: &Send, name: &str, text: &str) {
    match opens(name) {
        Some(Opens::View(v)) => ui.reveal(v),
        Some(Opens::Command(id)) => {
            crate::commands::run(ui, send, id);
        }
        Some(Opens::Goal) => super::goal_form::open(ui, text, false),
        None => {}
    }
}

/// The thread the session kept, as items.
pub fn thread_of(ui: Ui, m: &Value) -> Vec<Item> {
    let mut items = Vec::new();
    for msg in m["messages"].as_array().into_iter().flatten() {
        match msg["role"].as_str() {
            Some("user") => items.extend(super::question::kept(ui, msg)),
            Some("assistant") => super::answer::kept(ui, msg, &mut items, m["running"] == true),
            _ => {}
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_view_a_command_opens_is_known_by_its_name() {
        assert_eq!(opens("tasks"), Some(Opens::View(View::Tasks)));
        assert_eq!(opens("sessions"), Some(Opens::View(View::Sessions)));
        assert_eq!(opens("context"), Some(Opens::Command("codeArena.agent.showContext")));
        assert_eq!(opens("loops"), Some(Opens::Command(crate::loops::LOOPS)));
        assert_eq!(opens("goal"), Some(Opens::Goal));
        assert_eq!(opens("nowhere"), None);
    }

    #[test]
    fn the_status_line_is_set_by_its_message_and_by_state_and_a_toast_is_left_to_the_thread_too() {
        let ui = Ui::new();
        let send: Send = std::rc::Rc::new(|_| {});
        assert!(!take(ui, &serde_json::json!({"type": "status_line", "text": "main · qwen"}), &send));
        assert_eq!(ui.state::<Line>().text.get_untracked(), "main · qwen");
        take(ui, &serde_json::json!({"type": "state", "messages": []}), &send);
        assert_eq!(ui.state::<Line>().text.get_untracked(), "main · qwen", "a state without one keeps it");
        take(ui, &serde_json::json!({"type": "state", "status_line": ""}), &send);
        assert_eq!(ui.state::<Line>().text.get_untracked(), "");
        assert!(!take(ui, &serde_json::json!({"type": "notice", "text": "Synced.", "toast": true}), &send));
    }
}
