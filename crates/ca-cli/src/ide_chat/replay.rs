//! The running turn as a face that comes in late sees it: every message of the turn sent so far, kept while the
//! turn runs, said again after the thread before the turn (`state{running: true}`), each marked `replay: true`.
//!
//! What is kept is bounded: a run of words (or of thinking) is kept as one message, a call's progress only as its
//! last, and past [`MOST`] bytes the oldest messages are dropped (the question and the turn's start are kept). A
//! late face then sees the turn from where what is kept begins, after a notice that says so, and the driver sends
//! the whole thread again once the turn ends.

use std::collections::VecDeque;

use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::TryRecvError;

/// The most bytes of the turn's messages kept for a late face.
pub const MOST: usize = 1 << 20;

/// What a late face is told when the turn's start was dropped.
pub const CUT: &str = "The start of this answer is too long to show here: all of it shows once the turn ends.";

/// The messages that are the turn's own: what the thread is made of while it runs.
const THE_TURNS: &[&str] = &[
    "user",
    "turn_start",
    "text",
    "reasoning",
    "notice",
    "tool_waiting",
    "tool_start",
    "tool_end",
    "tool_progress",
    "step_end",
    "ask",
    "answered",
    "queued",
];

/// The turn's messages kept so far, and their size.
#[derive(Default)]
pub struct Kept {
    kept: VecDeque<(Value, usize)>,
    bytes: usize,
    cut: bool,
}

impl Kept {
    /// One message sent to the faces: kept when it is the turn's own and not itself said again.
    pub fn keep(&mut self, m: Value, size: usize) {
        let kind = m["type"].as_str().unwrap_or("");
        if !THE_TURNS.contains(&kind) || m["replay"] == true {
            return;
        }
        // Words after words (thinking after thinking) are one message.
        if matches!(kind, "text" | "reasoning")
            && let Some((last, n)) = self.kept.back_mut()
            && last["type"] == kind
        {
            let delta = m["delta"].as_str().unwrap_or("");
            if let Some(Value::String(d)) = last.get_mut("delta") {
                d.push_str(delta);
                *n += delta.len();
                self.bytes += delta.len();
                self.trim();
                return;
            }
        }
        // A call's progress: only the last counts.
        if kind == "tool_progress"
            && let Some(at) = self.kept.iter().position(|(v, _)| v["type"] == "tool_progress" && v["id"] == m["id"])
        {
            let (_, n) = self.kept[at];
            self.bytes = self.bytes - n + size;
            self.kept[at] = (m, size);
            self.trim();
            return;
        }
        self.bytes += size;
        self.kept.push_back((m, size));
        self.trim();
    }

    /// Past the most kept, the oldest go first, apart from the question and the turn's start.
    fn trim(&mut self) {
        while self.bytes > MOST {
            let Some(at) = self.kept.iter().position(|(v, _)| !matches!(v["type"].as_str(), Some("user" | "turn_start")))
            else {
                return;
            };
            if let Some((_, n)) = self.kept.remove(at) {
                self.bytes -= n;
                self.cut = true;
            }
        }
    }

    /// Whether some of the turn was dropped.
    pub fn cut(&self) -> bool {
        self.cut
    }

    /// The turn so far as it is said again: each message marked, and the notice that the start was dropped right
    /// after the turn's start.
    pub fn said(&self) -> Vec<Value> {
        let mut out = Vec::with_capacity(self.kept.len() + 1);
        for (v, _) in &self.kept {
            let mut v = v.clone();
            v["replay"] = json!(true);
            let start = v["type"] == "turn_start";
            out.push(v);
            if start && self.cut {
                out.push(json!({"type": "notice", "text": CUT, "replay": true}));
            }
        }
        out
    }
}

/// What the faces are sent while a turn runs, listened to from its start.
pub struct Replay {
    rx: broadcast::Receiver<String>,
    pub kept: Kept,
    /// The faces' channel is gone (never while the driver runs): nothing more to listen to.
    pub closed: bool,
}

impl Replay {
    /// Listened to from now: made before the turn's first message is sent.
    pub fn new(out: &broadcast::Sender<String>) -> Self {
        Replay { rx: out.subscribe(), kept: Kept::default(), closed: false }
    }

    /// The next message sent, kept (for the turn's loop to wait on beside the turn).
    pub async fn next(&mut self) {
        match self.rx.recv().await {
            Ok(m) => self.take(&m),
            Err(broadcast::error::RecvError::Lagged(_)) => self.kept.cut = true,
            Err(broadcast::error::RecvError::Closed) => self.closed = true,
        }
    }

    /// Everything sent until now, kept: before the turn so far is said again.
    pub fn drain(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(m) => self.take(&m),
                Err(TryRecvError::Lagged(_)) => self.kept.cut = true,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Closed) => {
                    self.closed = true;
                    return;
                }
            }
        }
    }

    fn take(&mut self, m: &str) {
        // Only the turn's own are read: the rest (a file, the bars) are not parsed at all.
        if !THE_TURNS.iter().any(|k| m.contains(&format!("\"type\":\"{k}\""))) {
            return;
        }
        if let Ok(v) = serde_json::from_str::<Value>(m) {
            self.kept.keep(v, m.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keep(k: &mut Kept, v: Value) {
        let n = v.to_string().len();
        k.keep(v, n);
    }

    #[test]
    fn the_turn_is_said_again_marked_with_its_words_as_one_and_a_calls_last_progress() {
        let mut k = Kept::default();
        keep(&mut k, json!({"type": "user", "text": "fix it", "q": 3}));
        keep(&mut k, json!({"type": "turn_start"}));
        keep(&mut k, json!({"type": "status", "model": "m"}));
        keep(&mut k, json!({"type": "text", "delta": "Let me "}));
        keep(&mut k, json!({"type": "text", "delta": "look."}));
        keep(&mut k, json!({"type": "tool_start", "id": "a", "name": "grep"}));
        keep(&mut k, json!({"type": "tool_progress", "id": "a", "progress": 1}));
        keep(&mut k, json!({"type": "tool_progress", "id": "a", "progress": 2}));
        keep(&mut k, json!({"type": "text", "delta": "again", "replay": true}));
        keep(&mut k, json!({"type": "text", "delta": "Found"}));
        let said = k.said();
        let kinds: Vec<&str> = said.iter().filter_map(|v| v["type"].as_str()).collect();
        assert_eq!(kinds, ["user", "turn_start", "text", "tool_start", "tool_progress", "text"]);
        assert_eq!(said[2]["delta"], "Let me look.");
        assert_eq!(said[4]["progress"], 2);
        assert_eq!(said[5]["delta"], "Found", "what was itself said again is not kept");
        assert!(said.iter().all(|v| v["replay"] == true));
        assert!(!k.cut());
    }

    #[test]
    fn past_the_most_the_oldest_go_but_the_question_and_the_start_stay_and_a_notice_says_so() {
        let mut k = Kept::default();
        keep(&mut k, json!({"type": "user", "text": "write a book"}));
        keep(&mut k, json!({"type": "turn_start"}));
        let page = "x".repeat(4000);
        for i in 0..400 {
            keep(&mut k, json!({"type": "tool_end", "id": i.to_string(), "text": page}));
        }
        assert!(k.cut() && k.bytes <= MOST);
        let said = k.said();
        assert_eq!((&said[0]["type"], &said[1]["type"]), (&json!("user"), &json!("turn_start")));
        assert_eq!(said[2], json!({"type": "notice", "text": CUT, "replay": true}));
        assert_eq!(said.last().map(|v| v["id"].clone()), Some(json!("399")), "the newest are kept");
        assert_ne!(said[3]["id"], "0", "the oldest went");
    }

    #[tokio::test]
    async fn what_is_sent_until_now_is_kept_and_the_rest_not_read() {
        let (out, _rx) = broadcast::channel(16);
        let mut r = Replay::new(&out);
        let _ = out.send(json!({"type": "turn_start"}).to_string());
        let _ = out.send(json!({"type": "file", "path": "a"}).to_string());
        let _ = out.send(json!({"type": "text", "delta": "hi"}).to_string());
        r.drain();
        let said = r.kept.said();
        assert_eq!(said.len(), 2);
        assert_eq!(said[1]["delta"], "hi");
    }
}
