//! What the chat's pickers offer, in the driver's `state`: each model as the gateway describes it, the levels of
//! thinking, and the modes with what each lets run.
//!
//! - `model_info: [{id, context?, max_output?, tools, thinking, vision}]`: one row for each of `models` the
//!   gateway described when the driver began (a model it did not describe has no row; `models` is still the list).
//! - `thinking_levels: [name]`: the presets Arena's web chat sends, `default` (the model's own) and `off` first.
//! - `modes: [{name, description}]`: the four of `ca_agent::Mode`, in the order a picker shows them.

use std::sync::Mutex;

use ca_llm::ModelInfo;
use serde_json::{Value, json};

use crate::ide_ext::Shared;

/// The levels a face offers, as Arena's web chat sends them (`reasoning_effort`).
pub const THINKING_LEVELS: &[&str] = &["default", "off", "low", "medium", "high", "xhigh"];

/// The modes, each with what it lets run without asking.
pub const MODES: &[(&str, &str)] = &[
    ("ask", "Edits and commands ask first"),
    ("auto-edit", "File edits run without asking; commands ask"),
    ("plan", "Read-only: no edits, no commands"),
    ("yolo", "Nothing asks: edits and commands run at once"),
];

/// The models as the gateway described them when the driver asked, kept for the driver's life.
#[derive(Default)]
pub struct Infos(Mutex<Vec<ModelInfo>>);

impl Infos {
    /// Keeps what the gateway said of its models (in place of what was kept).
    pub fn set(&self, all: Vec<ModelInfo>) {
        *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = all;
    }

    fn all(&self) -> Vec<ModelInfo> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

/// One model's row, as a face reads it.
fn row(m: &ModelInfo) -> Value {
    let mut v = json!({"id": m.id, "tools": m.tools, "thinking": m.thinking, "vision": m.vision});
    if let Some(c) = m.context {
        v["context"] = json!(c);
    }
    if let Some(o) = m.max_output {
        v["max_output"] = json!(o);
    }
    v
}

/// The pickers' part of `state`.
pub fn state(sh: &Shared, v: &mut Value) {
    let infos = sh.kept.of::<Infos>().all();
    v["model_info"] = infos.iter().filter(|m| sh.models.contains(&m.id)).map(row).collect();
    v["thinking_levels"] = json!(THINKING_LEVELS);
    v["modes"] = MODES.iter().map(|(name, description)| json!({"name": name, "description": description})).collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_models_row_says_only_what_the_gateway_said() {
        let full = ModelInfo {
            id: "m".into(),
            context: Some(131072),
            max_output: Some(16384),
            tools: true,
            thinking: false,
            vision: true,
        };
        assert_eq!(
            row(&full),
            json!({"id": "m", "context": 131072, "max_output": 16384, "tools": true, "thinking": false, "vision": true})
        );
        let bare = ModelInfo { id: "n".into(), tools: true, thinking: true, ..Default::default() };
        assert_eq!(row(&bare), json!({"id": "n", "tools": true, "thinking": true, "vision": false}));
    }

    #[test]
    fn every_mode_the_session_takes_is_offered_once() {
        for (name, _) in MODES {
            assert_eq!(ca_agent::Mode::parse(name).map(|m| m.name()), Some(*name));
        }
        assert_eq!(MODES.len(), 4);
    }
}
