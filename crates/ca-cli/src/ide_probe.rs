//! An area for the tests only: the smallest one there can be, to show that a message named in an area's
//! `MESSAGES` reaches that area (and nothing else does), through the same table the real areas are in.

use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, send};
use crate::ide_files::Side;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] =
        &[("probe_where", Rule::Side), ("probe_now", Rule::Turn), ("probe_set", Rule::Queue)];

    /// Where a page's path is in the project, as the areas resolve one.
    fn side(side: &Side, m: &Value) -> bool {
        let path = m["path"].as_str().unwrap_or("");
        let reply = match side.real(path) {
            Ok(real) => json!({"type": "probe_where", "path": path, "shown": side.shown(&real)}),
            Err(e) => json!({"type": "probe_where", "path": path, "error": e}),
        };
        send(side.out(), reply);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::ide_ext::{Later, meanwhile, rule};

    #[test]
    fn a_message_reaches_the_area_that_names_it_by_its_rule() {
        let d = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(d.path().join("src")).expect("dir");
        std::fs::write(d.path().join("src/main.rs"), "fn main() {}\n").expect("w");
        let ws = Arc::new(ca_workspace::Workspace::open(d.path(), &[]).expect("ws"));
        let (out, mut rx) = tokio::sync::broadcast::channel(16);
        let side = Side::new(ws, Arc::default(), out.clone(), None, &std::env::temp_dir());
        let mut said = || -> Vec<Value> {
            std::iter::from_fn(|| rx.try_recv().ok()).map(|t| serde_json::from_str(&t).expect("json")).collect()
        };
        // File-side: answered by the project's side, which asks the areas first.
        assert!(side.handle(&json!({"type": "probe_where", "path": "./src/main.rs"})));
        assert_eq!(said(), [json!({"type": "probe_where", "path": "./src/main.rs", "shown": "src/main.rs"})]);
        assert!(side.handle(&json!({"type": "probe_where", "path": "/etc/passwd"})));
        assert!(said()[0]["error"].is_string(), "a path out of the project is refused as the side's own are");
        // The side's own messages are still its own, and what needs the session is not answered from the side.
        assert!(side.handle(&json!({"type": "stat", "paths": []})));
        assert_eq!(said()[0]["type"], "stats");
        assert!(!side.handle(&json!({"type": "probe_now"})) && !side.handle(&json!({"type": "probe_set"})));
        assert!(said().is_empty());
        // While a turn runs: each by the rule its area gave it.
        let mut later = Later::default();
        assert_eq!((rule("probe_where"), rule("probe_now")), (Some(Rule::Side), Some(Rule::Turn)));
        assert!(meanwhile(&json!({"type": "probe_now"}), &mut later, &out));
        assert!(!meanwhile(&json!({"type": "probe_set", "name": "x"}), &mut later, &out));
        assert_eq!(said(), [json!({"type": "deferred", "of": "probe_set", "name": "x"})]);
        assert_eq!(later.next().expect("kept")["type"], "probe_set");
    }
}
