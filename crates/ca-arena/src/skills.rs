//! Skills in Arena: those made there that the person may use (theirs, their groups', the company's) and their
//! plugins', which Code Arena keeps and reads after the project's and the person's own; the person's own made or
//! changed from Code Arena; folders imported as the Skills page imports them (its checks and safeguards).

use std::collections::BTreeMap;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{ArenaClient, ArenaError};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ArenaSkill {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The plugin it came with, if one.
    #[serde(default)]
    pub plugin: Option<String>,
    /// Its folder: each file by its path inside it, SKILL.md among them.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// The person's own: theirs to change from here.
    #[serde(default)]
    pub mine: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Skills {
    #[serde(default)]
    pub skills: Vec<ArenaSkill>,
    /// Built-in skills an admin turned off: Code Arena leaves them out too.
    #[serde(default)]
    pub off: Vec<String>,
}

/// A skill's folder to import: its name as a folder, and its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Folder {
    pub folder: String,
    pub files: BTreeMap<String, String>,
}

const API: &str = "/api/code-arena";

impl ArenaClient {
    pub async fn skills(&self) -> Result<Skills, ArenaError> {
        self.get(&format!("{API}/skills")).await
    }

    /// One of the person's own skills made in Arena (used at once, in the web chat and every Code Arena).
    pub async fn make_skill(&self, name: &str, description: &str, instructions: &str) -> Result<Value, ArenaError> {
        let body = json!({"name": name, "description": description, "instructions": instructions});
        self.call(Method::POST, &format!("{API}/skills"), Some(&body)).await
    }

    /// Folders imported as the person's own skills, as the Skills page imports them. `preview`: only what it would do.
    /// `choices`: for a name they have already, `{"folder", "action": "replace" | "rename" | "skip", "name"}`.
    pub async fn import_skills(
        &self,
        folders: &[Folder],
        choices: &[Value],
        preview: bool,
    ) -> Result<Value, ArenaError> {
        let body = json!({"skills": folders, "choices": choices, "preview": preview});
        self.call(Method::POST, &format!("{API}/skills/import"), Some(&body)).await
    }
}
