//! Arena's memory, with the person's key (`/api/code-arena/memories`): the same memories as the web chat's: the person's
//! for every project, a project's by its repository's key, the company's. And the person's past chats (`/history`), and
//! the skills the agent works out (`/skills/learn`).

use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{ArenaClient, ArenaError, refusal};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Memory {
    pub id: String,
    pub text: String,
    /// "person", "project", "company".
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    /// The person's own (not the company's): theirs to forget.
    #[serde(default)]
    pub mine: bool,
}

/// What a session starts with: the prompt's block for the project, and every memory it may read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    /// Memory is on for this person.
    #[serde(default)]
    pub on: bool,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub in_note: Vec<String>,
    #[serde(default)]
    pub memories: Vec<Memory>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Recalled {
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub memories: Vec<RecalledMemory>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RecalledMemory {
    pub id: String,
    pub text: String,
}

/// A memory kept on this machine, brought to Arena.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Import {
    pub text: String,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// An answer in words, or why not (`/history`, `/skills/learn`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Said {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub error: bool,
}

const API: &str = "/api/code-arena";

/// Whether an error is Arena's "memory is not among what you may use" (feature.memory).
pub fn denied(e: &ArenaError) -> bool {
    matches!(e, ArenaError::Refused(_))
}

impl ArenaClient {
    pub async fn memories(&self, project: Option<&str>) -> Result<Listed, ArenaError> {
        let path = match project {
            Some(p) => format!(
                "{API}/memories?project={}",
                url::form_urlencoded::byte_serialize(p.as_bytes()).collect::<String>()
            ),
            None => format!("{API}/memories"),
        };
        self.get(&path).await
    }

    /// Keeps a memory: `scope` "person" or "project", `kind` preference, fact, decision, howto or lesson.
    pub async fn keep_memory(
        &self,
        text: &str,
        scope: &str,
        kind: &str,
        project: Option<&str>,
    ) -> Result<Memory, ArenaError> {
        let body = json!({"text": text, "scope": scope, "kind": kind, "project": project});
        self.call(Method::POST, &format!("{API}/memories"), Some(&body)).await
    }

    pub async fn forget_memory(&self, id: &str) -> Result<bool, ArenaError> {
        let raw = self.raw(Method::DELETE, &format!("{API}/memories/{id}"), None).await?;
        match raw.status {
            s if s.is_success() => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            _ => Err(refusal(&raw)),
        }
    }

    /// The memories that matter to a question, not among `besides`, within `budget` characters.
    pub async fn recall(
        &self,
        query: &str,
        project: Option<&str>,
        besides: &[String],
        budget: usize,
    ) -> Result<Recalled, ArenaError> {
        let body = json!({"query": query, "project": project, "besides": besides, "budget": budget});
        self.call(Method::POST, &format!("{API}/memories/recall"), Some(&body)).await
    }

    /// Memories kept on this machine, brought in once: how many were new.
    pub async fn import_memories(&self, items: &[Import]) -> Result<(u64, u64), ArenaError> {
        let v: Value =
            self.call(Method::POST, &format!("{API}/memories/import"), Some(&json!({"memories": items}))).await?;
        Ok((v["kept"].as_u64().unwrap_or(0), v["skipped"].as_u64().unwrap_or(0)))
    }

    /// The person's past chats and sessions by what they were about.
    pub async fn search_history(&self, query: &str) -> Result<Said, ArenaError> {
        self.call(Method::POST, &format!("{API}/history/search"), Some(&json!({"query": query}))).await
    }

    /// One past chat: its digest, the messages holding `query`, or those `from`…`to`.
    pub async fn open_history(
        &self,
        chat: &str,
        query: Option<&str>,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Said, ArenaError> {
        let body = json!({"chat": chat, "query": query, "from": from, "to": to});
        self.call(Method::POST, &format!("{API}/history/open"), Some(&body)).await
    }

    /// A skill the agent worked out, kept as the person's (or a better version of theirs).
    pub async fn learn_skill(
        &self,
        name: &str,
        description: &str,
        instructions: &str,
        why: &str,
    ) -> Result<Said, ArenaError> {
        let body = json!({"name": name, "description": description, "instructions": instructions, "why": why});
        self.call(Method::POST, &format!("{API}/skills/learn"), Some(&body)).await
    }
}
