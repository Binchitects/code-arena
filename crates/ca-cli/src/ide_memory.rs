//! What is remembered, for the Memory view: listed, kept, forgotten.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn). The work is `ca_memory`'s, as `/memory` and `/remember`
//! do it in the terminal: Arena's memory when it answered at the session's start, else the files on this machine.
//!
//! Messages, all answered while a turn runs too:
//! - `memory{key?}` [T]: answered by `memory{key, online, signed_in, denied, project, notebook, items: [{n, text,
//!   kind, where, pinned, own}]}`. `n` is the number `/memory` gives it; `where` is person, project or company;
//!   `notebook` is the project's long memory (`.remember/MEMORY.md`) while it is kept and the file is there.
//! - `memory_remember{text, project}` [T]: a `notice{text, of, error?}` ("Remembered for this project, in Arena."
//!   or "Remembered already.", or why not; `of` is the message answered, `error` is true for why not), then
//!   `memory` again.
//! - `memory_forget{n, text?}` [T]: a `notice{text, of, error?}` (what was forgotten, or why not), then `memory`
//!   again. With `text`, the memory at `n` must still be that one: the list may have changed since the view drew it.

use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, Shared, kind, send};

/// The most characters a memory kept from the view has (the view stops there too).
const MOST: usize = 500;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] =
        &[("memory", Rule::Turn), ("memory_remember", Rule::Turn), ("memory_forget", Rule::Turn)];

    fn shared(sh: &Shared, m: &Value) -> bool {
        match kind(m) {
            "memory" => send(&sh.out, listed(sh, &m["key"])),
            "memory_remember" => {
                let (sh, text, project) =
                    (sh.clone(), m["text"].as_str().unwrap_or("").to_string(), m["project"].as_bool());
                tokio::spawn(async move {
                    let said = if text.trim().chars().count() > MOST {
                        Err(format!("A memory is at most {MOST} characters."))
                    } else {
                        sh.memory.remember(project != Some(true), &text, "fact").await.map(remembered)
                    };
                    send(&sh.out, notice("memory_remember", said));
                    send(&sh.out, listed(&sh, &Value::Null));
                });
            }
            "memory_forget" => {
                let n = m["n"].as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(0);
                let (sh, text) = (sh.clone(), m["text"].as_str().map(str::to_string));
                tokio::spawn(async move {
                    let texts: Vec<String> = sh.memory.items().into_iter().map(|i| i.text).collect();
                    let said = match still(&texts, n, text.as_deref()) {
                        Err(e) => Err(e),
                        Ok(()) => sh.memory.forget(n).await.map(|t| format!("Forgot: {t}")),
                    };
                    send(&sh.out, notice("memory_forget", said));
                    send(&sh.out, listed(&sh, &Value::Null));
                });
            }
            _ => return false,
        }
        true
    }
}

/// The `memory` reply.
fn listed(sh: &Shared, key: &Value) -> Value {
    let mem = &sh.memory;
    let kept = sh.settings.bool(&ca_settings::schema::PROJECT_MEMORY);
    let main = ca_memory::project::file_of(&mem.root, None).ok().filter(|f| kept && f.is_file());
    let folder = std::path::Path::new(&sh.folder);
    let notebook =
        main.map(|f| f.strip_prefix(folder).map(|r| r.to_path_buf()).unwrap_or(f).to_string_lossy().replace('\\', "/"));
    let denied = mem.denied();
    let mut v = json!({
        "type": "memory",
        "online": mem.online(),
        "signed_in": mem.arena().is_some(),
        "denied": denied,
        "project": mem.project_key,
        "notebook": notebook,
        "items": if denied { Vec::new() } else { items_of(&mem.items()) },
    });
    if !key.is_null() {
        v["key"] = key.clone();
    }
    v
}

/// The items as the reply says them, numbered from 1 in `/memory`'s order.
fn items_of(items: &[ca_memory::Item]) -> Vec<Value> {
    items
        .iter()
        .enumerate()
        .map(|(i, m)| {
            json!({"n": i + 1, "text": m.text, "kind": m.kind, "where": m.scope, "pinned": m.pinned, "own": m.own})
        })
        .collect()
}

/// The notice that answers a request: what was done, or why not.
fn notice(of: &str, said: Result<String, String>) -> Value {
    match said {
        Ok(text) => json!({"type": "notice", "text": text, "of": of}),
        Err(text) => json!({"type": "notice", "text": text, "of": of, "error": true}),
    }
}

/// What remembering says: where it went, or that it was there already.
fn remembered((new, place): (bool, String)) -> String {
    if new { format!("Remembered {place}.") } else { "Remembered already.".into() }
}

/// Whether the memory at `n` (from 1) is still the one the view showed.
fn still(texts: &[String], n: usize, text: Option<&str>) -> Result<(), String> {
    match (texts.get(n.wrapping_sub(1)), text) {
        (None, _) => Err(format!("There is no memory {n}: the list changed. It is shown again.")),
        (Some(t), Some(want)) if t != want => Err("The list changed since it was shown: it is shown again.".into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, scope: &str, own: bool) -> ca_memory::Item {
        ca_memory::Item { text: text.into(), scope: scope.into(), kind: "fact".into(), pinned: scope == "company", own }
    }

    #[test]
    fn items_are_numbered_as_memory_lists_them() {
        let v = items_of(&[item("tabs", "person", true), item("no friday deploys", "company", false)]);
        assert_eq!(
            v[1],
            json!({"n": 2, "text": "no friday deploys", "kind": "fact", "where": "company", "pinned": true, "own": false})
        );
        assert_eq!(v[0]["n"], 1);
    }

    #[test]
    fn a_forget_is_refused_when_the_list_changed() {
        let texts = vec!["a".to_string(), "b".to_string()];
        assert_eq!(still(&texts, 2, Some("b")), Ok(()));
        assert_eq!(still(&texts, 2, None), Ok(()));
        assert!(still(&texts, 1, Some("b")).is_err());
        assert!(still(&texts, 3, None).is_err());
        assert!(still(&texts, 0, None).is_err());
    }

    #[test]
    fn remembering_says_where_or_already() {
        assert_eq!(remembered((true, "for this project, in Arena".into())), "Remembered for this project, in Arena.");
        assert_eq!(remembered((false, "x".into())), "Remembered already.");
    }
}
