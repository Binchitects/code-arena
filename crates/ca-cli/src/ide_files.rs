//! Coder mode's side of the IDE's stream: the project's files (a folder listed, a file opened and saved) and its
//! terminals. They answer while a turn runs too. Paths come relative to the project only, each part resolved as the
//! system resolves it (a link that leads out is refused); only plain files are read or written, never a pipe or a
//! device, and not a binary file or one over 5 MB. The Explorer's own operations are here too: a file or a folder
//! made, moved, or deleted (what is deleted is first kept aside, where the person can get it back).
//!
//! An area of the IDE that needs the project's files only (`ide_ext`'s `Rule::Side`) answers through this too:
//! [`Side::handle`] asks the areas first, and they resolve a page's path with [`Side::real`] or [`Side::entry`],
//! name one with [`Side::shown`], and keep what belongs to the folder in [`Side::kept`].

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::broadcast;

/// The project's own messages (`ide_ext`'s turn rules): all answered at once, a turn running or not.
pub const MESSAGES: &[(&str, crate::ide_ext::Rule)] = {
    use crate::ide_ext::Rule::Side;
    &[
        ("ls", Side),
        ("open", Side),
        ("save", Side),
        ("create", Side),
        ("rename", Side),
        ("delete", Side),
        ("stat", Side),
        ("watch", Side),
        ("backup", Side),
        ("backup_drop", Side),
        ("backups", Side),
        ("changes", Side),
        ("files", Side),
        ("search", Side),
        ("replace", Side),
        ("term_open", Side),
        ("term_in", Side),
        ("term_resize", Side),
        ("terms", Side),
        ("term_close", Side),
        ("copy", Side),
        ("import", Side),
        ("restore_deleted", Side),
        ("changes_accept", Side),
        ("changes_revert", Side),
        ("changes_accept_text", Side),
    ]
};

/// The largest file the editor opens.
const MOST_BYTES: u64 = 5 * 1024 * 1024;

/// The most entries a folder lists.
const MOST_ENTRIES: usize = 5000;

struct Term {
    pty: ca_pty::Pty,
    writer: Box<dyn Write + Send>,
    /// Its last output, for a page that comes back (a reload) to draw it again.
    screen: Arc<Mutex<String>>,
    /// What started it, for one the driver made (a task, a run, a debuggee): its tab's title and its kind.
    made: Option<(String, String)>,
}

/// The most terminals of the person's running at once (the driver's own are not counted).
const MOST_TERMS: usize = 10;

/// The numbers of the terminals the driver makes, apart from the faces' own.
pub const FIRST_DRIVER_TERM: u64 = 1_000_000;

static NEXT_DRIVER_TERM: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(FIRST_DRIVER_TERM);

/// What a terminal's shell is not to see of the driver's environment.
const NOT_IN_TERMS: &[&str] = &["ARENA_API_KEY"];

/// The shell a terminal runs: the one asked for (a path, or a name on `PATH`), else `$SHELL`, else bash, else sh.
pub(crate) fn shell_for(asked: Option<&str>, env_shell: Option<&Path>, path: Option<&str>) -> PathBuf {
    let found = |name: &str| -> Option<PathBuf> {
        let p = Path::new(name);
        if p.is_absolute() {
            return p.is_file().then(|| p.to_path_buf());
        }
        if name.is_empty() || name.contains('/') {
            return None;
        }
        std::env::split_paths(path?).map(|d| d.join(name)).find(|c| c.is_file())
    };
    asked
        .map(str::trim)
        .and_then(found)
        .or_else(|| env_shell.filter(|p| p.is_file()).map(Path::to_path_buf))
        .or_else(|| found("bash"))
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }))
}

/// The name of the shell a terminal opened without asking for one runs ("bash", "zsh"), for the faces' tabs: the
/// one [`shell_for`] finds, so the tab says what runs even when `$SHELL` is not set.
pub(crate) fn default_shell_name(env_shell: Option<&str>) -> String {
    let path = std::env::var_os("PATH").map(|p| p.to_string_lossy().into_owned());
    let env_shell = env_shell.map(Path::new).filter(|p| p.is_absolute());
    shell_name_of(&shell_for(None, env_shell, path.as_deref()))
}

fn shell_name_of(shell: &Path) -> String {
    shell.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "sh".into())
}

/// How much of a terminal's output is kept for a page that comes back.
const SCREEN_KEPT: usize = 256 * 1024;

fn keep_screen(screen: &Mutex<String>, text: &str) {
    if let Ok(mut s) = screen.lock() {
        s.push_str(text);
        if s.len() > SCREEN_KEPT {
            let mut cut = s.len() - SCREEN_KEPT;
            while !s.is_char_boundary(cut) {
                cut += 1;
            }
            s.drain(..cut);
        }
    }
}

/// What Coder mode needs, apart from the session: the workspace, the terminals, the stream.
#[derive(Clone)]
pub struct Side {
    ws: Arc<ca_workspace::Workspace>,
    checkpoints: Arc<ca_tools::checkpoints::Checkpoints>,
    out: broadcast::Sender<String>,
    terms: Arc<Mutex<HashMap<u64, Term>>>,
    /// `$SHELL`, when it is a path.
    shell: Option<PathBuf>,
    watch: Arc<Mutex<Watch>>,
    /// Where the unsaved changes of this project's files are kept (only the person may read them).
    backups: PathBuf,
    /// What the areas keep for this folder (their terminals, their servers): new with another folder.
    #[allow(dead_code)] // The areas' own: none keeps anything yet.
    pub kept: crate::ide_ext::Kept,
}

/// What the page shows, watched: each open file's and each open folder's last change (`None`: gone, or not known yet).
#[derive(Default)]
struct Watch {
    files: HashMap<String, Option<u64>>,
    folders: HashMap<String, Option<u64>>,
}

/// How often what is watched is looked at.
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// The unsaved changes kept per file, and per project, at most.
const MOST_BACKUP: usize = 5 * 1024 * 1024;
const MOST_BACKUPS: u64 = 50 * 1024 * 1024;

/// The biggest folder that is deleted (it is kept aside first, and a bigger one is not worth keeping twice).
const MOST_DELETED: u64 = 50 * 1024 * 1024;

/// How big a file or a folder's tree is; a link counts as itself, never as what it leads to. Counted no further
/// than past `most` (a huge folder is not walked whole to be refused).
fn tree_size(p: &Path, most: u64) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(p) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    let mut total = 0;
    for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
        total += tree_size(&e.path(), most.saturating_sub(total));
        if total > most {
            break;
        }
    }
    total
}

/// A file or a folder's tree copied; a link is copied as a link (never what it leads to).
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)?;
        return Ok(());
    }
    if !meta.is_dir() {
        return std::fs::copy(from, to).map(|_| ());
    }
    std::fs::create_dir(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        copy_tree(&e.path(), &to.join(e.file_name()))?;
    }
    Ok(())
}

/// Takes a file, a link or a folder's tree away (a link itself, not what it leads to).
fn remove_tree(p: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(p)?.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }
}

/// Moves a file or a folder's tree to another place: renamed when both are on one disk, else copied and then taken
/// away.
fn move_aside(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_then_remove(from, to)
}

/// Nothing is taken away unless the whole copy was made, and no half of a copy is left.
fn copy_then_remove(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Err(e) = copy_tree(from, to) {
        let _ = remove_tree(to);
        return Err(e);
    }
    remove_tree(from)
}

/// The folder a path is in, as the page names it (the project's top: empty).
fn parent_of(shown: &str) -> String {
    shown.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default()
}

/// A short, stable name for a text (FNV-1a): a backup's file name, a project's folder.
fn name_of(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn stamp(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| modified(&m))
}

/// Writes a file only its owner may read.
fn write_private(file: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(file)?;
        f.write_all(text.as_bytes())
    }
    #[cfg(not(unix))]
    std::fs::write(file, text)
}

fn send(out: &broadcast::Sender<String>, v: Value) {
    let _ = out.send(v.to_string());
}

pub(crate) fn modified(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Side {
    pub fn new(
        ws: Arc<ca_workspace::Workspace>,
        checkpoints: Arc<ca_tools::checkpoints::Checkpoints>,
        out: broadcast::Sender<String>,
        shell: Option<&str>,
        data: &Path,
    ) -> Side {
        let shell = shell.map(PathBuf::from).filter(|p| p.is_absolute());
        let backups = data.join("ide-backups").join(name_of(&ws.root().to_string_lossy()));
        Side {
            ws,
            checkpoints,
            out,
            terms: Arc::default(),
            shell,
            watch: Arc::default(),
            backups,
            kept: Default::default(),
        }
    }

    /// To every face.
    #[allow(dead_code)] // For the areas that answer from the project's side.
    pub fn out(&self) -> &broadcast::Sender<String> {
        &self.out
    }

    /// The project's folders.
    #[allow(dead_code)] // For the areas that answer from the project's side.
    pub fn workspace(&self) -> &Arc<ca_workspace::Workspace> {
        &self.ws
    }

    /// What the agent's files were before it changed them.
    #[allow(dead_code)] // For the areas that answer from the project's side.
    pub fn checkpoints(&self) -> &Arc<ca_tools::checkpoints::Checkpoints> {
        &self.checkpoints
    }

    /// The real path a page's path names: relative to the project, inside it.
    pub fn real(&self, rel: &str) -> Result<PathBuf, String> {
        let rel = rel.trim_start_matches("./");
        let given = Path::new(rel);
        if given.is_absolute() || rel.starts_with('/') || rel.starts_with('\\') || rel.contains(':') {
            return Err("only a path inside the project, from its top".into());
        }
        let root = self.ws.root();
        let joined = if rel.is_empty() { root.to_path_buf() } else { root.join(given) };
        let real = self.ws.resolve(&joined.to_string_lossy()).map_err(|e| e.to_string())?;
        if !real.starts_with(root) {
            return Err("it leads out of the project".into());
        }
        Ok(real)
    }

    /// The real place of an entry itself, for making, moving or deleting it: its folder resolved as the system
    /// resolves it and inside the project, its own name kept as it is (a link is the link, not where it leads).
    /// Never the project's top, and never git's own folder.
    #[allow(dead_code)] // The areas' too.
    pub fn entry(&self, rel: &str) -> Result<PathBuf, String> {
        let rel = rel.trim_start_matches("./").trim_end_matches('/');
        if rel.starts_with(['/', '\\']) || rel.contains(':') {
            return Err("only a path inside the project, from its top".into());
        }
        let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
        if name.is_empty() || name == "." || name == ".." || name.contains(['\\', '\0']) {
            return Err("not the name of a file or a folder".into());
        }
        let real = self.real(dir)?.join(name);
        if real.strip_prefix(self.ws.root()).is_ok_and(|r| r.components().any(|c| c.as_os_str() == ".git")) {
            return Err("git's own folder is left as it is".into());
        }
        Ok(real)
    }

    /// The message, then each folder changed listed again (every face's tree shows it).
    fn answer(&self, (v, folders): (Value, Vec<String>)) {
        send(&self.out, v);
        for f in folders {
            send(&self.out, self.ls(&f));
        }
    }

    /// An empty file, or a folder, made (with the folders on the way to it); not over what is there.
    fn create(&self, path: &str, dir: bool) -> (Value, Vec<String>) {
        let fail = |e: String| (json!({"type": "created", "path": path, "dir": dir, "error": e}), Vec::new());
        let real = match self.entry(path) {
            Ok(r) => r,
            Err(e) => return fail(e),
        };
        let Some(parent) = real.parent() else { return fail("no folder above it".into()) };
        // The first folder on the way that is there already: the one whose listing changes when folders are made.
        let mut top = parent;
        while !top.exists() {
            match top.parent() {
                Some(p) => top = p,
                None => break,
            }
        }
        let made = std::fs::create_dir_all(parent).and_then(|()| {
            if dir {
                std::fs::create_dir(&real)
            } else {
                std::fs::OpenOptions::new().write(true).create_new(true).open(&real).map(|_| ())
            }
        });
        if let Err(e) = made {
            return fail(match e.kind() {
                std::io::ErrorKind::AlreadyExists => "a file or folder of that name is already there".into(),
                _ => e.to_string(),
            });
        }
        let shown = self.shown(&real);
        let mut folders = vec![self.shown(top), parent_of(&shown)];
        folders.dedup();
        (json!({"type": "created", "path": shown, "dir": dir}), folders)
    }

    /// A file or a folder moved inside the project; not over what is there.
    fn rename(&self, path: &str, to: &str) -> (Value, Vec<String>) {
        let fail = |e: String| (json!({"type": "renamed", "path": path, "to": to, "error": e}), Vec::new());
        let (from, dest) = match (self.entry(path), self.entry(to)) {
            (Ok(f), Ok(d)) => (f, d),
            (Err(e), _) | (_, Err(e)) => return fail(e),
        };
        if std::fs::symlink_metadata(&from).is_err() {
            return fail("not there".into());
        }
        if std::fs::symlink_metadata(&dest).is_ok() {
            return fail("a file or folder of that name is already there".into());
        }
        if dest.starts_with(&from) {
            return fail("a folder is not moved into itself".into());
        }
        let moved = match dest.parent() {
            Some(p) => std::fs::create_dir_all(p).and_then(|()| std::fs::rename(&from, &dest)),
            None => std::fs::rename(&from, &dest),
        };
        if let Err(e) = moved {
            return fail(e.to_string());
        }
        let (was, now) = (self.shown(&from), self.shown(&dest));
        // Its unsaved changes are kept under its name, and say it: they follow it, under its new one.
        let old = self.backups.join(format!("{}.json", name_of(&was)));
        if let Some(mut kept) = std::fs::read_to_string(&old).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
        {
            kept["path"] = json!(now);
            let new = self.backups.join(format!("{}.json", name_of(&now)));
            if write_private(&new, &kept.to_string()).is_ok() {
                let _ = std::fs::remove_file(&old);
            }
        }
        let mut folders = vec![parent_of(&was), parent_of(&now)];
        folders.dedup();
        (json!({"type": "renamed", "path": was, "to": now}), folders)
    }

    /// Where deleted files are kept: a folder of their own among the backups (the unsaved changes are the files at
    /// its top), only the person's to read.
    fn deleted(&self) -> PathBuf {
        self.backups.join("deleted")
    }

    /// A file, a link or a folder's tree deleted: moved out of the project into the backups, under the moment it
    /// was deleted and its own path, so the person can get it back. A folder over 50 MB is refused.
    fn delete(&self, path: &str) -> (Value, Vec<String>) {
        let fail = |e: String| (json!({"type": "deleted", "path": path, "error": e}), Vec::new());
        let real = match self.entry(path) {
            Ok(r) => r,
            Err(e) => return fail(e),
        };
        let Ok(meta) = std::fs::symlink_metadata(&real) else { return fail("not there".into()) };
        if meta.is_dir() && tree_size(&real, MOST_DELETED) > MOST_DELETED {
            return fail("a folder bigger than 50 MB is not kept aside: delete it outside the editor".into());
        }
        let shown = self.shown(&real);
        let kept = match self.keep_aside(&real, &shown) {
            Ok(k) => k,
            Err(e) => return fail(format!("it could not be kept aside, so it was not deleted: {e}")),
        };
        let _ = std::fs::remove_file(self.backups.join(format!("{}.json", name_of(&shown))));
        (json!({"type": "deleted", "path": shown, "kept": kept.to_string_lossy()}), vec![parent_of(&shown)])
    }

    /// Moves what is deleted into the backups; says where it is now.
    fn keep_aside(&self, real: &Path, shown: &str) -> std::io::Result<PathBuf> {
        let all = self.deleted();
        std::fs::create_dir_all(&all)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&all, std::fs::Permissions::from_mode(0o700))?;
        }
        let mut at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // A folder of its own for each deletion: made anew, so two at one moment do not meet.
        let dir = loop {
            let dir = all.join(at.to_string());
            match std::fs::create_dir(&dir) {
                Ok(()) => break dir,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => at += 1,
                Err(e) => return Err(e),
            }
        };
        let kept = dir.join(shown);
        if let Some(p) = kept.parent() {
            std::fs::create_dir_all(p)?;
        }
        move_aside(real, &kept)?;
        Ok(kept)
    }

    pub fn shown(&self, real: &Path) -> String {
        real.strip_prefix(self.ws.root()).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default()
    }

    /// A message of Coder mode's, answered; false when it is none of them. An area's file-side message first.
    pub fn handle(&self, m: &Value) -> bool {
        if crate::ide_ext::side(self, m) {
            return true;
        }
        let path = m["path"].as_str().unwrap_or("");
        match m["type"].as_str().unwrap_or("") {
            "ls" => send(&self.out, self.ls(path)),
            "open" => send(&self.out, self.open(path)),
            "save" => send(&self.out, self.save(path, m["text"].as_str().unwrap_or(""))),
            "create" => self.answer(self.create(path, m["dir"] == true)),
            "rename" => self.answer(self.rename(path, m["to"].as_str().unwrap_or(""))),
            "delete" => self.answer(self.delete(path)),
            "stat" => send(&self.out, self.stat(m)),
            "watch" => self.set_watch(m),
            "backup" => {
                let r = self.backup(path, m["text"].as_str().unwrap_or(""));
                if let Err(e) = r {
                    send(
                        &self.out,
                        json!({"type": "notice", "text": format!("Your changes to {path} could not be kept aside: {e}")}),
                    );
                }
            }
            "backup_drop" => {
                let _ = std::fs::remove_file(self.backups.join(format!("{}.json", name_of(path))));
            }
            "backups" => send(&self.out, self.list_backups()),
            "changes" => send(&self.out, self.changes()),
            "files" => {
                let (root, out) = (self.ws.root().to_path_buf(), self.out.clone());
                tokio::task::spawn_blocking(move || send(&out, files(&root)));
            }
            "search" => {
                let (root, out) = (self.ws.root().to_path_buf(), self.out.clone());
                let ask = Ask::of(m);
                tokio::task::spawn_blocking(move || send(&out, search(&root, &ask)));
            }
            "replace" => self.replace_in_files(m),
            "term_open" => self.term_open(m),
            "term_in" => {
                let id = m["id"].as_u64().unwrap_or(0);
                if let Ok(mut t) = self.terms.lock()
                    && let Some(term) = t.get_mut(&id)
                {
                    let _ = term.writer.write_all(m["data"].as_str().unwrap_or("").as_bytes());
                    let _ = term.writer.flush();
                }
            }
            "term_resize" => {
                let id = m["id"].as_u64().unwrap_or(0);
                if let Ok(t) = self.terms.lock()
                    && let Some(term) = t.get(&id)
                {
                    let _ = term
                        .pty
                        .resize(m["cols"].as_u64().unwrap_or(80) as u16, m["rows"].as_u64().unwrap_or(24) as u16);
                }
            }
            // The terminals running, with what they last showed: a page that comes back takes them up.
            "terms" => {
                let list: Vec<Value> = self
                    .terms
                    .lock()
                    .map(|t| {
                        let mut ids: Vec<&u64> = t.keys().collect();
                        ids.sort();
                        ids.into_iter()
                            .map(|id| {
                                let term = &t[id];
                                let screen = term.screen.lock().map(|s| s.clone()).unwrap_or_default();
                                let mut v = json!({"id": id, "screen": screen});
                                if let Some((title, kind)) = &term.made {
                                    v["title"] = json!(title);
                                    v["kind"] = json!(kind);
                                }
                                v
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                send(&self.out, json!({"type": "terms", "terms": list}));
            }
            "term_close" => {
                let id = m["id"].as_u64().unwrap_or(0);
                if let Ok(mut t) = self.terms.lock() {
                    t.remove(&id);
                }
            }
            // Copied, brought in or put back off the loop: a big tree takes a while.
            "copy" => {
                let (me, m) = (self.clone(), m.clone());
                tokio::task::spawn_blocking(move || {
                    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
                    me.answer(me.copy(&s("path"), &s("to")))
                });
            }
            "import" => {
                let (me, m) = (self.clone(), m.clone());
                tokio::task::spawn_blocking(move || {
                    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
                    me.answer(me.import(&s("from"), &s("to"), m["replace"] == true))
                });
            }
            "restore_deleted" => {
                let (me, m) = (self.clone(), m.clone());
                tokio::task::spawn_blocking(move || {
                    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
                    me.answer(me.restore(&s("kept"), &s("to")))
                });
            }
            "changes_accept" | "changes_revert" | "changes_accept_text" => self.review_change(m),
            _ => return false,
        }
        true
    }

    fn ls(&self, path: &str) -> Value {
        let real = match self.real(path) {
            Ok(r) => r,
            Err(e) => return json!({"type": "ls", "path": path, "error": e}),
        };
        let Ok(rd) = std::fs::read_dir(&real) else {
            return json!({"type": "ls", "path": path, "error": "not a folder that can be read"});
        };
        let mut entries: Vec<(bool, String, bool)> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name == ".git" {
                    return None;
                }
                let link = e.file_type().ok().is_some_and(|t| t.is_symlink());
                let dir = std::fs::metadata(e.path()).map(|m| m.is_dir()).unwrap_or(false);
                Some((dir, name, link))
            })
            .take(MOST_ENTRIES)
            .collect();
        entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
        json!({
            "type": "ls",
            "path": self.shown(&real),
            "entries": entries.iter().map(|(dir, name, link)| json!({"name": name, "dir": dir, "link": link})).collect::<Vec<_>>(),
        })
    }

    fn open(&self, path: &str) -> Value {
        // `kind` says why it is not opened as text: too big, not text, or another reason.
        let fail_as = |e: &str, kind: &str| json!({"type": "file", "path": path, "error": e, "kind": kind});
        let fail = |e: &str| fail_as(e, "other");
        let real = match self.real(path) {
            Ok(r) => r,
            Err(e) => return fail(&e),
        };
        let Ok(meta) = std::fs::metadata(&real) else { return fail("not there") };
        if !meta.is_file() {
            return fail("not a plain file (a folder, a pipe or a device is not opened)");
        }
        if meta.len() > MOST_BYTES {
            return fail_as("bigger than 5 MB: open it with another program", "too_large");
        }
        let mut bytes = Vec::new();
        if std::fs::File::open(&real).and_then(|mut f| f.read_to_end(&mut bytes)).is_err() {
            return fail("it could not be read");
        }
        if bytes.iter().take(8000).any(|b| *b == 0) {
            return fail_as("a binary file: open it with another program", "binary");
        }
        let Ok(text) = String::from_utf8(bytes) else { return fail_as("not UTF-8 text", "binary") };
        json!({"type": "file", "path": self.shown(&real), "text": text, "modified": modified(&meta)})
    }

    fn save(&self, path: &str, text: &str) -> Value {
        let fail = |e: &str| json!({"type": "saved", "path": path, "error": e});
        let real = match self.real(path) {
            Ok(r) => r,
            Err(e) => return fail(&e),
        };
        if let Ok(meta) = std::fs::metadata(&real)
            && !meta.is_file()
        {
            return fail("not a plain file");
        }
        // A file saved into a folder not made yet (a code block saved from the chat as new/dir/a.rs): its folders
        // are made first, all inside the project as the path was checked.
        if let Some(dir) = real.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            return fail(&e.to_string());
        }
        if let Err(e) = std::fs::write(&real, text) {
            return fail(&e.to_string());
        }
        let m = std::fs::metadata(&real).map(|m| modified(&m)).unwrap_or(0);
        let shown = self.shown(&real);
        // The watch knows this change already: it is the person's own, not one from outside.
        if let Ok(mut w) = self.watch.lock()
            && let Some(known) = w.files.get_mut(&shown)
        {
            *known = Some(m);
        }
        let _ = std::fs::remove_file(self.backups.join(format!("{}.json", name_of(&shown))));
        json!({"type": "saved", "path": shown, "modified": m})
    }

    /// What to watch now: the open files and the open folders (those no longer listed stop being watched).
    fn set_watch(&self, m: &Value) {
        let list = |k: &str| -> Vec<String> {
            m[k].as_array().into_iter().flatten().filter_map(Value::as_str).take(2000).map(str::to_string).collect()
        };
        let (files, folders) = (list("files"), list("folders"));
        let Ok(mut w) = self.watch.lock() else { return };
        let mut next = Watch::default();
        for f in files {
            let known = w.files.remove(&f).unwrap_or_else(|| self.real(&f).ok().and_then(|r| stamp(&r)));
            next.files.insert(f, known);
        }
        for d in folders {
            let known = w.folders.remove(&d).unwrap_or_else(|| self.real(&d).ok().and_then(|r| stamp(&r)));
            next.folders.insert(d, known);
        }
        *w = next;
    }

    /// Looks at what is watched once a second, and tells the page what changed: a file's new stamp (or that it is
    /// gone), a folder listed again. Only what the page shows is looked at, so it costs little whatever the project.
    pub fn start_watching(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(WATCH_EVERY).await;
                let Some(s) = me.upgrade() else { break };
                s.look();
            }
        });
    }

    /// One look at what is watched: what changed is told, and becomes what is known.
    fn look(&self) {
        let (files, folders): (Vec<_>, Vec<_>) = match self.watch.lock() {
            Ok(w) => (
                w.files.iter().map(|(k, v)| (k.clone(), *v)).collect(),
                w.folders.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            ),
            Err(_) => return,
        };
        let now_of = |p: &str| self.real(p).ok().and_then(|r| stamp(&r));
        let changed_files: Vec<(String, Option<u64>)> = files
            .into_iter()
            .filter_map(|(f, known)| {
                let now = now_of(&f);
                (now != known).then_some((f, now))
            })
            .collect();
        let changed_folders: Vec<(String, Option<u64>)> = folders
            .into_iter()
            .filter_map(|(d, known)| {
                let now = now_of(&d);
                (now != known).then_some((d, now))
            })
            .collect();
        if let Ok(mut w) = self.watch.lock() {
            for (f, now) in &changed_files {
                if let Some(k) = w.files.get_mut(f) {
                    *k = *now;
                }
            }
            for (d, now) in &changed_folders {
                if let Some(k) = w.folders.get_mut(d) {
                    *k = *now;
                }
            }
        }
        if !changed_files.is_empty() {
            let items: Vec<Value> = changed_files
                .iter()
                .map(|(f, now)| match now {
                    Some(m) => json!({"path": f, "modified": m}),
                    None => json!({"path": f, "gone": true}),
                })
                .collect();
            send(&self.out, json!({"type": "stats", "items": items}));
        }
        for (d, _) in changed_folders {
            send(&self.out, self.ls(&d));
        }
    }

    /// Unsaved changes of a file kept aside (the page sends them a second after typing stops).
    fn backup(&self, path: &str, text: &str) -> Result<(), String> {
        if text.len() > MOST_BACKUP {
            return Err("bigger than 5 MB".into());
        }
        let shown = self.shown(&self.real(path)?);
        let file = self.backups.join(format!("{}.json", name_of(&shown)));
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        write_private(&file, &json!({"path": shown, "text": text, "at": at}).to_string()).map_err(|e| e.to_string())?;
        // The project's backups kept under 50 MB: the oldest go first.
        let mut all: Vec<(std::time::SystemTime, u64, PathBuf)> = std::fs::read_dir(&self.backups)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((m.modified().ok()?, m.len(), e.path()))
            })
            .collect();
        all.sort();
        let mut total: u64 = all.iter().map(|(_, n, _)| n).sum();
        for (_, n, p) in all {
            if total <= MOST_BACKUPS {
                break;
            }
            let _ = std::fs::remove_file(&p);
            total -= n;
        }
        Ok(())
    }

    fn list_backups(&self) -> Value {
        let items: Vec<Value> = std::fs::read_dir(&self.backups)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| serde_json::from_str::<Value>(&std::fs::read_to_string(e.path()).ok()?).ok())
            .collect();
        json!({"type": "backups", "items": items})
    }

    /// When each file was last changed (or that it is gone): open tabs are checked so when the window comes back.
    fn stat(&self, m: &Value) -> Value {
        let items: Vec<Value> = m["paths"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(500)
            .map(|p| match self.real(p).ok().and_then(|r| std::fs::metadata(r).ok()) {
                Some(meta) => json!({"path": p, "modified": modified(&meta)}),
                None => json!({"path": p, "gone": true}),
            })
            .collect();
        json!({"type": "stats", "items": items})
    }

    /// `replace{query, regex, case, word, include, exclude, replacement, paths?, matches?}`: the matches replaced
    /// in `paths` (the files searched when none), or only the `matches` given ({path, line, col}); answered with
    /// `replaced{files: [{path, count}], errors: [{path, error}]}` and then the files' `stats`, so an editor
    /// showing one without changes takes the new text in.
    fn replace_in_files(&self, m: &Value) {
        let ask = Ask::of(m);
        let with = m["replacement"].as_str().unwrap_or("").to_string();
        let mut only: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        for h in m["matches"].as_array().into_iter().flatten() {
            let (line, col) = (h["line"].as_u64().unwrap_or(0) as usize, h["col"].as_u64().unwrap_or(0) as usize);
            only.entry(h["path"].as_str().unwrap_or("").to_string()).or_default().push((line, col));
        }
        let mut named: Vec<String> =
            m["paths"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
        if named.is_empty() && !only.is_empty() {
            named = only.keys().cloned().collect();
            named.sort();
        }
        let mut refused = Vec::new();
        let files: Option<Vec<(String, PathBuf)>> = (!named.is_empty()).then(|| {
            named
                .iter()
                .filter_map(|p| match self.real(p) {
                    Ok(r) => Some((p.clone(), r)),
                    Err(e) => {
                        refused.push(json!({"path": p, "error": e}));
                        None
                    }
                })
                .collect()
        });
        let (root, out) = (self.ws.root().to_path_buf(), self.out.clone());
        tokio::task::spawn_blocking(move || {
            let files = match files.map(Ok).unwrap_or_else(|| searched(&root, &ask)) {
                Ok(f) => f,
                Err(e) => {
                    let mut reply = ask.keys("replaced");
                    reply["files"] = json!([]);
                    reply["errors"] = json!([{"path": "", "error": e}]);
                    return send(&out, reply);
                }
            };
            let mut reply = replace(&ask, &with, files, &only);
            if let Some(errors) = reply["errors"].as_array_mut() {
                errors.extend(refused);
            }
            let items: Vec<Value> = reply["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f["path"].as_str())
                .map(|p| match std::fs::metadata(root.join(p)) {
                    Ok(meta) => json!({"path": p, "modified": modified(&meta)}),
                    Err(_) => json!({"path": p, "gone": true}),
                })
                .collect();
            send(&out, reply);
            if !items.is_empty() {
                send(&out, json!({"type": "stats", "items": items}));
            }
        });
    }

    /// The files the agent changed in this run: each as it was before and as it is now (text only).
    fn changes(&self) -> Value {
        let text = |b: Option<Vec<u8>>| -> Option<String> {
            let b = b?;
            if b.len() as u64 > MOST_BYTES || b.iter().take(8000).any(|x| *x == 0) {
                return Some("(not shown: a binary file, or one over 5 MB)".into());
            }
            String::from_utf8(b).ok()
        };
        let files: Vec<Value> = self
            .checkpoints
            .originals()
            .into_iter()
            .filter(|(p, _)| p.starts_with(self.ws.root()))
            .filter_map(|(p, before)| {
                let now = std::fs::read(&p).ok();
                if now == before {
                    return None;
                }
                Some(json!({"path": self.shown(&p), "before": text(before), "after": text(now)}))
            })
            .collect();
        json!({"type": "changes", "files": files})
    }

    /// The agent's changes to a file accepted (none named: every file's), a part of them accepted (its `text` the
    /// file is compared with from now on), or reverted; answered with the changes as they are then, carrying the path
    /// asked about, and for a revert the file's stat and its folder listed again. Only a file the agent wrote in this
    /// run is touched.
    fn review_change(&self, m: &Value) {
        let path = m["path"].as_str().unwrap_or("");
        let kind = m["type"].as_str().unwrap_or("");
        let written = (!path.is_empty())
            .then(|| self.checkpoints.originals().into_iter().map(|(p, _)| p).find(|p| self.shown(p) == path))
            .flatten();
        let mut error: Option<String> = None;
        let mut reverted = false;
        match (kind, &written) {
            ("changes_accept", _) if path.is_empty() => self.checkpoints.accept_all(),
            ("changes_accept", Some(p)) => {
                self.checkpoints.accept(p);
            }
            ("changes_accept_text", Some(p)) => {
                self.checkpoints.set_original(p, Some(m["text"].as_str().unwrap_or("").as_bytes().to_vec()));
            }
            ("changes_revert", Some(p)) => match self.checkpoints.revert(p) {
                Some(r) if r.failed.is_empty() => reverted = true,
                Some(r) => error = r.failed.into_iter().next().map(|(_, e)| e),
                None => error = Some("the agent did not change it".into()),
            },
            _ => error = Some("the agent did not change it in this session".into()),
        }
        if let Some(e) = &error {
            let what = if kind == "changes_revert" { "reverted" } else { "accepted" };
            send(
                &self.out,
                json!({"type": "notice", "text": format!("The agent's changes to {path} could not be {what}: {e}")}),
            );
        }
        let mut v = self.changes();
        v["path"] = json!(path);
        send(&self.out, v);
        if reverted {
            send(&self.out, self.stat(&json!({"paths": [path]})));
            let folder = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            send(&self.out, self.ls(folder));
        }
    }

    fn term_open(&self, m: &Value) {
        let id = m["id"].as_u64().unwrap_or(0);
        let cols = m["cols"].as_u64().unwrap_or(80) as u16;
        let rows = m["rows"].as_u64().unwrap_or(24) as u16;
        let mine = self.terms.lock().map(|t| t.values().filter(|t| t.made.is_none()).count()).unwrap_or(0);
        if mine >= MOST_TERMS && !self.terms.lock().is_ok_and(|t| t.contains_key(&id)) {
            let error = format!("At most {MOST_TERMS} terminals run at once: kill one to start another.");
            send(&self.out, json!({"type": "term_exit", "id": id, "error": error}));
            return;
        }
        // In the folder asked for (the Explorer's Open in Integrated Terminal), when it is one of the project's.
        let asked = m["cwd"].as_str().and_then(|c| self.real(c).ok()).filter(|p| p.is_dir());
        let cwd = asked.as_deref().unwrap_or(self.ws.cwd()).to_path_buf();
        let path = std::env::var_os("PATH").map(|p| p.to_string_lossy().into_owned());
        let shell = shell_for(m["shell"].as_str(), self.shell.as_deref(), path.as_deref());
        self.term_spawn(id, &shell, &[], &cwd, (cols, rows), None);
    }

    /// A terminal the driver starts itself (a task, a run, a debuggee: `kind` is "task", "run" or "debug"): every
    /// face is told with `term_started{id, title, kind}` before its first output, so the window shows it at once.
    /// Its number, from [`FIRST_DRIVER_TERM`].
    #[allow(dead_code)] // The tasks, runs and debuggers call it.
    pub fn term_start(&self, title: &str, kind: &str, program: &Path, args: &[String], cwd: &Path) -> u64 {
        let id = NEXT_DRIVER_TERM.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        send(&self.out, json!({"type": "term_started", "id": id, "title": title, "kind": kind}));
        self.term_spawn(id, program, args, cwd, (80, 24), Some((title.to_string(), kind.to_string())));
        id
    }

    fn term_spawn(
        &self,
        id: u64,
        program: &Path,
        args: &[String],
        cwd: &Path,
        (cols, rows): (u16, u16),
        made: Option<(String, String)>,
    ) {
        let env = vec![
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".into(), "truecolor".into()),
            ("TERM_PROGRAM".into(), "code-arena".into()),
            ("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into()),
        ];
        let pty = match ca_pty::Pty::spawn_without(program, args, cwd, &env, NOT_IN_TERMS, cols, rows) {
            Ok(p) => p,
            Err(e) => {
                send(&self.out, json!({"type": "term_exit", "id": id, "error": e.to_string()}));
                return;
            }
        };
        let (Ok(mut reader), Ok(writer)) = (pty.reader(), pty.writer()) else {
            send(&self.out, json!({"type": "term_exit", "id": id, "error": "the terminal could not be read"}));
            return;
        };
        let screen: Arc<Mutex<String>> = Arc::default();
        if let Ok(mut t) = self.terms.lock() {
            if t.contains_key(&id) {
                // Already running (the page came back and asked again): left as it is.
                return;
            }
            t.insert(id, Term { pty, writer, screen: screen.clone(), made });
        }
        let out = self.out.clone();
        let terms = self.terms.clone();
        std::thread::spawn(move || {
            let mut carry = Vec::new();
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let text = ca_pty::utf8_text(&mut carry, &buf[..n]);
                        if !text.is_empty() {
                            keep_screen(&screen, &text);
                            send(&out, json!({"type": "term_out", "id": id, "data": text}));
                        }
                    }
                }
            }
            let code = terms.lock().ok().and_then(|mut t| t.remove(&id)).and_then(|mut t| {
                t.pty.stop();
                t.pty.ended().flatten()
            });
            send(&out, json!({"type": "term_exit", "id": id, "code": code}));
        });
    }
}

/// The largest file or folder brought in from outside the project.
const MOST_IMPORTED: u64 = 256 * 1024 * 1024;

/// The name a copy is given in a folder, as VS Code names it: the name itself while it is free, then "name copy.ext",
/// "name copy 2.ext" and on (a folder's, or a name with no extension, gets no extension).
pub(crate) fn copy_name(name: &str, dir: bool, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if !dir && i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut n = 1;
    loop {
        let candidate = if n == 1 { format!("{stem} copy{ext}") } else { format!("{stem} copy {n}{ext}") };
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

impl Side {
    /// A file or a folder's tree copied inside the project, to `to` or, when something is there, to the first free
    /// copy's name beside it ("name copy.ext"). The reply says where it went (`asked`: the `to` it answers); never
    /// into itself.
    fn copy(&self, path: &str, to: &str) -> (Value, Vec<String>) {
        let fail = |e: String| (json!({"type": "copied", "path": path, "asked": to, "error": e}), Vec::new());
        let (from, wanted) = match (self.entry(path), self.entry(to)) {
            (Ok(f), Ok(d)) => (f, d),
            (Err(e), _) | (_, Err(e)) => return fail(e),
        };
        let Ok(meta) = std::fs::symlink_metadata(&from) else { return fail("not there".into()) };
        let (Some(dir), Some(name)) = (wanted.parent(), wanted.file_name()) else {
            return fail("not the name of a file or a folder".into());
        };
        if !dir.is_dir() {
            return fail("the folder it goes in is not there".into());
        }
        let taken = |n: &str| std::fs::symlink_metadata(dir.join(n)).is_ok();
        let dest = dir.join(copy_name(&name.to_string_lossy(), meta.is_dir(), taken));
        if dest.starts_with(&from) {
            return fail("a folder is not copied into itself".into());
        }
        if let Err(e) = copy_tree(&from, &dest) {
            let _ = remove_tree(&dest);
            return fail(e.to_string());
        }
        let now = self.shown(&dest);
        (json!({"type": "copied", "path": self.shown(&from), "asked": to, "to": now}), vec![parent_of(&now)])
    }

    /// A file or a folder from elsewhere on this machine (dropped on the Explorer) copied in at `to`; one there
    /// already is replaced only when asked (it is then kept aside first, as a deleted one is), else the reply's
    /// error is "exists". Nothing over 256 MB.
    fn import(&self, from: &str, to: &str, replace: bool) -> (Value, Vec<String>) {
        let fail = |e: &str| (json!({"type": "imported", "from": from, "to": to, "error": e}), Vec::new());
        let source = Path::new(from);
        if !source.is_absolute() {
            return fail("only a file or a folder on this machine, by its whole path");
        }
        let dest = match self.entry(to) {
            Ok(d) => d,
            Err(e) => return fail(&e),
        };
        if std::fs::symlink_metadata(source).is_err() {
            return fail("not there");
        }
        if tree_size(source, MOST_IMPORTED) > MOST_IMPORTED {
            return fail("bigger than 256 MB: copy it outside the editor");
        }
        if dest.starts_with(source) || source.starts_with(&dest) {
            return fail("a folder is not copied into itself");
        }
        if !dest.parent().is_some_and(Path::is_dir) {
            return fail("the folder it goes in is not there");
        }
        if std::fs::symlink_metadata(&dest).is_ok() {
            if !replace {
                return fail("exists");
            }
            let shown = self.shown(&dest);
            if let Err(e) = self.keep_aside(&dest, &shown) {
                return fail(&format!("what is there could not be kept aside, so it was not replaced: {e}"));
            }
        }
        if let Err(e) = copy_tree(source, &dest) {
            let _ = remove_tree(&dest);
            return fail(&e.to_string());
        }
        let now = self.shown(&dest);
        (json!({"type": "imported", "from": from, "to": now}), vec![parent_of(&now)])
    }

    /// A file or a folder the Explorer deleted put back where it was (`kept`: where the delete said it is kept,
    /// which must be among the deleted ones); not over what is there now.
    fn restore(&self, kept: &str, to: &str) -> (Value, Vec<String>) {
        let fail = |e: &str| (json!({"type": "restored", "kept": kept, "to": to, "error": e}), Vec::new());
        let Ok(all) = std::fs::canonicalize(self.deleted()) else { return fail("nothing deleted is kept") };
        let source = Path::new(kept);
        // The folder it is in resolved (never a link out of the deleted ones), its own name as it is.
        let inside = source.parent().and_then(|p| std::fs::canonicalize(p).ok()).zip(source.file_name());
        let Some(source) = inside.map(|(dir, name)| dir.join(name)).filter(|s| s.starts_with(&all) && *s != all) else {
            return fail("only what the Explorer deleted is put back");
        };
        if std::fs::symlink_metadata(&source).is_err() {
            return fail("it is no longer kept");
        }
        let dest = match self.entry(to) {
            Ok(d) => d,
            Err(e) => return fail(&e),
        };
        if std::fs::symlink_metadata(&dest).is_ok() {
            return fail("a file or folder of that name is already there");
        }
        let moved = match dest.parent() {
            Some(p) => std::fs::create_dir_all(p).and_then(|()| move_aside(&source, &dest)),
            None => move_aside(&source, &dest),
        };
        if let Err(e) = moved {
            return fail(&e.to_string());
        }
        let now = self.shown(&dest);
        (json!({"type": "restored", "kept": kept, "to": now}), vec![parent_of(&now)])
    }
}

/// The most files quick open lists, and the most hits a search gives.
const MOST_FILES: usize = 50_000;
const MOST_HITS: usize = 2000;

/// The project's files as git sees them (its ignores kept), for quick open.
fn walker(root: &Path) -> ignore::Walk {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(std::cmp::Ord::cmp)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
}

fn files(root: &Path) -> Value {
    let mut paths = Vec::new();
    let mut more = false;
    for e in walker(root).flatten() {
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if paths.len() >= MOST_FILES {
            more = true;
            break;
        }
        if let Ok(rel) = e.path().strip_prefix(root) {
            paths.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    json!({"type": "files", "paths": paths, "more": more})
}

/// What a search (or a replace) looks for, as the Search view says it: the query (a regex when `regex`; any case
/// unless `case`; whole words only when `word`), in the files `include` names (none: all) and not in those
/// `exclude` names, both comma-separated globs as VS Code reads them.
#[derive(Clone, Debug, Default)]
struct Ask {
    query: String,
    regex: bool,
    case: bool,
    word: bool,
    include: String,
    exclude: String,
}

impl Ask {
    fn of(m: &Value) -> Ask {
        let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
        Ask {
            query: s("query"),
            regex: m["regex"] == true,
            case: m["case"] == true,
            word: m["word"] == true,
            include: s("include"),
            exclude: s("exclude"),
        }
    }

    /// The reply's keys: what it answers, so a page knows an answer to what it asked before.
    fn keys(&self, kind: &str) -> Value {
        json!({"type": kind, "query": self.query, "regex": self.regex, "case": self.case, "word": self.word,
            "include": self.include, "exclude": self.exclude})
    }

    /// The pattern a line is searched with, one for the search and the replace alike.
    fn pattern(&self) -> Result<regex::Regex, String> {
        let body = if self.regex { self.query.clone() } else { regex_escape(&self.query) };
        let wordy = |c: Option<char>| self.regex || c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let body = if self.word {
            let (a, z) = (wordy(self.query.chars().next()), wordy(self.query.chars().last()));
            format!("{}(?:{body}){}", if a { r"\b" } else { "" }, if z { r"\b" } else { "" })
        } else {
            body
        };
        // Any case unless asked, and lines as the editor reads them (`$` before a \r\n too).
        let body = if self.case { body } else { format!("(?i){body}") };
        regex::RegexBuilder::new(&body).multi_line(true).crlf(true).build().map_err(|e| e.to_string())
    }
}

/// Comma-separated globs as VS Code's Search view reads them: `*.rs` and `src` match at any depth, `./src` from
/// the project's root, and a folder's name means everything in it. None when there are none.
fn globs(text: &str) -> Result<Option<globset::GlobSet>, String> {
    let mut set = globset::GlobSetBuilder::new();
    let mut any = false;
    for p in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let p = p.replace('\\', "/");
        let p = match p.strip_prefix("./").or_else(|| p.strip_prefix('/')) {
            Some(rooted) => rooted.to_string(),
            None if p.starts_with("**/") => p,
            None => format!("**/{p}"),
        };
        let p = p.trim_end_matches('/');
        for g in [p.to_string(), format!("{p}/**")] {
            let glob = globset::GlobBuilder::new(&g)
                .literal_separator(true)
                .build()
                .map_err(|e| format!("Invalid glob pattern {g}: {e}"))?;
            set.add(glob);
        }
        any = true;
    }
    if !any {
        return Ok(None);
    }
    set.build().map(Some).map_err(|e| e.to_string())
}

/// The files a search or a replace looks in: those git sees, the include and exclude kept, not over 5 MB.
fn searched(root: &Path, ask: &Ask) -> Result<Vec<(String, PathBuf)>, String> {
    let (include, exclude) = (globs(&ask.include)?, globs(&ask.exclude)?);
    let mut out = Vec::new();
    for e in walker(root).flatten() {
        if !e.file_type().is_some_and(|t| t.is_file()) || e.metadata().map(|m| m.len() > MOST_BYTES).unwrap_or(true) {
            continue;
        }
        let rel = e.path().strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        if include.as_ref().is_some_and(|g| !g.is_match(&rel)) || exclude.as_ref().is_some_and(|g| g.is_match(&rel)) {
            continue;
        }
        out.push((rel, e.path().to_path_buf()));
    }
    Ok(out)
}

/// A line without its end (`\n` or `\r\n`), and that end.
fn line_and_end(piece: &str) -> (&str, &str) {
    let body = piece.strip_suffix('\n').map(|b| b.strip_suffix('\r').unwrap_or(b)).unwrap_or(piece);
    (body, &piece[body.len()..])
}

/// The largest char boundary at or before `at`.
fn boundary_before(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// What of a long line a hit shows: from a little before the match to a little after it, and where the match is
/// in that. A short line is shown whole.
fn shown_part(line: &str, col: usize, len: usize) -> (String, usize) {
    let from = if col > 120 { boundary_before(line, col - 60) } else { 0 };
    let to = boundary_before(line, (col + len.min(300)).max(from + 300));
    (line[from..to].to_string(), col - from)
}

/// Each match of `ask.query` in the files git sees, one hit a match: its file, its line (from 1), its column and
/// length (in bytes of the line, as the editor counts), and the part of the line shown with where the match is in
/// it (`at`, in bytes too).
fn search(root: &Path, ask: &Ask) -> Value {
    let mut reply = ask.keys("search");
    let fail = |mut reply: Value, e: String| {
        reply["error"] = json!(e);
        reply
    };
    if ask.query.is_empty() {
        reply["hits"] = json!([]);
        return reply;
    }
    let re = match ask.pattern() {
        Ok(r) => r,
        Err(e) => return fail(reply, e),
    };
    let files_in = match searched(root, ask) {
        Ok(f) => f,
        Err(e) => return fail(reply, e),
    };
    let mut hits: Vec<Value> = Vec::new();
    let mut files = 0;
    let mut more = false;
    'walk: for (rel, real) in files_in {
        // Read whole (5 MB at most), looked through at once, and line by line only when it has a match; a file
        // with a NUL in it is binary, one not UTF-8 is not text: neither is searched.
        let Ok(bytes) = std::fs::read(&real) else { continue };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else { continue };
        if !re.is_match(text) {
            continue;
        }
        let mut found = false;
        for (n, piece) in text.split_inclusive('\n').enumerate() {
            let (body, _) = line_and_end(piece);
            for m in re.find_iter(body).filter(|m| !m.is_empty()) {
                if hits.len() >= MOST_HITS {
                    more = true;
                    break 'walk;
                }
                if !found {
                    found = true;
                    files += 1;
                }
                let (shown, at) = shown_part(body, m.start(), m.len());
                hits.push(
                    json!({"path": rel, "line": n + 1, "col": m.start(), "len": m.len(), "text": shown, "at": at}),
                );
            }
        }
    }
    reply["hits"] = json!(hits);
    reply["files"] = json!(files);
    reply["more"] = json!(more);
    reply
}

/// `$1abc` read as group 1 then `abc` (the regex crate would read a group named `1abc`), as VS Code reads it.
fn replacement_of(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek().is_some_and(char::is_ascii_digit) {
            out.push_str("${");
            while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                out.push(d);
                chars.next();
            }
            out.push('}');
        } else if c == '$' && chars.peek() == Some(&'$') {
            out.push_str("$$");
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// A file's text with each match replaced (only those at `only`'s (line, col) when given), each line's own end
/// kept; and how many were.
fn replace_text(
    text: &str,
    re: &regex::Regex,
    with: &str,
    regex: bool,
    only: Option<&[(usize, usize)]>,
) -> (String, usize) {
    let with = if regex { replacement_of(with) } else { with.to_string() };
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    for (n, piece) in text.split_inclusive('\n').enumerate() {
        let (body, end) = line_and_end(piece);
        let mut last = 0;
        for caps in re.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            if m.is_empty() || only.is_some_and(|o| !o.contains(&(n + 1, m.start()))) {
                continue;
            }
            out.push_str(&body[last..m.start()]);
            if regex {
                caps.expand(&with, &mut out);
            } else {
                out.push_str(&with);
            }
            last = m.end();
            count += 1;
        }
        out.push_str(&body[last..]);
        out.push_str(end);
    }
    (out, count)
}

/// Every match replaced in the files given, or only the matches `only` gives (by file: line, col): a file not
/// plain UTF-8 text, or that could not be written, is said in `errors` and left as it was.
fn replace(ask: &Ask, with: &str, files: Vec<(String, PathBuf)>, only: &HashMap<String, Vec<(usize, usize)>>) -> Value {
    let mut reply = ask.keys("replaced");
    reply["files"] = json!([]);
    reply["errors"] = json!([]);
    let re = match ask.pattern() {
        Ok(r) if !ask.query.is_empty() => r,
        Ok(_) => return reply,
        Err(e) => {
            reply["errors"] = json!([{"path": "", "error": e}]);
            return reply;
        }
    };
    let (mut done, mut errors) = (Vec::new(), Vec::new());
    for (rel, real) in files {
        let at = only.get(&rel).map(Vec::as_slice);
        if !only.is_empty() && at.is_none() {
            continue;
        }
        let text = match std::fs::metadata(&real) {
            Ok(m) if !m.is_file() || m.len() > MOST_BYTES => Err("not a plain file under 5 MB".to_string()),
            Ok(_) => std::fs::read(&real).map_err(|e| e.to_string()).and_then(|b| {
                // Binary (a NUL in it), as the search reads it: not looked in.
                if b.contains(&0) {
                    Ok(String::new())
                } else {
                    String::from_utf8(b).map_err(|_| "not UTF-8 text".to_string())
                }
            }),
            Err(e) => Err(e.to_string()),
        };
        let text = match text {
            Ok(t) => t,
            Err(e) => {
                errors.push(json!({"path": rel, "error": e}));
                continue;
            }
        };
        let (new, count) = replace_text(&text, &re, with, ask.regex, at);
        if count == 0 {
            continue;
        }
        match std::fs::write(&real, new) {
            Ok(()) => done.push(json!({"path": rel, "count": count})),
            Err(e) => errors.push(json!({"path": rel, "error": e.to_string()})),
        }
    }
    reply["files"] = json!(done);
    reply["errors"] = json!(errors);
    reply
}

fn regex_escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side() -> (tempfile::TempDir, Side, broadcast::Receiver<String>) {
        let d = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(d.path().join("src")).expect("dir");
        std::fs::write(d.path().join("src/main.rs"), "fn main() {}\n").expect("w");
        std::fs::write(d.path().join("blob.bin"), [0u8, 1, 2]).expect("w");
        let ws = Arc::new(ca_workspace::Workspace::open(d.path(), &[]).expect("ws"));
        let (out, rx) = broadcast::channel(64);
        (d, Side::new(ws, Arc::default(), out, None, &std::env::temp_dir()), rx)
    }

    #[test]
    fn files_inside_the_project_only_plain_text_ones() {
        let (d, s, _rx) = side();
        let ls = s.ls("");
        let names: Vec<&str> =
            ls["entries"].as_array().expect("entries").iter().filter_map(|e| e["name"].as_str()).collect();
        assert_eq!(names, ["src", "blob.bin"], "folders first");
        assert_eq!(s.open("src/main.rs")["text"], "fn main() {}\n");
        assert!(s.open("blob.bin")["error"].as_str().is_some_and(|e| e.contains("binary")));
        assert_eq!(s.open("blob.bin")["kind"], "binary", "why it is not opened, for the tab that says so");
        assert_eq!(s.open("src")["kind"], "other");
        assert!(s.open("/etc/passwd")["error"].as_str().is_some_and(|e| e.contains("inside the project")));
        assert!(s.open("../outside.txt")["error"].is_string());
        assert!(s.open("src")["error"].as_str().is_some_and(|e| e.contains("not a plain file")));
        assert!(s.save("src/new.rs", "// new\n")["error"].is_null());
        assert_eq!(std::fs::read_to_string(d.path().join("src/new.rs")).expect("saved"), "// new\n");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", d.path().join("etc-link")).expect("link");
            assert!(s.open("etc-link/hostname")["error"].is_string(), "a link out is refused");
        }
    }

    fn ask(query: &str, regex: bool, case: bool) -> Ask {
        Ask { query: query.into(), regex, case, ..Ask::default() }
    }

    fn found(r: &Value) -> Vec<String> {
        r["hits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| format!("{}:{}:{}:{}", h["path"].as_str().unwrap_or(""), h["line"], h["col"], h["len"]))
            .collect()
    }

    #[test]
    fn a_search_gives_each_match_with_its_column_whole_words_and_globs_kept() {
        let (d, _s, _rx) = side();
        std::fs::create_dir_all(d.path().join("docs/deep")).expect("dir");
        std::fs::write(d.path().join("src/lib.rs"), "let cart = cart_of(cart);\r\n// é cart\n").expect("w");
        std::fs::write(d.path().join("docs/deep/a.md"), "cart\n").expect("w");
        let r = search(d.path(), &ask("cart", false, false));
        assert_eq!(
            found(&r),
            ["docs/deep/a.md:1:0:4", "src/lib.rs:1:4:4", "src/lib.rs:1:11:4", "src/lib.rs:1:19:4", "src/lib.rs:2:6:4"]
        );
        assert_eq!((r["files"].as_u64(), r["word"].as_bool(), r["include"].as_str()), (Some(2), Some(false), Some("")));
        let word = Ask { word: true, ..ask("cart", false, false) };
        assert_eq!(found(&search(d.path(), &word)).len(), 4, "cart_of is not the word cart");
        let only = |include: &str, exclude: &str| {
            let a = Ask { include: include.into(), exclude: exclude.into(), ..ask("cart", false, false) };
            let mut paths: Vec<String> =
                found(&search(d.path(), &a)).iter().map(|h| h.split(':').next().unwrap_or("").to_string()).collect();
            paths.dedup();
            paths
        };
        assert_eq!(only("*.md", ""), ["docs/deep/a.md"]);
        assert_eq!(only("docs", ""), ["docs/deep/a.md"], "a folder's name: all in it, at any depth");
        assert_eq!(only("deep", ""), ["docs/deep/a.md"]);
        assert!(only("./deep", "").is_empty(), "./ is from the root");
        assert_eq!(only("./src, *.md", "deep"), ["src/lib.rs"]);
        let bad = Ask { include: "{a".into(), ..ask("cart", false, false) };
        assert!(search(d.path(), &bad)["error"].is_string());
        // A long line shows the part around its match, and where the match is in it.
        std::fs::write(d.path().join("src/long.rs"), format!("{}needle{}\n", "x".repeat(500), "y".repeat(500)))
            .expect("w");
        let r = search(d.path(), &ask("needle", false, true));
        let h = &r["hits"][0];
        let (text, at) = (h["text"].as_str().unwrap_or(""), h["at"].as_u64().unwrap_or(0) as usize);
        assert_eq!((h["col"].as_u64(), &text[at..at + 6]), (Some(500), "needle"));
        assert!(text.len() < 400, "{}", text.len());
    }

    #[test]
    fn a_replace_changes_what_the_search_found_and_keeps_each_line_end() {
        let (d, _s, _rx) = side();
        let file = d.path().join("src/lib.rs");
        std::fs::write(&file, "foo(1) foo\r\nbar foo\nfoo").expect("w");
        let files = || vec![("src/lib.rs".to_string(), file.clone())];
        let none = HashMap::new();
        let end = ask("foo$", true, true);
        assert_eq!(found(&search(d.path(), &end)).len(), 3, "foo at a line's end, CRLF or not");
        let r = replace(&end, "x", files(), &none);
        assert_eq!(r["files"], json!([{"path": "src/lib.rs", "count": 3}]));
        assert_eq!(std::fs::read_to_string(&file).expect("r"), "foo(1) x\r\nbar x\nx");
        // Groups with a regex, `$1abc` as group 1 then abc; the text as it is without.
        replace(&ask(r"foo\((\d)\)", true, true), "$1abc", files(), &none);
        assert_eq!(std::fs::read_to_string(&file).expect("r"), "1abc x\r\nbar x\nx");
        replace(&ask("x", false, true), "$1", files(), &none);
        assert_eq!(std::fs::read_to_string(&file).expect("r"), "1abc $1\r\nbar $1\n$1");
        // Only the match asked for, by its line and column.
        let one: HashMap<String, Vec<(usize, usize)>> = [("src/lib.rs".to_string(), vec![(2, 4)])].into();
        let r = replace(&ask("$1", false, true), "y", files(), &one);
        assert_eq!(r["files"][0]["count"], 1);
        assert_eq!(std::fs::read_to_string(&file).expect("r"), "1abc $1\r\nbar y\n$1");
        // Not UTF-8: said, and left as it was.
        std::fs::write(&file, b"foo \xff").expect("w");
        let r = replace(&ask("foo", false, true), "x", files(), &none);
        assert!(r["errors"][0]["error"].is_string() && r["files"] == json!([]), "{r}");
        assert_eq!(std::fs::read(&file).expect("r"), b"foo \xff");
    }

    #[tokio::test]
    async fn a_replace_asked_of_the_side_answers_then_says_the_files_changed() {
        let (d, s, mut rx) = side();
        s.handle(
            &json!({"type": "replace", "query": "main", "replacement": "start", "paths": ["src/main.rs", "../out"]}),
        );
        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(drain(&mut rx));
            if got.iter().any(|v| v["type"] == "stats") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let r = got.iter().find(|v| v["type"] == "replaced").expect("replaced");
        assert_eq!((r["query"].as_str(), r["files"][0]["count"].as_u64()), (Some("main"), Some(1)));
        assert_eq!(r["errors"][0]["path"], "../out");
        assert!(got.iter().any(|v| v["type"] == "stats" && v["items"][0]["path"] == "src/main.rs"), "{got:?}");
        assert_eq!(std::fs::read_to_string(d.path().join("src/main.rs")).expect("r"), "fn start() {}\n");
    }

    #[test]
    fn quick_open_and_search_see_what_git_sees() {
        let (d, _s, _rx) = side();
        std::fs::write(d.path().join(".gitignore"), "target/\n").expect("w");
        std::fs::create_dir_all(d.path().join("target")).expect("dir");
        std::fs::write(d.path().join("target/out.rs"), "fn main() {}\n").expect("w");
        std::fs::write(d.path().join("src/lib.rs"), "pub fn main_thing() {}\n// Main (a.b)\n").expect("w");
        let f = files(d.path());
        let paths: Vec<&str> = f["paths"].as_array().expect("paths").iter().filter_map(Value::as_str).collect();
        assert!(paths.contains(&"src/main.rs") && paths.contains(&"src/lib.rs"), "{paths:?}");
        assert!(!paths.iter().any(|p| p.starts_with("target/")), "ignored by git: {paths:?}");
        let r = search(d.path(), &ask("main", false, false));
        let hits: Vec<String> = r["hits"]
            .as_array()
            .expect("hits")
            .iter()
            .map(|h| format!("{}:{}", h["path"].as_str().unwrap_or(""), h["line"]))
            .collect();
        assert_eq!(hits, ["src/lib.rs:1", "src/lib.rs:2", "src/main.rs:1"]);
        // As typed: the dot and the brackets are not a pattern; with case kept, "Main" only.
        assert_eq!(search(d.path(), &ask("(a.b)", false, false))["hits"].as_array().map(Vec::len), Some(1));
        assert_eq!(search(d.path(), &ask("Main", false, true))["hits"].as_array().map(Vec::len), Some(1));
        assert!(search(d.path(), &ask("(", true, false))["error"].is_string());
    }

    #[test]
    fn the_agents_changes_are_each_file_before_and_now() {
        let d = tempfile::tempdir().expect("temp");
        std::fs::write(d.path().join("a.txt"), "one\n").expect("w");
        let ws = Arc::new(ca_workspace::Workspace::open(d.path(), &[]).expect("ws"));
        let cps = Arc::new(ca_tools::checkpoints::Checkpoints::default());
        let (out, _rx) = broadcast::channel(8);
        let s = Side::new(ws.clone(), cps.clone(), out, None, &std::env::temp_dir());
        cps.begin(0);
        let a = ws.resolve("a.txt").expect("a");
        cps.before_write(&a);
        std::fs::write(&a, "two\n").expect("w");
        let b = ws.resolve("b.txt").expect("b");
        cps.before_write(&b);
        std::fs::write(&b, "new\n").expect("w");
        let c = s.changes();
        let files = c["files"].as_array().expect("files");
        assert_eq!(files.len(), 2);
        assert_eq!(
            (files[0]["path"].as_str(), files[0]["before"].as_str(), files[0]["after"].as_str()),
            (Some("a.txt"), Some("one\n"), Some("two\n"))
        );
        assert!(files[1]["before"].is_null() && files[1]["after"] == "new\n");
        // Put back as it was: no longer a change.
        std::fs::write(&a, "one\n").expect("w");
        assert_eq!(s.changes()["files"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn the_agents_changes_are_accepted_reverted_or_refused() {
        let d = tempfile::tempdir().expect("temp");
        std::fs::write(d.path().join("a.txt"), "one\n").expect("w");
        std::fs::write(d.path().join("mine.txt"), "mine\n").expect("w");
        let ws = Arc::new(ca_workspace::Workspace::open(d.path(), &[]).expect("ws"));
        let cps = Arc::new(ca_tools::checkpoints::Checkpoints::default());
        let (out, mut rx) = broadcast::channel(64);
        let s = Side::new(ws.clone(), cps.clone(), out, None, &std::env::temp_dir());
        cps.begin(0);
        for (name, text) in [("a.txt", "two\n"), ("sub/b.txt", "new\n"), ("c.txt", "made\n")] {
            let p = ws.resolve(name).expect("p");
            cps.before_write(&p);
            std::fs::create_dir_all(p.parent().expect("dir")).expect("dir");
            std::fs::write(&p, text).expect("w");
        }
        let paths = |v: &Value| -> Vec<String> {
            v["files"].as_array().into_iter().flatten().filter_map(|f| f["path"].as_str().map(str::to_string)).collect()
        };
        let last_changes = |got: &[Value]| got.iter().rev().find(|v| v["type"] == "changes").cloned().expect("changes");
        s.handle(&json!({"type": "changes_accept", "path": "a.txt"}));
        let got = drain(&mut rx);
        assert_eq!(last_changes(&got)["path"], "a.txt");
        assert_eq!(paths(&last_changes(&got)), ["c.txt", "sub/b.txt"]);
        assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).expect("r"), "two\n", "accepting keeps the file");
        // A made file reverted is gone; its stat and its folder are said again.
        s.handle(&json!({"type": "changes_revert", "path": "sub/b.txt"}));
        let got = drain(&mut rx);
        assert!(!d.path().join("sub/b.txt").exists());
        assert_eq!(paths(&last_changes(&got)), ["c.txt"]);
        assert!(got.iter().any(|v| v["type"] == "stats" && v["items"][0]["gone"] == true), "{got:?}");
        assert!(got.iter().any(|v| v["type"] == "ls" && v["path"] == "sub"), "{got:?}");
        // A file the agent did not write is never touched.
        s.handle(&json!({"type": "changes_revert", "path": "mine.txt"}));
        let got = drain(&mut rx);
        assert!(got.iter().any(|v| v["type"] == "notice"), "{got:?}");
        assert_eq!(std::fs::read_to_string(d.path().join("mine.txt")).expect("r"), "mine\n");
        // Part accepted, then the rest: none is left.
        s.handle(&json!({"type": "changes_accept_text", "path": "c.txt", "text": "ma"}));
        assert_eq!(paths(&last_changes(&drain(&mut rx))), ["c.txt"]);
        s.handle(&json!({"type": "changes_accept"}));
        assert!(paths(&last_changes(&drain(&mut rx))).is_empty());
    }

    fn drain(rx: &mut broadcast::Receiver<String>) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(t) = rx.try_recv() {
            out.push(serde_json::from_str(&t).expect("json"));
        }
        out
    }

    #[test]
    fn what_is_watched_says_when_it_changes_but_not_for_a_save_of_ones_own() {
        let (d, s, mut rx) = side();
        s.handle(&json!({"type": "watch", "files": ["src/main.rs", "gone.rs"], "folders": ["src"]}));
        s.look();
        assert!(drain(&mut rx).is_empty(), "nothing changed yet");
        // The person's own save: not told as a change.
        s.handle(&json!({"type": "save", "path": "src/main.rs", "text": "fn main() { }\n"}));
        drain(&mut rx);
        s.look();
        assert!(drain(&mut rx).is_empty(), "a save of one's own is not a change on disk");
        // From outside: the file's new stamp, and the folder listed again for the file made in it.
        let file = d.path().join("src/main.rs");
        let later = std::fs::metadata(&file).expect("m").modified().expect("t") + std::time::Duration::from_secs(5);
        std::fs::write(&file, "fn main() { println!(); }\n").expect("w");
        std::fs::File::options().write(true).open(&file).expect("f").set_modified(later).expect("touch");
        std::fs::write(d.path().join("src/new.rs"), "").expect("w");
        let dir_later = std::fs::metadata(d.path().join("src")).expect("m").modified().expect("t")
            + std::time::Duration::from_secs(5);
        std::fs::File::open(d.path().join("src")).expect("dir").set_modified(dir_later).expect("touch dir");
        s.look();
        let got = drain(&mut rx);
        assert!(got.iter().any(|v| v["type"] == "stats" && v["items"][0]["path"] == "src/main.rs"), "{got:?}");
        assert!(got.iter().any(|v| v["type"] == "ls" && v["path"] == "src"), "{got:?}");
        // Once told, not again.
        s.look();
        assert!(drain(&mut rx).is_empty());
        std::fs::remove_file(&file).expect("rm");
        s.look();
        let got = drain(&mut rx);
        assert!(got.iter().any(|v| v["items"][0]["gone"] == true), "{got:?}");
    }

    #[test]
    fn unsaved_changes_are_kept_aside_for_the_person_only_and_go_with_a_save() {
        let d = tempfile::tempdir().expect("temp");
        std::fs::write(d.path().join("a.txt"), "one\n").expect("w");
        let data = tempfile::tempdir().expect("data");
        let ws = Arc::new(ca_workspace::Workspace::open(d.path(), &[]).expect("ws"));
        let (out, mut rx) = broadcast::channel(16);
        let s = Side::new(ws, Arc::default(), out, None, data.path());
        s.handle(&json!({"type": "backup", "path": "a.txt", "text": "one, changed\n"}));
        s.handle(&json!({"type": "backups"}));
        let got = drain(&mut rx);
        let items = got.iter().find(|v| v["type"] == "backups").expect("listed")["items"].clone();
        assert_eq!((items[0]["path"].as_str(), items[0]["text"].as_str()), (Some("a.txt"), Some("one, changed\n")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let f = std::fs::read_dir(&s.backups).expect("dir").flatten().next().expect("one").path();
            assert_eq!(std::fs::metadata(f).expect("m").permissions().mode() & 0o777, 0o600);
        }
        s.handle(&json!({"type": "save", "path": "a.txt", "text": "one, changed\n"}));
        s.handle(&json!({"type": "backups"}));
        let got = drain(&mut rx);
        assert_eq!(
            got.iter().find(|v| v["type"] == "backups").expect("listed")["items"].as_array().map(Vec::len),
            Some(0)
        );
        assert!(s.handle(&json!({"type": "backup", "path": "../x.txt", "text": "x"})));
        assert!(drain(&mut rx).iter().any(|v| v["type"] == "notice"), "a path outside is refused");
    }

    #[test]
    fn a_save_into_folders_not_made_yet_makes_them() {
        let (d, s, mut rx) = side();
        s.handle(&json!({"type": "save", "path": "new/dir/a.rs", "text": "fn a() {}\n"}));
        let got = drain(&mut rx);
        let saved = got.iter().find(|v| v["type"] == "saved").expect("saved");
        assert_eq!((saved["path"].as_str(), saved.get("error")), (Some("new/dir/a.rs"), None));
        assert_eq!(std::fs::read_to_string(d.path().join("new/dir/a.rs")).expect("read"), "fn a() {}\n");
        s.handle(&json!({"type": "save", "path": "../out/a.rs", "text": "x"}));
        assert!(drain(&mut rx).iter().any(|v| v["type"] == "saved" && v["error"].is_string()), "outside is refused");
        assert!(!d.path().join("../out").exists());
    }

    /// A project and a data folder of its own (what is deleted is kept in it).
    fn side_with_data() -> (tempfile::TempDir, tempfile::TempDir, Side, broadcast::Receiver<String>) {
        let d = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(d.path().join("src")).expect("dir");
        std::fs::write(d.path().join("src/main.rs"), "fn main() {}\n").expect("w");
        let data = tempfile::tempdir().expect("data");
        let ws = Arc::new(ca_workspace::Workspace::open(d.path(), &[]).expect("ws"));
        let (out, rx) = broadcast::channel(64);
        let s = Side::new(ws, Arc::default(), out, None, data.path());
        (d, data, s, rx)
    }

    fn listed(got: &[Value]) -> Vec<&str> {
        got.iter().filter(|v| v["type"] == "ls").filter_map(|v| v["path"].as_str()).collect()
    }

    #[test]
    fn a_file_or_a_folder_is_made_once_and_its_folder_listed_again() {
        let (d, _data, s, mut rx) = side_with_data();
        assert!(s.handle(&json!({"type": "create", "path": "src/new.rs", "dir": false})));
        let got = drain(&mut rx);
        assert_eq!((got[0]["type"].as_str(), got[0]["path"].as_str()), (Some("created"), Some("src/new.rs")));
        assert!(got[0]["dir"] == false && got[0]["error"].is_null());
        assert_eq!(listed(&got), ["src"]);
        assert_eq!(std::fs::read(d.path().join("src/new.rs")).expect("made"), b"");
        // Not over what is there: the file stays as it is.
        std::fs::write(d.path().join("src/new.rs"), "kept").expect("w");
        s.handle(&json!({"type": "create", "path": "src/new.rs", "dir": false}));
        s.handle(&json!({"type": "create", "path": "src", "dir": true}));
        let got = drain(&mut rx);
        assert_eq!(got.len(), 2, "no listing after a failure: {got:?}");
        assert!(got.iter().all(|v| v["error"].as_str().is_some_and(|e| e.contains("already there"))), "{got:?}");
        assert_eq!(std::fs::read_to_string(d.path().join("src/new.rs")).expect("r"), "kept");
        // A folder, with the folders on the way: the first one that was there is listed, and the one it is in.
        s.handle(&json!({"type": "create", "path": "a/b/c", "dir": true}));
        let got = drain(&mut rx);
        assert!(d.path().join("a/b/c").is_dir());
        assert_eq!(listed(&got), ["", "a/b"]);
    }

    #[test]
    fn nothing_is_made_moved_or_deleted_out_of_the_project_nor_its_top_nor_gits_folder() {
        let (d, _data, s, mut rx) = side_with_data();
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("theirs.txt"), "theirs").expect("w");
        std::fs::create_dir_all(d.path().join(".git")).expect("git");
        let out = outside.path().to_string_lossy().into_owned();
        #[allow(unused_mut)]
        let mut asked = vec![
            json!({"type": "create", "path": "../x.txt"}),
            json!({"type": "create", "path": "/tmp/x.txt"}),
            json!({"type": "create", "path": format!("{out}/x.txt")}),
            json!({"type": "create", "path": ".git/hooks/x"}),
            json!({"type": "rename", "path": "src/main.rs", "to": "../main.rs"}),
            json!({"type": "rename", "path": "src/main.rs", "to": format!("{out}/main.rs")}),
            json!({"type": "rename", "path": "../x", "to": "x"}),
            json!({"type": "rename", "path": "", "to": "x"}),
            json!({"type": "rename", "path": "src/..", "to": "x"}),
            json!({"type": "rename", "path": "src", "to": "src/inner"}),
            json!({"type": "delete", "path": ""}),
            json!({"type": "delete", "path": "./"}),
            json!({"type": "delete", "path": "."}),
            json!({"type": "delete", "path": "src/.."}),
            json!({"type": "delete", "path": "../"}),
            json!({"type": "delete", "path": ".git"}),
            json!({"type": "delete", "path": format!("{out}/theirs.txt")}),
        ];
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), d.path().join("out")).expect("link");
            asked.push(json!({"type": "create", "path": "out/x.txt"}));
            asked.push(json!({"type": "rename", "path": "src/main.rs", "to": "out/main.rs"}));
            asked.push(json!({"type": "delete", "path": "out/theirs.txt"}));
        }
        for m in &asked {
            assert!(s.handle(m));
            let got = drain(&mut rx);
            assert_eq!(got.len(), 1, "{m}: {got:?}");
            assert!(got[0]["error"].is_string(), "{m} was not refused: {got:?}");
        }
        assert!(d.path().join("src/main.rs").is_file() && d.path().join(".git").is_dir());
        assert_eq!(std::fs::read_dir(outside.path()).expect("dir").count(), 1, "nothing made outside");
        assert_eq!(std::fs::read_to_string(outside.path().join("theirs.txt")).expect("r"), "theirs");
    }

    #[test]
    fn a_file_is_moved_but_not_over_another_and_both_folders_are_listed_again() {
        let (d, _data, s, mut rx) = side_with_data();
        std::fs::write(d.path().join("other.rs"), "other").expect("w");
        s.handle(&json!({"type": "rename", "path": "src/main.rs", "to": "other.rs"}));
        let got = drain(&mut rx);
        assert!(got.len() == 1 && got[0]["error"].as_str().is_some_and(|e| e.contains("already there")), "{got:?}");
        assert_eq!(std::fs::read_to_string(d.path().join("other.rs")).expect("r"), "other");
        s.handle(&json!({"type": "rename", "path": "missing.rs", "to": "x.rs"}));
        assert!(drain(&mut rx)[0]["error"].is_string());
        // Its unsaved changes follow it.
        s.handle(&json!({"type": "backup", "path": "src/main.rs", "text": "unsaved"}));
        s.handle(&json!({"type": "rename", "path": "src/main.rs", "to": "main.rs"}));
        let got = drain(&mut rx);
        assert_eq!(
            (got[0]["type"].as_str(), got[0]["path"].as_str(), got[0]["to"].as_str()),
            (Some("renamed"), Some("src/main.rs"), Some("main.rs"))
        );
        assert_eq!(listed(&got), ["src", ""]);
        assert!(d.path().join("main.rs").is_file() && !d.path().join("src/main.rs").exists());
        s.handle(&json!({"type": "backups"}));
        let items = drain(&mut rx)[0]["items"].clone();
        assert_eq!(items.as_array().map(Vec::len), Some(1));
        assert_eq!((items[0]["path"].as_str(), items[0]["text"].as_str()), (Some("main.rs"), Some("unsaved")));
        assert!(s.backups.join(format!("{}.json", name_of("main.rs"))).is_file());
        // A folder, with what is in it.
        s.handle(&json!({"type": "rename", "path": "src", "to": "lib"}));
        let got = drain(&mut rx);
        assert!(got[0]["error"].is_null() && d.path().join("lib").is_dir(), "{got:?}");
        assert_eq!(listed(&got), [""]);
    }

    #[test]
    fn what_is_deleted_is_kept_aside_first_and_does_not_show_as_unsaved_changes() {
        let (d, data, s, mut rx) = side_with_data();
        std::fs::write(d.path().join("src/package.json"), "{\"path\": \"x\"}").expect("w");
        s.handle(&json!({"type": "backup", "path": "src/main.rs", "text": "unsaved"}));
        s.handle(&json!({"type": "delete", "path": "src/main.rs"}));
        let got = drain(&mut rx);
        assert_eq!((got[0]["type"].as_str(), got[0]["path"].as_str()), (Some("deleted"), Some("src/main.rs")));
        assert_eq!(listed(&got), ["src"]);
        assert!(!d.path().join("src/main.rs").exists());
        let kept = PathBuf::from(got[0]["kept"].as_str().expect("where it is kept"));
        assert!(kept.starts_with(data.path()) && kept.ends_with("src/main.rs"), "{kept:?}");
        assert_eq!(std::fs::read_to_string(&kept).expect("kept"), "fn main() {}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(s.deleted()).expect("m").permissions().mode() & 0o777, 0o700);
        }
        // A folder's tree, whole; and none of it is taken for unsaved changes (nor the file's own, now gone).
        s.handle(&json!({"type": "delete", "path": "src"}));
        let got = drain(&mut rx);
        assert!(got[0]["error"].is_null() && !d.path().join("src").exists(), "{got:?}");
        let kept = PathBuf::from(got[0]["kept"].as_str().expect("kept"));
        assert!(kept.join("package.json").is_file());
        s.handle(&json!({"type": "backups"}));
        assert_eq!(drain(&mut rx)[0]["items"].as_array().map(Vec::len), Some(0));
        s.handle(&json!({"type": "delete", "path": "src"}));
        assert!(drain(&mut rx)[0]["error"].as_str().is_some_and(|e| e.contains("not there")));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_deleted_as_a_link_and_one_in_a_folder_never_brings_what_it_leads_to() {
        let (d, _data, s, mut rx) = side_with_data();
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("theirs.txt"), "theirs").expect("w");
        // A link to a file of the project: the link goes, the file stays.
        std::os::unix::fs::symlink(d.path().join("src/main.rs"), d.path().join("main-link")).expect("link");
        s.handle(&json!({"type": "delete", "path": "main-link"}));
        let got = drain(&mut rx);
        assert!(got[0]["error"].is_null(), "{got:?}");
        assert!(d.path().join("src/main.rs").is_file());
        assert!(std::fs::symlink_metadata(d.path().join("main-link")).is_err());
        // A link out of the project, alone and inside a folder that is deleted: what it leads to is not touched.
        std::os::unix::fs::symlink(outside.path(), d.path().join("out")).expect("link");
        std::os::unix::fs::symlink(outside.path(), d.path().join("src/out")).expect("link");
        s.handle(&json!({"type": "delete", "path": "out"}));
        s.handle(&json!({"type": "delete", "path": "src"}));
        let got = drain(&mut rx);
        assert!(got.iter().all(|v| v["error"].is_null()), "{got:?}");
        assert!(!d.path().join("src").exists());
        assert_eq!(std::fs::read_to_string(outside.path().join("theirs.txt")).expect("r"), "theirs");
        let kept = got.iter().filter_map(|v| v["kept"].as_str()).nth(1).map(PathBuf::from).expect("kept");
        assert!(std::fs::symlink_metadata(kept.join("out")).expect("m").file_type().is_symlink());
    }

    #[test]
    fn a_folder_over_fifty_megabytes_is_not_deleted() {
        let (d, _data, s, mut rx) = side_with_data();
        // A file with a hole in it: its length, without the bytes written.
        std::fs::File::create(d.path().join("src/big.bin")).expect("f").set_len(MOST_DELETED + 1).expect("len");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/", d.path().join("src/everything")).expect("link");
        #[cfg(unix)]
        assert!(tree_size(&d.path().join("src/everything"), u64::MAX) < 4096, "a link counts as itself");
        assert!(tree_size(&d.path().join("src"), u64::MAX) < MOST_DELETED + 4096);
        s.handle(&json!({"type": "delete", "path": "src"}));
        let got = drain(&mut rx);
        assert!(got.len() == 1 && got[0]["error"].as_str().is_some_and(|e| e.contains("50 MB")), "{got:?}");
        assert!(d.path().join("src/main.rs").is_file());
    }

    #[test]
    fn what_cannot_be_renamed_aside_is_copied_whole_and_only_then_taken_away() {
        let d = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(d.path().join("from/in")).expect("dir");
        std::fs::write(d.path().join("from/a.txt"), "a").expect("w");
        std::fs::write(d.path().join("from/in/b.txt"), "b").expect("w");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("theirs.txt"), "theirs").expect("w");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), d.path().join("from/out")).expect("link");
        copy_then_remove(&d.path().join("from"), &d.path().join("to")).expect("moved");
        assert!(!d.path().join("from").exists());
        assert_eq!(std::fs::read_to_string(d.path().join("to/in/b.txt")).expect("r"), "b");
        assert_eq!(std::fs::read_to_string(d.path().join("to/a.txt")).expect("r"), "a");
        #[cfg(unix)]
        assert!(std::fs::symlink_metadata(d.path().join("to/out")).expect("m").file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(outside.path().join("theirs.txt")).expect("r"), "theirs");
        // A copy that fails leaves what was there, and no half of a copy.
        std::fs::write(d.path().join("one.txt"), "one").expect("w");
        assert!(copy_then_remove(&d.path().join("one.txt"), &d.path().join("no/such/folder/one.txt")).is_err());
        assert_eq!(std::fs::read_to_string(d.path().join("one.txt")).expect("r"), "one");
    }

    #[cfg(unix)]
    #[test]
    fn a_terminal_runs_and_says_what_it_prints() {
        let (_d, s, mut rx) = side();
        s.handle(&json!({"type": "term_open", "id": 1, "cols": 80, "rows": 24}));
        s.handle(&json!({"type": "term_in", "id": 1, "data": "echo hello-$((40+2)); exit\n"}));
        let mut text = String::new();
        loop {
            let v: Value = serde_json::from_str(&rx.blocking_recv().expect("message")).expect("json");
            match v["type"].as_str() {
                Some("term_out") => text.push_str(v["data"].as_str().unwrap_or("")),
                Some("term_exit") => break,
                _ => {}
            }
        }
        assert!(text.contains("hello-42"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_is_the_one_asked_for_then_the_environments_then_bash_then_sh() {
        let d = tempfile::tempdir().expect("temp");
        let (zsh, bash) = (d.path().join("zsh"), d.path().join("bash"));
        std::fs::write(&zsh, "").expect("w");
        std::fs::write(&bash, "").expect("w");
        let path = d.path().to_string_lossy().into_owned();
        assert_eq!(shell_for(Some("zsh"), None, Some(&path)), zsh);
        assert_eq!(shell_for(Some(zsh.to_str().expect("s")), None, None), zsh);
        // One that is not there, or none asked: $SHELL, then bash on PATH, then sh.
        assert_eq!(shell_for(Some("fish"), Some(&zsh), Some(&path)), zsh);
        assert_eq!(shell_for(None, Some(&d.path().join("gone")), Some(&path)), bash);
        assert_eq!(shell_for(Some("../zsh"), None, Some("/nowhere")), PathBuf::from("/bin/sh"));
        // The tabs' name is the shell found, not a fixed "sh" when $SHELL is not set.
        assert_eq!(shell_name_of(&shell_for(None, None, Some(&path))), "bash");
        assert_eq!(shell_name_of(&zsh), "zsh");
    }

    #[cfg(unix)]
    #[test]
    fn a_terminal_says_it_is_code_arenas_and_the_driver_starts_its_own() {
        let (d, s, mut rx) = side();
        let id = s.term_start("Task - build", "task", Path::new("/bin/sh"), &[], d.path());
        assert!(id >= FIRST_DRIVER_TERM);
        let first: Value = serde_json::from_str(&rx.blocking_recv().expect("message")).expect("json");
        assert_eq!(first, json!({"type": "term_started", "id": id, "title": "Task - build", "kind": "task"}));
        s.handle(&json!({"type": "term_in", "id": id, "data": "echo \"[$TERM_PROGRAM]\"\n"}));
        loop {
            let v: Value = serde_json::from_str(&rx.blocking_recv().expect("message")).expect("json");
            if v["type"] == "term_out" && v["data"].as_str().is_some_and(|d| d.contains("[code-arena]")) {
                break;
            }
        }
        s.handle(&json!({"type": "terms"}));
        let listed = loop {
            let v: Value = serde_json::from_str(&rx.blocking_recv().expect("message")).expect("json");
            if v["type"] == "terms" {
                break v;
            }
        };
        let t = listed["terms"].as_array().expect("terms").iter().find(|t| t["id"] == id).cloned().expect("listed");
        assert_eq!((t["title"].as_str(), t["kind"].as_str()), (Some("Task - build"), Some("task")));
        s.handle(&json!({"type": "term_close", "id": id}));
    }

    #[cfg(unix)]
    #[test]
    fn no_more_than_ten_of_the_persons_terminals_run() {
        let (_d, s, mut rx) = side();
        for id in 1..=11 {
            s.handle(&json!({"type": "term_open", "id": id, "cols": 80, "rows": 24}));
        }
        let refused = loop {
            let v: Value = serde_json::from_str(&rx.blocking_recv().expect("message")).expect("json");
            if v["type"] == "term_exit" {
                break v;
            }
        };
        assert_eq!(refused["id"], 11);
        assert!(refused["error"].as_str().is_some_and(|e| e.starts_with("At most 10")), "{refused}");
        for id in 1..=10 {
            s.handle(&json!({"type": "term_close", "id": id}));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_terminal_outlives_its_page_and_shows_its_screen_again() {
        let (_d, s, mut rx) = side();
        s.handle(&json!({"type": "term_open", "id": 7, "cols": 80, "rows": 24}));
        s.handle(&json!({"type": "term_in", "id": 7, "data": "echo kept-$((2*21))\n"}));
        loop {
            let v: Value = serde_json::from_str(&rx.blocking_recv().expect("message")).expect("json");
            if v["type"] == "term_out" && v["data"].as_str().is_some_and(|d| d.contains("kept-42")) {
                break;
            }
        }
        // The page goes (a reload) and comes back: the terminal is still there, with its screen.
        s.handle(&json!({"type": "terms"}));
        let listed = loop {
            let v: Value = serde_json::from_str(&rx.blocking_recv().expect("message")).expect("json");
            if v["type"] == "terms" {
                break v;
            }
        };
        assert_eq!(listed["terms"][0]["id"], 7);
        assert!(listed["terms"][0]["screen"].as_str().is_some_and(|t| t.contains("kept-42")), "{listed}");
        s.handle(&json!({"type": "term_close", "id": 7}));
    }

    #[test]
    fn a_kept_screen_holds_its_last_part_whole() {
        let screen = Mutex::new(String::new());
        keep_screen(&screen, &"é".repeat(SCREEN_KEPT));
        keep_screen(&screen, "end");
        let s = screen.lock().expect("lock");
        assert!(s.len() <= SCREEN_KEPT && s.ends_with("end"));
    }

    #[test]
    fn a_copy_is_named_as_vs_code_names_it() {
        let taken = |names: &'static [&'static str]| move |n: &str| names.contains(&n);
        assert_eq!(copy_name("a.rs", false, taken(&[])), "a.rs");
        assert_eq!(copy_name("a.rs", false, taken(&["a.rs"])), "a copy.rs");
        assert_eq!(copy_name("a.rs", false, taken(&["a.rs", "a copy.rs"])), "a copy 2.rs");
        assert_eq!(copy_name("a.tar.gz", false, taken(&["a.tar.gz"])), "a.tar copy.gz");
        assert_eq!(copy_name(".env", false, taken(&[".env"])), ".env copy");
        assert_eq!(copy_name("v1.2", true, taken(&["v1.2", "v1.2 copy"])), "v1.2 copy 2");
    }

    #[test]
    fn a_copy_goes_beside_what_is_there_and_never_into_itself() {
        let (d, _data, s, mut rx) = side_with_data();
        let (v, folders) = s.copy("src/main.rs", "src/main.rs");
        assert_eq!((v["to"].as_str(), v["asked"].as_str()), (Some("src/main copy.rs"), Some("src/main.rs")));
        assert_eq!(folders, ["src"]);
        assert_eq!(std::fs::read_to_string(d.path().join("src/main copy.rs")).expect("r"), "fn main() {}\n");
        // A folder into another: its whole tree.
        std::fs::create_dir_all(d.path().join("lib")).expect("dir");
        let (v, _) = s.copy("src", "lib/src");
        assert_eq!(v["to"], "lib/src");
        assert!(d.path().join("lib/src/main copy.rs").is_file());
        // Into itself: refused, and nothing half made.
        let (v, folders) = s.copy("lib", "lib/src/lib");
        assert!(v["error"].as_str().is_some_and(|e| e.contains("into itself")), "{v}");
        assert!(folders.is_empty() && !d.path().join("lib/src/lib").exists());
        let (v, _) = s.copy("gone.rs", "src/gone.rs");
        assert_eq!(v["error"], "not there");
        assert!(drain(&mut rx).is_empty(), "the worker itself says nothing; handle sends what it gives");
    }

    #[test]
    fn a_file_from_outside_is_brought_in_and_replaces_only_when_asked() {
        let (d, _data, s, _rx) = side_with_data();
        let outside = tempfile::tempdir().expect("outside");
        let from = outside.path().join("notes.txt");
        std::fs::write(&from, "theirs").expect("w");
        let from = from.to_string_lossy().into_owned();
        let (v, folders) = s.import(&from, "src/notes.txt", false);
        assert!(v["error"].is_null() && v["to"] == "src/notes.txt", "{v}");
        assert_eq!(folders, ["src"]);
        let (v, _) = s.import(&from, "src/main.rs", false);
        assert_eq!(v["error"], "exists");
        assert_eq!(std::fs::read_to_string(d.path().join("src/main.rs")).expect("r"), "fn main() {}\n");
        let (v, _) = s.import(&from, "src/main.rs", true);
        assert!(v["error"].is_null(), "{v}");
        assert_eq!(std::fs::read_to_string(d.path().join("src/main.rs")).expect("r"), "theirs");
        // What was replaced is kept among the deleted ones.
        assert!(walkdir::WalkDir::new(s.deleted()).into_iter().flatten().any(|e| e.file_name() == "main.rs"));
        let (v, _) = s.import("notes.txt", "src/x.txt", false);
        assert!(v["error"].as_str().is_some_and(|e| e.contains("whole path")));
        let (v, _) = s.import(&from, "../x.txt", false);
        assert!(v["error"].is_string());
    }

    #[test]
    fn what_was_deleted_is_put_back_and_nothing_else_is_brought_in_that_way() {
        let (d, _data, s, _rx) = side_with_data();
        let (v, _) = s.delete("src/main.rs");
        let kept = v["kept"].as_str().expect("kept").to_string();
        let (v, folders) = s.restore(&kept, "src/main.rs");
        assert!(v["error"].is_null() && v["to"] == "src/main.rs", "{v}");
        assert_eq!(folders, ["src"]);
        assert_eq!(std::fs::read_to_string(d.path().join("src/main.rs")).expect("r"), "fn main() {}\n");
        // Not twice, not over what is there, and never a file from anywhere else.
        assert_eq!(s.restore(&kept, "src/again.rs").0["error"], "it is no longer kept");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("x"), "x").expect("w");
        let (v, _) = s.restore(&outside.path().join("x").to_string_lossy(), "x");
        assert_eq!(v["error"], "only what the Explorer deleted is put back");
        let escape = format!("{}/../../x", s.deleted().display());
        assert!(s.restore(&escape, "x").0["error"].is_string());
        assert!(!d.path().join("x").exists());
    }
}
