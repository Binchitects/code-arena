//! The web's news while the session waits (the chat's prompt): from Arena's stream of the person's chats' changes as
//! they happen; while it cannot be had (an Arena or a proxy without it), by asking every few seconds with the stamp,
//! which costs Arena nothing when nothing moved.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::ChatSync;

/// How long the stream may be quiet (Arena says something every 15 s) before it is opened again.
const QUIET: Duration = Duration::from_secs(45);
/// How often the chat is asked while the stream cannot be had.
const ASK_EVERY: Duration = if cfg!(test) { Duration::from_millis(30) } else { Duration::from_secs(5) };
/// After the stream could not be opened, how long before it is tried again.
const STREAM_RETRY: Duration = if cfg!(test) { Duration::from_millis(200) } else { Duration::from_secs(60) };

/// Told that the web chat has news: the listener takes it in when it may (not during a turn, which takes it in itself).
pub type News = tokio::sync::mpsc::UnboundedSender<()>;

impl ChatSync {
    /// Listens for the web's news from now on; `news` is told each time the chat changed there.
    pub fn listen(self: &Arc<Self>, news: News) {
        let me = Arc::clone(self);
        tokio::spawn(async move { me.watch_loop(news).await });
    }

    /// Whether the stream is open now (the web's messages come at once), for `/sync`.
    pub fn streaming(&self) -> bool {
        self.streaming.load(Ordering::Relaxed)
    }

    async fn watch_loop(self: Arc<Self>, news: News) {
        loop {
            if news.is_closed() {
                return;
            }
            match self.arena.watch().await {
                Ok(mut w) => {
                    self.streaming.store(true, Ordering::Relaxed);
                    loop {
                        match tokio::time::timeout(QUIET, w.next()).await {
                            Ok(Some(Some(e))) if e.kind == "chat" => {
                                let ours = self.chat_id();
                                if ours.is_some() && e.id == ours && !self.holding.load(Ordering::Relaxed) {
                                    let _ = news.send(());
                                }
                            }
                            // "watching", or a heartbeat.
                            Ok(Some(_)) => {}
                            // Ended, or quiet too long: opened again.
                            Ok(None) | Err(_) => break,
                        }
                        if news.is_closed() {
                            return;
                        }
                    }
                    self.streaming.store(false, Ordering::Relaxed);
                }
                Err(_) => {
                    // Asked every few seconds until the stream is tried again.
                    let until = tokio::time::Instant::now() + STREAM_RETRY;
                    while tokio::time::Instant::now() < until {
                        tokio::time::sleep(ASK_EVERY).await;
                        if news.is_closed() {
                            return;
                        }
                        if !self.holding.load(Ordering::Relaxed) && self.has_news().await {
                            let _ = news.send(());
                        }
                    }
                }
            }
        }
    }

    /// Whether the web chat moved since the session last had it (asked with the stamp).
    async fn has_news(&self) -> bool {
        let Some(chat) = self.chat_id() else { return false };
        let (since, stamp) = {
            let Ok(s) = self.store.lock() else { return false };
            let since = s.last_sent(&self.session).ok().flatten().map(|(_, a)| a);
            let stamp = s.chat_link(&self.session).ok().and_then(|l| l.stamp);
            (since, stamp)
        };
        match self.arena.read_chat(&chat, since.as_deref(), stamp.as_deref(), 1).await {
            Ok(Some(r)) => !r.unchanged && (!r.messages.is_empty() || r.fork),
            _ => false,
        }
    }
}
