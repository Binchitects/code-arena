//! Where the chat was compacted, as the faces draw it: `compacting` before a compaction asked for (`compact`) and
//! `compacted{summary, through_q}` after it (`nothing` when there was nothing older, `error` when the model gave no
//! summary), each with the key it answers; and in each page of the thread (`state`, `older`), every message the
//! latest compaction covers marked `covered`, its mark `{role: 'mark', type: 'compaction', after_q, summary}` before
//! the first message after it.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::session::Prepared;

/// The latest compaction: the last message it covers (its place in the session), the last question it covers,
/// and its summary.
pub struct Mark {
    pub through: i64,
    pub after_q: usize,
    pub summary: String,
}

/// The last question whose message is at or before `through` (0: none), from the questions by their messages.
pub fn after_q(numbers: &HashMap<i64, usize>, through: i64) -> usize {
    numbers.iter().filter(|(seq, _)| **seq <= through).map(|(_, n)| *n).max().unwrap_or(0)
}

/// A page of the thread (its messages, each with its place in the session in `seqs`) marked with the compaction:
/// what it covers `covered`, and its mark put before the first message after it. When the page ends within what it
/// covers, the mark goes at its end only when the page is the last one (`latest`): else the page after has it.
pub fn mark(messages: &mut Vec<Value>, seqs: &[i64], mark: &Mark, latest: bool) {
    let mut first_after = None;
    for (at, (v, seq)) in messages.iter_mut().zip(seqs).enumerate() {
        if *seq <= mark.through {
            v["covered"] = json!(true);
        } else if first_after.is_none() {
            first_after = Some(at);
        }
    }
    let at = match first_after {
        // A page that begins past the message right after it: an earlier page has that message, and the mark.
        Some(0) if seqs.first().is_some_and(|s| *s > mark.through + 1) => None,
        Some(at) => Some(at),
        None if latest && !messages.is_empty() => Some(messages.len()),
        None => None,
    };
    if let Some(at) = at {
        let v = json!({"role": "mark", "type": "compaction", "after_q": mark.after_q, "summary": mark.summary});
        messages.insert(at, v);
    }
}

/// What is said before a compaction: `compacting`, with the key it answers.
pub fn compacting(m: &Value) -> Value {
    with_key(json!({"type": "compacting"}), m)
}

/// What is said after a compaction: the summary and the last question it covers, or that there was nothing to
/// compact, or why the model gave no summary.
pub fn compacted(p: &Prepared, m: &Value, done: &Result<Option<String>, ca_llm::LlmError>) -> Value {
    let v = match done {
        Ok(Some(summary)) => {
            let kept = p.store.lock().ok().and_then(|s| {
                let (through, summary) = s.last_compaction(&p.session).ok()??;
                let numbers: HashMap<i64, usize> = crate::session::questions_in(&s, &p.session)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(n, seq, _)| (seq, n))
                    .collect();
                Some((after_q(&numbers, through), summary))
            });
            let (through_q, summary) = kept.unwrap_or((0, summary.clone()));
            json!({"type": "compacted", "summary": summary, "through_q": through_q})
        }
        Ok(None) => json!({"type": "compacted", "nothing": true}),
        Err(e) => json!({"type": "compacted", "error": e.to_string()}),
    };
    with_key(v, m)
}

fn with_key(mut v: Value, m: &Value) -> Value {
    if !m["key"].is_null() {
        v["key"] = m["key"].clone();
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(seqs: &[i64]) -> Vec<Value> {
        seqs.iter().map(|s| json!({"role": "user", "seq": s})).collect()
    }

    fn the_mark(through: i64) -> Mark {
        Mark { through, after_q: 2, summary: "So far: a parser.".into() }
    }

    #[test]
    fn the_mark_goes_before_the_first_message_after_what_it_covers() {
        let seqs = [0, 1, 2, 3, 4];
        let mut m = page(&seqs);
        mark(&mut m, &seqs, &the_mark(2), true);
        assert_eq!(m.len(), 6);
        assert_eq!(m[3]["role"], "mark");
        assert_eq!(m[3]["after_q"], 2);
        assert_eq!(m[3]["summary"], "So far: a parser.");
        assert!(m[..3].iter().all(|v| v["covered"] == true));
        assert!(m[4..].iter().all(|v| v["covered"].is_null()));
    }

    #[test]
    fn a_page_within_what_it_covers_has_it_at_its_end_only_when_it_is_the_last() {
        let seqs = [0, 1];
        let mut last = page(&seqs);
        mark(&mut last, &seqs, &the_mark(5), true);
        assert_eq!(last.last().unwrap()["role"], "mark");
        let mut older = page(&seqs);
        mark(&mut older, &seqs, &the_mark(5), false);
        assert_eq!(older.len(), 2);
        assert!(older.iter().all(|v| v["covered"] == true));
    }

    #[test]
    fn a_page_that_begins_after_it_leaves_it_to_the_page_before() {
        let seqs = [10, 11];
        let mut m = page(&seqs);
        mark(&mut m, &seqs, &the_mark(4), true);
        assert_eq!(m.len(), 2);
        // Right after it: the mark is this page's first.
        let seqs = [5, 6];
        let mut m = page(&seqs);
        mark(&mut m, &seqs, &the_mark(4), true);
        assert_eq!(m[0]["role"], "mark");
    }

    #[test]
    fn the_last_question_it_covers() {
        let numbers: HashMap<i64, usize> = [(0, 1), (4, 2), (9, 3)].into_iter().collect();
        assert_eq!(after_q(&numbers, 8), 2);
        assert_eq!(after_q(&numbers, 9), 3);
        assert_eq!(after_q(&numbers, -1), 0);
    }

    #[test]
    fn each_says_the_key_it_answers() {
        assert_eq!(compacting(&json!({"type": "compact", "key": "k1"})), json!({"type": "compacting", "key": "k1"}));
        assert_eq!(compacting(&json!({"type": "compact"})), json!({"type": "compacting"}));
    }
}
