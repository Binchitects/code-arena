//! A message that quotes part of an earlier answer (the person selected it and pressed Reply, or asked about it):
//! `send{text, quote}`. The quote is the person's own words to the model, never the model's: it is put first in what
//! the model reads, marked as a part of "an earlier answer", and the words typed follow. The thread shows only the
//! words typed, with the quote over them (`user{quote}`, kept in the question's details as `quote`).
//!
//! - A quote is cut to 4,000 characters, its line endings tidied and its ends trimmed; an empty one is none.
//! - A `</quote>` in it is written `<\/quote>`, so it cannot end early.
//! - A question asked again in new words (`again{text}`) keeps the quote it was asked with, read the same way.

use serde_json::json;

use crate::ide_ext::{Sent, Shared, kind};

/// The longest quote.
pub const MOST: usize = 4000;

/// A quote as it is kept: line endings tidied, trimmed, cut to `MOST` characters; none when nothing is left.
pub fn clean(quote: &str) -> Option<String> {
    let text = quote.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.chars().take(MOST).collect::<String>().trim_end().to_string())
}

/// What the model reads for a question: the quote first, marked as such, then what the person typed.
pub fn for_model(quote: &str, content: &str) -> String {
    let head = format!(
        "The person quotes this part of an earlier answer:\n<quote>\n{}\n</quote>",
        quote.replace("</quote>", "<\\/quote>")
    );
    if content.is_empty() { head } else { format!("{head}\n\n{content}") }
}

/// A message sent: its quote put first in what the model reads, and kept with the question.
pub fn sent(_sh: &Shared, sent: &mut Sent) {
    quoted(sent);
}

fn quoted(sent: &mut Sent) {
    let quote = match kind(&sent.msg) {
        "send" => sent.msg["quote"].as_str().and_then(clean),
        // Asked again in new words: the quote it was asked with is read with them (the old words had it already).
        "again" if sent.msg["text"].as_str().is_some_and(|t| !t.trim().is_empty()) => {
            sent.fields.get("quote").and_then(|q| q.as_str()).and_then(clean)
        }
        _ => None,
    };
    match quote {
        Some(q) => {
            sent.model = for_model(&q, &sent.model);
            sent.fields.insert("quote".into(), json!(q));
        }
        None if kind(&sent.msg) == "send" => {
            sent.fields.remove("quote");
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quote_is_tidied_and_cut() {
        assert_eq!(clean("  a\r\nb\rc  ").as_deref(), Some("a\nb\nc"));
        assert_eq!(clean(" \n "), None);
        let long = "é".repeat(MOST + 10);
        assert_eq!(clean(&long).map(|q| q.chars().count()), Some(MOST));
    }

    #[test]
    fn the_model_reads_the_quote_first_and_it_cannot_end_early() {
        assert_eq!(
            for_model("x </quote> y", "why?"),
            "The person quotes this part of an earlier answer:\n<quote>\nx <\\/quote> y\n</quote>\n\nwhy?"
        );
        assert!(for_model("x", "").ends_with("</quote>"));
    }

    #[test]
    fn a_message_sent_with_a_quote_keeps_it_and_shows_only_its_words() {
        let mut s = Sent::new(&json!({"type": "send", "text": "why?", "quote": " the parser "}), "why?".into(), false);
        quoted(&mut s);
        assert_eq!(s.shown, "why?");
        assert!(s.model.starts_with("The person quotes") && s.model.ends_with("the parser\n</quote>\n\nwhy?"));
        assert_eq!(s.fields["quote"], "the parser");
        let mut s = Sent::new(&json!({"type": "send", "text": "hi", "quote": "  "}), "hi".into(), false);
        quoted(&mut s);
        assert_eq!(s.model, "hi");
        assert!(!s.fields.contains_key("quote"));
        // Asked again in new words: the kept quote is read with them; the old words had it already.
        let mut s = Sent::new(&json!({"type": "again", "q": 1, "text": "and?"}), "and?".into(), false);
        s.fields.insert("quote".into(), json!("a part"));
        quoted(&mut s);
        assert!(s.model.contains("a part") && s.model.ends_with("and?"));
        let mut s = Sent::new(&json!({"type": "again", "q": 1}), "old".into(), false);
        s.fields.insert("quote".into(), json!("a part"));
        quoted(&mut s);
        assert_eq!(s.model, "old");
    }
}
