//! The Sessions view's part in Arena: the person's chats there (continued here), the other sessions sent there,
//! and where the session stands with its own chat.
//!
//! - `arena_chats{query}` -> `arena_chats{query, account, items: [{id, title, place, messages, model, updated,
//!   terminal}], more, error?}`: answered while a turn runs too. The chats whose words (title or folder) hold every
//!   word of the query, newest first, at most [`MOST`]; `more` says how many matched beyond them. A chat kept with a
//!   session of this folder is left out (that session is in the list already); `terminal`: Code Arena made it.
//! - `arena_chat_open{id}` -> `arena_chat_opened{id, error?}`: between turns only. The chat's session of this
//!   folder is taken up, or a new one made with the chat's history; the driver then sends `state` for it.
//! - `sessions_send_all` -> `sessions_sent{sent, messages, failed}`: every other session of this folder that Arena
//!   lacks, sent now, each to its own chat (`sent` sessions, `failed` still waiting).
//! - `state.sync` and `status.sync`, and `session_sync{session, sync}` when it changes: `{chat, url, account,
//!   waiting, trouble, behind, here}`, null while the session is not kept with Arena at all (signed out, or
//!   `arena.syncChats` off). `here`: kept here only (`/sync off`, or an Arena without chats).
//! - `session_notice{text, problem}`: what the session has to say between turns (Arena out of reach and back, the
//!   web's news taken in), for a toast, said once.
//!
//! Between turns the web's news is taken in at once: the thread and the list are sent again when it brought
//! anything. After a turn the chat is let go in the background (`end_turn_soon`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ca_arena::chats::ChatItem;
use serde_json::{Value, json};

use crate::ide_ext::{Ctx, Shared, send};
use crate::session::{self, Resume};

/// The most chats a list holds, and the most asked of Arena.
pub const MOST: usize = 200;
const ASKED: usize = 500;
/// How long a list read from Arena answers a find before it is read again.
const FRESH: Duration = Duration::from_secs(60);
/// How often the session's standing with Arena is looked at.
const LOOK_EVERY: Duration = Duration::from_secs(2);

/// What the area keeps for as long as the driver runs.
#[derive(Default)]
pub struct Kept {
    /// Whose Arena it is, once asked.
    account: Mutex<Option<String>>,
    asked_account: AtomicBool,
    /// The last list read from Arena, and when.
    chats: Mutex<Option<(Instant, Vec<ChatItem>)>>,
    /// The watch of the session's chat: the web's news and its standing.
    watching: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// A turn runs: what the session says waits for the turn to say it.
    turn: AtomicBool,
    /// The standing last told.
    told: Mutex<Value>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The session's standing with its chat in Arena, as `state.sync` says it.
pub fn sync_of(sh: &Shared) -> Value {
    let Some(s) = &sh.sync else { return Value::Null };
    let account = lock(&sh.kept.of::<Kept>().account).clone();
    let chat = s.chat_id();
    let url = chat.as_ref().map(|c| format!("{}/chat/{c}", s.arena().base()));
    json!({
        "chat": chat, "url": url, "account": account, "waiting": s.pending(), "trouble": s.trouble(),
        "behind": s.behind(), "here": s.kept_here(),
    })
}

/// Whether a chat's words hold every word asked (none asked: every chat).
pub fn matches(c: &ChatItem, words: &str) -> bool {
    let hay = format!("{} {}", c.title, c.origin_place.as_deref().unwrap_or("")).to_lowercase();
    words.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
}

/// The chats as the list shows them: those that match and are not kept here already, at most [`MOST`], and how
/// many more matched.
pub fn shown(list: &[ChatItem], words: &str, kept_here: impl Fn(&str) -> bool) -> (Vec<Value>, usize) {
    let all: Vec<&ChatItem> = list.iter().filter(|c| matches(c, words) && !kept_here(&c.id)).collect();
    let more = all.len().saturating_sub(MOST);
    let items = all
        .into_iter()
        .take(MOST)
        .map(|c| {
            json!({
                "id": c.id, "title": c.title, "place": c.origin_place, "messages": c.messages, "model": c.model,
                "updated": c.updated_at, "terminal": c.origin.as_deref() == Some("code-arena"),
            })
        })
        .collect();
    (items, more)
}

/// Whether a chat is kept with a session of this folder.
fn kept_in(store: &ca_session::Store, folder: &str, chat: &str) -> bool {
    store.by_chat(chat).ok().flatten().is_some_and(|s| store.summary(&s).is_ok_and(|x| x.folder == folder))
}

/// The account's name, asked of Arena once.
async fn account(sh: &Shared) -> Option<String> {
    let kept = sh.kept.of::<Kept>();
    if let Some(a) = lock(&kept.account).clone() {
        return Some(a);
    }
    let arena = sh.arena.clone()?;
    if kept.asked_account.swap(true, Ordering::Relaxed) {
        return None;
    }
    let me = tokio::time::timeout(Duration::from_secs(15), arena.me()).await.ok()?.ok()?;
    let name = me.display_name.filter(|n| !n.trim().is_empty()).unwrap_or(me.user_name);
    *lock(&kept.account) = Some(name.clone());
    Some(name)
}

/// `arena_chats{query}`: read off the driver's loop, then said.
pub fn chats(sh: &Shared, m: &Value) {
    let query = m["query"].as_str().unwrap_or("").trim().to_string();
    let asked = query.clone();
    let reply = move |items: Vec<Value>, more: usize, account: Option<String>, error: Option<String>| {
        let mut v = json!({"type": "arena_chats", "query": asked, "account": account, "items": items, "more": more});
        if let Some(e) = error {
            v["error"] = json!(e);
        }
        v
    };
    let (Some(arena), Ok(rt)) = (sh.arena.clone(), tokio::runtime::Handle::try_current()) else {
        send(&sh.out, reply(Vec::new(), 0, None, Some("Sign in to Arena to see its chats.".into())));
        return;
    };
    let sh = sh.clone();
    rt.spawn(async move {
        let kept = sh.kept.of::<Kept>();
        let cached = lock(&kept.chats).clone().filter(|(at, _)| !query.is_empty() && at.elapsed() < FRESH);
        let list = match cached {
            Some((_, l)) => l,
            None => match arena.chats(ASKED).await {
                Ok(l) => {
                    *lock(&kept.chats) = Some((Instant::now(), l.chats.clone()));
                    l.chats
                }
                Err(e) => {
                    let who = account(&sh).await;
                    send(&sh.out, reply(Vec::new(), 0, who, Some(format!("Arena's chats could not be read: {e}"))));
                    return;
                }
            },
        };
        let who = account(&sh).await;
        let (store, folder, words) = (sh.store.clone(), sh.folder.clone(), query.clone());
        let shown = tokio::task::spawn_blocking(move || {
            let store = lock(&store);
            shown(&list, &words, |id| kept_in(&store, &folder, id))
        })
        .await;
        if let Ok((items, more)) = shown {
            send(&sh.out, reply(items, more, who, None));
        }
    });
}

/// `arena_chat_open{id}`: the chat continued here.
pub async fn open(ctx: &mut Ctx<'_>, m: &Value) {
    let id = m["id"].as_str().unwrap_or("").to_string();
    let mut reply = json!({"type": "arena_chat_opened", "id": id});
    if let Err(e) = continue_here(ctx, &id).await {
        reply["error"] = json!(e);
        ctx.sh.say(&e);
    }
    send(&ctx.sh.out, reply);
}

async fn continue_here(ctx: &mut Ctx<'_>, id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Which chat to continue?".into());
    }
    let Some(arena) = ctx.drv.arena.clone() else { return Err("Sign in to Arena to continue its chats.".into()) };
    if ctx.p.sync.is_none() {
        return Err("Chats are kept with Arena only with arena.syncChats on.".into());
    }
    let chosen = lock(&ctx.sh.kept.of::<Kept>().chats)
        .as_ref()
        .and_then(|(_, l)| l.iter().find(|c| c.id == id).cloned());
    let (title, model) = chosen.map(|c| (c.title, c.model)).unwrap_or_default();
    // A session of this folder's that is kept with it already: taken up; else a new one, filled from the web.
    let (session, fresh) = {
        let store = ctx.p.store.lock().map_err(|_| "The sessions could not be read.".to_string())?;
        match store.by_chat(id).map_err(|e| e.to_string())? {
            Some(s) if store.summary(&s).is_ok_and(|x| x.folder == ctx.p.folder) => (s, false),
            _ => {
                let model = model.unwrap_or_else(|| ctx.p.agent.model.clone());
                let made = store.create(&ctx.p.folder, &model, ctx.p.agent.policy.mode.name()).map_err(|e| e.to_string())?;
                store.link_chat(&made, id).map_err(|e| e.to_string())?;
                (made, true)
            }
        }
    };
    if session != ctx.p.session {
        super::switch_session(ctx, Resume::Id(session)).await?;
    }
    if let Some(s) = ctx.p.sync.clone()
        && s.take_in().await.any()
    {
        session::reload_history(ctx.p);
    }
    let chat = id.to_string();
    tokio::spawn(async move {
        let _ = arena.continued(&chat).await;
    });
    let count = ctx.p.store.lock().ok().and_then(|s| s.summary(&ctx.p.session).ok()).map(|s| s.messages).unwrap_or(0);
    let name = if title.trim().is_empty() { String::new() } else { format!(" \"{}\"", title.trim()) };
    let n = if count == 1 { "1 message".to_string() } else { format!("{count} messages") };
    notice(&ctx.sh.out, false, &format!("{} the web chat{name} here: {n}.", if fresh { "Continuing" } else { "Back in" }));
    Ok(())
}

/// A line for a toast.
fn notice(out: &tokio::sync::broadcast::Sender<String>, problem: bool, text: &str) {
    send(out, json!({"type": "session_notice", "text": text, "problem": problem}));
}

/// `sessions_send_all`: every other session of this folder that Arena lacks, sent in the background.
pub fn send_all(ctx: &mut Ctx<'_>) {
    let out = ctx.sh.out.clone();
    let Some(arena) = ctx.drv.arena.clone() else {
        notice(&out, true, "Sign in to Arena to send the sessions there.");
        return;
    };
    let p = &ctx.p;
    let key = ca_memory::keys::project_key(p.agent.ctx.workspace.root());
    let list = p.store.lock().ok().and_then(|s| s.list(&p.folder, ASKED).ok()).unwrap_or_default();
    let mut due = Vec::new();
    for summary in list {
        if summary.id == p.session {
            continue;
        }
        let off = p.store.lock().ok().and_then(|s| s.chat_link(&summary.id).ok()).is_none_or(|l| l.off);
        if off {
            continue;
        }
        let quiet: ca_sync::Notice = Arc::new(|_, _| {});
        let s = ca_sync::ChatSync::new(
            arena.clone(),
            p.store.clone(),
            summary.id.clone(),
            p.folder.clone(),
            Some(key.clone()),
            summary.model.clone(),
            quiet,
        );
        if s.pending() > 0 {
            due.push(s);
        }
    }
    tokio::spawn(async move {
        let (mut sent, mut messages, mut failed) = (0, 0, 0);
        for s in due {
            let before = s.pending();
            // One send: what it could not send (Arena out of reach, the web answering) goes later.
            let _ = s.push().await;
            let after = s.pending();
            if after < before {
                sent += 1;
                messages += before - after;
            }
            if after > 0 {
                failed += 1;
            }
        }
        notice(&out, failed > 0, &sent_words(sent, messages, failed));
        send(&out, json!({"type": "sessions_sent", "sent": sent, "messages": messages, "failed": failed}));
    });
}

/// What sending the other sessions came to, in words.
pub fn sent_words(sent: usize, messages: usize, failed: usize) -> String {
    let of = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    match (sent, failed) {
        (0, 0) => "Arena has every session of this folder.".into(),
        (0, f) => format!("{} could not be sent now: Arena is out of reach, or the web is answering.", of(f, "session", "sessions")),
        (s, 0) => format!("Sent {} of {} to their chats in Arena.", of(messages, "message", "messages"), of(s, "session", "sessions")),
        (s, f) => format!(
            "Sent {} of {} to their chats in Arena; {} could not be sent now.",
            of(messages, "message", "messages"),
            of(s, "session", "sessions"),
            of(f, "session", "sessions")
        ),
    }
}

/// The watch of the session's chat begins (the one before ends): the web's news is posted to be taken in between
/// turns, and the standing is told when it changes, with what the session has to say.
pub fn start(sh: &Shared) {
    let kept = sh.kept.of::<Kept>();
    let mut watching = lock(&kept.watching);
    if let Some(h) = watching.take() {
        h.abort();
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    let (tx, mut news) = tokio::sync::mpsc::unbounded_channel::<()>();
    if let Some(s) = &sh.sync {
        s.listen(tx);
    } else {
        drop(tx);
    }
    let sh = sh.clone();
    *watching = Some(rt.spawn(async move {
        let _ = account(&sh).await;
        let mut tick = tokio::time::interval(LOOK_EVERY);
        let mut listening = true;
        loop {
            tokio::select! {
                n = news.recv(), if listening => match n {
                    // A turn takes in the web's news itself, at its start.
                    Some(()) if !sh.kept.of::<Kept>().turn.load(Ordering::Relaxed) => {
                        sh.post(json!({"type": "_session_news", "session": sh.session}));
                    }
                    Some(()) => {}
                    None => listening = false,
                },
                _ = tick.tick() => look(&sh),
            }
        }
    }));
}

/// The standing told when it changed; between turns, what the session has to say.
fn look(sh: &Shared) {
    let kept = sh.kept.of::<Kept>();
    let now = sync_of(sh);
    let changed = {
        let mut told = lock(&kept.told);
        let changed = *told != now;
        if changed {
            told.clone_from(&now);
        }
        changed
    };
    if changed {
        send(&sh.out, json!({"type": "session_sync", "session": sh.session, "sync": now}));
    }
    if !kept.turn.load(Ordering::Relaxed) {
        let lines: Vec<(bool, String)> = sh.notices.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default();
        for (problem, line) in lines {
            notice(&sh.out, problem, &line);
        }
    }
}

/// `_session_news`: what the web added, taken in between turns; the thread and the list sent again when it brought
/// anything.
pub async fn news(ctx: &mut Ctx<'_>, m: &Value) {
    if m["session"].as_str() != Some(ctx.p.session.as_str()) {
        return;
    }
    let Some(s) = ctx.p.sync.clone() else { return };
    let taken = s.take_in().await;
    for line in &taken.said {
        notice(&ctx.sh.out, false, line);
    }
    if taken.any() {
        session::reload_history(ctx.p);
        // The driver's own `state` (the thread again), and with it the list.
        ctx.sh.post(json!({"type": "state"}));
    }
}

/// A turn begins: what the session says waits for it.
pub fn before_turn(sh: &Shared) {
    sh.kept.of::<Kept>().turn.store(true, Ordering::Relaxed);
}

/// A turn ended: the chat is let go once Arena has its messages, in the background.
pub fn after_turn(ctx: &mut Ctx<'_>) {
    ctx.sh.kept.of::<Kept>().turn.store(false, Ordering::Relaxed);
    if let Some(s) = &ctx.p.sync {
        s.end_turn_soon();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str, title: &str, place: Option<&str>, origin: Option<&str>) -> ChatItem {
        ChatItem {
            id: id.into(),
            title: title.into(),
            origin: origin.map(str::to_string),
            origin_ref: None,
            origin_place: place.map(str::to_string),
            model: None,
            updated_at: None,
            messages: 3,
            code_arena: false,
        }
    }

    #[test]
    fn a_find_holds_every_word_in_the_title_or_the_folder_whatever_their_case() {
        let c = chat("c", "Fix the Parser", Some("/home/me/lexer"), None);
        assert!(matches(&c, ""));
        assert!(matches(&c, "parser fix"));
        assert!(matches(&c, "PARSER lexer"));
        assert!(!matches(&c, "parser tabs"));
    }

    #[test]
    fn the_list_leaves_out_what_is_kept_here_and_says_how_many_more_matched() {
        let mut list: Vec<ChatItem> = (0..MOST + 3).map(|i| chat(&format!("c{i}"), "chat", None, None)).collect();
        list.push(chat("t", "made here", Some("/w"), Some("code-arena")));
        let (items, more) = shown(&list, "", |id| id == "c0");
        assert_eq!((items.len(), more), (MOST, 3));
        assert_eq!(items[0]["id"], "c1");
        let (items, more) = shown(&list, "made", |_| false);
        assert_eq!((items.len(), more), (1, 0));
        assert_eq!((items[0]["terminal"].clone(), items[0]["place"].clone()), (json!(true), json!("/w")));
    }

    #[test]
    fn a_chat_is_left_out_only_when_a_session_of_this_folder_keeps_it() {
        let d = tempfile::tempdir().expect("temp");
        let s = ca_session::Store::open(&d.path().join("s.db")).expect("open");
        let a = s.create("/w", "m", "ask").expect("a");
        s.link_chat(&a, "chat-1").expect("link");
        assert!(kept_in(&s, "/w", "chat-1"));
        assert!(!kept_in(&s, "/x", "chat-1"));
        assert!(!kept_in(&s, "/w", "chat-2"));
    }

    #[test]
    fn sending_the_others_says_what_it_came_to() {
        assert_eq!(sent_words(0, 0, 0), "Arena has every session of this folder.");
        assert_eq!(sent_words(2, 7, 0), "Sent 7 messages of 2 sessions to their chats in Arena.");
        assert_eq!(sent_words(1, 1, 1), "Sent 1 message of 1 session to their chats in Arena; 1 session could not be sent now.");
        assert!(sent_words(0, 0, 2).starts_with("2 sessions could not be sent now"));
    }
}
