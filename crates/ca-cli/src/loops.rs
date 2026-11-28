//! Loops: a prompt run every so often as a background sub-agent while the session is open (`/loop 30m PROMPT`). A
//! run never overlaps the one before it (it is skipped, and counted); its report reaches the agent with the person's
//! next message, and the person sees a line of it at the prompt. Nobody is there to answer a question, so a loop only
//! reads, or with `--edits` edits files too (its commands are refused). Loops end with the session.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ca_tools::Tool;

use crate::schedules::{interval, interval_words};
use crate::session::Prepared;

pub struct Loop {
    pub id: u32,
    pub every: u64,
    pub prompt: String,
    pub edits: bool,
    pub runs: Arc<AtomicU32>,
    pub skipped: Arc<AtomicU32>,
    /// How its last run ended: 0 none has, 1 it did its work, 2 it failed.
    pub last: Arc<AtomicU8>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Loop {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The loops' reports, waiting for the person's next message.
pub type Reports = Arc<Mutex<Vec<String>>>;

/// `/loop`, `/loop EVERY [--edits] PROMPT`, `/loop stop N`.
pub fn command(rest: &str, p: &mut Prepared) -> String {
    let words: Vec<&str> = rest.split_whitespace().collect();
    match words.as_slice() {
        [] => {
            if p.loops.is_empty() {
                return "No loops. /loop 30m PROMPT runs PROMPT every 30 minutes while this session is open.".into();
            }
            p.loops
                .iter()
                .map(|l| {
                    format!(
                        "{}. every {}{}: {} ({} runs, {} skipped while one ran)",
                        l.id,
                        interval_words(l.every),
                        if l.edits { ", editing" } else { "" },
                        l.prompt,
                        l.runs.load(Ordering::Relaxed),
                        l.skipped.load(Ordering::Relaxed)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        ["stop", n] => match n.parse::<u32>() {
            Ok(n) if stop(p, n) => format!("Loop {n} stopped."),
            _ => format!("No loop {n}: /loop lists them."),
        },
        [every, rest @ ..] => {
            let Some(secs) = interval(every) else {
                return "/loop 30m PROMPT: how often first (90s, 30m, 2h, 1d).".into();
            };
            if secs < 60 {
                return "A loop runs at most every minute.".into();
            }
            let edits = rest.first() == Some(&"--edits");
            let prompt = rest.iter().skip(usize::from(edits)).copied().collect::<Vec<_>>().join(" ");
            if prompt.is_empty() {
                return "/loop 30m PROMPT: and what it does.".into();
            }
            let id = add(p, secs, prompt.clone(), edits);
            format!(
                "Loop {id}: every {}, {}: {prompt}. Its first run starts now; /loop stop {id} ends it.",
                interval_words(secs),
                if edits { "editing files (its commands are refused)" } else { "reading only" }
            )
        }
    }
}

/// A loop added (its first run starts now): its number. The crew is the session's as it is now
/// (`session::refresh_crew` first).
pub fn add(p: &mut Prepared, secs: u64, prompt: String, edits: bool) -> u32 {
    let id = p.loops.iter().map(|l| l.id).max().unwrap_or(0) + 1;
    let (runs, skipped, last) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)), Arc::new(AtomicU8::new(0)));
    let task = start(p, id, secs, prompt.clone(), edits, [runs.clone(), skipped.clone()], last.clone());
    p.loops.push(Loop { id, every: secs, prompt, edits, runs, skipped, last, task });
    id
}

/// Loop `id` stopped; false when there is none of that number.
pub fn stop(p: &mut Prepared, id: u32) -> bool {
    let had = p.loops.len();
    p.loops.retain(|l| l.id != id);
    p.loops.len() != had
}

fn start(
    p: &Prepared,
    id: u32,
    secs: u64,
    prompt: String,
    edits: bool,
    [runs, skipped]: [Arc<AtomicU32>; 2],
    last: Arc<AtomicU8>,
) -> tokio::task::JoinHandle<()> {
    let crew = p.crew.clone();
    let ctx = p.agent.ctx.clone();
    let reports = p.loop_reports.clone();
    let notices = p.notices.clone();
    let busy = Arc::new(AtomicBool::new(false));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(secs));
        loop {
            tick.tick().await;
            if busy.swap(true, Ordering::AcqRel) {
                skipped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            // Its own crew: nobody answers a question, so it reads only (or edits, with no command).
            let own = crew.lock().ok().and_then(|c| c.clone()).map(|mut c| {
                c.policy.mode = if edits { ca_agent::Mode::AutoEdit } else { ca_agent::Mode::Plan };
                c.approver = Arc::new(ca_agent::NoAsking);
                c.say = Arc::new(|_| {});
                c
            });
            let Some(own) = own else {
                busy.store(false, Ordering::Release);
                continue;
            };
            let (r, n, b, prompt, ctx) = (reports.clone(), notices.clone(), busy.clone(), prompt.clone(), ctx.clone());
            let (runs, last) = (runs.clone(), last.clone());
            tokio::spawn(async move {
                let t = ca_agent::task::TaskTool::new(Arc::new(Mutex::new(Some(own))));
                let o = t.run(serde_json::json!({"description": format!("loop {id}"), "prompt": prompt}), &ctx).await;
                let k = runs.fetch_add(1, Ordering::Relaxed) + 1;
                last.store(if o.is_error { 2 } else { 1 }, Ordering::Relaxed);
                let first = o.text.lines().nth(1).unwrap_or("").chars().take(120).collect::<String>();
                if let Ok(mut q) = r.lock() {
                    q.push(format!("(Code Arena: loop {id}'s run {k} ({prompt}) reports:\n{})", o.text));
                }
                if let Ok(mut q) = n.lock() {
                    q.push((o.is_error, format!("Loop {id} ran ({k}): {first}")));
                }
                b.store(false, Ordering::Release);
            });
        }
    })
}
