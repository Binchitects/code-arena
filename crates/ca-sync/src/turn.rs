//! A turn's part: what the web added, taken in before it; the chat held while it runs (the web queues the person's
//! messages meanwhile, and they join the turn at its next step); the answer being written shown on the web; the chat
//! let go once Arena has the turn's messages.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ca_arena::chats::{self, SyncRead};
use ca_llm::{Message, Role};

use crate::{ChatSync, from_arena};

/// How often a turn renews its hold on the chat (Arena lets it go after 90 s without).
const RENEW: Duration = Duration::from_secs(30);
/// How often a turn asks for what the person queued on the web.
const QUEUE_EVERY: Duration = if cfg!(test) { Duration::from_millis(20) } else { Duration::from_secs(3) };
/// How often a turn's start asks again while the web chat answers.
const ANSWERING_EVERY: Duration = if cfg!(test) { Duration::from_millis(20) } else { Duration::from_secs(2) };
/// How long a turn's start waits for the web chat's own answer to end.
const WEB_ANSWER_WAIT: Duration = Duration::from_secs(120);
/// The most messages one read gives.
const READ: usize = 200;
/// How long the end of a turn waits for its messages to reach Arena before it lets the chat go anyway.
const END_WAIT: Duration = Duration::from_secs(20);

/// What came in from the web chat before a turn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Taken {
    /// The messages added, in order, as the session keeps them now.
    pub added: Vec<Message>,
    /// What the model reads changed beyond those added at its end (the web forked, or its messages went before some of
    /// the session's, or it compacted): read the session's history again.
    pub rebuilt: bool,
    /// Lines to tell the person.
    pub said: Vec<String>,
}

impl Taken {
    pub fn any(&self) -> bool {
        !self.added.is_empty() || self.rebuilt
    }
}

impl ChatSync {
    pub(crate) fn chat_id(&self) -> Option<String> {
        if self.unsupported.load(Ordering::Relaxed) {
            return None;
        }
        let s = self.store.lock().ok()?;
        let link = s.chat_link(&self.session).ok()?;
        if link.off { None } else { link.chat }
    }

    /// What the web added since the session last had it, written into the session. Nothing when there is nothing
    /// new, or when Arena cannot say now (it comes at the next turn).
    pub async fn take_in(&self) -> Taken {
        let Some(chat) = self.chat_id() else { return Taken::default() };
        let _one = self.io.lock().await;
        let mut taken = Taken::default();
        let mut pending = self.take_behind();
        loop {
            let read = match pending.take() {
                Some(r) => r,
                None => {
                    let (since, stamp) = {
                        let Ok(s) = self.store.lock() else { return taken };
                        let since = s.last_sent(&self.session).ok().flatten().map(|(_, a)| a);
                        let stamp = s.chat_link(&self.session).ok().and_then(|l| l.stamp);
                        (since, stamp)
                    };
                    // Asked with the stamp only while nothing is waiting here: a fresh read otherwise.
                    match self.arena.read_chat(&chat, since.as_deref(), stamp.as_deref(), READ).await {
                        Ok(Some(r)) => r,
                        // The web chat is answering: its messages come when its answer ends.
                        Ok(None) => return taken,
                        Err(e) if chats::gone(&e) => {
                            self.gone();
                            return taken;
                        }
                        Err(_) => return taken,
                    }
                }
            };
            if read.unchanged {
                return taken;
            }
            let more = read.more;
            if !self.write_in(&read, &mut taken) {
                break;
            }
            if !more {
                break;
            }
        }
        // What waited here goes after what came in.
        self.poke();
        taken
    }

    /// One read of the web's, written into the session; `false` when it could not be placed (read again next time).
    fn write_in(&self, read: &SyncRead, taken: &mut Taken) -> bool {
        let Ok(s) = self.store.lock() else { return false };
        let id = &self.session;
        // Where the web's messages go: after `base`, the last message both have. A base this session does not know
        // changes nothing: the session is not cut back on a guess.
        let base_seq = match &read.base {
            Some(b) => match s.seq_by_arena(id, b).ok().flatten() {
                Some(seq) => Some(seq),
                None => return false,
            },
            None => None,
        };
        let total = s.summary(id).map(|x| x.messages).unwrap_or(0);
        let after = base_seq.map(|b| b + 1).unwrap_or(0);
        // What the session has after the base: the web's side has moved past it.
        let mut kept: Vec<Message> = Vec::new();
        if after < total && (read.fork || !read.messages.is_empty()) {
            let page = s.unsent(id, 100_000).unwrap_or_default();
            if read.fork {
                // The web shows another branch (a question edited, an answer written again): the session follows it
                // from where they part; its own messages stay on their branch on the web.
                taken.said.push("The web chat went another way: this session follows it from where they part.".into());
            }
            // Not yet on the web: after the web's, sent next (a fork leaves them with the branch it left).
            if !read.fork {
                kept = page.into_iter().filter(|m| m.seq >= after).map(|m| m.message).collect();
            }
            let _ = s.truncate(id, after);
            // Following the web is not a branch of the session's own.
            let _ = s.set_chat_branch(id, false);
            taken.rebuilt = true;
        }
        for m in &read.messages {
            let (Some(arena), Some(msg)) = (&m.id, from_arena(m)) else { continue };
            if s.seq_by_arena(id, arena).ok().flatten().is_some() {
                continue;
            }
            let begins = msg.role == Role::User && m.kind.as_deref() != Some("note");
            if s.append_from_arena(id, &msg, arena, begins).is_ok() {
                if msg.role == Role::User && !msg.content.is_empty() {
                    taken.said.push(format!("From the web: {}", first_line(&msg.content)));
                } else if msg.role == Role::Assistant && msg.tool_calls.is_empty() && !msg.content.trim().is_empty() {
                    taken.said.push(format!("Answered on the web:\n{}", msg.content.trim()));
                }
                taken.added.push(msg);
            }
        }
        // The session's own, not on the web yet: after the web's, sent next.
        for m in &kept {
            let _ = s.append(id, m, m.role == Role::User && m.tool_call_id.is_none());
        }
        if let Some(sum) = &read.summary
            && let Some(text) = &sum.text
            && let Ok(Some(through)) = s.seq_by_arena(id, &sum.up_to)
        {
            let had = s.model_view(id).ok().and_then(|v| v.through);
            if had.is_none_or(|h| h < through) {
                let _ = s.compact_from_arena(id, through, text);
                taken.rebuilt = true;
            }
        }
        let _ = s.set_chat_stamp(id, Some(&read.stamp));
        true
    }

    /// A turn starts here: what the web added is taken in, then the chat is the session's until the turn ends (the web
    /// queues the person's messages meanwhile, and what it queues joins the turn through `inbox`). While the web chat
    /// is answering, the turn waits for its answer, at most two minutes.
    pub async fn begin_turn(self: &Arc<Self>, inbox: Option<Arc<Mutex<Vec<String>>>>) -> Taken {
        // The last turn's end first: its letting go must not let go of this one.
        let ending = self.ending.lock().ok().and_then(|mut e| e.take());
        if let Some(e) = ending {
            let _ = e.await;
        }
        let mut taken = self.take_in().await;
        let Some(chat) = self.chat_id() else { return taken };
        let deadline = tokio::time::Instant::now() + WEB_ANSWER_WAIT;
        let mut said = false;
        loop {
            match self.arena.hold_turn(&chat).await {
                Ok(true) => break,
                Ok(false) if tokio::time::Instant::now() < deadline => {
                    if !said {
                        (self.notice)(
                            false,
                            "The web chat is answering: this turn starts when its answer ends.".into(),
                        );
                        said = true;
                    }
                    tokio::time::sleep(ANSWERING_EVERY).await;
                }
                Ok(false) => {
                    taken.said.push("The web chat is still answering: this turn goes on without its answer.".into());
                    return taken;
                }
                Err(e) if chats::gone(&e) => {
                    self.gone();
                    return taken;
                }
                // Out of reach: the turn goes on, and its messages go when Arena answers.
                Err(_) => return taken,
            }
        }
        if said {
            // Its answer came meanwhile.
            let more = self.take_in().await;
            taken.rebuilt |= more.rebuilt;
            taken.added.extend(more.added);
            taken.said.extend(more.said);
        }
        self.holding.store(true, Ordering::Relaxed);
        let me = Arc::clone(self);
        let task = tokio::spawn(async move { me.hold(chat, inbox).await });
        if let Ok(mut h) = self.hold_task.lock()
            && let Some(old) = h.replace(task)
        {
            old.abort();
        }
        taken
    }

    /// While the turn runs: the hold renewed, and the web's queue taken into the turn.
    async fn hold(self: Arc<Self>, chat: String, inbox: Option<Arc<Mutex<Vec<String>>>>) {
        let mut renewed = tokio::time::Instant::now();
        loop {
            tokio::time::sleep(QUEUE_EVERY).await;
            if renewed.elapsed() >= RENEW {
                let _ = self.arena.hold_turn(&chat).await;
                renewed = tokio::time::Instant::now();
            }
            if let Some(inbox) = &inbox
                && let Ok(q) = self.arena.take_queued(&chat).await
                && !q.is_empty()
                && let Ok(mut i) = inbox.lock()
            {
                i.extend(q.into_iter().map(|m| m.content));
            }
        }
    }

    /// The turn ended, in the background: the prompt does not wait for Arena (the next turn's start does).
    pub fn end_turn_soon(self: &Arc<Self>) {
        let me = Arc::clone(self);
        let task = tokio::spawn(async move { me.end_turn().await });
        if let Ok(mut e) = self.ending.lock() {
            *e = Some(task);
        }
    }

    /// The turn ended: the chat is let go once Arena has its messages (or after 20 s without).
    pub async fn end_turn(&self) {
        if let Ok(mut h) = self.hold_task.lock()
            && let Some(t) = h.take()
        {
            t.abort();
        }
        if !self.holding.swap(false, Ordering::Relaxed) {
            return;
        }
        // A draft on its way lands before the turn's end, which clears it there; one not sent yet is not needed now.
        if let Ok(mut d) = self.draft_next.lock() {
            d.take();
        }
        let until = tokio::time::Instant::now() + Duration::from_secs(5);
        while self.drafting.load(Ordering::Acquire) && tokio::time::Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.flush(END_WAIT).await;
        if let Some(chat) = self.chat_id() {
            let _ = self.arena.end_turn(&chat).await;
        }
    }

    /// The answer being written, as it stands: shown on the web while the turn holds the chat. One on its way at a
    /// time; a newer one replaces any not sent yet.
    pub fn draft(self: &Arc<Self>, text: &str, reasoning: &str) {
        if !self.holding.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(mut d) = self.draft_next.lock() {
            *d = Some((text.to_string(), reasoning.to_string()));
        }
        if self.drafting.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let next = me.draft_next.lock().ok().and_then(|mut d| d.take());
                let Some((text, reasoning)) = next else { break };
                if let Some(chat) = me.chat_id() {
                    let _ = me.arena.draft(&chat, &text, &reasoning).await;
                }
            }
            me.drafting.store(false, Ordering::Release);
        });
    }
}

fn first_line(text: &str) -> String {
    let l = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if l.chars().count() > 100 { format!("{}…", l.chars().take(99).collect::<String>()) } else { l.to_string() }
}
