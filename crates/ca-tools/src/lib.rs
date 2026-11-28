//! The agent's own tools: files, search, commands and git, inside the folders it may use.

pub mod checkpoints;
pub mod code_index;
pub mod commits;
mod files;
pub mod formatters;
mod git;
pub mod jobs;
pub mod plan;
pub mod sandbox;
mod search;
mod shell;
pub mod text;
pub mod vm;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ca_llm::ToolDef;
use ca_workspace::Workspace;
use serde_json::Value;

pub use files::{EditFile, ListDir, ReadFile, WriteFile};
pub use git::Git;
pub use search::{Glob, Grep};
pub use shell::{CommandOutput, RunShell, StopCommand, shell_for};

/// How a tool's calls ask the person, as the mode says for its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Reads only: never asks.
    Read,
    /// Changes files: asks in `ask` mode.
    Edit,
    /// Runs commands: asks in `ask` and `auto-edit`.
    Shell,
}

/// What a call gave the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub text: String,
    pub is_error: bool,
    /// The files it wrote, for the turn's list and its checkpoint.
    pub wrote: Vec<PathBuf>,
}

impl Output {
    pub fn ok(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: false, wrote: vec![] }
    }
    pub fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true, wrote: vec![] }
    }
}

/// What every call runs with.
#[derive(Debug, Clone)]
pub struct Context {
    pub workspace: Arc<Workspace>,
    /// `tools.shell`, when set.
    pub shell: Option<String>,
    pub shell_timeout: Duration,
    /// What each file held before the turn first wrote it, for `/rewind`.
    pub checkpoints: Arc<checkpoints::Checkpoints>,
    /// Commands with no time limit and in the background; `None` where a run cannot keep them (`-p`).
    pub jobs: Option<Arc<jobs::Jobs>>,
    /// Where the agent's commands run: `None` runs them as the person.
    pub sandbox: Option<Arc<sandbox::Sandbox>>,
    /// The session's working plan.
    pub plan: Arc<plan::PlanBook>,
    /// The mode is plan: a plan made now is proposed, for the person's review.
    pub planning: bool,
    /// The id of the call this run answers, when the turn says it (a sub-agent's progress is told by it).
    pub call: Option<String>,
}

impl Context {
    /// A program as the agent's commands run it: in the sandbox when there is one.
    pub fn command(&self, program: &str, args: &[String]) -> (String, Vec<String>) {
        match &self.sandbox {
            Some(s) => s.wrap(program, args, self.workspace.cwd()),
            None => (program.to_string(), args.to_vec()),
        }
    }

    pub fn new(workspace: Arc<Workspace>, shell: Option<String>, shell_timeout: Duration) -> Self {
        Context {
            workspace,
            shell,
            shell_timeout,
            checkpoints: Arc::new(checkpoints::Checkpoints::default()),
            jobs: None,
            sandbox: None,
            plan: Arc::new(plan::PlanBook::default()),
            planning: false,
            call: None,
        }
    }
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> ToolDef;
    fn kind(&self) -> Kind;
    /// What a kept rule's pattern is matched against, and the question shows: a command, a path.
    fn subject(&self, args: &Value) -> String;
    async fn run(&self, args: Value, ctx: &Context) -> Output;
    /// Its group, as `/tools` lists them and turns them off: Code Arena's own, or a server's.
    fn group(&self) -> String {
        "Code Arena's own".into()
    }
    /// Which of Arena's chat tools it is a function of (an assistant's tool groups name these); none for the rest.
    fn arena_tool(&self) -> Option<String> {
        None
    }
}

/// Every built-in tool.
pub fn builtin() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadFile),
        Arc::new(WriteFile),
        Arc::new(EditFile),
        Arc::new(ListDir),
        Arc::new(Glob),
        Arc::new(Grep),
        Arc::new(RunShell),
        Arc::new(CommandOutput),
        Arc::new(StopCommand),
        Arc::new(Git),
        Arc::new(plan::PlanTool),
    ]
}

/// A string argument, or why not.
pub(crate) fn arg_str<'a>(args: &'a Value, name: &str) -> Result<&'a str, Output> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Output::error(format!("{name} is missing: give it as a string")))
}

pub(crate) fn arg_u64(args: &Value, name: &str) -> Option<u64> {
    args.get(name).and_then(Value::as_u64)
}

pub(crate) fn arg_bool(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub fn context() -> (tempfile::TempDir, Context) {
        let dir = tempfile::tempdir().expect("temp");
        let ws = Workspace::open(dir.path(), &[]).expect("workspace");
        (dir, Context::new(Arc::new(ws), None, Duration::from_secs(30)))
    }
}
