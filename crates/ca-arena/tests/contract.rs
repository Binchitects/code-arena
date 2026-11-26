//! Each Arena's answers to the probes, replayed: what Code Arena makes of 8.0 and of 8.1, and of a later Arena
//! that lists what it has. The fixtures hold the answers; the plan's route table says the same in words.

use ca_arena::{ArenaClient, Feature, Kept, State};
use ca_core::Secret;
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn replay(fixture: &str) -> MockServer {
    let text =
        std::fs::read_to_string(format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"))).expect("fixture");
    let v: Value = serde_json::from_str(&text).expect("json");
    let server = MockServer::start().await;
    for a in v["answers"].as_array().expect("answers") {
        let mut r = ResponseTemplate::new(a["status"].as_u64().expect("status") as u16);
        let ct = a["contentType"].as_str().unwrap_or_default();
        let body = a["body"].as_str().unwrap_or_default().to_string();
        r = if ct.is_empty() { r.set_body_string(body) } else { r.set_body_raw(body, ct) };
        Mock::given(method(a["method"].as_str().expect("method")))
            .and(path(a["path"].as_str().expect("path")))
            .respond_with(r)
            .mount(&server)
            .await;
    }
    server
}

fn client(server: &MockServer) -> ArenaClient {
    ArenaClient::new(&server.uri(), Some(Secret::new("sk-test")), None).expect("client")
}

const ONLY_8_1: [Feature; 4] =
    [Feature::ChecklistsEdit, Feature::ProjectWork, Feature::AutomationAgents, Feature::AutomationScheduled];

#[tokio::test]
async fn arena_8_0() {
    let server = replay("arena-8.0.json").await;
    let caps = client(&server).capabilities().await.expect("probed");
    for f in Feature::ALL {
        let want = if ONLY_8_1.contains(f) { State::Absent } else { State::On };
        assert_eq!(caps.state(*f), want, "{}", f.name());
    }
}

#[tokio::test]
async fn arena_8_1() {
    let server = replay("arena-8.1.json").await;
    let caps = client(&server).capabilities().await.expect("probed");
    for f in Feature::ALL {
        assert_eq!(caps.state(*f), State::On, "{}", f.name());
    }
}

#[tokio::test]
async fn a_later_arena_that_lists_what_it_has_is_not_probed() {
    let server = MockServer::start().await;
    Mock::given(path("/api/code-arena/capabilities"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"features": ["chat_sync", "memory", "something_new"]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let caps = client(&server).capabilities().await.expect("listed");
    assert!(caps.has(Feature::ChatSync) && caps.has(Feature::Memory));
    assert_eq!(caps.state(Feature::Projects), State::Absent);
    // Nothing else was asked: only the list's route is mounted, and wiremock answers others 404, which would be Absent
    // too, so count the requests.
    assert_eq!(server.received_requests().await.map(|r| r.len()), Some(1));
}

#[tokio::test]
async fn what_was_answered_is_kept_until_the_settings_move() {
    let server = replay("arena-8.0.json").await;
    let dir = tempfile::tempdir().expect("temp");
    let file = Kept::file(dir.path(), &server.uri());
    let c = client(&server);
    let first = c.capabilities_kept(&file).await.expect("probed");
    let asked = server.received_requests().await.map(|r| r.len()).unwrap_or(0);
    // Asked again: only the settings stamp.
    assert_eq!(c.capabilities_kept(&file).await.expect("kept"), first);
    assert_eq!(server.received_requests().await.map(|r| r.len()).unwrap_or(0), asked + 1);
    // Another stamp: probed again.
    let mut k = Kept::read(&file).expect("kept file");
    k.stamp = Some("older".into());
    k.write(&file);
    c.capabilities_kept(&file).await.expect("probed again");
    assert!(server.received_requests().await.map(|r| r.len()).unwrap_or(0) > asked + 2);
    // An hour old: probed again.
    let k = Kept::read(&file).expect("kept file");
    assert!(!k.holds(&k.url, k.stamp.as_deref(), k.probed_at + ca_arena::capabilities::KEEP_SECS));
    assert!(k.holds(&k.url, k.stamp.as_deref(), k.probed_at + 60));
}
