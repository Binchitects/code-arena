//! `code-arena ide`: the session in a window of its own. The server (ca-server) serves the page and its live stream;
//! this drives the session from what the page sends, and sends the page what the turn does: the answer as it is
//! written, each tool call, the questions it asks (a card in the thread), and how the turn ended.
//!
//! This is the driver's one loop and its own messages (the session's state, a turn, its questions, the model,
//! the mode, a setting). Everything else the IDE asks is an area's, in a file of its own: `ide_ext` says how an
//! area joins the loop, and what becomes of a message that comes while a turn runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ca_agent::{Answer, Approver, Call, Events};
use ca_tools::Output;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::app::App;
use crate::ide_chat::replay::Replay;
use crate::ide_ext::{self, Ctx, Driver, Kept, Later, Rule, Sent, Shared, TurnEnd, send};
use crate::ide_files::Side;
use crate::session::{self, Opts, Prepared, Resume};

/// The driver's own messages, each with what becomes of it while a turn runs (`ide_ext`'s turn rules). Those
/// answered at once are the turn's own (`answer`, `stop`, `skip`, a message sent meanwhile) and what needs the
/// session's handles only.
pub const MESSAGES: &[(&str, Rule)] = &[
    ("hello", Rule::Turn),
    ("state", Rule::Turn),
    ("send", Rule::Turn),
    ("answer", Rule::Turn),
    ("stop", Rule::Turn),
    ("skip", Rule::Turn),
    ("stop_job", Rule::Turn),
    ("themes", Rule::Turn),
    ("keep_view_state", Rule::Turn),
    ("older", Rule::Turn),
    ("context", Rule::Latest),
    ("model", Rule::Queue),
    ("thinking", Rule::Queue),
    ("mode", Rule::Queue),
    ("set_setting", Rule::Queue),
    ("compact", Rule::Refuse("compact the conversation")),
    ("rewind", Rule::Refuse("go back to an earlier question")),
    ("again", Rule::Refuse("ask again")),
];

/// The licence this program is offered under, and its text (the About box).
const LICENSE_NAME: &str = "MIT";
const LICENSE_TEXT: &str = include_str!("../../../LICENSE");

/// What one tool result shows in the page at most.
const SHOWN_RESULT: usize = 4000;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Answer>>>>;

/// The turn's questions, as cards in the page's thread; the first page to answer decides.
struct PageApprover {
    out: broadcast::Sender<String>,
    pending: Pending,
    next: AtomicU64,
}

#[async_trait::async_trait]
impl Approver for PageApprover {
    async fn ask(&self, tool: &str, subject: &str) -> Option<Answer> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = self.pending.lock() {
            p.insert(id, tx);
        }
        send(&self.out, json!({"type": "ask", "id": id, "tool": tool, "subject": subject}));
        // A turn stopped (its questions dropped) answers no.
        Some(rx.await.unwrap_or(Answer::No))
    }
}

/// What the session is working through: its plan, its goal, its jobs (the bars above the box), and what the
/// areas add. From the session's handles, so it is said while a turn runs too.
fn tasks(sh: &Shared) -> Value {
    let plan = sh.plan.get();
    let goals = sh.goals.get();
    let jobs: Vec<Value> = sh
        .jobs
        .as_ref()
        .map(|j| j.list())
        .unwrap_or_default()
        .into_iter()
        .map(|j| json!({"id": j.id, "command": j.command, "running": j.running, "exit": j.exit, "background": j.background}))
        .collect();
    let mut v = json!({
        "type": "tasks",
        "plan": if plan.steps.is_empty() { Value::Null } else { serde_json::to_value(&plan).unwrap_or(Value::Null) },
        "goal": goals.current.as_ref().map(|g| json!({"text": g.text, "status": g.status, "turns": g.turns})),
        "waiting": goals.next.len(),
        "jobs": jobs,
    });
    ide_ext::tasks(sh, &mut v);
    v
}

/// What the turn does, as the page reads it.
struct PageEvents {
    out: broadcast::Sender<String>,
    /// The session's handles, for the bars (none while the conversation is compacted: nothing of theirs changes).
    tasks: Option<Shared>,
    /// Since when the model thinks (none: it does not now), and how long it thought before in this turn.
    thinking: Option<std::time::Instant>,
    thought_ms: u64,
}

impl PageEvents {
    fn new(out: broadcast::Sender<String>, tasks: Option<Shared>) -> Self {
        PageEvents { out, tasks, thinking: None, thought_ms: 0 }
    }

    /// The model's thinking gave way (to its words, a call, the step's or the turn's end): the faces are told how
    /// long it thought in this turn in all, and whether it was cut short (Skip thinking, or the turn stopped).
    fn thought_ends(&mut self, stopped: bool) {
        if let Some(since) = self.thinking.take() {
            self.thought_ms += since.elapsed().as_millis() as u64;
            let skipped = self.tasks.as_ref().is_some_and(|sh| sh.skip.asked());
            send(&self.out, json!({"type": "thought", "ms": self.thought_ms, "cut_short": skipped || stopped}));
        }
    }

    /// How a call ended, kept for the thread a face asks for again in this run.
    fn keep(&self, id: &str, fact: Fact) {
        if let Some(sh) = &self.tasks {
            sh.kept.of::<CallFacts>().keep(&sh.session, id, fact);
        }
    }
}

/// What a turn used as the faces show it under its answer, and as the session keeps it: the tokens read (and how
/// many of them from the cache) and written, how long it took and on which model. Its cost is added when a model's
/// prices are known (none are yet).
fn usage(u: &ca_llm::Usage, ms: u64, model: &str) -> Value {
    json!({"prompt": u.prompt, "cached": u.cached, "completion": u.completion, "ms": ms, "model": model})
}

/// What one argument of a call shows in the page at most (a file written whole is long).
const SHOWN_ARGUMENT: usize = 8000;

/// A result as the page shows it: its start, and whether the rest was cut.
fn shown_result(text: &str) -> (String, bool) {
    (text.chars().take(SHOWN_RESULT).collect(), text.chars().count() > SHOWN_RESULT)
}

/// A call's arguments as the page shows them: each text's start, a longer one ending in "…".
fn shown_arguments(v: &Value) -> Value {
    match v {
        Value::String(s) if s.chars().count() > SHOWN_ARGUMENT => {
            Value::String(s.chars().take(SHOWN_ARGUMENT).chain(std::iter::once('…')).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(shown_arguments).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), shown_arguments(v))).collect()),
        other => other.clone(),
    }
}

/// A call as the page is told of it: its id, its name, what it is about and what it was asked with.
fn said_call(kind: &str, call: &Call<'_>) -> Value {
    json!({"type": kind, "id": call.id, "name": call.name, "subject": call.subject,
           "arguments": shown_arguments(call.arguments)})
}

/// How a call ended: whether it gave a result, how long it ran, and the faces' word for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fact {
    ok: bool,
    ms: u64,
    status: &'static str,
}

/// How the calls of this run ended, by session and call id. The session's store keeps a call's arguments and its
/// result's text only, so a thread read back after a start says of a call only that it has a result.
#[derive(Default)]
struct CallFacts(Mutex<HashMap<(String, String), Fact>>);

impl CallFacts {
    /// The most calls remembered: past it the oldest run's are forgotten with the rest.
    const MOST: usize = 4000;

    fn keep(&self, session: &str, id: &str, fact: Fact) {
        if let Ok(mut all) = self.0.lock() {
            if all.len() >= Self::MOST {
                all.clear();
            }
            all.insert((session.to_string(), id.to_string()), fact);
        }
    }

    fn of(&self, session: &str, id: &str) -> Option<Fact> {
        self.0.lock().ok().and_then(|all| all.get(&(session.to_string(), id.to_string())).copied())
    }
}

/// A call as it ran: its tool's name, what it was asked with and what it is about (the model's own words for the
/// first two are mended before a call runs).
struct Read {
    name: String,
    arguments: Value,
    subject: String,
}

/// The calls of a kept answer's step, as the faces show them: each with what it was asked with, its result's start
/// (the message after it that answers its id) and, for a call of this run, how it ended.
fn kept_calls(
    step: &ca_llm::Message,
    after: &[ca_session::Stored],
    read: &dyn Fn(&ca_llm::ToolCall) -> Read,
    fact: &dyn Fn(&str) -> Option<Fact>,
) -> Vec<Value> {
    step.tool_calls
        .iter()
        .map(|call| {
            let Read { name, arguments, subject } = read(call);
            let result = after
                .iter()
                .map(|m| &m.message)
                .find(|m| m.role == ca_llm::Role::Tool && m.tool_call_id.as_deref() == Some(call.id.as_str()));
            let mut v = json!({
                "id": call.id,
                "name": name,
                "subject": subject,
                "arguments": shown_arguments(&arguments),
                "done": result.is_some(),
            });
            if let Some(r) = result {
                let (text, cut) = shown_result(&r.content);
                v["text"] = json!(text);
                v["cut"] = json!(cut);
                if let Some(f) = fact(&call.id) {
                    v["ok"] = json!(f.ok);
                    v["ms"] = json!(f.ms);
                    v["status"] = json!(f.status);
                } else if r.content.starts_with(ca_agent::SAID_NO) {
                    v["ok"] = json!(false);
                    v["status"] = json!("denied");
                }
            }
            v
        })
        .collect()
}
impl Events for PageEvents {
    fn text(&mut self, delta: &str) {
        self.thought_ends(false);
        send(&self.out, json!({"type": "text", "delta": delta}));
    }
    fn reasoning(&mut self, delta: &str) {
        self.thinking.get_or_insert_with(std::time::Instant::now);
        send(&self.out, json!({"type": "reasoning", "delta": delta}));
    }
    fn notice(&mut self, text: &str) {
        // A message sent meanwhile, read at this step: `took_in` first, and the notice marked with its number.
        let mut v = json!({"type": "notice", "text": text});
        if let Some(id) = self.tasks.as_ref().and_then(|sh| crate::ide_chat::queue::read(sh, text)) {
            v["took_in"] = json!(id);
        }
        send(&self.out, v);
    }
    fn tool_waiting(&mut self, call: &Call<'_>) {
        self.thought_ends(false);
        send(&self.out, said_call("tool_waiting", call));
    }
    fn tool_start(&mut self, call: &Call<'_>) {
        self.thought_ends(false);
        if let Some(sh) = &self.tasks {
            crate::ide_diffs::Diffs::begin(sh, call);
        }
        send(&self.out, said_call("tool_start", call));
    }
    fn tool_end(&mut self, call: &Call<'_>, output: &Output, took: std::time::Duration) {
        let (text, cut) = shown_result(&output.text);
        let (ok, ms) = (!output.is_error, took.as_millis() as u64);
        let status = if ok { "done" } else { "failed" };
        self.keep(call.id, Fact { ok, ms, status });
        let mut m = json!({"type": "tool_end", "id": call.id, "name": call.name, "subject": call.subject, "ok": ok,
                           "text": text, "cut": cut, "ms": ms, "status": status});
        if let Some(diff) = self.tasks.as_ref().and_then(|sh| crate::ide_diffs::Diffs::end(sh, call, output)) {
            m["diff"] = diff;
        }
        send(&self.out, m);
        // The plan, a goal's check or a job changed with it: the bars are told at once.
        if matches!(call.name, "plan" | "goal_check" | "run_shell" | "command_output" | "stop_command")
            && let Some(sh) = &self.tasks
        {
            send(&self.out, tasks(sh));
        }
    }
    fn tool_not_run(&mut self, call: &Call<'_>, why: &str, denied: bool) {
        // A call that never began ends all the same: the faces show it with why it did not run.
        let (text, cut) = shown_result(why);
        let status = if denied { "denied" } else { "failed" };
        self.keep(call.id, Fact { ok: false, ms: 0, status });
        let mut m = said_call("tool_end", call);
        for (key, value) in [("ok", json!(false)), ("text", json!(text)), ("cut", json!(cut)), ("ms", json!(0))] {
            m[key] = value;
        }
        m["status"] = json!(status);
        send(&self.out, m);
    }
    fn tool_progress(&mut self, id: &str, progress: f64, total: Option<f64>, message: &str) {
        send(
            &self.out,
            json!({"type": "tool_progress", "id": id, "progress": progress, "total": total, "message": message}),
        );
    }
    fn step_end(&mut self) {
        self.thought_ends(false);
        send(&self.out, json!({"type": "step_end"}));
    }
}

/// The manual's Code Arena page in the Arena signed in to (the help links to it); none without an Arena.
fn manual(arena: Option<&str>) -> Value {
    arena.map(|url| json!(format!("{}/help/code-arena", url.trim_end_matches('/')))).unwrap_or(Value::Null)
}

/// What this edition does not have, each with why: the commands and the settings that need Farzan Arena.
fn locked(edition: ca_settings::Edition) -> Value {
    let names = crate::edition::NEEDS_ARENA.iter().filter_map(|c| Some((*c, crate::edition::locked(edition, c)?)));
    let settings = crate::edition::SETTINGS_NEED_ARENA
        .iter()
        .filter_map(|k| Some((*k, crate::edition::setting_locked(edition, k)?)));
    Value::Object(names.chain(settings).map(|(name, why)| (name.to_string(), json!(why))).collect())
}

/// What a kept request is shown with beyond its words: what was kept with it when it was sent (its `details`: the
/// words as typed, its quote, the files sent with it, the skill it ran), whether it is a note of Code Arena's own,
/// and whether it was sent while an answer was written and read at its next step (a request that is not the first
/// of its turn).
fn question_details(v: &mut Value, first: bool, details: Option<&Value>) {
    let note = is_note(v["text"].as_str().unwrap_or(""));
    if let Some(o) = details.and_then(Value::as_object) {
        for (k, f) in o {
            if !matches!(k.as_str(), "role" | "text" | "turn" | "q" | "tools") {
                v[k] = f.clone();
            }
        }
    }
    if note {
        v["note"] = json!(true);
    } else if !first {
        v["taken_in"] = json!(true);
    }
}

/// A request that is a note of Code Arena's own (a goal's next turn, a loop's report), not the person's words.
fn is_note(text: &str) -> bool {
    text.starts_with("(Code Arena")
}

/// The skill a request ran: its name, when what the model was sent is a skill's instructions for `/NAME`.
fn skill_of(typed: &str, message: &str) -> Option<String> {
    let name = typed.strip_prefix('/')?.split_whitespace().next()?;
    message.starts_with(&format!("Use the skill {name}:")).then(|| name.to_string())
}

/// The session's tools, as a kept call is read with them (its name and arguments mended, what it is about): taken
/// at each `state` and at each turn's start, so a page of the thread asked for while a turn holds the session is
/// read the same.
#[derive(Default)]
struct Tools(Mutex<Vec<ToolOf>>);

/// A tool by its name, with its parameters' schema.
type ToolOf = (String, Value, Arc<dyn ca_tools::Tool>);

impl Tools {
    fn take_from(&self, p: &Prepared) {
        let all = p
            .agent
            .tools
            .iter()
            .chain(p.agent.deferred.iter())
            .map(|t| {
                let d = t.def();
                (d.name, d.parameters, t.clone())
            })
            .collect();
        if let Ok(mut g) = self.0.lock() {
            *g = all;
        }
    }

    fn all(&self) -> Vec<ToolOf> {
        self.0.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

/// The turns a page of the thread holds at most (the last ones in `state`, those before in `older`).
const PAGE_TURNS: i64 = 30;

/// A page of the session's thread as the faces draw it: the messages of the turns before `before_turn` (the last
/// ones when none), with how many turns come before them and the first turn they hold.
fn thread_page(
    store: &ca_session::Store,
    session: &str,
    before_turn: Option<i64>,
    tools: &[ToolOf],
    facts: &CallFacts,
    of_run: &dyn Fn(&str) -> Option<Value>,
) -> (Vec<Value>, i64, i64) {
    let numbers: HashMap<i64, usize> =
        session::questions_in(store, session).unwrap_or_default().into_iter().map(|(n, seq, _)| (seq, n)).collect();
    let schemas: Vec<(String, Value)> = tools.iter().map(|(n, s, _)| (n.clone(), s.clone())).collect();
    let read = |call: &ca_llm::ToolCall| match ca_agent::mend::mend(&call.name, &call.arguments, &schemas) {
        Ok(m) => {
            let subject =
                tools.iter().find(|(n, _, _)| *n == m.name).map(|(_, _, t)| t.subject(&m.args)).unwrap_or_default();
            Read { name: m.name, arguments: m.args, subject }
        }
        Err(_) => Read {
            name: call.name.clone(),
            arguments: ca_agent::mend::parse_args(&call.arguments, &mut vec![]).unwrap_or(Value::Null),
            subject: String::new(),
        },
    };
    let fact = |id: &str| facts.of(session, id);
    let Ok(page) = store.page(session, PAGE_TURNS, before_turn) else { return (Vec::new(), 0, 1) };
    let from = page.messages.first().map_or(0, |m| m.seq);
    let used = store.usages(session, from).unwrap_or_default();
    let details = page.messages.first().and_then(|m| store.details(session, m.seq).ok()).unwrap_or_default();
    let messages = page
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| !matches!(m.message.role, ca_llm::Role::System))
        .map(|(at, m)| {
            let mut v = json!({
                "role": m.message.role,
                "text": m.message.content,
                "tools": m.message.tool_calls.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
                "turn": m.turn,
                "q": numbers.get(&m.seq),
            });
            if let Some(r) = m.message.reasoning.as_deref().filter(|r| !r.is_empty()) {
                v["reasoning"] = json!(r);
            }
            if let Some(u) = used.get(&m.seq) {
                v["usage"] = u.clone();
            }
            // Each call with what the session kept of it, beside their names (which stay as they were).
            if !m.message.tool_calls.is_empty() {
                let after = &page.messages[at + 1..];
                let mut calls = kept_calls(&m.message, after, &read, &fact);
                crate::ide_diffs::add_kept(&mut calls, after, &details, of_run);
                v["tool_calls"] = json!(calls);
            }
            if m.message.role == ca_llm::Role::User {
                let first = at == 0 || page.messages[at - 1].turn != m.turn;
                question_details(&mut v, first, details.get(&m.seq));
            }
            v
        })
        .collect();
    (messages, page.turns_before, page.first_turn)
}

/// An earlier page of the thread (`older{before_turn}`), read off the loop: `older{messages, earlier, first_turn,
/// before_turn}`.
fn older(sh: &Shared, m: &Value) {
    let (sh, before) = (sh.clone(), m["before_turn"].as_i64());
    tokio::task::spawn_blocking(move || {
        let tools = sh.kept.of::<Tools>().all();
        let facts = sh.kept.of::<CallFacts>();
        let Ok(store) = sh.store.lock() else { return };
        let of_run = |id: &str| crate::ide_diffs::Diffs::of(&sh, &sh.session, id);
        let (messages, earlier, first) = thread_page(&store, &sh.session, before, &tools, &facts, &of_run);
        drop(store);
        send(
            &sh.out,
            json!({"type": "older", "messages": messages, "earlier": earlier, "first_turn": first,
                   "before_turn": before}),
        );
    });
}

/// The workbench's look kept for a folder, as `state` gives it.
fn view_state(data: &Path, root: &Path) -> Value {
    std::fs::read_to_string(view_state_at(data, root))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(Value::Null)
}

/// The session as a page that just connected needs it: who and where, and its last turns.
fn state(p: &Prepared, app: &App, sh: &Shared, running: bool) -> Value {
    let tools = sh.kept.of::<Tools>();
    tools.take_from(p);
    let facts = sh.kept.of::<CallFacts>();
    let (messages, earlier, first_turn) = p
        .store
        .lock()
        .ok()
        .map(|s| {
            let of_run = |id: &str| crate::ide_diffs::Diffs::of(sh, &p.session, id);
            thread_page(&s, &p.session, None, &tools.all(), &facts, &of_run)
        })
        .unwrap_or((Vec::new(), 0, 1));
    let mut v = json!({
        "type": "state",
        "session": p.session,
        "folder": p.folder,
        "root": p.agent.ctx.workspace.root().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "terminals": cfg!(unix),
        // The terminals' shell by its name, for their tabs ("bash", "zsh").
        "shell": app.settings.env().get("SHELL").and_then(|s| s.rsplit('/').next().map(str::to_string)).unwrap_or_else(|| "sh".into()),
        "branch": ca_agent::instructions::git_branch(p.agent.ctx.workspace.root()),
        "theme": theme(app),
        "model": p.agent.model,
        "mode": p.agent.policy.mode.name(),
        "thinking": thinking_name(&p.agent.thinking),
        "models": sh.models.as_slice(),
        "context": context(p),
        "edition": app.edition().name(),
        "version": ca_core::VERSION,
        // The About box: the licence, the source, where the log is kept, and the help's link to the manual.
        "license": LICENSE_NAME,
        "license_text": LICENSE_TEXT,
        "source": ca_core::SOURCE,
        "log": crate::log::path().map(|f| f.display().to_string()),
        "manual": manual(app.connection.url.as_deref()),
        "locked": locked(app.edition()),
        "running": running,
        "messages": messages,
        // How many turns come before those, for 'Show earlier' (`older{before_turn: first_turn}`).
        "earlier": earlier,
        "first_turn": first_turn,
        "view_state": view_state(&app.paths.data, p.agent.ctx.workspace.root()),
    });
    ide_ext::state(sh, Some(p), &mut v);
    v
}

/// Where the workbench's look for a folder is kept (where its views are, its parts' sizes): the data folder's
/// `ide/`, one file per folder, named by the folder's path (FNV-1a, so the same folder finds it again).
fn view_state_at(data: &Path, root: &Path) -> PathBuf {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in root.to_string_lossy().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    data.join("ide").join(format!("{h:016x}.json"))
}

fn keep_view_state(data: &Path, root: &Path, value: &Value) -> std::io::Result<()> {
    let file = view_state_at(data, root);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, serde_json::to_string_pretty(value).unwrap_or_default())
}

pub fn thinking_name(t: &ca_llm::Thinking) -> String {
    match t {
        ca_llm::Thinking::Default => "default".into(),
        ca_llm::Thinking::Off => "off".into(),
        ca_llm::Thinking::Level(l) => l.clone(),
    }
}

/// How full the window is: the ring in the status line.
fn context(p: &Prepared) -> Value {
    let w = &p.agent.window;
    json!({"used": p.agent.context_used(), "size": w.size, "compact_at": w.compact_at})
}

/// What fills the next request, by kind and its biggest messages: the Context panel.
fn context_detail(p: &Prepared) -> Value {
    let b = p.agent.context_breakdown();
    let w = &p.agent.window;
    let kinds = [
        ("Instructions", b.system),
        ("Tools", b.tools),
        ("Your messages", b.you),
        ("Answers", b.answers),
        ("Tool results", b.tool_results),
        ("Notes", b.notes),
    ];
    json!({
        "type": "context_detail",
        "context": context(p),
        "kinds": kinds.iter().map(|(k, c)| json!({"kind": k, "tokens": w.tokens(*c)})).collect::<Vec<_>>(),
        "biggest": b.biggest.iter().map(|(at, role, chars, start)| json!({"at": at, "role": role, "tokens": w.tokens(*chars), "start": start})).collect::<Vec<_>>(),
    })
}

/// A setting changed from the page: checked as the schema says (its kind, where it may be set, the edition's
/// locks), kept in the person's settings.json or one of the project's two (`layer`), for every language or in one
/// language's own block (`language`), and read again. `null` takes it out (its default).
pub fn keep_setting(
    app: &mut App,
    root: &Path,
    key: &str,
    value: &Value,
    layer: ca_settings::Layer,
    language: Option<&str>,
) -> Result<PathBuf, String> {
    let Some(setting) = ca_settings::schema::find(key) else {
        return Err(format!("there is no setting {key}"));
    };
    if let Some(why) = crate::edition::setting_locked(app.edition(), key) {
        return Err(why);
    }
    if layer != ca_settings::Layer::User && !setting.for_project() {
        return Err(format!("{key} is yours alone: a project's settings cannot set it"));
    }
    if let Some(id) = language {
        if !setting.for_language() {
            return Err(format!("{key} cannot be set for one language"));
        }
        if !crate::ide_settings::language_id(id) {
            return Err(format!("{id} is no language's id"));
        }
    }
    if !value.is_null() && !setting.accepts(value) {
        return Err(format!("{key} does not take {value}"));
    }
    let files = crate::ide_settings::Files::of(&app.paths, root);
    let file = files.layer(layer).to_path_buf();
    let done = if value.is_null() {
        ca_settings::settings::forget_for(&file, language, key)
    } else {
        ca_settings::settings::keep_for(&file, language, key, value)
    };
    done.map_err(|e| match e.to_string() {
        said if said.contains(&file.display().to_string()) => said,
        said => format!("{}: {said}", file.display()),
    })?;
    app.settings = files.read(&app.settings);
    Ok(file)
}

/// The colour theme chosen (`workbench.colorTheme`; empty: the system's light or dark).
fn theme(app: &App) -> String {
    app.settings.string(&ca_settings::schema::COLOR_THEME).unwrap_or_default()
}

/// What the status line shows, without the thread (after a turn, or a change: the page keeps its thread).
fn status(p: &Prepared, sh: &Shared) -> Value {
    let mut v = json!({"type": "status", "model": p.agent.model, "mode": p.agent.policy.mode.name(),
           "thinking": thinking_name(&p.agent.thinking), "context": context(p), "running": false,
           "branch": ca_agent::instructions::git_branch(p.agent.ctx.workspace.root())});
    ide_ext::status(sh, Some(p), &mut v);
    v
}

/// A browser in app mode (a window of its own, its own profile), else the system's way to open an address.
fn open(url: &str, data: &Path) -> String {
    let profile = data.join("ide-profile");
    let app_args = |b: &str| {
        (
            b.to_string(),
            vec![format!("--app={url}"), format!("--user-data-dir={}", profile.display()), "--no-first-run".into()],
        )
    };
    let mut tries: Vec<(String, Vec<String>)> = Vec::new();
    if cfg!(target_os = "macos") {
        for b in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ] {
            if Path::new(b).exists() {
                tries.push(app_args(b));
            }
        }
        tries.push(("open".into(), vec![url.into()]));
    } else if cfg!(windows) {
        for b in ["msedge", "chrome", "brave"] {
            tries.push((
                "cmd".into(),
                vec!["/C".into(), "start".into(), String::new(), b.into(), format!("--app={url}")],
            ));
        }
        tries.push(("cmd".into(), vec!["/C".into(), "start".into(), String::new(), url.into()]));
    } else {
        let path = std::env::var_os("PATH").unwrap_or_default();
        for b in
            ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge", "brave-browser"]
        {
            if std::env::split_paths(&path).any(|d| d.join(b).is_file()) {
                tries.push(app_args(b));
            }
        }
        tries.push(("xdg-open".into(), vec![url.into()]));
    }
    for (program, args) in tries {
        let started = std::process::Command::new(&program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if started.is_ok() {
            return program.rsplit(['/', '\\']).next().unwrap_or(&program).to_string();
        }
    }
    String::new()
}

/// Where the address is kept for this run (only the person may read it): a launcher reads it.
fn keep_address(data: &Path, url: &str) -> Option<PathBuf> {
    let file = data.join("ide.json");
    std::fs::create_dir_all(data).ok()?;
    let text = json!({"url": url, "pid": std::process::id()}).to_string();
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::remove_file(&file);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).ok()?;
        f.write_all(text.as_bytes()).ok()?;
    }
    #[cfg(not(unix))]
    std::fs::write(&file, text).ok()?;
    Some(file)
}

/// Where the IDE is shown: the native window (its messages in-process), or a browser (its page and its WebSocket).
pub enum Face {
    Native { from_ui: mpsc::UnboundedReceiver<ca_server::FromPage>, to_ui: broadcast::Sender<String> },
    Browser { port: u16, open: bool },
}

pub struct IdeArgs {
    pub opts: Opts,
    pub resume: Resume,
    pub face: Face,
}

/// Where the driver's messages come from, and the questions a turn waits on.
struct Wires {
    /// From the faces.
    from_page: mpsc::UnboundedReceiver<ca_server::FromPage>,
    /// From the driver itself (an area's background work: `Shared::post`).
    own: mpsc::UnboundedReceiver<Value>,
    pending: Pending,
}

/// Who started a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum How {
    /// The person sent it.
    Typed,
    /// The person sent it while a turn ran, too late for that turn to read. The faces showed it as `queued`
    /// then; it is said again as the question (`user{late}`), so that its answer is one of its own.
    Late,
    /// The session says it itself (`said_next`): a plan's review, a goal's next turn.
    Carried,
}

/// What the session runs next by itself, when nothing waits to be answered: first what the person sent too late
/// for the last turn to read, then what the session says next (`said_next`).
fn next_turn(said_next: &mut Option<String>, inbox: &Mutex<Vec<String>>) -> Option<(String, How)> {
    let late: Vec<String> = inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
    if !late.is_empty() {
        return Some((late.join("\n"), How::Late));
    }
    said_next.take().map(|text| (text, How::Carried))
}

/// Stop ends the chain, and so does a turn that failed: after either the session does not carry on by itself
/// (what it had to say next is dropped), so nothing runs on against the person's word or a model out of reach.
/// What the person sent meanwhile still runs.
fn chain_ends(end: &TurnEnd, said_next: &mut Option<String>) {
    if end.stopped || end.error.is_some() {
        *said_next = None;
    }
}

/// The driver's own messages that need the session's handles only: answered whether a turn runs or not.
/// `false`: not one of them.
fn own_shared(sh: &Shared, m: &Value) -> bool {
    match ide_ext::kind(m) {
        "stop_job" => {
            if let Some(j) = &sh.jobs {
                j.stop(m["id"].as_u64().unwrap_or(0) as u32);
            }
            send(&sh.out, tasks(sh));
        }
        "keep_view_state" => {
            if let Err(e) = keep_view_state(&sh.paths.data, sh.workspace.root(), &m["state"]) {
                ide_ext::say(&sh.out, &format!("The layout was not kept: {e}"));
            }
        }
        // The themes' folders are read off the loop: a turn is not kept waiting for them.
        "themes" => {
            let dirs = crate::ide_themes::folders(&sh.paths.config, sh.home.as_deref(), sh.workspace.root());
            let out = sh.out.clone();
            tokio::task::spawn_blocking(move || {
                send(&out, json!({"type": "themes", "items": crate::ide_themes::all(&dirs)}));
            });
        }
        "older" => older(sh, m),
        _ => return false,
    }
    true
}

/// What became of a message of the driver's own that needs the session.
enum Own {
    /// It is an area's.
    Not,
    Done,
    /// A turn is to run with it (`again`).
    Turn(Sent),
}

/// The driver's own messages that need the session, answered between turns.
async fn own_idle(m: &Value, p: &mut Prepared, app: &mut App, sh: &Shared) -> Own {
    let out = &sh.out;
    match ide_ext::kind(m) {
        "model" => {
            if let Some(name) = m["model"].as_str().filter(|n| !n.is_empty()) {
                p.agent.model = name.to_string();
                let _ = p.store.lock().map(|s| s.set_model(&p.session, name, p.agent.policy.mode.name()));
                if let Some(s) = &p.sync {
                    s.set_model(name);
                }
                send(out, status(p, sh));
            }
        }
        "thinking" => {
            p.agent.thinking = ca_llm::Thinking::parse(m["level"].as_str().unwrap_or(""));
            send(out, status(p, sh));
        }
        "mode" => {
            if let Some(mode) = m["mode"].as_str().and_then(ca_agent::Mode::parse) {
                p.agent.policy.mode = mode;
                send(out, status(p, sh));
            }
        }
        "context" => send(out, context_detail(p)),
        "set_setting" => {
            let key = m["key"].as_str().unwrap_or("");
            let root = p.agent.ctx.workspace.root().to_path_buf();
            // Whose it is: the person's (the default), the project's own file, or VS Code's beside it.
            let layer = match m["where"].as_str() {
                Some("project") => ca_settings::Layer::Project,
                Some("workspace") => ca_settings::Layer::Workspace,
                _ => ca_settings::Layer::User,
            };
            match keep_setting(app, &root, key, &m["value"], layer, m["lang"].as_str()) {
                Ok(file) => {
                    let mut kept = json!({"type": "setting_kept", "key": key, "value": m["value"], "file": file.display().to_string()});
                    for echoed in ["lang", "where"] {
                        if m[echoed].is_string() {
                            kept[echoed] = m[echoed].clone();
                        }
                    }
                    send(out, kept);
                    // Every setting as it is now, for the window's parts that read them.
                    crate::ide_settings::kept(sh, &app.settings);
                    if key == ca_settings::schema::COLOR_THEME.key {
                        send(out, json!({"type": "theme", "theme": theme(app)}));
                    }
                }
                Err(e) => send(out, json!({"type": "setting_refused", "key": key, "error": e})),
            }
        }
        "compact" => {
            let mut events = PageEvents::new(out.clone(), None);
            let said = match p.agent.compact(&mut events, true).await {
                Ok(Some(_)) => "Compacted: the model reads a summary of the earlier part from now on.".to_string(),
                Ok(None) => "Nothing older to summarize.".to_string(),
                Err(e) => format!("The model gave no summary ({e}): the conversation is as it was."),
            };
            ide_ext::say(out, &said);
            send(out, context_detail(p));
            send(out, status(p, sh));
        }
        // Back to before question q: the conversation, the files, or both; again: then that question once more.
        "rewind" | "again" => {
            let again = m["type"] == "again";
            let q = m["q"].as_u64().unwrap_or(0) as usize;
            let what = m["what"].as_str().unwrap_or("both");
            let (code, chat) = if again { (true, true) } else { (what != "chat", what != "code") };
            match session::rewind_to(p, q, code, chat) {
                Ok(r) => {
                    send(out, state(p, app, sh, false));
                    let mut said = vec![format!("Back to before: {}", r.text)];
                    if !r.put_back.is_empty() {
                        said.push(format!("Put back {}.", r.put_back.join(", ")));
                    }
                    if !r.removed.is_empty() {
                        said.push(format!("Removed {} (a turn made it).", r.removed.join(", ")));
                    }
                    if !r.failed.is_empty() {
                        said.push(format!("Could not put back: {}.", r.failed.join("; ")));
                    }
                    ide_ext::say(out, &said.join(" "));
                    let text = m["text"].as_str().unwrap_or("").trim().to_string();
                    if again && !text.is_empty() {
                        return Own::Turn(Sent::new(m, text, false));
                    }
                }
                Err(e) => ide_ext::say(out, &e.to_string()),
            }
        }
        _ => return Own::Not,
    }
    Own::Done
}

/// After a handler had the session in its hands: its handles are taken again, and when the session or the folder
/// is another, the faces get it whole and the areas' background work begins for it.
fn settled(sh: &mut Shared, p: &Prepared, app: &App, drv: &Driver, side: &Arc<Side>) {
    let another = sh.session != p.session || !Arc::ptr_eq(&sh.side, side);
    *sh = Shared::of(p, app, drv, side);
    if another {
        send(&drv.out, state(p, app, sh, false));
        send(&drv.out, tasks(sh));
        ide_ext::start(sh);
    }
}

pub async fn run(app: &mut App, args: IdeArgs) -> anyhow::Result<i32> {
    let ui = app.ui;
    // The face's messages in, the session's out: the same for the native window and a browser's page.
    let (from_page, out, served) = match args.face {
        Face::Native { from_ui, to_ui } => (from_ui, to_ui, None),
        Face::Browser { port, open } => {
            let s = ca_server::start(port, ca_server::built_page()).await?;
            (s.from_page, s.to_pages, Some((s.url, open)))
        }
    };
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let approver: Arc<dyn ca_agent::Approver> =
        Arc::new(PageApprover { out: out.clone(), pending: pending.clone(), next: AtomicU64::new(1) });
    let opts = Opts { interactive: true, ..args.opts.clone() };
    let mut p = session::prepare(app, &opts, approver.clone(), args.resume).await?;
    let inbox = Arc::new(Mutex::new(Vec::<String>::new()));
    p.agent.inbox = Some(inbox.clone());
    let side = crate::ide_files::Side::new(
        p.agent.ctx.workspace.clone(),
        p.agent.ctx.checkpoints.clone(),
        out.clone(),
        app.settings.env().get("SHELL"),
        &app.paths.data,
    );
    let mut side = Arc::new(side);
    side.start_watching();
    ui.say(&format!("{} {} · {} · {}", ui.bold(ca_core::PRODUCT), ca_core::VERSION, p.agent.model, ui.dim(&p.folder)));
    if let Some((url, open_it)) = &served {
        let kept = keep_address(&app.paths.data, url);
        ui.say(&format!("The IDE: {url}"));
        if let Some(f) = kept {
            ui.say(&ui.dim(&format!("(the address is kept in {}; Ctrl+C stops the IDE)", f.display())));
        }
        if *open_it {
            match open(url, &app.paths.data) {
                b if b.is_empty() => ui.say(&ui.yellow("No browser could be opened here: open the address above.")),
                b => ui.say(&ui.dim(&format!("Opened with {b}."))),
            }
        }
    }
    // The models the pickers offer: the gateway's (and the providers' of one's own), asked once.
    let models: Vec<String> =
        match tokio::time::timeout(std::time::Duration::from_secs(15), p.agent.gateway.models()).await {
            Ok(Ok(ms)) => ms.into_iter().map(|m| m.id).collect(),
            _ => vec![p.agent.model.clone()],
        };
    let (post, own) = mpsc::unbounded_channel();
    let mut w = Wires { from_page, own, pending };
    let mut drv = Driver {
        out: out.clone(),
        post,
        approver,
        inbox,
        opts,
        models,
        arena: app.arena().ok(),
        later: Later::default(),
        kept: Kept::default(),
    };
    let mut sh = Shared::of(&p, app, &drv, &side);
    ide_ext::start(&sh);
    loop {
        // What waited for a turn to end comes first, in the order it was asked; then what the session runs next
        // by itself; then what the faces say.
        let msg = if let Some(m) = drv.later.next() {
            m
        } else if let Some((text, how)) = next_turn(&mut p.said_next, &drv.inbox) {
            let mut sent = Sent::new(&Value::Null, text, false);
            if how == How::Late {
                sent.fields.insert("queued".into(), json!(crate::ide_chat::queue::unread(&sh)));
            }
            turn(&mut w, &mut p, app, &mut drv, &mut side, &mut sh, sent, how).await;
            continue;
        } else {
            tokio::select! {
                biased;
                m = w.from_page.recv() => match m {
                    // A name of the driver's own is not a face's to send.
                    Some((_, m)) if !ide_ext::kind(&m).starts_with('_') => m,
                    Some(_) => continue,
                    None => break,
                },
                Some(m) = w.own.recv() => m,
            }
        };
        if side.handle(&msg) {
            continue;
        }
        let name = ide_ext::kind(&msg);
        match ide_ext::rule(name) {
            None | Some(Rule::Side) => {}
            Some(Rule::Turn) => match name {
                "hello" | "state" => {
                    send(&out, state(&p, app, &sh, false));
                    send(&out, tasks(&sh));
                }
                "send" => {
                    // One that waited for the turn's end goes with its words as they are now, or not at all.
                    let text = if msg["late"] == true {
                        let Some(t) = crate::ide_chat::queue::due(&sh, &msg) else { continue };
                        t.trim().to_string()
                    } else {
                        msg["text"].as_str().unwrap_or("").trim().to_string()
                    };
                    if text.is_empty() {
                        continue;
                    }
                    let mut sent = Sent::new(&msg, text, false);
                    // It waited for a turn to end, and the faces showed it as `queued` then.
                    if msg["late"] == true {
                        sent.fields.insert("late".into(), json!(true));
                        if let Some(id) = msg["queued"].as_u64() {
                            sent.fields.insert("queued".into(), json!([id]));
                        }
                    }
                    ide_ext::shape(&sh, &mut sent);
                    if !sent.done {
                        let mut ctx = Ctx { app: &mut *app, p: &mut p, side: &mut side, drv: &mut drv, sh: &sh };
                        ide_ext::command(&mut ctx, &mut sent).await;
                        settled(&mut sh, &p, app, &drv, &side);
                    }
                    if !sent.done {
                        turn(&mut w, &mut p, app, &mut drv, &mut side, &mut sh, sent, How::Typed).await;
                    }
                }
                // A running turn's alone: there is none.
                "answer" | "stop" | "skip" => {}
                _ => {
                    if !own_shared(&sh, &msg) {
                        ide_ext::shared(&sh, &msg);
                    }
                }
            },
            Some(Rule::Queue | Rule::Latest | Rule::Refuse(_)) => {
                let own = own_idle(&msg, &mut p, app, &sh).await;
                if matches!(own, Own::Not) {
                    let mut ctx = Ctx { app: &mut *app, p: &mut p, side: &mut side, drv: &mut drv, sh: &sh };
                    ide_ext::idle(&mut ctx, &msg).await;
                }
                settled(&mut sh, &p, app, &drv, &side);
                if let Own::Turn(mut sent) = own {
                    ide_ext::shape(&sh, &mut sent);
                    if !sent.done {
                        turn(&mut w, &mut p, app, &mut drv, &mut side, &mut sh, sent, How::Typed).await;
                    }
                }
            }
        }
    }
    Ok(0)
}

/// A message that came while a turn runs: answered now, kept for the turn's end or refused, by its rule.
/// `true`: the person stopped the turn.
fn during(m: &Value, sh: &Shared, drv: &mut Driver, pending: &Pending) -> bool {
    if sh.side.handle(m) || !ide_ext::meanwhile(m, &mut drv.later, &drv.out) {
        return false;
    }
    let out = &drv.out;
    match ide_ext::kind(m) {
        "answer" => {
            let id = m["id"].as_u64().unwrap_or(0);
            let answer = match m["answer"].as_str().unwrap_or("") {
                "yes" => Answer::Yes,
                "always" => Answer::Always,
                _ => Answer::No,
            };
            let tx = pending.lock().ok().and_then(|mut g| g.remove(&id));
            if let Some(tx) = tx {
                let _ = tx.send(answer);
                send(out, json!({"type": "answered", "id": id, "answer": m["answer"]}));
            }
        }
        "stop" => return true,
        "skip" => {
            sh.skip.ask();
        }
        // Sent meanwhile: the turn reads it at its next step. A command (`/name`) is not words for the running
        // turn to read: it waits whole for the turn's end and runs then, as in the terminal (and so does what an
        // area says must wait). The faces show either as `queued`; the one that waits is said again as the
        // question when its own turn begins (`user{late}`).
        "send" => {
            let text = m["text"].as_str().unwrap_or("").trim().to_string();
            if !text.is_empty() {
                let mut sent = Sent::new(m, text, true);
                sent.later = sent.model.starts_with('/');
                ide_ext::shape(sh, &mut sent);
                if sent.later {
                    let mut kept = m.clone();
                    kept["late"] = json!(true);
                    if !drv.later.full() {
                        crate::ide_chat::queue::wait(sh, &mut sent, true);
                        kept["queued"] = sent.fields["id"].clone();
                    }
                    if ide_ext::defer(&kept, &mut drv.later, out, false) {
                        send(out, sent.with_fields(json!({"type": "queued", "text": sent.shown, "later": true})));
                    }
                } else if !sent.done {
                    crate::ide_chat::queue::wait(sh, &mut sent, false);
                    send(out, sent.with_fields(json!({"type": "queued", "text": sent.shown})));
                }
            }
        }
        _ => {
            if !own_shared(sh, m) {
                ide_ext::shared(sh, m);
            }
        }
    }
    false
}

/// One turn, and what the faces say while it runs: answers to its questions, Stop, messages sent meanwhile.
#[allow(clippy::too_many_arguments)]
async fn turn(
    w: &mut Wires,
    p: &mut Prepared,
    app: &mut App,
    drv: &mut Driver,
    side: &mut Arc<Side>,
    sh: &mut Shared,
    mut sent: Sent,
    how: How,
) {
    let ui = p.ui;
    let out = drv.out.clone();
    // `/NAME words`: a command or a skill of the project's or yours (`/one /two words`: each of them).
    let message = if how == How::Typed && sent.model.starts_with('/') {
        match crate::ide_chat::library::expand(&ca_agent::library::load(&p.sources), &sent.model) {
            Some(expanded) => expanded,
            None => {
                let name = sent.model.split_whitespace().next().unwrap_or(&sent.model);
                ide_ext::say(&out, &format!("No command or skill {name}."));
                return;
            }
        }
    } else {
        sent.model.clone()
    };
    let q = session::questions(p).map(|qs| qs.len() + 1).unwrap_or(0);
    if how == How::Typed
        && let Some(skill) = skill_of(&sent.model, &message)
    {
        sent.fields.insert("skill".into(), json!(skill));
    }
    if is_note(&sent.shown) {
        sent.fields.insert("note".into(), json!(true));
    }
    // A face that comes while the turn runs is shown the thread before it, then the turn so far said again: both
    // made from what is taken now and kept from here on (the session itself is the turn's until it ends).
    let mut replay = Replay::new(&out);
    let mut cut_shown = false;
    let mut user = match how {
        How::Typed => sent.with_fields(json!({"type": "user", "text": sent.shown, "q": q})),
        How::Carried => json!({"type": "user", "text": sent.shown, "q": q, "carried": true}),
        // The faces showed it as `queued` when it was sent. It is the question of this turn all the same: without
        // it they would write this answer on as the end of the last one.
        How::Late => sent.with_fields(json!({"type": "user", "text": sent.shown, "q": q, "late": true})),
    };
    if sent.fields.contains_key("note") {
        user["note"] = json!(true);
    }
    send(&out, user);
    crate::ide_chat::queue::turn_begins(sh, q);
    // Where the request will be kept, to keep its details beside it once the turn has written it.
    let at = p.store.lock().ok().and_then(|s| s.summary(&p.session).ok()).map(|s| s.messages);
    send(&out, json!({"type": "turn_start"}));
    let before = state(p, app, sh, true);
    ide_ext::before_turn(&mut Ctx { app: &mut *app, p: &mut *p, side: &mut *side, drv: &mut *drv, sh: &*sh }, &message)
        .await;
    session::begin_turn(p, ui, &message).await;
    // A turn the session carries on keeps the steps its goal's budget left it.
    if how != How::Carried {
        p.agent.max_steps = p.max_steps;
    }
    session::before_message(p, &message).await;
    // The session as the turn begins: what answers meanwhile reads this.
    *sh = Shared::of(p, app, drv, side);
    let started = std::time::Instant::now();
    let model = p.agent.model.clone();
    let jobs = p.agent.ctx.jobs.clone();
    let mut events = PageEvents::new(out.clone(), Some(sh.clone()));
    let result = {
        let running = p.agent.turn(&message, &mut events);
        tokio::pin!(running);
        loop {
            let m = tokio::select! {
                r = &mut running => break Some(r),
                Some((_, m)) = w.from_page.recv() => {
                    if ide_ext::kind(&m).starts_with('_') {
                        continue;
                    }
                    m
                }
                Some(m) = w.own.recv() => m,
                () = replay.next(), if !replay.closed => continue,
            };
            // `state{running: true}` with the thread before the turn, then the turn so far, in one go: every face
            // draws its thread again from them (those that saw the turn as well, so none shows it twice).
            if matches!(ide_ext::kind(&m), "hello" | "state") {
                replay.drain();
                let mut v = before.clone();
                v["view_state"] = view_state(&sh.paths.data, sh.workspace.root());
                send(&out, v);
                for said in replay.kept.said() {
                    send(&out, said);
                }
                // What waits now: the turn so far says what was sent, not what was changed or taken back since.
                crate::ide_chat::queue::said(sh, None);
                send(&out, tasks(sh));
                cut_shown |= replay.kept.cut();
                continue;
            }
            if during(&m, sh, drv, &w.pending) {
                break None;
            }
        }
    };
    if let Ok(mut g) = w.pending.lock() {
        g.clear();
    }
    if let Some(at) = at {
        keep_details(p, at, &sent);
    }
    events.thought_ends(result.is_none());
    crate::ide_diffs::Diffs::keep(sh, &p.store);
    send(&out, tasks(sh));
    let took = started.elapsed();
    let ms = took.as_millis() as u64;
    let mut end = TurnEnd { message, outcome: None, stopped: false, error: None, took, carried: how == How::Carried };
    match result {
        Some(Ok(o)) => {
            let used = usage(&o.usage, ms, &model);
            // Kept on the answer, so its footer is shown again after a reload.
            if let Ok(store) = p.store.lock() {
                let _ = store.set_usage(&p.session, &used);
            }
            send(
                &out,
                json!({"type": "turn_end", "ok": true, "ms": ms, "steps": o.steps, "tool_calls": o.tool_calls,
                       "model": model, "usage": used}),
            );
            end.outcome = Some(o);
        }
        Some(Err(e)) => {
            send(&out, json!({"type": "turn_end", "ok": false, "ms": ms, "model": model, "error": e.to_string()}));
            end.error = Some(e.to_string());
        }
        None => {
            if let Some(j) = &jobs
                && let Some(t) = j.current_turn()
            {
                j.stop_turn(t);
            }
            send(&out, json!({"type": "turn_end", "ok": false, "ms": ms, "model": model, "stopped": true}));
            end.stopped = true;
        }
    }
    ide_ext::after_turn(&mut Ctx { app: &mut *app, p: &mut *p, side: &mut *side, drv: &mut *drv, sh: &*sh }, &end)
        .await;
    chain_ends(&end, &mut p.said_next);
    session::keep_window(app, p);
    settled(sh, p, app, drv, side);
    send(&out, status(p, sh));
    // A face was shown the turn without its start (too long to keep): the whole thread now that it is kept.
    if cut_shown {
        send(&out, state(p, app, sh, false));
    }
}

/// Keeps with the request the turn wrote at `at` what it was sent with (the areas' fields) and the words as typed,
/// for a page that connects later. Nothing when it was not written there (the turn was blocked before it began).
fn keep_details(p: &Prepared, at: i64, sent: &Sent) {
    let Ok(store) = p.store.lock() else { return };
    let Ok(page) = store.page(&p.session, 1, None) else { return };
    let written = page.messages.iter().any(|m| m.seq == at && m.message.role == ca_llm::Role::User);
    let mut fields = sent.fields.clone();
    // The key a face asked with answers only that send; a note is told by its words; the numbers it waited under
    // are the run's.
    for k in ["note", "key", "id", "queued"] {
        fields.remove(k);
    }
    if sent.shown != page.messages.iter().find(|m| m.seq == at).map(|m| m.message.content.as_str()).unwrap_or("") {
        fields.insert("typed".into(), json!(sent.shown));
    }
    if written && !fields.is_empty() {
        let _ = store.merge_details(&p.session, at, &fields);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_request_is_shown_with_its_details_as_a_note_or_as_read_meanwhile() {
        let kept = json!({"typed": "explain", "quote": "the part", "text": "not this", "skill": "review"});
        let mut v = json!({"role": "user", "text": "@a.rs\n…\nexplain", "turn": 2, "q": 3});
        question_details(&mut v, true, Some(&kept));
        assert_eq!(v["typed"], "explain");
        assert_eq!(v["quote"], "the part");
        assert_eq!(v["skill"], "review");
        assert_eq!(v["text"], "@a.rs\n…\nexplain", "the words the model read stay");
        assert!(v.get("taken_in").is_none() && v.get("note").is_none());
        // The second request of a turn was read at its next step; a note of Code Arena's is said as such.
        let mut v = json!({"role": "user", "text": "and the tests", "turn": 2});
        question_details(&mut v, false, None);
        assert_eq!(v["taken_in"], true);
        let mut v = json!({"role": "user", "text": "(Code Arena: loop 2's run 1 (x) reports:\nok)", "turn": 2});
        question_details(&mut v, false, None);
        assert_eq!((v["note"].clone(), v.get("taken_in")), (json!(true), None));
    }

    #[test]
    fn a_skill_is_named_only_when_the_model_was_given_its_instructions() {
        assert_eq!(skill_of("/review the parser", "Use the skill review: follow…"), Some("review".into()));
        assert_eq!(skill_of("/explain it", "Explain this: it"), None);
        assert_eq!(skill_of("review", "Use the skill review: follow…"), None);
    }

    #[test]
    fn a_kept_call_carries_its_arguments_its_results_start_and_how_it_ended_in_this_run() {
        let message = |v: Value| -> ca_llm::Message { serde_json::from_value(v).expect("a message") };
        let stored = |m: ca_llm::Message| ca_session::Stored { seq: 0, turn: 1, message: m };
        let step = message(json!({"role": "assistant", "tool_calls": [
            {"id": "a", "name": "read_file", "arguments": "{\"path\":\"src/a.rs\"}"},
            {"id": "b", "name": "run_shell", "arguments": "{\"command\":\"rm -rf x\"}"},
            {"id": "c", "name": "grep", "arguments": "{\"pattern\":\"x\"}"},
        ]}));
        let said_no = format!("{}.", ca_agent::SAID_NO);
        let after = [
            stored(message(json!({"role": "tool", "tool_call_id": "a", "content": "x".repeat(SHOWN_RESULT + 5)}))),
            stored(message(json!({"role": "tool", "tool_call_id": "b", "content": said_no}))),
        ];
        let read = |c: &ca_llm::ToolCall| Read {
            name: c.name.clone(),
            arguments: serde_json::from_str(&c.arguments).unwrap_or(Value::Null),
            subject: "s".into(),
        };
        let fact = |id: &str| (id == "a").then_some(Fact { ok: true, ms: 12, status: "done" });
        let calls = kept_calls(&step, &after, &read, &fact);
        assert_eq!(calls[0]["arguments"]["path"], "src/a.rs");
        assert_eq!((&calls[0]["done"], &calls[0]["cut"], &calls[0]["ms"]), (&json!(true), &json!(true), &json!(12)));
        assert_eq!(calls[0]["text"].as_str().map(|t| t.chars().count()), Some(SHOWN_RESULT));
        // Said no to, though not in this run: the result's words say it.
        assert_eq!((&calls[1]["ok"], &calls[1]["status"]), (&json!(false), &json!("denied")));
        // No result: it did not run, and says nothing of how it ended.
        assert_eq!(calls[2]["done"], false);
        assert!(calls[2].get("ok").is_none() && calls[2].get("text").is_none());
    }

    #[test]
    fn a_calls_long_argument_is_shown_by_its_start_and_the_rest_as_it_is() {
        let long = "y".repeat(SHOWN_ARGUMENT + 1);
        let v = shown_arguments(&json!({"content": long, "n": 3, "list": ["a"]}));
        assert!(v["content"].as_str().is_some_and(|s| s.ends_with('…') && s.chars().count() == SHOWN_ARGUMENT + 1));
        assert_eq!((&v["n"], &v["list"]), (&json!(3), &json!(["a"])));
    }

    #[test]
    fn the_session_runs_what_came_too_late_first_then_what_it_says_next_itself() {
        let inbox = Mutex::new(vec!["and the tests".to_string(), "then commit".to_string()]);
        let mut said = Some("Goal (turn 2): make it pass".to_string());
        // What the person sent while the last turn wrote its last words, as one message of its own.
        assert_eq!(next_turn(&mut said, &inbox), Some(("and the tests\nthen commit".into(), How::Late)));
        assert!(inbox.lock().expect("lock").is_empty(), "taken out: the turn's first step does not read it again");
        assert!(said.is_some(), "what the session says next waits its turn");
        assert_eq!(next_turn(&mut said, &inbox), Some(("Goal (turn 2): make it pass".into(), How::Carried)));
        assert_eq!(next_turn(&mut said, &inbox), None, "then the faces are listened to again");
    }

    #[test]
    fn a_turn_stopped_or_failed_ends_the_chain_but_not_what_the_person_sent_meanwhile() {
        let end = |stopped: bool, error: Option<&str>| TurnEnd {
            message: "Goal (turn 2): make it pass".into(),
            outcome: None,
            stopped,
            error: error.map(str::to_string),
            took: std::time::Duration::from_secs(1),
            carried: true,
        };
        let next = || Some("Goal (turn 3): make it pass".to_string());
        let mut said = next();
        chain_ends(&end(false, None), &mut said);
        assert!(said.is_some(), "a turn that ended by itself carries on");
        chain_ends(&end(false, Some("the gateway is out of reach")), &mut said);
        assert_eq!(said, None, "not on against a model that does not answer");
        let inbox = Mutex::new(vec!["no, the other file".to_string()]);
        let mut said = next();
        chain_ends(&end(true, None), &mut said);
        assert_eq!(next_turn(&mut said, &inbox), Some(("no, the other file".into(), How::Late)));
        assert_eq!(next_turn(&mut said, &inbox), None, "the session says nothing more by itself");
    }

    #[test]
    fn the_about_boxs_fields_and_what_this_edition_locks() {
        assert_eq!(manual(Some("https://arena.example/")), json!("https://arena.example/help/code-arena"));
        assert_eq!(manual(None), Value::Null);
        assert!(LICENSE_TEXT.starts_with("MIT License"), "the licence's own text");
        let standalone = locked(ca_settings::Edition::Standalone);
        assert!(standalone["/web"].as_str().is_some_and(|why| why.contains("managed edition")), "{standalone}");
        assert!(standalone["arena.syncChats"].as_str().is_some_and(|why| why.contains("Farzan Arena")));
        assert_eq!(locked(ca_settings::Edition::Managed), json!({}));
    }

    #[test]
    fn the_workbenchs_look_is_kept_for_its_folder() {
        let data = tempfile::tempdir().expect("data");
        let look = json!({"layout": {"primary": ["files"]}});
        keep_view_state(data.path(), Path::new("/work/one"), &look).expect("kept");
        let file = view_state_at(data.path(), Path::new("/work/one"));
        assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(&file).expect("read")).expect("json"), look);
        assert_ne!(file, view_state_at(data.path(), Path::new("/work/two")), "another folder, another file");
    }
}
