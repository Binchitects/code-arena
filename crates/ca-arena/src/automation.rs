//! The person's agents (Agent Studio's workflows) and scheduled tasks in Arena, as its pages have them: listed, run,
//! and each run followed. Arena 8.1 on (`automation.agents`, `automation.scheduled`); an 8.0 has neither route.

use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::{ArenaClient, ArenaError, refusal};

const API: &str = "/api/code-arena";

/// An agent the person may run: theirs, or one shared with them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mine: bool,
    #[serde(default)]
    pub owner: Option<String>,
    /// How it starts: manual, form, schedule, webhook, gitlab, room.
    #[serde(default)]
    pub trigger: Option<String>,
}

/// A step of an agent's run, as it went.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// An agent's run: running, waiting (for its owner's approval), done, failed, stopped.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    pub id: String,
    #[serde(default)]
    pub workflow: Option<String>,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

impl AgentRun {
    pub fn ended(&self) -> bool {
        matches!(self.status.as_str(), "done" | "failed" | "stopped")
    }

    /// What it gave: the last step's output that says something.
    pub fn answer(&self) -> Option<&str> {
        self.steps.iter().rev().filter_map(|s| s.output.as_deref()).find(|o| !o.trim().is_empty())
    }
}

/// A run of a scheduled task: running, done, failed, skipped.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRun {
    pub id: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    pub status: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub manual: bool,
}

/// One of the person's scheduled tasks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub cron: String,
    #[serde(default)]
    pub time_zone: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub next_runs: Vec<String>,
    #[serde(default)]
    pub last_run: Option<TaskRun>,
    /// schedule, or the events that run it (gitlab).
    #[serde(default)]
    pub trigger: Option<String>,
}

/// How starting a scheduled task went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    Started,
    /// It is running now: this run must end first.
    Running(String),
}

/// The one of `names` that `word` names: by number (from 1), by name (any case), else the only one whose name holds it.
pub fn pick<'a, T>(items: &'a [T], name: impl Fn(&T) -> &str, word: &str) -> Result<&'a T, String> {
    let word = word.trim();
    if let Ok(n) = word.parse::<usize>()
        && (1..=items.len()).contains(&n)
    {
        return Ok(&items[n - 1]);
    }
    if let Some(x) = items.iter().find(|x| name(x).eq_ignore_ascii_case(word)) {
        return Ok(x);
    }
    let low = word.to_lowercase();
    let near: Vec<&T> = items.iter().filter(|x| name(x).to_lowercase().contains(&low)).collect();
    match near.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("none is called {word}")),
        many => {
            Err(format!("{word} could be {}", many.iter().map(|x| name(x).to_string()).collect::<Vec<_>>().join(", ")))
        }
    }
}

impl ArenaClient {
    pub async fn agents(&self) -> Result<Vec<Agent>, ArenaError> {
        self.get(&format!("{API}/agents")).await
    }

    /// Starts an agent with a text and (for one started by a form) its fields; the run's id.
    pub async fn run_agent(
        &self,
        id: &str,
        text: Option<&str>,
        fields: &[(String, String)],
    ) -> Result<String, ArenaError> {
        let fields: serde_json::Map<String, Value> =
            fields.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        let body = json!({"text": text, "fields": if fields.is_empty() { Value::Null } else { Value::Object(fields) }});
        let v: Value = self.call(Method::POST, &format!("{API}/agents/{id}/run"), Some(&body)).await?;
        v.get("id").and_then(Value::as_str).map(str::to_string).ok_or_else(|| ArenaError::Parse {
            path: format!("{API}/agents/{id}/run"),
            message: "no run id in the answer".into(),
        })
    }

    pub async fn agent_run(&self, run: &str) -> Result<AgentRun, ArenaError> {
        self.get(&format!("{API}/agents/runs/{run}")).await
    }

    pub async fn scheduled(&self) -> Result<Vec<Task>, ArenaError> {
        self.get(&format!("{API}/scheduled")).await
    }

    pub async fn run_scheduled(&self, id: &str) -> Result<Started, ArenaError> {
        let raw = self.raw(Method::POST, &format!("{API}/scheduled/{id}/run"), None).await?;
        if raw.status.is_success() {
            return Ok(Started::Started);
        }
        if raw.status == StatusCode::CONFLICT {
            return Ok(Started::Running(
                raw.problem().and_then(|p| p.error).unwrap_or_else(|| "It is running now.".into()),
            ));
        }
        Err(refusal(&raw))
    }

    pub async fn scheduled_runs(&self, id: &str) -> Result<Vec<TaskRun>, ArenaError> {
        self.get(&format!("{API}/scheduled/{id}/runs")).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ca_core::Secret;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn one_is_picked_by_number_name_or_a_part_only_it_has() {
        let names = vec!["Daily report".to_string(), "Weekly report".to_string(), "Triage".to_string()];
        fn n(s: &String) -> &str {
            s
        }
        assert_eq!(pick(&names, n, "2").expect("2"), "Weekly report");
        assert_eq!(pick(&names, n, "triage").expect("name"), "Triage");
        assert_eq!(pick(&names, n, "daily").expect("part"), "Daily report");
        assert!(pick(&names, n, "report").expect_err("two").contains("Daily report, Weekly report"));
        assert!(pick(&names, n, "nothing").is_err());
    }

    #[tokio::test]
    async fn an_agent_is_run_and_followed_and_a_task_started_once() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/code-arena/agents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": "a1", "name": "Reviewer", "description": "Reviews an MR.", "enabled": true, "mine": true, "trigger": "form", "groupIds": [], "reach": "me"}
            ])))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/code-arena/agents/a1/run"))
            .and(body_json(json!({"text": "look at !12", "fields": {"mr": "12"}})))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"id": "r1"})))
            .mount(&s)
            .await;
        Mock::given(path("/api/code-arena/agents/runs/r1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "r1", "workflowId": "a1", "workflow": "Reviewer", "trigger": "form", "status": "done", "error": null,
                "steps": [{"path": "0", "kind": "llm", "name": "Read", "status": "done", "output": "Looks right."}, {"kind": "text", "name": "End", "status": "done", "output": ""}]
            })))
            .mount(&s)
            .await;
        Mock::given(path("/api/code-arena/scheduled"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": "t1", "name": "Morning", "prompt": "Sum up", "cron": "0 9 * * *", "timeZone": "Asia/Tehran", "enabled": true, "running": false,
                 "nextRuns": ["2026-11-27T05:30:00Z"], "lastRun": {"id": "x", "status": "done", "manual": false}, "trigger": "schedule"}
            ])))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/code-arena/scheduled/t1/run"))
            .respond_with(
                ResponseTemplate::new(409).set_body_json(
                    json!({"status": "running", "error": "It is running now; wait for this run to end."}),
                ),
            )
            .mount(&s)
            .await;
        let c = ArenaClient::new(&s.uri(), Some(Secret::new("sk-x")), None).expect("client");
        let agents = c.agents().await.expect("agents");
        assert_eq!(agents[0].trigger.as_deref(), Some("form"));
        let run = c.run_agent("a1", Some("look at !12"), &[("mr".into(), "12".into())]).await.expect("run");
        let r = c.agent_run(&run).await.expect("detail");
        assert!(r.ended());
        assert_eq!(r.answer(), Some("Looks right."));
        let tasks = c.scheduled().await.expect("tasks");
        assert_eq!(tasks[0].last_run.as_ref().map(|r| r.status.as_str()), Some("done"));
        assert!(matches!(c.run_scheduled("t1").await.expect("409"), Started::Running(w) if w.contains("wait")));
    }
}
