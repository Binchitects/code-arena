//! The Tasks panel's side of the driver: the sub-agents a turn started, each with how it goes.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn). Before every turn it gives the session's crew a watch
//! (`ca_agent::task::Crew::watch`), which keeps each sub-agent in a book of the driver's and tells the faces of it.
//!
//! Messages:
//! - `agent_tasks` [T]: answered by `agent_tasks{items, running, most}`: the session's tasks (newest first), how
//!   many are at work and how many may work at once.
//! - `agent_task{id}` [T]: answered by `agent_task{id, …its row, prompt, tools, report, error, work}` (`work`: its
//!   steps, each a list of pieces `{kind: thinking | text | notice | tool, …}`), or `agent_task{id, error}`.
//! - `agent_task_stop{id}` [T]: the sub-agent stops where it is; answered by `agent_task_stop{id, text | error}`,
//!   and its `agent` event follows once it has.
//!
//! Event: `agent{id, call_id, title, assistant, status: working | done | failed | stopped, now, began, ms, tokens,
//! calls, model, edits, yours}`, when a sub-agent begins, at each piece of its work (at most every
//! `ca_agent::task::SAY_EVERY` while it streams) and when it ends. `now` is what it is doing ("" once it ended),
//! `began` when it began (milliseconds since 1970), `ms` how long it has worked, `tokens` how full its window got.

use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ca_agent::task::{Begun, End, Stop, SubEvent, Work};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::ide_ext::{Area, Ctx, Rule, Shared, send};

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] =
        &[("agent_tasks", Rule::Turn), ("agent_task", Rule::Turn), ("agent_task_stop", Rule::Turn)];

    fn shared(sh: &Shared, m: &Value) -> bool {
        let book = sh.kept.of::<Book>();
        let id = m["id"].as_str().unwrap_or("");
        let reply = match crate::ide_ext::kind(m) {
            "agent_tasks" => book.with(|b| list(b, &sh.session)),
            "agent_task" => book.with(|b| detail(b, &sh.session, id)),
            "agent_task_stop" => book.with(|b| stop(b, &sh.session, id)),
            _ => return false,
        };
        send(&sh.out, reply);
        true
    }

    async fn before_turn(ctx: &mut Ctx<'_>, _message: &str) {
        let watch = watch(ctx.sh.kept.of::<Book>(), ctx.sh.out.clone(), ctx.p.session.clone());
        if let Ok(mut c) = ctx.p.crew.lock()
            && let Some(crew) = c.as_mut()
        {
            crew.watch = Some(watch);
        }
    }
}

/// How many sub-agents work at once (the `task` tool's own limit).
const MOST_AT_ONCE: usize = 4;
/// The most tasks the book keeps: the oldest that ended go first.
const MOST_KEPT: usize = 200;
/// The most of a prompt, a report or a run of thinking or words kept.
const MOST_TEXT: usize = 20_000;

/// The sub-agents the driver heard of, in every session it ran.
#[derive(Default)]
pub struct Book(Mutex<Vec<Task>>);

impl Book {
    fn with<R>(&self, f: impl FnOnce(&mut Vec<Task>) -> R) -> R {
        match self.0.lock() {
            Ok(mut g) => f(&mut g),
            Err(e) => f(&mut e.into_inner()),
        }
    }
}

/// A sub-agent as the book keeps it.
pub struct Task {
    id: String,
    session: String,
    title: String,
    assistant: String,
    prompt: String,
    tools: Vec<String>,
    model: String,
    edits: bool,
    began: Instant,
    began_at: u64,
    ms: u64,
    status: &'static str,
    now: String,
    tokens: u64,
    calls: u64,
    report: String,
    error: Option<String>,
    /// Its steps, each its pieces.
    work: Vec<Vec<Value>>,
    /// Its last step's answer is complete: its calls are still that step's, its next words begin another.
    step_ended: bool,
    stop: Arc<Stop>,
}

/// The watch the session's crew is given for a turn: each sub-agent's news, kept and told.
fn watch(book: Arc<Book>, out: broadcast::Sender<String>, session: String) -> ca_agent::task::Watch {
    Arc::new(move |e: SubEvent| {
        if let Some(event) = book.with(|b| hear(b, &session, e)) {
            send(&out, event);
        }
    })
}

fn cut(text: &str) -> String {
    match text.char_indices().nth(MOST_TEXT) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    }
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A sub-agent's news, put in the book; the `agent` event that tells it.
fn hear(book: &mut Vec<Task>, session: &str, e: SubEvent) -> Option<Value> {
    let call = match &e {
        SubEvent::Started(b) => b.call.clone(),
        SubEvent::Step { call, .. } | SubEvent::Ended { call, .. } => call.clone(),
    };
    match e {
        SubEvent::Started(b) => {
            book.retain(|t| !(t.id == b.call && t.session == session));
            book.push(begun(b, session));
            while book.len() > MOST_KEPT {
                match book.iter().position(|t| t.status != "working") {
                    Some(i) => book.remove(i),
                    None => break,
                };
            }
        }
        SubEvent::Step { work, .. } => {
            let t = book.iter_mut().find(|t| t.id == call && t.session == session)?;
            step(t, work);
        }
        SubEvent::Ended { end, .. } => {
            let t = book.iter_mut().find(|t| t.id == call && t.session == session)?;
            ended(t, end);
        }
    }
    book.iter().find(|t| t.id == call && t.session == session).map(row)
}

fn begun(b: Begun, session: &str) -> Task {
    Task {
        id: b.call,
        session: session.to_string(),
        title: b.description,
        assistant: b.name,
        prompt: cut(&b.prompt),
        tools: b.tools,
        model: b.model,
        edits: b.edits,
        began: Instant::now(),
        began_at: unix_ms(),
        ms: 0,
        status: "working",
        now: "Starting…".into(),
        tokens: 0,
        calls: 0,
        report: String::new(),
        error: None,
        work: vec![vec![]],
        step_ended: false,
        stop: b.stop,
    }
}

/// A piece of work, put in its step: thinking and words that follow their own kind are one piece. A step's calls
/// run once its answer is complete, so the next step begins with the next thinking, words or notice.
fn step(t: &mut Task, work: Work) {
    if t.status != "working" {
        return;
    }
    t.ms = t.began.elapsed().as_millis() as u64;
    let begins = matches!(work, Work::Thinking(_) | Work::Text(_) | Work::Notice(_));
    if t.work.is_empty() || (begins && t.step_ended) {
        t.work.push(vec![]);
        t.step_ended = false;
    }
    let Some(pieces) = t.work.last_mut() else { return };
    let mut said = |kind: &str, text: String| match pieces.last_mut() {
        Some(p) if p["kind"] == kind => {
            let all = format!("{}{text}", p["text"].as_str().unwrap_or(""));
            p["text"] = json!(cut(&all));
        }
        _ => pieces.push(json!({"kind": kind, "text": cut(&text)})),
    };
    match work {
        Work::Thinking(text) => {
            said("thinking", text);
            t.now = "Thinking…".into();
        }
        Work::Text(text) => {
            said("text", text);
            t.now = "Writing…".into();
        }
        Work::Notice(text) => pieces.push(json!({"kind": "notice", "text": cut(&text)})),
        Work::ToolStart { id, name, subject } => {
            let first: String = subject.lines().next().unwrap_or("").chars().take(120).collect();
            t.now = if first.is_empty() { name.clone() } else { format!("{name} {first}") };
            pieces.push(json!({"kind": "tool", "id": id, "name": name, "subject": subject, "done": false}));
        }
        Work::ToolEnd { id, ok, text, ms } => {
            t.calls += 1;
            if let Some(p) = pieces.iter_mut().rev().find(|p| p["kind"] == "tool" && p["id"] == id.as_str()) {
                p["done"] = json!(true);
                p["ok"] = json!(ok);
                p["text"] = json!(text);
                p["ms"] = json!(ms);
            }
        }
        Work::StepEnd { tokens } => {
            t.tokens = t.tokens.max(tokens);
            t.step_ended = true;
        }
    }
}

fn ended(t: &mut Task, end: End) {
    if t.status != "working" {
        return;
    }
    t.ms = t.began.elapsed().as_millis() as u64;
    t.status = end.status;
    t.now = String::new();
    t.report = cut(&end.report);
    t.error = end.error;
    t.work.retain(|s| !s.is_empty());
}

/// A task as its row shows it, and as its `agent` event says it.
fn row(t: &Task) -> Value {
    let ms = if t.status == "working" { t.began.elapsed().as_millis() as u64 } else { t.ms };
    json!({
        "type": "agent",
        "id": t.id,
        "call_id": t.id,
        "title": t.title,
        "assistant": t.assistant,
        "status": t.status,
        "now": t.now,
        "began": t.began_at,
        "ms": ms,
        "tokens": t.tokens,
        "calls": t.calls,
        "model": t.model,
        "edits": t.edits,
        "yours": false,
    })
}

fn list(book: &[Task], session: &str) -> Value {
    let mine: Vec<&Task> = book.iter().filter(|t| t.session == session).collect();
    let items: Vec<Value> = mine
        .iter()
        .rev()
        .map(|t| {
            let mut r = row(t);
            r.as_object_mut().map(|o| o.remove("type"));
            r
        })
        .collect();
    let running = mine.iter().filter(|t| t.status == "working").count();
    json!({"type": "agent_tasks", "items": items, "running": running, "most": MOST_AT_ONCE})
}

fn detail(book: &[Task], session: &str, id: &str) -> Value {
    let Some(t) = book.iter().find(|t| t.id == id && t.session == session) else {
        return json!({"type": "agent_task", "id": id, "error": "There is no such task in this session."});
    };
    let mut v = row(t);
    v["type"] = json!("agent_task");
    v["prompt"] = json!(t.prompt);
    v["tools"] = json!(t.tools);
    v["report"] = json!(t.report);
    v["error"] = json!(t.error);
    v["work"] = json!(t.work);
    v
}

fn stop(book: &[Task], session: &str, id: &str) -> Value {
    match book.iter().find(|t| t.id == id && t.session == session) {
        Some(t) if t.status == "working" => {
            t.stop.ask();
            json!({"type": "agent_task_stop", "id": id, "text": format!("Stopping {}…", t.title)})
        }
        Some(_) => json!({"type": "agent_task_stop", "id": id, "error": "That task has ended already."}),
        None => json!({"type": "agent_task_stop", "id": id, "error": "There is no such task in this session."}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(call: &str) -> SubEvent {
        SubEvent::Started(Begun {
            call: call.into(),
            name: "reviewer".into(),
            description: "Review the parser".into(),
            prompt: "Read src/parse.rs and report.".into(),
            tools: vec!["read_file".into(), "grep".into()],
            model: "qwen".into(),
            edits: false,
            stop: Arc::default(),
        })
    }

    fn step_of(call: &str, work: Work) -> SubEvent {
        SubEvent::Step { call: call.into(), work }
    }

    #[test]
    fn a_sub_agent_is_kept_from_its_start_to_its_end() {
        let mut b = Vec::new();
        let e = hear(&mut b, "s1", started("c1")).expect("told");
        assert_eq!(
            (e["type"].as_str(), e["status"].as_str(), e["now"].as_str()),
            (Some("agent"), Some("working"), Some("Starting…"))
        );
        assert_eq!(
            (e["title"].as_str(), e["assistant"].as_str(), e["yours"].as_bool()),
            (Some("Review the parser"), Some("reviewer"), Some(false))
        );
        hear(&mut b, "s1", step_of("c1", Work::Thinking("Let me ".into())));
        hear(&mut b, "s1", step_of("c1", Work::Thinking("look.".into())));
        // The answer is complete before its calls run.
        hear(&mut b, "s1", step_of("c1", Work::StepEnd { tokens: 3400 }));
        let e = hear(
            &mut b,
            "s1",
            step_of(
                "c1",
                Work::ToolStart { id: "r1".into(), name: "read_file".into(), subject: "src/parse.rs".into() },
            ),
        )
        .expect("told");
        assert_eq!(e["now"], "read_file src/parse.rs");
        hear(
            &mut b,
            "s1",
            step_of("c1", Work::ToolEnd { id: "r1".into(), ok: true, text: "fn parse()".into(), ms: 12 }),
        );
        hear(&mut b, "s1", step_of("c1", Work::Text("It is fine.".into())));
        let e = hear(
            &mut b,
            "s1",
            SubEvent::Ended {
                call: "c1".into(),
                end: End { status: "done", report: "It is fine.".into(), error: None },
            },
        )
        .expect("told");
        assert_eq!(
            (e["status"].as_str(), e["now"].as_str(), e["calls"].as_u64(), e["tokens"].as_u64()),
            (Some("done"), Some(""), Some(1), Some(3400))
        );
        let d = detail(&b, "s1", "c1");
        assert_eq!(d["report"], "It is fine.");
        assert_eq!(
            d["work"][0][0],
            json!({"kind": "thinking", "text": "Let me look."}),
            "thinking in a row is one piece"
        );
        assert_eq!(d["work"][0][1]["ok"], true, "a step's calls are that step's");
        assert_eq!(d["work"][1][0]["kind"], "text");
        assert_eq!(d["work"].as_array().map(Vec::len), Some(2));
        // A step after its end changes nothing.
        assert_eq!(
            hear(&mut b, "s1", step_of("c1", Work::Text("more".into()))).map(|e| e["status"].clone()),
            Some(json!("done"))
        );
        assert_eq!(detail(&b, "s1", "c1")["work"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn the_list_is_the_sessions_own_newest_first() {
        let mut b = Vec::new();
        hear(&mut b, "s1", started("c1"));
        hear(&mut b, "s2", started("c9"));
        hear(&mut b, "s1", started("c2"));
        hear(
            &mut b,
            "s1",
            SubEvent::Ended {
                call: "c1".into(),
                end: End { status: "failed", report: String::new(), error: Some("no".into()) },
            },
        );
        let l = list(&b, "s1");
        let ids: Vec<&str> = l["items"].as_array().into_iter().flatten().filter_map(|i| i["id"].as_str()).collect();
        assert_eq!(ids, ["c2", "c1"]);
        assert_eq!((l["running"].as_u64(), l["most"].as_u64()), (Some(1), Some(4)));
        assert!(l["items"][0].get("type").is_none());
        assert!(
            hear(&mut b, "s1", step_of("c9", Work::Text("x".into()))).is_none(),
            "another session's task is not this one's"
        );
        assert_eq!(detail(&b, "s1", "c9")["error"], "There is no such task in this session.");
    }

    #[test]
    fn a_task_at_work_is_stopped_once() {
        let mut b = Vec::new();
        hear(&mut b, "s1", started("c1"));
        let r = stop(&b, "s1", "c1");
        assert_eq!(r["text"], "Stopping Review the parser…");
        assert!(b[0].stop.asked());
        hear(
            &mut b,
            "s1",
            SubEvent::Ended {
                call: "c1".into(),
                end: End { status: "stopped", report: String::new(), error: Some("Stopped by you.".into()) },
            },
        );
        assert_eq!(stop(&b, "s1", "c1")["error"], "That task has ended already.");
        assert_eq!(stop(&b, "s1", "zz")["error"], "There is no such task in this session.");
    }
}
