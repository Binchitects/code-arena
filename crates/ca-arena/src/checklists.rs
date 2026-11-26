//! Checklists, kept in Arena (Workspace → Checklists): one is turned on in a session's chat, the model ticks each item
//! with how it did or checked it, and the web chat shows the same ticks. Making, editing and deleting a checklist from
//! Code Arena needs an Arena that has it ([`Feature::ChecklistsEdit`](crate::Feature::ChecklistsEdit), 8.1 on).

use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::{ArenaClient, ArenaError, refusal};

/// One of the person's checklists.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Checklist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub items: Vec<String>,
}

/// An item of the checklist on in a chat.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunItem {
    pub number: u32,
    pub text: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub note: Option<String>,
    /// "model" or "person".
    #[serde(default)]
    pub by: Option<String>,
}

/// The checklist on in a chat, with its ticks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    #[serde(default)]
    pub checklist_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub items: Vec<RunItem>,
    #[serde(default)]
    pub done: u32,
    #[serde(default)]
    pub total: u32,
}

impl Run {
    /// As the model and the terminal read it: each item numbered, ticked or not, with its note.
    pub fn text(&self) -> String {
        let mut t = format!("Checklist \"{}\" ({} of {} done):", self.name, self.done, self.total);
        for i in &self.items {
            t.push_str(&format!(
                "\n{}. [{}] {}{}",
                i.number,
                if i.done { "x" } else { " " },
                i.text,
                i.note.as_ref().map(|n| format!(" ({n})")).unwrap_or_default()
            ));
        }
        t
    }
}

/// What the model is told while a checklist is on.
pub const INSTRUCTIONS: &str = "The person turned on a checklist in this session: before you say a piece of work is done, go through every item that applies: do it or check it with your tools, then tick it with checklist_check, saying in a few words how (\"cargo test: 42 passed\"); an item that does not apply is ticked with why. Do not tick what you did not do or check. End by saying which items are left, if any.";

const API: &str = "/api/code-arena";

impl ArenaClient {
    /// The person's checklists, by name.
    pub async fn checklists(&self) -> Result<Vec<Checklist>, ArenaError> {
        self.get(&format!("{API}/checklists")).await
    }

    /// One made (`id` none) or changed: as Arena keeps it.
    pub async fn save_checklist(
        &self,
        id: Option<&str>,
        name: &str,
        items: &[String],
    ) -> Result<Checklist, ArenaError> {
        let body = json!({"name": name, "items": items});
        match id {
            None => self.call(Method::POST, &format!("{API}/checklists"), Some(&body)).await,
            Some(id) => self.call(Method::PUT, &format!("{API}/checklists/{id}"), Some(&body)).await,
        }
    }

    pub async fn delete_checklist(&self, id: &str) -> Result<(), ArenaError> {
        let raw = self.raw(Method::DELETE, &format!("{API}/checklists/{id}"), None).await?;
        if raw.status.is_success() { Ok(()) } else { Err(refusal(&raw)) }
    }

    /// The checklist on in the chat; `None` when none is.
    pub async fn checklist_run(&self, chat: &str) -> Result<Option<Run>, ArenaError> {
        self.run_call(Method::GET, &format!("{API}/chats/{chat}/checklist"), None).await
    }

    /// Turns one of the person's checklists on in the chat.
    pub async fn start_checklist(&self, chat: &str, checklist: &str) -> Result<Option<Run>, ArenaError> {
        self.run_call(Method::PUT, &format!("{API}/chats/{chat}/checklist"), Some(&json!({"checklistId": checklist})))
            .await
    }

    /// Ticks an item (1 is the first), or takes the tick back; `by` is "model" or "person".
    pub async fn tick(
        &self,
        chat: &str,
        item: u32,
        done: bool,
        note: Option<&str>,
        by: &str,
    ) -> Result<Option<Run>, ArenaError> {
        let body = json!({"item": item, "done": done, "note": note, "by": by});
        self.run_call(Method::POST, &format!("{API}/chats/{chat}/checklist/tick"), Some(&body)).await
    }

    pub async fn stop_checklist(&self, chat: &str) -> Result<(), ArenaError> {
        let raw = self.raw(Method::DELETE, &format!("{API}/chats/{chat}/checklist"), None).await?;
        if raw.status.is_success() { Ok(()) } else { Err(refusal(&raw)) }
    }

    async fn run_call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Option<Run>, ArenaError> {
        let raw = self.raw(method, path, body).await?;
        if raw.status == StatusCode::NO_CONTENT || (raw.status.is_success() && raw.body.trim().is_empty()) {
            return Ok(None);
        }
        if !raw.status.is_success() {
            return Err(refusal(&raw));
        }
        serde_json::from_str(&raw.body)
            .map(Some)
            .map_err(|e| ArenaError::Parse { path: path.into(), message: e.to_string() })
    }
}

/// A repository's own checklist (`.code-arena/checklists/NAME.md`, or `.arena/checklists/`): its name (its first
/// heading, else the file's), its items (its `- ` lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoChecklist {
    pub name: String,
    pub items: Vec<String>,
    pub file: std::path::PathBuf,
}

/// The repository's checklists, for everyone who opens it.
pub fn repo_checklists(root: &std::path::Path) -> Vec<RepoChecklist> {
    let mut found: Vec<RepoChecklist> = Vec::new();
    for dir in [root.join(".code-arena").join("checklists"), root.join(".arena").join("checklists")] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<std::path::PathBuf> =
            rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
        files.sort();
        for file in files.into_iter().take(50) {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let heading = text.lines().find_map(|l| l.strip_prefix("# ")).map(str::trim).filter(|h| !h.is_empty());
            let name = heading
                .map(str::to_string)
                .unwrap_or_else(|| file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
            let items: Vec<String> = text
                .lines()
                .map(str::trim_start)
                .filter_map(|l| l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")))
                .map(|l| {
                    let l = l.trim();
                    l.strip_prefix("[ ] ")
                        .or_else(|| l.strip_prefix("[x] "))
                        .or_else(|| l.strip_prefix("[X] "))
                        .unwrap_or(l)
                })
                .filter(|l| !l.is_empty())
                .take(50)
                .map(str::to_string)
                .collect();
            if !items.is_empty() && !found.iter().any(|f| f.name.eq_ignore_ascii_case(&name)) {
                found.push(RepoChecklist { name: name.chars().take(100).collect(), items, file });
            }
        }
    }
    found
}

/// `Before a merge: the tests pass; the docs say what changed`: its name, and its items (by `;` or new lines).
pub fn split_list(text: &str) -> Option<(String, Vec<String>)> {
    let (name, rest) = text.split_once(':')?;
    let items: Vec<String> =
        rest.split([';', '\n']).map(str::trim).filter(|i| !i.is_empty()).map(str::to_string).collect();
    (!name.trim().is_empty() && !items.is_empty()).then(|| (name.trim().to_string(), items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_reads_as_arena_shapes_it() {
        let r: Run = serde_json::from_str(
            r#"{"id":"r1","checklistId":"k1","name":"Before a merge","startedAt":"x","updatedAt":"y",
              "items":[{"number":1,"text":"the tests pass","done":true,"note":"cargo test: 42 passed","by":"model","doneAt":"z"},
                       {"number":2,"text":"the docs say what changed","done":false,"note":null,"by":null,"doneAt":null}],
              "done":1,"total":2}"#,
        )
        .expect("run");
        assert_eq!(
            r.text(),
            "Checklist \"Before a merge\" (1 of 2 done):\n1. [x] the tests pass (cargo test: 42 passed)\n2. [ ] the docs say what changed"
        );
    }

    #[test]
    fn the_repositorys_checklists_and_a_list_typed_in_one_line() {
        let d = tempfile::tempdir().expect("temp");
        let dir = d.path().join(".code-arena/checklists");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("release.md"),
            "# Before a release\n\n- [ ] the changelog is dated\n* the tag is pushed\nnot an item\n",
        )
        .expect("write");
        std::fs::write(dir.join("empty.md"), "# Nothing\n").expect("write");
        let found = repo_checklists(d.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Before a release");
        assert_eq!(found[0].items, vec!["the changelog is dated", "the tag is pushed"]);
        assert_eq!(
            split_list("Before a merge: the tests pass; the docs say what changed"),
            Some(("Before a merge".into(), vec!["the tests pass".into(), "the docs say what changed".into()]))
        );
        assert_eq!(split_list("no colon"), None);
    }
}
