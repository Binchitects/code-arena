//! What the agent remembers. Signed in to Arena: Arena's memory (the same as the web chat's): the person's for every
//! project, the project's by its repository's key, the company's; the session starts with its block (pinned, the
//! company's, the newest), and each message brings what matters to it. Without Arena (out of reach, not signed in):
//! the files on this machine, which go to Arena when it next answers. And the project's own long memory in the
//! repository (`.remember/`). An Arena that denies the person `feature.memory` gets nothing remembered or recalled.

pub mod keys;
pub mod local;
pub mod project;
mod tools;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use ca_arena::ArenaClient;
use ca_arena::memory::{self as am, Import};

pub use tools::tools;

/// How long one call waits for Arena: memory never holds a session's start or a message for long.
const LIMIT: Duration = Duration::from_secs(8);

/// What the person, `/memory` and a tool are told when memory is denied.
pub const DENIED: &str = "Memory is off for you: Arena's admins have not given you feature.memory (Admin → Access). Nothing is remembered or recalled.";

/// How the memories found for a message start (as Arena says them).
pub const RECALL_HEADING: &str = "(Memories that may matter to this question; use them where they help:";

const MORE: &str = "More is remembered than is shown here: the memories that matter to each message of the person come with it, in Code Arena's note, and recall searches all of memory. Keep what a later session should know with remember.";

#[derive(Debug, Default)]
struct State {
    /// Arena answered at the session's start: memory is Arena's for this session.
    online: bool,
    denied: bool,
    /// The memories Arena gave at the start (pinned first, newest first): `/memory` numbers them.
    listed: Vec<am::Memory>,
    /// What came with earlier messages: not given again.
    given: Vec<String>,
}

/// One memory as the IDE lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub text: String,
    /// "person", "project" or "company".
    pub scope: String,
    /// What it is ("fact", "preference", ...), empty when it is not said.
    pub kind: String,
    pub pinned: bool,
    /// The person's own: theirs to forget.
    pub own: bool,
}

pub struct Memory {
    arena: Option<ArenaClient>,
    pub project_key: String,
    pub files: local::Files,
    /// The repository's root, where `.remember/` is.
    pub root: PathBuf,
    state: Mutex<State>,
}

impl Memory {
    pub fn new(arena: Option<ArenaClient>, folder: &Path, config: &Path, data: &Path, home: Option<&Path>) -> Self {
        let root = project::root(folder);
        Memory {
            arena,
            project_key: keys::project_key(&root),
            files: local::Files::new(config, data, home, &root),
            root,
            state: Mutex::new(State::default()),
        }
    }

    /// Arena said the person may not use memory (`feature.memory` in their denied list).
    pub fn deny(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.denied = true;
        }
    }

    pub fn denied(&self) -> bool {
        self.state.lock().map(|s| s.denied).unwrap_or(false)
    }

    pub fn online(&self) -> bool {
        self.state.lock().map(|s| s.online).unwrap_or(false)
    }

    pub fn arena(&self) -> Option<&ArenaClient> {
        self.arena.as_ref()
    }

    /// The prompt's memory for this session. With Arena: the files on this machine first go to it (once), then its block
    /// for this project, kept here too for a session without Arena. Without: the files, and the block kept last time.
    /// `say` tells the person what happened (the files went to Arena).
    pub async fn start(&self, say: &dyn Fn(String)) -> String {
        if self.denied() {
            return String::new();
        }
        if let Some(arena) = &self.arena {
            self.import(arena, say).await;
            match tokio::time::timeout(LIMIT, arena.memories(Some(&self.project_key))).await {
                Ok(Ok(list)) => {
                    let note = list.note.clone().filter(|n| !n.trim().is_empty());
                    if let Some(d) = self.files.cache.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let _ = std::fs::write(&self.files.cache, serde_json::json!({"note": note}).to_string());
                    if let Ok(mut s) = self.state.lock() {
                        s.online = true;
                        s.given = list.in_note.clone();
                        s.listed = list.memories;
                    }
                    return note.map(|n| format!("\n# Memory\n\n{n}\n\n{MORE}\n")).unwrap_or_default();
                }
                Ok(Err(e)) if am::denied(&e) => {
                    self.deny();
                    return String::new();
                }
                _ => {}
            }
        }
        self.offline()
    }

    fn offline(&self) -> String {
        let kept = std::fs::read_to_string(&self.files.cache)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v["note"].as_str().map(str::to_string))
            .filter(|n| !n.is_empty());
        let mut p = self.files.for_prompt();
        if let Some(k) = kept {
            p.push_str(&format!("\n# Memory (as Arena last gave it)\n\n{k}\n"));
        }
        p
    }

    /// The memory files' lines, sent to Arena; the files kept beside as `.imported` once it took them.
    async fn import(&self, arena: &ArenaClient, say: &dyn Fn(String)) {
        let mut items: Vec<Import> = self
            .files
            .read(true)
            .into_iter()
            .map(|text| Import { text, scope: "person".into(), project: None })
            .collect();
        items.extend(self.files.read(false).into_iter().map(|text| Import {
            text,
            scope: "project".into(),
            project: Some(self.project_key.clone()),
        }));
        if items.is_empty() {
            return;
        }
        if let Ok(Ok((kept, _))) = tokio::time::timeout(LIMIT, arena.import_memories(&items)).await {
            self.files.imported();
            say(format!(
                "{} memories kept on this machine went to Arena ({kept} new): it keeps Code Arena's memory now, the web chat's too.",
                items.len()
            ));
        }
    }

    /// Keeps a memory: in Arena when it answered at the start, else in the files (they go to Arena next time). Gives
    /// whether it was new, and where it went.
    pub async fn remember(&self, person: bool, text: &str, kind: &str) -> Result<(bool, String), String> {
        if self.denied() {
            return Err(DENIED.into());
        }
        let text = text.trim();
        if text.is_empty() {
            return Err("Say what to remember.".into());
        }
        let place = if person { "for every project" } else { "for this project" };
        if self.online()
            && let Some(arena) = &self.arena
        {
            let project = (!person).then_some(self.project_key.as_str());
            let scope = if person { "person" } else { "project" };
            return match tokio::time::timeout(LIMIT, arena.keep_memory(text, scope, kind, project)).await {
                Ok(Ok(kept)) => {
                    let new = self.state.lock().map(|s| s.listed.iter().all(|m| m.id != kept.id)).unwrap_or(true);
                    if let Ok(mut s) = self.state.lock()
                        && new
                    {
                        s.listed.insert(0, kept);
                    }
                    Ok((new, format!("{place}, in Arena")))
                }
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("Arena did not answer in time: try again.".into()),
            };
        }
        let added = self.files.add(person, text).map_err(|e| e.to_string())?;
        Ok((added, format!("{place} (on this machine: it goes to Arena when Code Arena next reaches it)")))
    }

    /// The memories that matter to a question, not given already: Arena's recall when it answered at the start, else
    /// the files' lines sharing its words. `None` when nothing matters.
    pub async fn recall(&self, query: &str, budget: usize) -> Option<String> {
        if self.denied() || query.trim().is_empty() {
            return None;
        }
        let besides: Vec<String> = self.state.lock().map(|s| s.given.clone()).unwrap_or_default();
        if self.online()
            && let Some(arena) = &self.arena
        {
            let r = tokio::time::timeout(LIMIT, arena.recall(query, Some(&self.project_key), &besides, budget)).await;
            let found = r.ok()?.ok()?;
            if let Ok(mut s) = self.state.lock() {
                s.given.extend(found.memories.iter().map(|m| m.id.clone()));
            }
            return found.note.filter(|n| !n.trim().is_empty());
        }
        let found = self.files.find(query, &besides, 10);
        if found.is_empty() {
            return None;
        }
        if let Ok(mut s) = self.state.lock() {
            s.given.extend(found.iter().cloned());
        }
        Some(format!("{RECALL_HEADING}{})", found.iter().map(|f| format!("\n- {f}")).collect::<String>()))
    }

    /// What `/memory` lists: Arena's as the session started (with ids), or the files'.
    pub fn listed(&self) -> Vec<(Option<String>, String, String)> {
        if self.online() {
            return self
                .state
                .lock()
                .map(|s| s.listed.iter().map(|m| (Some(m.id.clone()), m.scope.clone(), m.text.clone())).collect())
                .unwrap_or_default();
        }
        let mut out: Vec<(Option<String>, String, String)> =
            self.files.read(true).into_iter().map(|t| (None, "person".to_string(), t)).collect();
        out.extend(self.files.read(false).into_iter().map(|t| (None, "project".to_string(), t)));
        out
    }

    /// What `/memory` lists, in its order (its numbers are theirs), with what else is known of each: the IDE's
    /// Memory view shows them. The files' lines have no kind, are not pinned and are the person's own.
    pub fn items(&self) -> Vec<Item> {
        if self.online() {
            return self
                .state
                .lock()
                .map(|s| {
                    s.listed
                        .iter()
                        .map(|m| Item {
                            text: m.text.clone(),
                            scope: m.scope.clone(),
                            kind: m.kind.clone(),
                            pinned: m.pinned,
                            own: m.mine && m.scope != "company",
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        self.listed()
            .into_iter()
            .map(|(_, scope, text)| Item { text, scope, kind: String::new(), pinned: false, own: true })
            .collect()
    }

    /// Forgets the memory `/memory` lists at this place (from 1): the one forgotten.
    pub async fn forget(&self, number: usize) -> Result<String, String> {
        let all = self.listed();
        let (id, scope, text) = all
            .get(number.wrapping_sub(1))
            .cloned()
            .ok_or(format!("There is no memory {number}: /memory lists them."))?;
        match (id, &self.arena) {
            (Some(id), Some(arena)) => {
                if scope == "company" {
                    return Err("That is the company's: only Arena's admins change it.".into());
                }
                arena.forget_memory(&id).await.map_err(|e| e.to_string())?;
                if let Ok(mut s) = self.state.lock() {
                    s.listed.retain(|m| m.id != id);
                }
            }
            _ => {
                let file = if scope == "person" { &self.files.person } else { &self.files.project };
                let body = std::fs::read_to_string(file).unwrap_or_default();
                let mut gone = false;
                let kept: Vec<&str> = body
                    .lines()
                    .filter(|l| {
                        if !gone && l.trim_start().strip_prefix("- ").map(str::trim) == Some(text.as_str()) {
                            gone = true;
                            return false;
                        }
                        true
                    })
                    .collect();
                std::fs::write(file, kept.join("\n") + "\n").map_err(|e| e.to_string())?;
            }
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests;
