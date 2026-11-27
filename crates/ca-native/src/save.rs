//! Saving: what the session says of a file it was asked to save.

use floem::reactive::{SignalUpdate, SignalWith};
use serde_json::Value;

use crate::state::{Send, Ui};

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are of a save, taken in. Never taken away from the
/// rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    if m["type"] == "saved" {
        let path = s("path");
        match m["error"].as_str() {
            Some(e) => {
                if ui.close_saved.with_untracked(|c| c.as_deref() == Some(path.as_str())) {
                    ui.close_saved.set(None);
                }
                ui.message.set(format!("Could not save {path}: {e}"))
            }
            None => {
                if let Some(t) = ui.tab(&path) {
                    t.dirty.set(false);
                }
                if let Some(at) = m["modified"].as_u64() {
                    ui.modified.update(|k| {
                        k.insert(path.clone(), at);
                    });
                }
                ui.forget_on_disk(&path);
                ui.message.set(String::new());
                if ui.close_saved.with_untracked(|c| c.as_deref() == Some(path.as_str())) {
                    ui.close_saved.set(None);
                    ui.close_now(&path);
                }
            }
        }
    }
    false
}
