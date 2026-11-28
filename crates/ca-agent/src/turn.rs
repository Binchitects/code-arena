//! A turn: the person's message, then requests to the model and its tool calls until it answers.

use std::path::PathBuf;
use std::sync::Arc;

use ca_llm::{Finish, Gateway, LlmError, Message, Request, Sink, Thinking, ToolCall, ToolDef, Usage};
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::Value;

use crate::context::{self, Window};
use crate::mend::{self, Mended};
use crate::policy::{self, Answer, Approver, Decision, Policy, Question};

/// What the model reads of one tool result; the middle of a longer one is cut.
const RESULT_CHARS: usize = 16_000;
/// Times an answer cut at the output limit, or by a dropped stream, is carried on in one turn.
const CARRY_ON: usize = 8;

/// What the model is told of a call the person said no to (a session read back knows such a call by it).
pub const SAID_NO: &str = "The person said no to this call";

/// A call of a tool, as a turn's events say it: its number as the model gave it, its name, what it is about (a
/// path, a command) and what it runs with (null: arguments that could not be read).
#[derive(Debug, Clone, Copy)]
pub struct Call<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub subject: &'a str,
    pub arguments: &'a Value,
}

/// What a turn shows as it goes.
pub trait Events: Send {
    fn text(&mut self, _delta: &str) {}
    fn reasoning(&mut self, _delta: &str) {}
    fn notice(&mut self, _text: &str) {}
    /// A call that asks the person before it runs: the question follows, then `tool_start` or `tool_not_run`.
    fn tool_waiting(&mut self, _call: &Call<'_>) {}
    /// A call about to run (after it was allowed).
    fn tool_start(&mut self, _call: &Call<'_>) {}
    /// A call ended: what it gave and how long it ran (reading calls that ran at once end in order).
    fn tool_end(&mut self, _call: &Call<'_>, _output: &Output, _took: std::time::Duration) {}
    /// A call that did not run, and what the model is told of it: `denied` when the person, the mode or a hook
    /// said no; else it could not run as given (no tool of that name, arguments that cannot be read).
    fn tool_not_run(&mut self, _call: &Call<'_>, _why: &str, _denied: bool) {}
    /// How far a running call has got, where it says: so much done, of how much when it knows, and its words.
    fn tool_progress(&mut self, _id: &str, _progress: f64, _total: Option<f64>, _message: &str) {}
    /// The model's answer for this step is complete (before its tool calls run).
    fn step_end(&mut self) {}
    /// How full the window is after this step, said just before `step_end`: the request as the model counted it
    /// with its answer (Code Arena's estimate when the model said nothing), the window's size, and where it
    /// compacts (a share of the window; none: only when a request would not fit).
    fn step_window(&mut self, _used: u64, _size: u64, _compact_at: Option<u64>) {}
}

/// How often the answer being written is kept as a draft (the session's store; the web chat).
const DRAFT_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// How many symbols a plan's impact looks up.
const MOST_GROUNDED: usize = 8;

/// What a second reading answers when the plan holds as it is.
const PLAN_HOLDS: &str = "The plan holds.";

/// What `change_impact` said of a symbol, in a line: which lines use it, where, who depends on the repository, which
/// tests name it.
fn impact_line(symbol: &str, found: &Value) -> String {
    if let Some(e) = found["error"].as_str() {
        return format!("{symbol}: {e}");
    }
    let uses: Vec<&Value> =
        found["references"].as_array().into_iter().flatten().filter(|r| r["is_definition"] != true).collect();
    let mut repos: Vec<(String, usize)> = Vec::new();
    for r in &uses {
        let name = r["repo"].as_str().unwrap_or("").to_string();
        match repos.iter_mut().find(|(n, _)| *n == name) {
            Some((_, c)) => *c += 1,
            None => repos.push((name, 1)),
        }
    }
    let by: Vec<String> = repos.iter().take(4).map(|(n, c)| format!("{n} ({c})")).collect();
    let mut tests: Vec<&str> = uses
        .iter()
        .filter_map(|r| r["path"].as_str())
        .filter(|p| p.to_ascii_lowercase().contains("test") || p.to_ascii_lowercase().contains("spec"))
        .collect();
    tests.dedup();
    let dependents = found["dependents"].as_u64().unwrap_or(0);
    let mut t = if uses.is_empty() {
        format!("{symbol}: no line found that uses it")
    } else {
        format!(
            "{symbol}: {}{} lines use it, in {}",
            uses.len(),
            if found["references_truncated"] == true { "+" } else { "" },
            by.join(", ")
        )
    };
    t.push_str(&match dependents {
        0 => "; no other repository depends on this one".to_string(),
        1 => "; 1 repository depends on this one".to_string(),
        n => format!("; {n} repositories depend on this one"),
    });
    if tests.is_empty() {
        t.push_str("; no test names it.");
    } else {
        t.push_str(&format!("; tests that name it: {}.", tests.into_iter().take(5).collect::<Vec<_>>().join(", ")));
    }
    t
}

/// "Skip thinking" for the turn running (Tab in the terminal, the web's "Answer now"): the thinking stops and the model
/// answers at once, and the rest of the turn does not think.
#[derive(Debug, Default)]
pub struct Skip {
    asked: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl Skip {
    /// Asked for the turn running; `false` when it was asked already.
    pub fn ask(&self) -> bool {
        let first = !self.asked.swap(true, std::sync::atomic::Ordering::SeqCst);
        if first {
            self.notify.notify_one();
        }
        first
    }

    pub fn asked(&self) -> bool {
        self.asked.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn reset(&self) {
        self.asked.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The stream's words to the person, and every two seconds to the recorder as a draft.
struct EventSink<'a> {
    events: &'a mut dyn Events,
    recorder: Option<&'a mut Box<dyn Recorder>>,
    text: String,
    reasoning: String,
    kept: std::time::Instant,
    /// The answer's words began (a skip then does not cut it).
    said: Arc<std::sync::atomic::AtomicBool>,
}

impl EventSink<'_> {
    fn keep(&mut self) {
        if self.kept.elapsed() < DRAFT_EVERY {
            return;
        }
        self.kept = std::time::Instant::now();
        if let Some(r) = self.recorder.as_mut() {
            r.draft(&self.text, &self.reasoning);
        }
    }
}

impl Sink for EventSink<'_> {
    fn text(&mut self, d: &str) {
        if !d.trim().is_empty() {
            self.said.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.events.text(d);
        self.text.push_str(d);
        self.keep();
    }
    fn reasoning(&mut self, d: &str) {
        self.events.reasoning(d);
        self.reasoning.push_str(d);
        self.keep();
    }
    fn notice(&mut self, t: &str) {
        self.events.notice(t);
    }
}

#[derive(Debug)]
pub enum TurnError {
    Llm(LlmError),
    /// It took more requests than a turn may.
    Steps(usize),
    /// A hook blocked the person's message (`UserPromptSubmit`), or stopped the session.
    Blocked(String),
    /// The model's call was cut off at its answer's limit twice in the same place.
    Cut(String),
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TurnError::Llm(e) => write!(f, "{e}"),
            TurnError::Steps(n) => write!(f, "the turn stopped after {n} requests to the model (agent.maxSteps)"),
            TurnError::Blocked(why) => write!(f, "{why}"),
            TurnError::Cut(name) => write!(
                f,
                "the model's {name} call was cut off at the limit of one answer twice in the same place: the turn ends here (ask for the work in smaller parts)"
            ),
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
    /// Tools listed, not sent whole: the model loads them with `load_tools` (or by calling one).
    pub deferred: Vec<Arc<dyn Tool>>,
    /// Servers that connect after the session began send their tools here.
    pub joins: Option<tokio::sync::mpsc::UnboundedReceiver<Joined>>,
    pub ctx: Context,
    pub policy: Policy,
    pub approver: Arc<dyn Approver>,
    pub max_steps: usize,
    pub system: String,
    pub history: Vec<Message>,
    /// Where each message goes as it is added (the session's store); none for a run that keeps nothing.
    pub recorder: Option<Box<dyn Recorder>>,
    /// Messages the person sent while the turn worked: read at its next step, as theirs.
    pub inbox: Option<Arc<std::sync::Mutex<Vec<String>>>>,
    /// The model's window, and when the session compacts.
    pub window: Window,
    /// Where results too big for the window are kept, to be read on with `read_result`.
    pub aside: Option<Box<dyn Aside>>,
    /// Which history entries are Code Arena's own notes (read by the model, not kept in the session).
    pub notes: Vec<bool>,
    /// `chat:<id>`, `project:<id>`: where Arena's usage log counts the requests.
    pub tags: Vec<String>,
    /// The person's "skip thinking" for the turn running.
    pub skip: Arc<Skip>,
    /// A proposed plan is read again with fresh eyes before the person's review (`plan.secondReading`).
    pub second_reading: bool,
    /// The repository's key (`host/group/name`), for Farzan Graph's impact of a plan's symbols.
    pub repo_key: Option<String>,
    /// The person's hooks (and a project's they trusted).
    pub hooks: Arc<crate::hooks::Hooks>,
}

/// Keeps a result aside and names it (`r1`), for `read_result`.
pub trait Aside: Send {
    fn keep(&mut self, text: &str) -> Option<String>;
}

/// Tools that joined: a server connected.
pub struct Joined {
    pub tools: Vec<Arc<dyn Tool>>,
    pub deferred: Vec<Arc<dyn Tool>>,
    /// Code Arena's note for the model: the server's instructions, the tools it may load.
    pub note: Option<String>,
}

const LOAD_TOOLS: &str = "load_tools";

fn load_tools_def() -> ToolDef {
    ToolDef {
        name: LOAD_TOOLS.into(),
        description: "Loads tools listed in Code Arena's note (not sent whole, to keep the requests small), so they can be called: give their names.".into(),
        parameters: serde_json::json!({"type": "object", "properties": {"names": {"type": "array", "items": {"type": "string"}}}, "required": ["names"]}),
    }
}

/// Keeps each message as it is added, before the turn goes on.
pub trait Recorder: Send {
    /// `begins_turn`: the person's own message, which starts a turn. Gives its place in the session.
    fn record(&mut self, message: &Message, begins_turn: bool) -> Option<i64>;
    /// A compaction: the model reads `summary` in place of every kept message but the last `kept`.
    fn compacted(&mut self, _summary: &str, _kept: usize) {}
    /// The answer being written, as it stands (every two seconds while it comes); its message replaces it.
    fn draft(&mut self, _text: &str, _reasoning: &str) {}
}

impl Agent {
    fn defs(&self) -> Vec<ToolDef> {
        let mut d: Vec<ToolDef> = self.tools.iter().map(|t| t.def()).collect();
        if !self.deferred.is_empty() {
            d.push(load_tools_def());
        }
        d
    }

    /// Every tool a call may name: those sent, those listed (loaded when called), and `load_tools`.
    fn schemas(&self) -> Vec<(String, Value)> {
        let mut s: Vec<(String, Value)> =
            self.tools.iter().chain(self.deferred.iter()).map(|t| t.def()).map(|d| (d.name, d.parameters)).collect();
        if !self.deferred.is_empty() {
            let l = load_tools_def();
            s.push((l.name, l.parameters));
        }
        s
    }

    /// A listed tool called by name is loaded then; it stays loaded, so the requests' start stays the same.
    fn load(&mut self, name: &str) -> bool {
        match self.deferred.iter().position(|t| t.def().name == name) {
            Some(i) => {
                let t = self.deferred.remove(i);
                self.tools.push(t);
                true
            }
            None => false,
        }
    }

    /// Takes in the tools of servers that connected since the last step.
    fn take_joins(&mut self, events: &mut dyn Events) {
        let Some(rx) = self.joins.as_mut() else { return };
        let mut joined = Vec::new();
        while let Ok(j) = rx.try_recv() {
            joined.push(j);
        }
        for j in joined {
            if !j.tools.is_empty() || !j.deferred.is_empty() {
                events.notice(&format!("{} tools joined.", j.tools.len() + j.deferred.len()));
            }
            self.tools.extend(j.tools);
            self.deferred.extend(j.deferred);
            if let Some(n) = j.note {
                // A note of Code Arena's: the model reads it; the session does not keep it (it is said again next time).
                self.note(n);
            }
        }
    }

    fn tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.def().name == name).cloned()
    }

    pub async fn turn(&mut self, text: &str, events: &mut dyn Events) -> Result<TurnOutcome, TurnError> {
        let said = self.hooks.run("UserPromptSubmit", "", serde_json::json!({"prompt": text})).await;
        for n in &said.notes {
            events.notice(n);
        }
        if let Some(why) = said.block.or(said.stop) {
            return Err(TurnError::Blocked(format!("A hook blocked the message: {why}")));
        }
        let text = crate::mentions::attach(text, &self.ctx.workspace);
        let seq = self.add(Message::user(text), true).unwrap_or(self.history.len() as i64);
        for c in said.context {
            self.note(format!("(Code Arena: a hook of the person's says: {c})"));
        }
        let mut stops_blocked = 0;
        // The turn's checkpoint: what each file holds before the turn first writes it.
        self.ctx.checkpoints.begin(seq);
        if let Some(j) = &self.ctx.jobs {
            j.begin_turn(seq);
        }
        let mut out = TurnOutcome::default();
        let mut carried = 0;
        let mut overflowed = false;
        let mut shared_waited = false;
        // The last call cut off at the answer's limit: its tool and how its arguments began.
        let mut last_cut: Option<(String, String)> = None;
        self.skip.reset();
        let plan_at_start = self.ctx.plan.get();
        loop {
            self.take_joins(events);
            // A plan made in plan mode is proposed, for the person's review.
            self.ctx.planning = self.policy.mode == crate::policy::Mode::Plan;
            if out.steps > 0 {
                let waiting: Vec<String> = match &self.inbox {
                    Some(i) => i.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default(),
                    None => vec![],
                };
                for text in waiting {
                    events.notice(&format!("› {text}  (sent while it worked)"));
                    self.add(Message::user(text), false);
                }
            }
            if out.steps >= self.max_steps {
                return Err(TurnError::Steps(out.steps));
            }
            self.condense_used_results();
            self.make_room(events, false).await;
            let defs = self.defs();
            let mut request = self.request(defs.clone(), None);
            // Skipped: this turn thinks no more.
            if self.skip.asked() {
                request.thinking = Thinking::Off;
            }
            let chars = context::request_chars(&self.system, &self.history, &defs);
            let said = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let answered = {
                let mut sink = EventSink {
                    events: &mut *events,
                    recorder: self.recorder.as_mut(),
                    text: String::new(),
                    reasoning: String::new(),
                    kept: std::time::Instant::now(),
                    said: said.clone(),
                };
                let skip = self.skip.clone();
                let thinking = request.thinking != Thinking::Off;
                let asking = self.gateway.complete(&request, &mut sink);
                tokio::pin!(asking);
                loop {
                    tokio::select! {
                        r = &mut asking => break Some(r),
                        _ = skip.notify.notified(), if thinking => {
                            // Still only thinking: cut, and asked again at once without it.
                            if !said.load(std::sync::atomic::Ordering::SeqCst) {
                                break None;
                            }
                        }
                    }
                }
            };
            let answered = match answered {
                Some(r) => r,
                None => {
                    events.notice("Thinking skipped: the model answers now.");
                    request.thinking = Thinking::Off;
                    let mut sink = EventSink {
                        events: &mut *events,
                        recorder: self.recorder.as_mut(),
                        text: String::new(),
                        reasoning: String::new(),
                        kept: std::time::Instant::now(),
                        said,
                    };
                    self.gateway.complete(&request, &mut sink).await
                }
            };
            let c = match answered {
                // llama.cpp's cache, shared by the requests it runs at once, was full (others filled it): asked
                // again after two seconds as it is, compacted only if refused again.
                Err(LlmError::ContextOverflow { overflow, .. }) if overflow.shared_cache && !shared_waited => {
                    shared_waited = true;
                    events.notice(
                        "The engine's cache, shared by the requests it runs, was full: asking again in two seconds.",
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    continue;
                }
                Err(LlmError::ContextOverflow { message, overflow }) if !overflowed => {
                    // The engine said how much it holds: that is the window from now on.
                    if let Some(n) = overflow.context
                        && n < self.window.size
                    {
                        let was = self.window.size;
                        self.window.engine_holds(n);
                        if self.window.size == n {
                            events.notice(&format!(
                                "The engine holds {} tokens, not the {} the gateway says: Code Arena compacts for {} from now on.",
                                context::short_tokens(n),
                                context::short_tokens(was),
                                context::short_tokens(n)
                            ));
                        }
                    }
                    // The window refused it: compacted at once and asked again, once.
                    overflowed = true;
                    events
                        .notice(&format!("The window refused the request ({message}): compacting, then asking again."));
                    self.make_room(events, true).await;
                    continue;
                }
                r => r?,
            };
            self.window.learn(chars, c.usage.prompt);
            out.steps += 1;
            out.usage.add(c.usage);
            let used = match c.usage.prompt {
                0 => self.window.tokens(chars),
                n => n,
            } + c.usage.completion;
            events.step_window(used, self.window.size, self.window.compact_at);
            events.step_end();
            let calls = c.message.tool_calls.clone();
            let finish = c.finish;
            let words = c.message.content.clone();
            self.add(c.message, false);
            if !calls.is_empty() {
                out.tool_calls += calls.len();
                // Cut at the answer's limit: the last call is not whole (mended, it would write half a file). It does
                // not run; the model does it in parts. Cut again the same way, the turn ends.
                if matches!(finish, Finish::Length | Finish::Dropped) {
                    let (whole, cut) = calls.split_at(calls.len() - 1);
                    let cut = &cut[0];
                    let key = (cut.name.clone(), cut.arguments.chars().take(400).collect::<String>());
                    if !whole.is_empty() {
                        self.run_calls(whole, events, &mut out).await;
                    }
                    if last_cut.as_ref() == Some(&key) {
                        self.add(
                            Message::tool(cut.id.clone(), "Cut off again in the same place: it did not run."),
                            false,
                        );
                        return Err(TurnError::Cut(cut.name.clone()));
                    }
                    last_cut = Some(key);
                    let n = cut.arguments.chars().count();
                    events.notice(&format!(
                        "The model's {} call was cut off at the limit of one answer ({n} characters): it did not run, and the model does it in parts.",
                        cut.name
                    ));
                    self.add(
                        Message::tool(
                            cut.id.clone(),
                            format!(
                                "This call was cut off after {n} characters of arguments, at the limit of one answer: it did not run. Do it in smaller parts, each well within one answer: for a long file, write_file its first part, then write_file with append: true for each part after; for a long command, write it to a script file first."
                            ),
                        ),
                        false,
                    );
                    continue;
                }
                self.run_calls(&calls, events, &mut out).await;
                continue;
            }
            if matches!(finish, Finish::Length | Finish::Dropped) && carried < CARRY_ON {
                carried += 1;
                // Code Arena's own note: the answer goes on where it stopped, without a word to the person.
                self.note(
                    "(Your answer was cut off. Carry on exactly where it stopped, without repeating what you said.)"
                        .into(),
                );
                out.answer.push_str(&words);
                continue;
            }
            out.answer.push_str(&words);
            // A plan proposed in this turn: grounded in Farzan Graph, and read again before the person's review.
            if self.read_plan_again(&plan_at_start, events).await {
                continue;
            }
            // A Stop hook may send the model on (three times at most a turn).
            if self.hooks.has("Stop") {
                let said = self
                    .hooks
                    .run("Stop", "", serde_json::json!({"stop_hook_active": stops_blocked > 0, "last_answer": words}))
                    .await;
                for n in &said.notes {
                    events.notice(n);
                }
                if let Some(why) = said.block
                    && stops_blocked < 3
                {
                    stops_blocked += 1;
                    events.notice(&format!("A Stop hook sends the model on: {why}"));
                    self.note(format!("(Code Arena: a hook of the person's says the work is not done: {why})"));
                    continue;
                }
            }
            return Ok(out);
        }
    }

    /// A plan the turn proposed: what each step's symbols reach (Farzan Graph's `change_impact`), and a second reading
    /// with fresh eyes. `true` when the reading has notes: the model hears them and changes the plan before it ends.
    async fn read_plan_again(&mut self, before: &ca_tools::plan::Plan, events: &mut dyn Events) -> bool {
        let mut plan = self.ctx.plan.get();
        if plan.review.as_deref() != Some(ca_tools::plan::PROPOSED)
            || plan.is_empty()
            || plan.critique.is_some()
            || &plan == before
        {
            return false;
        }
        if let (Some(tool), Some(key)) =
            (self.tools.iter().find(|t| t.def().name.ends_with("change_impact")).cloned(), self.repo_key.clone())
            && let Some((_, repo)) = key.split_once('/')
        {
            let mut looked = 0;
            for step in plan.steps.iter_mut() {
                if step.symbols.is_empty() || step.impact.is_some() || looked >= MOST_GROUNDED {
                    continue;
                }
                let mut said = Vec::new();
                for symbol in step.symbols.iter().take(MOST_GROUNDED - looked) {
                    looked += 1;
                    let o = tool.run(serde_json::json!({"repo": repo, "symbol": symbol, "depth": 2}), &self.ctx).await;
                    if !o.is_error
                        && let Ok(found) = serde_json::from_str::<Value>(&o.text)
                    {
                        said.push(impact_line(symbol, &found));
                    }
                }
                if !said.is_empty() {
                    step.impact = Some(said.join("\n"));
                }
            }
        }
        let mut notes = String::new();
        if self.second_reading {
            events.notice("Reading the plan again with fresh eyes before your review.");
            let asked =
                (0..self.history.len()).rev().find(|&i| self.is_persons(i)).map(|i| self.history[i].content.clone());
            let ask = Request {
                model: self.model.clone(),
                messages: vec![
                    Message::system(
                        "You read a plan for a coding task with fresh eyes, before it is carried out. You change nothing.",
                    ),
                    Message::user(format!(
                        "{}{}\n\nAnswer with short notes only, a line each: what is missing, what is risky, which step has no check or a check that would not show it is done, what is in the wrong order. If the plan holds as it is, answer exactly: {PLAN_HOLDS}",
                        asked
                            .map(|a| format!(
                                "What the person asked:\n{}\n\n",
                                a.chars().take(2000).collect::<String>()
                            ))
                            .unwrap_or_default(),
                        plan.text()
                    )),
                ],
                max_tokens: Some(1500),
                tags: self.tags.clone(),
                ..Default::default()
            };
            if let Ok(c) = self.gateway.complete(&ask, &mut ca_llm::Quiet).await {
                let t = c.message.content.trim().to_string();
                if !t.is_empty() && !t.to_ascii_lowercase().starts_with(&PLAN_HOLDS.to_ascii_lowercase()) {
                    notes = t.chars().take(4000).collect();
                }
            }
        }
        plan.critique = Some(notes.clone());
        self.ctx.plan.set(plan);
        if notes.is_empty() {
            return false;
        }
        events.notice("The second reading has notes: the model looks at them.");
        self.note(format!(
            "(Code Arena: a second reading of your plan, with fresh eyes, says:\n{notes}\n\nChange the plan where they hold (the plan tool), then end your answer as before.)"
        ));
        true
    }

    fn request(&self, tools: Vec<ToolDef>, tool_choice: Option<&str>) -> Request {
        let mut messages = Vec::with_capacity(self.history.len() + 1);
        messages.push(Message::system(self.system.clone()));
        messages.extend(self.history.iter().cloned());
        Request {
            model: self.model.clone(),
            messages,
            tools,
            thinking: self.thinking.clone(),
            max_tokens: self.max_tokens,
            tags: self.tags.clone(),
            tool_choice: tool_choice.map(str::to_string),
        }
    }

    /// About how many tokens the next request holds.
    pub fn context_used(&self) -> u64 {
        self.window.tokens(context::request_chars(&self.system, &self.history, &self.defs()))
    }

    /// What fills the next request, by kind, as text (for a tokenizer).
    pub fn context_texts(&self) -> [String; 6] {
        context::kinds_text(&self.system, &self.history, &self.notes, &self.defs())
    }

    /// What fills the next request, piece by piece.
    pub fn context_pieces(&self) -> Vec<context::Piece> {
        context::pieces(&self.system, &self.history, &self.notes, &self.defs())
    }

    /// What fills the next request, by kind.
    pub fn context_breakdown(&self) -> context::Breakdown {
        context::breakdown(&self.system, &self.history, &self.notes, &self.defs())
    }

    /// Results the model has used (two answers or more have followed them) go to it condensed from now on: their
    /// key lines and the handle that reads them whole. A pass that would free little waits, so the start of the
    /// requests the engine has cached does not change for a little.
    fn condense_used_results(&mut self) {
        let mut answers_after = 0;
        let mut candidates = Vec::new();
        for i in (0..self.history.len()).rev() {
            let m = &self.history[i];
            match m.role {
                ca_llm::Role::Assistant => answers_after += 1,
                ca_llm::Role::Tool
                    if answers_after >= 2 && m.content.len() > 3_000 && !m.content.starts_with("(Summarised") =>
                {
                    candidates.push(i)
                }
                _ => {}
            }
        }
        let freed: usize = candidates.iter().map(|&i| self.history[i].content.len()).sum();
        if freed < 6_000 {
            return;
        }
        for i in candidates {
            let full = std::mem::take(&mut self.history[i].content);
            let handle = self.aside.as_mut().and_then(|a| a.keep(&full));
            self.history[i].content = context::condensed(&full, handle.as_deref());
        }
    }

    /// Room in the window for the next request: earlier results cleared first, then the older part of the
    /// conversation summarized. `force`: the window refused a request, whatever the threshold says.
    async fn make_room(&mut self, events: &mut dyn Events, force: bool) {
        let used = self.context_used();
        if !force && used <= self.window.threshold() {
            return;
        }
        let freed = self.clear_old_results();
        if freed > 0 {
            events
                .notice(&format!("Cleared earlier tool results (~{} tokens) to make room.", self.window.tokens(freed)));
            if !force && self.context_used() <= self.window.threshold() {
                return;
            }
        }
        match self.compact(events, force).await {
            Ok(Some(_)) => {}
            Ok(None) => events.notice("Nothing older to summarize: the turn goes on as it is."),
            Err(e) => events.notice(&format!("The conversation could not be summarized ({e}): it goes on as it is.")),
        }
    }

    /// Results older than the person's last two messages (past 1,000 characters) are kept aside and replaced by a
    /// line saying how to read them again. Gives the characters freed.
    fn clear_old_results(&mut self) -> usize {
        let mut persons = 0;
        let mut cut_at = None;
        for i in (0..self.history.len()).rev() {
            if self.is_persons(i) {
                persons += 1;
                if persons == 2 {
                    cut_at = Some(i);
                    break;
                }
            }
        }
        let Some(cut) = cut_at else { return 0 };
        let mut freed = 0;
        for i in 0..cut {
            let m = &self.history[i];
            if m.role == ca_llm::Role::Tool && m.content.len() > 1_000 && !m.content.starts_with("(Cleared") {
                let full = std::mem::take(&mut self.history[i].content);
                let handle = self.aside.as_mut().and_then(|a| a.keep(&full));
                let line = match handle {
                    Some(h) => format!(
                        "(Cleared to make room in the window: this result ({} characters) is kept as {h}. read_result {{\"id\":\"{h}\"}} reads it again.)",
                        full.len()
                    ),
                    None => format!("(Cleared to make room in the window: this result had {} characters.)", full.len()),
                };
                freed += full.len().saturating_sub(line.len());
                self.history[i].content = line;
            }
        }
        freed
    }

    /// A message the person typed (not a tool's result, not one of Code Arena's notes).
    fn is_persons(&self, i: usize) -> bool {
        let m = &self.history[i];
        m.role == ca_llm::Role::User && !self.notes.get(i).copied().unwrap_or(false)
    }

    /// Summarizes the older part of the conversation, keeping the recent part whole (within the target share of
    /// the window, from a message of the person's). The session keeps every message; only what the model reads
    /// changes. Gives the summary, or `None` when there is nothing older to summarize.
    pub async fn compact(&mut self, events: &mut dyn Events, force: bool) -> Result<Option<String>, LlmError> {
        let budget = (self.window.size * self.window.target / 100) as usize;
        let budget_chars = (budget as f64 * self.window.chars_per_token) as usize;
        // From the end back, as far as the target allows, then on back to a message of the person's.
        let mut kept_from = self.history.len();
        let mut chars = 0;
        for i in (0..self.history.len()).rev() {
            chars += context::message_chars(&self.history[i]);
            if chars > budget_chars && kept_from < self.history.len() {
                break;
            }
            kept_from = i;
        }
        while kept_from < self.history.len() && !self.is_persons(kept_from) {
            kept_from += 1;
        }
        let last_persons = (0..self.history.len()).rev().find(|&i| self.is_persons(i)).unwrap_or(0);
        // The person's last message is past the target, or the engine refused what fits it: keep from that message.
        if kept_from >= self.history.len() || force {
            kept_from = last_persons;
        }
        // One long turn past the target on its own: its older steps are summarized, the person's message kept.
        let turn_chars: usize = self.history[last_persons..].iter().map(context::message_chars).sum();
        let only_notes_before = (0..last_persons).all(|i| self.notes.get(i).copied().unwrap_or(false));
        if kept_from == last_persons && turn_chars > budget_chars && (only_notes_before || force) {
            return self.compact_steps(events, last_persons, budget_chars).await;
        }
        if kept_from == 0 {
            return Ok(None);
        }
        let _ = self
            .hooks
            .run(
                "PreCompact",
                if force { "auto" } else { "manual" },
                serde_json::json!({"trigger": if force { "auto" } else { "manual" }}),
            )
            .await;
        events.notice("Compacting: the older part of the conversation is being summarized.");
        let older: Vec<Message> = self.history[..kept_from].to_vec();
        let mut ask = self.request(self.defs(), Some("none"));
        ask.messages.truncate(1);
        ask.messages.extend(older.iter().cloned());
        ask.messages.push(Message::user(context::SUMMARY_ASK));
        ask.max_tokens = Some(self.window.reserve.clamp(1024, 8192) as u32);
        let summary = match self.gateway.complete(&ask, &mut ca_llm::Quiet).await {
            Ok(c)
                if !c.message.content.trim().is_empty()
                    && !context::looped(&c.message.content)
                    && c.finish != Finish::Length =>
            {
                c.message.content
            }
            Ok(_) | Err(_) => fallback_summary(&older, &self.notes[..kept_from.min(self.notes.len())]),
        };
        let kept_recorded =
            (kept_from..self.history.len()).filter(|&i| !self.notes.get(i).copied().unwrap_or(false)).count();
        if let Some(r) = self.recorder.as_mut() {
            r.compacted(&summary, kept_recorded);
        }
        let kept: Vec<Message> = self.history.drain(kept_from..).collect();
        let kept_notes: Vec<bool> = if self.notes.len() > kept_from {
            self.notes.drain(kept_from..).collect()
        } else {
            vec![false; kept.len()]
        };
        self.history.clear();
        self.notes.clear();
        // The plan as it stands goes with the summary: a compaction never loses it.
        let plan = self.ctx.plan.get();
        let plan_text = if plan.is_empty() {
            String::new()
        } else {
            format!("\n\nYour working plan as it stands:\n{}", plan.text())
        };
        self.note(format!(
            "(Code Arena: the earlier part of this conversation is summarized here; every message is kept.)\n\n{summary}{plan_text}"
        ));
        self.history.extend(kept);
        self.notes.extend(kept_notes);
        events.notice(&format!("Compacted: the model now reads about {} tokens.", self.context_used()));
        Ok(Some(summary))
    }

    /// The steps of the turn running, after the person's message at `from`, summarized but the last ones (within
    /// `budget_chars`, from an answer of the model's, so no result is parted from its call). Only what the model reads
    /// changes; the session keeps every step.
    async fn compact_steps(
        &mut self,
        events: &mut dyn Events,
        from: usize,
        budget_chars: usize,
    ) -> Result<Option<String>, LlmError> {
        self.sync_notes();
        let mut keep_from = self.history.len();
        let mut chars = 0;
        for i in (from + 1..self.history.len()).rev() {
            chars += context::message_chars(&self.history[i]);
            if chars > budget_chars && keep_from < self.history.len() {
                break;
            }
            if self.history[i].role == ca_llm::Role::Assistant {
                keep_from = i;
            }
        }
        // The last answer alone is past the target: it stays whole all the same, the steps before it go.
        if keep_from >= self.history.len() {
            keep_from = (from + 1..self.history.len())
                .rev()
                .find(|&i| self.history[i].role == ca_llm::Role::Assistant)
                .unwrap_or(self.history.len());
        }
        if keep_from <= from + 2 {
            return Ok(None);
        }
        events.notice("Compacting this turn's earlier steps: the work so far is summarized, your message kept.");
        let older: Vec<Message> = self.history[from + 1..keep_from].to_vec();
        let mut ask = self.request(self.defs(), Some("none"));
        ask.messages.truncate(1);
        ask.messages.push(self.history[from].clone());
        ask.messages.extend(older.iter().cloned());
        ask.messages.push(Message::user(
            "(Code Arena: summarize the steps above, of the task this message gave you, for yourself to go on from: what was found, what was changed (files, and how), what was tried and failed, and what is left. Facts, not a story; keep paths, names, commands and numbers.)",
        ));
        ask.max_tokens = Some(self.window.reserve.clamp(1024, 8192) as u32);
        let summary = match self.gateway.complete(&ask, &mut ca_llm::Quiet).await {
            Ok(c)
                if !c.message.content.trim().is_empty()
                    && !context::looped(&c.message.content)
                    && c.finish != Finish::Length =>
            {
                c.message.content
            }
            Ok(_) | Err(_) => fallback_summary(&older, &vec![false; older.len()]),
        };
        let kept: Vec<Message> = self.history.drain(keep_from..).collect();
        let kept_notes: Vec<bool> = if self.notes.len() > keep_from {
            self.notes.drain(keep_from..).collect()
        } else {
            vec![false; kept.len()]
        };
        self.history.truncate(from + 1);
        self.notes.truncate(from + 1);
        self.note(format!(
            "(Code Arena: the earlier steps of this turn are summarized here; every step is kept.)\n\n{summary}"
        ));
        self.history.extend(kept);
        self.notes.extend(kept_notes);
        events.notice(&format!("Compacted: the model now reads about {} tokens.", self.context_used()));
        Ok(Some(summary))
    }

    /// The calls of one answer: those that only read run at once; one that changes something runs after the calls
    /// before it, in the order given. Each result goes into the history in the order of the calls.
    async fn run_calls(&mut self, calls: &[ToolCall], events: &mut dyn Events, out: &mut TurnOutcome) {
        let schemas = self.schemas();
        let mut i = 0;
        while i < calls.len() {
            if calls[i].name == LOAD_TOOLS {
                let names: Vec<String> = mend::parse_args(&calls[i].arguments, &mut vec![])
                    .ok()
                    .and_then(|v| v.get("names").cloned())
                    .and_then(|n| serde_json::from_value(n).ok())
                    .unwrap_or_default();
                let (loaded, unknown): (Vec<String>, Vec<String>) =
                    names.into_iter().partition(|n| self.load(n) || self.tool(n).is_some());
                let mut text = if loaded.is_empty() {
                    "Nothing loaded.".to_string()
                } else {
                    format!("Loaded {}: they can be called from now on.", loaded.join(", "))
                };
                if !unknown.is_empty() {
                    text.push_str(&format!(" Not listed: {}.", unknown.join(", ")));
                }
                self.push_result(&calls[i], Output::ok(text), None);
                i += 1;
                continue;
            }
            // A run of reading calls, together.
            let mut reads: Vec<(usize, Mended, Arc<dyn Tool>)> = Vec::new();
            let mut j = i;
            while j < calls.len() {
                let Ok(m) = mend::mend(&calls[j].name, &calls[j].arguments, &schemas) else { break };
                self.load(&m.name);
                match self.tool(&m.name) {
                    Some(t) if t.kind() == Kind::Read => reads.push((j, m, t)),
                    _ => break,
                }
                j += 1;
            }
            if !reads.is_empty() {
                let mut ready = Vec::new();
                for (idx, mut m, t) in reads {
                    let pre = Self::pre_tool(&self.hooks.clone(), &m.name, &m.args, events).await;
                    if let Some(why) = pre.block {
                        let why = format!("A hook blocked this call: {why}");
                        let said = Call { id: &calls[idx].id, name: &m.name, subject: "", arguments: &m.args };
                        events.tool_not_run(&said, &why, true);
                        self.push_result(&calls[idx], Output::error(why), None);
                        continue;
                    }
                    if let Some(u) = pre.updated_input {
                        m.args = u;
                    }
                    let subject = t.subject(&m.args);
                    let said = Call { id: &calls[idx].id, name: &m.name, subject: &subject, arguments: &m.args };
                    match self.policy.decide(&m.name, Kind::Read, &subject) {
                        Decision::Refuse(why) => {
                            events.tool_not_run(&said, &why, true);
                            self.push_result(&calls[idx], Output::error(why), None);
                        }
                        _ => {
                            events.tool_start(&said);
                            ready.push((idx, m, t));
                        }
                    }
                }
                let ctx = self.ctx.clone();
                // Each read's own time: they run at once, and end together.
                let runs = ready.iter().map(|(_, m, t)| {
                    let ctx = ctx.clone();
                    let t = t.clone();
                    let args = m.args.clone();
                    async move {
                        let began = std::time::Instant::now();
                        let o = t.run(args, &ctx).await;
                        (o, began.elapsed())
                    }
                });
                let results = futures_util::future::join_all(runs).await;
                for ((idx, m, t), (o, took)) in ready.into_iter().zip(results) {
                    let subject = t.subject(&m.args);
                    let said = Call { id: &calls[idx].id, name: &m.name, subject: &subject, arguments: &m.args };
                    events.tool_end(&said, &o, took);
                    let o = Self::post_tool(&self.hooks.clone(), &m.name, &m.args, o, events).await;
                    self.push_result(&calls[idx], o, mend::note(&m));
                }
                i = j;
                continue;
            }
            // One call that changes something, or that cannot run as given.
            let call = &calls[i];
            match mend::mend(&call.name, &call.arguments, &schemas) {
                Err(why) => {
                    let said = Call { id: &call.id, name: &call.name, subject: "", arguments: &Value::Null };
                    events.tool_not_run(&said, &why.0, false);
                    self.push_result(call, Output::error(why.0), None);
                }
                Ok(mut m) => {
                    self.load(&m.name);
                    let pre = Self::pre_tool(&self.hooks.clone(), &m.name, &m.args, events).await;
                    if let Some(why) = pre.block {
                        let why = format!("A hook blocked this call: {why}");
                        let said = Call { id: &call.id, name: &m.name, subject: "", arguments: &m.args };
                        events.tool_not_run(&said, &why, true);
                        self.push_result(call, Output::error(why), None);
                        i += 1;
                        continue;
                    }
                    if let Some(u) = pre.updated_input {
                        m.args = u;
                    }
                    let Some(tool) = self.tool(&m.name) else {
                        let why = format!("There is no tool named {}.", m.name);
                        let said = Call { id: &call.id, name: &m.name, subject: "", arguments: &m.args };
                        events.tool_not_run(&said, &why, false);
                        self.push_result(call, Output::error(why), None);
                        i += 1;
                        continue;
                    };
                    let kind = tool.kind();
                    let subject = tool.subject(&m.args);
                    let said = Call { id: &call.id, name: &m.name, subject: &subject, arguments: &m.args };
                    // A hook's "allow" spares the question (not a mode's refusal); its "ask" asks whatever the mode.
                    let decision = match (self.policy.decide(&m.name, kind, &subject), pre.permission.as_deref()) {
                        (Decision::Ask, Some("allow")) => Decision::Run,
                        (Decision::Run, Some("ask")) => Decision::Ask,
                        (d, _) => d,
                    };
                    let forced = pre.permission.as_deref() == Some("ask");
                    // Laya reads a command that is not refused; 60% or more asks whatever the mode.
                    let risk = match decision {
                        Decision::Refuse(_) => None,
                        _ => self.policy.risk(&m.name, &subject).await,
                    };
                    let laya_asks = decision == Decision::Run && risk.is_some_and(|r| r.asks());
                    let decision = policy::weigh(decision, risk.as_ref());
                    let allowed = match decision {
                        Decision::Run => Ok(()),
                        Decision::Refuse(why) => Err(why),
                        Decision::Ask => {
                            events.tool_waiting(&said);
                            // A question that would come back anyway offers no "always".
                            let always = policy::always_text(&m.name, kind, &subject)
                                .filter(|_| !forced && !risk.is_some_and(|r| r.asks()));
                            let title = policy::title(&m.name, kind);
                            let (timeless, background, _) = policy::waits(&subject);
                            let q = Question {
                                tool: &m.name,
                                subject: &subject,
                                title: &title,
                                arguments: &m.args,
                                always: always.as_deref(),
                                risk,
                                timeless,
                                background,
                            };
                            match Self::ask(self.hooks.clone(), self.approver.clone(), &q).await {
                                Some(Answer::Yes) => Ok(()),
                                Some(Answer::Always) if always.is_some() => {
                                    self.policy.remember(&m.name, kind, &subject);
                                    Ok(())
                                }
                                Some(Answer::Always) => Ok(()),
                                Some(Answer::No) => {
                                    Err(format!("{SAID_NO}: ask what they would like instead, or find another way."))
                                }
                                None if laya_asks => Err(format!(
                                    "This run cannot ask, and Laya reads this command as risky ({}): it was not run. Say what you would have done, or find a safer way.",
                                    risk.map(|r| r.line()).unwrap_or_default()
                                )),
                                None => Err(format!(
                                    "This run cannot ask, and {} asks in {} mode: it was not run. Say what you would have done; the person can run again with --mode auto-edit (edits) or --mode yolo (commands too).",
                                    m.name,
                                    self.policy.mode.name()
                                )),
                            }
                        }
                    };
                    match allowed {
                        Err(why) => {
                            events.tool_not_run(&said, &why, true);
                            self.push_result(call, Output::error(why), None);
                        }
                        Ok(()) => {
                            events.tool_start(&said);
                            let began = std::time::Instant::now();
                            let o = tool.run(m.args.clone(), &self.ctx).await;
                            events.tool_end(&said, &o, began.elapsed());
                            let o = Self::post_tool(&self.hooks.clone(), &m.name, &m.args, o, events).await;
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

    /// The person asked about a call (their Notification hooks told first).
    async fn ask(hooks: Arc<crate::hooks::Hooks>, approver: Arc<dyn Approver>, q: &Question<'_>) -> Option<Answer> {
        if hooks.has("Notification") {
            let _ = hooks
                .run(
                    "Notification",
                    "",
                    serde_json::json!({"message": format!("Code Arena asks to run {}: {}", q.tool, q.subject)}),
                )
                .await;
        }
        approver.ask(q).await
    }

    async fn pre_tool(
        hooks: &crate::hooks::Hooks,
        name: &str,
        args: &Value,
        events: &mut dyn Events,
    ) -> crate::hooks::Said {
        if !hooks.has("PreToolUse") {
            return crate::hooks::Said::default();
        }
        let said = hooks.run("PreToolUse", name, serde_json::json!({"tool_name": name, "tool_input": args})).await;
        for n in &said.notes {
            events.notice(n);
        }
        said
    }

    /// A `PostToolUse` hook's word goes with the result to the model.
    async fn post_tool(
        hooks: &crate::hooks::Hooks,
        name: &str,
        args: &Value,
        mut o: Output,
        events: &mut dyn Events,
    ) -> Output {
        if !hooks.has("PostToolUse") {
            return o;
        }
        let said = hooks
            .run(
                "PostToolUse",
                name,
                serde_json::json!({"tool_name": name, "tool_input": args, "tool_response": {"output": o.text, "is_error": o.is_error}}),
            )
            .await;
        for n in &said.notes {
            events.notice(n);
        }
        if let Some(why) = said.block {
            o.text.push_str(&format!("\n(A hook of the person's says: {why})"));
        }
        for c in said.context {
            o.text.push_str(&format!("\n(A hook of the person's adds: {c})"));
        }
        o
    }

    fn add(&mut self, m: Message, begins_turn: bool) -> Option<i64> {
        let seq = self.recorder.as_mut().and_then(|r| r.record(&m, begins_turn));
        self.sync_notes();
        self.history.push(m);
        self.notes.push(false);
        seq
    }

    /// A note of Code Arena's for the model: not kept in the session, not shown.
    pub fn note(&mut self, text: String) {
        self.sync_notes();
        self.history.push(Message::user(text));
        self.notes.push(true);
    }

    /// History given whole (a resumed session) has no flags yet: its entries are the session's.
    fn sync_notes(&mut self) {
        if self.notes.len() < self.history.len() {
            self.notes.resize(self.history.len(), false);
        }
    }

    fn push_result(&mut self, call: &ToolCall, o: Output, note: Option<String>) {
        let mut text = if o.text.chars().count() > RESULT_CHARS
            && let Some(h) = self.aside.as_mut().and_then(|a| a.keep(&o.text))
        {
            aside_view(&o.text, &h)
        } else {
            ca_tools::text::cut_middle(&o.text, RESULT_CHARS)
        };
        if let Some(n) = note {
            text.push_str("\n\n");
            text.push_str(&n);
        }
        self.add(Message::tool(call.id.clone(), text), false);
    }
}

/// A big result as the model reads it: its start and its end, and how to read the rest.
fn aside_view(text: &str, handle: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let head: Vec<&str> = lines.iter().take(80).copied().collect();
    let tail: Vec<&str> = lines.iter().skip(lines.len().saturating_sub(40).max(head.len())).copied().collect();
    let shown = |v: &[&str]| v.iter().map(|l| l.chars().take(300).collect::<String>()).collect::<Vec<_>>().join("\n");
    let not_shown = lines.len().saturating_sub(head.len() + tail.len());
    format!(
        "{}\n… {not_shown} lines not shown ({} characters in all). It is kept as {handle}: read_result {{\"id\":\"{handle}\",\"from\":{},\"lines\":200}} reads on, and {{\"id\":\"{handle}\",\"find\":\"text\"}} gives the lines that hold some text.\n{}",
        shown(&head),
        text.len(),
        head.len() + 1,
        shown(&tail)
    )
}

/// A summary that could not be made: what the older messages held, from the person's messages and the files and
/// commands the tools touched. The turn goes on.
fn fallback_summary(older: &[Message], notes: &[bool]) -> String {
    let mut s = String::from(
        "## State\nThe model gave no summary; this is what the earlier messages held.\n\n## The person asked\n",
    );
    for (i, m) in older.iter().enumerate() {
        if m.role == ca_llm::Role::User && !notes.get(i).copied().unwrap_or(false) {
            s.push_str(&format!(
                "- {}\n",
                m.content.lines().next().unwrap_or("").chars().take(200).collect::<String>()
            ));
        }
    }
    s.push_str("\n## Calls made\n");
    for m in older {
        for c in &m.tool_calls {
            s.push_str(&format!("- {} {}\n", c.name, c.arguments.chars().take(120).collect::<String>()));
        }
    }
    if let Some(last) = older.iter().rev().find(|m| m.role == ca_llm::Role::Assistant && !m.content.is_empty()) {
        s.push_str(&format!("\n## The last answer\n{}\n", last.content.chars().take(1500).collect::<String>()));
    }
    s
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
            ctx: Context::new(Arc::new(ws), None, Duration::from_secs(30)),
            policy: Policy::new(mode, Rules::default()),
            approver: Arc::new(NoAsking),
            max_steps: 20,
            system: "sys".into(),
            history: vec![],
            recorder: None,
            inbox: None,
            deferred: vec![],
            joins: None,
            window: Window::default(),
            aside: None,
            notes: vec![],
            tags: vec![],
            skip: Arc::default(),
            second_reading: false,
            repo_key: None,
            hooks: Arc::default(),
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
        // The turn's checkpoint knows the file was not there: going back removes it.
        let r = a.ctx.checkpoints.rewind(0);
        assert_eq!(r.removed.len(), 1);
        assert!(!d.path().join("x.txt").exists());
    }

    /// Laya that reads every command as destroying something.
    struct Alarmed;
    #[async_trait::async_trait]
    impl crate::policy::CommandCheck for Alarmed {
        async fn check(&self, _command: &str) -> Option<crate::policy::Risk> {
            Some(crate::policy::Risk { destructive: 0.96, outside: 0.93, network: 0.03 })
        }
    }

    #[tokio::test]
    async fn a_command_laya_flags_asks_even_in_yolo_and_a_run_that_cannot_ask_refuses_it() {
        let (_s, mut a, d, seen) = agent(
            vec![call("run_shell", r#"{"command": "touch made.txt"}"#), say("I could not.", "stop")],
            Mode::Yolo,
        )
        .await;
        a.policy.laya = Some(crate::policy::Laya(Arc::new(Alarmed)));
        a.turn("make it", &mut Quiet).await.expect("turn");
        assert!(!d.path().join("made.txt").exists());
        let told = seen.lock().expect("lock")[1]["messages"][3]["content"].as_str().unwrap_or("").to_string();
        assert!(told.contains("Laya reads this command as risky (Laya: destructive 96%"), "{told}");
    }

    fn cut_call(name: &str, args: &str) -> String {
        let event = serde_json::json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c-cut", "function": {"name": name, "arguments": args}}]}, "finish_reason": "length"}]});
        format!("data: {event}\n\ndata: [DONE]\n\n")
    }

    #[tokio::test]
    async fn a_cut_call_does_not_run_and_is_done_in_parts() {
        let cut = r#"{"path": "big.txt", "content": "line one\nline tw"#;
        let (_s, mut a, d, seen) = agent(
            vec![
                cut_call("write_file", cut),
                call("write_file", r#"{"path": "big.txt", "content": "line one\n"}"#),
                call("write_file", r#"{"path": "big.txt", "content": "line two\n", "append": true}"#),
                say("Written in two parts.", "stop"),
            ],
            Mode::Yolo,
        )
        .await;
        let out = a.turn("write big.txt", &mut Quiet).await.expect("turn");
        assert_eq!(out.answer, "Written in two parts.");
        assert_eq!(std::fs::read_to_string(d.path().join("big.txt")).expect("file"), "line one\nline two\n");
        let told = seen.lock().expect("lock")[1]["messages"].to_string();
        assert!(told.contains("cut off after") && told.contains("append: true"), "{told}");
    }

    #[tokio::test]
    async fn a_call_cut_twice_in_the_same_place_ends_the_turn() {
        let cut = r#"{"path": "big.txt", "content": "the same long start"#;
        let (_s, mut a, d, _seen) =
            agent(vec![cut_call("write_file", cut), cut_call("write_file", cut)], Mode::Yolo).await;
        let e = a.turn("write big.txt", &mut Quiet).await.expect_err("ends");
        assert!(matches!(e, TurnError::Cut(ref n) if n == "write_file"), "{e}");
        assert!(!d.path().join("big.txt").exists(), "nothing half written");
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
    async fn listed_tools_load_by_load_tools_or_by_being_called() {
        let (_s, mut a, _d, seen) = agent(
            vec![
                call("load_tools", r#"{"names": ["grep", "nope"]}"#),
                call("glob", r#"{"pattern": "*.txt"}"#),
                say("Done.", "stop"),
            ],
            Mode::Ask,
        )
        .await;
        // grep and glob are listed only.
        let all = std::mem::take(&mut a.tools);
        for t in all {
            if t.def().name == "grep" || t.def().name == "glob" {
                a.deferred.push(t);
            } else {
                a.tools.push(t);
            }
        }
        a.turn("find notes", &mut Quiet).await.expect("turn");
        let seen = seen.lock().expect("lock");
        let names = |i: usize| -> Vec<String> {
            seen[i]["tools"]
                .as_array()
                .expect("tools")
                .iter()
                .map(|t| t["function"]["name"].as_str().expect("n").to_string())
                .collect()
        };
        assert!(names(0).contains(&"load_tools".to_string()) && !names(0).contains(&"grep".to_string()));
        assert!(names(1).contains(&"grep".to_string()));
        assert!(
            seen[1]["messages"][3]["content"]
                .as_str()
                .expect("t")
                .starts_with("Loaded grep: they can be called from now on. Not listed: nope.")
        );
        // glob was called straight from the list: loaded then, and run.
        assert!(names(2).contains(&"glob".to_string()) && !names(2).contains(&"load_tools".to_string()));
        assert!(seen[2]["messages"][5]["content"].as_str().expect("t").contains("notes.txt"));
    }

    struct Kept(Arc<std::sync::Mutex<Vec<String>>>);
    impl crate::turn::Aside for Kept {
        fn keep(&mut self, text: &str) -> Option<String> {
            let mut k = self.0.lock().expect("lock");
            k.push(text.to_string());
            Some(format!("r{}", k.len()))
        }
    }
    struct Compactions(Arc<std::sync::Mutex<Vec<(String, usize)>>>);
    impl crate::turn::Recorder for Compactions {
        fn record(&mut self, _m: &Message, _b: bool) -> Option<i64> {
            None
        }
        fn compacted(&mut self, summary: &str, kept: usize) {
            self.0.lock().expect("lock").push((summary.to_string(), kept));
        }
    }

    #[tokio::test]
    async fn past_the_threshold_the_older_part_is_summarized() {
        let summary = say("## Goal\nanswer questions", "stop");
        let (_s, mut a, _d, seen) =
            agent(vec![say("one", "stop"), say("two", "stop"), summary, say("three", "stop")], Mode::Ask).await;
        let done = Arc::new(std::sync::Mutex::new(vec![]));
        a.recorder = Some(Box::new(Compactions(done.clone())));
        a.turn(&"a".repeat(2_000), &mut Quiet).await.expect("turn 1");
        a.turn(&"b".repeat(2_000), &mut Quiet).await.expect("turn 2");
        // A small window: the next request passes 80% of it.
        a.window = Window::new(Some(1_500), Some(100), Some(80), 25);
        a.turn("third question", &mut Quiet).await.expect("turn 3");
        let seen = seen.lock().expect("lock");
        let ask = &seen[2];
        assert_eq!(ask["tool_choice"], "none");
        assert!(
            ask["messages"].as_array().expect("m").last().expect("last")["content"]
                .as_str()
                .expect("t")
                .starts_with("Summarize the conversation")
        );
        let after = seen[3]["messages"].as_array().expect("m");
        assert!(after[1]["content"].as_str().expect("t").contains("## Goal\nanswer questions"));
        assert_eq!(after.last().expect("last")["content"], "third question");
        assert_eq!(after.len(), 3, "system, the summary, the question kept whole");
        let done = done.lock().expect("lock");
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].1, 1, "the store keeps every message: the model reads the summary for all but the last one");
    }

    #[tokio::test]
    async fn a_result_too_big_for_the_window_is_kept_aside() {
        let (_s, mut a, d, seen) = agent(
            vec![call("read_file", r#"{"path": "big.txt", "offset": 1, "limit": 2000}"#), say("ok", "stop")],
            Mode::Ask,
        )
        .await;
        std::fs::write(
            d.path().join("big.txt"),
            (0..2000).map(|i| format!("line {i} {}\n", "x".repeat(20))).collect::<String>(),
        )
        .expect("write");
        let kept = Arc::new(std::sync::Mutex::new(vec![]));
        a.aside = Some(Box::new(Kept(kept.clone())));
        a.turn("read it", &mut Quiet).await.expect("turn");
        assert_eq!(kept.lock().expect("lock").len(), 1);
        let content = seen.lock().expect("lock")[1]["messages"][3]["content"].as_str().expect("t").to_string();
        assert!(content.contains("It is kept as r1: read_result"), "{}", &content[..200]);
        assert!(content.len() < 16_000);
    }

    #[tokio::test]
    async fn the_windows_refusal_compacts_and_asks_again_once() {
        let server = MockServer::start().await;
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        Mock::given(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({"error": {"message": "maximum context length is 1000 tokens"}})),
            )
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(Script { answers: vec![say("ok", "stop")], at: AtomicUsize::new(0), seen: seen.clone() })
            .mount(&server)
            .await;
        let (_s2, mut a, _d, _) = agent(vec![], Mode::Ask).await;
        a.gateway = Gateway::new(&server.uri(), Secret::new("sk-test"), None).expect("gw");
        a.history.push(Message::user("an old question"));
        a.history.push(Message::assistant("an old answer"));
        let out = a.turn("new", &mut Quiet).await.expect("turn");
        assert_eq!(out.answer, "ok");
        // The summary was asked, then the question again.
        assert_eq!(seen.lock().expect("lock").len(), 2);
    }

    #[tokio::test]
    async fn the_step_limit_ends_a_runaway_turn() {
        let (_s, mut a, _d, _) = agent(vec![call("list_dir", "{}")], Mode::Ask).await;
        a.max_steps = 3;
        assert!(matches!(a.turn("loop", &mut Quiet).await, Err(TurnError::Steps(3))));
    }

    /// Events that say when the model began to think.
    struct Watch(tokio::sync::mpsc::UnboundedSender<()>, String);
    impl Events for Watch {
        fn reasoning(&mut self, _d: &str) {
            let _ = self.0.send(());
        }
        fn text(&mut self, d: &str) {
            self.1.push_str(d);
        }
    }

    #[tokio::test]
    async fn tab_cuts_the_thinking_and_the_model_answers_at_once() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // A model that thinks without end the first time, and answers when asked not to think.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let bodies = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let b2 = bodies.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let b3 = b2.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    let mut got = Vec::new();
                    // The headers and the body, by its length.
                    loop {
                        let n = sock.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        got.extend_from_slice(&buf[..n]);
                        let text = String::from_utf8_lossy(&got).to_string();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len = text[..end]
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            if got.len() >= end + 4 + len {
                                let body = String::from_utf8_lossy(&got[end + 4..end + 4 + len]).to_string();
                                b3.lock().expect("bodies").push(body.clone());
                                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n";
                                let _ = sock.write_all(head.as_bytes()).await;
                                if body.contains("enable_thinking") {
                                    let ev =
                                        r#"{"choices":[{"delta":{"content":"The answer."},"finish_reason":"stop"}]}"#;
                                    let _ = sock.write_all(format!("data: {ev}\n\ndata: [DONE]\n\n").as_bytes()).await;
                                    return;
                                }
                                let ev = r#"{"choices":[{"delta":{"reasoning_content":"Let me think about it"}}]}"#;
                                let _ = sock.write_all(format!("data: {ev}\n\n").as_bytes()).await;
                                let _ = sock.flush().await;
                                // Thinks on, until cut.
                                tokio::time::sleep(Duration::from_secs(60)).await;
                                return;
                            }
                        }
                    }
                });
            }
        });
        let (_server, mut a, _dir, _seen) = agent(vec![], Mode::Ask).await;
        a.gateway = Gateway::new(&format!("http://{addr}"), Secret::new("sk-test"), None).expect("gw");
        let skip = a.skip.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut w = Watch(tx, String::new());
        let turn = a.turn("why?", &mut w);
        tokio::pin!(turn);
        let out = loop {
            tokio::select! {
                r = &mut turn => break r.expect("answered"),
                Some(()) = rx.recv() => { skip.ask(); }
            }
        };
        assert_eq!(out.answer, "The answer.");
        let bodies = bodies.lock().expect("bodies");
        assert_eq!(bodies.len(), 2);
        assert!(bodies[1].contains("\"enable_thinking\":false"), "{}", bodies[1]);
    }

    /// A model that reads four times, then answers; asked for a summary (no tools to call), it gives one.
    struct Steps(AtomicUsize, Arc<std::sync::Mutex<Vec<Value>>>);
    impl wiremock::Respond for Steps {
        fn respond(&self, r: &wiremock::Request) -> wiremock::ResponseTemplate {
            let body: Value = serde_json::from_slice(&r.body).unwrap_or_default();
            self.1.lock().expect("seen").push(body.clone());
            let ev = if body["tool_choice"] == "none" {
                serde_json::json!({"choices": [{"delta": {"content": "Read notes.txt four times: it says 42."}, "finish_reason": "stop"}]})
            } else if self.0.fetch_add(1, Ordering::SeqCst) < 4 {
                serde_json::json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": format!("c{}", self.0.load(Ordering::SeqCst)), "function": {"name": "read_file", "arguments": "{\"path\":\"notes.txt\"}"}}]}, "finish_reason": "tool_calls"}]})
            } else {
                serde_json::json!({"choices": [{"delta": {"content": "It is 42."}, "finish_reason": "stop"}]})
            };
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {ev}\n\ndata: [DONE]\n\n"))
        }
    }

    #[tokio::test]
    async fn one_long_turn_compacts_its_own_steps_and_keeps_the_question() {
        let server = MockServer::start().await;
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        Mock::given(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(Steps(AtomicUsize::new(0), seen.clone()))
            .mount(&server)
            .await;
        let (_s, mut a, d, _) = agent(vec![], Mode::Ask).await;
        a.gateway = Gateway::new(&server.uri(), Secret::new("sk-test"), None).expect("gw");
        std::fs::write(d.path().join("notes.txt"), "the answer is 42\n".repeat(150)).expect("big");
        a.window = Window::new(Some(3_000), Some(200), Some(80), 25);
        let out = a.turn("what do the notes say?", &mut Quiet).await.expect("turn");
        assert_eq!(out.answer, "It is 42.");
        let seen = seen.lock().expect("seen");
        assert!(seen.iter().any(|b| b["tool_choice"] == "none"), "a summary was asked");
        let last = seen.last().expect("last")["messages"].to_string();
        assert!(last.contains("what do the notes say?"), "the question is kept");
        assert!(last.contains("the earlier steps of this turn are summarized"), "{last}");
        assert!(last.contains("it says 42"), "{last}");
    }

    /// Farzan Graph's `change_impact`, as a fake: two lines use the symbol, one a test.
    struct Impact;
    #[async_trait::async_trait]
    impl Tool for Impact {
        fn def(&self) -> ToolDef {
            ToolDef {
                name: "mcp__graph__change_impact".into(),
                description: "impact".into(),
                parameters: serde_json::json!({"type": "object"}),
            }
        }
        fn kind(&self) -> Kind {
            Kind::Read
        }
        fn subject(&self, _a: &Value) -> String {
            String::new()
        }
        async fn run(&self, args: Value, _ctx: &Context) -> Output {
            assert_eq!(args["repo"], "payments/api");
            Output::ok(
                serde_json::json!({"dependents": 2, "references": [
                    {"repo": "payments/api", "path": "src/pay.rs", "is_definition": true},
                    {"repo": "payments/api", "path": "src/use.rs"},
                    {"repo": "payments/web", "path": "tests/pay_test.rs"}
                ]})
                .to_string(),
            )
        }
    }

    #[tokio::test]
    async fn a_proposed_plan_is_grounded_and_read_again_once() {
        let plan = r#"{"title":"Pay","steps":[{"title":"Change parse","symbols":["parse_amount"]}]}"#;
        let call = |args: &str| {
            let ev = serde_json::json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "p1", "function": {"name": "plan", "arguments": args}}]}, "finish_reason": "tool_calls"}]});
            format!("data: {ev}\n\ndata: [DONE]\n\n")
        };
        let (_s, mut a, _d, seen) = agent(
            vec![
                call(plan),
                say("Here is the plan.", "stop"),
                say("Step 1 has no check.", "stop"),
                say("Noted: I would add a check.", "stop"),
            ],
            Mode::Plan,
        )
        .await;
        a.second_reading = true;
        a.repo_key = Some("gitlab.corp/payments/api".into());
        a.tools.push(Arc::new(Impact));
        let out = a.turn("change how amounts parse", &mut Quiet).await.expect("turn");
        assert!(out.answer.ends_with("Noted: I would add a check."), "{}", out.answer);
        let p = a.ctx.plan.get();
        assert_eq!(p.critique.as_deref(), Some("Step 1 has no check."));
        let impact = p.steps[0].impact.clone().expect("impact");
        assert!(
            impact.contains("2 lines use it")
                && impact.contains("2 repositories depend")
                && impact.contains("tests/pay_test.rs"),
            "{impact}"
        );
        let seen = seen.lock().expect("seen");
        assert_eq!(seen.len(), 4, "the reading once, then one more step");
        assert!(seen[3].to_string().contains("a second reading of your plan"));
    }
}
