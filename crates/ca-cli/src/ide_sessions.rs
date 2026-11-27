//! This folder's sessions: listed (newest first, with how many days ago each was last worked in), taken up, forked
//! (all of one, or what came before one of its questions), searched by their words, and a new one begun (the Sessions
//! view's New Session, `/clear` and `/new`).
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn).
//!
//! - `sessions` -> `sessions{items: [{id, title, updated, days, messages, chat, unsent, current}]}`: answered
//!   while a turn runs too, read off the driver's loop. `chat`: kept with a chat in Arena; `unsent`: Arena does not
//!   have all of it yet. The session worked in is there even before it has a message.
//! - `session_search{words}` -> `session_found{words, items: [{id, seq, text}]}`: messages of the folder's
//!   sessions that hold every word.
//! - `session_new`, `session_resume{id}`, `session_fork{id?, before_q?}`: between turns only (refused while one
//!   runs). The session is prepared again with the driver's approver and inbox; the driver then sends `state` and
//!   `tasks` for it itself.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::ide_ext::{Area, Ctx, Rule, Sent, Shared, send};
use crate::ide_files::Side;
use crate::session::{self, Opts, Resume};

/// This area in the driver.
pub struct Here;

/// The most sessions a list holds, and the most messages a search finds.
const MOST: usize = 200;
const FOUND: usize = 50;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("sessions", Rule::Turn),
        ("session_search", Rule::Turn),
        ("session_new", Rule::Refuse("start a new session")),
        ("session_resume", Rule::Refuse("take up another session")),
        ("session_fork", Rule::Refuse("fork the session")),
    ];

    /// The list and the search: read from the store off the driver's loop, then said.
    fn shared(sh: &Shared, m: &Value) -> bool {
        let (store, folder, current, out) = (sh.store.clone(), sh.folder.clone(), sh.session.clone(), sh.out.clone());
        let m = m.clone();
        tokio::task::spawn_blocking(move || {
            let Ok(store) = store.lock() else { return };
            let reply = if crate::ide_ext::kind(&m) == "sessions" {
                json!({"type": "sessions", "items": items(&store, &folder, &current, today())})
            } else {
                let words = m["words"].as_str().unwrap_or("");
                json!({"type": "session_found", "words": words, "items": found(&store, &folder, words)})
            };
            drop(store);
            send(&out, reply);
        });
        true
    }

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        let resume = match crate::ide_ext::kind(m) {
            "session_new" => Ok(Resume::New),
            "session_resume" => match m["id"].as_str() {
                Some(id) if id == ctx.p.session => return true,
                Some(id) => Ok(Resume::Id(id.to_string())),
                None => Err("Which session to take up?".to_string()),
            },
            "session_fork" => fork(ctx, m),
            _ => return false,
        };
        match resume {
            Ok(r) => {
                if let Err(e) = switch_session(ctx, r).await {
                    ctx.sh.say(&e);
                }
            }
            Err(e) => ctx.sh.say(&e),
        }
        true
    }

    /// `/clear` and `/new`: a new session, the last one kept.
    async fn command(ctx: &mut Ctx<'_>, sent: &mut Sent) {
        if matches!(sent.model.trim(), "/clear" | "/new") {
            sent.done = true;
            if let Err(e) = switch_session(ctx, Resume::New).await {
                ctx.sh.say(&e);
            }
        }
    }
}

/// The fork asked: the session forked now in the store when only what came before a question is to go with it.
fn fork(ctx: &mut Ctx<'_>, m: &Value) -> Result<Resume, String> {
    let from = m["id"].as_str().map(str::to_string).unwrap_or_else(|| ctx.p.session.clone());
    let store = ctx.p.store.lock().map_err(|_| "The sessions could not be read.".to_string())?;
    let messages = store.summary(&from).map_err(|e| e.to_string())?.messages;
    if messages == 0 {
        return Err("This session has nothing to fork yet.".into());
    }
    let Some(q) = m["before_q"].as_u64() else {
        drop(store);
        return Ok(Resume::Fork(Box::new(Resume::Id(from))));
    };
    let questions = session::questions_in(&store, &from).map_err(|e| e.to_string())?;
    let seq = seq_of(&questions, q as usize).ok_or_else(|| format!("There is no question {q} to fork from."))?;
    store.fork(&from, Some(seq)).map(Resume::Id).map_err(|e| e.to_string())
}

/// Where question `q` (from 1, as the thread numbers them) begins in its session.
fn seq_of(questions: &[(usize, i64, String)], q: usize) -> Option<i64> {
    questions.iter().find(|(k, _, _)| *k == q).map(|(_, seq, _)| *seq)
}

/// Another session in this one's place: prepared with the driver's approver and inbox, on the model and in the
/// mode in force for a new one; the project's side made again when its folder is another. Nothing changes when it
/// cannot be prepared (why is given back).
pub async fn switch_session(ctx: &mut Ctx<'_>, resume: Resume) -> Result<(), String> {
    let mut cwd = ctx.p.agent.ctx.workspace.cwd().to_path_buf();
    if let Resume::Id(id) = &resume {
        let folder = ctx.p.store.lock().ok().and_then(|s| s.summary(id).ok()).map(|s| s.folder);
        match folder {
            Some(f) if std::path::Path::new(&f).is_dir() => cwd = f.into(),
            Some(_) => return Err("That session's folder is not there any more.".into()),
            None => return Err(format!("There is no session {id}.")),
        }
    }
    let new = resume == Resume::New;
    let opts = Opts {
        cwd: Some(cwd),
        mode: Some(ctx.p.agent.policy.mode.name().into()),
        model: new.then(|| ctx.p.agent.model.clone()),
        carry_plan: false,
        ..ctx.drv.opts.clone()
    };
    let mut p = session::prepare(ctx.app, &opts, ctx.drv.approver.clone(), resume)
        .await
        .map_err(|e| format!("The session could not be taken up: {e}"))?;
    if new {
        p.agent.thinking = ctx.p.agent.thinking.clone();
    }
    p.agent.inbox = Some(ctx.drv.inbox.clone());
    let root = p.agent.ctx.workspace.root().to_path_buf();
    *ctx.p = p;
    if root != ctx.side.workspace().root() {
        let side = Arc::new(Side::new(
            ctx.p.agent.ctx.workspace.clone(),
            ctx.p.agent.ctx.checkpoints.clone(),
            ctx.drv.out.clone(),
            ctx.app.settings.env().get("SHELL"),
            &ctx.app.paths.data,
        ));
        side.start_watching();
        *ctx.side = side;
    }
    Ok(())
}

/// Today, as days since the epoch in this computer's time zone.
fn today() -> i64 {
    day_of(chrono::Utc::now().timestamp_millis())
}

/// The local day a time (milliseconds since the epoch) falls on, as days since the epoch.
fn day_of(ms: i64) -> i64 {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.date_naive().signed_duration_since(chrono::NaiveDate::default()).num_days(),
        None => ms.div_euclid(86_400_000),
    }
}

/// The folder's sessions as the list shows them, newest first; the one worked in first when it has no message yet.
fn items(store: &ca_session::Store, folder: &str, current: &str, today: i64) -> Vec<Value> {
    let list = store.list(folder, MOST).unwrap_or_default();
    let mut out = Vec::with_capacity(list.len() + 1);
    if !list.iter().any(|s| s.id == current) {
        out.push(json!({"id": current, "title": "", "updated": 0, "days": 0, "messages": 0, "chat": false,
            "unsent": false, "current": true}));
    }
    for s in list {
        let link = store.chat_link(&s.id).unwrap_or_default();
        let chat = link.chat.is_some() && !link.off;
        let unsent = chat && store.unsent(&s.id, 1).is_ok_and(|u| !u.is_empty());
        out.push(json!({
            "id": s.id, "title": s.title, "updated": s.updated, "days": (today - day_of(s.updated)).max(0),
            "messages": s.messages, "chat": chat, "unsent": unsent, "current": s.id == current,
        }));
    }
    out
}

/// The messages that hold every word, as the search shows them.
fn found(store: &ca_session::Store, folder: &str, words: &str) -> Vec<Value> {
    store
        .search(folder, words, FOUND)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, seq, text)| json!({"id": id, "seq": seq, "text": text}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ca_llm::Message;

    #[test]
    fn the_folders_sessions_are_listed_newest_first_with_the_one_worked_in_marked() {
        let d = tempfile::tempdir().expect("temp");
        let s = ca_session::Store::open(&d.path().join("s.db")).expect("open");
        let a = s.create("/w", "m", "ask").expect("a");
        s.append(&a, &Message::user("first words"), true).expect("u");
        let b = s.create("/w", "m", "ask").expect("b");
        s.append(&b, &Message::user("second words"), true).expect("u");
        let elsewhere = s.create("/x", "m", "ask").expect("x");
        s.append(&elsewhere, &Message::user("other folder"), true).expect("u");
        let list = items(&s, "/w", &a, today());
        let ids: Vec<&str> = list.iter().map(|i| i["id"].as_str().unwrap_or("")).collect();
        assert_eq!(ids, [b.as_str(), a.as_str()]);
        assert_eq!((list[0]["current"].clone(), list[1]["current"].clone()), (json!(false), json!(true)));
        assert_eq!((list[1]["title"].clone(), list[1]["messages"].clone()), (json!("first words"), json!(1)));
        assert_eq!(
            (list[1]["days"].clone(), list[1]["chat"].clone(), list[1]["unsent"].clone()),
            (json!(0), json!(false), json!(false))
        );
        // A new session, nothing said in it yet, is there first.
        let fresh = s.create("/w", "m", "ask").expect("new");
        let list = items(&s, "/w", &fresh, today());
        assert_eq!(
            (list.len(), list[0]["id"].as_str(), list[0]["current"].clone()),
            (3, Some(fresh.as_str()), json!(true))
        );
    }

    #[test]
    fn a_session_kept_with_arena_says_so_and_whether_arena_has_all_of_it() {
        let d = tempfile::tempdir().expect("temp");
        let s = ca_session::Store::open(&d.path().join("s.db")).expect("open");
        let a = s.create("/w", "m", "ask").expect("a");
        s.append(&a, &Message::user("kept"), true).expect("u");
        s.link_chat(&a, "chat-1").expect("link");
        let list = items(&s, "/w", &a, today());
        assert_eq!((list[0]["chat"].clone(), list[0]["unsent"].clone()), (json!(true), json!(true)));
    }

    #[test]
    fn a_search_finds_the_messages_that_hold_every_word() {
        let d = tempfile::tempdir().expect("temp");
        let s = ca_session::Store::open(&d.path().join("s.db")).expect("open");
        let a = s.create("/w", "m", "ask").expect("a");
        s.append(&a, &Message::user("the parser breaks on tabs"), true).expect("u");
        s.append(&a, &Message::user("the lexer is fine"), true).expect("u");
        let hits = found(&s, "/w", "parser tabs");
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0]["id"].as_str(), hits[0]["seq"].clone()), (Some(a.as_str()), json!(0)));
        assert!(found(&s, "/w", "  ").is_empty() && found(&s, "/x", "parser").is_empty());
    }

    #[test]
    fn a_question_is_found_by_its_number_and_a_day_by_the_local_calendar() {
        let qs = vec![(1, 0, "a".to_string()), (2, 5, "b".to_string())];
        assert_eq!((seq_of(&qs, 2), seq_of(&qs, 3)), (Some(5), None));
        let now = chrono::Utc::now().timestamp_millis();
        assert_eq!(today() - day_of(now - 86_400_000), 1);
        assert_eq!(today() - day_of(now), 0);
    }
}
