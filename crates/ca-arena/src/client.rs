//! A client for one Arena, with the person's API key.

use std::path::Path;
use std::time::Duration;

use ca_core::Secret;
use ca_net::NetError;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::types::{Info, Me, Problem};

#[derive(Debug, thiserror::Error)]
pub enum ArenaError {
    #[error(transparent)]
    Net(#[from] NetError),
    /// The key was refused (401): revoked, mistyped, or none.
    #[error("Arena refused the API key: {0}. Make a new one under Your account → API key and run code-arena login")]
    Key(String),
    /// Signed in, but not allowed (403), in Arena's words.
    #[error("{0}")]
    Refused(String),
    /// Arena answered with an error of its own.
    #[error("Arena answered {status}: {message}")]
    Status { status: u16, code: Option<String>, message: String },
    #[error("Arena's answer to {path} could not be read: {message}")]
    Parse { path: String, message: String },
}

/// One answer, kept raw: what capability probing reads.
#[derive(Debug, Clone)]
pub struct Raw {
    pub status: StatusCode,
    pub content_type: String,
    pub body: String,
}

impl Raw {
    /// Arena's own `{"status", "error"}`, when the body is that.
    pub fn problem(&self) -> Option<Problem> {
        if !self.content_type.contains("json") {
            return None;
        }
        serde_json::from_str(&self.body).ok()
    }
}

#[derive(Clone)]
pub struct ArenaClient {
    base: String,
    key: Option<Secret>,
    http: reqwest::Client,
}

impl std::fmt::Debug for ArenaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArenaClient").field("base", &self.base).field("key", &self.key).finish()
    }
}

impl ArenaClient {
    /// `base` is `https://DOMAIN`, without a trailing slash.
    pub fn new(base: &str, key: Option<Secret>, ca: Option<&Path>) -> Result<Self, ArenaError> {
        Ok(Self { base: base.trim_end_matches('/').to_string(), key, http: ca_net::client(ca)? })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Whether a key is sent: a client made with none still reads what Arena says to anyone (`/api/info`).
    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }

    /// A request as Code Arena sends every one: its key, `X-Requested-With` (which Arena asks of every call that
    /// changes something), JSON.
    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.untimed(method, path).header("Accept", "application/json").timeout(Duration::from_secs(30))
    }

    /// A request with no time limit: a stream that stays open (`/watch`).
    pub(crate) fn untimed(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.http.request(method, format!("{}{path}", self.base)).header("X-Requested-With", "code-arena");
        if let Some(k) = &self.key {
            r = r.bearer_auth(k.expose());
        }
        r
    }

    /// Sends and keeps the answer as it is, whatever its status.
    pub async fn raw(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Raw, ArenaError> {
        let mut r = self.request(method, path);
        if let Some(b) = body {
            r = r.json(b);
        }
        let res = r.send().await.map_err(|e| ArenaError::Net(ca_net::explain(&e)))?;
        let status = res.status();
        let content_type =
            res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let body = res.text().await.map_err(|e| ArenaError::Net(ca_net::explain(&e)))?;
        Ok(Raw { status, content_type, body })
    }

    /// A call whose answer is JSON of type `T`; Arena's refusals as [`ArenaError`].
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<T, ArenaError> {
        let raw = self.raw(method, path, body).await?;
        if !raw.status.is_success() {
            return Err(refusal(&raw));
        }
        let text = if raw.body.trim().is_empty() { "null" } else { raw.body.as_str() };
        serde_json::from_str(text).map_err(|e| ArenaError::Parse { path: path.to_string(), message: e.to_string() })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ArenaError> {
        self.call(Method::GET, path, None).await
    }

    /// `GET /api/info`: public; the name, the version (shown, never used to decide anything), Farzan Graph's address.
    pub async fn info(&self) -> Result<Info, ArenaError> {
        self.get("/api/info").await
    }

    /// `GET /api/code-arena/me`: whose the key is.
    pub async fn me(&self) -> Result<Me, ArenaError> {
        self.get("/api/code-arena/me").await
    }

    /// `GET /api/code-arena/config`: the models, thinking levels, tools as the web chat has them.
    pub async fn config(&self) -> Result<Value, ArenaError> {
        self.get("/api/code-arena/config").await
    }

    /// Tokens of each text by the model's own tokenizer (`POST /api/code-arena/tokenize`), and their total.
    pub async fn tokenize(&self, model: &str, texts: &[String]) -> Result<(Vec<u64>, u64), ArenaError> {
        let body = serde_json::json!({"model": model, "texts": texts});
        let v: Value = self.call(Method::POST, "/api/code-arena/tokenize", Some(&body)).await?;
        let counts =
            v["counts"].as_array().map(|a| a.iter().map(|c| c.as_u64().unwrap_or(0)).collect()).unwrap_or_default();
        Ok((counts, v["total"].as_u64().unwrap_or(0)))
    }

    /// When the person's chats compact (Your account → Chat), in percent of the window: theirs, else the company's
    /// default; 0 for only when a request would not fit. `None` when Arena says neither.
    pub async fn compact_at(&self) -> Result<Option<i64>, ArenaError> {
        let v: Value = self.get("/api/code-arena/preferences").await?;
        Ok(v["compactAt"].as_i64().or_else(|| v["compactAtDefault"].as_i64()))
    }

    /// `POST /api/code-arena/sign-out`: the key Code Arena signed in with is revoked.
    pub async fn sign_out(&self) -> Result<(), ArenaError> {
        let raw = self.raw(Method::POST, "/api/code-arena/sign-out", Some(&Value::Object(Default::default()))).await?;
        if raw.status.is_success() { Ok(()) } else { Err(refusal(&raw)) }
    }
}

/// What feedback is, in Arena's words for it: an idea, a problem, praise, or anything else.
pub const FEEDBACK_KINDS: [&str; 4] = ["idea", "problem", "praise", "other"];

impl ArenaClient {
    /// `POST /api/feedback`: what the person says about Arena or Code Arena, kept for the admins (Admin → Feedback),
    /// whose answer comes to the person's bell. `kind` is one of [`FEEDBACK_KINDS`], `page` where it was sent from,
    /// each screenshot a picture's bytes in base64 (at most 3, of 2 MB each, as Arena takes them).
    pub async fn feedback(&self, kind: &str, text: &str, page: &str, screenshots: &[String]) -> Result<(), ArenaError> {
        let mut body = serde_json::json!({"kind": kind, "text": text, "page": page});
        if !screenshots.is_empty() {
            body["images"] = screenshots.iter().map(|d| serde_json::json!({"data": d})).collect();
        }
        let raw = self.raw(Method::POST, "/api/feedback", Some(&body)).await?;
        if raw.status.is_success() { Ok(()) } else { Err(refusal(&raw)) }
    }
}

pub(crate) fn refusal(raw: &Raw) -> ArenaError {
    let problem = raw.problem();
    let message = problem
        .as_ref()
        .and_then(|p| p.error.clone())
        .unwrap_or_else(|| raw.status.canonical_reason().unwrap_or("no reason given").to_string());
    match raw.status {
        StatusCode::UNAUTHORIZED => ArenaError::Key(message),
        StatusCode::FORBIDDEN => ArenaError::Refused(message),
        s => ArenaError::Status { status: s.as_u16(), code: problem.map(|p| p.status), message },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn sends_the_key_and_the_header_arena_asks_for() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/code-arena/me"))
            .and(header("authorization", "Bearer sk-test"))
            .and(header("x-requested-with", "code-arena"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"userName": "sara"})))
            .mount(&server)
            .await;
        let c = ArenaClient::new(&server.uri(), Some(Secret::new("sk-test")), None).expect("client");
        assert_eq!(c.me().await.expect("me").user_name, "sara");
    }

    #[tokio::test]
    async fn refusals_in_arenas_words() {
        let server = MockServer::start().await;
        Mock::given(path("/api/code-arena/me"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(serde_json::json!({"status": "key", "error": "That key was revoked."})),
            )
            .mount(&server)
            .await;
        Mock::given(path("/api/code-arena/config"))
            .respond_with(ResponseTemplate::new(403).set_body_json(
                serde_json::json!({"status": "feature", "error": "Code Arena is not among what you may use: ask an admin."}),
            ))
            .mount(&server)
            .await;
        let c = ArenaClient::new(&server.uri(), Some(Secret::new("sk-x")), None).expect("client");
        match c.me().await {
            Err(ArenaError::Key(m)) => assert_eq!(m, "That key was revoked."),
            other => panic!("{other:?}"),
        }
        match c.config().await {
            Err(ArenaError::Refused(m)) => assert!(m.contains("ask an admin")),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn feedback_goes_to_arena_with_its_kind_and_page() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/feedback"))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "kind": "idea", "text": "More themes", "page": "Code Arena", "images": [{"data": "iVBOR"}]
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": "f-1"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/feedback"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({"kind": "other"})))
            .respond_with(
                ResponseTemplate::new(429)
                    .set_body_json(serde_json::json!({"status": "limit", "error": "At most 20 a day."})),
            )
            .mount(&server)
            .await;
        let c = ArenaClient::new(&server.uri(), Some(Secret::new("sk-x")), None).expect("client");
        assert!(c.has_key());
        c.feedback("idea", "More themes", "Code Arena", &["iVBOR".into()]).await.expect("sent");
        match c.feedback("other", "x", "Code Arena", &[]).await {
            Err(ArenaError::Status { status: 429, message, .. }) => assert_eq!(message, "At most 20 a day."),
            other => panic!("{other:?}"),
        }
        assert!(!ArenaClient::new(&server.uri(), None, None).expect("client").has_key());
    }

    #[test]
    fn the_key_is_not_in_debug() {
        let c = ArenaClient::new("https://a.example", Some(Secret::new("sk-secret")), None).expect("client");
        assert!(!format!("{c:?}").contains("sk-secret"));
    }
}
