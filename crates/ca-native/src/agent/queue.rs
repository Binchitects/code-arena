//! The messages sent while an answer is written, waiting to be read.
//!
//! One that came too late for the answer (and a command, which waits whole for its end) is answered by a turn of
//! its own, begun with its question said again (`user` with `late`): the question takes the place of the bubble
//! that waited. Several that waited are said again as one question, their words one under the other.

use floem::reactive::{SignalUpdate, SignalWith};
use serde_json::Value;

use crate::model::{Item, Kind, Question};
use crate::state::{Send, Ui};

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are a message queued, taken in: one queued, and the question of a turn that says
/// again what waited. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "queued" => {
            ui.push(Kind::Queued { id: m["id"].clone(), question: Question::from_json(m), later: m["later"] == true })
        }
        // The question itself is in the thread already (`question.rs` is given the message first).
        "user" if m["late"] == true => {
            let gone = ui.items.with_untracked(|l| said_again(l, m["text"].as_str().unwrap_or("")));
            if !gone.is_empty() {
                ui.items.update(|l| l.retain(|i| !gone.contains(&i.id)));
            }
        }
        _ => {}
    }
    false
}

/// The queued messages a late question says again, by their items' numbers: the last run of them, in the order
/// they were sent, whose words one under the other are the question's. None when no run is.
fn said_again(items: &[Item], text: &str) -> Vec<u64> {
    let queued: Vec<(u64, &str)> = items
        .iter()
        .filter_map(|i| match &i.kind {
            Kind::Queued { question, .. } => Some((i.id, question.text.as_str())),
            _ => None,
        })
        .collect();
    for end in (0..queued.len()).rev() {
        // From the run's last message back to its first: the question ends with each in turn.
        let mut rest = text;
        for start in (0..=end).rev() {
            let Some(before) = rest.strip_suffix(queued[start].1) else { break };
            if before.is_empty() {
                return queued[start..=end].iter().map(|(id, _)| *id).collect();
            }
            let Some(before) = before.strip_suffix('\n') else { break };
            rest = before;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use serde_json::json;

    use super::*;

    /// The thread in short: each request and each queued message by its words.
    fn thread(ui: Ui) -> Vec<String> {
        ui.items.with_untracked(|l| {
            l.iter()
                .filter_map(|i| match &i.kind {
                    Kind::User(q) => Some(format!("user {}", q.text)),
                    Kind::Queued { question, later, .. } => {
                        Some(format!("queued {}{}", question.text, if *later { " (later)" } else { "" }))
                    }
                    Kind::Assistant(_) => Some("answer".into()),
                    _ => None,
                })
                .collect()
        })
    }

    #[test]
    fn a_question_that_waited_takes_the_place_of_its_queued_bubble() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let take = |m: Value| ui.take(&m, &send);
        take(json!({"type": "user", "text": "first", "q": 1}));
        take(json!({"type": "text", "delta": "An answer"}));
        // Read by the turn at its next step: its bubble stays where it was sent.
        take(json!({"type": "queued", "text": "and the tests"}));
        take(json!({"type": "text", "delta": " goes on."}));
        // A command waits whole for the answer's end; two more come too late for the turn.
        take(json!({"type": "queued", "text": "/explain it", "later": true}));
        take(json!({"type": "queued", "text": "then commit"}));
        take(json!({"type": "queued", "text": "and push"}));
        assert_eq!(
            thread(ui),
            [
                "user first",
                "answer",
                "queued and the tests",
                "answer",
                "queued /explain it (later)",
                "queued then commit",
                "queued and push"
            ]
        );
        take(json!({"type": "turn_end", "ok": true}));
        // The command's own turn: its question in the bubble's place, the others still waiting.
        take(json!({"type": "user", "text": "/explain it", "q": 2, "late": true}));
        take(json!({"type": "text", "delta": "Explained."}));
        take(json!({"type": "turn_end", "ok": true}));
        // The two that came too late, said again as one question.
        take(json!({"type": "user", "text": "then commit\nand push", "q": 3, "late": true}));
        assert_eq!(
            thread(ui),
            [
                "user first",
                "answer",
                "queued and the tests",
                "answer",
                "user /explain it",
                "answer",
                "user then commit\nand push"
            ]
        );
        // A question no bubble waited for (another window's, or its words changed since) removes none, and a
        // question typed as usual never does.
        take(json!({"type": "user", "text": "something else", "q": 4, "late": true}));
        take(json!({"type": "user", "text": "and the tests", "q": 5}));
        assert_eq!(thread(ui).iter().filter(|t| t.starts_with("queued")).count(), 1);
    }

    #[test]
    fn the_run_of_queued_messages_a_question_says_again() {
        let item = |id: u64, text: &str| Item {
            id,
            kind: Kind::Queued { id: Value::Null, question: Question::of(text), later: false },
        };
        let items = [item(1, "a"), item(2, "b\nc"), item(3, "a"), item(4, "d")];
        assert_eq!(said_again(&items, "d"), [4]);
        assert_eq!(said_again(&items, "a"), [3], "the last one of these words");
        assert_eq!(said_again(&items, "b\nc\na\nd"), [2, 3, 4]);
        assert_eq!(said_again(&items, "a\nb\nc"), [1, 2]);
        assert_eq!(said_again(&items, "c\na"), Vec::<u64>::new(), "not a part of a message");
        assert_eq!(said_again(&items, "a\nd\n"), Vec::<u64>::new());
        assert_eq!(said_again(&[], "a"), Vec::<u64>::new());
    }
}
