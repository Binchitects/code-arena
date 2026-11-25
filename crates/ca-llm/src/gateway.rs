//! Arena's gateway: its models, and answers streamed with their tool calls.

use std::path::Path;
use std::time::Duration;

use ca_core::Secret;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::Value;

use crate::sse::SseReader;
use crate::types::{Completion, Finish, Message, Request, Role, Sink, ToolCall, Usage};

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error(transparent)]
    Net(#[from] ca_net::NetError),
    #[error(
        "the gateway refused your API key (401): make a new one under Your account → API key and run code-arena login"
    )]
    Key,
    /// A refusal to say as it is: Arena's safeguards, a key's limit, an admin's stop.
    #[error("{0}")]
    Refused(String),
    #[error("{url} has no OpenAI API (404): give the gateway with code-arena login --gateway URL")]
    NoApi { url: String },
    #[error("the gateway answered {status}: {message}")]
    Status { status: u16, message: String },
    #[error("the window refused the request: {0}")]
    ContextOverflow(String),
    #[error("the answer stalled: nothing came for {0} s")]
    Stalled(u64),
    #[error("{0}")]
    Parse(String),
}

/// A model as the gateway describes it (`/v1/model/info`, when it shares it).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelInfo {
    pub id: String,
    /// Its window, in tokens.
    pub context: Option<u64>,
    /// Its longest answer, in tokens.
    pub max_output: Option<u64>,
    pub tools: bool,
    pub thinking: bool,
    pub vision: bool,
}

#[derive(Clone)]
pub struct Gateway {
    base: String,
    key: Secret,
    http: reqwest::Client,
    /// Tries for a gateway that is briefly away, before anything was shown.
    pub retries: u32,
    pub retry_delay: Duration,
    /// How long an answer may stay quiet (no first byte, or none between bytes) before it counts as stalled.
    pub stall: Duration,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gateway").field("base", &self.base).finish_non_exhaustive()
    }
}

impl Gateway {
    /// `base` without `/v1`.
    pub fn new(base: &str, key: Secret, ca: Option<&Path>) -> Result<Self, LlmError> {
        Ok(Self {
            base: base.trim_end_matches('/').trim_end_matches("/v1").to_string(),
            key,
            http: ca_net::client(ca)?,
            retries: 5,
            retry_delay: Duration::from_secs(3),
            stall: Duration::from_secs(15 * 60),
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    async fn get(&self, path: &str, timeout: Duration) -> Result<Value, LlmError> {
        let res = self
            .http
            .get(format!("{}{path}", self.base))
            .bearer_auth(self.key.expose())
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| LlmError::Net(ca_net::explain(&e)))?;
        let status = res.status();
        let text = res.text().await.map_err(|e| LlmError::Net(ca_net::explain(&e)))?;
        if !status.is_success() {
            return Err(failure(status, &text, &format!("{}{path}", self.base)));
        }
        serde_json::from_str(&text).map_err(|e| LlmError::Parse(format!("{}{path}: {e}", self.base)))
    }

    /// The models the key may use (`/v1/models`), with their windows where `/v1/model/info` says them. Models that
    /// are not for chat (embeddings, speech) are left out.
    pub async fn models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let list = self.get("/v1/models", Duration::from_secs(30)).await?;
        let Some(data) = list.get("data").and_then(Value::as_array) else {
            return Err(LlmError::Parse(format!(
                "{}/v1/models did not answer with a model list: is this the gateway?",
                self.base
            )));
        };
        let mut ids: Vec<String> = data.iter().filter_map(|m| m.get("id")?.as_str().map(str::to_string)).collect();
        ids.dedup();
        // Not every gateway shares it: the window then comes from the settings, or a default.
        let info = self.get("/v1/model/info", Duration::from_secs(15)).await.ok();
        let rows: Vec<&Value> =
            info.as_ref().and_then(|i| i.get("data")?.as_array()).map(|a| a.iter().collect()).unwrap_or_default();
        Ok(ids
            .into_iter()
            .filter_map(|id| {
                let mi = rows
                    .iter()
                    .find(|r| r.get("model_name").and_then(Value::as_str) == Some(id.as_str()))
                    .and_then(|r| r.get("model_info"));
                let Some(mi) = mi else {
                    return Some(ModelInfo { id, tools: true, thinking: true, ..Default::default() });
                };
                if mi.get("mode").and_then(Value::as_str).is_some_and(|m| m != "chat") {
                    return None;
                }
                let flag = |k: &str, d: bool| mi.get(k).and_then(Value::as_bool).unwrap_or(d);
                Some(ModelInfo {
                    context: mi.get("max_input_tokens").and_then(Value::as_u64),
                    max_output: mi.get("max_output_tokens").and_then(Value::as_u64),
                    tools: flag("supports_function_calling", true),
                    thinking: flag("supports_reasoning", true),
                    vision: flag("supports_vision", false),
                    id,
                })
            })
            .collect())
    }

    /// One answer, streamed into `sink`; tool calls put together from their pieces. A gateway briefly away (502,
    /// 503, 504, a refused connection) is tried again while nothing has been shown; a key past its limit is not
    /// (the minute is not over in a few seconds, and each try counts).
    pub async fn complete(&self, request: &Request, sink: &mut dyn Sink) -> Result<Completion, LlmError> {
        let body = request.to_wire();
        let mut tries = 0;
        loop {
            match self.stream_once(&body, sink).await {
                Err(Attempt::Retry(why)) if tries < self.retries => {
                    tries += 1;
                    sink.notice(&format!("{why}: trying again ({tries} of {})", self.retries));
                    tokio::time::sleep(self.retry_delay * tries).await;
                }
                Err(Attempt::Retry(why)) => {
                    return Err(LlmError::Refused(format!("{why}, and still after {} tries", self.retries)));
                }
                Err(Attempt::Fail(e)) => return Err(e),
                Ok(c) => return Ok(c),
            }
        }
    }

    async fn stream_once(&self, body: &Value, sink: &mut dyn Sink) -> Result<Completion, Attempt> {
        let url = format!("{}/v1/chat/completions", self.base);
        let send = self.http.post(&url).bearer_auth(self.key.expose()).json(body).send();
        let res = match tokio::time::timeout(self.stall, send).await {
            Err(_) => return Err(Attempt::Fail(LlmError::Stalled(self.stall.as_secs()))),
            Ok(Err(e)) if e.is_connect() => {
                return Err(Attempt::Retry(format!("the gateway could not be reached ({})", ca_net::explain(&e))));
            }
            Ok(Err(e)) => return Err(Attempt::Fail(LlmError::Net(ca_net::explain(&e)))),
            Ok(Ok(r)) => r,
        };
        let status = res.status();
        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            if matches!(status, StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT)
            {
                return Err(Attempt::Retry(format!("the gateway answered {}", status.as_u16())));
            }
            return Err(Attempt::Fail(failure(status, &text, &url)));
        }
        let mut answer = Assembly::default();
        let mut reader = SseReader::default();
        let mut stream = res.bytes_stream();
        let mut shown = false;
        loop {
            let next = match tokio::time::timeout(self.stall, stream.next()).await {
                Err(_) => return Err(Attempt::Fail(LlmError::Stalled(self.stall.as_secs()))),
                Ok(n) => n,
            };
            let chunk = match next {
                None => break,
                Some(Ok(b)) => b,
                // Dropped mid-answer: what came is kept, and the agent asks the model to carry on.
                Some(Err(_)) if shown => {
                    answer.finish.get_or_insert(Finish::Dropped);
                    break;
                }
                Some(Err(e)) => {
                    return Err(Attempt::Retry(format!("the stream broke before it began ({})", ca_net::explain(&e))));
                }
            };
            for data in reader.feed(&String::from_utf8_lossy(&chunk)) {
                if answer.take(&data, sink).map_err(Attempt::Fail)? {
                    shown = true;
                }
            }
        }
        if let Some(data) = reader.finish() {
            answer.take(&data, sink).map_err(Attempt::Fail)?;
        }
        Ok(answer.done())
    }
}

enum Attempt {
    Retry(String),
    Fail(LlmError),
}

/// The pieces of one streamed answer.
#[derive(Default)]
struct Assembly {
    text: String,
    reasoning: String,
    calls: Vec<ToolCall>,
    finish: Option<Finish>,
    usage: Usage,
    done: bool,
}

impl Assembly {
    /// Takes one event's data; true when something was shown.
    fn take(&mut self, data: &str, sink: &mut dyn Sink) -> Result<bool, LlmError> {
        if data.trim() == "[DONE]" {
            self.done = true;
            return Ok(false);
        }
        let v: Value = serde_json::from_str(data).map_err(|e| LlmError::Parse(format!("a streamed event: {e}")))?;
        if let Some(err) = v.get("error") {
            let message =
                err.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| err.to_string());
            return Err(LlmError::Refused(message));
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            self.usage = read_usage(u);
        }
        let mut shown = false;
        for choice in v.get("choices").and_then(Value::as_array).into_iter().flatten() {
            if let Some(d) = choice.get("delta") {
                if let Some(t) = d.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                    self.text.push_str(t);
                    sink.text(t);
                    shown = true;
                }
                let r = d.get("reasoning_content").or_else(|| d.get("reasoning")).and_then(Value::as_str);
                if let Some(r) = r.filter(|r| !r.is_empty()) {
                    self.reasoning.push_str(r);
                    sink.reasoning(r);
                    shown = true;
                }
                for tc in d.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                    self.call_piece(tc);
                    shown = true;
                }
            }
            if let Some(f) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish = Some(match f {
                    "stop" => Finish::Stop,
                    "tool_calls" | "function_call" => Finish::ToolCalls,
                    "length" => Finish::Length,
                    _ => Finish::Other,
                });
            }
        }
        Ok(shown)
    }

    fn call_piece(&mut self, tc: &Value) {
        let index =
            tc.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(self.calls.len().saturating_sub(1));
        while self.calls.len() <= index {
            self.calls.push(ToolCall { id: String::new(), name: String::new(), arguments: String::new() });
        }
        let call = &mut self.calls[index];
        if let Some(id) = tc.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            call.id = id.to_string();
        }
        if let Some(f) = tc.get("function") {
            if let Some(n) = f.get("name").and_then(Value::as_str) {
                call.name.push_str(n);
            }
            if let Some(a) = f.get("arguments").and_then(Value::as_str) {
                call.arguments.push_str(a);
            }
        }
    }

    fn done(mut self) -> Completion {
        for (i, c) in self.calls.iter_mut().enumerate() {
            if c.id.is_empty() {
                c.id = format!("call_{i}");
            }
        }
        self.calls.retain(|c| !c.name.is_empty());
        let finish = match self.finish {
            Some(f) => f,
            None if self.done => Finish::Stop,
            None => Finish::Dropped,
        };
        let finish = if finish == Finish::Stop && !self.calls.is_empty() { Finish::ToolCalls } else { finish };
        Completion {
            message: Message {
                role: Role::Assistant,
                content: self.text,
                tool_calls: self.calls,
                tool_call_id: None,
                reasoning: (!self.reasoning.is_empty()).then_some(self.reasoning),
            },
            finish,
            usage: self.usage,
        }
    }
}

fn read_usage(u: &Value) -> Usage {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        prompt: n("prompt_tokens"),
        completion: n("completion_tokens"),
        cached: u.pointer("/prompt_tokens_details/cached_tokens").and_then(Value::as_u64).unwrap_or(0),
    }
}

/// A refusal in the words that help: Arena's safeguards and limits as Arena says them, the window's own refusal
/// as an overflow the agent compacts for.
fn failure(status: StatusCode, body: &str, url: &str) -> LlmError {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let message = parsed
        .pointer("/error/message")
        .or_else(|| parsed.get("error").filter(|e| e.is_string()))
        .or_else(|| parsed.get("detail"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| body.chars().take(400).collect());
    let lower = message.to_ascii_lowercase();
    if lower.contains("context size")
        || lower.contains("maximum context length")
        || lower.contains("contextwindowexceeded")
        || lower.contains("context length")
    {
        return LlmError::ContextOverflow(message);
    }
    match status {
        StatusCode::UNAUTHORIZED => LlmError::Key,
        StatusCode::NOT_FOUND => LlmError::NoApi { url: url.to_string() },
        s if s == StatusCode::TOO_MANY_REQUESTS || s == StatusCode::BAD_REQUEST || s == StatusCode::FORBIDDEN => {
            LlmError::Refused(message)
        }
        s => LlmError::Status { status: s.as_u16(), message },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Quiet;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sse(events: &[&str]) -> String {
        events.iter().map(|e| format!("data: {e}\n\n")).collect()
    }

    fn gateway(server: &MockServer) -> Gateway {
        let mut g = Gateway::new(&server.uri(), Secret::new("sk-test"), None).expect("gateway");
        g.retry_delay = Duration::from_millis(1);
        g
    }

    #[derive(Default)]
    struct Collect {
        text: String,
        reasoning: String,
        notices: Vec<String>,
    }
    impl Sink for Collect {
        fn text(&mut self, d: &str) {
            self.text.push_str(d);
        }
        fn reasoning(&mut self, d: &str) {
            self.reasoning.push_str(d);
        }
        fn notice(&mut self, t: &str) {
            self.notices.push(t.to_string());
        }
    }

    #[tokio::test]
    async fn models_with_their_windows() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data": [{"id": "qwen"}, {"id": "embed"}, {"id": "bare"}]})),
            )
            .mount(&server)
            .await;
        Mock::given(path("/v1/model/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": [
                {"model_name": "qwen", "model_info": {"max_input_tokens": 131072, "max_output_tokens": 16384, "supports_vision": true}},
                {"model_name": "embed", "model_info": {"mode": "embedding"}}
            ]})))
            .mount(&server)
            .await;
        let models = gateway(&server).models().await.expect("models");
        assert_eq!(models.len(), 2);
        assert_eq!(
            models[0],
            ModelInfo {
                id: "qwen".into(),
                context: Some(131072),
                max_output: Some(16384),
                tools: true,
                thinking: true,
                vision: true
            }
        );
        assert_eq!(models[1].id, "bare");
    }

    #[tokio::test]
    async fn streams_text_thinking_and_tool_calls() {
        let server = MockServer::start().await;
        let body = sse(&[
            r#"{"choices":[{"delta":{"reasoning_content":"Look "}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_content":"first."}}]}"#,
            r#"{"choices":[{"delta":{"content":"Reading."}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_","arguments":"{\"pa"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"file","arguments":"th\":\"a.rs\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_tokens_details":{"cached_tokens":80}}}"#,
            "[DONE]",
        ]);
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer sk-test"))
            .and(body_partial_json(serde_json::json!({"stream": true, "stream_options": {"include_usage": true}})))
            .respond_with(
                ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(body),
            )
            .mount(&server)
            .await;
        let mut sink = Collect::default();
        let req = Request { model: "qwen".into(), messages: vec![Message::user("hi")], ..Default::default() };
        let c = gateway(&server).complete(&req, &mut sink).await.expect("completes");
        assert_eq!(sink.text, "Reading.");
        assert_eq!(sink.reasoning, "Look first.");
        assert_eq!(c.finish, Finish::ToolCalls);
        assert_eq!(
            c.message.tool_calls,
            vec![ToolCall { id: "c1".into(), name: "read_file".into(), arguments: r#"{"path":"a.rs"}"#.into() }]
        );
        assert_eq!(c.usage, Usage { prompt: 100, cached: 80, completion: 20 });
    }

    #[tokio::test]
    async fn a_gateway_briefly_away_is_tried_again() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ])))
            .mount(&server)
            .await;
        let mut sink = Collect::default();
        let req = Request { model: "m".into(), messages: vec![Message::user("hi")], ..Default::default() };
        let c = gateway(&server).complete(&req, &mut sink).await.expect("completes");
        assert_eq!(c.message.content, "ok");
        assert_eq!(sink.notices.len(), 2);
    }

    #[tokio::test]
    async fn refusals_are_said_as_arena_words_them() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({"error": {"message": "Arena's safeguards refused the request: it holds a password."}})))
            .mount(&server)
            .await;
        let req = Request { model: "m".into(), messages: vec![Message::user("hi")], ..Default::default() };
        let err = gateway(&server).complete(&req, &mut Quiet).await.expect_err("refused");
        assert_eq!(err.to_string(), "Arena's safeguards refused the request: it holds a password.");
    }

    #[test]
    fn the_windows_refusal_is_an_overflow() {
        let e = failure(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"This model's maximum context length is 32768 tokens"}}"#,
            "u",
        );
        assert!(matches!(e, LlmError::ContextOverflow(_)));
        assert!(matches!(failure(StatusCode::UNAUTHORIZED, "", "u"), LlmError::Key));
    }

    #[tokio::test]
    async fn a_stream_with_no_end_is_dropped_not_lost() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse(&[r#"{"choices":[{"delta":{"content":"half an ans"}}]}"#])),
            )
            .mount(&server)
            .await;
        let req = Request { model: "m".into(), messages: vec![Message::user("hi")], ..Default::default() };
        let c = gateway(&server).complete(&req, &mut Quiet).await.expect("completes");
        assert_eq!(c.finish, Finish::Dropped);
        assert_eq!(c.message.content, "half an ans");
    }
}
