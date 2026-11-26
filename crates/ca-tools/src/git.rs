//! git, reading only: status, diff, log, show, blame, branch. The repository's own settings run nothing in it (no
//! hooks, fsmonitor, external diff, text conversion, pager), nothing reaches the network, and no file outside the
//! folders the tools may use is named.

use std::process::Stdio;
use std::time::Duration;

use ca_llm::ToolDef;
use serde_json::{Value, json};

use crate::text::cut_middle;
use crate::{Context, Kind, Output, Tool};

const SUBCOMMANDS: &[&str] = &["status", "diff", "log", "show", "blame", "branch"];

/// Arguments that would run something, write somewhere, or reach outside, in any subcommand.
const REFUSED: &[&str] = &[
    "--no-index",
    "--ext-diff",
    "--textconv",
    "--exec",
    "--upload-pack",
    "--contents",
    "--remerge-diff",
    "--git-dir",
    "--work-tree",
];

/// What `git branch` may be given: listing only (a bare name would make a branch).
const BRANCH_FLAGS: &[&str] = &[
    "-a",
    "--all",
    "-r",
    "--remotes",
    "-v",
    "-vv",
    "--verbose",
    "-l",
    "--list",
    "--show-current",
    "--contains",
    "--no-contains",
    "--merged",
    "--no-merged",
    "--sort",
    "--format",
    "--points-at",
    "--column",
    "--no-column",
];

/// Settings given on git's command line over the repository's own: nothing of the repository's runs.
const SAFE_CONFIG: &[&str] = &[
    "core.fsmonitor=false",
    "core.hooksPath=/dev/null",
    "core.pager=cat",
    "diff.external=",
    "core.untrackedCache=false",
    "protocol.allow=never",
    "submodule.recurse=false",
    "color.ui=false",
];

pub struct Git;

impl Git {
    fn check(&self, args: &[String], ctx: &Context) -> Result<(), String> {
        let sub = args.first().ok_or("give a subcommand: status, diff, log, show, blame or branch")?;
        if !SUBCOMMANDS.contains(&sub.as_str()) {
            return Err(format!(
                "git {sub} is not one this tool runs (it only reads: {}); commits go through run_shell",
                SUBCOMMANDS.join(", ")
            ));
        }
        let refuse = |a: &str| Err(format!("{a} is not taken here: this tool only reads, inside the folder"));
        let mut listing = false;
        for a in &args[1..] {
            let (name, value) = match a.split_once('=') {
                Some((n, v)) if a.starts_with('-') => (n, Some(v)),
                _ => (a.as_str(), None),
            };
            if REFUSED.contains(&name) || name.starts_with("--output") {
                return refuse(a);
            }
            if sub == "branch" {
                if a.starts_with('-') {
                    if !BRANCH_FLAGS.contains(&name) {
                        return refuse(a);
                    }
                    listing |= name == "-l" || name == "--list";
                } else if !listing {
                    return Err(format!(
                        "git branch {a} would make a branch: this tool only reads (use --list PATTERN)"
                    ));
                }
            }
            // A path named anywhere must be inside, by its name or through a link.
            let candidate = if a.starts_with('-') { value } else { Some(a.as_str()) };
            if let Some(p) = candidate
                && (std::path::Path::new(p).is_absolute() || p.split(['/', '\\']).any(|part| part == ".."))
                && ctx.workspace.resolve(p).is_err()
            {
                return Err(format!("{p} is outside the folders the tools may use"));
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Tool for Git {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "git".into(),
            description: "Reads the repository with git: status, diff, log, show, blame or branch, with git's own arguments (args: [\"log\", \"-5\", \"--oneline\"]). It changes nothing; commit with run_shell.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"args": {"type": "array", "items": {"type": "string"}, "description": "The subcommand and its arguments."}},
                "required": ["args"]
            }),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        args.get("args")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let list: Vec<String> = match args.get("args") {
            Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            Some(Value::String(s)) => s.split_whitespace().map(str::to_string).collect(),
            _ => return Output::error("args is missing: give the subcommand and its arguments as a list"),
        };
        if let Err(why) = self.check(&list, ctx) {
            return Output::error(why);
        }
        // The repository's own filters and merge drivers (commands its settings name) do not run.
        let off = match filters_off(ctx.workspace.cwd()).await {
            Ok(o) => o,
            Err(why) => return Output::error(why),
        };
        let mut cmd = tokio::process::Command::new("git");
        for c in SAFE_CONFIG.iter().map(|c| c.to_string()).chain(off) {
            cmd.arg("-c").arg(c);
        }
        cmd.arg("--no-pager").arg("--no-optional-locks");
        cmd.arg(&list[0]);
        if matches!(list[0].as_str(), "diff" | "log" | "show") {
            cmd.arg("--no-ext-diff").arg("--no-textconv");
        }
        cmd.args(&list[1..])
            .current_dir(ctx.workspace.cwd())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env_remove("GIT_EXTERNAL_DIFF")
            .env_remove("ARENA_API_KEY")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let out = match tokio::time::timeout(Duration::from_secs(60), cmd.output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Output::error(format!("git could not start: {e} (is git installed?)")),
            Err(_) => return Output::error("git took more than 60 s, and was stopped"),
        };
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.trim().is_empty() {
            text.push_str(&err);
        }
        let text = cut_middle(text.trim_end(), 30_000);
        let text = if text.is_empty() { "(nothing)".to_string() } else { text };
        if out.status.success() { Output::ok(text) } else { Output::error(text) }
    }
}

/// The repository's filters and merge drivers, switched off (git config reads them and runs nothing): every
/// `filter.NAME.clean`, `smudge` and `process` and every `merge.NAME.driver` emptied, an empty name too (which
/// `filter=` or `merge=` in .gitattributes picks).
pub(crate) async fn filters_off(dir: &std::path::Path) -> Result<Vec<String>, String> {
    let out = tokio::process::Command::new("git")
        .args(["config", "-z", "--get-regexp", r"^(filter|merge)\."])
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("git could not start: {e}"))?;
    Ok(off_from(&String::from_utf8_lossy(&out.stdout))?)
}

/// What [`filters_off`] gives, from `git config -z --get-regexp` (KEY\nVALUE entries, or KEY alone).
pub(crate) fn off_from(output: &str) -> Result<Vec<String>, String> {
    let mut off = Vec::new();
    let mut names = std::collections::HashSet::new();
    for entry in output.split('\0').filter(|e| !e.is_empty()) {
        let key = entry.split('\n').next().unwrap_or("");
        let section = if key.starts_with("filter.") { "filter." } else { "merge." };
        let Some(dot) = key.rfind('.') else { continue };
        if dot < section.len() || !names.insert(key[..dot].to_string()) {
            continue;
        }
        let name = &key[section.len()..dot];
        if name.contains('=') || name.contains('\n') {
            return Err(format!(
                "The repository's settings name a {} that cannot be switched off from git's command line ({name}): git does not run here.",
                section.trim_end_matches('.')
            ));
        }
        if section == "filter." {
            for v in ["clean=", "smudge=", "process=", "required=false"] {
                off.push(format!("filter.{name}.{v}"));
            }
        } else {
            off.push(format!("merge.{name}.driver="));
        }
    }
    Ok(off)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_repositorys_filters_and_merge_drivers_are_switched_off() {
        let out = "filter.lfs.clean\ngit-lfs clean -- %f\0filter.lfs.smudge\ngit-lfs smudge -- %f\0filter.lfs.required\0merge.ours.driver\ntrue\0merge.renormalize\0";
        let off = super::off_from(out).expect("off");
        assert_eq!(
            off,
            vec![
                "filter.lfs.clean=",
                "filter.lfs.smudge=",
                "filter.lfs.process=",
                "filter.lfs.required=false",
                "merge.ours.driver="
            ]
        );
        assert!(super::off_from("filter.a=b.clean\nx\0").is_err());
    }

    #[tokio::test]
    async fn a_filter_the_repository_names_does_not_run() {
        let d = tempfile::tempdir().expect("temp");
        let git =
            |args: &[&str]| std::process::Command::new("git").args(args).current_dir(d.path()).output().expect("git");
        if !git(&["init", "-q"]).status.success() {
            return;
        }
        let marker = d.path().join("ran");
        std::fs::write(d.path().join("a.txt"), "x\n").expect("w");
        git(&["add", "a.txt"]);
        git(&["-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "-qm", "a"]);
        git(&["config", "filter.evil.clean", &format!("touch {}; cat", marker.display())]);
        std::fs::write(d.path().join(".gitattributes"), "*.txt filter=evil\n").expect("w");
        std::fs::write(d.path().join("a.txt"), "changed\n").expect("w");
        let ws = ca_workspace::Workspace::open(d.path(), &[]).expect("ws");
        let ctx = crate::Context::new(std::sync::Arc::new(ws), None, std::time::Duration::from_secs(30));
        // diff and status run clean filters on what changed.
        let o = crate::Tool::run(&super::Git, serde_json::json!({"args": ["status", "--short"]}), &ctx).await;
        assert!(!o.is_error, "{}", o.text);
        let _ = crate::Tool::run(&super::Git, serde_json::json!({"args": ["diff"]}), &ctx).await;
        assert!(!marker.exists(), "the repository's filter ran");
        // Git on its own runs it: the test would see it.
        git(&["diff"]);
        assert!(marker.exists(), "plain git runs the filter");
    }
}

#[cfg(test)]
mod tests_more {
    use super::*;
    use crate::testing::context;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_reading_subcommands_and_arguments() {
        let (_d, ctx) = context();
        assert!(Git.check(&args(&["log", "-5", "--oneline"]), &ctx).is_ok());
        assert!(Git.check(&args(&["diff", "-M", "HEAD~1"]), &ctx).is_ok(), "-M is rename detection in diff");
        assert!(Git.check(&args(&["commit", "-m", "x"]), &ctx).is_err());
        assert!(Git.check(&args(&["push"]), &ctx).is_err());
        assert!(Git.check(&args(&["diff", "--no-index", "a", "b"]), &ctx).is_err());
        assert!(Git.check(&args(&["branch", "-D", "main"]), &ctx).is_err());
        assert!(Git.check(&args(&["log", "--output=/tmp/x"]), &ctx).is_err());
        assert!(Git.check(&args(&["blame", "--contents", "/etc/passwd", "x"]), &ctx).is_err());
        assert!(Git.check(&args(&["log", "--", "../../etc/passwd"]), &ctx).is_err());
        assert!(Git.check(&args(&["-c", "core.pager=sh"]), &ctx).is_err());
    }
}
