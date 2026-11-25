//! A turn: the person's message, then requests to the model and its tool calls until it answers.

use std::path::PathBuf;
use std::sync::Arc;

use ca_llm::{Finish, Gateway, LlmError, Message, Request, Sink, Thinking, ToolCall, ToolDef, Usage};
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::Value;

use crate::mend::{self, Mended};
use crate::policy::{Answer, Approver, Decision, Policy};

/// What the model reads of one tool result; the middle of a longer one is cut.
const RESULT_CHARS: usize = 16_000;
/// Times an answer cut at the output limit, or by a dropped stream, is carried on in one turn.
const CARRY_ON: usize = 8;

/// What a turn shows as it goes.
pub trait Events: Send {
    fn text(&mut self, _delta: &str) {}
    fn reasoning(&mut self, _delta: &str) {}
    fn notice(&mut self, _text: &str) {}
    /// A call about to run (after it was allowed).
    fn tool_start(&mut self, _name: &str, _subject: &str) {}
    /// A call ended: its name, its subject and what it gave (reading calls that ran at once end in order).
    fn tool_end(&mut self, _name: &str, _subject: &str, _output: &Output) {}
    /// The model's answer for this step is complete (before its tool calls run).
    fn step_end(&mut self) {}
}

struct EventSink<'a>(&'a mut dyn Events);

impl Sink for EventSink<'_> {
    fn text(&mut self, d: &str) {
        self.0.text(d);
    }
    fn reasoning(&mut self, d: &str) {
        self.0.reasoning(d);
    }
    fn notice(&mut self, t: &str) {
        self.0.notice(t);
    }
}

#[derive(Debug)]
pub enum TurnError {
    Llm(LlmError),
    /// It took more requests than a turn may.
    Steps(usize),
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TurnError::Llm(e) => write!(f, "{e}"),
            TurnError::Steps(n) => write!(f, "the turn stopped after {n} requests to the model (agent.maxSteps)"),
        }
    }
}

impl std::error::Error for TurnError {}

impl From<LlmError> for TurnError {
    fn from(e: LlmError) -> Self {
        TurnError::Llm(e)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TurnOutcome {
    /// The model's last words.
    pub answer: String,
    pub usage: Usage,
    /// Requests made to the model.
    pub steps: usize,
    pub tool_calls: usize,
    /// The files the turn's tools wrote.
    pub wrote: Vec<PathBuf>,
}

pub struct Agent {
    pub gateway: Gateway,
    pub model: String,
    pub thinking: Thinking,
    pub max_tokens: Option<u32>,
    pub tools: Vec<Arc<dyn Tool>>,
    pub ctx: Context,
    pub policy: Policy,
    pub approver: Arc<dyn Approver>,
    pub max_steps: usize,
    pub system: String,
    pub history: Vec<Message>,
}

impl Agent {
    fn defs(&self) -> Vec<ToolDef> {
        self.tools.iter().map(|t| t.def()).collect()
    }

    fn schemas(&self) -> Vec<(String, Value)> {
        self.tools.iter().map(|t| t.def()).map(|d| (d.name, d.parameters)).collect()
    }

    fn tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.def().name == name).cloned()
    }

    pub async fn turn(&mut self, text: &str, events: &mut dyn Events) -> Result<TurnOutcome, TurnError> {
        self.history.push(Message::user(text));
        let mut out = TurnOutcome::default();
        let mut carried = 0;
        let defs = self.defs();
        loop {
            if out.steps >= self.max_steps {
                return Err(TurnError::Steps(out.steps));
            }
            let mut messages = Vec::with_capacity(self.history.len() + 1);
            messages.push(Message::system(self.system.clone()));
            messages.extend(self.history.iter().cloned());
            let request = Request {
                model: self.model.clone(),
                messages,
                tools: defs.clone(),
                thinking: self.thinking.clone(),
                max_tokens: self.max_tokens,
                tags: vec![],
            };
            let c = self.gateway.complete(&request, &mut EventSink(events)).await?;
            out.steps += 1;
            out.usage.add(c.usage);
            events.step_end();
            let calls = c.message.tool_calls.clone();
            let finish = c.finish;
            let words = c.message.content.clone();
            self.history.push(c.message);
            if !calls.is_empty() {
                out.tool_calls += calls.len();
                self.run_calls(&calls, events, &mut out).await;
                continue;
            }
            if matches!(finish, Finish::Length | Finish::Dropped) && carried < CARRY_ON {
                carried += 1;
                // Code Arena's own note: the answer goes on where it stopped, without a word to the person.
                self.history.push(Message::user(
                    "(Your answer was cut off. Carry on exactly where it stopped, without repeating what you said.)",
                ));
                out.answer.push_str(&words);
                continue;
            }
            out.answer.push_str(&words);
            return Ok(out);
        }
    }

    /// The calls of one answer: those that only read run at once; one that changes something runs after the calls
    /// before it, in the order given. Each result goes into the history in the order of the calls.
    async fn run_calls(&mut self, calls: &[ToolCall], events: &mut dyn Events, out: &mut TurnOutcome) {
        let schemas = self.schemas();
        let mut i = 0;
        while i < calls.len() {
            // A run of reading calls, together.
            let mut reads: Vec<(usize, Mended, Arc<dyn Tool>)> = Vec::new();
            let mut j = i;
            while j < calls.len() {
                match mend::mend(&calls[j].name, &calls[j].arguments, &schemas) {
                    Ok(m) => match self.tool(&m.name) {
                        Some(t) if t.kind() == Kind::Read => reads.push((j, m, t)),
                        _ => break,
                    },
                    Err(_) => break,
                }
                j += 1;
            }
            if !reads.is_empty() {
                let mut ready = Vec::new();
                for (idx, m, t) in reads {
                    let subject = t.subject(&m.args);
                    match self.policy.decide(&m.name, Kind::Read, &subject) {
                        Decision::Refuse(why) => self.push_result(&calls[idx], Output::error(why), None),
                        _ => {
                            events.tool_start(&m.name, &subject);
                            ready.push((idx, m, t));
                        }
                    }
                }
                let ctx = self.ctx.clone();
                let runs = ready.iter().map(|(_, m, t)| {
                    let ctx = ctx.clone();
                    let t = t.clone();
                    let args = m.args.clone();
                    async move { t.run(args, &ctx).await }
                });
                let results = futures_util::future::join_all(runs).await;
                for ((idx, m, t), o) in ready.into_iter().zip(results) {
                    events.tool_end(&m.name, &t.subject(&m.args), &o);
                    self.push_result(&calls[idx], o, mend::note(&m));
                }
                i = j;
                continue;
            }
            // One call that changes something, or that cannot run as given.
            let call = &calls[i];
            match mend::mend(&call.name, &call.arguments, &schemas) {
                Err(why) => self.push_result(call, Output::error(why.0), None),
                Ok(m) => {
                    let Some(tool) = self.tool(&m.name) else {
                        self.push_result(call, Output::error(format!("There is no tool named {}.", m.name)), None);
                        i += 1;
                        continue;
                    };
                    let kind = tool.kind();
                    let subject = tool.subject(&m.args);
                    let allowed = match self.policy.decide(&m.name, kind, &subject) {
                        Decision::Run => Ok(()),
                        Decision::Refuse(why) => Err(why),
                        Decision::Ask => match self.approver.ask(&m.name, &subject).await {
                            Some(Answer::Yes) => Ok(()),
                            Some(Answer::Always) => {
                                self.policy.remember(&m.name, kind, &subject);
                                Ok(())
                            }
                            Some(Answer::No) => Err("The person said no to this call: ask what they would like instead, or find another way.".into()),
                            None => Err(format!(
                                "This run cannot ask, and {} asks in {} mode: it was not run. Say what you would have done; the person can run again with --mode auto-edit (edits) or --mode yolo (commands too).",
                                m.name,
                                self.policy.mode.name()
                            )),
                        },
                    };
                    match allowed {
                        Err(why) => self.push_result(call, Output::error(why), None),
                        Ok(()) => {
                            events.tool_start(&m.name, &subject);
                            let o = tool.run(m.args.clone(), &self.ctx).await;
                            events.tool_end(&m.name, &subject, &o);
                            for w in &o.wrote {
                                if !out.wrote.contains(w) {
                                    out.wrote.push(w.clone());
                                }
                            }
                            self.push_result(call, o, mend::note(&m));
                        }
                    }
                }
            }
            i += 1;
        }
    }

    fn push_result(&mut self, call: &ToolCall, o: Output, note: Option<String>) {
        let mut text = ca_tools::text::cut_middle(&o.text, RESULT_CHARS);
        if let Some(n) = note {
            text.push_str("\n\n");
            text.push_str(&n);
        }
        self.history.push(Message::tool(call.id.clone(), text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{Mode, NoAsking, Rules};
    use ca_core::Secret;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use wiremock::{Mock, MockServer, Request as WireRequest, Respond, ResponseTemplate};

    /// A model that answers a script, one answer per request, and keeps what it was sent.
    struct Script {
        answers: Vec<String>,
        at: AtomicUsize,
        seen: Arc<std::sync::Mutex<Vec<Value>>>,
    }

    impl Respond for Script {
        fn respond(&self, req: &WireRequest) -> ResponseTemplate {
            self.seen.lock().expect("lock").push(serde_json::from_slice(&req.body).expect("json"));
            let i = self.at.fetch_add(1, Ordering::SeqCst);
            let body = self.answers.get(i).cloned().unwrap_or_else(|| self.answers.last().cloned().unwrap_or_default());
            ResponseTemplate::new(200).set_body_string(body)
        }
    }

    fn call(name: &str, args: &str) -> String {
        let event = serde_json::json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": format!("c-{name}"), "function": {"name": name, "arguments": args}}]}, "finish_reason": "tool_calls"}]});
        format!("data: {event}\n\ndata: [DONE]\n\n")
    }

    fn say(text: &str, finish: &str) -> String {
        let event = serde_json::json!({"choices": [{"delta": {"content": text}, "finish_reason": finish}], "usage": {"prompt_tokens": 10, "completion_tokens": 2}});
        format!("data: {event}\n\ndata: [DONE]\n\n")
    }

    async fn agent(
        answers: Vec<String>,
        mode: Mode,
    ) -> (MockServer, Agent, tempfile::TempDir, Arc<std::sync::Mutex<Vec<Value>>>) {
        let server = MockServer::start().await;
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        Mock::given(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(Script { answers, at: AtomicUsize::new(0), seen: seen.clone() })
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().expect("temp");
        std::fs::write(dir.path().join("notes.txt"), "the answer is 42\n").expect("write");
        let ws = ca_workspace::Workspace::open(dir.path(), &[]).expect("ws");
        let a = Agent {
            gateway: Gateway::new(&server.uri(), Secret::new("sk-test"), None).expect("gw"),
            model: "m".into(),
            thinking: Thinking::Default,
            max_tokens: None,
            tools: ca_tools::builtin(),
            ctx: Context { workspace: Arc::new(ws), shell: None, shell_timeout: Duration::from_secs(30) },
            policy: Policy::new(mode, Rules::default()),
            approver: Arc::new(NoAsking),
            max_steps: 20,
            system: "sys".into(),
            history: vec![],
        };
        (server, a, dir, seen)
    }

    struct Quiet;
    impl Events for Quiet {}

    #[tokio::test]
    async fn reads_then_answers() {
        let (_s, mut a, _d, seen) =
            agent(vec![call("Read", r#"{"file_path": "notes.txt"}"#), say("It is 42.", "stop")], Mode::Ask).await;
        let out = a.turn("what is the answer?", &mut Quiet).await.expect("turn");
        assert_eq!(out.answer, "It is 42.");
        assert_eq!((out.steps, out.tool_calls), (2, 1));
        let second = &seen.lock().expect("lock")[1];
        let tool_msg = &second["messages"][3];
        assert_eq!(tool_msg["role"], "tool");
        let content = tool_msg["content"].as_str().expect("text");
        assert!(content.contains("the answer is 42"), "{content}");
        assert!(content.contains("Read → read_file"), "{content}");
        assert_eq!(second["messages"][0]["content"], "sys");
    }

    #[tokio::test]
    async fn a_run_that_cannot_ask_refuses_an_edit_in_ask_mode() {
        let (_s, mut a, d, seen) = agent(
            vec![call("write_file", r#"{"path": "x.txt", "content": "hi"}"#), say("I could not.", "stop")],
            Mode::Ask,
        )
        .await;
        let out = a.turn("write x", &mut Quiet).await.expect("turn");
        assert!(out.wrote.is_empty());
        assert!(!d.path().join("x.txt").exists());
        let msgs = &seen.lock().expect("lock")[1]["messages"];
        assert!(msgs[3]["content"].as_str().expect("t").contains("This run cannot ask"));
    }

    #[tokio::test]
    async fn auto_edit_writes() {
        let (_s, mut a, d, _) = agent(
            vec![call("write_file", r#"{"path": "x.txt", "content": "hi"}"#), say("Done.", "stop")],
            Mode::AutoEdit,
        )
        .await;
        let out = a.turn("write x", &mut Quiet).await.expect("turn");
        assert_eq!(std::fs::read_to_string(d.path().join("x.txt")).expect("read"), "hi");
        assert_eq!(out.wrote.len(), 1);
    }

    #[tokio::test]
    async fn an_answer_cut_at_the_limit_is_carried_on() {
        let (_s, mut a, _d, seen) = agent(vec![say("Half ", "length"), say("and the rest.", "stop")], Mode::Ask).await;
        let out = a.turn("tell me", &mut Quiet).await.expect("turn");
        assert_eq!(out.answer, "Half and the rest.");
        let msgs = &seen.lock().expect("lock")[1]["messages"];
        assert!(msgs[3]["content"].as_str().expect("t").starts_with("(Your answer was cut off."));
    }

    #[tokio::test]
    async fn a_call_missing_its_arguments_is_explained_to_the_model() {
        let (_s, mut a, _d, seen) =
            agent(vec![call("edit_file", r#"{"path": "notes.txt"}"#), say("Sorry.", "stop")], Mode::Yolo).await;
        a.turn("edit", &mut Quiet).await.expect("turn");
        let msgs = &seen.lock().expect("lock")[1]["messages"];
        assert!(msgs[3]["content"].as_str().expect("t").starts_with("edit_file needs old_string and new_string."));
    }

    #[tokio::test]
    async fn the_step_limit_ends_a_runaway_turn() {
        let (_s, mut a, _d, _) = agent(vec![call("list_dir", "{}")], Mode::Ask).await;
        a.max_steps = 3;
        assert!(matches!(a.turn("loop", &mut Quiet).await, Err(TurnError::Steps(3))));
    }
}
