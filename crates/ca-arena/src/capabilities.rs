//! What an Arena has, found out by asking it.
//!
//! Each optional feature is probed once with a harmless request, and its answer is read so:
//!
//! - an unknown route answers 404 (or 405) with no JSON (Arena's `MapFallback` and ASP.NET's status pages): **absent**;
//! - a feature an admin has not installed answers 404 `{"status": "feature-off"}`: **off**, with what turns it on;
//! - a route that answers anything else (200, Arena's own JSON 404 for a thing not there, 400 for a body it did not
//!   like) **is there**; 403 says it is there but not the person's to use.
//!
//! Versions are never compared: an Arena 8.0 lacks `checklists.edit`, `project_work` and `automation.*` because it
//! answers so, and a later one that adds them is used for them at once.

use std::collections::BTreeMap;

use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{ArenaClient, ArenaError, Raw};

/// An id no chat or checklist has: the probes ask about it, so nothing is read or changed.
const NOBODY: &str = "00000000-0000-0000-0000-000000000000";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Sessions kept in step with chats (`/chats`, messages, turns, drafts, the web's queue).
    ChatSync,
    Memory,
    /// Skills in Arena (list, make, change, import, learn).
    Skills,
    /// Turning checklists on in a chat and ticking them.
    Checklists,
    /// Making, editing and deleting checklists (8.1 on).
    ChecklistsEdit,
    Goals,
    Plans,
    Projects,
    /// The project's goals and tasks from a chat (`project_plan`, `project_goal`, `project_task`; 8.1 on).
    ProjectWork,
    Assistants,
    /// Agent Studio's agents run from Code Arena (8.1 on).
    AutomationAgents,
    /// The person's scheduled tasks in Arena (8.1 on).
    AutomationScheduled,
    /// The model's own tokenizer for the context view.
    Tokenize,
    /// The person's past chats and sessions by what they were about.
    History,
    Broadcasts,
}

impl Feature {
    pub const ALL: &'static [Feature] = &[
        Feature::ChatSync,
        Feature::Memory,
        Feature::Skills,
        Feature::Checklists,
        Feature::ChecklistsEdit,
        Feature::Goals,
        Feature::Plans,
        Feature::Projects,
        Feature::ProjectWork,
        Feature::Assistants,
        Feature::AutomationAgents,
        Feature::AutomationScheduled,
        Feature::Tokenize,
        Feature::History,
        Feature::Broadcasts,
    ];

    /// The name `doctor` prints: `checklists.edit`, `automation.agents`.
    pub fn name(self) -> &'static str {
        match self {
            Feature::ChatSync => "chat_sync",
            Feature::Memory => "memory",
            Feature::Skills => "skills",
            Feature::Checklists => "checklists",
            Feature::ChecklistsEdit => "checklists.edit",
            Feature::Goals => "goals",
            Feature::Plans => "plans",
            Feature::Projects => "projects",
            Feature::ProjectWork => "project_work",
            Feature::Assistants => "assistants",
            Feature::AutomationAgents => "automation.agents",
            Feature::AutomationScheduled => "automation.scheduled",
            Feature::Tokenize => "tokenize",
            Feature::History => "history",
            Feature::Broadcasts => "broadcasts",
        }
    }

    /// The harmless request that tells whether the route is there.
    fn probe(self) -> (Method, String, Option<Value>) {
        let api = "/api/code-arena";
        match self {
            Feature::ChatSync => (Method::GET, format!("{api}/chats?limit=1"), None),
            Feature::Memory => (Method::GET, format!("{api}/memories"), None),
            Feature::Skills => (Method::GET, format!("{api}/skills"), None),
            Feature::Checklists => (Method::GET, format!("{api}/checklists"), None),
            // A change to a checklist no one has: 8.1 answers that it is not there (or that the body will not do);
            // 8.0 has no such route.
            Feature::ChecklistsEdit => (Method::PUT, format!("{api}/checklists/{NOBODY}"), Some(json!({}))),
            Feature::Goals => (Method::GET, format!("{api}/chats/{NOBODY}/goal"), None),
            Feature::Plans => (Method::GET, format!("{api}/chats/{NOBODY}/plan"), None),
            Feature::Projects => (Method::GET, format!("{api}/projects"), None),
            Feature::ProjectWork => (Method::GET, format!("{api}/chats/{NOBODY}/project-work"), None),
            Feature::Assistants => (Method::GET, format!("{api}/assistants"), None),
            Feature::AutomationAgents => (Method::GET, format!("{api}/agents"), None),
            Feature::AutomationScheduled => (Method::GET, format!("{api}/scheduled"), None),
            // No model named: a 400 of Arena's says the route is there, and nothing is counted.
            Feature::Tokenize => (Method::POST, format!("{api}/tokenize"), Some(json!({}))),
            Feature::History => (Method::POST, format!("{api}/history/search"), Some(json!({}))),
            Feature::Broadcasts => (Method::GET, format!("{api}/broadcasts"), None),
        }
    }
}

/// Where a feature stands on one server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "why", rename_all = "snake_case")]
pub enum State {
    On,
    /// The server has it, but the person may not use it (403), in Arena's words.
    Denied(String),
    /// The server has it, but an admin has not installed it, with what turns it on.
    Off(String),
    /// This server has no such route.
    Absent,
    /// The probe got no answer that says (a 5xx, the network); asked again next time.
    Unknown(String),
}

impl State {
    pub fn usable(&self) -> bool {
        matches!(self, State::On)
    }
}

/// What a server answered, per feature.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub features: BTreeMap<Feature, State>,
}

impl Capabilities {
    pub fn has(&self, f: Feature) -> bool {
        self.features.get(&f).is_some_and(State::usable)
    }

    pub fn state(&self, f: Feature) -> State {
        self.features.get(&f).cloned().unwrap_or(State::Unknown("not asked".into()))
    }

    /// Whether any probe got no clear answer, so the set should be asked again soon.
    pub fn incomplete(&self) -> bool {
        self.features.values().any(|s| matches!(s, State::Unknown(_)))
    }
}

/// A probe's answer, read.
pub fn classify(raw: &Raw) -> State {
    let problem = raw.problem();
    let words = || problem.as_ref().and_then(|p| p.error.clone()).unwrap_or_default();
    match raw.status {
        s if s.is_success() => State::On,
        StatusCode::FORBIDDEN => State::Denied(words()),
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => match &problem {
            Some(p) if p.status == "feature-off" => State::Off(words()),
            // Arena's own "not there" for the id asked about: the route is there.
            Some(_) => State::On,
            None => State::Absent,
        },
        s if s.is_server_error() => State::Unknown(format!("{} {}", s.as_u16(), s.canonical_reason().unwrap_or(""))),
        // 400, 409, 422…: the route read the request and said no to its body.
        _ => State::On,
    }
}

impl Feature {
    pub fn from_name(name: &str) -> Option<Feature> {
        Feature::ALL.iter().copied().find(|f| f.name() == name)
    }
}

/// What a server answered, kept for the next start: asked again after [`KEEP_SECS`], when the probe was not
/// complete, or when the account's settings stamp (`GET /settings`) moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    pub url: String,
    /// Seconds since 1970.
    pub probed_at: u64,
    #[serde(default)]
    pub stamp: Option<String>,
    pub capabilities: Capabilities,
}

/// How long what a server answered is trusted.
pub const KEEP_SECS: u64 = 3600;

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Kept {
    /// Where a server's answers are kept in `dir` (the data folder's `servers/`): one file per host.
    pub fn file(dir: &std::path::Path, url: &str) -> std::path::PathBuf {
        let host = url.split("://").nth(1).unwrap_or(url).replace([':', '/'], "_");
        dir.join(format!("{host}.json"))
    }

    pub fn read(file: &std::path::Path) -> Option<Kept> {
        serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
    }

    pub fn write(&self, file: &std::path::Path) {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(file, text);
        }
    }

    /// Whether it still holds for this server, now that its stamp is `stamp`.
    pub fn holds(&self, url: &str, stamp: Option<&str>, now: u64) -> bool {
        self.url == url
            && now.saturating_sub(self.probed_at) < KEEP_SECS
            && !self.capabilities.incomplete()
            && (stamp.is_none() || self.stamp.as_deref() == stamp)
    }
}

impl ArenaClient {
    /// The account's settings stamp (`GET /api/code-arena/settings`), which moves when its models, tools or skills change.
    pub async fn settings_stamp(&self) -> Option<String> {
        let v: Value = self.get("/api/code-arena/settings").await.ok()?;
        v.get("stamp").and_then(Value::as_str).map(str::to_string)
    }

    /// What this server has, from `file` while it holds, else asked and kept there.
    pub async fn capabilities_kept(&self, file: &std::path::Path) -> Result<Capabilities, ArenaError> {
        let stamp = self.settings_stamp().await;
        if let Some(k) = Kept::read(file)
            && k.holds(self.base(), stamp.as_deref(), now_secs())
        {
            return Ok(k.capabilities);
        }
        let caps = self.capabilities().await?;
        Kept { url: self.base().to_string(), probed_at: now_secs(), stamp, capabilities: caps.clone() }.write(file);
        Ok(caps)
    }

    /// A later Arena's own list (`GET /api/code-arena/capabilities` answering `{"features": ["chat_sync", …]}`): what
    /// it names is on, the rest it lacks. `None` when the server has no such route, or its answer is not that (a bare
    /// array is not taken: a catch-all answering `[]` would otherwise turn everything off).
    async fn listed(&self) -> Result<Option<Capabilities>, ArenaError> {
        let raw = match self.raw(Method::GET, "/api/code-arena/capabilities", None).await {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };
        if raw.status == StatusCode::UNAUTHORIZED {
            return Err(crate::client::refusal(&raw));
        }
        if !raw.status.is_success() || !raw.content_type.contains("json") {
            return Ok(None);
        }
        let Ok(v) = serde_json::from_str::<Value>(&raw.body) else { return Ok(None) };
        let Some(names) = v.get("features").and_then(Value::as_array) else {
            return Ok(None);
        };
        let on: Vec<Feature> = names.iter().filter_map(Value::as_str).filter_map(Feature::from_name).collect();
        let mut caps = Capabilities::default();
        for &f in Feature::ALL {
            caps.features.insert(f, if on.contains(&f) { State::On } else { State::Absent });
        }
        Ok(Some(caps))
    }

    /// Asks the server about every feature: its own list when it has one, else each probed, all at once. A key the
    /// server refuses stops it: nothing can be said then.
    pub async fn capabilities(&self) -> Result<Capabilities, ArenaError> {
        if let Some(caps) = self.listed().await? {
            return Ok(caps);
        }
        let probes = Feature::ALL.iter().map(|&f| async move {
            let (method, path, body) = f.probe();
            (f, self.raw(method, &path, body.as_ref()).await)
        });
        let mut caps = Capabilities::default();
        for (f, answer) in futures_util::future::join_all(probes).await {
            let state = match answer {
                Ok(raw) if raw.status == StatusCode::UNAUTHORIZED => return Err(crate::client::refusal(&raw)),
                Ok(raw) => classify(&raw),
                Err(e) => State::Unknown(e.to_string()),
            };
            caps.features.insert(f, state);
        }
        Ok(caps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ca_core::Secret;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn raw(status: u16, content_type: &str, body: &str) -> Raw {
        Raw {
            status: StatusCode::from_u16(status).expect("status"),
            content_type: content_type.into(),
            body: body.into(),
        }
    }

    #[test]
    fn answers_are_read_as_the_server_means_them() {
        // ASP.NET's status page for an unknown route.
        assert_eq!(classify(&raw(404, "text/plain", "Status Code: 404; Not Found")), State::Absent);
        assert_eq!(classify(&raw(404, "", "")), State::Absent);
        assert_eq!(classify(&raw(405, "", "")), State::Absent);
        assert_eq!(
            classify(&raw(
                404,
                "application/json",
                r#"{"status":"feature-off","error":"Projects is not installed here."}"#
            )),
            State::Off("Projects is not installed here.".into())
        );
        assert_eq!(
            classify(&raw(404, "application/json; charset=utf-8", r#"{"status":"gone","error":"x"}"#)),
            State::On
        );
        assert_eq!(classify(&raw(400, "text/plain", "Status Code: 400; Bad Request")), State::On);
        assert_eq!(classify(&raw(200, "application/json", "[]")), State::On);
        assert_eq!(
            classify(&raw(403, "application/json", r#"{"status":"feature","error":"ask an admin"}"#)),
            State::Denied("ask an admin".into())
        );
        assert!(matches!(classify(&raw(502, "", "")), State::Unknown(_)));
    }

    /// An Arena as 8.0 answers: the routes 8.1 added are not there.
    async fn arena_8_0() -> MockServer {
        let server = MockServer::start().await;
        let absent = ["/api/code-arena/agents", "/api/code-arena/scheduled"];
        for p in absent {
            Mock::given(path(p))
                .respond_with(ResponseTemplate::new(404).set_body_string("Status Code: 404; Not Found"))
                .mount(&server)
                .await;
        }
        Mock::given(path(format!("/api/code-arena/chats/{NOBODY}/project-work")))
            .respond_with(ResponseTemplate::new(404).set_body_string("Status Code: 404; Not Found"))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(format!("/api/code-arena/checklists/{NOBODY}")))
            .respond_with(ResponseTemplate::new(404).set_body_string("Status Code: 404; Not Found"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/code-arena/chats/{NOBODY}/goal")))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(
                    json!({"status": "gone", "error": "That chat is not there any more: it was deleted."}),
                ),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({"status": "invalid", "error": "No model."})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn an_8_0_server_lacks_what_8_1_added() {
        let server = arena_8_0().await;
        let c = ArenaClient::new(&server.uri(), Some(Secret::new("sk-test")), None).expect("client");
        let caps = c.capabilities().await.expect("probed");
        for f in
            [Feature::AutomationAgents, Feature::AutomationScheduled, Feature::ProjectWork, Feature::ChecklistsEdit]
        {
            assert_eq!(caps.state(f), State::Absent, "{}", f.name());
        }
        for f in [
            Feature::ChatSync,
            Feature::Memory,
            Feature::Checklists,
            Feature::Goals,
            Feature::Tokenize,
            Feature::History,
        ] {
            assert!(caps.has(f), "{}: {:?}", f.name(), caps.state(f));
        }
        assert!(!caps.incomplete());
    }

    #[tokio::test]
    async fn a_refused_key_stops_the_probe() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({"status": "key", "error": "No such key."})))
            .mount(&server)
            .await;
        let c = ArenaClient::new(&server.uri(), Some(Secret::new("sk-bad")), None).expect("client");
        assert!(matches!(c.capabilities().await, Err(ArenaError::Key(_))));
    }

    #[test]
    fn capabilities_round_trip_as_json() {
        let mut caps = Capabilities::default();
        caps.features.insert(Feature::Projects, State::Off("not installed".into()));
        caps.features.insert(Feature::Memory, State::On);
        let text = serde_json::to_string(&caps).expect("json");
        assert_eq!(serde_json::from_str::<Capabilities>(&text).expect("back"), caps);
    }
}
