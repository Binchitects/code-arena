//! Arena's broadcasts to everyone signed in (a notice, a warning, something critical): the ones to show, and
//! one dismissed.
//!
//! `GET /api/code-arena/broadcasts` answers the list, bare or as `{"broadcasts": […]}` (or `items`); each is read as
//! it comes, so a field Arena names otherwise is still found: its `id` (a string or a number), its `level` (`info`,
//! `warning`, `critical`; `severity` or `type` too, anything else is `info`), its `text` (or `message`, `title`)
//! and when it was made (`created_at`, `createdAt`, `at`), by which the newest are first. One with no id or no text is
//! left out. `POST /api/code-arena/broadcasts/{id}/dismiss` hides one for the person; a critical one stays.

use reqwest::Method;
use serde::Serialize;
use serde_json::Value;

use crate::client::{ArenaClient, ArenaError};

/// How much a broadcast matters: a critical one cannot be dismissed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    Warning,
    Critical,
}

impl Level {
    fn of(word: &str) -> Level {
        match word.to_ascii_lowercase().as_str() {
            "warning" | "warn" => Level::Warning,
            "critical" | "error" | "danger" => Level::Critical,
            _ => Level::Info,
        }
    }
}

/// One broadcast as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Broadcast {
    pub id: String,
    pub level: Level,
    pub text: String,
    /// When it was made, as Arena said it (ISO 8601 sorts as text); empty when not said.
    #[serde(skip)]
    pub at: String,
}

/// The broadcasts in an answer of Arena's, the newest first (those it gives no time keep its order, after them).
pub fn read(v: &Value) -> Vec<Broadcast> {
    let list = v.as_array().or_else(|| v["broadcasts"].as_array()).or_else(|| v["items"].as_array());
    let first = |b: &Value, names: &[&str]| -> String {
        names
            .iter()
            .find_map(|n| match &b[*n] {
                Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .unwrap_or_default()
    };
    let mut out: Vec<Broadcast> = list
        .into_iter()
        .flatten()
        .filter_map(|b| {
            let id = first(b, &["id", "_id"]);
            let text = first(b, &["text", "message", "title"]);
            (!id.is_empty() && !text.is_empty()).then(|| Broadcast {
                id,
                level: Level::of(&first(b, &["level", "severity", "type"])),
                text,
                at: first(b, &["created_at", "createdAt", "at"]),
            })
        })
        .collect();
    // Stable: those of the same time (or none) keep Arena's order.
    out.sort_by(|a, b| b.at.cmp(&a.at));
    out
}

const API: &str = "/api/code-arena";

impl ArenaClient {
    /// The broadcasts to show, the newest first. An Arena without them (404) has none.
    pub async fn broadcasts(&self) -> Result<Vec<Broadcast>, ArenaError> {
        match self.get::<Value>(&format!("{API}/broadcasts")).await {
            Ok(v) => Ok(read(&v)),
            Err(ArenaError::Status { status: 404, .. }) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// One broadcast dismissed for the person.
    pub async fn dismiss_broadcast(&self, id: &str) -> Result<(), ArenaError> {
        let id: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
        self.call::<Value>(Method::POST, &format!("{API}/broadcasts/{id}/dismiss"), None).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[test]
    fn broadcasts_are_read_as_arena_gives_them_the_newest_first() {
        let v = json!({"broadcasts": [
            {"id": 1, "level": "info", "text": "Maintenance tonight", "created_at": "2026-11-01T10:00:00Z"},
            {"id": "b", "severity": "CRITICAL", "message": "Gateway down", "created_at": "2026-11-02T10:00:00Z"},
            {"id": "c", "type": "warning", "title": "Slow models"},
            {"id": "d", "text": "  "},
            {"text": "no id"},
        ]});
        let got = read(&v);
        let ids: Vec<&str> = got.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["b", "1", "c"]);
        assert_eq!(got[0].level, Level::Critical);
        assert_eq!(got[1].level, Level::Info);
        assert_eq!(got[2].level, Level::Warning);
        assert_eq!(got[0].text, "Gateway down");
        assert_eq!(read(&json!([{"id": "x", "text": "bare"}]))[0].id, "x");
        assert!(read(&json!({"error": "nothing"})).is_empty());
        assert_eq!(serde_json::to_value(&got[0]).expect("json"), json!({"id": "b", "level": "critical", "text": "Gateway down"}));
    }

    #[tokio::test]
    async fn an_arena_without_broadcasts_has_none_and_one_is_dismissed_by_its_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/code-arena/broadcasts/b-1/dismiss"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        let arena = ArenaClient::new(&server.uri(), None, None).expect("client");
        assert_eq!(arena.broadcasts().await.expect("none"), []);
        arena.dismiss_broadcast("b-1/../x").await.expect_err("an id is only its own characters");
        arena.dismiss_broadcast("b-1").await.expect("dismissed");
    }
}
