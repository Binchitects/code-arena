//! The messages the person sent while a turn ran, as the faces show them waiting: each numbered, changed or taken
//! back until it is read, and said to be read when it is.
//!
//! - A message sent meanwhile is `queued{id, text, later?, ...}`: the turn reads it at its next step (it is put in
//!   the session's inbox), or, `later`, it waits whole for the turn's end (a command).
//! - `queue_edit{id, text}` → `queue{id, items}`, or `queue_error{id}` when it is read already (or the words are
//!   none). `queue_cancel{id}` → `queue{id, items}` or `queue_error{id}`; `queue_clear` → `queue{items: []}`.
//!   `items` is every message still waiting, in the order sent: `{id, text, later, ...}`.
//! - Read by the turn at its next step: `took_in{id, q, text}` just before the notice that says so (the notice
//!   gains `took_in: id`, so a face that reads `took_in` does not take it in twice).
//! - Read as a turn of its own after the turn's end: that turn's `user{late: true}` gains `queued: [id]`.
//! - A face that comes in while the turn runs is sent `queue{items}` after the turn so far, and `state` has
//!   `queued: items`.
//!
//! The inbox is the session's and may hold more than was sent here (what Arena's web chat queued), so a message
//! is found in it by its words, never by its place.

use std::sync::{Mutex, MutexGuard, PoisonError};

use serde_json::{Map, Value, json};

use crate::ide_ext::{Sent, Shared, kind, send};

/// One message waiting.
#[derive(Debug, Clone, PartialEq)]
struct Waiting {
    id: u64,
    /// What the thread shows.
    shown: String,
    /// What the model reads (what is in the inbox, for one read at the next step).
    model: String,
    /// It waits whole for the turn's end.
    later: bool,
    /// What the areas added to its `queued` (its quote, its files).
    fields: Map<String, Value>,
}

impl Waiting {
    fn item(&self) -> Value {
        let mut v = json!({"id": self.id, "text": self.shown, "later": self.later});
        if let Some(o) = v.as_object_mut() {
            for (k, f) in &self.fields {
                o.entry(k.clone()).or_insert_with(|| f.clone());
            }
        }
        v
    }
}

#[derive(Debug, Default)]
struct List {
    next: u64,
    waiting: Vec<Waiting>,
    /// The running turn's question number.
    q: usize,
}

/// What waits, kept by the driver (`sh.kept.of::<Queue>()`).
#[derive(Debug, Default)]
pub struct Queue(Mutex<List>);

impl Queue {
    fn list(&self) -> MutexGuard<'_, List> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Numbers a message sent meanwhile and, unless it waits for the turn's end, puts it in the inbox for the
    /// turn's next step. Its number.
    fn wait(&self, inbox: &Mutex<Vec<String>>, sent: &Sent, later: bool) -> u64 {
        let mut l = self.list();
        l.next += 1;
        let id = l.next;
        if !later && let Ok(mut i) = inbox.lock() {
            i.push(sent.model.clone());
        }
        let mut fields = sent.fields.clone();
        fields.remove("id");
        l.waiting.push(Waiting { id, shown: sent.shown.clone(), model: sent.model.clone(), later, fields });
        id
    }

    /// What still waits, as the faces are told it.
    fn items(&self) -> Vec<Value> {
        self.list().waiting.iter().map(Waiting::item).collect()
    }

    /// The first message waiting for the next step with these words, taken out: the turn has read it.
    fn read(&self, model: &str) -> Option<(Waiting, usize)> {
        let mut l = self.list();
        let at = l.waiting.iter().position(|w| !w.later && w.model == model)?;
        Some((l.waiting.remove(at), l.q))
    }

    /// Every message that waited for the next step, taken out: the turn ended before reading them.
    fn unread(&self) -> Vec<u64> {
        let mut l = self.list();
        let (gone, kept) = std::mem::take(&mut l.waiting).into_iter().partition::<Vec<_>, _>(|w| !w.later);
        l.waiting = kept;
        gone.into_iter().map(|w| w.id).collect()
    }

    /// A message that waited for the turn's end, taken out: its words now, or none when it was taken back.
    fn due(&self, id: u64) -> Option<String> {
        let mut l = self.list();
        let at = l.waiting.iter().position(|w| w.later && w.id == id)?;
        Some(l.waiting.remove(at).model)
    }

    /// Changes a waiting message's words, or (`None`) takes it back. `false`: it waits no more (it was read).
    fn change(&self, inbox: &Mutex<Vec<String>>, id: u64, text: Option<&str>) -> bool {
        let mut l = self.list();
        let Some(at) = l.waiting.iter().position(|w| w.id == id) else { return false };
        if !l.waiting[at].later {
            // Read already when it is no more in the inbox: the turn took it at its step's start.
            let Ok(mut i) = inbox.lock() else { return false };
            let Some(inbox_at) = i.iter().position(|m| *m == l.waiting[at].model) else {
                return false;
            };
            match text {
                Some(t) => i[inbox_at] = t.to_string(),
                None => {
                    i.remove(inbox_at);
                }
            }
        }
        match text {
            Some(t) => {
                let w = &mut l.waiting[at];
                (w.shown, w.model) = (t.to_string(), t.to_string());
            }
            None => {
                l.waiting.remove(at);
            }
        }
        true
    }

    /// Takes back everything that waits.
    fn clear(&self, inbox: &Mutex<Vec<String>>) {
        let mut l = self.list();
        if let Ok(mut i) = inbox.lock() {
            for w in l.waiting.iter().filter(|w| !w.later) {
                if let Some(at) = i.iter().position(|m| *m == w.model) {
                    i.remove(at);
                }
            }
        }
        l.waiting.clear();
    }
}

/// A message sent while a turn runs, waiting: numbered (its `queued` says the number) and, unless it waits for
/// the turn's end, put in the inbox.
pub fn wait(sh: &Shared, sent: &mut Sent, later: bool) {
    let id = sh.kept.of::<Queue>().wait(&sh.inbox, sent, later);
    sent.fields.insert("id".into(), json!(id));
}

/// The running turn's question number, for `took_in`.
pub fn turn_begins(sh: &Shared, q: usize) {
    sh.kept.of::<Queue>().list().q = q;
}

/// The turn read a message at its step's start (its notice says so): `took_in`, and the field the notice gains.
pub fn read(sh: &Shared, notice: &str) -> Option<u64> {
    let model = notice.strip_prefix("› ")?.strip_suffix("(sent while it worked)")?.trim_end();
    let (w, q) = sh.kept.of::<Queue>().read(model)?;
    send(&sh.out, json!({"type": "took_in", "id": w.id, "q": q, "text": w.shown}));
    Some(w.id)
}

/// The messages the turn did not read, now said again as a turn of their own: their numbers.
pub fn unread(sh: &Shared) -> Vec<u64> {
    sh.kept.of::<Queue>().unread()
}

/// A message that waited whole for the turn's end and is now sent: its words as they are now, or none when the
/// person took it back. One that was not numbered here is sent as it is.
pub fn due(sh: &Shared, m: &Value) -> Option<String> {
    match m["queued"].as_u64() {
        Some(id) => sh.kept.of::<Queue>().due(id),
        None => Some(m["text"].as_str().unwrap_or("").to_string()),
    }
}

/// What waits, as `state` and a late face have it.
pub fn items(sh: &Shared) -> Value {
    json!(sh.kept.of::<Queue>().items())
}

/// Tells every face what waits now.
pub fn said(sh: &Shared, id: Option<u64>) {
    let mut v = json!({"type": "queue", "items": items(sh)});
    if let Some(id) = id {
        v["id"] = json!(id);
    }
    send(&sh.out, v);
}

/// `queue_edit`, `queue_cancel`, `queue_clear`.
pub fn asked(sh: &Shared, m: &Value) -> bool {
    let queue = sh.kept.of::<Queue>();
    let id = m["id"].as_u64().unwrap_or(0);
    let changed = match kind(m) {
        "queue_edit" => match m["text"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => queue.change(&sh.inbox, id, Some(t)),
            None => false,
        },
        "queue_cancel" => queue.change(&sh.inbox, id, None),
        "queue_clear" => {
            queue.clear(&sh.inbox);
            said(sh, None);
            return true;
        }
        _ => return false,
    };
    if changed {
        said(sh, Some(id));
    } else {
        send(&sh.out, json!({"type": "queue_error", "id": m["id"]}));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(text: &str) -> Sent {
        Sent::new(&Value::Null, text.to_string(), true)
    }

    #[test]
    fn a_message_waiting_is_changed_or_taken_back_until_the_turn_reads_it() {
        let (q, inbox) = (Queue::default(), Mutex::new(Vec::new()));
        let a = q.wait(&inbox, &sent("and the tests"), false);
        // Arena's web chat put one in between.
        inbox.lock().expect("inbox").push("from the web".into());
        let b = q.wait(&inbox, &sent("then commit"), false);
        let c = q.wait(&inbox, &sent("/explain it"), true);
        assert_eq!(
            *inbox.lock().expect("inbox"),
            ["and the tests", "from the web", "then commit"],
            "a command waits apart"
        );
        assert!(q.change(&inbox, b, Some("then commit and push")));
        assert_eq!(*inbox.lock().expect("inbox"), ["and the tests", "from the web", "then commit and push"]);
        assert!(q.change(&inbox, a, None));
        assert_eq!(*inbox.lock().expect("inbox"), ["from the web", "then commit and push"]);
        assert!(q.change(&inbox, c, Some("/explain the parser")));
        // The turn's next step reads the inbox: what waited is read and can no longer change.
        std::mem::take(&mut *inbox.lock().expect("inbox"));
        assert!(!q.change(&inbox, b, Some("no")), "read already");
        assert!(!q.change(&inbox, 99, None), "none such");
        assert_eq!(q.read("then commit and push").map(|(w, _)| w.id), Some(b));
        assert_eq!(q.read("then commit and push"), None, "read once");
        let ids: Vec<u64> = q.items().iter().filter_map(|i| i["id"].as_u64()).collect();
        assert_eq!(ids, [c]);
        assert_eq!(q.due(c).as_deref(), Some("/explain the parser"));
        assert_eq!(q.due(c), None, "sent once");
    }

    #[test]
    fn what_the_turn_did_not_read_is_said_again_and_clear_takes_all_back() {
        let (q, inbox) = (Queue::default(), Mutex::new(Vec::new()));
        let a = q.wait(&inbox, &sent("one"), false);
        let later = q.wait(&inbox, &sent("/two"), true);
        let b = q.wait(&inbox, &sent("three"), false);
        assert_eq!(q.unread(), [a, b]);
        assert_eq!(q.items().len(), 1, "the command still waits");
        q.wait(&inbox, &sent("four"), false);
        q.clear(&inbox);
        assert!(q.items().is_empty());
        assert_eq!(
            *inbox.lock().expect("inbox"),
            ["one", "three"],
            "the inbox is the driver's to empty at the turn's end"
        );
        assert_eq!(q.due(later), None, "taken back");
    }

    #[test]
    fn an_item_has_what_the_areas_added_to_it() {
        let (q, inbox) = (Queue::default(), Mutex::new(Vec::new()));
        let mut s = sent("look here");
        s.fields.insert("quote".into(), json!("a part"));
        s.fields.insert("id".into(), json!("not this"));
        let id = q.wait(&inbox, &s, false);
        assert_eq!(q.items(), [json!({"id": id, "text": "look here", "later": false, "quote": "a part"})]);
    }
}
