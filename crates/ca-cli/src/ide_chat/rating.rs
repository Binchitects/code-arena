//! An answer's rating: thumbs up or down, a reason with a down one, words, and whether the admins may read the chat.
//!
//! - `rate{q, up, reason?, comment?, share?}` or `rate{q, clear: true}` → `rated{q, up}` (`up` null: taken back,
//!   or nothing to rate), with the request's `key`. The rating is kept on the last message of question `q`'s
//!   answer (shown again in `state`'s messages as `rating`), then told to that answer in Arena when the session is
//!   kept there; what keeps it from Arena is said in the thread.
//! - `state.rating`: whether the answers can be rated (the session is kept with a chat in Arena).

use serde_json::{Value, json};

use crate::ide_ext::{Shared, kind, say, send};
use crate::session::questions_in;

/// The reasons Arena takes for a down vote, as its web chat offers them.
pub const REASONS: &[&str] = &["wrong", "incomplete", "too_long", "unsafe", "ignored_instructions", "other"];

/// The most of a comment Arena keeps.
const MOST: usize = 1000;

/// Answers `rate`. `true`: it was one.
pub fn asked(sh: &Shared, m: &Value) -> bool {
    if kind(m) != "rate" {
        return false;
    }
    let q = m["q"].as_u64().unwrap_or(0) as usize;
    let rating = rating_of(m);
    let seq = sh.store.lock().ok().and_then(|store| {
        let questions = questions_in(&store, &sh.session).ok()?;
        let at = questions.iter().position(|(n, _, _)| *n == q)?;
        let next = questions.get(at + 1).map(|(_, seq, _)| *seq);
        let seq = store.last_answer(&sh.session, questions[at].1, next).ok().flatten()?;
        store.set_rating(&sh.session, seq, rating.as_ref()).ok()?;
        Some(seq)
    });
    let up = seq.and(rating.as_ref()).map(|r| r["up"].clone()).unwrap_or(Value::Null);
    let mut reply = json!({"type": "rated", "q": q, "up": up});
    if let Some(key) = m.get("key").filter(|k| k.is_string() || k.is_number()) {
        reply["key"] = key.clone();
    }
    send(&sh.out, reply);
    if let (Some(seq), Some(sync)) = (seq, sh.sync.clone()) {
        let out = sh.out.clone();
        tokio::spawn(async move {
            if let Err(e) = sync.rate(seq, rating.as_ref()).await {
                say(&out, &format!("The rating is kept here but did not reach Arena: {e}"));
            }
        });
    }
    true
}

/// Whether the session's answers can be rated: it is kept with a chat in Arena.
pub fn state(sh: &Shared, v: &mut Value) {
    let kept = sh.sync.is_some()
        && sh.store.lock().ok().and_then(|s| s.chat_link(&sh.session).ok()).is_some_and(|l| !l.off);
    v["rating"] = json!(kept);
}

/// The rating a `rate` asks for, as Arena keeps it (none: taken back): a reason only with a down vote and only one
/// Arena knows, words trimmed and cut to what Arena keeps, empty ones left out.
pub fn rating_of(m: &Value) -> Option<Value> {
    if m["clear"] == true || !m["up"].is_boolean() {
        return None;
    }
    let up = m["up"] == true;
    let mut r = json!({"up": up, "share": m["share"] == true});
    if let Some(reason) = m["reason"].as_str().filter(|r| !up && REASONS.contains(r)) {
        r["reason"] = json!(reason);
    }
    if let Some(c) = m["comment"].as_str().map(str::trim).filter(|c| !c.is_empty()) {
        r["comment"] = json!(c.chars().take(MOST).collect::<String>());
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rating_keeps_only_what_arena_takes() {
        let down = json!({"type": "rate", "q": 2, "up": false, "reason": "wrong", "comment": "  not so \n", "share": true});
        assert_eq!(rating_of(&down), Some(json!({"up": false, "reason": "wrong", "comment": "not so", "share": true})));
        // A reason goes with a down vote only, and only one Arena knows; empty words are none.
        let up = json!({"q": 2, "up": true, "reason": "wrong", "comment": " "});
        assert_eq!(rating_of(&up), Some(json!({"up": true, "share": false})));
        assert_eq!(rating_of(&json!({"up": false, "reason": "rude"})), Some(json!({"up": false, "share": false})));
        let long = "x".repeat(1200);
        assert_eq!(rating_of(&json!({"up": true, "comment": long}))
            .and_then(|r| r["comment"].as_str().map(str::len)), Some(1000));
        // Taken back, or no vote said.
        assert_eq!(rating_of(&json!({"q": 2, "clear": true, "up": true})), None);
        assert_eq!(rating_of(&json!({"q": 2})), None);
    }
}
