//! What was sent in this folder, for the box's ↑: the file the terminal keeps for its own ↑
//! (`chat::history::InputHistory`, one per folder), read for the IDE and added to by it, so that what is sent in
//! one is recalled in the other.
//!
//! - `history` → `history{items: [text]}`: the folder's lines, the newest first, each once, 500 at most. A `key`
//!   the request carries is said again in the reply.
//! - Each message a face sends is kept as a line, once: where it was sent, not again when it is sent on after the
//!   turn it waited for.
//! - A `key` a `send` carries is said again in the `user` or `queued` that answers it (and in `refused`, by the
//!   driver itself), so a face knows which of its messages was taken.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};

use crate::chat::history::InputHistory;
use crate::ide_ext::{Sent, Shared, kind, send};

/// The most lines a face is given.
const MOST: usize = 500;

/// One reader or writer of the folder's file at a time, in the order asked: a line sent is in the file before a
/// list asked for after it is read. (A line added cuts the file back when it is long, so two at once would not do.)
#[derive(Default)]
struct InTurn {
    /// The next number to give out, and the number whose turn it is.
    next: Mutex<u64>,
    now: Mutex<u64>,
    moved: Condvar,
}

impl InTurn {
    /// A place in the line, taken where the order is known (on the driver's loop).
    fn place(&self) -> u64 {
        let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
        *next += 1;
        *next - 1
    }

    /// Does the work when its place has come, and lets the next one go (whatever became of the work).
    fn at(&self, place: u64, work: impl FnOnce()) {
        struct Done<'a>(MutexGuard<'a, u64>, &'a Condvar);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                *self.0 += 1;
                self.1.notify_all();
            }
        }
        let now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        let now = self.moved.wait_while(now, |now| *now != place).unwrap_or_else(PoisonError::into_inner);
        let _done = Done(now, &self.moved);
        work();
    }
}

/// The folder's file, as the terminal names it.
fn file(sh: &Shared) -> InputHistory {
    InputHistory::new(&sh.paths.data, sh.workspace.cwd())
}

/// The lines as a face is given them: the newest first, `MOST` at most (`lines` is the oldest first, each once).
fn newest_first(lines: Vec<String>) -> Vec<String> {
    lines.into_iter().rev().take(MOST).collect()
}

/// The line a message sent adds to the folder's history; none for what is no message of the person's (the
/// session's own next turn, a question asked again) and for one sent on after the turn it waited for (it was kept
/// when it was sent).
fn line(msg: &Value) -> Option<&str> {
    let text = msg["text"].as_str().map(str::trim).filter(|t| !t.is_empty())?;
    (kind(msg) == "send" && msg["late"] != true).then_some(text)
}

/// The key a message carries, when it is one a reply can say again.
fn key(msg: &Value) -> Option<&Value> {
    Some(&msg["key"]).filter(|k| k.is_string() || k.is_number())
}

/// What reads or writes the folder's file: in its turn, and off the driver's loop when there is one to be off.
fn in_turn(sh: &Shared, work: impl FnOnce() + Send + 'static) {
    let line = sh.kept.of::<InTurn>();
    let place = line.place();
    let work = move || line.at(place, work);
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => drop(rt.spawn_blocking(work)),
        Err(_) => work(),
    }
}

/// `history`: the folder's lines, the newest first.
pub fn asked(sh: &Shared, m: &Value) -> bool {
    let (history, out, key) = (file(sh), sh.out.clone(), key(m).cloned());
    in_turn(sh, move || {
        let mut reply = json!({"type": "history", "items": newest_first(history.load())});
        if let Some(key) = key {
            reply["key"] = key;
        }
        send(&out, reply);
    });
    true
}

/// The key a message sent carries, put in what the faces are told of it (`user`, `queued`).
fn say_key_again(sent: &mut Sent) {
    if let Some(key) = key(&sent.msg).cloned() {
        sent.fields.entry("key").or_insert(key);
    }
}

/// A message the person sent: its line kept in the folder's history, and its key said again in what answers it.
pub fn sent(sh: &Shared, sent: &mut Sent) {
    say_key_again(sent);
    if let Some(text) = line(&sent.msg) {
        let (history, text) = (file(sh), text.to_string());
        in_turn(sh, move || history.add(&text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_face_is_given_the_newest_lines_first_and_no_more_than_it_can_use() {
        let d = tempfile::tempdir().expect("temp");
        let h = InputHistory::new(d.path(), std::path::Path::new("/src/shop"));
        for text in ["one", "two", "one", "three"] {
            h.add(text);
        }
        // Each once, where it was sent last.
        assert_eq!(newest_first(h.load()), ["three", "one", "two"]);
        let many: Vec<String> = (0..MOST + 20).map(|i| format!("m{i}")).collect();
        let given = newest_first(many);
        assert_eq!((given.len(), given[0].as_str()), (MOST, format!("m{}", MOST + 19).as_str()));
    }

    #[test]
    fn what_reads_and_writes_the_file_does_so_in_the_order_asked() {
        let line = std::sync::Arc::new(InTurn::default());
        let done = std::sync::Arc::new(Mutex::new(Vec::new()));
        let places: Vec<u64> = (0..8).map(|_| line.place()).collect();
        // Started the last first: each still waits for its place.
        let threads: Vec<_> = places
            .into_iter()
            .rev()
            .map(|place| {
                let (line, done) = (line.clone(), done.clone());
                std::thread::spawn(move || line.at(place, || done.lock().expect("lock").push(place)))
            })
            .collect();
        for t in threads {
            t.join().expect("ran");
        }
        assert_eq!(*done.lock().expect("lock"), [0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn a_message_is_kept_where_it_was_sent_and_not_again_when_it_is_sent_on() {
        assert_eq!(line(&json!({"type": "send", "text": "  fix the test \n"})), Some("fix the test"));
        // Sent on after the turn it waited for: it was kept when it was sent.
        assert_eq!(line(&json!({"type": "send", "text": "/explain it", "late": true})), None);
        // A question asked again, the session's own next turn, nothing typed.
        assert_eq!(line(&json!({"type": "again", "q": 2, "text": "fix the test"})), None);
        assert_eq!(line(&Value::Null), None);
        assert_eq!(line(&json!({"type": "send", "text": "  "})), None);
    }

    #[test]
    fn the_key_a_message_carries_is_said_again_in_what_answers_it() {
        let mut sent = Sent::new(&json!({"type": "send", "text": "hello", "key": "w1-4"}), "hello".into(), false);
        say_key_again(&mut sent);
        assert_eq!(
            sent.with_fields(json!({"type": "user", "text": "hello", "q": 1})),
            json!({"type": "user", "text": "hello", "q": 1, "key": "w1-4"})
        );
        // Nothing a reply could say again.
        for msg in [json!({"type": "send", "text": "hello"}), json!({"type": "send", "text": "hello", "key": {"a": 1}})]
        {
            let mut sent = Sent::new(&msg, "hello".into(), true);
            say_key_again(&mut sent);
            assert!(sent.fields.is_empty());
        }
    }
}
