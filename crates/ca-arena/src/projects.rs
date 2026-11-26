//! Projects and assistants in Arena, from Code Arena: a session's chat put in one of the person's projects (its goals
//! and tasks then the model's to keep true to the work, on an Arena that has project work, 8.1 on), and the
//! assistants a session works as.

use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::{ArenaClient, ArenaError, refusal};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub global: bool,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Projects {
    #[serde(default)]
    pub projects: Vec<Project>,
    /// The project this repository's chats are in, when Arena knows one.
    #[serde(default)]
    pub suggested: Option<Value>,
}

/// The project a chat is in, as its model is told it, and the functions it may call.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Work {
    pub project: WorkProject,
    #[serde(default)]
    pub may_change: bool,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub functions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkProject {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Assistant {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<String>,
    /// The skills it uses, by name.
    #[serde(default)]
    pub skills: Option<Value>,
    #[serde(default)]
    pub mine: bool,
}

const API: &str = "/api/code-arena";

impl ArenaClient {
    /// The person's active projects they may add to; `repo`: the repository's key, for the one it is in.
    pub async fn projects(&self, repo: Option<&str>) -> Result<Projects, ArenaError> {
        let path = match repo {
            Some(r) => format!(
                "{API}/projects?repo={}",
                url::form_urlencoded::byte_serialize(r.as_bytes()).collect::<String>()
            ),
            None => format!("{API}/projects"),
        };
        self.get(&path).await
    }

    /// The chat put in a project (`None`: in none).
    pub async fn move_chat(&self, chat: &str, project: Option<&str>) -> Result<(), ArenaError> {
        let raw =
            self.raw(Method::PUT, &format!("{API}/chats/{chat}/project"), Some(&json!({"projectId": project}))).await?;
        if raw.status.is_success() { Ok(()) } else { Err(refusal(&raw)) }
    }

    /// The project the chat is in and what its model may do there; `None` when it is in none.
    pub async fn project_work(&self, chat: &str) -> Result<Option<Work>, ArenaError> {
        let raw = self.raw(Method::GET, &format!("{API}/chats/{chat}/project-work"), None).await?;
        if raw.status == StatusCode::NO_CONTENT || (raw.status.is_success() && raw.body.trim().is_empty()) {
            return Ok(None);
        }
        if !raw.status.is_success() {
            return Err(refusal(&raw));
        }
        serde_json::from_str(&raw.body)
            .map(Some)
            .map_err(|e| ArenaError::Parse { path: "project-work".into(), message: e.to_string() })
    }

    /// One of the project functions (`project_plan`, `project_goal`, `project_task`): its words, and whether it failed.
    pub async fn project_call(&self, chat: &str, function: &str, args: &Value) -> Result<(String, bool), ArenaError> {
        let v: Value =
            self.call(Method::POST, &format!("{API}/chats/{chat}/project-work/{function}"), Some(args)).await?;
        Ok((v["text"].as_str().unwrap_or_default().to_string(), v["error"].as_bool().unwrap_or(false)))
    }

    pub async fn assistants(&self) -> Result<Vec<Assistant>, ArenaError> {
        self.get(&format!("{API}/assistants")).await
    }
}
