//! A session's message in Arena's shape, and back.

use ca_arena::chats::SyncMessage;
use ca_llm::{Message, Role, ToolCall};
use ca_session::{Store, Stored};
use serde_json::{Value, json};

/// A kept message as Arena takes it: its ref (`session:seq`), and for a tool's result the tool's name.
pub fn to_arena(store: &Store, session: &str, m: &Stored, model: &str) -> SyncMessage {
    let msg = &m.message;
    let role = match msg.role {
        Role::User | Role::System => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    let tool_calls = (!msg.tool_calls.is_empty()).then(|| {
        Value::Array(
            msg.tool_calls
                .iter()
                .map(
                    |c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.arguments}}),
                )
                .collect(),
        )
    });
    let name = match (&msg.role, &msg.tool_call_id) {
        (Role::Tool, Some(call)) => store.call_name(session, m.seq, call).ok().flatten(),
        _ => None,
    };
    // Code Arena's own words to its model (a summary's preface, a note that the connection dropped): a notice on the
    // web, not the person's.
    let kind = (msg.role == Role::User && msg.content.starts_with("(Code Arena")).then(|| "note".to_string());
    SyncMessage {
        role: role.into(),
        content: Value::String(msg.content.clone()),
        reasoning: msg.reasoning.clone().filter(|r| !r.is_empty()),
        tool_calls,
        tool_call_id: if msg.role == Role::Tool { msg.tool_call_id.clone() } else { None },
        name,
        model: (msg.role == Role::Assistant && !model.is_empty()).then(|| model.to_string()),
        reference: Some(format!("{session}:{}", m.seq)),
        kind,
        ..Default::default()
    }
}

/// A message the web added, as the session keeps it; `None` for one the model does not read.
pub fn from_arena(m: &SyncMessage) -> Option<Message> {
    let text = m.text();
    let msg = match m.role.as_str() {
        "user" => Message::user(text),
        "assistant" => {
            let mut a = Message::assistant(text);
            a.reasoning = m.reasoning.clone().filter(|r| !r.is_empty());
            if let Some(Value::Array(calls)) = &m.tool_calls {
                a.tool_calls = calls
                    .iter()
                    .filter_map(|c| {
                        let f = c.get("function")?;
                        Some(ToolCall {
                            id: c.get("id")?.as_str()?.to_string(),
                            name: f.get("name")?.as_str()?.to_string(),
                            arguments: match f.get("arguments") {
                                Some(Value::String(s)) => s.clone(),
                                Some(v) => v.to_string(),
                                None => "{}".into(),
                            },
                        })
                    })
                    .collect();
            }
            a
        }
        "tool" => Message::tool(m.tool_call_id.clone().unwrap_or_default(), text),
        _ => return None,
    };
    Some(msg)
}
