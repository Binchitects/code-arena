//! Providers of models beside Arena's gateway, with the person's own key: Anthropic (its Messages API), or any API that
//! speaks OpenAI's chat completions (OpenAI, DeepSeek, OpenRouter, Mistral, a local Ollama, a company's own vLLM). Their
//! models are offered as `name:model` (`anthropic:claude-sonnet-5-5`) and asked of them directly. Without Arena they are
//! the only models there are (the standalone edition); signed in to Arena, its admins decide whether they are used.

use serde_json::{Map, Value, json};

/// How a provider's API is spoken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// OpenAI's chat completions.
    OpenAi,
    /// Anthropic's Messages API.
    Anthropic,
}

/// A provider known by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known {
    pub name: &'static str,
    pub kind: Kind,
    /// Its API's address, without `/v1`.
    pub url: &'static str,
    /// The environment variable that holds the key.
    pub key_env: &'static str,
    pub about: &'static str,
    /// Whether it takes `stream_options` (the usage at the stream's end).
    pub usage_option: bool,
    /// Whether it takes `reasoning_effort` for thinking.
    pub effort: bool,
}

pub const KNOWN: &[Known] = &[
    Known {
        name: "anthropic",
        kind: Kind::Anthropic,
        url: "https://api.anthropic.com",
        key_env: "ANTHROPIC_API_KEY",
        about: "Anthropic: Claude (console.anthropic.com, API keys)",
        usage_option: false,
        effort: true,
    },
    Known {
        name: "openai",
        kind: Kind::OpenAi,
        url: "https://api.openai.com",
        key_env: "OPENAI_API_KEY",
        about: "OpenAI (platform.openai.com, API keys)",
        usage_option: true,
        effort: true,
    },
    Known {
        name: "deepseek",
        kind: Kind::OpenAi,
        url: "https://api.deepseek.com",
        key_env: "DEEPSEEK_API_KEY",
        about: "DeepSeek (platform.deepseek.com, API keys)",
        usage_option: true,
        effort: false,
    },
    Known {
        name: "openrouter",
        kind: Kind::OpenAi,
        url: "https://openrouter.ai/api",
        key_env: "OPENROUTER_API_KEY",
        about: "OpenRouter: many providers' models (openrouter.ai, keys)",
        usage_option: true,
        effort: false,
    },
    Known {
        name: "mistral",
        kind: Kind::OpenAi,
        url: "https://api.mistral.ai",
        key_env: "MISTRAL_API_KEY",
        about: "Mistral (console.mistral.ai, API keys)",
        usage_option: false,
        effort: false,
    },
    Known {
        name: "ollama",
        kind: Kind::OpenAi,
        url: "http://localhost:11434",
        key_env: "OLLAMA_API_KEY",
        about: "Ollama on this machine (no key needed)",
        usage_option: true,
        effort: false,
    },
];

pub fn known(name: &str) -> Option<&'static Known> {
    KNOWN.iter().find(|k| k.name == name)
}

/// A provider's name: lower-case letters, digits and `-`, not from a `-`.
pub fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// What one provider is, as the settings have it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub name: String,
    pub kind: Kind,
    /// Its API's address, without `/v1`.
    pub url: String,
    /// The models offered; empty: those its API lists.
    pub models: Vec<String>,
    /// Its models' window when its API does not say (tokens).
    pub context: Option<u64>,
    pub usage_option: bool,
    pub effort: bool,
}

impl Provider {
    /// A provider by its settings (`{"kind": "openai", "url": "http://gpu:8000", "models": [...], "context": 32768}`),
    /// a known one's from its name.
    pub fn parse(name: &str, o: &Value) -> Result<Provider, String> {
        if !valid_name(name) {
            return Err(format!("{name} is not a provider's name (lower-case letters, digits and -)"));
        }
        let k = known(name);
        let kind = match o.get("kind").and_then(Value::as_str) {
            Some("anthropic") => Kind::Anthropic,
            Some("openai") => Kind::OpenAi,
            Some(other) => return Err(format!("{name}'s kind is anthropic or openai (chat completions), not {other}")),
            None => k.map(|k| k.kind).unwrap_or(Kind::OpenAi),
        };
        let url = match o.get("url").and_then(Value::as_str) {
            Some(u) => u.to_string(),
            None => k
                .map(|k| k.url.to_string())
                .ok_or(format!("{name} needs its \"url\" (its API's address, without /v1)"))?,
        };
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(format!("{name}'s url is not an http(s) address: {url}"));
        }
        let url = url.trim_end_matches('/');
        let url = url.strip_suffix("/v1").unwrap_or(url).to_string();
        let models = o
            .get("models")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let custom = k.is_none() || o.get("url").is_some();
        Ok(Provider {
            name: name.into(),
            kind,
            url,
            models,
            context: o.get("context").and_then(Value::as_u64),
            // A provider of one's own (vLLM, llama.cpp, LiteLLM) takes the usage option; an unknown remote may not.
            usage_option: o.get("usageOption").and_then(Value::as_bool).unwrap_or(match k {
                Some(k) if !custom => k.usage_option,
                _ => true,
            }),
            effort: k.map(|k| k.effort).unwrap_or(false),
        })
    }
}

/// The body a provider of OpenAI's kind takes: Arena's own fields left out (its template switches, its usage tags), the
/// thinking level as `reasoning_effort` where the provider takes it.
pub fn openai_body(mut body: Value, model: &str, thinking: &crate::Thinking, p: &Provider) -> Value {
    if let Some(o) = body.as_object_mut() {
        o.remove("chat_template_kwargs");
        o.remove("metadata");
        if !p.usage_option {
            o.remove("stream_options");
        }
        o.insert("model".into(), json!(model));
        if p.effort
            && let crate::Thinking::Level(l) = thinking
        {
            o.insert("reasoning_effort".into(), json!(l));
        }
    }
    body
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The chat-completions body as Anthropic's Messages API takes it: the system apart (cached), turns that alternate,
/// tool calls and results as blocks, thinking as a budget below the answer's room.
pub fn anthropic_body(body: &Value, model: &str, thinking: &crate::Thinking) -> Value {
    let mut system = String::new();
    let mut messages: Vec<Map<String, Value>> = Vec::new();
    let mut add = |messages: &mut Vec<Map<String, Value>>, role: &str, block: Value| {
        if let Some(last) = messages.last_mut()
            && last.get("role").and_then(Value::as_str) == Some(role)
        {
            if let Some(Value::Array(c)) = last.get_mut("content") {
                c.push(block);
            }
            return;
        }
        let mut m = Map::new();
        m.insert("role".into(), json!(role));
        m.insert("content".into(), json!([block]));
        messages.push(m);
    };
    for m in body["messages"].as_array().into_iter().flatten() {
        match m.get("role").and_then(Value::as_str).unwrap_or("user") {
            "system" | "developer" => {
                let s = text_of(&m["content"]);
                if !s.is_empty() {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&s);
                }
            }
            "tool" => add(
                &mut messages,
                "user",
                json!({"type": "tool_result", "tool_use_id": m["tool_call_id"].as_str().unwrap_or(""), "content": text_of(&m["content"])}),
            ),
            "assistant" => {
                let said = text_of(&m["content"]);
                if !said.is_empty() {
                    add(&mut messages, "assistant", json!({"type": "text", "text": said}));
                }
                for call in m["tool_calls"].as_array().into_iter().flatten() {
                    let f = &call["function"];
                    let args = f["arguments"].as_str().filter(|a| !a.trim().is_empty()).unwrap_or("{}");
                    let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                    let input = if input.is_object() { input } else { json!({}) };
                    add(
                        &mut messages,
                        "assistant",
                        json!({"type": "tool_use", "id": call["id"].as_str().unwrap_or(""), "name": f["name"].as_str().unwrap_or(""), "input": input}),
                    );
                }
            }
            _ => {
                let t = text_of(&m["content"]);
                add(
                    &mut messages,
                    "user",
                    json!({"type": "text", "text": if t.is_empty() { "(nothing)".to_string() } else { t }}),
                );
            }
        }
    }
    // A turn must hold something.
    for m in &mut messages {
        if let Some(Value::Array(c)) = m.get_mut("content")
            && c.is_empty()
        {
            c.push(json!({"type": "text", "text": "(nothing)"}));
        }
    }
    let max = body["max_tokens"].as_u64().unwrap_or(16_000);
    let mut req = json!({"model": model, "max_tokens": max, "messages": messages, "stream": true});
    if !system.is_empty() {
        // The instructions, the same each turn: cached by Anthropic.
        req["system"] = json!([{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}]);
    }
    let tools: Vec<Value> = body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("function"))
        .map(|f| {
            json!({"name": f["name"], "description": f["description"].as_str().unwrap_or(""), "input_schema": if f["parameters"].is_object() { f["parameters"].clone() } else { json!({"type": "object"}) }})
        })
        .collect();
    if !tools.is_empty() {
        req["tools"] = json!(tools);
        if body["tool_choice"].as_str() == Some("none") {
            req["tool_choice"] = json!({"type": "none"});
        }
    }
    if let crate::Thinking::Level(l) = thinking {
        let budget = match l.as_str() {
            "low" => 2048,
            "medium" => 8192,
            "high" => 16384,
            _ => 32768,
        };
        if max <= budget + 1024 {
            req["max_tokens"] = json!(budget + 8192);
        }
        req["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
    }
    req
}

/// Anthropic's stream, event by event, into what chat completions would have said.
#[derive(Debug, Default)]
pub struct AnthropicStream {
    calls: Vec<(String, String, String)>,
    call_at: std::collections::HashMap<u64, usize>,
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub finish: Option<crate::Finish>,
    pub ended: bool,
}

/// What one event added.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Piece {
    pub text: Option<String>,
    pub thinking: Option<String>,
}

impl AnthropicStream {
    pub fn read(&mut self, data: &str) -> Result<Piece, String> {
        let Ok(e) = serde_json::from_str::<Value>(data) else { return Ok(Piece::default()) };
        match e["type"].as_str() {
            Some("message_start") => {
                let u = &e["message"]["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                self.input = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
                self.cached = n("cache_read_input_tokens");
            }
            Some("content_block_start") if e["content_block"]["type"] == "tool_use" => {
                let b = &e["content_block"];
                self.call_at.insert(e["index"].as_u64().unwrap_or(0), self.calls.len());
                let id = b["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("call_{}", self.calls.len()));
                self.calls.push((id, b["name"].as_str().unwrap_or("").to_string(), String::new()));
            }
            Some("content_block_delta") => {
                let d = &e["delta"];
                match d["type"].as_str() {
                    Some("text_delta") => {
                        return Ok(Piece { text: d["text"].as_str().map(str::to_string), thinking: None });
                    }
                    Some("thinking_delta") => {
                        return Ok(Piece { text: None, thinking: d["thinking"].as_str().map(str::to_string) });
                    }
                    Some("input_json_delta") => {
                        if let Some(&at) = self.call_at.get(&e["index"].as_u64().unwrap_or(0)) {
                            self.calls[at].2.push_str(d["partial_json"].as_str().unwrap_or(""));
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(r) = e["delta"]["stop_reason"].as_str() {
                    self.finish = Some(match r {
                        "tool_use" => crate::Finish::ToolCalls,
                        "max_tokens" => crate::Finish::Length,
                        _ => crate::Finish::Stop,
                    });
                }
                if let Some(o) = e["usage"]["output_tokens"].as_u64() {
                    self.output = o;
                }
            }
            Some("message_stop") => self.ended = true,
            Some("error") => {
                return Err(e["error"]["message"].as_str().unwrap_or("Anthropic stopped the answer.").to_string());
            }
            _ => {}
        }
        Ok(Piece::default())
    }

    pub fn calls(&self) -> Vec<crate::ToolCall> {
        self.calls
            .iter()
            .map(|(id, name, args)| crate::ToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments: if args.is_empty() { "{}".into() } else { args.clone() },
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Thinking;

    #[test]
    fn known_and_own_providers() {
        let p = Provider::parse("deepseek", &json!({})).expect("known");
        assert_eq!((p.kind, p.url.as_str()), (Kind::OpenAi, "https://api.deepseek.com"));
        let p = Provider::parse("gpu", &json!({"url": "http://gpu:8000/v1/", "models": ["qwen3"]})).expect("own");
        assert_eq!((p.url.as_str(), p.models.clone()), ("http://gpu:8000", vec!["qwen3".to_string()]));
        assert!(Provider::parse("gpu", &json!({})).is_err());
        assert!(Provider::parse("Bad", &json!({})).is_err());
        assert!(Provider::parse("x", &json!({"url": "ftp://x"})).is_err());
    }

    #[test]
    fn an_openai_body_has_only_what_openai_takes() {
        let body = json!({"model": "openai:gpt-x", "messages": [], "stream": true, "stream_options": {"include_usage": true},
            "chat_template_kwargs": {"enable_thinking": false}, "metadata": {"tags": ["chat:1"]}});
        let mistral = Provider::parse("mistral", &json!({})).expect("p");
        let b = openai_body(body.clone(), "mistral-large", &Thinking::Level("high".into()), &mistral);
        assert_eq!(b, json!({"model": "mistral-large", "messages": [], "stream": true}));
        let openai = Provider::parse("openai", &json!({})).expect("p");
        let b = openai_body(body, "gpt-x", &Thinking::Level("high".into()), &openai);
        assert_eq!(b["reasoning_effort"], "high");
        assert_eq!(b["stream_options"]["include_usage"], true);
    }

    #[test]
    fn a_conversation_in_anthropics_shape() {
        let body = json!({"messages": [
            {"role": "system", "content": "Be brief."},
            {"role": "user", "content": "read a.rs"},
            {"role": "assistant", "content": "", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"a.rs\"}"}}]},
            {"role": "tool", "tool_call_id": "c1", "content": "fn a() {}"},
            {"role": "user", "content": "and?"}
        ], "tools": [{"type": "function", "function": {"name": "read_file", "description": "Reads", "parameters": {"type": "object"}}}], "max_tokens": 1000});
        let a = anthropic_body(&body, "claude-x", &Thinking::Level("low".into()));
        assert_eq!(a["system"][0]["text"], "Be brief.");
        assert_eq!(a["system"][0]["cache_control"]["type"], "ephemeral");
        let ms = a["messages"].as_array().expect("messages");
        assert_eq!(ms.len(), 3, "the result and the next words are one user turn: {ms:?}");
        assert_eq!(ms[1]["content"][0]["type"], "tool_use");
        assert_eq!(ms[1]["content"][0]["input"]["path"], "a.rs");
        assert_eq!(ms[2]["content"][0]["type"], "tool_result");
        assert_eq!(ms[2]["content"][1]["text"], "and?");
        assert_eq!(a["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(a["thinking"]["budget_tokens"], 2048);
        assert_eq!(a["max_tokens"], 10240);
    }

    #[test]
    fn anthropics_stream_read() {
        let mut s = AnthropicStream::default();
        for e in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":90}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"grep"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"q\":"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"x\"}"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}"#,
            r#"{"type":"message_stop"}"#,
        ] {
            s.read(e).expect("read");
        }
        assert_eq!((s.input, s.cached, s.output), (100, 90, 7));
        assert_eq!(s.calls()[0].arguments, "{\"q\":\"x\"}");
        assert_eq!(s.finish, Some(crate::Finish::ToolCalls));
        assert!(s.ended);
        assert!(s.read(r#"{"type":"error","error":{"message":"Overloaded"}}"#).is_err());
    }
}
