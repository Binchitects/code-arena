//! Arena's gateway: its models, and answers streamed with their tool calls.

use std::path::Path;
use std::time::Duration;

use ca_core::Secret;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::Value;

use crate::providers::{self, AnthropicStream, Kind, Provider};
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
    #[error("the window refused the request: {message}")]
    ContextOverflow { message: String, overflow: crate::overflow::Overflow },
    #[error("the answer stalled: nothing came for {0} s")]
    Stalled(u64),
    #[error("{0}")]
    Parse(String),
    /// A key past its limit (429), and how long to wait before asking again.
    #[error("{message}")]
    TooMany { message: String, again: Duration },
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
    /// Arena's gateway is there (none in the standalone edition: only providers).
    primary: bool,
    /// Providers beside it, each with its key: their models are `name:model`.
    providers: Vec<(Provider, Secret)>,
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
            primary: true,
            providers: vec![],
            http: ca_net::client(ca)?,
            retries: 5,
            retry_delay: Duration::from_secs(3),
            stall: Duration::from_secs(15 * 60),
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// The standalone edition's: no Arena, only the person's providers.
    pub fn standalone(providers: Vec<(Provider, Secret)>, ca: Option<&Path>) -> Result<Self, LlmError> {
        let mut g = Self::new("", Secret::default(), ca)?;
        g.primary = false;
        g.providers = providers;
        Ok(g)
    }

    /// Providers beside Arena's gateway (none: Arena's admins turned them off, or there are none).
    pub fn set_providers(&mut self, providers: Vec<(Provider, Secret)>) {
        self.providers = providers;
    }

    pub fn providers(&self) -> impl Iterator<Item = &Provider> {
        self.providers.iter().map(|(p, _)| p)
    }

    /// Whether Arena's gateway is there.
    pub fn has_arena(&self) -> bool {
        self.primary
    }

    /// The provider a model named `name:model` is asked of, and its own name for the model.
    fn route<'a>(&'a self, model: &'a str) -> Option<(&'a Provider, &'a Secret, &'a str)> {
        let (name, rest) = model.split_once(':')?;
        self.providers.iter().find(|(p, _)| p.name == name).map(|(p, k)| (p, k, rest))
    }

    /// A provider's models, as its API lists them (or as its settings name them).
    async fn provider_models(&self, p: &Provider, key: &Secret) -> Result<Vec<ModelInfo>, LlmError> {
        let ids: Vec<String> = if !p.models.is_empty() {
            p.models.clone()
        } else {
            let mut r = self.http.get(format!("{}/v1/models", p.url)).timeout(Duration::from_secs(20));
            r = match p.kind {
                Kind::Anthropic => r.header("x-api-key", key.expose()).header("anthropic-version", ANTHROPIC_VERSION),
                Kind::OpenAi => r.bearer_auth(key.expose()),
            };
            let res = r.send().await.map_err(|e| LlmError::Net(ca_net::explain(&e)))?;
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(provider_failure(p, failure(status, &text, &format!("{}/v1/models", p.url))));
            }
            let v: Value = serde_json::from_str(&text).map_err(|e| LlmError::Parse(format!("{}: {e}", p.name)))?;
            v["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(str::to_string)).collect()
        };
        Ok(ids
            .into_iter()
            .map(|id| ModelInfo {
                id: format!("{}:{id}", p.name),
                context: p.context,
                tools: true,
                thinking: true,
                ..Default::default()
            })
            .collect())
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
        let mut theirs = Vec::new();
        let mut first_err = None;
        for (p, k) in &self.providers {
            match self.provider_models(p, k).await {
                Ok(m) => theirs.extend(m),
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        if !self.primary {
            return match (theirs.is_empty(), first_err) {
                (true, Some(e)) => Err(e),
                _ => Ok(theirs),
            };
        }
        let mut mine = self.arena_models().await?;
        mine.extend(theirs);
        Ok(mine)
    }

    async fn arena_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
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
        let (url, auth, body) = match self.route(&request.model) {
            Some((p, key, model)) => match p.kind {
                Kind::OpenAi => (
                    format!("{}/v1/chat/completions", p.url),
                    Auth::Bearer(key.clone()),
                    providers::openai_body(request.to_wire(), model, &request.thinking, p),
                ),
                Kind::Anthropic => (
                    format!("{}/v1/messages", p.url),
                    Auth::Anthropic(key.clone()),
                    providers::anthropic_body(&request.to_wire(), model, &request.thinking),
                ),
            },
            None if !self.primary => {
                let names: Vec<String> = self.providers.iter().map(|(p, _)| format!("{}:MODEL", p.name)).collect();
                return Err(LlmError::Refused(format!(
                    "no provider has the model {}: a model is named after its provider ({})",
                    request.model,
                    if names.is_empty() {
                        "none is set: code-arena login --provider NAME".into()
                    } else {
                        names.join(", ")
                    }
                )));
            }
            None => (format!("{}/v1/chat/completions", self.base), Auth::Bearer(self.key.clone()), request.to_wire()),
        };
        let provider = self.route(&request.model).map(|(p, _, _)| p);
        let mut tries = 0;
        let mut waiting_since: Option<tokio::time::Instant> = None;
        let mut last_said: Option<tokio::time::Instant> = None;
        loop {
            match self.stream_once(&url, &auth, &body, sink).await {
                Err(Attempt::Busy { message, again }) => {
                    let since = *waiting_since.get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() >= LINE_WAIT {
                        return Err(LlmError::Refused(message));
                    }
                    if last_said.is_none_or(|t| t.elapsed() >= Duration::from_secs(120)) {
                        sink.notice(&format!(
                            "{message} Waiting in its line: asking again every {} s (Ctrl+C or Stop ends the wait).",
                            again.as_secs()
                        ));
                        last_said = Some(tokio::time::Instant::now());
                    }
                    tokio::time::sleep(again).await;
                }
                Err(Attempt::Retry(why)) if tries < self.retries => {
                    tries += 1;
                    sink.notice(&format!("{why}: trying again ({tries} of {})", self.retries));
                    tokio::time::sleep(self.retry_delay * tries).await;
                }
                Err(Attempt::Retry(why)) => {
                    return Err(LlmError::Refused(format!("{why}, and still after {} tries", self.retries)));
                }
                Err(Attempt::Fail(e)) => {
                    return Err(match provider {
                        Some(p) => provider_failure(p, e),
                        None => e,
                    });
                }
                Ok(c) => return Ok(c),
            }
        }
    }

    /// The editor's code completion: the text between `prefix` and `suffix` of a file, from the model's own
    /// fill-in-the-middle tokens (`/v1/completions`, not streamed), tidied. Asked once: no tries again, no waiting
    /// in a line, at most `FILL_LIMIT`; a key past its limit (429) says how long to wait.
    pub async fn fill(
        &self,
        model: &str,
        path: &str,
        prefix: &str,
        suffix: &str,
        tags: &[String],
    ) -> Result<(String, Usage), LlmError> {
        let (url, key, name) = match self.route(model) {
            Some((p, _, _)) if p.kind == Kind::Anthropic => {
                return Err(LlmError::Refused(format!("{} has no code completion (/v1/completions)", p.name)));
            }
            Some((p, key, name)) => (format!("{}/v1/completions", p.url), key.clone(), name),
            None if !self.primary => return Err(LlmError::Refused(format!("no provider has the model {model}"))),
            None => (format!("{}/v1/completions", self.base), self.key.clone(), model),
        };
        let mut body = crate::fim::body(name, path, prefix, suffix);
        if !tags.is_empty() && self.route(model).is_none() {
            body["metadata"] = serde_json::json!({"tags": tags});
        }
        let res = self
            .http
            .post(&url)
            .json(&body)
            .bearer_auth(key.expose())
            .timeout(FILL_LIMIT)
            .send()
            .await
            .map_err(|e| LlmError::Net(ca_net::explain(&e)))?;
        let status = res.status();
        let again = res
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let text = res.text().await.map_err(|e| LlmError::Net(ca_net::explain(&e)))?;
        if status == StatusCode::TOO_MANY_REQUESTS {
            let message = match failure(status, &text, &url) {
                LlmError::Refused(m) => m,
                e => e.to_string(),
            };
            return Err(LlmError::TooMany { message, again: Duration::from_secs(again.unwrap_or(30)) });
        }
        if !status.is_success() {
            return Err(failure(status, &text, &url));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| LlmError::Parse(format!("{url}: {e}")))?;
        let said = v.pointer("/choices/0/text").and_then(Value::as_str).unwrap_or("");
        let usage = read_usage(&v["usage"]);
        Ok((crate::fim::tidy(name, said, suffix), usage))
    }

    async fn stream_once(
        &self,
        url: &str,
        auth: &Auth,
        body: &Value,
        sink: &mut dyn Sink,
    ) -> Result<Completion, Attempt> {
        let url = url.to_string();
        let r = self.http.post(&url).json(body);
        let r = match auth {
            Auth::Bearer(k) => r.bearer_auth(k.expose()),
            Auth::Anthropic(k) => r.header("x-api-key", k.expose()).header("anthropic-version", ANTHROPIC_VERSION),
        };
        let anthropic = matches!(auth, Auth::Anthropic(_));
        let send = r.send();
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
            if let LlmError::Refused(m) = failure(status, &text, &url)
                && let Some(again) = busy(&m)
            {
                return Err(Attempt::Busy { message: m, again });
            }
            if matches!(status, StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT)
            {
                return Err(Attempt::Retry(format!("the gateway answered {}", status.as_u16())));
            }
            return Err(Attempt::Fail(failure(status, &text, &url)));
        }
        let mut answer = Assembly::default();
        let mut anth = anthropic.then(AnthropicStream::default);
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
                let took = match anth.as_mut() {
                    Some(a) => answer.take_anthropic(a, &data, sink),
                    None => answer.take(&data, sink),
                };
                if took.map_err(Attempt::Fail)? {
                    shown = true;
                }
            }
        }
        if let Some(data) = reader.finish() {
            match anth.as_mut() {
                Some(a) => answer.take_anthropic(a, &data, sink),
                None => answer.take(&data, sink),
            }
            .map_err(Attempt::Fail)?;
        }
        if let Some(a) = anth {
            answer.calls = a.calls();
            answer.finish = a.finish;
            answer.done = a.ended;
            answer.usage = Usage { prompt: a.input, cached: a.cached, completion: a.output };
        }
        Ok(answer.done())
    }
}

/// The Anthropic API version Code Arena speaks.
const ANTHROPIC_VERSION: &str = "2023-06-01";

enum Auth {
    Bearer(Secret),
    Anthropic(Secret),
}

/// A provider's refusal, in its own name: its key is not Arena's.
fn provider_failure(p: &Provider, e: LlmError) -> LlmError {
    match e {
        LlmError::Key => {
            let env = providers::known(&p.name).map(|k| k.key_env).unwrap_or("its key");
            LlmError::Refused(format!(
                "{} refused the API key (401): check {env}, or give it again with code-arena login --provider {}",
                p.name, p.name
            ))
        }
        LlmError::NoApi { url } => {
            LlmError::Refused(format!("{} has no such API at {url} (404): check its url", p.name))
        }
        other => other,
    }
}

enum Attempt {
    Retry(String),
    /// The model's line is full: Arena holds the request back, and says when to ask again.
    Busy {
        message: String,
        again: Duration,
    },
    Fail(LlmError),
}

/// How long Code Arena waits in a model's line before it gives up, as a person in the web chat would.
/// How long a code completion may take: past it, the person has typed on.
pub const FILL_LIMIT: Duration = Duration::from_secs(15);

const LINE_WAIT: Duration = Duration::from_secs(15 * 60);

/// Arena's words when a model's line is full: "… is busy: 2 answers of the chat wait for it. Try again in 30 seconds."
fn busy(message: &str) -> Option<Duration> {
    if !message.contains(" is busy") {
        return None;
    }
    let secs = message
        .split("Try again in ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(30);
    Some(Duration::from_secs(secs.clamp(1, 300)))
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
    /// One of Anthropic's events; true when something was shown.
    fn take_anthropic(&mut self, a: &mut AnthropicStream, data: &str, sink: &mut dyn Sink) -> Result<bool, LlmError> {
        let piece = a.read(data).map_err(LlmError::Refused)?;
        let mut shown = false;
        if let Some(t) = piece.text.filter(|t| !t.is_empty()) {
            self.text.push_str(&t);
            sink.text(&t);
            shown = true;
        }
        if let Some(r) = piece.thinking.filter(|r| !r.is_empty()) {
            self.reasoning.push_str(&r);
            sink.reasoning(&r);
            shown = true;
        }
        Ok(shown)
    }

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
                images: vec![],
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
    let message = if message.trim().is_empty() {
        format!("{} (it gave no reason)", status.canonical_reason().unwrap_or("no answer"))
    } else {
        message
    };
    if let Some(overflow) = crate::overflow::parse(&message).or_else(|| crate::overflow::parse(body)) {
        // The whole body may say more (llama.cpp's n_ctx beside its message).
        let mut overflow = overflow;
        if let Some(whole) = crate::overflow::parse(body) {
            overflow.prompt = overflow.prompt.or(whole.prompt);
            overflow.context = overflow.context.or(whole.context);
        }
        return LlmError::ContextOverflow { message, overflow };
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

    #[tokio::test]
    async fn a_full_line_is_waited_in() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({"error": {"message": "Qwen3-4B is busy: 2 answers of the chat wait for it. Try again in 1 seconds."}})))
            .up_to_n_times(1)
            .with_priority(1)
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
        assert_eq!(sink.notices.len(), 1);
        assert!(sink.notices[0].contains("Waiting in its line"));
    }

    #[test]
    fn the_line_is_full_by_arenas_words() {
        assert_eq!(busy("Qwen3-4B is busy: 2 answers wait. Try again in 30 seconds."), Some(Duration::from_secs(30)));
        assert_eq!(busy("Arena's safeguards refused the request"), None);
    }

    #[test]
    fn the_windows_refusal_is_an_overflow() {
        let e = failure(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"This model's maximum context length is 32768 tokens"}}"#,
            "u",
        );
        assert!(matches!(e, LlmError::ContextOverflow { overflow, .. } if overflow.context == Some(32_768)));
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

    /// A provider of OpenAI's kind that refuses what it does not know, as Mistral and others do.
    struct Strict;
    impl wiremock::Respond for Strict {
        fn respond(&self, r: &wiremock::Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&r.body).unwrap_or_default();
            let known = ["model", "messages", "stream", "tools", "max_tokens", "tool_choice"];
            if let Some(extra) = body.as_object().and_then(|o| o.keys().find(|k| !known.contains(&k.as_str()))) {
                return ResponseTemplate::new(422)
                    .set_body_json(serde_json::json!({"error": {"message": format!("extra field {extra}")}}));
            }
            if r.headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer sk-mistral") {
                return ResponseTemplate::new(401);
            }
            let ev = serde_json::json!({"choices": [{"delta": {"content": format!("{} here", body["model"].as_str().unwrap_or(""))}, "finish_reason": "stop"}]});
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse(&[&ev.to_string(), "[DONE]"]))
        }
    }

    #[tokio::test]
    async fn code_is_filled_in_with_the_models_tokens_and_a_limit_says_how_long_to_wait() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/completions"))
            .and(body_partial_json(serde_json::json!({"model": "qwen", "max_tokens": 96, "stream": false})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"text": "a + b)\n\n"}],
                "usage": {"prompt_tokens": 40, "completion_tokens": 4}
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/completions"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "12")
                    .set_body_json(serde_json::json!({"error": {"message": "Too many requests"}})),
            )
            .mount(&server)
            .await;
        let g = gateway(&server);
        let (text, usage) = g.fill("qwen", "a.rs", "fn f() { g(", ");\n}", &["chat:1".into()]).await.expect("filled");
        assert_eq!(text, "a + b");
        assert_eq!((usage.prompt, usage.completion), (40, 4));
        match g.fill("qwen", "a.rs", "x", "", &[]).await {
            Err(LlmError::TooMany { again, .. }) => assert_eq!(again, Duration::from_secs(12)),
            other => panic!("not a limit: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_providers_model_goes_to_it_with_only_what_it_takes() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/mistral/v1/chat/completions")).respond_with(Strict).mount(&server).await;
        Mock::given(method("GET"))
            .and(path("/mistral/v1/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": [{"id": "mistral-large"}]})),
            )
            .mount(&server)
            .await;
        let p = Provider::parse(
            "mistral",
            &serde_json::json!({"url": format!("{}/mistral", server.uri()), "usageOption": false}),
        )
        .expect("p");
        let mut g = Gateway::standalone(vec![(p, Secret::new("sk-mistral"))], None).expect("g");
        g.retry_delay = Duration::from_millis(1);
        assert!(!g.has_arena());
        let models = g.models().await.expect("models");
        assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["mistral:mistral-large"]);
        let req = Request {
            model: "mistral:mistral-large".into(),
            messages: vec![Message::user("hi")],
            thinking: crate::Thinking::Off,
            tags: vec!["chat:1".into()],
            ..Default::default()
        };
        let c = g.complete(&req, &mut Quiet).await.expect("answered");
        assert_eq!(c.message.content, "mistral-large here");
        // A model of no provider's, with no Arena: said so.
        let e =
            g.complete(&Request { model: "qwen".into(), ..Default::default() }, &mut Quiet).await.expect_err("none");
        assert!(e.to_string().contains("mistral:MODEL"), "{e}");
    }

    #[tokio::test]
    async fn anthropics_messages_api_and_its_key_refused_in_its_name() {
        let server = MockServer::start().await;
        let events = [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":5}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#,
            r#"{"type":"message_stop"}"#,
        ];
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "sk-ant-test"))
            .and(header("anthropic-version", ANTHROPIC_VERSION))
            .and(body_partial_json(serde_json::json!({"model": "model-x", "system": [{"text": "sys"}]})))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse(&events)),
            )
            .mount(&server)
            .await;
        let p = Provider::parse("anthropic", &serde_json::json!({"url": server.uri()})).expect("p");
        let g = Gateway::standalone(vec![(p.clone(), Secret::new("sk-ant-test"))], None).expect("g");
        let req = Request {
            model: "anthropic:model-x".into(),
            messages: vec![Message::system("sys"), Message::user("hi")],
            ..Default::default()
        };
        let mut c = Collect::default();
        let done = g.complete(&req, &mut c).await.expect("answered");
        assert_eq!(c.text, "Hello");
        assert_eq!((done.usage.prompt, done.usage.completion), (5, 2));
        assert_eq!(done.finish, Finish::Stop);
        let wrong = Gateway::standalone(vec![(p, Secret::new("sk-wrong"))], None).expect("g");
        let e = wrong.complete(&req, &mut Quiet).await.expect_err("refused");
        assert!(e.to_string().contains("ANTHROPIC_API_KEY") || e.to_string().contains("anthropic"), "{e}");
    }

    #[tokio::test]
    async fn arenas_models_and_a_providers_together() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": [{"id": "qwen"}]})))
            .mount(&server)
            .await;
        let mut g = gateway(&server);
        let p = Provider::parse("local", &serde_json::json!({"url": "http://127.0.0.1:9", "models": ["llama"]}))
            .expect("p");
        g.set_providers(vec![(p, Secret::default())]);
        let ids: Vec<String> = g.models().await.expect("models").into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["qwen", "local:llama"]);
    }
}
