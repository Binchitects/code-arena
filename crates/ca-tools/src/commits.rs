//! Commits the model groups and writes the messages for, git's side: the changes as hunks with ids, the repository's
//! last subjects (its style), a group staged by its hunks alone (the index gets exactly those lines) and committed
//! with Code Arena's trailers, the person's identity the author. Never during a merge, rebase, cherry-pick or revert.

use std::path::{Path, PathBuf};
use std::process::Stdio;

/// One hunk of the working tree's changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// `h1`, `h2`…: what the model names it by.
    pub id: String,
    pub file: String,
    /// The file's own header (`diff --git`, `---`, `+++` and mode lines).
    pub header: String,
    /// `@@ … @@` and its lines; empty for a change git shows without hunks (a binary file, a mode change, a file
    /// deleted or made empty): such a file is one hunk, staged whole.
    pub body: String,
    pub whole_file: bool,
}

/// git in a folder, the repository's filters and drivers off, `input` fed to it when given: its output, or why not.
pub async fn git(dir: &Path, args: &[&str], input: Option<&str>) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    let mut cmd = tokio::process::Command::new("git");
    // The repository's own filters and drivers do not run (as for the git tool).
    for c in crate::git::filters_off(dir).await? {
        cmd.arg("-c").arg(c);
    }
    cmd.args(["-c", "core.quotepath=false", "-c", "color.ui=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("ARENA_API_KEY")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("git could not start: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(text.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("git {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// The repository's top folder.
pub async fn top(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"], None).await.ok().map(|t| PathBuf::from(t.trim()))
}

/// What git is in the middle of, if anything: no commits are made then.
pub async fn in_progress(dir: &Path) -> Option<&'static str> {
    let gd = git(dir, &["rev-parse", "--git-dir"], None).await.ok()?;
    let gd = dir.join(gd.trim());
    [
        ("MERGE_HEAD", "a merge"),
        ("rebase-merge", "a rebase"),
        ("rebase-apply", "a rebase"),
        ("CHERRY_PICK_HEAD", "a cherry-pick"),
        ("REVERT_HEAD", "a revert"),
    ]
    .into_iter()
    .find(|(f, _)| gd.join(f).exists())
    .map(|(_, w)| w)
}

/// The files with changes now (tracked and new), relative to the top.
pub async fn changed(top: &Path) -> Result<Vec<String>, String> {
    let s = git(top, &["status", "--porcelain=v1", "-z", "--untracked-files=all"], None).await?;
    let mut out = Vec::new();
    let mut parts = s.split('\0').filter(|p| !p.is_empty());
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        out.push(path.to_string());
        // A rename's source follows it.
        if code.starts_with('R') || code.starts_with('C') {
            parts.next();
        }
    }
    Ok(out)
}

/// The repository's last subjects, newest first: its style.
pub async fn subjects(top: &Path, most: usize) -> Vec<String> {
    git(top, &["log", &format!("-{most}"), "--format=%s"], None)
        .await
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The changes of `files` (all when none are named) as hunks: new files are shown as additions (git's intent to add,
/// taken back after).
pub async fn hunks(top: &Path, files: Option<&[String]>) -> Result<Vec<Hunk>, String> {
    let mut list: Vec<String> = match files {
        Some(f) => f.to_vec(),
        None => changed(top).await?,
    };
    list.sort();
    list.dedup();
    if list.is_empty() {
        return Ok(vec![]);
    }
    // Untracked ones: marked as to be added, so the diff shows them.
    let untracked: Vec<String> = git(top, &["ls-files", "--others", "--exclude-standard", "-z", "--"], None)
        .await
        .unwrap_or_default()
        .split('\0')
        .filter(|p| !p.is_empty() && list.iter().any(|l| l == p))
        .map(str::to_string)
        .collect();
    if !untracked.is_empty() {
        let mut args = vec!["add", "--intent-to-add", "--"];
        args.extend(untracked.iter().map(String::as_str));
        git(top, &args, None).await?;
    }
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--binary", "HEAD", "--"];
    if git(top, &["rev-parse", "--verify", "-q", "HEAD"], None).await.is_err() {
        // No commit yet: against the empty tree.
        args = vec!["diff", "--no-ext-diff", "--no-textconv", "--binary", "--cached", "--"];
    }
    args.extend(list.iter().map(String::as_str));
    let diff = git(top, &args, None).await;
    if !untracked.is_empty() {
        let mut args = vec!["reset", "-q", "--"];
        args.extend(untracked.iter().map(String::as_str));
        let _ = git(top, &args, None).await;
    }
    Ok(parse(&diff?))
}

/// A unified diff, as hunks with ids.
pub fn parse(diff: &str) -> Vec<Hunk> {
    let mut out = Vec::new();
    let mut n = 0;
    for file in diff
        .split("\ndiff --git ")
        .enumerate()
        .map(|(i, f)| if i == 0 { f.to_string() } else { format!("diff --git {f}") })
    {
        if !file.starts_with("diff --git ") {
            continue;
        }
        let file = if file.ends_with('\n') { file } else { format!("{file}\n") };
        let name = file
            .lines()
            .find_map(|l| l.strip_prefix("+++ b/"))
            .or_else(|| file.lines().find_map(|l| l.strip_prefix("--- a/")))
            .map(str::to_string)
            .unwrap_or_else(|| file.lines().next().unwrap_or("").rsplit(" b/").next().unwrap_or("").to_string());
        let (header, rest) = match file.find("\n@@ ") {
            Some(at) => (file[..at + 1].to_string(), file[at + 1..].to_string()),
            None => (file.clone(), String::new()),
        };
        let binary = header.contains("GIT binary patch") || header.contains("Binary files");
        if rest.is_empty() || binary {
            n += 1;
            out.push(Hunk {
                id: format!("h{n}"),
                file: name,
                header: file.clone(),
                body: String::new(),
                whole_file: true,
            });
            continue;
        }
        let mut cur = String::new();
        for line in rest.lines() {
            if line.starts_with("@@ ") && !cur.is_empty() {
                n += 1;
                out.push(Hunk {
                    id: format!("h{n}"),
                    file: name.clone(),
                    header: header.clone(),
                    body: std::mem::take(&mut cur),
                    whole_file: false,
                });
            }
            cur.push_str(line);
            cur.push('\n');
        }
        if !cur.is_empty() {
            n += 1;
            out.push(Hunk {
                id: format!("h{n}"),
                file: name.clone(),
                header: header.clone(),
                body: cur,
                whole_file: false,
            });
        }
    }
    out
}

/// The hunks as the model reads them: each with its id, file and lines (long ones cut).
pub fn shown(hunks: &[Hunk], most_chars: usize) -> String {
    let mut out = String::new();
    for h in hunks {
        let body =
            if h.whole_file { "(the whole file: a new, deleted or binary file)\n".to_string() } else { h.body.clone() };
        let body: String = if body.len() > 4000 {
            format!(
                "{}…(cut)\n",
                &body[..body.char_indices().take_while(|(i, _)| *i < 4000).last().map(|(i, _)| i).unwrap_or(0)]
            )
        } else {
            body
        };
        out.push_str(&format!("### {} · {}\n{body}\n", h.id, h.file));
        if out.len() > most_chars {
            out.push_str("(more hunks follow, not shown: group them with the hunks of their files)\n");
            break;
        }
    }
    out
}

/// One group staged by its hunks alone (the index gets exactly those lines) and committed with `message`. The commit.
pub async fn commit(top: &Path, hunks: &[&Hunk], message: &str, session: &str) -> Result<String, String> {
    // Nothing else staged: only this group's lines go in.
    git(top, &["reset", "-q"], None).await?;
    let mut patch = String::new();
    let mut whole: Vec<&str> = Vec::new();
    let mut last_header = "";
    for h in hunks {
        if h.whole_file {
            whole.push(&h.file);
            continue;
        }
        if h.header != last_header {
            patch.push_str(&h.header);
            last_header = &h.header;
        }
        patch.push_str(&h.body);
    }
    if !patch.is_empty() {
        git(top, &["apply", "--cached", "--recount", "--whitespace=nowarn", "-"], Some(&patch)).await?;
    }
    if !whole.is_empty() {
        let mut args = vec!["add", "-A", "--"];
        args.extend(whole);
        git(top, &args, None).await?;
    }
    let text = format!(
        "{}\n\nCo-Authored-By: Code Arena <noreply@code-arena.dev>\nCode-Arena-Session: {session}\n",
        message.trim()
    );
    git(top, &["commit", "-q", "-F", "-"], Some(&text)).await?;
    Ok(git(top, &["rev-parse", "--short", "HEAD"], None).await?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) {
        let o = std::process::Command::new("git").args(args).current_dir(dir).output().expect("git");
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    }

    #[tokio::test]
    async fn hunks_grouped_and_committed_by_their_lines_alone() {
        let d = tempfile::tempdir().expect("temp");
        let r = d.path();
        if std::process::Command::new("git").arg("--version").output().is_err() {
            return;
        }
        sh(r, &["init", "-q"]);
        sh(r, &["config", "user.email", "t@example.com"]);
        sh(r, &["config", "user.name", "T"]);
        let lines: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        std::fs::write(r.join("a.txt"), &lines).expect("w");
        sh(r, &["add", "."]);
        sh(r, &["commit", "-qm", "start"]);
        // Two far-apart changes in a.txt, and a new file.
        let edited = lines.replace("line 2\n", "line two\n").replace("line 38\n", "line thirty-eight\n");
        std::fs::write(r.join("a.txt"), edited).expect("w");
        std::fs::write(r.join("new.txt"), "fresh\n").expect("w");
        let top = top(r).await.expect("top");
        assert_eq!(in_progress(&top).await, None);
        let hs = hunks(&top, None).await.expect("hunks");
        assert_eq!(hs.len(), 3, "{hs:#?}");
        assert!(shown(&hs, 10_000).contains("### h1 · a.txt"));
        // The first group: the top of a.txt and the new file; the second: the bottom.
        let first: Vec<&Hunk> = hs.iter().filter(|h| h.id == "h1" || h.file == "new.txt").collect();
        let c1 = commit(&top, &first, "feat: two\n\nWhy: the first.", "s1").await.expect("commit 1");
        let rest: Vec<&Hunk> = hs.iter().filter(|h| h.id == "h2").collect();
        commit(&top, &rest, "feat: thirty-eight", "s1").await.expect("commit 2");
        let log = std::process::Command::new("git")
            .args(["log", "--format=%s%n%b", "-3"])
            .current_dir(r)
            .output()
            .expect("log");
        let log = String::from_utf8_lossy(&log.stdout);
        assert!(
            log.contains("feat: thirty-eight") && log.contains("feat: two") && log.contains("Code-Arena-Session: s1"),
            "{log}"
        );
        let show = std::process::Command::new("git")
            .args(["show", "--stat", "--format=", &c1])
            .current_dir(r)
            .output()
            .expect("show");
        let show = String::from_utf8_lossy(&show.stdout);
        assert!(show.contains("a.txt") && show.contains("new.txt"), "{show}");
        // All of it went in, nothing else.
        assert!(changed(&top).await.expect("status").is_empty());
        assert_eq!(subjects(&top, 5).await, vec!["feat: thirty-eight", "feat: two", "start"]);
    }
}
