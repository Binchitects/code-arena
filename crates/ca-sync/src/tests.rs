use super::*;
use ca_core::Secret;
use ca_llm::{Message, ToolCall};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Arena's append: each message gets the id `a` + its ref, and the request is kept to be looked at.
struct Takes(Arc<Mutex<Vec<Value>>>);

impl Respond for Takes {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&r.body).unwrap_or_default();
        let ids: Vec<String> = body["messages"]
            .as_array()
            .map(|ms| ms.iter().map(|m| format!("a{}", m["ref"].as_str().unwrap_or_default())).collect())
            .unwrap_or_default();
        if let Ok(mut seen) = self.0.lock() {
            seen.push(body);
        }
        ResponseTemplate::new(200).set_body_json(json!({"ids": ids, "count": 0, "stamp": "s", "summarized": false}))
    }
}

async fn arena() -> (MockServer, Arc<Mutex<Vec<Value>>>) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "chat-1", "created": true})))
        .expect(1)
        .mount(&server)
        .await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/chat-1/messages"))
        .respond_with(Takes(Arc::clone(&seen)))
        .mount(&server)
        .await;
    (server, seen)
}

fn session(store: &Store) -> String {
    let id = store.create("/w", "qwen", "ask").expect("create");
    store.append(&id, &Message::user("read the parser"), true).expect("u");
    let mut a = Message::assistant("");
    a.tool_calls.push(ToolCall { id: "c1".into(), name: "read_file".into(), arguments: "{\"path\":\"p.rs\"}".into() });
    store.append(&id, &a, false).expect("call");
    store.append(&id, &Message::tool("c1", "fn parse() {}"), false).expect("result");
    store.append(&id, &Message::assistant("It parses."), false).expect("answer");
    id
}

fn sync(server: &MockServer, store: &Arc<Mutex<Store>>, id: &str) -> (Arc<ChatSync>, Arc<Mutex<Vec<String>>>) {
    let said = Arc::new(Mutex::new(Vec::new()));
    let s2 = Arc::clone(&said);
    let notice: Notice = Arc::new(move |_, w| s2.lock().expect("said").push(w));
    let arena = ArenaClient::new(&server.uri(), Some(Secret::new("sk-test")), None).expect("client");
    (ChatSync::new(arena, Arc::clone(store), id.into(), "/w".into(), None, "qwen".into(), notice), said)
}

#[tokio::test]
async fn the_chat_is_made_and_each_message_goes_after_the_last_arena_has() {
    let (server, seen) = arena().await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let id = session(&store.lock().expect("store"));
    let (sync, _) = sync(&server, &store, &id);
    assert_eq!(sync.push().await, Pushed::Done);
    {
        let seen = seen.lock().expect("seen");
        let first = &seen[0];
        assert_eq!(first["parent"], Value::Null);
        assert_eq!(first["branch"], json!(false));
        let ms = first["messages"].as_array().expect("messages");
        assert_eq!(ms.len(), 4);
        assert_eq!(ms[0]["ref"], json!(format!("{id}:0")));
        assert_eq!(ms[1]["toolCalls"][0]["function"]["name"], json!("read_file"));
        assert_eq!(ms[2]["name"], json!("read_file"));
        assert_eq!(ms[2]["toolCallId"], json!("c1"));
        assert_eq!(ms[3]["model"], json!("qwen"));
    }
    store.lock().expect("store").append(&id, &Message::user("and the tests?"), true).expect("u");
    assert_eq!(sync.push().await, Pushed::Done);
    {
        let seen = seen.lock().expect("seen");
        assert_eq!(seen[1]["parent"], json!(format!("a{id}:3")));
        assert_eq!(seen[1]["messages"].as_array().map(Vec::len), Some(1));
    }
    assert_eq!(sync.pending(), 0);
    // Nothing new: nothing sent.
    assert_eq!(sync.push().await, Pushed::Done);
    assert_eq!(seen.lock().expect("seen").len(), 2);
    // Going back to before a turn Arena has: the next goes on a branch from what is left.
    store.lock().expect("store").truncate(&id, 4).expect("rewind");
    store.lock().expect("store").append(&id, &Message::user("another way"), true).expect("u");
    assert_eq!(sync.push().await, Pushed::Done);
    let seen = seen.lock().expect("seen");
    assert_eq!(seen[2]["parent"], json!(format!("a{id}:3")));
    assert_eq!(seen[2]["branch"], json!(true));
    assert!(sync.describe().contains("Arena has every message"), "{}", sync.describe());
}

#[tokio::test]
async fn the_summary_goes_once_arena_has_what_it_covers() {
    let (server, seen) = arena().await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let id = session(&store.lock().expect("store"));
    store.lock().expect("store").compact(&id, 1, "## Goal\nread the parser").expect("compact");
    let (sync, _) = sync(&server, &store, &id);
    assert_eq!(sync.push().await, Pushed::Done);
    let seen = seen.lock().expect("seen");
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1]["messages"], json!([]));
    assert_eq!(seen[1]["summary"], json!({"upTo": format!("a{id}:1"), "text": "## Goal\nread the parser"}));
    assert!(store.lock().expect("store").unsent_summary(&id).expect("summary").is_none());
}

#[tokio::test]
async fn what_the_web_added_stops_sending_until_it_is_taken_in() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "chat-1", "created": false})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/chat-1/messages"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "id": "chat-1", "title": "t", "count": 5, "stamp": "w5", "base": null, "fork": false, "more": false,
            "summary": null, "status": "behind", "ids": ["w0"],
            "messages": [{"role": "user", "content": "from the web", "id": "w4"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let id = session(&store.lock().expect("store"));
    let (sync, _) = sync(&server, &store, &id);
    assert_eq!(sync.push().await, Pushed::Behind);
    // Asked again before the take-in: nothing is sent.
    assert_eq!(sync.push().await, Pushed::Behind);
    // The first was there already (sent before, its answer lost): it is known by its ref.
    assert_eq!(store.lock().expect("store").last_sent(&id).expect("last"), Some((0, "w0".into())));
    let read = sync.take_behind().expect("what the web added");
    assert_eq!(read.messages[0].text(), "from the web");
}

#[tokio::test]
async fn a_chat_deleted_on_the_web_ends_the_sync() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "chat-1", "created": true})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/chat-1/messages"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"status": "gone", "error": "That chat is not there."})),
        )
        .mount(&server)
        .await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let id = session(&store.lock().expect("store"));
    let (sync, said) = sync(&server, &store, &id);
    assert_eq!(sync.push().await, Pushed::Off);
    assert!(store.lock().expect("store").chat_link(&id).expect("link").off);
    assert!(said.lock().expect("said")[0].contains("deleted"));
    assert_eq!(sync.pending(), 0);
}

#[tokio::test]
async fn an_arena_out_of_reach_is_said_once_and_tried_again() {
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let id = session(&store.lock().expect("store"));
    // Nothing listens there, and nothing may: port 1 is the system's (binding it needs root), so no other test's
    // server can take it, as one can take a port just let go.
    let uri = "http://127.0.0.1:1".to_string();
    let said = Arc::new(Mutex::new(Vec::new()));
    let s2 = Arc::clone(&said);
    let notice: Notice = Arc::new(move |_, w| s2.lock().expect("said").push(w));
    let arena = ArenaClient::new(&uri, Some(Secret::new("sk-test")), None).expect("client");
    let sync = ChatSync::new(arena, Arc::clone(&store), id, "/w".into(), None, "qwen".into(), notice);
    assert!(matches!(sync.push().await, Pushed::Retry(_)));
    assert!(matches!(sync.push().await, Pushed::Retry(_)));
    assert_eq!(said.lock().expect("said").len(), 1);
    assert_eq!(sync.pending(), 4);
}

#[test]
fn the_webs_messages_come_back_as_the_model_reads_them() {
    let m = ca_arena::chats::SyncMessage {
        role: "assistant".into(),
        content: json!("checking"),
        tool_calls: Some(
            json!([{"id": "k1", "type": "function", "function": {"name": "web_search", "arguments": "{\"q\":\"x\"}"}}]),
        ),
        ..Default::default()
    };
    let back = from_arena(&m).expect("message");
    assert_eq!(back.tool_calls[0].name, "web_search");
    assert_eq!(back.tool_calls[0].arguments, "{\"q\":\"x\"}");
    assert!(from_arena(&ca_arena::chats::SyncMessage { role: "system".into(), ..Default::default() }).is_none());
}

/// A session Arena has whole: its four messages are `a{id}:0`…`a{id}:3` there.
async fn sent_session(server: &MockServer, store: &Arc<Mutex<Store>>) -> (String, Arc<ChatSync>) {
    let id = session(&store.lock().expect("store"));
    let (sync, _) = sync(server, store, &id);
    assert_eq!(sync.push().await, Pushed::Done);
    (id, sync)
}

fn read_answer(base: Option<String>, fork: bool, messages: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "chat-1", "title": "t", "count": 9, "stamp": "web-1", "base": base, "fork": fork, "more": false,
        "summary": null, "messages": messages
    }))
}

#[tokio::test]
async fn what_the_web_added_comes_in_after_the_last_message_both_have() {
    let (server, _) = arena().await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let (id, sync) = sent_session(&server, &store).await;
    // Something of the session's that Arena does not have yet.
    store.lock().expect("store").append(&id, &Message::user("mine, not sent"), true).expect("u");
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/chat-1"))
        .respond_with(read_answer(
            Some(format!("a{id}:3")),
            false,
            json!([{"role": "user", "content": "asked on the web", "id": "w1"},
                   {"role": "assistant", "content": "answered on the web", "id": "w2", "model": "qwen"}]),
        ))
        .mount(&server)
        .await;
    let taken = sync.take_in().await;
    assert_eq!(taken.added.len(), 2);
    assert!(taken.rebuilt, "the session's own message moved after the web's");
    assert_eq!(
        taken.said,
        vec!["From the web: asked on the web".to_string(), "Answered on the web:\nanswered on the web".to_string()]
    );
    let s = store.lock().expect("store");
    let all: Vec<String> =
        s.page(&id, 100, None).expect("page").messages.into_iter().map(|m| m.message.content).collect();
    assert_eq!(&all[4..], ["asked on the web", "answered on the web", "mine, not sent"]);
    assert_eq!(s.last_sent(&id).expect("last"), Some((5, "w2".into())));
    assert_eq!(s.unsent(&id, 10).expect("unsent").len(), 1);
    assert_eq!(s.chat_link(&id).expect("link").stamp.as_deref(), Some("web-1"));
}

#[tokio::test]
async fn when_the_web_went_another_way_the_session_follows_it() {
    let (server, _) = arena().await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let (id, sync) = sent_session(&server, &store).await;
    // The web wrote the first answer again: the branch on screen parts after the question.
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/chat-1"))
        .respond_with(read_answer(
            Some(format!("a{id}:0")),
            true,
            json!([{"role": "assistant", "content": "another answer", "id": "w9"}]),
        ))
        .mount(&server)
        .await;
    let taken = sync.take_in().await;
    assert!(taken.rebuilt);
    let s = store.lock().expect("store");
    let all: Vec<String> =
        s.page(&id, 100, None).expect("page").messages.into_iter().map(|m| m.message.content).collect();
    assert_eq!(all, ["read the parser", "another answer"]);
    // Following the web is no branch of the session's own.
    assert!(!s.chat_link(&id).expect("link").branch);
    assert_eq!(s.last_sent(&id).expect("last"), Some((1, "w9".into())));
}

#[tokio::test]
async fn nothing_new_on_the_web_changes_nothing() {
    let (server, _) = arena().await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let (_id, sync) = sent_session(&server, &store).await;
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/chat-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"unchanged": true, "stamp": "s"})))
        .mount(&server)
        .await;
    assert!(!sync.take_in().await.any());
}

#[tokio::test]
async fn a_turn_holds_the_chat_takes_the_webs_queue_and_lets_it_go() {
    let (server, _) = arena().await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let (_id, sync) = sent_session(&server, &store).await;
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/chat-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"unchanged": true, "stamp": "s"})))
        .mount(&server)
        .await;
    // The web chat is answering at first: the turn waits for it.
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/chat-1/turn"))
        .respond_with(
            ResponseTemplate::new(409).set_body_json(json!({"status": "answering", "error": "The chat is answering."})),
        )
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/chat-1/turn"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"until": "2026-11-26T10:00:00Z"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/chat-1/queue/take"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"messages": [{"id": "q1", "content": "also the docs"}]})),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/chat-1/queue/take"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"messages": []})))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/code-arena/chats/chat-1/draft"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/code-arena/chats/chat-1/turn"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let inbox = Arc::new(Mutex::new(Vec::new()));
    sync.begin_turn(Some(inbox.clone()), "").await;
    // What the person queued on the web joins the turn.
    let got = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(t) = inbox.lock().expect("inbox").first().cloned() {
                return t;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the queued message came");
    assert_eq!(got, "also the docs");
    // The answer being written reaches the web while the turn holds the chat.
    sync.draft("half an ans", "");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let reqs = server.received_requests().await.unwrap_or_default();
            if reqs.iter().any(|r| r.method.as_str() == "PUT" && r.url.path().ends_with("/draft")) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the draft was sent");
    sync.end_turn().await;
    let reqs = server.received_requests().await.expect("requests");
    assert_eq!(reqs.iter().filter(|r| r.method.as_str() == "DELETE").count(), 1);
}

#[tokio::test]
async fn the_webs_news_is_heard_by_asking_when_there_is_no_stream() {
    let (server, _) = arena().await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let (id, sync) = sent_session(&server, &store).await;
    // No /watch here (404): the chat is asked with its stamp instead.
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/chat-1"))
        .respond_with(read_answer(
            Some(format!("a{id}:3")),
            false,
            json!([{"role": "user", "content": "hi", "id": "w1"}]),
        ))
        .mount(&server)
        .await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    sync.listen(tx);
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("news came").expect("open");
    assert!(!sync.streaming());
}

#[tokio::test]
async fn the_webs_news_comes_from_the_stream_for_this_chat_only() {
    let (server, _) = arena().await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let (_id, sync) = sent_session(&server, &store).await;
    let body = "data: {\"type\":\"watching\"}\n\ndata: {\"type\":\"chat\",\"id\":\"other\",\"what\":\"messages\"}\n\ndata: {\"type\":\"chat\",\"id\":\"chat-1\",\"what\":\"messages\"}\n\n";
    Mock::given(method("GET"))
        .and(path("/api/code-arena/watch"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    sync.listen(tx);
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("news came").expect("open");
    // One change of this chat's, none for the other's.
    assert!(rx.try_recv().is_err());
}

/// Arena's append, answered only when the test says: a send still on its way.
struct Slow(Arc<tokio::sync::Notify>, Arc<Mutex<Vec<Value>>>);

impl Respond for Slow {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&r.body).unwrap_or_default();
        let ids: Vec<String> = body["messages"]
            .as_array()
            .map(|ms| ms.iter().map(|m| format!("a{}", m["ref"].as_str().unwrap_or_default())).collect())
            .unwrap_or_default();
        self.1.lock().expect("seen").push(body);
        self.0.notify_one();
        ResponseTemplate::new(200)
            .set_delay(Duration::from_millis(300))
            .set_body_json(json!({"ids": ids, "count": 0, "stamp": "s", "summarized": false}))
    }
}

#[tokio::test]
async fn the_web_is_taken_in_only_once_a_send_on_its_way_has_landed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "chat-1", "created": true})))
        .mount(&server)
        .await;
    let arrived = Arc::new(tokio::sync::Notify::new());
    let seen = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/chat-1/messages"))
        .respond_with(Slow(arrived.clone(), seen.clone()))
        .mount(&server)
        .await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let id = session(&store.lock().expect("store"));
    // The web's read names the session's last message, which Arena has, though its answer is still on its way.
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/chat-1"))
        .respond_with(read_answer(
            Some(format!("a{id}:3")),
            false,
            json!([{"role": "user", "content": "web", "id": "w1"}]),
        ))
        .mount(&server)
        .await;
    let (sync, _) = sync(&server, &store, &id);
    let pushing = {
        let s = Arc::clone(&sync);
        tokio::spawn(async move { s.push().await })
    };
    arrived.notified().await;
    let taken = sync.take_in().await;
    assert_eq!(pushing.await.expect("pushed"), Pushed::Done);
    assert_eq!(taken.added.len(), 1);
    assert!(!taken.rebuilt);
    let s = store.lock().expect("store");
    let all: Vec<(String, Option<String>)> = s
        .page(&id, 100, None)
        .expect("page")
        .messages
        .iter()
        .map(|m| (m.message.content.clone(), s.arena_id(&id, m.seq).expect("id")))
        .collect();
    assert_eq!(all[0], ("read the parser".to_string(), Some(format!("a{id}:0"))));
    assert_eq!(all[4], ("web".to_string(), Some("w1".to_string())));
    assert_eq!(all.len(), 5);
    assert_eq!(seen.lock().expect("seen").len(), 1, "nothing was sent twice");
}

#[tokio::test]
async fn a_read_whose_base_the_session_does_not_know_changes_nothing() {
    let (server, _) = arena().await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let (id, sync) = sent_session(&server, &store).await;
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/chat-1"))
        .respond_with(read_answer(
            Some("not-here".into()),
            false,
            json!([{"role": "user", "content": "x", "id": "w1"}]),
        ))
        .mount(&server)
        .await;
    assert!(!sync.take_in().await.any());
    assert_eq!(store.lock().expect("store").summary(&id).expect("s").messages, 4);
}

#[tokio::test]
async fn the_plan_goes_whole_to_the_chats_plans_board() {
    let (server, _) = arena().await;
    Mock::given(method("PUT"))
        .and(path("/api/code-arena/chats/chat-1/plan"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"link": "/plans/p1"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/code-arena/chats/chat-1/plan"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let (id, sync) = sent_session(&server, &store).await;
    let plan = json!({"title": "Fix it", "steps": [{"number": 1, "title": "test", "status": "todo"}]}).to_string();
    store.lock().expect("store").set_plan(&id, Some(&plan)).expect("plan");
    assert_eq!(sync.pending(), 1);
    assert_eq!(sync.push().await, Pushed::Done);
    assert_eq!(sync.pending(), 0);
    let reqs = server.received_requests().await.expect("requests");
    let put = reqs.iter().find(|r| r.method.as_str() == "PUT").expect("put");
    let body: Value = serde_json::from_slice(&put.body).expect("json");
    assert_eq!(body["title"], "Fix it");
    assert_eq!(body["place"], "/w");
    assert_eq!(body["steps"][0]["number"], 1);
    // Cleared: taken off the board.
    store.lock().expect("store").set_plan(&id, None).expect("cleared");
    assert_eq!(sync.push().await, Pushed::Done);
    assert_eq!(sync.pending(), 0);
}

/// Arena keeps what is sent, but its answer is lost on the way (502).
struct KeptButLost(Arc<Mutex<Vec<Value>>>);

impl Respond for KeptButLost {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&r.body).unwrap_or_default();
        self.0.lock().expect("kept").extend(body["messages"].as_array().cloned().unwrap_or_default());
        ResponseTemplate::new(502)
    }
}

#[tokio::test]
async fn messages_arena_kept_whose_answer_was_lost_are_known_by_their_ref_not_taken_in_twice() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "chat-1", "created": true})))
        .mount(&server)
        .await;
    let kept = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/chat-1/messages"))
        .respond_with(KeptButLost(kept.clone()))
        .mount(&server)
        .await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let id = session(&store.lock().expect("store"));
    let (sync, _) = sync(&server, &store, &id);
    assert!(matches!(sync.push().await, Pushed::Retry(_)));
    // The web's read gives them back, with Arena's ids and the session's refs, and one of the web's after them.
    let mut given: Vec<Value> = kept
        .lock()
        .expect("kept")
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut m = m.clone();
            m["id"] = json!(format!("k{i}"));
            m
        })
        .collect();
    given.push(json!({"role": "user", "content": "asked on the web", "id": "w9"}));
    Mock::given(method("GET"))
        .and(path("/api/code-arena/chats/chat-1"))
        .respond_with(read_answer(None, false, json!(given)))
        .mount(&server)
        .await;
    let taken = sync.take_in().await;
    assert_eq!(taken.added.len(), 1, "only the web's own: {:?}", taken.added);
    assert!(!taken.said.iter().any(|s| s.contains("read the parser")), "{:?}", taken.said);
    let s = store.lock().expect("store");
    let all: Vec<String> =
        s.page(&id, 100, None).expect("page").messages.into_iter().map(|m| m.message.content).collect();
    assert_eq!(all.len(), 5, "{all:?}");
    assert_eq!(all[4], "asked on the web");
    assert!(s.unsent(&id, 10).expect("unsent").is_empty(), "nothing is sent again");
    assert_eq!(s.arena_id(&id, 0).expect("id").as_deref(), Some("k0"));
}

#[tokio::test]
async fn a_first_turn_makes_the_chat_and_holds_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "chat-1", "created": true})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats/chat-1/turn"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"until": "x"})))
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let id = store.lock().expect("store").create("/w", "qwen", "ask").expect("create");
    let (sync, _) = sync(&server, &store, &id);
    // Nothing kept yet: the question is the turn's own, and the chat is named by it.
    sync.begin_turn(None, "what files are here?").await;
    let reqs = server.received_requests().await.expect("requests");
    let made: Value = serde_json::from_slice(&reqs[0].body).expect("json");
    assert_eq!(made["title"], "what files are here?");
    assert!(reqs.iter().any(|r| r.url.path().ends_with("/turn")), "the first turn holds the chat");
}

#[tokio::test]
async fn a_rating_goes_to_the_answer_arena_has_and_not_from_a_chat_kept_here() {
    let (server, _) = arena().await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let id = session(&store.lock().expect("store"));
    let (sync, _) = sync(&server, &store, &id);
    assert_eq!(sync.push().await, Pushed::Done);
    let down = json!({"up": false, "reason": "wrong"});
    Mock::given(method("PUT"))
        .and(path(format!("/api/code-arena/chats/chat-1/messages/a{id}:3/feedback")))
        .and(wiremock::matchers::body_json(&down))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"up": false})))
        .expect(1)
        .mount(&server)
        .await;
    sync.rate(3, Some(&down)).await.expect("rated");
    sync.set_on(false);
    assert!(sync.rate(3, None).await.expect_err("kept here").contains("kept here only"));
}

#[tokio::test]
async fn where_it_stands_is_said_without_taking_what_waits() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/code-arena/chats"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let store = Arc::new(Mutex::new(Store::in_memory().expect("store")));
    let id = session(&store.lock().expect("store"));
    let (sync, _) = sync(&server, &store, &id);
    assert_eq!((sync.behind(), sync.trouble(), sync.kept_here()), (false, None, false));
    assert!(matches!(sync.push().await, Pushed::Retry(_)));
    assert!(sync.trouble().is_some(), "Arena out of reach is said until a send gets through");
    sync.set_on(false);
    assert!(sync.kept_here());
}
