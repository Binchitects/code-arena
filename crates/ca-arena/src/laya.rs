//! Laya, Arena's reading of a command before it runs: how likely it destroys something, reaches outside the
//! workspace, and uses the network. An Arena without Laya answers 404, and the command is asked about as before.

use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::json;

use crate::client::{ArenaClient, ArenaError, refusal};

/// Where Laya reads a command.
pub const ROUTE: &str = "/api/code-arena/laya/check";

/// Laya's reading, each likelihood from 0 to 1. All three are needed: an answer without them is no reading, never
/// an all-clear.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Reading {
    pub destructive: f64,
    pub outside: f64,
    pub network: f64,
}

impl ArenaClient {
    /// Laya's reading of a command run in `cwd`; `None` when this Arena has no Laya, or answers with something
    /// that is not a reading.
    pub async fn laya_check(&self, command: &str, cwd: &str) -> Result<Option<Reading>, ArenaError> {
        let body = json!({"command": command, "cwd": cwd});
        let raw = self.raw(Method::POST, ROUTE, Some(&body)).await?;
        match raw.status {
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED => Ok(None),
            s if s.is_success() => Ok(reading(&raw.body)),
            _ => Err(refusal(&raw)),
        }
    }
}

/// A reading from an answer's body; none when it lacks one of the likelihoods.
fn reading(body: &str) -> Option<Reading> {
    serde_json::from_str(body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_needs_all_three_likelihoods() {
        let r = reading(r#"{"destructive": 0.96, "outside": 0.93, "network": 0.03}"#);
        assert_eq!(r, Some(Reading { destructive: 0.96, outside: 0.93, network: 0.03 }));
        assert_eq!(reading("{}"), None);
        assert_eq!(reading(r#"{"destructive": 0.96, "outside": 0.93}"#), None);
        assert_eq!(reading(r#"{"error": "busy"}"#), None);
    }
}
