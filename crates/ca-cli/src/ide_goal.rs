//! Goal mode and the loops in the IDE: a goal set, paused, carried on turn after turn; a loop added or stopped.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn). The work itself is the terminal's (`goals.rs`,
//! `loops.rs`): after each turn the goal's check runs and its next turn is what the session says next
//! (`said_next`), while it is worked on.
//!
//! Messages:
//! - `goal_set{text, check: {kind: command | condition, text} | null, budget: {steps, minutes, tokens, until_met},
//!   after}` [Q]: answered by `goal_set{said | error}` (with the face's `id`). The goal is worked on at once, or waits
//!   its turn behind the one worked on. No check: the agent writes one down first.
//! - `goal_pause`, `goal_resume`, `goal_cancel`, `goal_next` (the first waiting started), `goal_drop{place}` (from 1)
//!   [T]: answered by a message of the same name with `said` or `error` (and `place`).
//! - `loops` [T]: answered by `loops{items: [{id, every, every_words, prompt, edits, runs, skipped, last}]}` (`last`:
//!   "ok", "failed" or null).
//! - `loop_add{every: "15m" | "30m" | "1h" | "4h" | "1d" | ..., prompt, edits}`, `loop_stop{id}` [Q]: answered by
//!   `loop_add{every, id, said | error}` or `loop_stop{id, said | error}`, then `loops`.
//! - `/goal ...` and `/loop ...` typed in the box are the terminal's commands, answered as a notice.
//!
//! After each goal message, `tasks` is said again: its `goal` carries the whole goal (`check`, `budget`, `steps`,
//! `tokens`, `ms`, `passed`, `said`, `outcome` beside `text`, `status`, `turns`) and `next` the goals waiting.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use ca_agent::goal::{Budget, Check, Goal};
use serde_json::{Value, json};

use crate::ide_ext::{Area, Ctx, Rule, Sent, Shared, TurnEnd, kind, say, send};

/// The longest goal, loop prompt and check the faces may send.
pub const MOST_TEXT: usize = 4_000;
/// The bounds of a budget: steps, minutes, and the fewest tokens.
pub const MOST_STEPS: u64 = 100_000;
pub const MOST_MINUTES: u64 = 43_200;
pub const FEWEST_TOKENS: u64 = 10_000;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("goal_set", Rule::Queue),
        ("goal_pause", Rule::Turn),
        ("goal_resume", Rule::Turn),
        ("goal_cancel", Rule::Turn),
        ("goal_next", Rule::Turn),
        ("goal_drop", Rule::Turn),
        ("_goal_carry", Rule::Latest),
        ("loops", Rule::Turn),
        ("loop_add", Rule::Queue),
        ("loop_stop", Rule::Queue),
    ];

    /// Another session (or folder): its loops are its own.
    fn start(sh: &Shared) {
        let had = sh.kept.of::<LoopsSeen>().0.lock().is_ok_and(|s| !s.is_empty());
        set_seen(sh, Vec::new());
        // The faces are told only when loops went with the session that had them (a page's first word is `state`).
        if had {
            send(&sh.out, loops_message(sh));
        }
    }

    fn shared(sh: &Shared, m: &Value) -> bool {
        let k = kind(m);
        if k == "loops" {
            send(&sh.out, loops_message(sh));
            return true;
        }
        let place = m["place"].as_u64().unwrap_or(0) as usize;
        let done = match k {
            "goal_pause" => sh.goals.change(crate::goals::pause),
            "goal_resume" => sh.goals.change(crate::goals::resume),
            "goal_cancel" => sh.goals.change(crate::goals::cancel),
            "goal_next" => sh.goals.change(crate::goals::start_next),
            "goal_drop" => sh.goals.change(|g| crate::goals::drop_waiting(g, place)),
            _ => return false,
        };
        let mut reply = said(k, done.clone());
        if k == "goal_drop" {
            reply["place"] = json!(place);
        }
        send(&sh.out, reply);
        // A goal worked on again is carried on between turns (while a turn runs, its end carries it on).
        if done.is_ok() && matches!(k, "goal_resume" | "goal_next") {
            sh.post(json!({"type": "_goal_carry"}));
        }
        send(&sh.out, crate::ide::tasks(sh));
        true
    }

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        let out = ctx.sh.out.clone();
        match kind(m) {
            "_goal_carry" => carry(ctx),
            "goal_set" => {
                let mut reply = match goal_of(m) {
                    Ok(goal) => said("goal_set", ctx.p.goals.change(|g| g.set(goal))),
                    Err(e) => said("goal_set", Err(e)),
                };
                if let Some(id) = m.get("id").filter(|v| !v.is_null()) {
                    reply["id"] = id.clone();
                }
                send(&out, reply);
                carry(ctx);
                send(&out, crate::ide::tasks(ctx.sh));
            }
            "loop_add" => {
                let every = m["every"].as_str().unwrap_or("").to_string();
                let mut reply = match loop_of(m) {
                    Ok((secs, prompt, edits)) => {
                        crate::session::refresh_crew(ctx.p, ctx.p.ui);
                        let id = crate::loops::add(ctx.p, secs, prompt, edits);
                        json!({"type": "loop_add", "id": id, "said": format!("Loop {id} added: its first run starts now.")})
                    }
                    Err(e) => json!({"type": "loop_add", "error": e}),
                };
                reply["every"] = json!(every);
                send(&out, reply);
                loops_changed(ctx);
            }
            "loop_stop" => {
                let id = m["id"].as_u64().unwrap_or(0);
                let stopped = u32::try_from(id).is_ok_and(|n| crate::loops::stop(ctx.p, n));
                let done = if stopped { Ok(format!("Loop {id} stopped.")) } else { Err(format!("No loop {id}.")) };
                let mut reply = said("loop_stop", done);
                reply["id"] = json!(id);
                send(&out, reply);
                loops_changed(ctx);
            }
            _ => return false,
        }
        true
    }

    /// `/goal ...` and `/loop ...`, as in the terminal.
    async fn command(ctx: &mut Ctx<'_>, sent: &mut Sent) {
        let text = sent.model.trim().to_string();
        let (name, rest) = text.split_once(char::is_whitespace).unwrap_or((&text, ""));
        let rest = rest.trim();
        let out = ctx.sh.out.clone();
        match name {
            "/goal" => {
                say(&out, &crate::goals::command(rest, ctx.p));
                // A goal set, resumed or started next is worked on at once.
                if !rest.is_empty() && !matches!(rest, "pause" | "cancel") && !rest.starts_with("drop") {
                    carry(ctx);
                }
                send(&out, crate::ide::tasks(ctx.sh));
            }
            "/loop" => {
                crate::session::refresh_crew(ctx.p, ctx.p.ui);
                say(&out, &crate::loops::command(rest, ctx.p));
                loops_changed(ctx);
            }
            _ => return,
        }
        sent.done = true;
    }

    /// The goal's check after the turn, and its next turn while it is worked on.
    async fn after_turn(ctx: &mut Ctx<'_>, end: &TurnEnd) {
        if !ctx.p.goals.get().working() {
            return;
        }
        let out = ctx.sh.out.clone();
        let next = crate::goals::after_turn(ctx.p, end.outcome.as_ref(), end.took, &|s| say(&out, &s)).await;
        if let Some(next) = next
            && ctx.p.said_next.is_none()
        {
            ctx.p.said_next = Some(next);
        }
        send(&out, crate::ide::tasks(ctx.sh));
    }

    /// The whole goal, and the goals waiting after it.
    fn tasks(sh: &Shared, v: &mut Value) {
        let goals = sh.goals.get();
        if let (Some(g), Some(o)) = (&goals.current, v["goal"].as_object_mut())
            && let Value::Object(more) = goal_json(g)
        {
            for (k, f) in more {
                o.entry(k).or_insert(f);
            }
        }
        v["next"] = Value::Array(goals.next.iter().map(goal_json).collect());
    }
}

/// The goal worked on, carried on as the next turn when nothing else is said next.
fn carry(ctx: &mut Ctx<'_>) {
    if ctx.p.said_next.is_none() {
        ctx.p.said_next = ctx.p.goals.get().prompt();
    }
}

/// A reply of `kind` with what was said, or what was wrong.
fn said(kind: &str, done: Result<String, String>) -> Value {
    match done {
        Ok(s) => json!({"type": kind, "said": s}),
        Err(e) => json!({"type": kind, "error": e}),
    }
}

/// A goal as the faces read it.
fn goal_json(g: &Goal) -> Value {
    json!({
        "text": g.text, "status": g.status, "turns": g.turns,
        "check": g.check.as_ref().map(|c| json!({"kind": c.kind, "text": c.text, "by": c.by})),
        "budget": {"steps": g.budget.steps, "minutes": g.budget.minutes, "tokens": g.budget.tokens,
                   "until_met": g.budget.until_met},
        "steps": g.steps, "tokens": g.tokens, "ms": g.ms, "passed": g.passed, "said": g.said, "outcome": g.outcome,
    })
}

/// The goal a `goal_set` asks for, or what is wrong with it.
pub fn goal_of(m: &Value) -> Result<Goal, String> {
    let text = m["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        return Err("Say what the goal is.".into());
    }
    if text.chars().count() > MOST_TEXT {
        return Err("A goal is at most 4,000 characters.".into());
    }
    let check = match &m["check"] {
        Value::Null => None,
        c => {
            let kind = c["kind"].as_str().unwrap_or("");
            let words = c["text"].as_str().unwrap_or("").trim();
            if !matches!(kind, "command" | "condition") || words.is_empty() {
                return Err("Say the command, or the condition, it is checked by.".into());
            }
            if words.chars().count() > MOST_TEXT {
                return Err("A check is at most 4,000 characters.".into());
            }
            Some(Check { kind: kind.into(), text: words.into(), by: "person".into() })
        }
    };
    let b = &m["budget"];
    let d = Budget::default();
    let budget = Budget {
        steps: b["steps"].as_u64().unwrap_or(d.steps),
        minutes: b["minutes"].as_u64().unwrap_or(d.minutes),
        tokens: b["tokens"].as_u64().unwrap_or(d.tokens),
        until_met: b["until_met"].as_bool().unwrap_or(false),
    };
    if !in_bounds(&budget) {
        return Err("The budget is out of bounds.".into());
    }
    Ok(Goal::new(text, check, budget))
}

/// Whether a budget is one a goal may have: steps 1 to 100,000, minutes 1 to 43,200, tokens 10,000 or more (any,
/// until it is met).
pub fn in_bounds(b: &Budget) -> bool {
    b.until_met
        || (1..=MOST_STEPS).contains(&b.steps) && (1..=MOST_MINUTES).contains(&b.minutes) && b.tokens >= FEWEST_TOKENS
}

/// What a `loop_add` asks for: how often (seconds), the prompt and whether it may change files; or what is wrong.
fn loop_of(m: &Value) -> Result<(u64, String, bool), String> {
    let secs = crate::schedules::interval(m["every"].as_str().unwrap_or(""))
        .ok_or("Say how often: 15m, 30m, 1h, 4h or 1d.")?;
    if secs < 60 {
        return Err("A loop runs at most every minute.".into());
    }
    let prompt = m["prompt"].as_str().unwrap_or("").trim();
    if prompt.is_empty() {
        return Err("Say what each run does.".into());
    }
    if prompt.chars().count() > MOST_TEXT {
        return Err("A loop's prompt is at most 4,000 characters.".into());
    }
    Ok((secs, prompt.to_string(), m["edits"].as_bool().unwrap_or(false)))
}

/// A loop as the faces list it, read while a turn runs too: its counts are the loop's own.
#[derive(Clone)]
struct Seen {
    id: u32,
    every: u64,
    prompt: String,
    edits: bool,
    runs: Arc<std::sync::atomic::AtomicU32>,
    skipped: Arc<std::sync::atomic::AtomicU32>,
    last: Arc<std::sync::atomic::AtomicU8>,
}

/// The session's loops as last seen between turns (the loops themselves are the session's).
#[derive(Default)]
struct LoopsSeen(Mutex<Vec<Seen>>);

fn set_seen(sh: &Shared, seen: Vec<Seen>) {
    if let Ok(mut s) = sh.kept.of::<LoopsSeen>().0.lock() {
        *s = seen;
    }
}

/// The loops changed: kept as seen, and the faces told.
fn loops_changed(ctx: &mut Ctx<'_>) {
    let seen = ctx
        .p
        .loops
        .iter()
        .map(|l| Seen {
            id: l.id,
            every: l.every,
            prompt: l.prompt.clone(),
            edits: l.edits,
            runs: l.runs.clone(),
            skipped: l.skipped.clone(),
            last: l.last.clone(),
        })
        .collect();
    set_seen(ctx.sh, seen);
    send(&ctx.sh.out, loops_message(ctx.sh));
}

fn loops_message(sh: &Shared) -> Value {
    let seen = sh.kept.of::<LoopsSeen>().0.lock().map(|s| s.clone()).unwrap_or_default();
    let items: Vec<Value> = seen
        .iter()
        .map(|l| {
            let last = match l.last.load(Ordering::Relaxed) {
                1 => json!("ok"),
                2 => json!("failed"),
                _ => Value::Null,
            };
            json!({"id": l.id, "every": l.every, "every_words": crate::schedules::interval_words(l.every),
                   "prompt": l.prompt, "edits": l.edits, "runs": l.runs.load(Ordering::Relaxed),
                   "skipped": l.skipped.load(Ordering::Relaxed), "last": last})
        })
        .collect();
    json!({"type": "loops", "items": items})
}

#[cfg(test)]
mod tests {
    use super::*;
    use ca_agent::goal::{self, Goals};

    fn waiting(g: &Goals) -> Vec<&str> {
        g.next.iter().map(|n| n.text.as_str()).collect()
    }

    #[test]
    fn a_goal_set_is_read_with_its_check_and_budget_and_refused_out_of_bounds() {
        let g = goal_of(&json!({"text": " fix it ", "check": {"kind": "command", "text": "cargo test"},
                                "budget": {"steps": 5, "minutes": 2, "tokens": 20000}}))
        .expect("as written");
        assert_eq!(
            (g.text.as_str(), g.check.expect("as written").by.as_str(), g.budget.steps, g.budget.until_met),
            ("fix it", "person", 5, false)
        );
        let g = goal_of(&json!({"text": "x", "check": null, "budget": {"until_met": true, "steps": 0}}))
            .expect("as written");
        assert!(g.check.is_none() && g.budget.until_met, "until_met is read as the plan names it");
        assert_eq!(goal_of(&json!({"text": "x"})).expect("as written").budget, Budget::default());
        let out = |b: Value| goal_of(&json!({"text": "x", "budget": b})).expect_err("refused");
        assert_eq!(out(json!({"steps": 0})), "The budget is out of bounds.");
        assert_eq!(out(json!({"steps": 100_001})), "The budget is out of bounds.");
        assert_eq!(out(json!({"minutes": 43_201})), "The budget is out of bounds.");
        assert_eq!(out(json!({"tokens": 9_999})), "The budget is out of bounds.");
        assert!(goal_of(&json!({"text": "  "})).is_err());
        assert!(goal_of(&json!({"text": "x", "check": {"kind": "command", "text": " "}})).is_err());
        assert!(goal_of(&json!({"text": "x".repeat(4_001)})).is_err());
    }

    #[test]
    fn the_goals_are_paused_resumed_cancelled_dropped_and_the_next_started() {
        let mut g = Goals::default();
        assert!(crate::goals::pause(&mut g).is_err());
        g.set(Goal::new("one", None, Budget::default())).expect("as written");
        g.set(Goal::new("two", None, Budget::default())).expect("as written");
        g.set(Goal::new("three", None, Budget::default())).expect("as written");
        assert_eq!(waiting(&g), ["two", "three"]);
        assert!(crate::goals::start_next(&mut g).is_err(), "not while one is worked on");
        crate::goals::pause(&mut g).expect("as written");
        assert_eq!(g.current.as_ref().expect("as written").status, goal::PAUSED);
        assert!(g.prompt().is_none(), "a paused goal is not carried on");
        crate::goals::resume(&mut g).expect("as written");
        assert!(g.prompt().is_some());
        crate::goals::drop_waiting(&mut g, 2).expect("as written");
        assert!(crate::goals::drop_waiting(&mut g, 2).is_err());
        crate::goals::cancel(&mut g).expect("as written");
        assert!(
            g.current.as_ref().expect("as written").outcome.as_deref().expect("as written").starts_with("Cancelled")
        );
        assert_eq!(crate::goals::start_next(&mut g).expect("as written"), "Next goal: two");
        assert_eq!(g.current.as_ref().expect("as written").status, goal::WORKING);
        assert!(waiting(&g).is_empty());
    }

    #[test]
    fn a_loop_says_how_often_and_what_it_does() {
        assert_eq!(
            loop_of(&json!({"every": "1h", "prompt": " run the tests ", "edits": true})).expect("as written"),
            (3600, "run the tests".into(), true)
        );
        assert!(!loop_of(&json!({"every": "15m", "prompt": "x"})).expect("as written").2);
        assert!(loop_of(&json!({"every": "30s", "prompt": "x"})).is_err());
        assert!(loop_of(&json!({"every": "often", "prompt": "x"})).is_err());
        assert!(loop_of(&json!({"every": "1d", "prompt": ""})).is_err());
    }

    #[test]
    fn the_goals_messages_have_their_rules() {
        use crate::ide_ext::rule;
        assert_eq!(rule("goal_set"), Some(Rule::Queue));
        assert_eq!(rule("goal_pause"), Some(Rule::Turn));
        assert_eq!(rule("loops"), Some(Rule::Turn));
        assert_eq!(rule("loop_add"), Some(Rule::Queue));
    }
}
