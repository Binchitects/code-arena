//! A session kept in step with its chat in Arena, both ways.
//!
//! Each message the session keeps is sent in the background, in order, again until Arena has it (a lost connection,
//! Arena restarting): the chat is made at the first message, found again by the session's id, and every message
//! goes after the one Arena last gave an id for. A session that went back to before messages Arena has (`/rewind`)
//! sends on a branch of its own. When the web added after that message, Arena says what the session lacks, and
//! sending waits for it to be taken in at the next turn's start.

mod convert;
mod turn;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ca_arena::ArenaClient;
use ca_arena::chats::{self, Append, NewChat, SyncRead, SyncSummary};
use ca_session::Store;
use tokio::sync::Notify;

pub use convert::{from_arena, to_arena};
pub use turn::Taken;

/// The most messages one request sends.
pub const BATCH: usize = 200;
/// The longest wait between tries while Arena cannot be reached.
const RETRY_AT_MOST: Duration = Duration::from_secs(10);

/// What the session tells the person: a line, and whether it is a problem.
pub type Notice = Arc<dyn Fn(bool, String) + Send + Sync>;

#[derive(Debug, Default)]
struct State {
    /// Arena has messages this session has not taken in: sending waits for the next turn's start.
    behind: Option<SyncRead>,
    /// "Arena cannot be reached" was said: not again until a send gets through.
    said_unreachable: bool,
    /// The last error, for `/sync`.
    last_error: Option<String>,
    /// What was sent, for `/sync`.
    sent: usize,
    /// Sends that failed (Arena out of reach): a flush gives up at the first.
    failed: usize,
}

pub struct ChatSync {
    arena: ArenaClient,
    store: Arc<Mutex<Store>>,
    session: String,
    place: String,
    project: Option<String>,
    model: Mutex<String>,
    state: Mutex<State>,
    poke: Notify,
    idle: Notify,
    notice: Notice,
    /// The server has no chat sync: nothing is sent.
    unsupported: std::sync::atomic::AtomicBool,
    /// A turn holds the chat now.
    holding: std::sync::atomic::AtomicBool,
    hold_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The answer being written, waiting to go; and whether one is on its way.
    draft_next: Mutex<Option<(String, String)>>,
    drafting: std::sync::atomic::AtomicBool,
    /// The last turn's end, under way.
    ending: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// How a push ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pushed {
    /// Arena has everything.
    Done,
    /// The web added meanwhile: take it in first.
    Behind,
    /// Try again after this.
    Retry(Duration),
    /// Kept apart from Arena: nothing to do.
    Off,
}

impl ChatSync {
    pub fn new(
        arena: ArenaClient,
        store: Arc<Mutex<Store>>,
        session: String,
        place: String,
        project: Option<String>,
        model: String,
        notice: Notice,
    ) -> Arc<Self> {
        Arc::new(ChatSync {
            arena,
            store,
            session,
            place,
            project,
            model: Mutex::new(model),
            state: Mutex::new(State::default()),
            poke: Notify::new(),
            idle: Notify::new(),
            notice,
            unsupported: std::sync::atomic::AtomicBool::new(false),
            holding: std::sync::atomic::AtomicBool::new(false),
            hold_task: Mutex::new(None),
            draft_next: Mutex::new(None),
            drafting: std::sync::atomic::AtomicBool::new(false),
            ending: Mutex::new(None),
        })
    }

    /// Sends in the background from now on: at each [`poke`](Self::poke), and again until Arena has it all.
    pub fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let me = Arc::clone(self);
        tokio::spawn(async move { me.run().await })
    }

    async fn run(self: Arc<Self>) {
        let mut wait = Duration::from_millis(500);
        loop {
            match self.push().await {
                Pushed::Done | Pushed::Behind | Pushed::Off => {
                    wait = Duration::from_millis(500);
                    self.idle.notify_waiters();
                    self.poke.notified().await;
                }
                Pushed::Retry(after) => {
                    let after = after.max(wait);
                    wait = (wait * 2).min(RETRY_AT_MOST);
                    self.idle.notify_waiters();
                    tokio::select! {
                        _ = tokio::time::sleep(after) => {}
                        _ = self.poke.notified() => {}
                    }
                }
            }
        }
    }

    /// Sends in the background once the server says it keeps chats ([`Feature::ChatSync`]); what it answered is
    /// kept in `caps_file`. A server without it is left alone, and said so once.
    pub fn start_checked(self: &Arc<Self>, caps_file: std::path::PathBuf) {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            match me.arena.capabilities_kept(&caps_file).await {
                Ok(caps) if caps.has(ca_arena::Feature::ChatSync) => me.run().await,
                Ok(caps) => {
                    if let Ok(mut st) = me.state.lock() {
                        st.last_error = Some(format!(
                            "this Arena does not keep Code Arena's chats ({:?})",
                            caps.state(ca_arena::Feature::ChatSync)
                        ));
                    }
                    me.unsupported.store(true, std::sync::atomic::Ordering::Relaxed);
                    me.idle.notify_waiters();
                }
                // Not reached now: tried as each send would be, from the loop.
                Err(_) => me.run().await,
            }
        });
    }

    /// The session kept something new: send it.
    pub fn poke(&self) {
        self.poke.notify_one();
    }

    pub fn set_model(&self, model: &str) {
        if let Ok(mut m) = self.model.lock() {
            *m = model.to_string();
        }
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    /// Waits at most `limit` for what the session kept to reach Arena (a one-shot run, before it ends).
    pub async fn flush(&self, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        let failed = self.state.lock().map(|s| s.failed).unwrap_or(0);
        loop {
            if self.pending() == 0 {
                return true;
            }
            let idle = self.idle.notified();
            self.poke();
            if tokio::time::timeout_at(deadline, idle).await.is_err() {
                return self.pending() == 0;
            }
            // The web added meanwhile, or Arena is out of reach: the rest goes with the next run.
            if self.state.lock().map(|s| s.behind.is_some() || s.failed > failed).unwrap_or(false) {
                return self.pending() == 0;
            }
        }
    }

    /// How many messages Arena does not have yet.
    pub fn pending(&self) -> usize {
        if self.unsupported.load(std::sync::atomic::Ordering::Relaxed) {
            return 0;
        }
        let Ok(s) = self.store.lock() else { return 0 };
        if s.chat_link(&self.session).map(|l| l.off).unwrap_or(true) {
            return 0;
        }
        s.unsent(&self.session, BATCH).map(|v| v.len()).unwrap_or(0)
    }

    /// What the web added that this session lacks, as the last refused send said (taken by the take-in).
    pub fn take_behind(&self) -> Option<SyncRead> {
        self.state.lock().ok()?.behind.take()
    }

    /// `/sync off` keeps the session here only; `/sync on` sends it again (to a new chat when its own was deleted).
    pub fn set_on(&self, on: bool) {
        if let Ok(s) = self.store.lock() {
            let _ = s.set_chat_off(&self.session, !on);
        }
        if on {
            self.poke();
        }
    }

    /// Where it stands, in words (`/sync`).
    pub fn describe(&self) -> String {
        let link = self.store.lock().ok().and_then(|s| s.chat_link(&self.session).ok()).unwrap_or_default();
        let st = self.state.lock();
        let (sent, err, behind) = st.map(|s| (s.sent, s.last_error.clone(), s.behind.is_some())).unwrap_or_default();
        let pending = self.pending();
        if self.unsupported.load(std::sync::atomic::Ordering::Relaxed) {
            return format!("Kept here only: {}.", err.unwrap_or_default());
        }
        match (&link.chat, link.off) {
            (_, true) => "Kept here only: this session is not sent to Arena (/sync on sends it again).".into(),
            (None, _) => "Not in Arena yet: the chat is made at the first message.".into(),
            (Some(c), false) => {
                let mut line = format!("Kept with the chat {}/chat/{c}", self.arena.base());
                if pending > 0 {
                    line.push_str(&format!("; {pending} message(s) on their way"));
                } else {
                    line.push_str("; Arena has every message");
                }
                if sent > 0 {
                    line.push_str(&format!(" ({sent} sent now)"));
                }
                if behind {
                    line.push_str("; the web added messages, taken in at the next turn");
                }
                if let Some(e) = err {
                    line.push_str(&format!("; last problem: {e}"));
                }
                line
            }
        }
    }

    fn fail(&self, words: String) -> Pushed {
        let first = self
            .state
            .lock()
            .map(|mut s| {
                s.failed += 1;
                s.last_error = Some(words.clone());
                !std::mem::replace(&mut s.said_unreachable, true)
            })
            .unwrap_or(false);
        if first {
            (self.notice)(
                true,
                format!("The chat in Arena is not kept up now ({words}); it is sent when Arena answers."),
            );
        }
        Pushed::Retry(Duration::from_secs(1))
    }

    fn reached(&self) {
        if let Ok(mut s) = self.state.lock() {
            if s.said_unreachable {
                (self.notice)(false, "Arena is reached again: the chat is kept up.".into());
            }
            s.said_unreachable = false;
            s.last_error = None;
        }
    }

    /// The chat Arena keeps this session in, made the first time.
    async fn chat(&self) -> Result<Option<String>, Pushed> {
        let (link, first) = {
            let Ok(s) = self.store.lock() else { return Err(Pushed::Off) };
            let link = s.chat_link(&self.session).map_err(|_| Pushed::Off)?;
            let first = s.unsent(&self.session, 1).ok().and_then(|v| v.into_iter().next());
            (link, first)
        };
        if link.off {
            return Err(Pushed::Off);
        }
        if let Some(c) = link.chat {
            return Ok(Some(c));
        }
        let Some(first) = first else { return Ok(None) };
        let title = first.message.content.lines().find(|l| !l.trim().is_empty()).map(|l| l.chars().take(80).collect());
        let new = NewChat {
            reference: self.session.clone(),
            place: self.place.clone(),
            model: self.model.lock().ok().map(|m| m.clone()),
            title,
            project: self.project.clone(),
        };
        match self.arena.make_chat(&new).await {
            Ok(made) => {
                if let Ok(s) = self.store.lock() {
                    let _ = s.link_chat(&self.session, &made.id);
                }
                Ok(Some(made.id))
            }
            Err(e) => Err(self.fail(e.to_string())),
        }
    }

    /// Sends what Arena lacks, a batch at a time, then the last summary it lacks.
    pub async fn push(&self) -> Pushed {
        if self.state.lock().map(|s| s.behind.is_some()).unwrap_or(false) {
            return Pushed::Behind;
        }
        let chat = match self.chat().await {
            Ok(Some(c)) => c,
            Ok(None) => return Pushed::Done,
            Err(p) => return p,
        };
        loop {
            let (batch, parent, branch, messages) = {
                let Ok(s) = self.store.lock() else { return Pushed::Off };
                let batch = match s.unsent(&self.session, BATCH) {
                    Ok(b) => b,
                    Err(e) => return self.fail(e.to_string()),
                };
                let parent = s.last_sent(&self.session).ok().flatten().map(|(_, a)| a);
                let branch = s.chat_link(&self.session).map(|l| l.branch).unwrap_or(false);
                let model = self.model.lock().map(|m| m.clone()).unwrap_or_default();
                let messages: Vec<_> = batch.iter().map(|m| to_arena(&s, &self.session, m, &model)).collect();
                (batch, parent, branch, messages)
            };
            if batch.is_empty() {
                return self.push_summary(&chat).await;
            }
            match self.arena.append(&chat, parent.as_deref(), branch, &messages, None).await {
                Ok(Append::Done(done)) => {
                    self.reached();
                    let sent: Vec<(i64, String)> = batch.iter().map(|m| m.seq).zip(done.ids).collect();
                    if let Ok(s) = self.store.lock() {
                        let _ = s.mark_sent(&self.session, &sent);
                        let _ = s.set_chat_stamp(&self.session, Some(&done.stamp));
                    }
                    if let Ok(mut st) = self.state.lock() {
                        st.sent += sent.len();
                    }
                    if sent.len() < batch.len() {
                        return Pushed::Retry(Duration::from_secs(1));
                    }
                }
                Ok(Append::Behind(read)) => {
                    self.reached();
                    // Those it had already (sent before, their answer lost) are known by their ref.
                    if let Some(ids) = &read.ids
                        && !ids.is_empty()
                    {
                        let had: Vec<(i64, String)> = batch.iter().map(|m| m.seq).zip(ids.iter().cloned()).collect();
                        if let Ok(s) = self.store.lock() {
                            let _ = s.mark_sent(&self.session, &had);
                        }
                    }
                    if let Ok(mut st) = self.state.lock() {
                        st.behind = Some(read);
                    }
                    return Pushed::Behind;
                }
                Ok(Append::Answering) => {
                    self.reached();
                    return Pushed::Retry(Duration::from_secs(5));
                }
                Err(e) if chats::gone(&e) => return self.gone(),
                Err(e) => return self.fail(e.to_string()),
            }
        }
    }

    async fn push_summary(&self, chat: &str) -> Pushed {
        let unsent = self.store.lock().ok().and_then(|s| s.unsent_summary(&self.session).ok().flatten());
        let Some(u) = unsent else { return Pushed::Done };
        let parent = self.store.lock().ok().and_then(|s| s.last_sent(&self.session).ok().flatten()).map(|(_, a)| a);
        let summary = SyncSummary { up_to: u.up_to.clone(), text: Some(u.text.clone()) };
        match self.arena.append(chat, parent.as_deref(), false, &[], Some(&summary)).await {
            Ok(Append::Done(_)) => {
                if let Ok(s) = self.store.lock() {
                    let _ = s.summary_sent(&self.session, u.through);
                }
                Pushed::Done
            }
            Ok(Append::Behind(read)) => {
                if let Ok(mut st) = self.state.lock() {
                    st.behind = Some(read);
                }
                Pushed::Behind
            }
            Ok(Append::Answering) => Pushed::Retry(Duration::from_secs(5)),
            Err(e) if chats::gone(&e) => self.gone(),
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// The chat was deleted on the web: this session is kept here only.
    fn gone(&self) -> Pushed {
        if let Ok(s) = self.store.lock() {
            let _ = s.forget_chat(&self.session);
            let _ = s.set_chat_off(&self.session, true);
        }
        (self.notice)(
            true,
            "The chat was deleted in Arena: this session is kept here only (/sync on makes a new one).".into(),
        );
        Pushed::Off
    }
}

#[cfg(test)]
mod tests;
