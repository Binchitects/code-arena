//! Code completion as one types (the grey text ahead of the cursor), asked of the model.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn).
//!
//! - `complete{id, path, prefix, suffix}` [T] → `completion{id, text}` (empty: nothing worth showing) or
//!   `completion{id, error, retry_after?}` (`retry_after`, in seconds: the key is past its limit). The session's
//!   model fills in the middle (`Gateway::fill`: its family's tokens, `/v1/completions`, 96 tokens at most, 15 s at
//!   most), its requests counted where the session's are. One at a time: a newer request ends the one before, which
//!   is then answered by nothing (the window asks only for the newest). Neither the prefix nor the suffix is kept
//!   or written anywhere.

use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, Shared, kind, send};

/// This area in the driver.
pub struct Here;

/// The request being asked, ended when a newer one comes.
#[derive(Default)]
struct Asking(Mutex<Option<tokio::task::AbortHandle>>);

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[("complete", Rule::Turn)];

    fn shared(sh: &Shared, m: &Value) -> bool {
        if kind(m) != "complete" {
            return false;
        }
        let id = m["id"].clone();
        let text = |k: &str| m[k].as_str().unwrap_or("").to_string();
        let (path, prefix, suffix) = (text("path"), text("prefix"), text("suffix"));
        let sh2 = sh.clone();
        let task = tokio::spawn(async move {
            let said = sh2.gateway.fill(&sh2.was.model, &path, &prefix, &suffix, &sh2.tags).await;
            send(&sh2.out, answer(id, said));
        });
        let asking = sh.kept.of::<Asking>();
        let before = asking.0.lock().unwrap_or_else(PoisonError::into_inner).replace(task.abort_handle());
        if let Some(before) = before {
            before.abort();
        }
        true
    }
}

/// The reply to a request, carrying its id.
fn answer(id: Value, said: Result<(String, ca_llm::Usage), ca_llm::LlmError>) -> Value {
    match said {
        Ok((text, u)) => json!({
            "type": "completion",
            "id": id,
            "text": text,
            "usage": {"prompt": u.prompt, "cached": u.cached, "completion": u.completion},
        }),
        Err(ca_llm::LlmError::TooMany { message, again }) => {
            json!({"type": "completion", "id": id, "error": message, "retry_after": again.as_secs()})
        }
        Err(e) => json!({"type": "completion", "id": id, "error": e.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::ide_ext::rule;

    #[test]
    fn a_request_is_answered_at_once_with_its_id() {
        assert_eq!(rule("complete"), Some(Rule::Turn));
        let ok = answer(json!(7), Ok(("x + 1".into(), ca_llm::Usage { prompt: 9, cached: 0, completion: 3 })));
        assert_eq!((ok["id"].as_u64(), ok["text"].as_str()), (Some(7), Some("x + 1")));
        assert_eq!(ok["usage"]["completion"], 3);
        let limit = ca_llm::LlmError::TooMany { message: "slow down".into(), again: Duration::from_secs(30) };
        let busy = answer(json!(8), Err(limit));
        assert_eq!((busy["id"].as_u64(), busy["retry_after"].as_u64()), (Some(8), Some(30)));
        let other = answer(json!(9), Err(ca_llm::LlmError::Parse("bad".into())));
        assert_eq!(other["error"], "bad");
        assert!(other.get("retry_after").is_none());
    }
}
