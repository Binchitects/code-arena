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
    // Nothing listens there.
    let server = MockServer::start().await;
    let uri = server.uri();
    drop(server);
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
