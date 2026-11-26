//! Hooks: commands the person (or a project they trusted) runs at the agent's moments, Claude Code's format read as it
//! is: `{"hooks": {"PreToolUse": [{"matcher": "run_shell|edit_file", "hooks": [{"type": "command", "command": "...",
//! "timeout": 60}]}]}}`. Each gets the moment as JSON on stdin; it answers by its exit code (0: go on, its stdout added
//! as context where the moment takes some; 2: blocked, its stderr the reason) or with JSON on stdout (`decision`,
//! `reason`, `continue`, `stopReason`, `hookSpecificOutput` with `permissionDecision`, `updatedInput`,
//! `additionalContext`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

/// The moments hooks run at.
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
    "SubagentStop",
    "PreCompact",
    "PreCommit",
];

/// Claude Code's names for Code Arena's tools, so a hook written for it matches here too.
fn claude_name(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "run_shell" => "Bash",
        "read_file" => "Read",
        "write_file" => "Write",
        "edit_file" => "Edit",
        "glob" => "Glob",
        "grep" => "Grep",
        "list_dir" => "LS",
        "plan" => "TodoWrite",
        "task" => "Task",
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub struct Hook {
    pub event: String,
    matcher: Option<regex::Regex>,
    pub command: String,
    pub timeout: Duration,
    /// Where it was declared: said when it blocks.
    pub from: String,
}

impl Hook {
    fn matches(&self, subject: &str) -> bool {
        match &self.matcher {
            None => true,
            Some(re) => re.is_match(subject) || claude_name(subject).is_some_and(|c| re.is_match(c)),
        }
    }
}

/// What the hooks said of a moment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Said {
    /// Blocked, and why: the call is not run, the prompt not sent, the stop not taken.
    pub block: Option<String>,
    /// `PreToolUse`: "allow" (no question), "deny", or "ask" (the person is asked whatever the mode).
    pub permission: Option<String>,
    /// `PreToolUse`: the call's arguments as the hook rewrote them.
    pub updated_input: Option<Value>,
    /// What the model is told (`SessionStart`, `UserPromptSubmit`, `PostToolUse`).
    pub context: Vec<String>,
    /// The session stops here (`continue: false`), with what to say.
    pub stop: Option<String>,
    /// For the person: a hook that failed (an exit code other than 0 and 2, or its time limit).
    pub notes: Vec<String>,
}

/// A folder's hooks: the person's always; a project's once trusted.
#[derive(Debug, Clone, Default)]
pub struct Hooks {
    pub list: Vec<Hook>,
    pub cwd: PathBuf,
    pub session: String,
}

/// The hooks a settings object declares (`{"hooks": {...}}` or the hooks map itself), with where they came from; what
/// does not read as a hook is said.
pub fn parse(v: &Value, from: &str) -> (Vec<Hook>, Vec<String>) {
    let map = v.get("hooks").unwrap_or(v);
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let Some(obj) = map.as_object() else { return (out, problems) };
    for (event, groups) in obj {
        if !EVENTS.contains(&event.as_str()) {
            problems.push(format!("{from}: no moment is called {event} (they are {})", EVENTS.join(", ")));
            continue;
        }
        for g in groups.as_array().into_iter().flatten() {
            let matcher = match g.get("matcher").and_then(Value::as_str).filter(|m| !m.is_empty() && *m != "*") {
                Some(m) => match regex::Regex::new(&format!("^(?:{m})$")) {
                    Ok(re) => Some(re),
                    Err(e) => {
                        problems.push(format!("{from}: the matcher {m} does not read: {e}"));
                        continue;
                    }
                },
                None => None,
            };
            for h in g.get("hooks").and_then(Value::as_array).into_iter().flatten() {
                let Some(command) = h.get("command").and_then(Value::as_str).filter(|c| !c.trim().is_empty()) else {
                    problems.push(format!("{from}: a {event} hook has no command"));
                    continue;
                };
                if h.get("type").and_then(Value::as_str).is_some_and(|t| t != "command") {
                    problems.push(format!("{from}: only command hooks run here ({event})"));
                    continue;
                }
                out.push(Hook {
                    event: event.clone(),
                    matcher: matcher.clone(),
                    command: command.to_string(),
                    timeout: Duration::from_secs(h.get("timeout").and_then(Value::as_u64).unwrap_or(60).clamp(1, 600)),
                    from: from.to_string(),
                });
            }
        }
    }
    (out, problems)
}

/// A project's hooks as last trusted (kept in the data folder): run only while what the project declares is that.
pub fn trusted(data: &Path, project: &Path, declared: &Value) -> bool {
    std::fs::read_to_string(trust_file(data, project))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .as_ref()
        == Some(declared)
}

/// The project's hooks trusted, as they are now.
pub fn trust(data: &Path, project: &Path, declared: &Value) -> std::io::Result<()> {
    let f = trust_file(data, project);
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(f, serde_json::to_string_pretty(declared).unwrap_or_default())
}

fn trust_file(data: &Path, project: &Path) -> PathBuf {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in project.to_string_lossy().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    data.join("trusted-hooks").join(format!("{h:016x}.json"))
}

impl Hooks {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn has(&self, event: &str) -> bool {
        self.list.iter().any(|h| h.event == event)
    }

    /// Runs the moment's hooks (those whose matcher takes `subject`: a tool's name), in order, with `payload` (the
    /// common fields added). A block stops the rest.
    pub async fn run(&self, event: &str, subject: &str, payload: Value) -> Said {
        let mut said = Said::default();
        let hooks: Vec<&Hook> = self.list.iter().filter(|h| h.event == event && h.matches(subject)).collect();
        if hooks.is_empty() {
            return said;
        }
        let mut input = payload;
        if let Some(o) = input.as_object_mut() {
            o.insert("hook_event_name".into(), json!(event));
            o.insert("session_id".into(), json!(self.session));
            o.insert("cwd".into(), json!(self.cwd.display().to_string()));
            if let Some(t) = o.get("tool_name").and_then(Value::as_str).and_then(claude_name) {
                o.insert("claude_tool_name".into(), json!(t));
            }
        }
        for h in hooks {
            let (code, out, err) = run_one(h, &self.cwd, &input).await;
            let answer: Option<Value> = serde_json::from_str(out.trim()).ok().filter(Value::is_object);
            match (&answer, code) {
                (Some(a), _) => {
                    if a.get("continue") == Some(&Value::Bool(false)) {
                        said.stop = Some(
                            a.get("stopReason")
                                .and_then(Value::as_str)
                                .unwrap_or("A hook stopped the session.")
                                .to_string(),
                        );
                    }
                    let reason = a.get("reason").and_then(Value::as_str).map(str::to_string);
                    if a.get("decision").and_then(Value::as_str) == Some("block") {
                        said.block =
                            Some(reason.clone().unwrap_or_else(|| format!("{}'s {event} hook blocked it", h.from)));
                    }
                    let specific = a.get("hookSpecificOutput").unwrap_or(&Value::Null);
                    if let Some(p) = specific.get("permissionDecision").and_then(Value::as_str) {
                        said.permission = Some(p.to_string());
                        if p == "deny" {
                            said.block = Some(
                                specific
                                    .get("permissionDecisionReason")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                                    .or(reason)
                                    .unwrap_or_else(|| format!("{}'s hook denied it", h.from)),
                            );
                        }
                    }
                    if let Some(u) = specific.get("updatedInput").filter(|u| u.is_object()) {
                        said.updated_input = Some(u.clone());
                        if let Some(o) = input.as_object_mut() {
                            o.insert("tool_input".into(), u.clone());
                        }
                    }
                    if let Some(c) =
                        specific.get("additionalContext").and_then(Value::as_str).filter(|c| !c.trim().is_empty())
                    {
                        said.context.push(c.to_string());
                    }
                }
                (None, Some(0)) => {
                    if matches!(event, "SessionStart" | "UserPromptSubmit") && !out.trim().is_empty() {
                        said.context.push(out.trim().to_string());
                    }
                }
                (None, Some(2)) => {
                    let why = err.trim();
                    said.block = Some(if why.is_empty() {
                        format!("{}'s {event} hook blocked it", h.from)
                    } else {
                        why.to_string()
                    });
                }
                // Another exit, or none (it was stopped at its time limit): said, not blocking.
                (None, c) => {
                    let why = err.trim().lines().last().unwrap_or("").to_string();
                    said.notes.push(format!(
                        "(A {event} hook of {} {}{})",
                        h.from,
                        match c {
                            Some(c) => format!("ended with exit code {c}"),
                            None => format!("did not answer within {} s", h.timeout.as_secs()),
                        },
                        if why.is_empty() { String::new() } else { format!(": {why}") }
                    ));
                }
            }
            if said.block.is_some() || said.stop.is_some() {
                break;
            }
        }
        said
    }
}

async fn run_one(h: &Hook, cwd: &Path, input: &Value) -> (Option<i32>, String, String) {
    use tokio::io::AsyncWriteExt;
    let (shell, flag) = if cfg!(windows) { ("cmd.exe", "/c") } else { ("sh", "-c") };
    let mut cmd = tokio::process::Command::new(shell);
    cmd.arg(flag)
        .arg(&h.command)
        .current_dir(cwd)
        .env("CLAUDE_PROJECT_DIR", cwd)
        .env("CODE_ARENA_PROJECT_DIR", cwd)
        .env_remove("ARENA_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let Ok(mut child) = cmd.spawn() else {
        return (Some(127), String::new(), format!("{} could not start", h.command));
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.to_string().as_bytes()).await;
    }
    match tokio::time::timeout(h.timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => (
            o.status.code(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Ok(Err(e)) => (Some(127), String::new(), e.to_string()),
        Err(_) => (None, String::new(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks(v: Value) -> Hooks {
        let (list, problems) = parse(&v, "test");
        assert!(problems.is_empty(), "{problems:?}");
        Hooks { list, cwd: std::env::temp_dir(), session: "s1".into() }
    }

    #[tokio::test]
    async fn exit_codes_and_json_answers() {
        let h = hooks(json!({"hooks": {
            "PreToolUse": [
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "grep -q 'rm -rf' && { echo 'no rm -rf here' >&2; exit 2; } || exit 0"}]},
                {"matcher": "edit_file|write_file", "hooks": [{"type": "command", "command": "echo '{\"hookSpecificOutput\":{\"permissionDecision\":\"ask\",\"updatedInput\":{\"path\":\"b.txt\"}}}'"}]}
            ],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "echo 'The repo uses tabs.'"}]}]
        }}));
        let s = h
            .run("PreToolUse", "run_shell", json!({"tool_name": "run_shell", "tool_input": {"command": "rm -rf /"}}))
            .await;
        assert_eq!(s.block.as_deref(), Some("no rm -rf here"));
        let s =
            h.run("PreToolUse", "run_shell", json!({"tool_name": "run_shell", "tool_input": {"command": "ls"}})).await;
        assert_eq!(s, Said::default());
        let s =
            h.run("PreToolUse", "edit_file", json!({"tool_name": "edit_file", "tool_input": {"path": "a.txt"}})).await;
        assert_eq!((s.permission.as_deref(), s.updated_input), (Some("ask"), Some(json!({"path": "b.txt"}))));
        let s = h.run("UserPromptSubmit", "", json!({"prompt": "hi"})).await;
        assert_eq!(s.context, vec!["The repo uses tabs."]);
        // A moment with no hook of its own: nothing.
        assert_eq!(h.run("Stop", "", json!({})).await, Said::default());
    }

    #[tokio::test]
    async fn the_hook_reads_the_moment_on_stdin() {
        let h = hooks(json!({"PostToolUse": [{"matcher": ".*", "hooks": [{"command": "cat"}]}]}));
        // `cat` answers with what it got: a JSON object, read as the hook's answer: nothing in it says to block.
        let s = h.run("PostToolUse", "read_file", json!({"tool_name": "read_file"})).await;
        assert_eq!(s.block, None);
        let h = hooks(
            json!({"Stop": [{"hooks": [{"command": "echo '{\"decision\":\"block\",\"reason\":\"run the tests first\"}'"}]}]}),
        );
        assert_eq!(h.run("Stop", "", json!({})).await.block.as_deref(), Some("run the tests first"));
    }

    #[test]
    fn what_does_not_read_is_said_and_a_project_is_trusted_as_it_is() {
        let (list, problems) =
            parse(&json!({"OnSave": [], "PreToolUse": [{"matcher": "(", "hooks": [{"command": "x"}]}]}), "p");
        assert!(list.is_empty() && problems.len() == 2, "{problems:?}");
        let d = tempfile::tempdir().expect("temp");
        let v = json!({"PreToolUse": []});
        assert!(!trusted(d.path(), Path::new("/repo"), &v));
        trust(d.path(), Path::new("/repo"), &v).expect("trust");
        assert!(trusted(d.path(), Path::new("/repo"), &v));
        assert!(
            !trusted(d.path(), Path::new("/repo"), &json!({"PreToolUse": [{"hooks": []}]})),
            "changed: asked again"
        );
    }
}
