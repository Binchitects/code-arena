use super::*;
use ca_core::Secret;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let d = tempfile::tempdir().expect("temp");
    let (c, data, repo) = (d.path().join("config"), d.path().join("data"), d.path().join("repo"));
    std::fs::create_dir_all(repo.join(".git")).expect("repo");
    (d, c, data, repo)
}

#[tokio::test]
async fn with_arena_the_files_go_to_it_and_its_block_starts_the_session() {
    let (_d, config, data, repo) = dirs();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/memories/import"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"kept": 1, "skipped": 0})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/code-arena/memories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "on": true, "note": "- The tests run with nextest.", "inNote": ["m1"],
            "memories": [{"id": "m1", "text": "The tests run with nextest.", "scope": "project", "kind": "howto", "pinned": false, "mine": true}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/memories"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "m2", "text": "x", "scope": "person", "kind": "preference", "mine": true})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/memories/recall"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "note": "(Memories that may matter to this question; use them where they help:\n- Deploys go through the staging branch.)",
            "memories": [{"id": "m3", "text": "Deploys go through the staging branch.", "kind": "fact", "scope": "project", "score": 0.8}]
        })))
        .mount(&server)
        .await;
    let arena = ArenaClient::new(&server.uri(), Some(Secret::new("sk-test")), None).expect("client");
    let m = Memory::new(Some(arena), &repo, &config, &data, None);
    m.files.add(true, "Answers in short sentences").expect("file");
    let said = std::sync::Mutex::new(Vec::new());
    let prompt = m.start(&|s| said.lock().expect("said").push(s)).await;
    assert!(prompt.contains("# Memory\n\n- The tests run with nextest."), "{prompt}");
    assert!(said.lock().expect("said")[0].contains("went to Arena"));
    assert!(m.files.read(true).is_empty(), "the file went to Arena");
    assert_eq!(m.remember(true, "Prefers tabs", "preference").await, Ok((true, "for every project, in Arena".into())));
    let note = m.recall("how do we deploy?", 1500).await.expect("found");
    assert!(note.contains("staging branch"));
    let reqs = server.received_requests().await.expect("requests");
    let recall = reqs.iter().find(|r| r.url.path().ends_with("/recall")).expect("recall");
    let body: serde_json::Value = serde_json::from_slice(&recall.body).expect("json");
    // What the block gave already is not given again.
    assert_eq!(body["besides"], json!(["m1"]));
    assert_eq!(m.listed().len(), 2);
}

#[tokio::test]
async fn an_arena_that_denies_memory_gets_nothing_kept_or_recalled() {
    let (_d, config, data, repo) = dirs();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/code-arena/memories"))
        .respond_with(ResponseTemplate::new(403).set_body_json(
            json!({"status": "feature", "error": "Memory is not among what you may use: ask an admin."}),
        ))
        .mount(&server)
        .await;
    let arena = ArenaClient::new(&server.uri(), Some(Secret::new("sk-test")), None).expect("client");
    let m = Memory::new(Some(arena), &repo, &config, &data, None);
    assert_eq!(m.start(&|_| {}).await, "");
    assert!(m.denied());
    assert_eq!(m.remember(false, "x", "fact").await, Err(DENIED.to_string()));
    assert!(m.recall("anything", 100).await.is_none());
    assert!(tools(Arc::new(m)).iter().all(|t| t.def().name != "remember"));
}

#[tokio::test]
async fn without_arena_the_files_and_what_arena_last_gave() {
    let (_d, config, data, repo) = dirs();
    let m = Memory::new(None, &repo, &config, &data, None);
    std::fs::create_dir_all(m.files.cache.parent().expect("dir")).expect("dir");
    std::fs::write(&m.files.cache, json!({"note": "- From Arena, last time."}).to_string()).expect("cache");
    assert_eq!(m.remember(false, "Build with make ci", "howto").await.map(|r| r.0), Ok(true));
    let prompt = m.start(&|_| {}).await;
    assert!(prompt.contains("- Build with make ci"));
    assert!(prompt.contains("# Memory (as Arena last gave it)\n\n- From Arena, last time."));
    assert!(m.recall("how do I build it?", 500).await.expect("found").contains("make ci"));
    // Given once: not again.
    assert!(m.recall("how do I build it?", 500).await.is_none());
    assert_eq!(m.forget(1).await, Ok("Build with make ci".into()));
    assert!(m.files.read(false).is_empty());
}

use std::sync::Arc;

#[tokio::test]
async fn remember_project_and_skill_save_write_in_the_repository() {
    let (_d, config, data, repo) = dirs();
    let m = Arc::new(Memory::new(None, &repo, &config, &data, None));
    let ws = ca_workspace::Workspace::open(&repo, &[]).expect("ws");
    let ctx = ca_tools::Context::new(Arc::new(ws), None, std::time::Duration::from_secs(5));
    let all = tools(m.clone());
    let by = |n: &str| all.iter().find(|t| t.def().name == n).expect("tool").clone();
    let o = by("remember_project").run(json!({"section": "How to build", "text": "make ci"}), &ctx).await;
    assert!(!o.is_error, "{}", o.text);
    assert!(
        std::fs::read_to_string(repo.join(".remember/MEMORY.md")).expect("kept").contains("## How to build\n\nmake ci")
    );
    let o = by("skill_save")
        .run(json!({"name": "cut-a-release", "description": "When a version is released", "instructions": "1. Date the CHANGELOG.\n2. Tag it.\n3. Push the tag."}), &ctx)
        .await;
    assert!(!o.is_error, "{}", o.text);
    let skill = std::fs::read_to_string(repo.join(".code-arena/skills/cut-a-release/SKILL.md")).expect("skill");
    assert!(skill.starts_with("---\nname: cut-a-release\ndescription: When a version is released\n"));
    let o = by("skill_save").run(json!({"name": "Bad Name", "description": "x", "instructions": "y"}), &ctx).await;
    assert!(o.is_error);
    // No Arena: the person's skills are not there to keep in.
    let o = by("skill_save")
        .run(json!({"name": "mine", "scope": "person", "description": "x", "instructions": "a long enough set of steps to be a skill"}), &ctx)
        .await;
    assert!(o.is_error && o.text.contains("not signed in"));
}
