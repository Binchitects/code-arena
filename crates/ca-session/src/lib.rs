//! Sessions kept in one SQLite database in the data folder: every message on disk before the turn goes on, read
//! back a page of turns at a time, searched with FTS5, with compactions, results kept aside and the answer being
//! written kept as a draft. Nothing reads a whole session to show its end.

use std::path::Path;

use ca_llm::Message;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("the session store: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("a stored message could not be read: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no session {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, SessionError>;

/// The schema's version. Before 1.0 a store of another version is not migrated: it is moved aside and a new one
/// begun (the plan's "no backward compatibility before 1.0").
const SCHEMA: i64 = 2;

const CREATE: &str = r#"
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  folder TEXT NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  mode TEXT NOT NULL DEFAULT '',
  created INTEGER NOT NULL,
  updated INTEGER NOT NULL,
  messages INTEGER NOT NULL DEFAULT 0,
  turns INTEGER NOT NULL DEFAULT 0,
  chat TEXT,
  chat_stamp TEXT,
  chat_branch INTEGER NOT NULL DEFAULT 0,
  chat_off INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX sessions_by_folder ON sessions(folder, updated DESC);
CREATE TABLE messages (
  session TEXT NOT NULL,
  seq INTEGER NOT NULL,
  turn INTEGER NOT NULL,
  role TEXT NOT NULL,
  body TEXT NOT NULL,
  at INTEGER NOT NULL,
  arena TEXT,
  PRIMARY KEY (session, seq)
) WITHOUT ROWID;
CREATE INDEX messages_by_turn ON messages(session, turn);
CREATE VIRTUAL TABLE messages_text USING fts5(text, session UNINDEXED, seq UNINDEXED);
CREATE TABLE compactions (
  session TEXT NOT NULL,
  through INTEGER NOT NULL,
  summary TEXT NOT NULL,
  at INTEGER NOT NULL,
  sent INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (session, through)
) WITHOUT ROWID;
CREATE TABLE results (
  session TEXT NOT NULL,
  id TEXT NOT NULL,
  text TEXT NOT NULL,
  PRIMARY KEY (session, id)
) WITHOUT ROWID;
CREATE TABLE drafts (
  session TEXT PRIMARY KEY,
  content TEXT NOT NULL,
  reasoning TEXT NOT NULL,
  at INTEGER NOT NULL
);
"#;

/// From schema 1 (0.2) to 2: what keeps a session in step with its chat in Arena. Sessions are kept.
const UPGRADE_1: &str = r#"
ALTER TABLE sessions ADD COLUMN chat TEXT;
ALTER TABLE sessions ADD COLUMN chat_stamp TEXT;
ALTER TABLE sessions ADD COLUMN chat_branch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN chat_off INTEGER NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN arena TEXT;
ALTER TABLE compactions ADD COLUMN sent INTEGER NOT NULL DEFAULT 0;
"#;

/// Where a session stands with its chat in Arena.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatLink {
    /// Arena's chat, once made.
    pub chat: Option<String>,
    /// The branch's stamp as last read in step: asked with it, Arena says only "unchanged".
    pub stamp: Option<String>,
    /// The session went back to before messages Arena has: what it sends next goes on a branch of its own.
    pub branch: bool,
    /// Kept apart from Arena (`/sync off`, or its chat was deleted there).
    pub off: bool,
}

/// A summary Arena does not have yet, and the Arena id of the last message it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsentSummary {
    pub through: i64,
    pub text: String,
    pub up_to: String,
}

/// A session as the lists show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub id: String,
    pub folder: String,
    pub title: String,
    pub model: String,
    pub mode: String,
    pub created: i64,
    pub updated: i64,
    pub messages: i64,
    pub turns: i64,
}

/// A message by its place in the session (from 0, never changed by a compaction) and the turn it is in.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub seq: i64,
    pub turn: i64,
    pub message: Message,
}

/// A page of a session: its last turns, and how many came before them.
#[derive(Debug, Clone)]
pub struct Page {
    pub messages: Vec<Stored>,
    pub first_turn: i64,
    pub turns_before: i64,
}

/// What the model reads: the last summary, then the messages after what it covers.
#[derive(Debug, Clone)]
pub struct ModelView {
    pub summary: Option<String>,
    /// The last message the summary covers.
    pub through: Option<i64>,
    pub messages: Vec<Message>,
}

pub struct Store {
    db: Connection,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version == 0 {
            db.execute_batch(CREATE)?;
            db.pragma_update(None, "user_version", SCHEMA)?;
        } else if version == 1 {
            db.execute_batch(UPGRADE_1)?;
            db.pragma_update(None, "user_version", SCHEMA)?;
        } else if version != SCHEMA {
            drop(db);
            let aside = path.with_extension(format!("v{version}.db"));
            std::fs::rename(path, &aside)
                .map_err(|e| SessionError::Sql(rusqlite::Error::InvalidPath(aside.join(e.to_string()))))?;
            return Self::open(path);
        }
        Ok(Self { db })
    }

    pub fn in_memory() -> Result<Self> {
        let db = Connection::open_in_memory()?;
        db.execute_batch(CREATE)?;
        Ok(Self { db })
    }

    /// A new session in `folder`. Its id says when it began (`20261126-091530-3f2a1c`).
    pub fn create(&self, folder: &str, model: &str, mode: &str) -> Result<String> {
        let t = chrono::Local::now();
        let id = format!(
            "{}-{:06x}",
            t.format("%Y%m%d-%H%M%S"),
            (t.timestamp_subsec_nanos() ^ std::process::id()) & 0xff_ffff
        );
        let at = now();
        self.db.execute(
            "INSERT INTO sessions (id, folder, model, mode, created, updated) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![id, folder, model, mode, at],
        )?;
        Ok(id)
    }

    pub fn summary(&self, id: &str) -> Result<Summary> {
        self.db
            .query_row(
                "SELECT id, folder, title, model, mode, created, updated, messages, turns FROM sessions WHERE id = ?1",
                [id],
                row_summary,
            )
            .optional()?
            .ok_or_else(|| SessionError::NotFound(id.to_string()))
    }

    /// The folder's sessions, newest first.
    pub fn list(&self, folder: &str, limit: usize) -> Result<Vec<Summary>> {
        let mut q = self.db.prepare(
            "SELECT id, folder, title, model, mode, created, updated, messages, turns FROM sessions WHERE folder = ?1 AND messages > 0 ORDER BY updated DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = q.query_map(params![folder, limit as i64], row_summary)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The folder's last session that has messages.
    pub fn latest(&self, folder: &str) -> Result<Option<Summary>> {
        Ok(self.list(folder, 1)?.into_iter().next())
    }

    /// Appends a message, on disk before this returns. A person's message begins a turn. Its place is given back.
    pub fn append(&self, id: &str, message: &Message, begins_turn: bool) -> Result<i64> {
        let tx = self.db.unchecked_transaction()?;
        let (messages, turns): (i64, i64) = tx
            .query_row("SELECT messages, turns FROM sessions WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(|| SessionError::NotFound(id.to_string()))?;
        let turn = if begins_turn { turns + 1 } else { turns.max(1) };
        let role = serde_json::to_value(message.role)?.as_str().unwrap_or("user").to_string();
        let body = serde_json::to_string(message)?;
        let at = now();
        tx.execute(
            "INSERT INTO messages (session, seq, turn, role, body, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, messages, turn, role, body, at],
        )?;
        if !message.content.is_empty() {
            tx.execute(
                "INSERT INTO messages_text (text, session, seq) VALUES (?1, ?2, ?3)",
                params![message.content, id, messages],
            )?;
        }
        let title_update = if begins_turn && messages == 0 { Some(title_of(&message.content)) } else { None };
        tx.execute(
            "UPDATE sessions SET messages = ?2, turns = ?3, updated = ?4, title = COALESCE(?5, title) WHERE id = ?1",
            params![id, messages + 1, turn, at, title_update],
        )?;
        tx.execute("DELETE FROM drafts WHERE session = ?1", [id])?;
        tx.commit()?;
        Ok(messages)
    }

    pub fn set_model(&self, id: &str, model: &str, mode: &str) -> Result<()> {
        self.db.execute("UPDATE sessions SET model = ?2, mode = ?3 WHERE id = ?1", params![id, model, mode])?;
        Ok(())
    }

    /// The last `turns` turns, or those before `before_turn`: a page, read by the index, whatever the session's length.
    pub fn page(&self, id: &str, turns: i64, before_turn: Option<i64>) -> Result<Page> {
        let total: i64 = self.summary(id)?.turns;
        let end = before_turn.unwrap_or(total + 1);
        let first = (end - turns).max(0);
        let mut q = self.db.prepare(
            "SELECT seq, turn, body FROM messages WHERE session = ?1 AND turn >= ?2 AND turn < ?3 ORDER BY seq",
        )?;
        let rows = q.query_map(params![id, first, end], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
        })?;
        let mut messages = Vec::new();
        for row in rows {
            let (seq, turn, body) = row?;
            messages.push(Stored { seq, turn, message: serde_json::from_str(&body)? });
        }
        Ok(Page { messages, first_turn: first.max(1), turns_before: (first - 1).max(0) })
    }

    /// What the model reads: the latest summary and what follows it. Reading starts at the last compaction.
    pub fn model_view(&self, id: &str) -> Result<ModelView> {
        let last: Option<(i64, String)> = self
            .db
            .query_row(
                "SELECT through, summary FROM compactions WHERE session = ?1 ORDER BY through DESC LIMIT 1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let from = last.as_ref().map(|(t, _)| t + 1).unwrap_or(0);
        let mut q = self.db.prepare("SELECT body FROM messages WHERE session = ?1 AND seq >= ?2 ORDER BY seq")?;
        let rows = q.query_map(params![id, from], |r| r.get::<_, String>(0))?;
        let mut messages = Vec::new();
        for b in rows {
            messages.push(serde_json::from_str(&b?)?);
        }
        Ok(ModelView { through: last.as_ref().map(|(t, _)| *t), summary: last.map(|(_, s)| s), messages })
    }

    /// A compaction: the model reads `summary` in place of messages up to `through`. Every message stays.
    pub fn compact(&self, id: &str, through: i64, summary: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO compactions (session, through, summary, at) VALUES (?1, ?2, ?3, ?4)",
            params![id, through, summary, now()],
        )?;
        Ok(())
    }

    /// The session cut back to before message `seq` (a rewind): later messages and compactions go.
    pub fn truncate(&self, id: &str, seq: i64) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let sent: i64 = tx.query_row(
            "SELECT COUNT(*) FROM messages WHERE session = ?1 AND seq >= ?2 AND arena IS NOT NULL",
            params![id, seq],
            |r| r.get(0),
        )?;
        if sent > 0 {
            tx.execute("UPDATE sessions SET chat_branch = 1 WHERE id = ?1", [id])?;
        }
        tx.execute("DELETE FROM messages WHERE session = ?1 AND seq >= ?2", params![id, seq])?;
        tx.execute("DELETE FROM messages_text WHERE session = ?1 AND seq >= ?2", params![id, seq])?;
        tx.execute("DELETE FROM compactions WHERE session = ?1 AND through >= ?2", params![id, seq])?;
        let turns: i64 =
            tx.query_row("SELECT COALESCE(MAX(turn), 0) FROM messages WHERE session = ?1", [id], |r| r.get(0))?;
        tx.execute(
            "UPDATE sessions SET messages = ?2, turns = ?3, updated = ?4 WHERE id = ?1",
            params![id, seq, turns, now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// A tool result kept aside (`r1`, `r2`…), for `read_result`.
    pub fn keep_result(&self, id: &str, text: &str) -> Result<String> {
        let n: i64 = self.db.query_row("SELECT COUNT(*) FROM results WHERE session = ?1", [id], |r| r.get(0))?;
        let rid = format!("r{}", n + 1);
        self.db.execute("INSERT INTO results (session, id, text) VALUES (?1, ?2, ?3)", params![id, rid, text])?;
        Ok(rid)
    }

    pub fn result(&self, id: &str, rid: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT text FROM results WHERE session = ?1 AND id = ?2", params![id, rid], |r| r.get(0))
            .optional()?)
    }

    /// The answer being written, kept every few seconds; a crash leaves it to be recovered.
    pub fn draft(&self, id: &str, content: &str, reasoning: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO drafts (session, content, reasoning, at) VALUES (?1, ?2, ?3, ?4)",
            params![id, content, reasoning, now()],
        )?;
        Ok(())
    }

    /// A draft left by a run that ended before its answer did, appended as the answer it got to.
    pub fn recover_draft(&self, id: &str) -> Result<Option<Message>> {
        let d: Option<(String, String)> = self
            .db
            .query_row("SELECT content, reasoning FROM drafts WHERE session = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((content, reasoning)) = d else { return Ok(None) };
        let mut m = Message::assistant(format!("{content}\n\n(Code Arena closed before this answer ended.)"));
        m.reasoning = (!reasoning.is_empty()).then_some(reasoning);
        self.append(id, &m, false)?;
        Ok(Some(m))
    }

    /// Where the session stands with its chat in Arena.
    pub fn chat_link(&self, id: &str) -> Result<ChatLink> {
        self.db
            .query_row("SELECT chat, chat_stamp, chat_branch, chat_off FROM sessions WHERE id = ?1", [id], |r| {
                Ok(ChatLink {
                    chat: r.get(0)?,
                    stamp: r.get(1)?,
                    branch: r.get::<_, i64>(2)? != 0,
                    off: r.get::<_, i64>(3)? != 0,
                })
            })
            .optional()?
            .ok_or_else(|| SessionError::NotFound(id.to_string()))
    }

    /// The session is kept with this chat from now on.
    pub fn link_chat(&self, id: &str, chat: &str) -> Result<()> {
        self.db.execute("UPDATE sessions SET chat = ?2, chat_off = 0 WHERE id = ?1", params![id, chat])?;
        Ok(())
    }

    /// Kept apart from Arena (or with it again).
    pub fn set_chat_off(&self, id: &str, off: bool) -> Result<()> {
        self.db.execute("UPDATE sessions SET chat_off = ?2 WHERE id = ?1", params![id, off as i64])?;
        Ok(())
    }

    /// The chat is gone (deleted in Arena): what the session sent there is forgotten, so it goes again to a new one.
    pub fn forget_chat(&self, id: &str) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute("UPDATE sessions SET chat = NULL, chat_stamp = NULL, chat_branch = 0 WHERE id = ?1", [id])?;
        tx.execute("UPDATE messages SET arena = NULL WHERE session = ?1", [id])?;
        tx.execute("UPDATE compactions SET sent = 0 WHERE session = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_chat_stamp(&self, id: &str, stamp: Option<&str>) -> Result<()> {
        self.db.execute("UPDATE sessions SET chat_stamp = ?2 WHERE id = ?1", params![id, stamp])?;
        Ok(())
    }

    pub fn set_chat_branch(&self, id: &str, branch: bool) -> Result<()> {
        self.db.execute("UPDATE sessions SET chat_branch = ?2 WHERE id = ?1", params![id, branch as i64])?;
        Ok(())
    }

    /// The last message Arena has, and its id there.
    pub fn last_sent(&self, id: &str) -> Result<Option<(i64, String)>> {
        Ok(self
            .db
            .query_row(
                "SELECT seq, arena FROM messages WHERE session = ?1 AND arena IS NOT NULL ORDER BY seq DESC LIMIT 1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// Arena's id for a message, when it has it.
    pub fn arena_id(&self, id: &str, seq: i64) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT arena FROM messages WHERE session = ?1 AND seq = ?2", params![id, seq], |r| r.get(0))
            .optional()?
            .flatten())
    }

    /// The first messages Arena does not have, after the last it has, in order.
    pub fn unsent(&self, id: &str, limit: usize) -> Result<Vec<Stored>> {
        let after = self.last_sent(id)?.map(|(s, _)| s).unwrap_or(-1);
        let mut q = self.db.prepare(
            "SELECT seq, turn, body FROM messages WHERE session = ?1 AND seq > ?2 AND arena IS NULL ORDER BY seq LIMIT ?3",
        )?;
        let rows = q.query_map(params![id, after, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, turn, body) = row?;
            out.push(Stored { seq, turn, message: serde_json::from_str(&body)? });
        }
        Ok(out)
    }

    /// The name of the tool a call id belongs to, from the last messages before `seq` (a result's tool, for Arena).
    pub fn call_name(&self, id: &str, before: i64, call: &str) -> Result<Option<String>> {
        let mut q = self.db.prepare(
            "SELECT body FROM messages WHERE session = ?1 AND seq < ?2 AND role = 'assistant' ORDER BY seq DESC LIMIT 20",
        )?;
        let rows = q.query_map(params![id, before], |r| r.get::<_, String>(0))?;
        for b in rows {
            let m: Message = serde_json::from_str(&b?)?;
            if let Some(c) = m.tool_calls.iter().find(|c| c.id == call) {
                return Ok(Some(c.name.clone()));
            }
        }
        Ok(None)
    }

    /// Arena took these messages: each place with its id there. What the session sends next follows them.
    pub fn mark_sent(&self, id: &str, sent: &[(i64, String)]) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        for (seq, arena) in sent {
            tx.execute("UPDATE messages SET arena = ?3 WHERE session = ?1 AND seq = ?2", params![id, seq, arena])?;
        }
        tx.execute("UPDATE sessions SET chat_branch = 0 WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// A message the web added, kept with its id there: the next turn's model reads it.
    pub fn append_from_arena(&self, id: &str, message: &Message, arena: &str, begins_turn: bool) -> Result<i64> {
        let seq = self.append(id, message, begins_turn)?;
        self.db.execute("UPDATE messages SET arena = ?3 WHERE session = ?1 AND seq = ?2", params![id, seq, arena])?;
        Ok(seq)
    }

    /// The last summary Arena does not have, once Arena has the message it covers up to.
    pub fn unsent_summary(&self, id: &str) -> Result<Option<UnsentSummary>> {
        Ok(self
            .db
            .query_row(
                "SELECT c.through, c.summary, m.arena FROM compactions c JOIN messages m ON m.session = c.session AND m.seq = c.through
                 WHERE c.session = ?1 AND c.sent = 0 AND m.arena IS NOT NULL ORDER BY c.through DESC LIMIT 1",
                [id],
                |r| Ok(UnsentSummary { through: r.get(0)?, text: r.get(1)?, up_to: r.get(2)? }),
            )
            .optional()?)
    }

    /// Arena has the summary up to `through` (and every one before it).
    pub fn summary_sent(&self, id: &str, through: i64) -> Result<()> {
        self.db
            .execute("UPDATE compactions SET sent = 1 WHERE session = ?1 AND through <= ?2", params![id, through])?;
        Ok(())
    }

    /// The place of the message Arena knows by this id.
    pub fn seq_by_arena(&self, id: &str, arena: &str) -> Result<Option<i64>> {
        Ok(self
            .db
            .query_row("SELECT seq FROM messages WHERE session = ?1 AND arena = ?2", params![id, arena], |r| r.get(0))
            .optional()?)
    }

    /// A summary the web's compaction wrote, kept as Arena has it already.
    pub fn compact_from_arena(&self, id: &str, through: i64, summary: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO compactions (session, through, summary, at, sent) VALUES (?1, ?2, ?3, ?4, 1)",
            params![id, through, summary, now()],
        )?;
        Ok(())
    }

    /// The session kept with this chat, if there is one.
    pub fn by_chat(&self, chat: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT id FROM sessions WHERE chat = ?1 ORDER BY updated DESC LIMIT 1", [chat], |r| r.get(0))
            .optional()?)
    }

    /// Messages of the folder's sessions that hold every word: (session, seq, a line of the text).
    pub fn search(&self, folder: &str, words: &str, limit: usize) -> Result<Vec<(String, i64, String)>> {
        let query =
            words.split_whitespace().map(|w| format!("\"{}\"", w.replace('"', ""))).collect::<Vec<_>>().join(" ");
        if query.is_empty() {
            return Ok(vec![]);
        }
        let mut q = self.db.prepare(
            "SELECT t.session, t.seq, snippet(messages_text, 0, '', '', '…', 12) FROM messages_text t JOIN sessions s ON s.id = t.session
             WHERE messages_text MATCH ?1 AND s.folder = ?2 ORDER BY s.updated DESC, t.seq LIMIT ?3",
        )?;
        let rows = q.query_map(params![query, folder, limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

fn row_summary(r: &rusqlite::Row<'_>) -> rusqlite::Result<Summary> {
    Ok(Summary {
        id: r.get(0)?,
        folder: r.get(1)?,
        title: r.get(2)?,
        model: r.get(3)?,
        mode: r.get(4)?,
        created: r.get(5)?,
        updated: r.get(6)?,
        messages: r.get(7)?,
        turns: r.get(8)?,
    })
}

/// A session's title: its first message's first line, cut at 80 characters.
fn title_of(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > 80 {
        format!("{}…", line.chars().take(79).collect::<String>())
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ca_llm::ToolCall;

    fn turn(s: &Store, id: &str, n: usize) {
        s.append(id, &Message::user(format!("question {n}")), true).expect("user");
        let mut a = Message::assistant("");
        a.tool_calls.push(ToolCall { id: format!("c{n}"), name: "read_file".into(), arguments: "{}".into() });
        s.append(id, &a, false).expect("call");
        s.append(id, &Message::tool(format!("c{n}"), "result"), false).expect("result");
        s.append(id, &Message::assistant(format!("answer {n}")), false).expect("answer");
    }

    #[test]
    fn pages_of_turns_from_the_end() {
        let s = Store::in_memory().expect("store");
        let id = s.create("/w", "m", "ask").expect("create");
        for n in 1..=100 {
            turn(&s, &id, n);
        }
        let sum = s.summary(&id).expect("summary");
        assert_eq!((sum.messages, sum.turns, sum.title.as_str()), (400, 100, "question 1"));
        let p = s.page(&id, 30, None).expect("page");
        assert_eq!(p.messages.len(), 120);
        assert_eq!(p.messages[0].message.content, "question 71");
        assert_eq!(p.turns_before, 70);
        let earlier = s.page(&id, 30, Some(p.first_turn)).expect("page");
        assert_eq!(earlier.messages[0].message.content, "question 41");
        assert_eq!(earlier.messages.last().expect("last").message.content, "answer 70");
    }

    #[test]
    fn the_model_reads_from_the_last_compaction() {
        let s = Store::in_memory().expect("store");
        let id = s.create("/w", "m", "ask").expect("create");
        for n in 1..=3 {
            turn(&s, &id, n);
        }
        s.compact(&id, 7, "## Goal\nanswer questions").expect("compact");
        let v = s.model_view(&id).expect("view");
        assert_eq!(v.summary.as_deref(), Some("## Goal\nanswer questions"));
        assert_eq!(v.messages.len(), 4);
        assert_eq!(v.messages[0].content, "question 3");
        // Every message stays for the person.
        assert_eq!(s.page(&id, 10, None).expect("page").messages.len(), 12);
    }

    #[test]
    fn search_rewind_results_and_drafts() {
        let s = Store::in_memory().expect("store");
        let id = s.create("/w", "m", "ask").expect("create");
        turn(&s, &id, 1);
        s.append(&id, &Message::user("where are the parser tests"), true).expect("u");
        let hits = s.search("/w", "parser tests", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, 4);
        assert!(s.search("/other", "parser", 10).expect("search").is_empty());

        s.truncate(&id, 4).expect("rewind");
        assert_eq!(s.summary(&id).expect("s").messages, 4);
        assert!(s.search("/w", "parser", 10).expect("search").is_empty());

        let r = s.keep_result(&id, "a long output").expect("keep");
        assert_eq!(r, "r1");
        assert_eq!(s.result(&id, "r1").expect("read").as_deref(), Some("a long output"));

        s.draft(&id, "half an ans", "").expect("draft");
        let m = s.recover_draft(&id).expect("recover").expect("there");
        assert!(m.content.starts_with("half an ans"));
        assert!(s.recover_draft(&id).expect("again").is_none());
    }

    #[test]
    fn what_arena_has_and_what_it_lacks() {
        let s = Store::in_memory().expect("store");
        let id = s.create("/w", "m", "ask").expect("create");
        turn(&s, &id, 1);
        assert_eq!(s.chat_link(&id).expect("link"), ChatLink::default());
        s.link_chat(&id, "chat-1").expect("link");
        assert_eq!(s.unsent(&id, 10).expect("unsent").len(), 4);
        assert_eq!(s.call_name(&id, 2, "c1").expect("name").as_deref(), Some("read_file"));
        s.mark_sent(&id, &[(0, "a0".into()), (1, "a1".into())]).expect("sent");
        assert_eq!(s.last_sent(&id).expect("last"), Some((1, "a1".into())));
        assert_eq!(s.unsent(&id, 10).expect("unsent").iter().map(|m| m.seq).collect::<Vec<_>>(), vec![2, 3]);
        s.mark_sent(&id, &[(2, "a2".into()), (3, "a3".into())]).expect("sent");
        // A summary goes once Arena has what it covers.
        s.compact(&id, 3, "the summary").expect("compact");
        let u = s.unsent_summary(&id).expect("summary").expect("one");
        assert_eq!((u.through, u.up_to.as_str()), (3, "a3"));
        s.summary_sent(&id, 3).expect("sent");
        assert!(s.unsent_summary(&id).expect("summary").is_none());
        // The web's message comes in with its id.
        s.append_from_arena(&id, &Message::user("from the web"), "w4", true).expect("in");
        assert_eq!(s.last_sent(&id).expect("last"), Some((4, "w4".into())));
        // Going back to before what Arena has: the next send is a branch.
        s.truncate(&id, 4).expect("rewind");
        assert!(s.chat_link(&id).expect("link").branch);
        s.append(&id, &Message::user("another way"), true).expect("u");
        s.mark_sent(&id, &[(4, "b4".into())]).expect("sent");
        assert!(!s.chat_link(&id).expect("link").branch);
        assert_eq!(s.by_chat("chat-1").expect("by chat").as_deref(), Some(id.as_str()));
    }

    #[test]
    fn a_0_2_store_is_upgraded_and_keeps_its_sessions() {
        let d = tempfile::tempdir().expect("temp");
        let path = d.path().join("sessions.db");
        {
            let db = Connection::open(&path).expect("open");
            // Schema 1, as 0.2 made it.
            let v1 = CREATE
                .replace(",\n  chat TEXT,\n  chat_stamp TEXT,\n  chat_branch INTEGER NOT NULL DEFAULT 0,\n  chat_off INTEGER NOT NULL DEFAULT 0", "")
                .replace("  arena TEXT,\n", "")
                .replace("  sent INTEGER NOT NULL DEFAULT 0,\n", "");
            assert!(!v1.contains("chat_off") && !v1.contains("arena TEXT") && !v1.contains("sent INTEGER"));
            db.execute_batch(&v1).expect("v1");
            db.pragma_update(None, "user_version", 1).expect("version");
            db.execute(
                "INSERT INTO sessions (id, folder, created, updated, messages, turns) VALUES ('s1', '/w', 0, 0, 0, 0)",
                [],
            )
            .expect("session");
        }
        let s = Store::open(&path).expect("upgraded");
        s.append("s1", &Message::user("still here"), true).expect("append");
        assert_eq!(s.summary("s1").expect("kept").messages, 1);
        assert_eq!(s.unsent("s1", 5).expect("unsent").len(), 1);
    }

    #[test]
    fn lists_by_folder_newest_first_and_persists() {
        let d = tempfile::tempdir().expect("temp");
        let path = d.path().join("sessions.db");
        let (a, b) = {
            let s = Store::open(&path).expect("open");
            let a = s.create("/w", "m", "ask").expect("a");
            s.append(&a, &Message::user("first"), true).expect("u");
            let b = s.create("/w", "m", "ask").expect("b");
            s.append(&b, &Message::user("second"), true).expect("u");
            let _empty = s.create("/w", "m", "ask").expect("empty");
            (a, b)
        };
        let s = Store::open(&path).expect("reopen");
        let list = s.list("/w", 10).expect("list");
        assert_eq!(list.iter().map(|x| x.id.clone()).collect::<Vec<_>>(), vec![b.clone(), a]);
        assert_eq!(s.latest("/w").expect("latest").expect("one").id, b);
    }
}
