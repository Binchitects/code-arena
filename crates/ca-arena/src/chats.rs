//! A session kept in step with a chat in Arena (`/api/code-arena/chats`), both ways. The shapes are Arena's
//! (`CodeArenaSync.cs`, the same in 8.0 and 8.1 but for `ms`, which 8.0 ignores). Every message goes with Arena's
//! id for it: Code Arena reads what follows the last message both have, and adds after a message it names.

use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{ArenaClient, ArenaError, refusal};

/// A message as Code Arena and Arena exchange it: the model's own shape, with its model, tokens and time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncMessage {
    pub role: String,
    /// Text, as both ends keep it (a read may give `null`).
    #[serde(default)]
    pub content: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// OpenAI's form: `[{"id", "type": "function", "function": {"name", "arguments"}}]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// A tool result's tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<i64>,
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Code Arena's place for it (`session:seq`), so a message sent twice is known as one.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Arena's id for it (in what Arena gives).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// How an answer ended when not as it should: "stopped", "failed", "declined".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// "note": Code Arena's own word to its model; "meanwhile": the person's, sent while a turn worked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<SyncAttachment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    /// How long it took here (8.1 on; 8.0 ignores it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<i64>,
}

impl SyncMessage {
    /// Its text, whatever form the content came in.
    pub fn text(&self) -> String {
        match &self.content {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            Value::Array(parts) => {
                parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")
            }
            other => other.to_string(),
        }
    }
}

/// A file that went with a question on the web: "image" (fetched with [`ArenaClient::attachment`]), "text", "file",
/// "audio", "video".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncAttachment {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub content_type: String,
}

/// A summary of the branch down to the message `up_to`, read instead of those messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    pub up_to: String,
    #[serde(default)]
    pub text: Option<String>,
}

/// The branch on screen as Code Arena reads it: the messages after `base` (the last message both have), `fork` when
/// what Code Arena last had is not on it any more, its last summary, and whether more follow.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRead {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub stamp: String,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub fork: bool,
    #[serde(default)]
    pub more: bool,
    #[serde(default)]
    pub summary: Option<SyncSummary>,
    #[serde(default)]
    pub messages: Vec<SyncMessage>,
    /// "behind", in a refused append.
    #[serde(default)]
    pub status: Option<String>,
    /// In a refused append: the ids of those it had already (sent before, their answer lost).
    #[serde(default)]
    pub ids: Option<Vec<String>>,
    /// Only "nothing moved since the stamp given".
    #[serde(default)]
    pub unchanged: bool,
}

/// A chat of the person's, as the list gives it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatItem {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub origin_ref: Option<String>,
    #[serde(default)]
    pub origin_place: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub messages: i64,
    /// Code Arena's turn holds it now.
    #[serde(default)]
    pub code_arena: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ChatList {
    #[serde(default)]
    pub chats: Vec<ChatItem>,
    #[serde(default)]
    pub more: bool,
}

/// The chat made (or found again by the session's ref).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Made {
    pub id: String,
    #[serde(default)]
    pub created: bool,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
}

/// What a new chat is made with.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewChat {
    /// The session's id: the same chat is found again by it.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Where it works: the folder, shown on the web.
    pub place: String,
    pub model: Option<String>,
    pub title: Option<String>,
    /// The repository's key (its memories are that project's).
    pub project: Option<String>,
}

/// An append Arena took.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Appended {
    /// Arena's ids for the messages sent, in their order.
    #[serde(default)]
    pub ids: Vec<String>,
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub stamp: String,
    #[serde(default)]
    pub summarized: bool,
}

/// What an append came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Append {
    Done(Appended),
    /// The web added after the parent, or shows another branch: what Code Arena lacks, to take in first.
    Behind(SyncRead),
    /// The web chat is answering: send again when its answer ends.
    Answering,
}

/// A message the person queued on the web while Code Arena's turn worked.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Queued {
    pub id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub quote: Option<String>,
}

/// Whether Arena's answer is its 409 "the web chat is answering".
fn answering(raw: &crate::client::Raw) -> bool {
    raw.status == StatusCode::CONFLICT && raw.problem().is_some_and(|p| p.status == "answering")
}

/// Whether an error is Arena's "that chat is not there" (deleted on the web).
pub fn gone(e: &ArenaError) -> bool {
    matches!(e, ArenaError::Status { status: 404, code: Some(c), .. } if c == "gone")
}

const API: &str = "/api/code-arena";

impl ArenaClient {
    /// The person's chats, newest first.
    pub async fn chats(&self, limit: usize) -> Result<ChatList, ArenaError> {
        self.get(&format!("{API}/chats?limit={limit}")).await
    }

    /// The chat for a session: made the first time, the same one after (found by its ref).
    pub async fn make_chat(&self, new: &NewChat) -> Result<Made, ArenaError> {
        self.call(Method::POST, &format!("{API}/chats"), Some(&serde_json::to_value(new).unwrap_or_default())).await
    }

    /// The branch on screen after `since` (none: from the start); with `stamp`, only `unchanged` when nothing moved.
    /// `Ok(None)` while the web chat is answering.
    pub async fn read_chat(
        &self,
        chat: &str,
        since: Option<&str>,
        stamp: Option<&str>,
        limit: usize,
    ) -> Result<Option<SyncRead>, ArenaError> {
        let mut path = format!("{API}/chats/{chat}?limit={limit}");
        if let Some(s) = since {
            path.push_str(&format!("&since={s}"));
        }
        if let Some(s) = stamp {
            path.push_str(&format!(
                "&stamp={}",
                url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>()
            ));
        }
        let raw = self.raw(Method::GET, &path, None).await?;
        if answering(&raw) {
            return Ok(None);
        }
        if !raw.status.is_success() {
            return Err(refusal(&raw));
        }
        serde_json::from_str(&raw.body).map(Some).map_err(|e| ArenaError::Parse { path, message: e.to_string() })
    }

    /// Adds messages after `parent` (the last message Code Arena has from Arena; none: from the start). `branch`: they go
    /// on a branch of their own from `parent` (Code Arena went back to before a turn).
    pub async fn append(
        &self,
        chat: &str,
        parent: Option<&str>,
        branch: bool,
        messages: &[SyncMessage],
        summary: Option<&SyncSummary>,
    ) -> Result<Append, ArenaError> {
        let path = format!("{API}/chats/{chat}/messages");
        let body = json!({"parent": parent, "branch": branch, "messages": messages, "summary": summary});
        let raw = self.raw(Method::POST, &path, Some(&body)).await?;
        if answering(&raw) {
            return Ok(Append::Answering);
        }
        let parse = |text: &str| ArenaError::Parse { path: path.clone(), message: text.to_string() };
        match raw.status {
            s if s.is_success() => serde_json::from_str(&raw.body).map(Append::Done).map_err(|e| parse(&e.to_string())),
            StatusCode::CONFLICT => {
                serde_json::from_str(&raw.body).map(Append::Behind).map_err(|e| parse(&e.to_string()))
            }
            _ => Err(refusal(&raw)),
        }
    }

    /// Code Arena's turn starts: the chat is its own until it ends the turn (or stops renewing it for 90 s). `false`
    /// while the web chat is answering.
    pub async fn hold_turn(&self, chat: &str) -> Result<bool, ArenaError> {
        let raw = self.raw(Method::POST, &format!("{API}/chats/{chat}/turn"), Some(&json!({}))).await?;
        if answering(&raw) {
            return Ok(false);
        }
        if raw.status.is_success() { Ok(true) } else { Err(refusal(&raw)) }
    }

    /// The turn ended: the chat is free, and what the person queued on the web goes now.
    pub async fn end_turn(&self, chat: &str) -> Result<(), ArenaError> {
        let raw = self.raw(Method::DELETE, &format!("{API}/chats/{chat}/turn"), None).await?;
        if raw.status.is_success() { Ok(()) } else { Err(refusal(&raw)) }
    }

    /// The answer being written, as it stands: the web shows it as it comes. `false` when no turn holds the chat.
    pub async fn draft(&self, chat: &str, text: &str, reasoning: &str) -> Result<bool, ArenaError> {
        let body = json!({"text": text, "reasoning": reasoning});
        let raw = self.raw(Method::PUT, &format!("{API}/chats/{chat}/draft"), Some(&body)).await?;
        match raw.status {
            s if s.is_success() => Ok(true),
            StatusCode::CONFLICT => Ok(false),
            _ => Err(refusal(&raw)),
        }
    }

    /// What the person queued on the web while this turn works, taken out of the line, in order.
    pub async fn take_queued(&self, chat: &str) -> Result<Vec<Queued>, ArenaError> {
        #[derive(Deserialize)]
        struct Taken {
            #[serde(default)]
            messages: Vec<Queued>,
        }
        let t: Taken = self.call(Method::POST, &format!("{API}/chats/{chat}/queue/take"), Some(&json!({}))).await?;
        Ok(t.messages)
    }

    /// The session's working plan, whole, on the chat's Plans board (`None`: cleared, taken off). `Ok(false)` when this
    /// Arena keeps no plans for Code Arena's chats.
    pub async fn put_plan(&self, chat: &str, plan: Option<&Value>) -> Result<bool, ArenaError> {
        let path = format!("{API}/chats/{chat}/plan");
        let raw = match plan {
            Some(p) => self.raw(Method::PUT, &path, Some(p)).await?,
            None => self.raw(Method::DELETE, &path, None).await?,
        };
        match raw.status {
            s if s.is_success() => Ok(true),
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED if raw.problem().is_none() => Ok(false),
            _ => Err(refusal(&raw)),
        }
    }

    /// The session's goal, whole, shown above the web chat (`None`: none, taken off). `Ok(false)` when this Arena keeps
    /// no goals for Code Arena's chats.
    pub async fn put_goal(&self, chat: &str, goal: Option<&Value>) -> Result<bool, ArenaError> {
        let path = format!("{API}/chats/{chat}/goal");
        let raw = match goal {
            Some(g) => self.raw(Method::PUT, &path, Some(g)).await?,
            None => self.raw(Method::DELETE, &path, None).await?,
        };
        match raw.status {
            s if s.is_success() => Ok(true),
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED if raw.problem().is_none() => Ok(false),
            _ => Err(refusal(&raw)),
        }
    }

    /// The person's rating of an answer of the chat, by its id in Arena, as the web's thumbs keep it: `{up, reason?,
    /// comment?, share?}` (`None`: taken back). Arena keeps a reason with a down vote only, and at most 1,000
    /// characters of comment.
    pub async fn rate(&self, chat: &str, message: &str, rating: Option<&Value>) -> Result<(), ArenaError> {
        let path = format!("{API}/chats/{chat}/messages/{message}/feedback");
        let raw = match rating {
            Some(r) => self.raw(Method::PUT, &path, Some(r)).await?,
            None => self.raw(Method::DELETE, &path, None).await?,
        };
        if raw.status.is_success() { Ok(()) } else { Err(refusal(&raw)) }
    }

    /// The person continues the chat here: archived on the web, it comes back to the list.
    pub async fn continued(&self, chat: &str) -> Result<(), ArenaError> {
        let raw = self.raw(Method::POST, &format!("{API}/chats/{chat}/continued"), Some(&json!({}))).await?;
        if raw.status.is_success() { Ok(()) } else { Err(refusal(&raw)) }
    }
}

/// A change in the person's chats, as the watch says it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WatchEvent {
    #[serde(rename = "type")]
    pub kind: String,
    /// The chat that changed.
    #[serde(default)]
    pub id: Option<String>,
    /// "messages", "title", "project", "deleted".
    #[serde(default)]
    pub what: Option<String>,
}

/// The stream of the person's chats' changes (`GET /api/code-arena/watch`), open for as long as it is read.
pub struct Watch {
    res: reqwest::Response,
    buf: String,
}

impl Watch {
    /// The next change; `Some(None)` for a heartbeat (Arena says something every 15 s); `None` when the stream ended.
    pub async fn next(&mut self) -> Option<Option<WatchEvent>> {
        loop {
            if let Some(end) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..end + 2).collect();
                let data: String = block
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect::<Vec<_>>()
                    .join("\n");
                if data.is_empty() {
                    return Some(None);
                }
                if let Ok(e) = serde_json::from_str::<WatchEvent>(&data) {
                    return Some(Some(e));
                }
                continue;
            }
            match self.res.chunk().await {
                Ok(Some(bytes)) => self.buf.push_str(&String::from_utf8_lossy(&bytes).replace("\r\n", "\n")),
                _ => return None,
            }
        }
    }
}

impl ArenaClient {
    /// Opens the stream of the person's chats' changes. An Arena without it answers 404, a proxy that does not pass
    /// streams answers something else: the caller asks every few seconds instead.
    pub async fn watch(&self) -> Result<Watch, ArenaError> {
        let res = self
            .untimed(Method::GET, &format!("{API}/watch"))
            .header("Accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| ArenaError::Net(ca_net::explain(&e)))?;
        let ct = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        if !res.status().is_success() || !ct.contains("event-stream") {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(refusal(&crate::client::Raw { status, content_type: ct, body }));
        }
        Ok(Watch { res, buf: String::new() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_parses_as_arena_writes_it() {
        let body = r#"{"id":"c1","title":"Fix the parser","model":"qwen","origin":"code-arena","originPlace":"/r","count":3,
            "stamp":"m3:","base":"m1","fork":false,"more":false,"summary":null,
            "messages":[{"role":"user","content":"and the tests?","reasoning":null,"toolCalls":null,"toolCallId":null,
              "name":null,"model":null,"promptTokens":null,"cachedTokens":null,"completionTokens":null,
              "at":"2026-11-26T10:00:00+00:00","ref":null,"id":"m2","status":null,"kind":null,"attachments":null,"quote":null}],
            "status":null,"ids":null}"#;
        let r: SyncRead = serde_json::from_str(body).expect("read");
        assert_eq!(r.base.as_deref(), Some("m1"));
        assert_eq!(r.messages[0].text(), "and the tests?");
        assert_eq!(r.messages[0].id.as_deref(), Some("m2"));
    }

    #[tokio::test]
    async fn the_watch_says_each_change_and_its_heartbeats() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let body = "data: {\"type\":\"watching\"}\n\n: still here\n\ndata: {\"type\":\"chat\",\"id\":\"c1\",\"what\":\"messages\"}\n\n";
        Mock::given(method("GET"))
            .and(path("/api/code-arena/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .mount(&server)
            .await;
        let c = ArenaClient::new(&server.uri(), Some(ca_core::Secret::new("sk-test")), None).expect("client");
        let mut w = c.watch().await.expect("open");
        assert_eq!(w.next().await.and_then(|e| e).map(|e| e.kind), Some("watching".into()));
        assert_eq!(w.next().await, Some(None));
        let e = w.next().await.and_then(|e| e).expect("a change");
        assert_eq!((e.id.as_deref(), e.what.as_deref()), (Some("c1"), Some("messages")));
        assert_eq!(w.next().await, None);
    }

    #[tokio::test]
    async fn a_rating_is_put_on_the_answer_and_taken_back() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let at = "/api/code-arena/chats/c1/messages/m2/feedback";
        let down = json!({"up": false, "reason": "wrong", "comment": "it was not", "share": true});
        Mock::given(method("PUT"))
            .and(path(at))
            .and(body_json(&down))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"up": false})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE")).and(path(at)).respond_with(ResponseTemplate::new(204)).expect(1).mount(&server).await;
        let c = ArenaClient::new(&server.uri(), Some(ca_core::Secret::new("sk-test")), None).expect("client");
        c.rate("c1", "m2", Some(&down)).await.expect("rated");
        c.rate("c1", "m2", None).await.expect("taken back");
        assert!(c.rate("c1", "m9", Some(&json!({"up": true}))).await.is_err());
    }

    #[tokio::test]
    async fn an_arena_without_the_watch_is_told() {
        let server = wiremock::MockServer::start().await;
        let c = ArenaClient::new(&server.uri(), Some(ca_core::Secret::new("sk-test")), None).expect("client");
        assert!(c.watch().await.is_err());
    }

    #[test]
    fn a_message_goes_with_only_what_it_has() {
        let m = SyncMessage {
            role: "user".into(),
            content: json!("hi"),
            reference: Some("s:0".into()),
            ..Default::default()
        };
        assert_eq!(serde_json::to_value(&m).expect("json"), json!({"role": "user", "content": "hi", "ref": "s:0"}));
    }
}
