//! MCP clients: streamable HTTP (Arena's tools, Farzan Graph's, a person's server at a URL) and stdio (a server run
//! as a child process). Each call has a deadline that the server's progress reports start again; past it the call
//! is given up and the server told.

mod http;
mod media;
mod stdio;

use std::path::Path;
use std::time::Duration;

use ca_core::Secret;
use serde_json::{Value, json};

pub use media::{Media, decode_base64};
pub use stdio::StdioSpec;

const PROTOCOL: &str = "2025-06-18";

/// Ten minutes without a word, unless the tool says it takes longer.
pub const DEADLINE: Duration = Duration::from_secs(600);

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error(transparent)]
    Net(#[from] ca_net::NetError),
    #[error("{url} refused the credentials ({status}): {message}")]
    Refused { url: String, status: u16, message: String },
    #[error("{url} has no MCP endpoint (404)")]
    NotThere { url: String },
    #[error("{url} answered {status}")]
    Status { url: String, status: u16 },
    #[error("{url}: {message}")]
    Protocol { url: String, message: String },
    #[error("{0} could not start: {1}")]
    Spawn(String, String),
    #[error("{0} ended")]
    Ended(String),
    #[error("no answer for {0} s: the call was given up and the server told")]
    Timeout(u64),
}

impl McpError {
    /// The server went away (a dropped connection, a proxy's 502, 503 or 504, a child that ended): connect again.
    pub fn gone(&self) -> bool {
        matches!(self, McpError::Net(_) | McpError::Ended(_))
            || matches!(self, McpError::Status { status: 502..=504, .. })
    }
}

/// A tool as the server lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// The server says it changes nothing (`annotations.readOnlyHint`).
    pub read_only: bool,
    /// How long it may take, when it says (`_meta`'s `arena/timeoutSeconds` or `timeoutSeconds`).
    pub timeout: Option<Duration>,
    /// Which of Arena's chat tools it is a function of (`_meta`'s `arena/tool`: web, python, code-intel…).
    pub arena_tool: Option<String>,
}

/// What a call gave: its text, and the pictures, sounds and files it returned.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CallResult {
    pub text: String,
    pub is_error: bool,
    pub media: Vec<Media>,
}

/// A progress report from the server while a call runs.
pub type Progress<'a> = &'a mut (dyn FnMut(&str) + Send);

/// One way of reaching a server.
#[async_trait::async_trait]
pub trait Transport: Send {
    /// Sends a request and waits for its answer (`result`), each progress report starting the wait again.
    async fn request(
        &mut self,
        id: u64,
        message: &Value,
        idle: Duration,
        progress: Progress<'_>,
    ) -> Result<Value, McpError>;
    async fn notify(&mut self, message: &Value) -> Result<(), McpError>;
    fn describe(&self) -> String;
}

pub struct Client {
    transport: Box<dyn Transport>,
    next_id: u64,
    /// The server's instructions for the model, from `initialize`.
    pub instructions: Option<String>,
    pub server_name: Option<String>,
}

impl Client {
    pub fn http(
        url: &str,
        key: Option<Secret>,
        headers: Vec<(String, Secret)>,
        ca: Option<&Path>,
    ) -> Result<Self, McpError> {
        Ok(Self::over(Box::new(http::HttpTransport::new(url, key, headers, ca)?)))
    }

    pub async fn stdio(spec: StdioSpec) -> Result<Self, McpError> {
        Ok(Self::over(Box::new(stdio::StdioTransport::start(spec).await?)))
    }

    pub fn over(transport: Box<dyn Transport>) -> Self {
        Self { transport, next_id: 1, instructions: None, server_name: None }
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        idle: Duration,
        progress: Progress<'_>,
    ) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut params = params;
        if let Value::Object(m) = &mut params {
            // A progress token on every request, so long calls can say how they go.
            m.insert("_meta".into(), json!({"progressToken": format!("p{id}")}));
        }
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        match self.transport.request(id, &msg, idle, progress).await {
            Err(McpError::Timeout(s)) => {
                let _ = self
                    .transport
                    .notify(&json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id, "reason": "no answer in time"}}))
                    .await;
                Err(McpError::Timeout(s))
            }
            r => r,
        }
    }

    pub async fn initialize(&mut self) -> Result<(), McpError> {
        let result = self
            .request(
                "initialize",
                json!({"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "code-arena", "version": ca_core::VERSION}}),
                Duration::from_secs(30),
                &mut |_| {},
            )
            .await?;
        self.instructions = result.get("instructions").and_then(Value::as_str).map(str::to_string);
        self.server_name = result.pointer("/serverInfo/name").and_then(Value::as_str).map(str::to_string);
        self.transport.notify(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await?;
        Ok(())
    }

    /// Every tool, page after page.
    pub async fn list_tools(&mut self) -> Result<Vec<ToolInfo>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..100 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let page = self.request("tools/list", params, Duration::from_secs(30), &mut |_| {}).await?;
            for t in page.get("tools").and_then(Value::as_array).into_iter().flatten() {
                let Some(name) = t.get("name").and_then(Value::as_str) else { continue };
                let meta = t.get("_meta");
                let timeout = meta
                    .and_then(|m| m.get("arena/timeoutSeconds").or_else(|| m.get("timeoutSeconds")))
                    .and_then(Value::as_u64)
                    .map(Duration::from_secs);
                tools.push(ToolInfo {
                    name: name.to_string(),
                    description: t.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
                    input_schema: t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object"})),
                    read_only: t.pointer("/annotations/readOnlyHint").and_then(Value::as_bool).unwrap_or(false),
                    timeout,
                    arena_tool: meta.and_then(|m| m.get("arena/tool")).and_then(Value::as_str).map(str::to_string),
                });
            }
            cursor = page.get("nextCursor").and_then(Value::as_str).map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    /// Calls a tool. `idle`: how long it may stay quiet; a progress report starts it again.
    pub async fn call_tool(
        &mut self,
        name: &str,
        args: Value,
        idle: Duration,
        progress: Progress<'_>,
    ) -> Result<CallResult, McpError> {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}), idle, progress).await?;
        Ok(media::read_result(&r))
    }

    /// A resource the result linked to (`resources/read`).
    pub async fn read_resource(&mut self, uri: &str) -> Result<Value, McpError> {
        self.request("resources/read", json!({"uri": uri}), Duration::from_secs(120), &mut |_| {}).await
    }

    pub fn describe(&self) -> String {
        self.transport.describe()
    }
}

/// For a session's first look: an HTTP server with only the key.
pub type HttpMcp = Client;

impl Client {
    pub fn new(url: &str, key: Option<Secret>, ca: Option<&Path>) -> Result<Self, McpError> {
        Self::http(url, key, vec![], ca)
    }
}

/// A JSON-RPC message from a server: a notification of progress, or the answer to a request.
pub(crate) enum Incoming {
    Progress(String),
    Answer(u64, Result<Value, String>),
    Other,
}

pub(crate) fn classify(msg: &Value) -> Incoming {
    if msg.get("method").and_then(Value::as_str) == Some("notifications/progress") {
        let p = msg.get("params");
        let text = p
            .and_then(|p| p.get("message").and_then(Value::as_str).map(str::to_string))
            .or_else(|| {
                let done = p?.get("progress")?.as_f64()?;
                Some(match p?.get("total").and_then(Value::as_f64) {
                    Some(t) if t > 0.0 => format!("{:.0}%", done * 100.0 / t),
                    _ => format!("{done}"),
                })
            })
            .unwrap_or_default();
        return Incoming::Progress(text);
    }
    if let Some(id) = msg.get("id").and_then(Value::as_u64)
        && (msg.get("result").is_some() || msg.get("error").is_some())
    {
        if let Some(e) = msg.get("error") {
            let m = e.get("message").and_then(Value::as_str).unwrap_or("an error").to_string();
            return Incoming::Answer(id, Err(m));
        }
        return Incoming::Answer(id, Ok(msg.get("result").cloned().unwrap_or(Value::Null)));
    }
    Incoming::Other
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A server that answers each request with its own id, as a real one does.
    struct Server;

    impl wiremock::Respond for Server {
        fn respond(&self, r: &wiremock::Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&r.body).expect("json");
            let id = body.get("id").cloned();
            let with_session = r.headers.get("mcp-session-id").and_then(|v| v.to_str().ok()) == Some("s1");
            let sse = |v: Value| {
                ResponseTemplate::new(200)
                    .set_body_string(format!("event: message\ndata: {v}\n\n"))
                    .insert_header("content-type", "text/event-stream")
            };
            match body["method"].as_str() {
                Some("initialize") => ResponseTemplate::new(200).insert_header("mcp-session-id", "s1").set_body_json(
                    json!({"jsonrpc": "2.0", "id": id, "result": {"serverInfo": {"name": "arena"}, "instructions": "Use web_search for news."}}),
                ),
                _ if !with_session => ResponseTemplate::new(400),
                Some("notifications/initialized") => ResponseTemplate::new(202),
                Some("tools/list") if body["params"].get("cursor").is_none() => sse(json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [
                    {"name": "web_search", "annotations": {"readOnlyHint": true}, "_meta": {"arena/timeoutSeconds": 900, "arena/tool": "web"}}
                ], "nextCursor": "p2"}})),
                Some("tools/list") => ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "make_image", "description": "Pictures"}]}})),
                Some("tools/call") => {
                    let token = body.pointer("/params/_meta/progressToken").cloned();
                    let progress = json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": token, "progress": 1, "total": 2, "message": "half way"}});
                    let answer = json!({"jsonrpc": "2.0", "id": id, "result": {"content": [
                        {"type": "text", "text": "found 3 pages"},
                        {"type": "image", "mimeType": "image/png", "data": "iVBORw0KGgo="}
                    ]}});
                    ResponseTemplate::new(200)
                        .set_body_string(format!("data: {progress}\n\ndata: {answer}\n\n"))
                        .insert_header("content-type", "text/event-stream")
                }
                _ => ResponseTemplate::new(400),
            }
        }
    }

    #[tokio::test]
    async fn initialize_list_and_call_with_progress_and_a_picture() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(Server).mount(&server).await;
        let mut m = Client::new(&server.uri(), Some(Secret::new("sk")), None).expect("client");
        m.initialize().await.expect("init");
        assert_eq!(m.server_name.as_deref(), Some("arena"));
        assert_eq!(m.instructions.as_deref(), Some("Use web_search for news."));
        let tools = m.list_tools().await.expect("tools");
        assert_eq!(tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["web_search", "make_image"]);
        assert!(tools[0].read_only && !tools[1].read_only);
        assert_eq!(tools[0].timeout, Some(Duration::from_secs(900)));
        assert_eq!(tools[0].arena_tool.as_deref(), Some("web"));
        let mut said = Vec::new();
        let r = m
            .call_tool("web_search", json!({"q": "x"}), DEADLINE, &mut |p| said.push(p.to_string()))
            .await
            .expect("call");
        assert_eq!(said, vec!["half way"]);
        assert_eq!(r.text, "found 3 pages");
        assert_eq!(r.media.len(), 1);
        assert_eq!(r.media[0].mime, "image/png");
        assert_eq!(&r.media[0].bytes[..4], &[0x89, b'P', b'N', b'G']);
    }

    #[tokio::test]
    async fn no_endpoint_and_refusals_are_said() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        let mut m = Client::new(&server.uri(), None, None).expect("client");
        assert!(matches!(m.initialize().await, Err(McpError::NotThere { .. })));
    }
}
