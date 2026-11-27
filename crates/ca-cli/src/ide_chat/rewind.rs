//! Going back to an earlier question, and asking it again: what a face is shown before it goes back, and how the
//! question is asked once more.
//!
//! - `rewind_preview{q}` → `rewind_preview{q, messages, files: [{path, removed}], lost, checkpoint}`: how many
//!   messages would go, and the files that would be put back (or removed: a turn made them). `checkpoint` is false
//!   when this run kept nothing of that question's turns (they ran before the IDE was started): the code cannot be
//!   put back then. `lost` names the files that changed but whose content was not kept (none yet: every file a
//!   turn writes is kept). A `key` the request carries is said again.
//! - `rewind{q, what}` and `again{q}` are answered by the driver (it holds the session): after going back it says
//!   `rewound{q, again, code, chat}` (with the request's `key`), so a face knows its words can go back in its box.
//! - `again{q, text?, files?, length?, model?, thinking?, quote?}`: the question asked again as it was asked (its
//!   words as typed, its quote and the lines sent with it), or in new words (`text`). `files: false` leaves the files
//!   as they are; `length` ("shorter", "longer") asks the model for that; `model` and `thinking` are for this answer
//!   only.

use serde_json::{Map, Value, json};

use crate::ide_ext::{Sent, Shared, kind, send};
use crate::session::{Prepared, questions_in};

/// Answers `rewind_preview`. `true`: it was one.
pub fn asked(sh: &Shared, m: &Value) -> bool {
    if kind(m) != "rewind_preview" {
        return false;
    }
    let q = m["q"].as_u64().unwrap_or(0) as usize;
    let mut reply =
        json!({"type": "rewind_preview", "q": q, "messages": 0, "files": [], "lost": [], "checkpoint": false});
    if let Ok(store) = sh.store.lock()
        && let Ok(questions) = questions_in(&store, &sh.session)
        && let Some((_, seq, _)) = questions.iter().find(|(k, _, _)| *k == q)
    {
        let turns = store.summary(&sh.session).map(|s| s.turns).unwrap_or(0);
        let shown = store
            .page(&sh.session, turns.max(1), None)
            .map(|page| page.messages.iter().filter(|m| m.seq >= *seq && seen(&m.message)).count())
            .unwrap_or(0);
        let files: Vec<Value> = sh
            .checkpoints
            .preview(*seq)
            .into_iter()
            .map(|(path, removed)| json!({"path": sh.workspace.display(&path), "removed": removed}))
            .collect();
        reply["messages"] = json!(shown);
        reply["files"] = json!(files);
        reply["checkpoint"] = json!(sh.checkpoints.has(*seq));
    }
    if let Some(key) = m.get("key").filter(|k| k.is_string() || k.is_number()) {
        reply["key"] = key.clone();
    }
    send(&sh.out, reply);
    true
}

/// A message the thread shows: the person's and the answers' words, not a tool's result or a step that only called.
fn seen(m: &ca_llm::Message) -> bool {
    match m.role {
        ca_llm::Role::User => true,
        ca_llm::Role::Assistant => !m.content.trim().is_empty(),
        _ => false,
    }
}

/// Question `q` as it was asked: the words the model read and the details kept beside them (as typed, its quote, the
/// lines sent with it).
pub fn asked_as(p: &Prepared, q: usize) -> Option<(String, Value)> {
    let store = p.store.lock().ok()?;
    let (_, seq, _) = questions_in(&store, &p.session).ok()?.into_iter().find(|(k, _, _)| *k == q)?;
    let turns = store.summary(&p.session).ok()?.turns;
    let page = store.page(&p.session, turns.max(1), None).ok()?;
    let content = page.messages.into_iter().find(|m| m.seq == seq)?.message.content;
    let details = store.details(&p.session, seq).ok()?.remove(&seq).unwrap_or(Value::Null);
    Some((content, details))
}

/// What a face is told after going back to before question `q`.
pub fn rewound(m: &Value, code: bool, chat: bool) -> Value {
    let mut v = json!({"type": "rewound", "q": m["q"], "again": kind(m) == "again", "code": code, "chat": chat});
    if let Some(key) = m.get("key").filter(|k| k.is_string() || k.is_number()) {
        v["key"] = key.clone();
    }
    v
}

/// What the model is asked, besides the question, for an answer of that length.
fn length_said(length: &str) -> Option<&'static str> {
    match length {
        "shorter" => Some("(Code Arena: answer this again, shorter than before.)"),
        "longer" => Some("(Code Arena: answer this again, longer and in more detail than before.)"),
        _ => None,
    }
}

/// The question asked again: `m` is the `again` message, `asked` the question as it was asked (from
/// [`asked_as`]). None when there are no words to ask.
pub fn again(m: &Value, asked: Option<(String, Value)>) -> Option<Sent> {
    let (content, details) = asked.unwrap_or_default();
    let new = m["text"].as_str().map(str::trim).filter(|t| !t.is_empty());
    let mut sent = match new {
        Some(text) => Sent::new(m, text.to_string(), false),
        None if !content.trim().is_empty() => {
            let mut sent = Sent::new(m, content.clone(), false);
            if let Some(typed) = details["typed"].as_str().filter(|t| !t.is_empty()) {
                sent.shown = typed.to_string();
            }
            sent
        }
        None => return None,
    };
    // What it was sent with is kept with it again: the same details, apart from the words as typed (said anew).
    let mut fields: Map<String, Value> = details.as_object().cloned().unwrap_or_default();
    for k in ["typed", "note", "key", "id", "taken_in"] {
        fields.remove(k);
    }
    if new.is_some() {
        // New words: the lines sent with the old ones are not part of them; the quote is, unless the face took it.
        fields.remove("files");
        fields.remove("skill");
        match m["quote"].as_str() {
            Some("") => {
                fields.remove("quote");
            }
            Some(q) => {
                fields.insert("quote".into(), json!(q));
            }
            None => {}
        }
    }
    sent.fields = fields;
    if let Some(said) = length_said(m["length"].as_str().unwrap_or("")) {
        sent.model = format!("{}\n\n{said}", sent.model);
    }
    Some(sent)
}

/// The model and the thinking an `again` asks for this answer only.
pub fn for_once(m: &Value) -> (Option<String>, Option<ca_llm::Thinking>) {
    if kind(m) != "again" {
        return (None, None);
    }
    let model = m["model"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let thinking = m["thinking"].as_str().filter(|s| !s.is_empty()).map(ca_llm::Thinking::parse);
    (model, thinking)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_asked_again_keeps_its_typed_words_and_what_it_was_sent_with() {
        let asked = Some((
            "@a.rs\n```\nfn a() {}\n```\nexplain".to_string(),
            json!({"typed": "explain", "quote": "the part", "files": [{"path": "a.rs"}], "taken_in": true}),
        ));
        let sent = again(&json!({"type": "again", "q": 2}), asked.clone()).expect("asked");
        assert_eq!(sent.shown, "explain");
        assert!(sent.model.starts_with("@a.rs"), "the model reads what it read before");
        assert_eq!(sent.fields["quote"], "the part");
        assert_eq!(sent.fields["files"][0]["path"], "a.rs");
        assert!(!sent.fields.contains_key("typed") && !sent.fields.contains_key("taken_in"));
        // Shorter: the model is asked so, the thread shows the question as it was.
        let sent = again(&json!({"type": "again", "q": 2, "length": "shorter"}), asked.clone()).expect("asked");
        assert!(sent.model.ends_with("shorter than before.)") && sent.shown == "explain");
        // New words: the old lines go, the quote stays unless taken away.
        let sent = again(&json!({"type": "again", "q": 2, "text": " say more "}), asked.clone()).expect("asked");
        assert_eq!((sent.shown.as_str(), sent.model.as_str()), ("say more", "say more"));
        assert!(!sent.fields.contains_key("files") && sent.fields["quote"] == "the part");
        let sent = again(&json!({"type": "again", "q": 2, "text": "x", "quote": ""}), asked).expect("asked");
        assert!(!sent.fields.contains_key("quote"));
        // Nothing to ask: no turn.
        assert!(again(&json!({"type": "again", "q": 9}), None).is_none());
    }

    #[test]
    fn another_model_and_thinking_are_for_an_again_only() {
        let m = json!({"type": "again", "q": 1, "model": "big", "thinking": "high"});
        assert_eq!(for_once(&m), (Some("big".into()), Some(ca_llm::Thinking::Level("high".into()))));
        assert_eq!(for_once(&json!({"type": "again", "q": 1})), (None, None));
        assert_eq!(for_once(&json!({"type": "send", "model": "big"})), (None, None));
    }

    #[test]
    fn a_face_is_told_it_went_back_with_the_key_it_asked_with() {
        let v = rewound(&json!({"type": "rewind", "q": 3, "what": "chat", "key": "k-1"}), false, true);
        assert_eq!(v, json!({"type": "rewound", "q": 3, "again": false, "code": false, "chat": true, "key": "k-1"}));
    }
}
