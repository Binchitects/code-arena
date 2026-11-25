//! MCP over streamable HTTP: initialize, then list the server's tools. An answer may come as JSON or as an
//! event stream; both are read.

use std::path::Path;
use std::time::Duration;

use ca_core::Secret;
use serde_json::{Value, json};

const PROTOCOL: &str = "2025-06-18";

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
}

/// A tool as the server lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// The server says it changes nothing (`annotations.readOnlyHint`).
    pub read_only: bool,
}

pub struct HttpMcp {
    url: String,
    key: Option<Secret>,
    http: reqwest::Client,
    session: Option<String>,
    next_id: u64,
    /// The server's instructions for the model, from `initialize`.
    pub instructions: Option<String>,
    pub server_name: Option<String>,
}

impl HttpMcp {
    pub fn new(url: &str, key: Option<Secret>, ca: Option<&Path>) -> Result<Self, McpError> {
        Ok(Self {
            url: url.to_string(),
            key,
            http: ca_net::client(ca)?,
            session: None,
            next_id: 1,
            instructions: None,
            server_name: None,
        })
    }

    async fn post(&mut self, body: &Value, expect_answer: bool) -> Result<Option<Value>, McpError> {
        let mut r = self
            .http
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL)
            .timeout(Duration::from_secs(30))
            .json(body);
        if let Some(k) = &self.key {
            r = r.bearer_auth(k.expose());
        }
        if let Some(s) = &self.session {
            r = r.header("Mcp-Session-Id", s);
        }
        let res = r.send().await.map_err(|e| McpError::Net(ca_net::explain(&e)))?;
        let status = res.status();
        if let Some(s) = res.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            self.session = Some(s.to_string());
        }
        let content_type =
            res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let text = res.text().await.map_err(|e| McpError::Net(ca_net::explain(&e)))?;
        match status.as_u16() {
            401 | 403 => {
                let message = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| {
                        v.get("error").and_then(|e| {
                            e.as_str().map(str::to_string).or_else(|| e.get("message")?.as_str().map(str::to_string))
                        })
                    })
                    .unwrap_or_else(|| text.chars().take(200).collect());
                return Err(McpError::Refused { url: self.url.clone(), status: status.as_u16(), message });
            }
            404 => return Err(McpError::NotThere { url: self.url.clone() }),
            s if !(200..300).contains(&s) => return Err(McpError::Status { url: self.url.clone(), status: s }),
            _ => {}
        }
        if !expect_answer {
            return Ok(None);
        }
        let id = body.get("id").cloned();
        // An event stream by its header, or by its shape (some servers label it as text).
        let stream = content_type.contains("text/event-stream")
            || text.trim_start().starts_with("event:")
            || text.trim_start().starts_with("data:");
        let messages: Vec<Value> = if stream {
            text.split("\n\n")
                .filter_map(|ev| {
                    let data: Vec<&str> =
                        ev.lines().filter_map(|l| l.strip_prefix("data:").map(str::trim_start)).collect();
                    (!data.is_empty()).then(|| serde_json::from_str(&data.join("\n")).ok()).flatten()
                })
                .collect()
        } else {
            serde_json::from_str::<Value>(&text).map(|v| vec![v]).unwrap_or_default()
        };
        let answer = messages
            .into_iter()
            .find(|m| m.get("id") == id.as_ref())
            .ok_or_else(|| McpError::Protocol { url: self.url.clone(), message: "no answer to the request".into() })?;
        if let Some(err) = answer.get("error") {
            let message = err.get("message").and_then(Value::as_str).unwrap_or("an error").to_string();
            return Err(McpError::Protocol { url: self.url.clone(), message });
        }
        Ok(answer.get("result").cloned())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        Ok(self.post(&body, true).await?.unwrap_or(Value::Null))
    }

    pub async fn initialize(&mut self) -> Result<(), McpError> {
        let result = self
            .request(
                "initialize",
                json!({"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "code-arena", "version": ca_core::VERSION}}),
            )
            .await?;
        self.instructions = result.get("instructions").and_then(Value::as_str).map(str::to_string);
        self.server_name = result.pointer("/serverInfo/name").and_then(Value::as_str).map(str::to_string);
        self.post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), false).await?;
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
            let page = self.request("tools/list", params).await?;
            for t in page.get("tools").and_then(Value::as_array).into_iter().flatten() {
                let Some(name) = t.get("name").and_then(Value::as_str) else { continue };
                tools.push(ToolInfo {
                    name: name.to_string(),
                    description: t.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
                    input_schema: t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object"})),
                    read_only: t.pointer("/annotations/readOnlyHint").and_then(Value::as_bool).unwrap_or(false),
                });
            }
            cursor = page.get("nextCursor").and_then(Value::as_str).map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }
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
            match body["method"].as_str() {
                Some("initialize") => ResponseTemplate::new(200).insert_header("mcp-session-id", "s1").set_body_json(
                    json!({"jsonrpc": "2.0", "id": id, "result": {"serverInfo": {"name": "arena"}, "instructions": "Use web_search for news."}}),
                ),
                _ if !with_session => ResponseTemplate::new(400),
                Some("notifications/initialized") => ResponseTemplate::new(202),
                Some("tools/list") if body["params"].get("cursor").is_none() => {
                    let answer = json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "web_search", "annotations": {"readOnlyHint": true}}], "nextCursor": "p2"}});
                    ResponseTemplate::new(200).set_body_string(format!("event: message\ndata: {answer}\n\n")).insert_header("content-type", "text/event-stream")
                }
                Some("tools/list") => ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "make_image", "description": "Pictures"}]}})),
                _ => ResponseTemplate::new(400),
            }
        }
    }

    #[tokio::test]
    async fn initialize_then_tools_page_by_page_over_json_and_sse() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(Server).mount(&server).await;
        let mut m = HttpMcp::new(&server.uri(), Some(Secret::new("sk")), None).expect("client");
        m.initialize().await.expect("init");
        assert_eq!(m.server_name.as_deref(), Some("arena"));
        assert_eq!(m.instructions.as_deref(), Some("Use web_search for news."));
        let tools = m.list_tools().await.expect("tools");
        assert_eq!(tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["web_search", "make_image"]);
        assert!(tools[0].read_only && !tools[1].read_only);
    }

    #[tokio::test]
    async fn no_endpoint_and_refusals_are_said() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        let mut m = HttpMcp::new(&server.uri(), None, None).expect("client");
        assert!(matches!(m.initialize().await, Err(McpError::NotThere { .. })));
    }
}
