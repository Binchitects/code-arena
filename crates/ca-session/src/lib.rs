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
const SCHEMA: i64 = 4;

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
  chat_off INTEGER NOT NULL DEFAULT 0,
  plan TEXT,
  plan_sent INTEGER NOT NULL DEFAULT 1,
  goals TEXT,
  goals_sent INTEGER NOT NULL DEFAULT 1,
  parent TEXT,
  parent_seq INTEGER
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
  usage TEXT,
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
ALTER TABLE sessions ADD COLUMN plan TEXT;
ALTER TABLE sessions ADD COLUMN plan_sent INTEGER NOT NULL DEFAULT 1;
ALTER TABLE messages ADD COLUMN arena TEXT;
ALTER TABLE compactions ADD COLUMN sent INTEGER NOT NULL DEFAULT 0;
"#;

/// From schema 2 (0.3) to 3: the session's goals.
const UPGRADE_2: &str = r#"
ALTER TABLE sessions ADD COLUMN goals TEXT;
ALTER TABLE sessions ADD COLUMN goals_sent INTEGER NOT NULL DEFAULT 1;
"#;

/// From schema 3 (0.4's first) to 4: the session a fork was made from.
const UPGRADE_3: &str = r#"
ALTER TABLE sessions ADD COLUMN parent TEXT;
ALTER TABLE sessions ADD COLUMN parent_seq INTEGER;
"#;

/// Columns added within a schema's version: each is added to a store that lacks it when the store opens, so a store
/// of the same version made before it keeps its sessions, and a build before it (which names the columns it reads)
/// still opens the store. Table, column, and its declaration.
const ADDED: &[(&str, &str, &str)] = &[
    // What an answer's turn used, on its last message: `{prompt, cached, completion, ms, model, cost?}`.
    ("messages", "usage", "TEXT"),
];

/// Adds to the store the columns of `ADDED` it lacks.
fn add_missing(db: &Connection) -> Result<()> {
    for (table, column, decl) in ADDED {
        let has: i64 =
            db.query_row("SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2", params![table, column], |r| {
                r.get(0)
            })?;
        if has == 0 {
            db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))?;
        }
    }
    Ok(())
}

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
        } else if (1..SCHEMA).contains(&version) {
            if version == 1 {
                db.execute_batch(UPGRADE_1)?;
            }
            if version <= 2 {
                db.execute_batch(UPGRADE_2)?;
            }
            db.execute_batch(UPGRADE_3)?;
            db.pragma_update(None, "user_version", SCHEMA)?;
        } else if version != SCHEMA {
            drop(db);
            let aside = path.with_extension(format!("v{version}.db"));
            std::fs::rename(path, &aside)
                .map_err(|e| SessionError::Sql(rusqlite::Error::InvalidPath(aside.join(e.to_string()))))?;
            return Self::open(path);
        }
        add_missing(&db)?;
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

    /// A fork: a new session in the same folder with this one's messages before `before` (all of them: `None`), its
    /// summaries, results kept aside, plan and goals. It has no chat in Arena yet (its own is made when it syncs, with
    /// all of it); the session it came from stays as it was. The new id.
    pub fn fork(&self, id: &str, before: Option<i64>) -> Result<String> {
        let s = self.summary(id)?;
        let cut = before.unwrap_or(s.messages).clamp(0, s.messages);
        let new = self.create(&s.folder, &s.model, &s.mode)?;
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO messages (session, seq, turn, role, body, at, usage) SELECT ?2, seq, turn, role, body, at, usage FROM messages WHERE session = ?1 AND seq < ?3",
            params![id, new, cut],
        )?;
        tx.execute(
            "INSERT INTO messages_text (text, session, seq) SELECT text, ?2, seq FROM messages_text WHERE session = ?1 AND seq < ?3",
            params![id, new, cut],
        )?;
        tx.execute(
            "INSERT INTO compactions (session, through, summary, at) SELECT ?2, through, summary, at FROM compactions WHERE session = ?1 AND through < ?3",
            params![id, new, cut],
        )?;
        tx.execute(
            "INSERT INTO results (session, id, text) SELECT ?2, id, text FROM results WHERE session = ?1",
            params![id, new],
        )?;
        let turns: i64 =
            tx.query_row("SELECT COALESCE(MAX(turn), 0) FROM messages WHERE session = ?1", [&new], |r| r.get(0))?;
        let whole = cut == s.messages;
        tx.execute(
            "UPDATE sessions SET messages = ?2, turns = ?3, title = ?4, parent = ?5, parent_seq = ?6,
               plan = CASE WHEN ?7 THEN (SELECT plan FROM sessions WHERE id = ?5) END, plan_sent = 0,
               goals = CASE WHEN ?7 THEN (SELECT goals FROM sessions WHERE id = ?5) END, goals_sent = 0
             WHERE id = ?1",
            params![new, cut, turns, s.title, id, cut, whole],
        )?;
        tx.commit()?;
        Ok(new)
    }

    /// What the turn just ended used (`{prompt, cached, completion, ms, model, cost?}`), kept on the session's last
    /// message when it is the model's: the answer's footer, shown again after a reload. Nothing when the last
    /// message is another's.
    pub fn set_usage(&self, id: &str, usage: &serde_json::Value) -> Result<()> {
        self.db.execute(
            "UPDATE messages SET usage = ?2 WHERE session = ?1 AND role = 'assistant'
               AND seq = (SELECT MAX(seq) FROM messages WHERE session = ?1)",
            params![id, usage.to_string()],
        )?;
        Ok(())
    }

    /// What the turns of messages `from` on used, by the place of the message each is kept on.
    pub fn usages(&self, id: &str, from: i64) -> Result<std::collections::HashMap<i64, serde_json::Value>> {
        let mut q = self
            .db
            .prepare("SELECT seq, usage FROM messages WHERE session = ?1 AND seq >= ?2 AND usage IS NOT NULL")?;
        let rows = q.query_map(params![id, from], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (seq, usage) = row?;
            out.insert(seq, serde_json::from_str(&usage)?);
        }
        Ok(out)
    }

    /// The session a fork was made from, and how many of its messages it took.
    pub fn parent(&self, id: &str) -> Result<Option<(String, i64)>> {
        Ok(self
            .db
            .query_row("SELECT parent, parent_seq FROM sessions WHERE id = ?1", [id], |r| {
                Ok(r.get::<_, Option<String>>(0)?.zip(r.get::<_, Option<i64>>(1)?))
            })
            .optional()?
            .flatten())
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

    /// The session's working plan changed (JSON; none: cleared): kept, and to be sent to Arena.
    pub fn set_plan(&self, id: &str, plan: Option<&str>) -> Result<()> {
        self.db.execute(
            "UPDATE sessions SET plan = ?2, plan_sent = 0, updated = ?3 WHERE id = ?1",
            params![id, plan, now()],
        )?;
        Ok(())
    }

    pub fn plan(&self, id: &str) -> Result<Option<String>> {
        Ok(self.db.query_row("SELECT plan FROM sessions WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten())
    }

    /// The plan, when Arena does not have its latest change: `Some(None)` for a plan cleared.
    pub fn plan_unsent(&self, id: &str) -> Result<Option<Option<String>>> {
        Ok(self
            .db
            .query_row("SELECT plan FROM sessions WHERE id = ?1 AND plan_sent = 0", [id], |r| r.get(0))
            .optional()?)
    }

    pub fn plan_sent(&self, id: &str, plan: Option<&str>) -> Result<()> {
        // Only the plan that went: one changed meanwhile goes too.
        self.db.execute("UPDATE sessions SET plan_sent = 1 WHERE id = ?1 AND plan IS ?2", params![id, plan])?;
        Ok(())
    }

    /// The session's goals changed (JSON; none: cleared): kept, and to be sent to Arena.
    pub fn set_goals(&self, id: &str, goals: Option<&str>) -> Result<()> {
        self.db.execute("UPDATE sessions SET goals = ?2, goals_sent = 0 WHERE id = ?1", params![id, goals])?;
        Ok(())
    }

    pub fn goals(&self, id: &str) -> Result<Option<String>> {
        Ok(self.db.query_row("SELECT goals FROM sessions WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten())
    }

    pub fn goals_unsent(&self, id: &str) -> Result<Option<Option<String>>> {
        Ok(self
            .db
            .query_row("SELECT goals FROM sessions WHERE id = ?1 AND goals_sent = 0", [id], |r| r.get(0))
            .optional()?)
    }

    pub fn goals_sent(&self, id: &str, goals: Option<&str>) -> Result<()> {
        self.db.execute("UPDATE sessions SET goals_sent = 1 WHERE id = ?1 AND goals IS ?2", params![id, goals])?;
        Ok(())
    }

    /// The last plan another session of the folder left (with its session and when), for `/plan resume`.
    pub fn plan_left(&self, folder: &str, except: &str) -> Result<Option<(String, i64, String)>> {
        Ok(self
            .db
            .query_row(
                "SELECT id, updated, plan FROM sessions WHERE folder = ?1 AND id <> ?2 AND plan IS NOT NULL ORDER BY updated DESC LIMIT 1",
                params![folder, except],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
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
    fn a_turns_usage_is_kept_on_its_answer_and_forks_with_it() {
        let s = Store::in_memory().expect("store");
        let id = s.create("/w", "m", "ask").expect("create");
        turn(&s, &id, 1);
        let used = serde_json::json!({"prompt": 8600, "cached": 6278, "completion": 1200, "ms": 12000, "model": "m"});
        s.set_usage(&id, &used).expect("usage");
        // A last message that is the person's takes none.
        s.append(&id, &Message::user("question 2"), true).expect("user");
        s.set_usage(&id, &serde_json::json!({"prompt": 1})).expect("usage");
        let kept = s.usages(&id, 0).expect("usages");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[&3], used);
        let fork = s.fork(&id, None).expect("fork");
        assert_eq!(s.usages(&fork, 0).expect("usages")[&3], used);
    }

    #[test]
    fn a_store_of_this_version_without_the_usage_column_gains_it_and_keeps_its_sessions() {
        let dir = std::env::temp_dir().join(format!("ca-session-usage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("sessions.db");
        {
            let db = Connection::open(&path).expect("db");
            db.execute_batch(&CREATE.replace("  usage TEXT,\n", "")).expect("create");
            db.pragma_update(None, "user_version", SCHEMA).expect("version");
        }
        let id = {
            let s = Store::open(&path).expect("open");
            let id = s.create("/w", "m", "ask").expect("create");
            turn(&s, &id, 1);
            id
        };
        let s = Store::open(&path).expect("open again");
        s.set_usage(&id, &serde_json::json!({"prompt": 5})).expect("usage");
        assert_eq!(s.usages(&id, 0).expect("usages")[&3]["prompt"], 5);
        assert_eq!(s.summary(&id).expect("summary").messages, 4);
        let _ = std::fs::remove_dir_all(&dir);
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
    fn the_plan_is_kept_sent_once_and_left_for_another_session() {
        let s = Store::in_memory().expect("store");
        let a = s.create("/w", "m", "ask").expect("a");
        let b = s.create("/w", "m", "ask").expect("b");
        assert_eq!(s.plan_unsent(&a).expect("unsent"), None);
        s.set_plan(&a, Some("{\"title\":\"T\"}")).expect("set");
        assert_eq!(s.plan(&a).expect("plan").as_deref(), Some("{\"title\":\"T\"}"));
        let unsent = s.plan_unsent(&a).expect("unsent").expect("one");
        // Changed while it went: still to send.
        s.set_plan(&a, Some("{\"title\":\"T2\"}")).expect("set");
        s.plan_sent(&a, unsent.as_deref()).expect("sent");
        assert!(s.plan_unsent(&a).expect("unsent").is_some());
        s.plan_sent(&a, Some("{\"title\":\"T2\"}")).expect("sent");
        assert!(s.plan_unsent(&a).expect("unsent").is_none());
        assert_eq!(s.plan_left("/w", &b).expect("left").map(|x| x.0), Some(a.clone()));
        assert!(s.plan_left("/w", &a).expect("left").is_none());
    }

    #[test]
    fn a_0_2_store_is_upgraded_and_keeps_its_sessions() {
        let d = tempfile::tempdir().expect("temp");
        let path = d.path().join("sessions.db");
        {
            let db = Connection::open(&path).expect("open");
            // Schema 1, as 0.2 made it.
            let v1 = CREATE
                .replace(",\n  chat TEXT,\n  chat_stamp TEXT,\n  chat_branch INTEGER NOT NULL DEFAULT 0,\n  chat_off INTEGER NOT NULL DEFAULT 0,\n  plan TEXT,\n  plan_sent INTEGER NOT NULL DEFAULT 1,\n  goals TEXT,\n  goals_sent INTEGER NOT NULL DEFAULT 1,\n  parent TEXT,\n  parent_seq INTEGER", "")
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
    fn a_fork_takes_the_session_or_its_start_and_leaves_it_as_it_was() {
        let s = Store::in_memory().expect("store");
        let id = s.create("/w", "m", "ask").expect("create");
        for n in 1..=3 {
            turn(&s, &id, n);
        }
        s.compact(&id, 7, "the first two turns").expect("compact");
        s.keep_result(&id, "a long result").expect("result");
        s.set_plan(&id, Some("{\"title\":\"p\"}")).expect("plan");
        s.link_chat(&id, "chat-1").expect("chat");
        s.mark_sent(&id, &[(0, "m0".into())]).expect("sent");
        let whole = s.fork(&id, None).expect("fork");
        assert_ne!(whole, id);
        let w = s.summary(&whole).expect("summary");
        assert_eq!((w.messages, w.turns, w.title.as_str()), (12, 3, "question 1"));
        assert_eq!(s.parent(&whole).expect("parent"), Some((id.clone(), 12)));
        assert_eq!(s.parent(&id).expect("none"), None);
        let (a, b) = (s.model_view(&id).expect("view"), s.model_view(&whole).expect("view"));
        assert_eq!((a.summary, a.messages), (b.summary, b.messages));
        assert_eq!(s.result(&whole, "r1").expect("r").as_deref(), Some("a long result"));
        assert!(s.plan(&whole).expect("plan").is_some());
        assert_eq!(s.plan_unsent(&whole).expect("unsent").map(|p| p.is_some()), Some(true));
        // Its own chat, made when it syncs: all of it goes.
        assert_eq!(s.chat_link(&whole).expect("link").chat, None);
        assert_eq!(s.unsent(&whole, 100).expect("unsent").len(), 12);
        assert_eq!(s.search("/w", "question 2", 10).expect("search").iter().filter(|h| h.0 == whole).count(), 1);
        // From before the second question: the first turn only, no plan of later.
        let start = s.fork(&id, Some(4)).expect("fork");
        let st = s.summary(&start).expect("summary");
        assert_eq!((st.messages, st.turns), (4, 1));
        assert!(s.model_view(&start).expect("view").summary.is_none(), "the summary covered more");
        assert!(s.plan(&start).expect("plan").is_none());
        s.append(&start, &Message::user("another way"), true).expect("append");
        assert_eq!(s.summary(&id).expect("kept").messages, 12, "the session it came from is as it was");
        assert_eq!(s.chat_link(&id).expect("link").chat.as_deref(), Some("chat-1"));
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
