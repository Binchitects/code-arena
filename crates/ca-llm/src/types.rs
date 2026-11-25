//! A conversation as the model reads it.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// As the model wrote them: JSON, or nearly (mended before the call runs).
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// The model's thinking: kept and shown, never sent back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self::of(Role::System, text)
    }
    pub fn user(text: impl Into<String>) -> Self {
        Self::of(Role::User, text)
    }
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::of(Role::Assistant, text)
    }
    pub fn tool(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Message { tool_call_id: Some(call_id.into()), ..Self::of(Role::Tool, text) }
    }
    fn of(role: Role, text: impl Into<String>) -> Self {
        Message { role, content: text.into(), tool_calls: vec![], tool_call_id: None, reasoning: None }
    }

    /// As OpenAI's chat completions take it.
    pub fn to_wire(&self) -> Value {
        let mut m = json!({"role": self.role, "content": self.content});
        if !self.tool_calls.is_empty() {
            m["tool_calls"] = self
                .tool_calls
                .iter()
                .map(
                    |c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.arguments}}),
                )
                .collect();
            if self.content.is_empty() {
                m["content"] = Value::Null;
            }
        }
        if let Some(id) = &self.tool_call_id {
            m["tool_call_id"] = json!(id);
        }
        m
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// A JSON schema of its arguments.
    pub parameters: Value,
}

impl ToolDef {
    pub fn to_wire(&self) -> Value {
        json!({"type": "function", "function": {"name": self.name, "description": self.description, "parameters": self.parameters}})
    }
}

/// How much the model thinks, sent as Arena's web chat sends it (through `chat_template_kwargs`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Thinking {
    /// The model's own (or the Arena's default level, when it says one).
    #[default]
    Default,
    Off,
    Level(String),
}

impl Thinking {
    pub fn parse(word: &str) -> Self {
        match word {
            "" | "default" => Thinking::Default,
            "off" | "none" => Thinking::Off,
            w => Thinking::Level(w.to_string()),
        }
    }

    fn kwargs(&self) -> Option<Value> {
        match self {
            Thinking::Default => None,
            Thinking::Off => Some(json!({"enable_thinking": false})),
            Thinking::Level(l) => Some(json!({"reasoning_effort": l})),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub thinking: Thinking,
    pub max_tokens: Option<u32>,
    /// `chat:<id>`, `project:<id>`: how Arena's usage log knows where a request counts.
    pub tags: Vec<String>,
    /// `none`: the tools are offered but not to be called (a summary).
    pub tool_choice: Option<String>,
}

impl Request {
    pub fn to_wire(&self) -> Value {
        let mut body = json!({
            "model": self.model,
            "messages": self.messages.iter().map(Message::to_wire).collect::<Vec<_>>(),
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        if !self.tools.is_empty() {
            body["tools"] = self.tools.iter().map(ToolDef::to_wire).collect();
        }
        if let Some(k) = self.thinking.kwargs() {
            body["chat_template_kwargs"] = k;
        }
        if let Some(m) = self.max_tokens {
            body["max_tokens"] = json!(m);
        }
        if let Some(tc) = &self.tool_choice {
            body["tool_choice"] = json!(tc);
        }
        if !self.tags.is_empty() {
            body["metadata"] = json!({"tags": self.tags});
        }
        body
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt: u64,
    pub cached: u64,
    pub completion: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.prompt += other.prompt;
        self.cached += other.cached;
        self.completion += other.completion;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    Stop,
    ToolCalls,
    /// Cut at the output limit: the agent asks the model to carry on.
    Length,
    /// The stream ended without saying why (a dropped connection): what came is kept.
    Dropped,
    Other,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub message: Message,
    pub finish: Finish,
    pub usage: Usage,
}

/// Where a streamed answer goes as it comes.
pub trait Sink: Send {
    fn text(&mut self, _delta: &str) {}
    fn reasoning(&mut self, _delta: &str) {}
    /// A notice for the person: a retry, a wait.
    fn notice(&mut self, _text: &str) {}
}

/// A sink that shows nothing.
pub struct Quiet;
impl Sink for Quiet {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_goes_as_the_chat_sends_it() {
        let mut r = Request { model: "m".into(), messages: vec![Message::user("hi")], ..Default::default() };
        assert!(r.to_wire().get("chat_template_kwargs").is_none());
        r.thinking = Thinking::Off;
        assert_eq!(r.to_wire()["chat_template_kwargs"], json!({"enable_thinking": false}));
        r.thinking = Thinking::parse("high");
        assert_eq!(r.to_wire()["chat_template_kwargs"], json!({"reasoning_effort": "high"}));
    }

    #[test]
    fn tool_calls_and_results_on_the_wire() {
        let mut a = Message::assistant("");
        a.tool_calls.push(ToolCall { id: "c1".into(), name: "read_file".into(), arguments: r#"{"path":"a"}"#.into() });
        a.reasoning = Some("never sent".into());
        let w = a.to_wire();
        assert_eq!(w["content"], Value::Null);
        assert_eq!(w["tool_calls"][0]["function"]["name"], "read_file");
        assert!(w.get("reasoning").is_none());
        assert_eq!(Message::tool("c1", "ok").to_wire()["tool_call_id"], "c1");
    }
}
