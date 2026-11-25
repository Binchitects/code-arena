//! Streamable HTTP: each request a POST, its answer JSON or an event stream read as it comes (so progress reports
//! arrive while the call runs).

use std::path::Path;
use std::time::Duration;

use ca_core::Secret;
use futures_util::StreamExt;
use serde_json::Value;

use crate::{Incoming, McpError, PROTOCOL, Progress, Transport, classify};

pub struct HttpTransport {
    url: String,
    key: Option<Secret>,
    headers: Vec<(String, Secret)>,
    http: reqwest::Client,
    session: Option<String>,
}

impl HttpTransport {
    pub fn new(
        url: &str,
        key: Option<Secret>,
        headers: Vec<(String, Secret)>,
        ca: Option<&Path>,
    ) -> Result<Self, McpError> {
        Ok(Self { url: url.to_string(), key, headers, http: ca_net::client(ca)?, session: None })
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut r = self
            .http
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL)
            .json(body);
        if let Some(k) = &self.key {
            r = r.bearer_auth(k.expose());
        }
        for (name, value) in &self.headers {
            r = r.header(name.as_str(), value.expose());
        }
        if let Some(s) = &self.session {
            r = r.header("Mcp-Session-Id", s);
        }
        r
    }

    fn refusal(&self, status: u16, text: &str) -> McpError {
        match status {
            401 | 403 => {
                let message = serde_json::from_str::<Value>(text)
                    .ok()
                    .and_then(|v| {
                        v.get("error").and_then(|e| {
                            e.as_str().map(str::to_string).or_else(|| e.get("message")?.as_str().map(str::to_string))
                        })
                    })
                    .unwrap_or_else(|| text.chars().take(200).collect());
                McpError::Refused { url: self.url.clone(), status, message }
            }
            404 => McpError::NotThere { url: self.url.clone() },
            s => McpError::Status { url: self.url.clone(), status: s },
        }
    }
}

#[async_trait::async_trait]
impl Transport for HttpTransport {
    async fn request(
        &mut self,
        id: u64,
        message: &Value,
        idle: Duration,
        progress: Progress<'_>,
    ) -> Result<Value, McpError> {
        let send = self.post(message).send();
        let res = match tokio::time::timeout(idle, send).await {
            Err(_) => return Err(McpError::Timeout(idle.as_secs())),
            Ok(r) => r.map_err(|e| McpError::Net(ca_net::explain(&e)))?,
        };
        if let Some(s) = res.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            self.session = Some(s.to_string());
        }
        let status = res.status().as_u16();
        let content_type =
            res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        if !(200..300).contains(&status) {
            let text = res.text().await.unwrap_or_default();
            return Err(self.refusal(status, &text));
        }
        let protocol = |m: String| McpError::Protocol { url: self.url.clone(), message: m };
        let mut stream = res.bytes_stream();
        let mut buf = String::new();
        loop {
            let chunk = match tokio::time::timeout(idle, stream.next()).await {
                Err(_) => return Err(McpError::Timeout(idle.as_secs())),
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(McpError::Net(ca_net::explain(&e))),
                Ok(Some(Ok(b))) => b,
            };
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // An event stream by its header or by its shape (some servers label it as text): read event by event.
            let streaming = content_type.contains("text/event-stream")
                || buf.trim_start().starts_with("event:")
                || buf.trim_start().starts_with("data:");
            if !streaming {
                continue;
            }
            while let Some(end) = buf.find("\n\n") {
                let event: String = buf.drain(..end + 2).collect();
                let data: Vec<&str> =
                    event.lines().filter_map(|l| l.strip_prefix("data:").map(str::trim_start)).collect();
                if data.is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<Value>(&data.join("\n")) else { continue };
                match classify(&msg) {
                    Incoming::Progress(text) => progress(&text),
                    Incoming::Answer(got, answer) if got == id => return answer.map_err(protocol),
                    _ => {}
                }
            }
        }
        // A JSON answer, or what was left of a stream.
        let rest = buf.trim();
        let candidates: Vec<Value> = if rest.starts_with('{') || rest.starts_with('[') {
            match serde_json::from_str::<Value>(rest) {
                Ok(Value::Array(a)) => a,
                Ok(v) => vec![v],
                Err(_) => vec![],
            }
        } else {
            rest.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|d| serde_json::from_str(d.trim()).ok())
                .collect()
        };
        for msg in candidates {
            match classify(&msg) {
                Incoming::Progress(text) => progress(&text),
                Incoming::Answer(got, answer) if got == id => return answer.map_err(protocol),
                _ => {}
            }
        }
        Err(protocol("no answer to the request".into()))
    }

    async fn notify(&mut self, message: &Value) -> Result<(), McpError> {
        let res = self
            .post(message)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| McpError::Net(ca_net::explain(&e)))?;
        let status = res.status().as_u16();
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(self.refusal(status, &res.text().await.unwrap_or_default()))
        }
    }

    fn describe(&self) -> String {
        self.url.clone()
    }
}
