//! Laya, Arena's reading of a command before it runs: how likely it destroys something, reaches outside the
//! workspace, and uses the network. An Arena without Laya answers 404, and the command is asked about as before.

use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::json;

use crate::client::{ArenaClient, ArenaError, refusal};

/// Where Laya reads a command.
pub const ROUTE: &str = "/api/code-arena/laya/check";

/// Laya's reading, each likelihood from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize)]
pub struct Reading {
    #[serde(default)]
    pub destructive: f64,
    #[serde(default)]
    pub outside: f64,
    #[serde(default)]
    pub network: f64,
}

impl ArenaClient {
    /// Laya's reading of a command run in `cwd`; `None` when this Arena has no Laya.
    pub async fn laya_check(&self, command: &str, cwd: &str) -> Result<Option<Reading>, ArenaError> {
        let body = json!({"command": command, "cwd": cwd});
        let raw = self.raw(Method::POST, ROUTE, Some(&body)).await?;
        match raw.status {
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED => Ok(None),
            s if s.is_success() => serde_json::from_str(&raw.body)
                .map(Some)
                .map_err(|e| ArenaError::Parse { path: ROUTE.to_string(), message: e.to_string() }),
            _ => Err(refusal(&raw)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_takes_what_it_is_given() {
        let r: Reading = serde_json::from_str(r#"{"destructive": 0.96, "outside": 0.93}"#).unwrap();
        assert_eq!(r, Reading { destructive: 0.96, outside: 0.93, network: 0.0 });
    }
}
