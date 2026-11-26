//! A status line of the person's own, in the common shape (`"statusLine": {"type": "command", "command": "…"}`):
//! the command gets the session as JSON on stdin and the first line it prints is shown above the prompt, drawn again
//! after each turn. When Code Arena's is not set, that of an agent `compat.agents` names (`~/.NAME/settings.json`).

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use crate::app::App;

/// How long the command may take: the prompt waits on it.
const WAIT: Duration = Duration::from_secs(2);

/// The command to run, if one is set.
pub fn command(app: &App) -> Option<String> {
    let of = |v: &Value| {
        (v.get("type").and_then(Value::as_str).unwrap_or("command") == "command")
            .then(|| v.get("command").and_then(Value::as_str).map(str::to_string))
            .flatten()
            .filter(|c| !c.trim().is_empty())
    };
    of(&app.settings.value(&ca_settings::schema::STATUS_LINE)).or_else(|| {
        let home = app.home.as_ref()?;
        app.agents().folders().iter().find_map(|f| {
            let v: Value =
                serde_json::from_str(&std::fs::read_to_string(home.join(f).join("settings.json")).ok()?).ok()?;
            of(v.get("statusLine")?)
        })
    })
}

/// What the command is told, in the common shape, and Code Arena's mode and edition.
pub fn input(p: &crate::session::Prepared, app: &App, usage: &ca_llm::Usage) -> Value {
    let style = app.settings.string(&ca_settings::schema::OUTPUT_STYLE).unwrap_or_else(|| "default".into());
    json!({
        "hook_event_name": "Status",
        "session_id": p.session,
        "cwd": p.agent.ctx.workspace.cwd(),
        "model": {"id": p.agent.model, "display_name": p.agent.model},
        "workspace": {"current_dir": p.agent.ctx.workspace.cwd(), "project_dir": p.agent.ctx.workspace.root()},
        "version": ca_core::VERSION,
        "output_style": {"name": style},
        "cost": {"total_input_tokens": usage.prompt, "total_output_tokens": usage.completion, "total_cached_tokens": usage.cached},
        "mode": p.agent.policy.mode.name(),
        "edition": app.edition().name(),
    })
}

/// The first line the command prints; `None` when it prints nothing, fails or takes too long.
pub async fn run(command: &str, input: &Value, cwd: &std::path::Path) -> Option<String> {
    #[cfg(windows)]
    let mut c = tokio::process::Command::new("cmd");
    #[cfg(windows)]
    c.arg("/C").arg(command);
    #[cfg(not(windows))]
    let mut c = tokio::process::Command::new("sh");
    #[cfg(not(windows))]
    c.arg("-c").arg(command);
    let mut child = c
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.to_string().as_bytes()).await;
    }
    let out = tokio::time::timeout(WAIT, child.wait_with_output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Colours (SGR) are kept; other control characters are not.
    let line: String =
        text.lines().find(|l| !l.trim().is_empty())?.chars().filter(|c| !c.is_control() || *c == '\u{1b}').collect();
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn the_first_line_the_command_prints_from_what_it_is_told() {
        let d = tempfile::tempdir().expect("temp");
        let input = json!({"model": {"display_name": "qwen"}});
        let line = run(
            "read j; echo; echo \"$j\" | sed 's/.*display_name\":\"\\([^\"]*\\)\".*/model \\1/'; echo second",
            &input,
            d.path(),
        )
        .await;
        assert_eq!(line.as_deref(), Some("model qwen"));
        assert_eq!(run("exit 3", &input, d.path()).await, None);
        assert_eq!(run("true", &input, d.path()).await, None);
    }
}
