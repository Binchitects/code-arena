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
    /// Its tool groups: the ids of Arena's chat tools it has on (web, python…); none: those on in a new chat.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub mine: bool,
}

const API: &str = "/api/code-arena";

/// A project as its page shows it: its repositories, goals and tasks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub due_on: Option<String>,
    #[serde(default)]
    pub repositories: Vec<Value>,
    #[serde(default)]
    pub goals: Vec<ViewGoal>,
    #[serde(default)]
    pub tasks: Vec<ViewTask>,
    #[serde(default)]
    pub members: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewGoal {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub due_on: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewTask {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub done: bool,
}

/// A repository's key (`host/group/name`) as Arena takes it: its path and host.
pub fn repository_of(key: &str) -> Option<(String, String)> {
    let (host, path) = key.split_once('/')?;
    (host.contains('.') && path.contains('/')).then(|| (path.to_string(), host.to_string()))
}

impl ArenaClient {
    /// Makes a project of the person's (as the page does: theirs, active), with this repository in it when given; its id.
    pub async fn make_project(
        &self,
        name: &str,
        description: Option<&str>,
        repository: Option<(&str, &str)>,
    ) -> Result<String, ArenaError> {
        let v: Value =
            self.call(Method::POST, "/api/projects", Some(&json!({"name": name, "description": description}))).await?;
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| ArenaError::Parse { path: "/api/projects".into(), message: "no id in the answer".into() })?;
        if let Some((path, host)) = repository {
            let _: Value = self
                .call(
                    Method::POST,
                    &format!("/api/projects/{id}/repositories"),
                    Some(&json!({"path": path, "host": host})),
                )
                .await?;
        }
        Ok(id)
    }

    /// A project as its page shows it, as the person may see it.
    pub async fn project_view(&self, id: &str) -> Result<ProjectView, ArenaError> {
        self.get(&format!("/api/projects/{id}")).await
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use ca_core::Secret;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn a_project_is_made_with_the_repository_and_shown_as_its_page_has_it() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/projects"))
            .and(body_json(json!({"name": "Payments", "description": "The new API"})))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "p1"})))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/projects/p1/repositories"))
            .and(body_json(json!({"path": "payments/api", "host": "gitlab.corp"})))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"path": "payments/api", "host": "gitlab.corp"})),
            )
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/projects/p1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "p1", "name": "Payments", "status": "active", "role": "owner", "dueOn": null,
                "repositories": [{"path": "payments/api", "host": "gitlab.corp"}],
                "members": [{"name": "Ali"}],
                "goals": [{"id": "g1", "title": "Refunds work", "status": "open"}],
                "tasks": [{"id": "t1", "title": "Refund endpoint", "goalId": "g1", "done": true, "assignee": "Ali"}]
            })))
            .mount(&s)
            .await;
        let c = ArenaClient::new(&s.uri(), Some(Secret::new("sk-x")), None).expect("client");
        let repo = repository_of("gitlab.corp/payments/api").expect("repo");
        let id = c.make_project("Payments", Some("The new API"), Some((&repo.0, &repo.1))).await.expect("made");
        let v = c.project_view(&id).await.expect("view");
        assert_eq!(
            (v.goals[0].title.as_str(), v.tasks[0].done, v.tasks[0].goal_id.as_deref()),
            ("Refunds work", true, Some("g1"))
        );
    }

    #[tokio::test]
    async fn a_person_who_may_not_make_projects_hears_arenas_words() {
        let s = MockServer::start().await;
        Mock::given(path("/api/projects"))
            .respond_with(ResponseTemplate::new(403).set_body_json(
                json!({"status": "permission", "error": "Making projects is not among what you may do."}),
            ))
            .mount(&s)
            .await;
        let c = ArenaClient::new(&s.uri(), Some(Secret::new("sk-x")), None).expect("client");
        let e = c.make_project("X", None, None).await.expect_err("refused");
        assert!(e.to_string().contains("Making projects is not among what you may do"), "{e}");
    }
}
