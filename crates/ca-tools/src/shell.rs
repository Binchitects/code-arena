//! run_shell: a command in the person's shell, in the working directory, with a time limit, without the API key.

use std::process::Stdio;
use std::time::Duration;

use ca_llm::ToolDef;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::text::cut_middle;
use crate::{Context, Kind, Output, Tool, arg_str, arg_u64};

/// What the model reads of an output; the middle of a longer one is cut.
const SHOWN: usize = 30_000;
/// What is kept of an output at most, while it runs.
const KEPT: usize = 64 * 1024 * 1024;
const MOST_SECONDS: u64 = 600;

/// The variables a command never gets: the person's key stays Code Arena's.
pub const HIDDEN_VARS: &[&str] = &["ARENA_API_KEY"];

/// The shell and the flag that hands it a command line.
pub fn shell_for(setting: Option<&str>) -> (String, Vec<String>) {
    if let Some(s) = setting.filter(|s| !s.is_empty()) {
        let flag = if s.to_ascii_lowercase().contains("cmd") {
            "/c"
        } else if s.to_ascii_lowercase().contains("pwsh") || s.to_ascii_lowercase().contains("powershell") {
            "-Command"
        } else {
            "-c"
        };
        return (s.to_string(), vec![flag.to_string()]);
    }
    if cfg!(windows) {
        for bash in [r"C:\Program Files\Git\bin\bash.exe", r"C:\Program Files (x86)\Git\bin\bash.exe"] {
            if std::path::Path::new(bash).exists() {
                return (bash.to_string(), vec!["-c".into()]);
            }
        }
        if which("pwsh.exe") {
            return ("pwsh.exe".into(), vec!["-NoProfile".into(), "-Command".into()]);
        }
        return ("cmd.exe".into(), vec!["/c".into()]);
    }
    if which("bash") { ("bash".into(), vec!["-c".into()]) } else { ("sh".into(), vec!["-c".into()]) }
}

fn which(name: &str) -> bool {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|d| d.is_absolute() && d.join(name).is_file())
}

/// What an exit code past 128 says: the signal that ended the command.
pub fn signal_words(code: i32) -> Option<&'static str> {
    match code {
        130 => Some("interrupted (SIGINT)"),
        134 => Some("aborted (SIGABRT)"),
        137 => Some("killed (SIGKILL), often for running out of memory"),
        139 => Some("a segmentation fault (SIGSEGV)"),
        143 => Some("terminated (SIGTERM)"),
        _ => None,
    }
}

pub struct RunShell;

#[async_trait::async_trait]
impl Tool for RunShell {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "run_shell".into(),
            description: "Runs a command line in the shell (bash; on Windows Git's bash, else PowerShell), in the working directory. Its output and exit code come back; a long output is cut in the middle, its error lines listed. 120 s unless timeout says otherwise (at most 600).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command line."},
                    "timeout": {"type": "integer", "description": "Seconds before it is stopped (1 to 600)."}
                },
                "required": ["command"]
            }),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Shell
    }
    fn subject(&self, args: &Value) -> String {
        args.get("command").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let command = match arg_str(&args, "command") {
            Ok(c) => c.to_string(),
            Err(o) => return o,
        };
        let limit = arg_u64(&args, "timeout")
            .map(|s| Duration::from_secs(s.clamp(1, MOST_SECONDS)))
            .unwrap_or(ctx.shell_timeout);
        let (shell, flags) = shell_for(ctx.shell.as_deref());
        let mut cmd = tokio::process::Command::new(&shell);
        cmd.args(&flags)
            .arg(&command)
            .current_dir(ctx.workspace.cwd())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for v in HIDDEN_VARS {
            cmd.env_remove(v);
        }
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Output::error(format!("{shell} could not start: {e}")),
        };
        let pid = child.id();
        let mut out = child.stdout.take();
        let mut err = child.stderr.take();
        // stdout and stderr together, in the order they come (as a terminal shows them, near enough).
        let read_all = async {
            let mut buf = Vec::new();
            let (mut a, mut b) = (vec![0u8; 8192], vec![0u8; 8192]);
            let (mut a_open, mut b_open) = (out.is_some(), err.is_some());
            while a_open || b_open {
                tokio::select! {
                    n = async { match out.as_mut() { Some(o) => o.read(&mut a).await, None => Ok(0) } }, if a_open => {
                        match n { Ok(0) | Err(_) => a_open = false, Ok(n) => if buf.len() < KEPT { buf.extend_from_slice(&a[..n]) } }
                    }
                    n = async { match err.as_mut() { Some(e) => e.read(&mut b).await, None => Ok(0) } }, if b_open => {
                        match n { Ok(0) | Err(_) => b_open = false, Ok(n) => if buf.len() < KEPT { buf.extend_from_slice(&b[..n]) } }
                    }
                }
            }
            buf
        };
        let (bytes, status, timed_out) = match tokio::time::timeout(limit, async {
            let bytes = read_all.await;
            let status = child.wait().await;
            (bytes, status)
        })
        .await
        {
            Ok((bytes, status)) => (bytes, status.ok(), false),
            Err(_) => {
                stop_group(pid);
                (Vec::new(), None, true)
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        let shown = cut_middle(text.trim_end(), SHOWN);
        if timed_out {
            return Output::error(format!(
                "Stopped after {} s (its limit). Give a longer timeout (at most 600), or run it in parts.\n{shown}",
                limit.as_secs()
            ));
        }
        let code = status.and_then(|s| s.code());
        let head = match code {
            Some(0) => "Exit code 0".to_string(),
            Some(c) => match signal_words(c) {
                Some(w) => format!("Exit code {c}: {w}"),
                None => format!("Exit code {c}"),
            },
            None => "Ended by a signal".to_string(),
        };
        let body = if shown.is_empty() { "(no output)".to_string() } else { shown };
        let o = format!("{head}\n{body}");
        if code == Some(0) { Output::ok(o) } else { Output::error(o) }
    }
}

/// Stops the command and everything it started (its process group).
fn stop_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
        let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testing::context;

    #[tokio::test]
    async fn output_and_exit_code() {
        let (_d, ctx) = context();
        let o = RunShell.run(json!({"command": "echo hi; echo oops >&2; exit 3"}), &ctx).await;
        assert!(o.is_error);
        assert!(o.text.starts_with("Exit code 3\n"), "{}", o.text);
        assert!(o.text.contains("hi") && o.text.contains("oops"));
    }

    #[tokio::test]
    async fn runs_in_the_working_directory() {
        let (d, ctx) = context();
        let o = RunShell.run(json!({"command": "pwd"}), &ctx).await;
        let real = std::fs::canonicalize(d.path()).expect("canon");
        assert!(o.text.contains(&real.display().to_string()), "{}", o.text);
    }

    #[tokio::test]
    async fn a_command_past_its_limit_is_stopped_with_what_it_started() {
        let (_d, ctx) = context();
        let started = std::time::Instant::now();
        let o = RunShell.run(json!({"command": "sleep 30 & sleep 30", "timeout": 1}), &ctx).await;
        assert!(o.is_error);
        assert!(o.text.starts_with("Stopped after 1 s"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn signals_are_named() {
        assert_eq!(signal_words(137), Some("killed (SIGKILL), often for running out of memory"));
        assert_eq!(signal_words(1), None);
    }
}
