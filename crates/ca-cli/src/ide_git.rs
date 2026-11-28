//! Source control: the repository's state, staging, commits, and a file as a revision has it.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver. Every message here needs only the project
//! (`Rule::Side`), so it is answered at once, a turn running or not; git itself runs on a thread of its own and the
//! reply is sent from there.
//!
//! - `git_status{repo?}` → `git_status{repo, root, branch, detached, upstream, ahead, behind, files, total, error}`.
//!   `repo` is the repository's folder as the project names it (empty or left out: the project's top) and every
//!   reply carries it back. `root` is where git's top is (null: no repository there). Each of `files` (at most
//!   2000 of `total`) is `{path, from, x, y, staged, changed, untracked, conflict}`, its paths from the project's
//!   top as the faces name files.
//! - `git_stage{repo, paths}`, `git_unstage{repo, paths}`, `git_discard{repo, paths}`: files or folders; each is
//!   followed by `git_status`, and a failure by `git_failed{repo, of, error}`. What is discarded of a file git does
//!   not track is deleted as the Explorer deletes, kept aside where it can be got back.
//! - `git_commit{repo, message, all, amend}` → `git_committed{repo, hash, error}` (`all`: every change is staged
//!   first, as VS Code's smart commit), then `git_status`.
//! - `git_show{repo, path, rev, working, key}` → `git_shown{repo, path, rev, working, key, exists, text, binary,
//!   too_large}`: `rev` empty is the index, else a revision (`HEAD`); `working` reads the file in the working tree
//!   instead; `key`, when given, is said back.
//! - `git_init{repo}` → a repository made there, then `git_status`.
//! - `git_branches{repo, key}` → `git_branches{repo, key, items, remotes, error}`: each of `items` (the latest
//!   commit first) is `{name, kind: local|remote|tag, current, hash, upstream, ahead, behind, subject}`.
//! - `git_switch{repo, name, kind}`, `git_branch_create{repo, name, from?}`, `git_branch_rename{repo, from?, to}`,
//!   `git_branch_delete{repo, name, force}`, `git_fetch{repo, all, auto}`, `git_pull{repo, rebase}`,
//!   `git_push{repo, force, remote?}` (a branch with no upstream is published), `git_sync{repo, rebase}` (a pull,
//!   then a push) and `git_undo_commit{repo}` → `git_done{repo, key, op, ok, error}` then `git_status`. A delete
//!   refused because the branch is not merged says `unmerged`; a publish, `published` (the remote); an undo, the
//!   commit's `message`; an automatic fetch, `auto` (one is skipped, and says `skipped`, while another fetch, pull or push runs). A
//!   fetch, pull, push or sync is stopped after ten minutes; a fetch or a push does not hold staging up.
//! - `git_merge_info{repo, path}` → `git_merge_info{repo, path, unmerged, kind, current, incoming, base_text,
//!   current_text, incoming_text, binary, too_large, error}`: what the merge editor shows of a file in conflict.
//!   `unmerged` is whether the index still has the file in conflict; `kind` is `merge`, `rebase`, `cherry-pick` or
//!   `revert` (what is under way); `current` and `incoming` are each `{name, commit, short, subject}` (the branch
//!   or the commit's name, its hash, short and long, its first line); the three texts are git's stages of the file
//!   (empty when a side has none).
//! - `git_merge_complete{repo, path, anyway}` → the file staged and `git_done{repo, of, path}`, then `git_status`;
//!   while it still has conflict markers (and `anyway` is not true), `git_failed{repo, of, path, error: "markers"}`.
//! - `git_repositories{detect, depth, ignored, open, key}` → `git_repositories{repo, key, items}`: the repositories
//!   Source Control shows, each `{path, name}` (`path` is what the other messages take as `repo`): the project's own
//!   first when it is in one, then (as `git.autoRepositoryDetection` says in `detect`) those in its folders down to
//!   `depth` levels, skipping the folder names in `ignored`, and those the `open` files are in. At most 100.
//!
//! Once a face has asked a repository's status it is told again by itself whenever git's own files change (the
//! index, HEAD, its log): a commit, a checkout or staging made in a terminal. The faces ask again after a file is
//! saved, made, moved or deleted, and after a turn.
//!
//! For the areas after this one (branches, history, several repositories): [`git`] runs git as every request here
//! does (no prompt, no pager, ssh in batch mode, no Arena key, what it says scrubbed of credentials), [`dir_of`]
//! resolves a message's `repo`, and [`push_status`] tells every face the repository's state.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::ide_ext::{Area, Rule, Shared, send};
use crate::ide_files::Side;

/// The most files a status lists.
pub(crate) const MOST_FILES: usize = 2000;

/// The biggest file `git_show` sends.
const MOST_SHOWN: u64 = 5 * 1024 * 1024;

/// How often git's own files are looked at, for a change made outside the window.
const LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("git_status", Rule::Side),
        ("git_stage", Rule::Side),
        ("git_unstage", Rule::Side),
        ("git_discard", Rule::Side),
        ("git_commit", Rule::Side),
        ("git_show", Rule::Side),
        ("git_init", Rule::Side),
        ("git_branches", Rule::Side),
        ("git_switch", Rule::Side),
        ("git_branch_create", Rule::Side),
        ("git_branch_rename", Rule::Side),
        ("git_branch_delete", Rule::Side),
        ("git_fetch", Rule::Side),
        ("git_pull", Rule::Side),
        ("git_push", Rule::Side),
        ("git_sync", Rule::Side),
        ("git_undo_commit", Rule::Side),
        ("git_merge_info", Rule::Side),
        ("git_merge_complete", Rule::Side),
        ("git_repositories", Rule::Side),
    ];

    fn start(sh: &Shared) {
        let kept = sh.side.kept.of::<Git>();
        if let Ok(mut me) = kept.me.lock() {
            *me = Arc::downgrade(&sh.side);
        }
        if kept.watching.swap(true, Ordering::SeqCst) || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let side = Arc::downgrade(&sh.side);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(LOOK_EVERY).await;
                let Some(side) = side.upgrade() else { break };
                let kept = side.kept.of::<Git>();
                let out = side.out().clone();
                drop(side);
                let _ = tokio::task::spawn_blocking(move || look(&kept, &out)).await;
            }
        });
    }

    fn side(side: &Side, m: &Value) -> bool {
        let of = m["type"].as_str().unwrap_or("");
        let repo = m["repo"].as_str().unwrap_or("").to_string();
        let out = side.out().clone();
        let dir = match dir_of(side, &repo) {
            Ok(d) => d,
            Err(e) => {
                send(
                    &out,
                    json!({"type": if of == "git_status" { "git_status" } else { "git_failed" }, "repo": repo, "of": of, "error": e, "key": m["key"]}),
                );
                return true;
            }
        };
        let kept = side.kept.of::<Git>();
        let me = kept.me.lock().ok().and_then(|w| w.upgrade());
        let (m, of) = (m.clone(), of.to_string());
        later(move || {
            // One fetch, pull, push or sync at a time, waited for before the index's lock (never while holding it);
            // an automatic fetch never waits, and says it was skipped.
            let network = matches!(of.as_str(), "git_fetch" | "git_pull" | "git_push" | "git_sync");
            let _net = match (network, m["auto"].as_bool().unwrap_or(false)) {
                (false, _) => None,
                (true, false) => kept.network.lock().ok(),
                (true, true) => match kept.network.try_lock() {
                    Ok(g) => Some(g),
                    Err(_) => {
                        let skipped = json!({"type": "git_done", "repo": repo, "op": of, "ok": true, "auto": true,
                            "skipped": true, "key": m["key"]});
                        send(&out, skipped);
                        return;
                    }
                },
            };
            // A fetch or a push leaves the index alone: it keeps neither staging nor the status waiting.
            let _one = (!matches!(of.as_str(), "git_fetch" | "git_push")).then(|| kept.lock.lock());
            answer(&of, &m, &repo, &dir, &out, &kept, me.as_deref());
        });
        true
    }
}

/// Runs work on a thread that may wait (inside the driver), or at once (in a test, with no runtime).
fn later(work: impl FnOnce() + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(h) => {
            h.spawn_blocking(work);
        }
        Err(_) => work(),
    }
}

/// What this area keeps for a folder: one git request at a time (no two take the index's lock), the folder's side,
/// and each repository asked about with what was last said of it.
#[derive(Default)]
pub(crate) struct Git {
    lock: Mutex<()>,
    /// One fetch, pull, push or sync at a time.
    network: Mutex<()>,
    me: Mutex<Weak<Side>>,
    watching: AtomicBool,
    asked: Mutex<HashMap<String, Asked>>,
}

/// A repository a face asked about: where it is, git's own folder, the stamps of git's files when it was last
/// looked at, and the status last said.
#[derive(Clone, Default)]
struct Asked {
    dir: PathBuf,
    git_dir: Option<PathBuf>,
    stamps: Vec<Option<u64>>,
    said: String,
}

/// The stamps of the files git changes when its state does: the index, HEAD, HEAD's log.
fn stamps(git_dir: &Path) -> Vec<Option<u64>> {
    ["index", "HEAD", "logs/HEAD"]
        .iter()
        .map(|f| std::fs::metadata(git_dir.join(f)).ok().map(|m| crate::ide_files::modified(&m)))
        .collect()
}

/// One look at each repository asked about: told again when git's files changed and its status with them.
fn look(kept: &Git, out: &broadcast::Sender<String>) {
    let asked: Vec<(String, Asked)> = match kept.asked.lock() {
        Ok(a) => a.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Err(_) => return,
    };
    for (repo, a) in asked {
        let Some(git_dir) = &a.git_dir else { continue };
        if stamps(git_dir) == a.stamps {
            continue;
        }
        let _one = kept.lock.lock();
        push_status_if_new(kept, out, &repo, &a.dir);
    }
}

/// The folder a message's `repo` names: the project's top when it is empty.
pub(crate) fn dir_of(side: &Side, repo: &str) -> Result<PathBuf, String> {
    if repo.is_empty() { Ok(side.workspace().root().to_path_buf()) } else { side.real(repo) }
}

/// A path a face gave, safe to hand git: from the project's top, never out of it.
fn path_ok(p: &str) -> bool {
    let p = p.trim_start_matches("./");
    !p.is_empty()
        && !p.starts_with(['/', '\\'])
        && !p.contains(['\0', ':'])
        && !Path::new(p).components().any(|c| matches!(c, std::path::Component::ParentDir))
}

/// The paths of a message, each checked.
fn paths_of(m: &Value) -> Result<Vec<String>, String> {
    let paths: Vec<String> =
        m["paths"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    if paths.is_empty() {
        return Err("no file was given".into());
    }
    match paths.iter().find(|p| !path_ok(p)) {
        Some(p) => Err(format!("{p} is not a path inside the project")),
        None => Ok(paths),
    }
}

/// What git says, with any credentials in a remote's address taken out.
pub(crate) fn scrub(text: &str) -> String {
    static AT: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"([a-zA-Z][a-zA-Z0-9+.-]*://)[^/@\s]+@").expect("regex"));
    AT.replace_all(text.trim(), "${1}***@").into_owned()
}

/// Git run in a folder as every request here runs it: never asking for a password, never paging, ssh in batch
/// mode, without the Arena key; `input` is given on its standard input. Ok: what it wrote; Err: what it said, scrubbed.
pub(crate) fn git(dir: &Path, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let mut child = spawn(dir, args, input.is_some())?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(input);
    }
    let o = child.wait_with_output().map_err(|e| format!("git could not be run: {e}"))?;
    said_of(o.status.success(), o.stdout, &o.stderr)
}

/// Git started as [`git`] runs it, its output piped.
fn spawn(dir: &Path, args: &[&str], input: bool) -> Result<std::process::Child, String> {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .args(["-c", "color.ui=false", "-c", "core.quotepath=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .env_remove("ARENA_API_KEY")
        .stdin(if input { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "git was not found: install it to use source control".to_string(),
        _ => format!("git could not be run: {e}"),
    })
}

/// What a finished git gives: what it wrote, or what it said first (not the usage it may print after), scrubbed.
fn said_of(ok: bool, stdout: Vec<u8>, stderr: &[u8]) -> Result<Vec<u8>, String> {
    if ok {
        return Ok(stdout);
    }
    let said = String::from_utf8_lossy(stderr);
    let said = if said.trim().is_empty() { String::from_utf8_lossy(&stdout) } else { said };
    let first: Vec<&str> = said.trim().lines().take(6).collect();
    Err(scrub(&first.join("\n")))
}

/// Where a folder's repository is: its top, the folder's place under it (`sub/`, or empty), git's own folder.
/// None: no repository there.
fn where_is(dir: &Path) -> Result<Option<(PathBuf, String, PathBuf)>, String> {
    match git(dir, &["rev-parse", "--show-toplevel", "--show-prefix", "--absolute-git-dir"], None) {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o).into_owned();
            let lines: Vec<&str> = text.lines().collect();
            let (top, git_dir) = (lines.first().copied().unwrap_or(""), lines.last().copied().unwrap_or(""));
            let prefix = if lines.len() >= 3 { lines[1] } else { "" };
            Ok(Some((PathBuf::from(top), prefix.to_string(), PathBuf::from(git_dir))))
        }
        Err(e) if e.contains("not a git repository") => Ok(None),
        Err(e) => Err(e),
    }
}

/// The branch line of `git status --branch`: the branch (none: detached), its upstream, ahead and behind.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Branch {
    pub name: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
}

pub(crate) fn branch_of(header: &str) -> Branch {
    let h = header.trim_start_matches("## ");
    let mut b = Branch::default();
    if let Some(name) = h.strip_prefix("No commits yet on ").or_else(|| h.strip_prefix("Initial commit on ")) {
        b.name = Some(name.to_string());
        return b;
    }
    if h.starts_with("HEAD (no branch)") {
        return b;
    }
    let (names, counts) = match h.split_once(" [") {
        Some((n, c)) => (n, c.trim_end_matches(']')),
        None => (h, ""),
    };
    match names.split_once("...") {
        Some((n, u)) => {
            b.name = Some(n.to_string());
            b.upstream = Some(u.to_string());
        }
        None => b.name = Some(names.to_string()),
    }
    for part in counts.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            b.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            b.behind = n.parse().unwrap_or(0);
        }
    }
    b
}

/// One file of a status: its path, where it was before a rename, its index (`x`) and working tree (`y`) letters.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub path: String,
    pub from: Option<String>,
    pub x: char,
    pub y: char,
}

impl Entry {
    pub fn conflict(&self) -> bool {
        matches!(
            (self.x, self.y),
            ('D', 'D') | ('A', 'U') | ('U', 'D') | ('U', 'A') | ('D', 'U') | ('A', 'A') | ('U', 'U')
        )
    }

    pub fn untracked(&self) -> bool {
        self.x == '?'
    }

    pub fn staged(&self) -> bool {
        !self.conflict() && !self.untracked() && self.x != ' ' && self.x != '!'
    }

    pub fn changed(&self) -> bool {
        !self.conflict() && !self.untracked() && self.y != ' ' && self.y != '!'
    }
}

/// `git status -z --porcelain=v1 --branch`: the branch line and the files (a rename's path first, then where it
/// was). Ignored files are left out.
pub(crate) fn parse_status(out: &[u8]) -> (Branch, Vec<Entry>) {
    let text = String::from_utf8_lossy(out);
    let mut parts = text.split('\0').filter(|p| !p.is_empty());
    let mut branch = Branch::default();
    let mut files = Vec::new();
    while let Some(p) = parts.next() {
        if p.starts_with("## ") {
            branch = branch_of(p);
            continue;
        }
        let mut chars = p.chars();
        let (Some(x), Some(y)) = (chars.next(), chars.next()) else { continue };
        let path = p.get(3..).unwrap_or("").to_string();
        let from =
            if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') { parts.next().map(str::to_string) } else { None };
        if x == '!' {
            continue;
        }
        files.push(Entry { path, from, x, y });
    }
    (branch, files)
}

/// A path git gave (from its top) as the project names it: from the project's top.
fn from_project(prefix: &str, p: &str) -> String {
    match p.strip_prefix(prefix) {
        Some(rest) if !prefix.is_empty() => rest.to_string(),
        _ if prefix.is_empty() => p.to_string(),
        _ => format!("{}{p}", "../".repeat(prefix.matches('/').count())),
    }
}

/// The status of the repository a folder is in, as `git_status` says it (without its type and repo), with git's
/// own folder.
fn status(dir: &Path) -> (Value, Option<PathBuf>) {
    let (top, prefix, git_dir) = match where_is(dir) {
        Ok(Some(w)) => w,
        Ok(None) => return (json!({"root": null, "files": [], "total": 0}), None),
        Err(e) => return (json!({"root": null, "files": [], "total": 0, "error": e}), None),
    };
    let out = match git(
        dir,
        &["--no-optional-locks", "status", "-z", "--porcelain=v1", "--branch", "--untracked-files=all", "--", "."],
        None,
    ) {
        Ok(o) => o,
        Err(e) => return (json!({"root": top.to_string_lossy(), "files": [], "total": 0, "error": e}), Some(git_dir)),
    };
    let (branch, files) = parse_status(&out);
    let total = files.len();
    let files: Vec<Value> = files
        .iter()
        .take(MOST_FILES)
        .map(|e| {
            json!({
                "path": from_project(&prefix, &e.path),
                "from": e.from.as_deref().map(|f| from_project(&prefix, f)),
                "x": e.x.to_string(),
                "y": e.y.to_string(),
                "staged": e.staged(),
                "changed": e.changed(),
                "untracked": e.untracked(),
                "conflict": e.conflict(),
            })
        })
        .collect();
    (
        json!({
            "root": top.to_string_lossy(),
            "branch": branch.name,
            "detached": branch.name.is_none(),
            "upstream": branch.upstream,
            "ahead": branch.ahead,
            "behind": branch.behind,
            "files": files,
            "total": total,
        }),
        Some(git_dir),
    )
}

/// The repository's status said to every face, and kept as what was last said of it.
pub(crate) fn push_status(kept: &Git, out: &broadcast::Sender<String>, repo: &str, dir: &Path) {
    let said = status_message(kept, repo, dir);
    let _ = out.send(said);
}

/// As `push_status`, but only when it is not what was last said.
fn push_status_if_new(kept: &Git, out: &broadcast::Sender<String>, repo: &str, dir: &Path) {
    let before = kept.asked.lock().ok().and_then(|a| a.get(repo).map(|a| a.said.clone()));
    let said = status_message(kept, repo, dir);
    if before.as_deref() != Some(said.as_str()) {
        let _ = out.send(said);
    }
}

/// The status message of a repository, kept with the stamps git's files had before it was read (so a change made
/// while it was read is seen at the next look).
fn status_message(kept: &Git, repo: &str, dir: &Path) -> String {
    let known = kept.asked.lock().ok().and_then(|a| a.get(repo).and_then(|a| a.git_dir.clone()));
    let before = known.as_deref().map(stamps);
    let (mut v, git_dir) = status(dir);
    v["type"] = json!("git_status");
    v["repo"] = json!(repo);
    let said = v.to_string();
    let stamps = match (&git_dir, before) {
        (Some(g), Some(b)) if known.as_ref() == Some(g) => b,
        (Some(g), _) => stamps(g),
        _ => Vec::new(),
    };
    if let Ok(mut a) = kept.asked.lock() {
        a.insert(repo.to_string(), Asked { dir: dir.to_path_buf(), git_dir, stamps, said: said.clone() });
    }
    said
}

/// Paths given to git on its standard input, each ended by a NUL.
fn nul_list(paths: &[String]) -> Vec<u8> {
    let mut b = Vec::new();
    for p in paths {
        b.extend_from_slice(p.trim_start_matches("./").as_bytes());
        b.push(0);
    }
    b
}

const FROM_INPUT: [&str; 2] = ["--pathspec-from-file=-", "--pathspec-file-nul"];

/// Whether the repository has a commit yet.
fn has_head(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--verify", "-q", "HEAD"], None).is_ok()
}

pub(crate) fn stage(dir: &Path, paths: &[String]) -> Result<(), String> {
    git(dir, &["--literal-pathspecs", "add", "-A", FROM_INPUT[0], FROM_INPUT[1]], Some(&nul_list(paths))).map(|_| ())
}

pub(crate) fn unstage(dir: &Path, paths: &[String]) -> Result<(), String> {
    let args: &[&str] = if has_head(dir) {
        &["--literal-pathspecs", "restore", "--staged", FROM_INPUT[0], FROM_INPUT[1]]
    } else {
        &["--literal-pathspecs", "rm", "--cached", "-r", "-q", "--ignore-unmatch", FROM_INPUT[0], FROM_INPUT[1]]
    };
    git(dir, args, Some(&nul_list(paths))).map(|_| ())
}

/// The files `git ls-files` lists under the paths (`ls-files` reads no paths from its input, so they are given
/// a few hundred at a time), from the project's top.
fn listed_under(dir: &Path, how: &[&str], paths: &[String]) -> Result<Vec<String>, String> {
    let mut all = Vec::new();
    for some in paths.chunks(200) {
        let mut args = vec!["--literal-pathspecs", "ls-files", "-z"];
        args.extend_from_slice(how);
        args.push("--");
        args.extend(some.iter().map(|p| p.trim_start_matches("./")));
        let o = git(dir, &args, None)?;
        all.extend(String::from_utf8_lossy(&o).split('\0').filter(|p| !p.is_empty()).map(str::to_string));
    }
    Ok(all)
}

/// The changes to the paths that git tracks taken back to what the index has; the files it does not track are
/// handed to `delete` (the Explorer's delete, which keeps them aside). Says the files that could not be deleted.
pub(crate) fn discard(dir: &Path, paths: &[String], delete: &dyn Fn(&str) -> bool) -> Result<(), String> {
    let untracked = listed_under(dir, &["--others", "--exclude-standard"], paths)?;
    let tracked = listed_under(dir, &[], paths)?;
    let under = |t: &str, p: &str| {
        let p = p.trim_start_matches("./").trim_end_matches('/');
        p == "." || t == p || t.starts_with(&format!("{p}/"))
    };
    let some_tracked: Vec<String> = paths.iter().filter(|p| tracked.iter().any(|t| under(t, p))).cloned().collect();
    if !some_tracked.is_empty() {
        let input = nul_list(&some_tracked);
        git(dir, &["--literal-pathspecs", "restore", "--worktree", FROM_INPUT[0], FROM_INPUT[1]], Some(&input))?;
    }
    let failed: Vec<&String> = untracked.iter().filter(|u| !delete(u)).collect();
    match failed.as_slice() {
        [] => Ok(()),
        [one] => Err(format!("{one} could not be deleted")),
        many => Err(format!("{} files could not be deleted", many.len())),
    }
}

/// A commit made of what is staged (`all`: of every change, staged first); its short hash.
pub(crate) fn commit(dir: &Path, message: &str, all: bool, amend: bool) -> Result<String, String> {
    let message = message.trim();
    if message.is_empty() && !amend {
        return Err("Type a message to commit.".into());
    }
    if all {
        git(dir, &["add", "-A", "--", "."], None)?;
    }
    let mut args = vec!["commit", "--quiet"];
    if amend {
        args.push("--amend");
    }
    if message.is_empty() {
        args.push("--no-edit");
        git(dir, &args, None)?;
    } else {
        args.extend(["-F", "-"]);
        git(dir, &args, Some(message.as_bytes()))?;
    }
    let hash = git(dir, &["rev-parse", "--short", "HEAD"], None)?;
    Ok(String::from_utf8_lossy(&hash).trim().to_string())
}

/// A file as a revision has it (`rev` empty: the index), as `git_shown` says it (without its type and keys).
pub(crate) fn show(dir: &Path, path: &str, rev: &str) -> Value {
    if rev.starts_with('-') || rev.contains([':', ' ', '\0']) || !path_ok(path) {
        return json!({"exists": false, "error": "not a revision or a path git takes"});
    }
    let object = format!("{rev}:./{}", path.trim_start_matches("./"));
    let size = match git(dir, &["cat-file", "-s", &object], None) {
        Ok(o) => String::from_utf8_lossy(&o).trim().parse::<u64>().unwrap_or(0),
        Err(_) => return json!({"exists": false}),
    };
    if size > MOST_SHOWN {
        return json!({"exists": true, "too_large": true});
    }
    match git(dir, &["cat-file", "blob", &object], None) {
        Ok(bytes) => text_of(&bytes),
        Err(e) => json!({"exists": false, "error": e}),
    }
}

/// A file's bytes as `git_shown` says them: text, or that it is binary.
fn text_of(bytes: &[u8]) -> Value {
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return json!({"exists": true, "binary": true});
    }
    json!({"exists": true, "text": String::from_utf8_lossy(bytes)})
}

/// A file in the working tree, as `git_shown` says it.
fn show_working(dir: &Path, path: &str) -> Value {
    if !path_ok(path) {
        return json!({"exists": false, "error": "not a path inside the project"});
    }
    let file = dir.join(path.trim_start_matches("./"));
    // A link that leads out of the repository is not read.
    let inside = match (std::fs::canonicalize(&file), std::fs::canonicalize(dir)) {
        (Ok(f), Ok(d)) => f.starts_with(d),
        _ => true,
    };
    if !inside {
        return json!({"exists": false, "error": "it leads out of the project"});
    }
    match std::fs::metadata(&file) {
        Ok(m) if m.len() > MOST_SHOWN => json!({"exists": true, "too_large": true}),
        Ok(m) if m.is_file() => match std::fs::read(&file) {
            Ok(b) => text_of(&b),
            Err(e) => json!({"exists": false, "error": e.to_string()}),
        },
        _ => json!({"exists": false}),
    }
}

/// What is under way in a repository whose git folder this is: a rebase, a cherry-pick, a revert or a merge (the
/// rebase first: a stopped rebase may leave a cherry-pick's file too).
fn under_way(git_dir: &Path) -> &'static str {
    if git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir() {
        "rebase"
    } else if git_dir.join("CHERRY_PICK_HEAD").is_file() {
        "cherry-pick"
    } else if git_dir.join("REVERT_HEAD").is_file() {
        "revert"
    } else {
        "merge"
    }
}

/// A file of git's folder, its first line trimmed (none: not there or empty).
fn first_line(file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    let line = text.lines().next()?.trim().to_string();
    (!line.is_empty()).then_some(line)
}

/// A commit as the merge editor names a side: `{name, commit, short, subject}`; the name given, else the branch git
/// names the commit by, else its short hash.
fn side_of(dir: &Path, rev: &str, name: Option<String>) -> Value {
    let said = git(dir, &["log", "-1", "--format=%H%x00%h%x00%s", rev, "--"], None).unwrap_or_default();
    let said = String::from_utf8_lossy(&said).trim_end().to_string();
    let mut parts = said.splitn(3, '\0');
    let (commit, short, subject) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let name = name.filter(|n| !n.is_empty()).or_else(|| {
        let named =
            git(dir, &["name-rev", "--name-only", "--no-undefined", "--exclude=refs/tags/*", rev], None).ok()?;
        let named = String::from_utf8_lossy(&named).trim().to_string();
        (!named.is_empty()).then_some(named)
    });
    json!({"name": name.unwrap_or_else(|| short.to_string()), "commit": commit, "short": short, "subject": subject})
}

/// One of git's stages of a file in conflict (1 the base, 2 the current, 3 the incoming), as `git_shown` says a file.
fn stage_of(dir: &Path, path: &str, n: u8) -> Value {
    let object = format!(":{n}:./{}", path.trim_start_matches("./"));
    let size = match git(dir, &["cat-file", "-s", &object], None) {
        Ok(o) => String::from_utf8_lossy(&o).trim().parse::<u64>().unwrap_or(0),
        Err(_) => return json!({"exists": false}),
    };
    if size > MOST_SHOWN {
        return json!({"exists": true, "too_large": true});
    }
    match git(dir, &["cat-file", "blob", &object], None) {
        Ok(bytes) => text_of(&bytes),
        Err(_) => json!({"exists": false}),
    }
}

/// What the merge editor shows of a file in conflict, as `git_merge_info` says it (without its type and keys).
pub(crate) fn merge_info(dir: &Path, path: &str) -> Value {
    if !path_ok(path) {
        return json!({"unmerged": false, "error": "not a path inside the project"});
    }
    let git_dir = match where_is(dir) {
        Ok(Some((_, _, g))) => g,
        Ok(None) => return json!({"unmerged": false, "error": "the folder has no git repository"}),
        Err(e) => return json!({"unmerged": false, "error": e}),
    };
    let listed = git(dir, &["--literal-pathspecs", "ls-files", "-u", "--", path.trim_start_matches("./")], None);
    let unmerged = listed.is_ok_and(|o| !o.is_empty());
    let kind = under_way(&git_dir);
    let branch = git(dir, &["symbolic-ref", "--short", "-q", "HEAD"], None)
        .ok()
        .map(|o| String::from_utf8_lossy(&o).trim().to_string());
    let (incoming_rev, incoming_name) = match kind {
        // The branch being rebased is the incoming side's; its commit being put on the other is REBASE_HEAD.
        "rebase" => {
            let head = ["rebase-merge", "rebase-apply"]
                .iter()
                .find_map(|d| first_line(&git_dir.join(d).join("head-name")))
                .map(|h| h.trim_start_matches("refs/heads/").to_string());
            ("REBASE_HEAD", head)
        }
        "cherry-pick" => ("CHERRY_PICK_HEAD", None),
        "revert" => ("REVERT_HEAD", None),
        _ => {
            // "Merge branch 'feature'": the branch merged, as the message git wrote names it.
            let named = first_line(&git_dir.join("MERGE_MSG")).and_then(|m| {
                let rest =
                    m.strip_prefix("Merge branch '").or_else(|| m.strip_prefix("Merge remote-tracking branch '"))?;
                rest.split('\'').next().map(str::to_string)
            });
            ("MERGE_HEAD", named)
        }
    };
    let current = side_of(dir, "HEAD", if kind == "rebase" { None } else { branch });
    let incoming = if git(dir, &["rev-parse", "--verify", "-q", incoming_rev], None).is_ok() {
        side_of(dir, incoming_rev, incoming_name)
    } else {
        json!({"name": incoming_name.unwrap_or_default(), "commit": "", "short": "", "subject": ""})
    };
    let mut v = json!({"unmerged": unmerged, "kind": kind, "current": current, "incoming": incoming,
        "binary": false, "too_large": false});
    for (n, key) in [(1, "base_text"), (2, "current_text"), (3, "incoming_text")] {
        let stage = stage_of(dir, path, n);
        for flag in ["binary", "too_large"] {
            if stage[flag] == true {
                v[flag] = json!(true);
            }
        }
        v[key] = json!(stage["text"].as_str().unwrap_or(""));
    }
    v
}

/// Whether a text still has a conflict's markers: a `<<<<<<<` line, a `=======` after it, a `>>>>>>>` after that.
pub(crate) fn has_markers(text: &str) -> bool {
    let mut seen = 0;
    for l in text.lines() {
        match seen {
            0 if l.starts_with("<<<<<<<") => seen = 1,
            1 if l.starts_with("=======") => seen = 2,
            2 if l.starts_with(">>>>>>>") => return true,
            _ => {}
        }
    }
    false
}

/// Complete Merge: the file as saved staged, unless it still has markers and that was not asked for anyway.
pub(crate) fn merge_complete(dir: &Path, path: &str, anyway: bool) -> Result<(), String> {
    if !path_ok(path) {
        return Err(format!("{path} is not a path inside the project"));
    }
    let file = dir.join(path.trim_start_matches("./"));
    if !anyway && std::fs::read(&file).is_ok_and(|b| has_markers(&String::from_utf8_lossy(&b))) {
        return Err("markers".into());
    }
    stage(dir, &[path.to_string()])
}

/// A request answered, on git's thread.
fn answer(
    of: &str,
    m: &Value,
    repo: &str,
    dir: &Path,
    out: &broadcast::Sender<String>,
    kept: &Git,
    side: Option<&Side>,
) {
    let failed = |e: String| send(out, json!({"type": "git_failed", "repo": repo, "of": of, "error": e}));
    match of {
        "git_status" => push_status(kept, out, repo, dir),
        "git_stage" | "git_unstage" | "git_discard" => {
            let done = paths_of(m).and_then(|paths| match of {
                "git_stage" => stage(dir, &paths),
                "git_unstage" => unstage(dir, &paths),
                _ => {
                    let root = side.map(|s| s.workspace().root().to_path_buf());
                    discard(dir, &paths, &|p: &str| {
                        let (Some(side), Some(root)) = (side, &root) else { return false };
                        let Ok(shown) = dir.join(p).strip_prefix(root).map(|r| r.to_string_lossy().into_owned()) else {
                            return false;
                        };
                        side.handle(&json!({"type": "delete", "path": shown}));
                        !dir.join(p).exists()
                    })
                }
            });
            if let Err(e) = done {
                failed(e);
            }
            push_status(kept, out, repo, dir);
        }
        "git_commit" => {
            let done = commit(
                dir,
                m["message"].as_str().unwrap_or(""),
                m["all"].as_bool().unwrap_or(false),
                m["amend"].as_bool().unwrap_or(false),
            );
            let reply = match done {
                Ok(hash) => json!({"type": "git_committed", "repo": repo, "hash": hash}),
                Err(e) => json!({"type": "git_committed", "repo": repo, "error": e}),
            };
            send(out, reply);
            push_status(kept, out, repo, dir);
        }
        "git_show" => {
            let path = m["path"].as_str().unwrap_or("");
            let rev = m["rev"].as_str().unwrap_or("");
            let working = m["working"].as_bool().unwrap_or(false);
            let mut v = if working { show_working(dir, path) } else { show(dir, path, rev) };
            v["type"] = json!("git_shown");
            v["repo"] = json!(repo);
            v["path"] = json!(path);
            v["rev"] = json!(rev);
            v["working"] = json!(working);
            // The key the face asked with, said back (quick diff's requests, beside the changes tabs').
            if let Some(key) = m["key"].as_str() {
                v["key"] = json!(key);
            }
            send(out, v);
        }
        "git_merge_info" => {
            let path = m["path"].as_str().unwrap_or("");
            let mut v = merge_info(dir, path);
            v["type"] = json!("git_merge_info");
            v["repo"] = json!(repo);
            v["path"] = json!(path);
            send(out, v);
        }
        "git_merge_complete" => {
            let path = m["path"].as_str().unwrap_or("");
            let reply = match merge_complete(dir, path, m["anyway"].as_bool().unwrap_or(false)) {
                Ok(()) => json!({"type": "git_done", "repo": repo, "of": of, "op": of, "ok": true, "path": path}),
                Err(e) => json!({"type": "git_failed", "repo": repo, "of": of, "path": path, "error": e}),
            };
            send(out, reply);
            push_status(kept, out, repo, dir);
        }
        "git_init" => {
            if let Err(e) = git(dir, &["init", "--quiet"], None) {
                failed(e);
            }
            push_status(kept, out, repo, dir);
        }
        "git_branches" => {
            let mut v = branches(dir);
            v["type"] = json!("git_branches");
            v["repo"] = json!(repo);
            v["key"] = m["key"].clone();
            send(out, v);
        }
        "git_repositories" => {
            let ignored: Vec<String> =
                m["ignored"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let open: Vec<String> =
                m["open"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let detect = match &m["detect"] {
                Value::Bool(b) => b.to_string(),
                Value::String(s) => s.clone(),
                _ => "true".to_string(),
            };
            let depth = m["depth"].as_i64().unwrap_or(1);
            let top = matches!(where_is(dir), Ok(Some(_)));
            let items: Vec<Value> = repositories(dir, top, &detect, depth, &ignored, &open)
                .into_iter()
                .map(|(path, name)| json!({"path": path, "name": name}))
                .collect();
            send(out, json!({"type": "git_repositories", "repo": repo, "key": m["key"], "items": items}));
        }
        _ => remote_answer(of, m, repo, dir, out, kept),
    }
}

/// The most repositories Source Control shows, as VS Code's.
const MOST_REPOSITORIES: usize = 100;

/// The deepest a scan goes when `git.repositoryScanMaxDepth` is -1.
const DEEPEST_SCAN: i64 = 10;

/// Whether a folder is a repository's top: it has git's folder (or, a submodule's or a worktree's, git's file).
fn is_top(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(".git")).is_ok_and(|m| m.is_dir() || m.is_file())
}

/// The repositories Source Control shows, each `(folder from the project's top, name)`: the project's own first
/// (`top`: the project is in one), then, as `detect` (`git.autoRepositoryDetection`: "true", "subFolders",
/// "openEditors", "false") says, those in its folders down to `depth` levels (skipping `ignored` names and
/// hidden folders) and those the `open` files are in. At most 100, in the order found.
pub(crate) fn repositories(
    root: &Path,
    top: bool,
    detect: &str,
    depth: i64,
    ignored: &[String],
    open: &[String],
) -> Vec<(String, String)> {
    let mut found: Vec<String> = Vec::new();
    if top {
        found.push(String::new());
    }
    let depth = if depth < 0 { DEEPEST_SCAN } else { depth.min(DEEPEST_SCAN) };
    if matches!(detect, "true" | "subFolders") && depth > 0 {
        let mut level = vec![PathBuf::new()];
        for _ in 0..depth {
            let mut next = Vec::new();
            for rel in level {
                let Ok(entries) = std::fs::read_dir(root.join(&rel)) else { continue };
                let mut dirs: Vec<(String, PathBuf)> = entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .filter_map(|e| e.file_name().to_str().map(|n| (n.to_string(), rel.join(n))))
                    .filter(|(n, _)| !n.starts_with('.') && !ignored.iter().any(|i| i == n))
                    .collect();
                dirs.sort();
                for (_, d) in dirs {
                    if is_top(&root.join(&d)) {
                        found.push(d.to_string_lossy().replace('\\', "/"));
                        if found.len() >= MOST_REPOSITORIES {
                            return named(root, found);
                        }
                    }
                    next.push(d);
                }
            }
            level = next;
        }
    }
    if matches!(detect, "true" | "openEditors") {
        for file in open.iter().filter(|p| path_ok(p)) {
            let mut at = Path::new(file.trim_start_matches("./")).parent();
            while let Some(d) = at.filter(|d| !d.as_os_str().is_empty()) {
                if is_top(&root.join(d)) {
                    let d = d.to_string_lossy().replace('\\', "/");
                    if !found.contains(&d) && found.len() < MOST_REPOSITORIES {
                        found.push(d);
                    }
                    break;
                }
                at = d.parent();
            }
        }
    }
    named(root, found)
}

/// Each repository's folder with its name: its last folder's, the project's own the project's folder's.
fn named(root: &Path, found: Vec<String>) -> Vec<(String, String)> {
    found
        .into_iter()
        .map(|p| {
            let name = match p.rsplit('/').next().filter(|n| !n.is_empty()) {
                Some(n) => n.to_string(),
                None => root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            };
            (p, name)
        })
        .collect()
}

/// The longest a fetch, pull, push or sync may take before it is stopped.
const MOST_NETWORK: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Git as [`git`] runs it, stopped once it has taken `limit`. What it writes is read beside it; a git stopped is
/// not waited for further (what it started, ssh, may keep its output open).
pub(crate) fn git_for(dir: &Path, args: &[&str], limit: std::time::Duration) -> Result<Vec<u8>, String> {
    fn read(pipe: Option<impl std::io::Read + std::marker::Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut b);
            }
            b
        })
    }
    let mut child = spawn(dir, args, false)?;
    let stdout = read(child.stdout.take());
    let stderr = read(child.stderr.take());
    let end = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(done)) => {
                let (o, e) = (stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default());
                return said_of(done.success(), o, &e);
            }
            Ok(None) if std::time::Instant::now() >= end => {
                let _ = child.kill();
                let _ = child.wait();
                let minutes = limit.as_secs().div_ceil(60);
                return Err(format!("git {} took longer than {minutes} minutes and was stopped", args[0]));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(format!("git could not be run: {e}")),
        }
    }
}

/// A name the window gives (a branch, a remote, a revision) that git cannot take for an option.
fn name_ok(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && !name.contains(['\0', '\n', ' '])
}

/// The repository's branches, remote branches and tags, the latest commit first, and its remotes, as
/// `git_branches` says them: `items` of `{name, kind: local|remote|tag, current, hash, upstream, ahead, behind,
/// subject}`, `remotes`, `error`.
pub(crate) fn branches(dir: &Path) -> Value {
    let format = "--format=%(HEAD)%00%(refname)%00%(objectname:short)%00%(upstream:short)%00\
                  %(upstream:track,nobracket)%00%(subject)";
    let refs = ["for-each-ref", "--sort=-committerdate", format, "refs/heads", "refs/remotes", "refs/tags"];
    let out = match git(dir, &refs, None) {
        Ok(o) => o,
        Err(e) => return json!({"items": [], "remotes": [], "error": e}),
    };
    let text = String::from_utf8_lossy(&out);
    let items: Vec<Value> = text.lines().filter_map(branch_line).take(MOST_FILES).collect();
    json!({"items": items, "remotes": remotes(dir).unwrap_or_default()})
}

/// One line of `for-each-ref` as an item of `git_branches` (none: a remote's HEAD, or a line it cannot read).
fn branch_line(line: &str) -> Option<Value> {
    let f: Vec<&str> = line.split('\0').collect();
    let [head, refname, hash, upstream, track, subject] = f[..] else { return None };
    let (kind, name) = if let Some(n) = refname.strip_prefix("refs/heads/") {
        ("local", n)
    } else if let Some(n) = refname.strip_prefix("refs/remotes/") {
        ("remote", n)
    } else {
        ("tag", refname.strip_prefix("refs/tags/")?)
    };
    if kind == "remote" && (name.ends_with("/HEAD") || !name.contains('/')) {
        return None;
    }
    let count = |word: &str| {
        track.split(", ").find_map(|p| p.strip_prefix(word)).and_then(|n| n.trim().parse::<u64>().ok()).unwrap_or(0)
    };
    Some(json!({
        "name": name, "kind": kind, "current": head == "*", "hash": hash,
        "upstream": (!upstream.is_empty()).then_some(upstream), "ahead": count("ahead "), "behind": count("behind "),
        "subject": subject,
    }))
}

/// The repository's remotes, by name.
fn remotes(dir: &Path) -> Result<Vec<String>, String> {
    git(dir, &["remote"], None)
        .map(|o| String::from_utf8_lossy(&o).lines().map(str::to_string).filter(|r| !r.is_empty()).collect())
}

/// The branch checked out (none: detached), and its upstream.
fn current(dir: &Path) -> (Option<String>, Option<String>) {
    let text = |args: &[&str]| {
        git(dir, args, None).ok().map(|o| String::from_utf8_lossy(&o).trim().to_string()).filter(|n| !n.is_empty())
    };
    (
        text(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
        text(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]),
    )
}

/// Where a branch without an upstream is published: the remote asked for, else the only one, else `origin`.
fn remote_for(dir: &Path, asked: Option<&str>) -> Result<String, String> {
    let remotes = remotes(dir)?;
    if let Some(a) = asked.filter(|a| !a.is_empty()) {
        return if remotes.iter().any(|r| r == a) {
            Ok(a.to_string())
        } else {
            Err(format!("There is no remote '{a}'."))
        };
    }
    match remotes.as_slice() {
        [] => Err("Your repository has no remotes configured to publish to.".into()),
        [only] => Ok(only.clone()),
        many if many.iter().any(|r| r == "origin") => Ok("origin".into()),
        _ => Err("Your repository has several remotes: pick the one to publish to.".into()),
    }
}

/// Checks out a branch: a local one; a remote one through its local branch, made to track it when there is none;
/// a tag or a commit, detached.
pub(crate) fn switch(dir: &Path, name: &str, kind: &str) -> Result<(), String> {
    if !name_ok(name) {
        return Err(format!("'{name}' is not a valid name."));
    }
    match kind {
        "local" => git(dir, &["switch", "--quiet", name], None).map(drop),
        "remote" => {
            let local = name.split_once('/').map(|(_, l)| l).unwrap_or(name);
            let exists = git(dir, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{local}")], None).is_ok();
            if exists {
                git(dir, &["switch", "--quiet", local], None).map(drop)
            } else {
                git(dir, &["switch", "--quiet", "--track", name], None).map(drop)
            }
        }
        _ => git(dir, &["switch", "--quiet", "--detach", name], None).map(drop),
    }
}

/// Puts the last commit back: its changes staged again, its message given back. The first commit too.
pub(crate) fn undo_commit(dir: &Path) -> Result<String, String> {
    let message = git(dir, &["log", "-1", "--format=%B"], None)
        .map_err(|_| "There is no commit to undo.".to_string())
        .map(|o| String::from_utf8_lossy(&o).trim().to_string())?;
    if git(dir, &["rev-parse", "--verify", "--quiet", "HEAD~1"], None).is_ok() {
        git(dir, &["reset", "--soft", "HEAD~1"], None)?;
    } else {
        git(dir, &["update-ref", "-d", "HEAD"], None)?;
    }
    Ok(message)
}

/// Pushes the branch checked out: to its upstream, or published to a remote when it has none (Ok: the remote it
/// was published to).
pub(crate) fn push(dir: &Path, force: bool, remote: Option<&str>) -> Result<Option<String>, String> {
    let (Some(branch), upstream) = current(dir) else {
        return Err("Not on a branch: check one out to push it.".into());
    };
    let mut args = vec!["push", "--quiet"];
    if force {
        args.push("--force-with-lease");
    }
    if upstream.is_some() {
        return git_for(dir, &args, MOST_NETWORK).map(|_| None);
    }
    let to = remote_for(dir, remote)?;
    args.extend(["--set-upstream", to.as_str(), branch.as_str()]);
    git_for(dir, &args, MOST_NETWORK).map(|_| Some(to.clone()))
}

/// Pulls into the branch checked out, merging or (`rebase`) rebasing.
pub(crate) fn pull(dir: &Path, rebase: bool) -> Result<(), String> {
    let args: &[&str] = if rebase { &["pull", "--quiet", "--rebase"] } else { &["pull", "--quiet"] };
    git_for(dir, args, MOST_NETWORK).map(drop)
}

/// A branch, fetch, pull, push or sync request answered, on git's thread: `git_done{repo, key, op, ok, error}`
/// (with `unmerged`, `message` or `published` where they are said), then the status.
fn remote_answer(of: &str, m: &Value, repo: &str, dir: &Path, out: &broadcast::Sender<String>, kept: &Git) {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    let b = |k: &str| m[k].as_bool().unwrap_or(false);
    let mut more = serde_json::Map::new();
    let valid = |n: &str| if name_ok(n) { Ok(()) } else { Err(format!("'{n}' is not a valid branch name.")) };
    let done: Result<(), String> = match of {
        "git_switch" => switch(dir, &s("name"), m["kind"].as_str().unwrap_or("local")),
        "git_branch_create" => {
            let (name, from) = (s("name"), s("from"));
            valid(&name).and_then(|_| {
                if from.is_empty() {
                    git(dir, &["switch", "--quiet", "-c", &name], None).map(drop)
                } else {
                    valid(&from).and_then(|_| git(dir, &["switch", "--quiet", "-c", &name, &from], None).map(drop))
                }
            })
        }
        "git_branch_rename" => {
            let (from, to) = (s("from"), s("to"));
            valid(&to).and_then(|_| {
                if from.is_empty() {
                    git(dir, &["branch", "-m", &to], None).map(drop)
                } else {
                    valid(&from).and_then(|_| git(dir, &["branch", "-m", &from, &to], None).map(drop))
                }
            })
        }
        "git_branch_delete" => {
            let name = s("name");
            let done =
                valid(&name).and_then(|_| git(dir, &["branch", if b("force") { "-D" } else { "-d" }, &name], None));
            if done.as_ref().is_err_and(|e| e.contains("not fully merged")) {
                more.insert("unmerged".into(), json!(true));
            }
            done.map(drop)
        }
        "git_fetch" => {
            let args: &[&str] = if b("all") { &["fetch", "--quiet", "--all"] } else { &["fetch", "--quiet"] };
            git_for(dir, args, MOST_NETWORK).map(drop)
        }
        "git_pull" => pull(dir, b("rebase")),
        "git_push" | "git_sync" => {
            let pulled = if of == "git_sync" { pull(dir, b("rebase")) } else { Ok(()) };
            pulled.and_then(|_| push(dir, b("force"), m["remote"].as_str())).map(|to| {
                if let Some(to) = to {
                    more.insert("published".into(), json!(to));
                }
            })
        }
        "git_undo_commit" => undo_commit(dir).map(|message| {
            more.insert("message".into(), json!(message));
        }),
        _ => return,
    };
    let mut v = json!({"type": "git_done", "repo": repo, "op": of, "ok": done.is_ok(), "key": m["key"]});
    if let Err(e) = done {
        v["error"] = json!(e);
    }
    if b("auto") {
        v["auto"] = json!(true);
    }
    for (k, val) in more {
        v[k] = val;
    }
    send(out, v);
    // A fetch or a push ran without the index's lock: the status is read under it.
    let _one = matches!(of, "git_fetch" | "git_push").then(|| kept.lock.lock());
    push_status(kept, out, repo, dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_branch_line_says_the_branch_its_upstream_and_how_far_apart_they_are() {
        assert_eq!(
            branch_of("## main...origin/main [ahead 2, behind 1]"),
            Branch { name: Some("main".into()), upstream: Some("origin/main".into()), ahead: 2, behind: 1 }
        );
        assert_eq!(branch_of("## dev...origin/dev [gone]").upstream.as_deref(), Some("origin/dev"));
        assert_eq!(branch_of("## main"), Branch { name: Some("main".into()), ..Branch::default() });
        assert_eq!(branch_of("## No commits yet on trunk").name.as_deref(), Some("trunk"));
        assert_eq!(branch_of("## HEAD (no branch)"), Branch::default());
    }

    #[test]
    fn a_status_lists_each_file_once_with_where_a_renamed_one_was() {
        let out = b"## main\0M  a.rs\0 M b.rs\0MM c.rs\0R  new.rs\0old.rs\0?? notes.txt\0UU both.rs\0!! target/\0 D gone.rs\0";
        let (b, files) = parse_status(out);
        assert_eq!(b.name.as_deref(), Some("main"));
        let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["a.rs", "b.rs", "c.rs", "new.rs", "notes.txt", "both.rs", "gone.rs"]);
        let of = |p: &str| files.iter().find(|f| f.path == p).expect("listed").clone();
        assert_eq!(of("new.rs").from.as_deref(), Some("old.rs"));
        assert!(of("a.rs").staged() && !of("a.rs").changed());
        assert!(!of("b.rs").staged() && of("b.rs").changed());
        assert!(of("c.rs").staged() && of("c.rs").changed(), "a file changed again after staging is in both groups");
        assert!(of("notes.txt").untracked() && !of("notes.txt").staged() && !of("notes.txt").changed());
        assert!(of("both.rs").conflict() && !of("both.rs").staged() && !of("both.rs").changed());
        assert!(of("gone.rs").changed());
    }

    #[test]
    fn what_git_says_is_told_without_a_remotes_credentials() {
        assert_eq!(
            scrub("fatal: could not read from https://me:s3cret@example.com/r.git\n"),
            "fatal: could not read from https://***@example.com/r.git"
        );
        assert_eq!(scrub("ssh://git@host/r"), "ssh://***@host/r");
        assert_eq!(scrub("nothing here"), "nothing here");
    }

    #[test]
    fn only_paths_inside_the_project_are_handed_to_git() {
        assert!(path_ok("src/a.rs") && path_ok("./src") && path_ok("."));
        assert!(
            !path_ok("../x")
                && !path_ok("/etc/passwd")
                && !path_ok("a/../../b")
                && !path_ok("")
                && !path_ok(":(glob)*")
        );
        assert_eq!(from_project("sub/", "sub/a.rs"), "a.rs");
        assert_eq!(from_project("", "a.rs"), "a.rs");
        assert_eq!(from_project("sub/", "other/a.rs"), "../other/a.rs");
    }

    #[test]
    fn repositories_are_found_in_folders_and_open_files() {
        let d = tempfile::tempdir().expect("temp");
        let root = d.path();
        for dir in ["lib/.git", "apps/web/.git", "node_modules/x/.git", ".hidden/y/.git", "deep/a/b/.git", "plain/src"]
        {
            std::fs::create_dir_all(root.join(dir)).expect("dir");
        }
        // A submodule's git is a file.
        std::fs::create_dir_all(root.join("sub")).expect("dir");
        std::fs::write(root.join("sub/.git"), "gitdir: ../.git/modules/sub\n").expect("w");
        let ignored = vec!["node_modules".to_string()];
        let paths = |v: Vec<(String, String)>| v.into_iter().map(|(p, _)| p).collect::<Vec<_>>();
        assert_eq!(paths(repositories(root, true, "true", 1, &ignored, &[])), ["", "lib", "sub"]);
        assert_eq!(paths(repositories(root, false, "subFolders", 2, &ignored, &[])), ["lib", "sub", "apps/web"]);
        assert_eq!(paths(repositories(root, false, "true", -1, &[], &[])).len(), 5);
        let open = vec!["deep/a/b/c/d.rs".to_string(), "plain/src/x.rs".to_string(), "../out.rs".to_string()];
        assert_eq!(paths(repositories(root, true, "openEditors", 1, &ignored, &open)), ["", "deep/a/b"]);
        assert_eq!(paths(repositories(root, true, "false", 3, &ignored, &open)), [""]);
        let named = repositories(root, false, "true", 2, &ignored, &[]);
        assert_eq!(named[2], ("apps/web".to_string(), "web".to_string()));
    }

    #[test]
    fn at_most_a_hundred_repositories_are_found() {
        let d = tempfile::tempdir().expect("temp");
        for i in 0..120 {
            std::fs::create_dir_all(d.path().join(format!("r{i:03}/.git"))).expect("dir");
        }
        assert_eq!(repositories(d.path(), true, "true", 1, &[], &[]).len(), MOST_REPOSITORIES);
    }

    /// A repository made for a test, with a name and an address of its own; none when git is not there.
    fn repo() -> Option<tempfile::TempDir> {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("git is not installed: skipped");
            return None;
        }
        let d = tempfile::tempdir().expect("temp");
        for args in [
            &["init", "--quiet", "-b", "main"][..],
            &["config", "user.name", "Test"],
            &["config", "user.email", "test@example.com"],
            &["config", "commit.gpgsign", "false"],
        ] {
            git(d.path(), args, None).expect("git");
        }
        Some(d)
    }

    #[test]
    fn files_are_staged_unstaged_discarded_and_committed() {
        let Some(d) = repo() else { return };
        let dir = d.path();
        let (v, _) = status(dir);
        assert_eq!((v["branch"].as_str(), v["files"].as_array().map(Vec::len)), (Some("main"), Some(0)));
        std::fs::write(dir.join("a.txt"), "one\n").expect("w");
        std::fs::create_dir(dir.join("src")).expect("dir");
        std::fs::write(dir.join("src/b.txt"), "two\n").expect("w");
        // Unstaging before the first commit.
        stage(dir, &["a.txt".into(), "src".into()]).expect("stage");
        unstage(dir, &["src".into()]).expect("unstage");
        let (v, _) = status(dir);
        let x_of = |v: &Value, p: &str| {
            v["files"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|f| f["path"] == p)
                .map(|f| f["x"].as_str().unwrap_or("").to_string())
        };
        assert_eq!((x_of(&v, "a.txt").as_deref(), x_of(&v, "src/b.txt").as_deref()), (Some("A"), Some("?")));
        assert_eq!(commit(dir, "  ", false, false), Err("Type a message to commit.".into()));
        let hash = commit(dir, "First", false, false).expect("commit");
        assert!(hash.len() >= 7, "{hash}");
        // Changed, then discarded: the tracked file is as it was, the untracked one handed to delete.
        std::fs::write(dir.join("a.txt"), "changed\n").expect("w");
        let asked = std::cell::RefCell::new(Vec::new());
        discard(dir, &[".".into()], &|p| {
            asked.borrow_mut().push(p.to_string());
            true
        })
        .expect("discard");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).expect("r"), "one\n");
        assert_eq!(*asked.borrow(), ["src/b.txt"]);
        // The index and HEAD as the diff's sides have them; amending keeps one commit.
        std::fs::write(dir.join("a.txt"), "staged\n").expect("w");
        stage(dir, &["a.txt".into()]).expect("stage");
        assert_eq!(show(dir, "a.txt", "")["text"], "staged\n");
        assert_eq!(show(dir, "a.txt", "HEAD")["text"], "one\n");
        assert_eq!(show(dir, "nothing.txt", "HEAD")["exists"], false);
        assert_eq!(show(dir, "a.txt", "--output=x")["exists"], false);
        commit(dir, "First, amended", false, true).expect("amend");
        let log = git(dir, &["log", "--format=%s"], None).expect("log");
        assert_eq!(String::from_utf8_lossy(&log).trim(), "First, amended");
        // Commit all stages every change first.
        std::fs::write(dir.join("c.txt"), "new\n").expect("w");
        commit(dir, "Second", true, false).expect("all");
        let (v, _) = status(dir);
        assert_eq!(v["files"].as_array().map(Vec::len), Some(0), "{v}");
    }

    #[test]
    fn a_renamed_file_changed_again_is_staged_and_unstaged_by_its_paths() {
        let Some(d) = repo() else { return };
        let dir = d.path();
        std::fs::write(dir.join("old.rs"), "one\n").expect("w");
        commit(dir, "First", true, false).expect("commit");
        git(dir, &["mv", "old.rs", "new.rs"], None).expect("mv");
        std::fs::write(dir.join("new.rs"), "two\n").expect("w");
        let (v, _) = status(dir);
        assert_eq!((v["files"][0]["x"].as_str(), v["files"][0]["from"].as_str()), (Some("R"), Some("old.rs")), "{v}");
        stage(dir, &["new.rs".into()]).expect("the new path alone is staged");
        unstage(dir, &["new.rs".into(), "old.rs".into()]).expect("both halves are unstaged");
        let (v, _) = status(dir);
        let xy: Vec<String> = v["files"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| format!("{}{} {}", f["x"].as_str().unwrap_or(""), f["y"].as_str().unwrap_or(""), f["path"]))
            .collect();
        assert_eq!(xy, [" D \"old.rs\"", "?? \"new.rs\""]);
        #[cfg(unix)]
        {
            let out = tempfile::tempdir().expect("temp");
            std::fs::write(out.path().join("secret.txt"), "not yours").expect("w");
            std::os::unix::fs::symlink(out.path().join("secret.txt"), dir.join("link.txt")).expect("link");
            assert_eq!(show_working(dir, "link.txt")["exists"], false, "a link out of the project is not read");
            assert_eq!(show_working(dir, "new.rs")["text"], "two\n");
        }
    }

    #[test]
    fn the_projects_side_answers_and_a_discarded_untracked_file_is_kept_aside() {
        let Some(d) = repo() else { return };
        let data = tempfile::tempdir().expect("temp");
        std::fs::write(d.path().join("notes.txt"), "mine\n").expect("w");
        let ws = Arc::new(ca_workspace::Workspace::open(d.path(), &[]).expect("ws"));
        let (out, mut rx) = broadcast::channel(64);
        let side = Arc::new(Side::new(ws, Arc::default(), out, None, data.path()));
        *side.kept.of::<Git>().me.lock().expect("me") = Arc::downgrade(&side);
        let mut said = || -> Vec<Value> {
            std::iter::from_fn(|| rx.try_recv().ok()).map(|t| serde_json::from_str(&t).expect("json")).collect()
        };
        assert!(side.handle(&json!({"type": "git_status"})));
        let s = said();
        assert_eq!((s[0]["type"].as_str(), s[0]["repo"].as_str()), (Some("git_status"), Some("")));
        assert_eq!(s[0]["files"][0]["path"], "notes.txt");
        assert!(side.handle(&json!({"type": "git_discard", "repo": "", "paths": ["notes.txt"]})));
        let s = said();
        assert!(!d.path().join("notes.txt").exists());
        assert!(s.iter().any(|m| m["type"] == "deleted" && m["kept"].is_string()), "{s:?}");
        let last = s.iter().rfind(|m| m["type"] == "git_status").expect("status");
        assert_eq!(last["files"].as_array().map(Vec::len), Some(0));
        assert!(side.handle(&json!({"type": "git_stage", "paths": ["../out"]})));
        assert_eq!(said()[0]["type"], "git_failed");
        assert!(side.handle(&json!({"type": "git_status", "repo": "/etc"})));
        assert!(said()[0]["error"].is_string());
    }

    #[test]
    fn a_folder_without_a_repository_says_so_and_a_status_is_told_again_only_when_it_changed() {
        let Some(d) = repo() else { return };
        let plain = tempfile::tempdir().expect("temp");
        let (v, git_dir) = status(plain.path());
        assert!(v["root"].is_null() && git_dir.is_none() && v.get("error").is_none(), "{v}");
        let kept = Git::default();
        let (out, mut rx) = broadcast::channel(16);
        push_status(&kept, &out, "", d.path());
        assert_eq!(rx.try_recv().map(|t| t.contains("\"git_status\"")), Ok(true));
        push_status_if_new(&kept, &out, "", d.path());
        assert!(rx.try_recv().is_err(), "the same status is not told twice");
        std::fs::write(d.path().join("x.txt"), "x").expect("w");
        stage(d.path(), &["x.txt".into()]).expect("stage");
        look(&kept, &out);
        let told: Value = serde_json::from_str(&rx.try_recv().expect("told")).expect("json");
        assert_eq!(told["files"][0]["path"], "x.txt");
    }

    /// A repository with one commit, cloned from a bare one beside it (`origin`); the bare one's folder too.
    fn cloned() -> Option<(tempfile::TempDir, PathBuf, PathBuf)> {
        let d = repo()?;
        std::fs::write(d.path().join("a.txt"), "a\n").expect("w");
        commit(d.path(), "First", true, false).expect("commit");
        let bare = d.path().join("origin.git");
        let clone = d.path().join("work");
        let top = d.path().to_path_buf();
        git(&top, &["clone", "--quiet", "--bare", ".", "origin.git"], None).expect("bare");
        git(&top, &["clone", "--quiet", "origin.git", "work"], None).expect("clone");
        for args in [&["config", "user.name", "Test"][..], &["config", "user.email", "t@example.com"]] {
            git(&clone, args, None).expect("config");
        }
        Some((d, bare, clone))
    }

    fn done_of(said: &[Value]) -> &Value {
        said.iter().find(|m| m["type"] == "git_done").expect("git_done")
    }

    fn answered(dir: &Path, m: Value) -> Vec<Value> {
        let (out, mut rx) = broadcast::channel(64);
        let kept = Git::default();
        let of = m["type"].as_str().unwrap_or("").to_string();
        answer(&of, &m, "", dir, &out, &kept, None);
        let mut said = Vec::new();
        while let Ok(t) = rx.try_recv() {
            said.push(serde_json::from_str(&t).expect("json"));
        }
        said
    }

    #[test]
    fn a_line_of_the_branches_says_its_kind_upstream_and_how_far_apart() {
        let v = branch_line("*\0refs/heads/main\0abc1234\0origin/main\0ahead 2, behind 1\0Fix it").expect("one");
        assert_eq!(
            (v["name"].as_str(), v["kind"].as_str(), v["current"].as_bool()),
            (Some("main"), Some("local"), Some(true))
        );
        assert_eq!(
            (v["upstream"].as_str(), v["ahead"].as_u64(), v["behind"].as_u64()),
            (Some("origin/main"), Some(2), Some(1))
        );
        let v = branch_line(" \0refs/remotes/origin/dev\0abc\0\0\0Dev").expect("remote");
        assert_eq!(
            (v["name"].as_str(), v["kind"].as_str(), v["upstream"].is_null()),
            (Some("origin/dev"), Some("remote"), true)
        );
        assert!(branch_line(" \0refs/remotes/origin/HEAD\0abc\0\0\0x").is_none());
        assert_eq!(branch_line(" \0refs/tags/v1\0abc\0\0\0x").expect("tag")["kind"], "tag");
        assert!(!name_ok("-f") && !name_ok("") && !name_ok("a b") && name_ok("feature/x"));
    }

    #[test]
    fn a_git_that_takes_too_long_is_stopped() {
        let Some(d) = repo() else { return };
        // `git rev-parse` answers at once (and, unlike `git var GIT_EDITOR`, whatever the terminal is); a limit of
        // nothing stops even it, or lets it finish: either says so plainly.
        match git_for(d.path(), &["rev-parse", "--is-inside-work-tree"], std::time::Duration::ZERO) {
            Ok(_) => {}
            Err(e) => assert!(e.contains("took longer than 0 minutes"), "{e}"),
        }
        assert!(git_for(d.path(), &["status"], MOST_NETWORK).is_ok());
    }

    #[test]
    fn branches_are_made_renamed_switched_and_deleted_and_an_unmerged_one_is_said() {
        let Some((_d, _bare, dir)) = cloned() else { return };
        let said = answered(&dir, json!({"type": "git_branch_create", "name": "dev", "key": "k1"}));
        assert_eq!((done_of(&said)["ok"].as_bool(), done_of(&said)["key"].as_str()), (Some(true), Some("k1")));
        assert_eq!(current(&dir).0.as_deref(), Some("dev"));
        std::fs::write(dir.join("b.txt"), "b\n").expect("w");
        commit(&dir, "On dev", true, false).expect("commit");
        answered(&dir, json!({"type": "git_branch_rename", "to": "feature"}));
        assert_eq!(current(&dir).0.as_deref(), Some("feature"));
        let said = answered(&dir, json!({"type": "git_switch", "name": "main", "kind": "local"}));
        assert_eq!(done_of(&said)["ok"], true);
        assert_eq!(said.last().expect("status")["type"], "git_status");
        let said = answered(&dir, json!({"type": "git_branch_delete", "name": "feature", "force": false}));
        assert_eq!((done_of(&said)["ok"].as_bool(), done_of(&said)["unmerged"].as_bool()), (Some(false), Some(true)));
        let said = answered(&dir, json!({"type": "git_branch_delete", "name": "feature", "force": true}));
        assert_eq!(done_of(&said)["ok"], true);
        let said = answered(&dir, json!({"type": "git_branch_create", "name": "-D"}));
        assert_eq!(done_of(&said)["ok"], false);
        let listed = branches(&dir);
        let names: Vec<&str> =
            listed["items"].as_array().expect("items").iter().filter_map(|i| i["name"].as_str()).collect();
        assert!(names.contains(&"main") && names.contains(&"origin/main"), "{names:?}");
        assert_eq!(listed["remotes"], json!(["origin"]));
    }

    #[test]
    fn a_branch_is_published_pushed_fetched_pulled_and_its_last_commit_undone() {
        let Some((d, bare, dir)) = cloned() else { return };
        answered(&dir, json!({"type": "git_branch_create", "name": "dev"}));
        let said = answered(&dir, json!({"type": "git_push", "force": false}));
        assert_eq!(
            (done_of(&said)["ok"].as_bool(), done_of(&said)["published"].as_str()),
            (Some(true), Some("origin"))
        );
        assert_eq!(current(&dir).1.as_deref(), Some("origin/dev"));
        std::fs::write(dir.join("c.txt"), "c\n").expect("w");
        commit(&dir, "Third", true, false).expect("commit");
        let said = answered(&dir, json!({"type": "git_sync", "rebase": false}));
        assert_eq!(done_of(&said)["ok"], true, "{said:?}");
        assert!(git(&bare, &["rev-parse", "--verify", "--quiet", "refs/heads/dev"], None).is_ok());
        // Another clone pushes; this one fetches (behind 1), then pulls.
        let other = d.path().join("other");
        git(d.path(), &["clone", "--quiet", "-b", "dev", "origin.git", "other"], None).expect("clone");
        std::fs::write(other.join("d.txt"), "d\n").expect("w");
        git(&other, &["add", "."], None).expect("add");
        git(&other, &["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-qm", "Fourth"], None)
            .expect("c");
        git(&other, &["push", "--quiet"], None).expect("push");
        let said = answered(&dir, json!({"type": "git_fetch", "all": true, "auto": true}));
        assert_eq!((done_of(&said)["ok"].as_bool(), done_of(&said)["auto"].as_bool()), (Some(true), Some(true)));
        assert_eq!(said.last().expect("status")["behind"], 1);
        let said = answered(&dir, json!({"type": "git_pull", "rebase": true}));
        assert_eq!(done_of(&said)["ok"], true, "{said:?}");
        assert!(dir.join("d.txt").exists());
        let said = answered(&dir, json!({"type": "git_undo_commit"}));
        assert_eq!((done_of(&said)["ok"].as_bool(), done_of(&said)["message"].as_str()), (Some(true), Some("Fourth")));
        assert!(
            git(&dir, &["diff", "--cached", "--name-only"], None).map(|o| o.starts_with(b"d.txt")).unwrap_or(false)
        );
    }

    #[test]
    fn the_first_commit_is_undone_too_and_a_push_with_no_remote_says_why() {
        let Some(d) = repo() else { return };
        std::fs::write(d.path().join("a.txt"), "a\n").expect("w");
        commit(d.path(), "Only", true, false).expect("commit");
        assert_eq!(undo_commit(d.path()).as_deref(), Ok("Only"));
        assert!(!has_head(d.path()));
        commit(d.path(), "Again", true, false).expect("commit");
        let said = answered(d.path(), json!({"type": "git_push", "force": false}));
        assert_eq!(done_of(&said)["error"], "Your repository has no remotes configured to publish to.");
    }

    /// A repository where `a.txt` was changed on `main` and on `feature`, each a commit of its own.
    fn diverged() -> Option<tempfile::TempDir> {
        let d = repo()?;
        let p = d.path();
        let commit = |text: &str, msg: &str| {
            std::fs::write(p.join("a.txt"), text).expect("w");
            git(p, &["add", "a.txt"], None).expect("add");
            git(p, &["commit", "-q", "-m", msg], None).expect("commit");
        };
        commit("one\ntwo\nthree\n", "base");
        git(p, &["checkout", "-q", "-b", "feature"], None).expect("branch");
        commit("one\nTWO\nthree\n", "Shout two");
        git(p, &["checkout", "-q", "main"], None).expect("main");
        commit("one\n2\nthree\n", "Number two");
        Some(d)
    }

    #[test]
    fn a_merges_conflict_is_told_with_its_three_stages_and_completed_only_without_markers() {
        let Some(d) = diverged() else { return };
        let p = d.path();
        assert!(git(p, &["merge", "-q", "feature"], None).is_err(), "the merge stops on the conflict");
        let info = merge_info(p, "a.txt");
        assert_eq!(info["unmerged"], true, "{info}");
        assert_eq!(info["kind"], "merge");
        assert_eq!(info["current"]["name"], "main");
        assert_eq!(info["current"]["subject"], "Number two");
        assert_eq!(info["incoming"]["name"], "feature");
        assert_eq!(info["incoming"]["subject"], "Shout two");
        assert!(info["incoming"]["short"].as_str().map(str::len).unwrap_or(0) >= 7);
        assert_eq!(info["base_text"], "one\ntwo\nthree\n");
        assert_eq!(info["current_text"], "one\n2\nthree\n");
        assert_eq!(info["incoming_text"], "one\nTWO\nthree\n");
        assert_eq!(info["binary"], false);
        assert_eq!(merge_complete(p, "a.txt", false), Err("markers".into()), "the file still has its markers");
        std::fs::write(p.join("a.txt"), "one\nTWO\nthree\n").expect("w");
        merge_complete(p, "a.txt", false).expect("completed");
        let after = merge_info(p, "a.txt");
        assert_eq!(after["unmerged"], false, "staged, it is merged: {after}");
        // Asked to anyway, a file with its markers is staged as it is.
        git(p, &["checkout", "-m", "--", "a.txt"], None).expect("the conflict made again");
        assert_eq!(merge_info(p, "a.txt")["unmerged"], true);
        merge_complete(p, "a.txt", true).expect("completed with conflicts");
        assert_eq!(merge_info(p, "a.txt")["unmerged"], false);
        assert!(has_markers(&std::fs::read_to_string(p.join("a.txt")).expect("r")));
        assert_eq!(merge_complete(p, "../out.txt", true).map_err(|e| e.contains("not a path")), Err(true));
        assert!(!has_markers("a\n=======\nb\n"), "a lone ======= is text");
        assert!(has_markers("<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\n"));
    }

    #[test]
    fn a_rebase_and_a_cherry_pick_say_what_is_under_way_and_whose_side_is_whose() {
        let Some(d) = diverged() else { return };
        let p = d.path();
        let main =
            String::from_utf8_lossy(&git(p, &["rev-parse", "--short", "main"], None).expect("rev")).trim().to_string();
        git(p, &["checkout", "-q", "feature"], None).expect("feature");
        assert!(git(p, &["rebase", "-q", "main"], None).is_err(), "the rebase stops on the conflict");
        let info = merge_info(p, "a.txt");
        assert_eq!(info["kind"], "rebase", "{info}");
        assert_eq!(info["incoming"]["name"], "feature", "the branch being rebased comes in");
        assert_eq!(info["incoming"]["subject"], "Shout two");
        assert_eq!(info["current"]["short"], main.as_str(), "the commit it is put on is the current side");
        assert_eq!(info["current_text"], "one\n2\nthree\n");
        git(p, &["rebase", "--abort"], None).expect("abort");
        git(p, &["checkout", "-q", "main"], None).expect("main");
        assert!(git(p, &["cherry-pick", "feature"], None).is_err(), "the cherry-pick stops on the conflict");
        let info = merge_info(p, "a.txt");
        assert_eq!(info["kind"], "cherry-pick", "{info}");
        assert_eq!(info["incoming"]["subject"], "Shout two");
        assert_eq!(info["current"]["name"], "main");
    }
}
