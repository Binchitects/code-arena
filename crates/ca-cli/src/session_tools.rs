//! Tools that read the session's store: a result kept aside (`read_result`), this session's own messages
//! (`session_search`, `session_read`), and the folder's other sessions (`search_sessions`).

use std::sync::{Arc, Mutex};

use ca_llm::ToolDef;
use ca_session::Store;
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::{Value, json};

pub fn tools(store: Arc<Mutex<Store>>, session: String, folder: String) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadResult { store: store.clone(), session: session.clone() }),
        Arc::new(SessionSearch { store: store.clone(), session: session.clone(), folder: folder.clone(), all: false }),
        Arc::new(SessionRead { store: store.clone(), session }),
        Arc::new(SessionSearch { store, session: String::new(), folder, all: true }),
    ]
}

struct ReadResult {
    store: Arc<Mutex<Store>>,
    session: String,
}

#[async_trait::async_trait]
impl Tool for ReadResult {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "read_result".into(),
            description: "Reads a tool result kept aside (r1, r2…: a result too long for the window, or cleared from it): from a line, so many lines, or the lines that hold some text.".into(),
            parameters: json!({"type": "object", "properties": {
                "id": {"type": "string", "description": "The result's name: r1, r2…"},
                "from": {"type": "integer", "description": "The first line to give, from 1."},
                "lines": {"type": "integer", "description": "How many lines (200 when not given)."},
                "find": {"type": "string", "description": "Gives only the lines that hold this text, with their numbers."}
            }, "required": ["id"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        args.get("id").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    async fn run(&self, args: Value, _ctx: &Context) -> Output {
        let id = args.get("id").and_then(Value::as_str).unwrap_or_default();
        let text = match self.store.lock().map(|s| s.result(&self.session, id)) {
            Ok(Ok(Some(t))) => t,
            Ok(Ok(None)) => return Output::error(format!("No result {id} is kept in this session.")),
            _ => return Output::error("The session's store could not be read."),
        };
        let lines: Vec<&str> = text.lines().collect();
        if let Some(find) = args.get("find").and_then(Value::as_str) {
            let hits: Vec<String> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l.contains(find))
                .take(200)
                .map(|(i, l)| format!("{:>6}\t{l}", i + 1))
                .collect();
            return Output::ok(if hits.is_empty() {
                format!("No line of {id} holds {find:?}.")
            } else {
                hits.join("\n")
            });
        }
        let from = args.get("from").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
        let n = args.get("lines").and_then(Value::as_u64).unwrap_or(200).clamp(1, 2000) as usize;
        let end = (from - 1 + n).min(lines.len());
        if from > lines.len() {
            return Output::error(format!("{id} has {} lines.", lines.len()));
        }
        let mut out: String =
            lines[from - 1..end].iter().enumerate().map(|(i, l)| format!("{:>6}\t{l}\n", from + i)).collect();
        if end < lines.len() {
            out.push_str(&format!("… {} more lines: from {}.", lines.len() - end, end + 1));
        }
        Output::ok(out)
    }
}

/// `session_search` (this session's messages, whatever a compaction summarized) or `search_sessions` (the
/// folder's other sessions).
struct SessionSearch {
    store: Arc<Mutex<Store>>,
    session: String,
    folder: String,
    all: bool,
}

#[async_trait::async_trait]
impl Tool for SessionSearch {
    fn def(&self) -> ToolDef {
        if self.all {
            ToolDef {
                name: "search_sessions".into(),
                description:
                    "Finds what was said in this folder's earlier sessions: the messages that hold every word given."
                        .into(),
                parameters: json!({"type": "object", "properties": {"words": {"type": "string"}}, "required": ["words"]}),
            }
        } else {
            ToolDef {
                name: "session_search".into(),
                description: "Finds messages of this session that hold every word given, also those a compaction summarized: their numbers (#N) and a line of each. session_read reads them word for word.".into(),
                parameters: json!({"type": "object", "properties": {"words": {"type": "string"}}, "required": ["words"]}),
            }
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        args.get("words").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    async fn run(&self, args: Value, _ctx: &Context) -> Output {
        let words = args.get("words").and_then(Value::as_str).unwrap_or_default();
        let Ok(store) = self.store.lock() else { return Output::error("The session's store could not be read.") };
        let hits = match store.search(&self.folder, words, 50) {
            Ok(h) => h,
            Err(e) => return Output::error(e.to_string()),
        };
        let hits: Vec<String> = hits
            .into_iter()
            .filter(|(s, _, _)| if self.all { *s != self.session } else { *s == self.session })
            .take(20)
            .map(
                |(s, seq, text)| {
                    if self.all { format!("{s} #{}: {text}", seq + 1) } else { format!("#{}: {text}", seq + 1) }
                },
            )
            .collect();
        Output::ok(if hits.is_empty() { format!("Nothing says {words:?}.") } else { hits.join("\n") })
    }
}

struct SessionRead {
    store: Arc<Mutex<Store>>,
    session: String,
}

#[async_trait::async_trait]
impl Tool for SessionRead {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "session_read".into(),
            description:
                "Reads messages of this session word for word, by their numbers (#N from session_search; up to 20)."
                    .into(),
            parameters: json!({"type": "object", "properties": {"numbers": {"type": "array", "items": {"type": "integer"}}}, "required": ["numbers"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        args.get("numbers").map(Value::to_string).unwrap_or_default()
    }
    async fn run(&self, args: Value, _ctx: &Context) -> Output {
        let numbers: Vec<i64> = args
            .get("numbers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .take(20)
            .collect();
        let Ok(store) = self.store.lock() else { return Output::error("The session's store could not be read.") };
        let total = store.summary(&self.session).map(|s| s.turns).unwrap_or(0);
        let page = match store.page(&self.session, total.max(1), None) {
            Ok(p) => p,
            Err(e) => return Output::error(e.to_string()),
        };
        let mut out = Vec::new();
        for n in numbers {
            match page.messages.iter().find(|m| m.seq + 1 == n) {
                Some(m) => {
                    let role = serde_json::to_value(m.message.role)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default();
                    out.push(format!("#{n} ({role}):\n{}", m.message.content));
                }
                None => out.push(format!("#{n}: no such message")),
            }
        }
        Output::ok(out.join("\n\n"))
    }
}
