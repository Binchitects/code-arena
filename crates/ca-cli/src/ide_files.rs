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
        ("term_open", Side),
        ("term_in", Side),
        ("term_resize", Side),
        ("terms", Side),
        ("term_close", Side),
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
pub struct Side {
    ws: Arc<ca_workspace::Workspace>,
    checkpoints: Arc<ca_tools::checkpoints::Checkpoints>,
    out: broadcast::Sender<String>,
    terms: Arc<Mutex<HashMap<u64, Term>>>,
    shell: PathBuf,
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

fn modified(meta: &std::fs::Metadata) -> u64 {
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
        let shell = shell
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }));
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
                let (query, regex, case) =
                    (m["query"].as_str().unwrap_or("").to_string(), m["regex"] == true, m["case"] == true);
                tokio::task::spawn_blocking(move || send(&out, search(&root, &query, regex, case)));
            }
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
                            .map(|id| json!({"id": id, "screen": t[id].screen.lock().map(|s| s.clone()).unwrap_or_default()}))
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
        let fail = |e: &str| json!({"type": "file", "path": path, "error": e});
        let real = match self.real(path) {
            Ok(r) => r,
            Err(e) => return fail(&e),
        };
        let Ok(meta) = std::fs::metadata(&real) else { return fail("not there") };
        if !meta.is_file() {
            return fail("not a plain file (a folder, a pipe or a device is not opened)");
        }
        if meta.len() > MOST_BYTES {
            return fail("bigger than 5 MB: open it with another program");
        }
        let mut bytes = Vec::new();
        if std::fs::File::open(&real).and_then(|mut f| f.read_to_end(&mut bytes)).is_err() {
            return fail("it could not be read");
        }
        if bytes.iter().take(8000).any(|b| *b == 0) {
            return fail("a binary file: open it with another program");
        }
        let Ok(text) = String::from_utf8(bytes) else { return fail("not UTF-8 text") };
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

    fn term_open(&self, m: &Value) {
        let id = m["id"].as_u64().unwrap_or(0);
        let cols = m["cols"].as_u64().unwrap_or(80) as u16;
        let rows = m["rows"].as_u64().unwrap_or(24) as u16;
        let env = vec![("TERM".to_string(), "xterm-256color".to_string()), ("COLORTERM".into(), "truecolor".into())];
        let pty = match ca_pty::Pty::spawn(&self.shell, &[], self.ws.cwd(), &env, cols, rows) {
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
            t.insert(id, Term { pty, writer, screen: screen.clone() });
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

/// Each line that holds `query` (a regex when `regex`; any case unless `case`), in the files git sees.
fn search(root: &Path, query: &str, regex: bool, case: bool) -> Value {
    use grep_searcher::{BinaryDetection, SearcherBuilder, sinks::UTF8};
    let fail = |e: String| json!({"type": "search", "query": query, "error": e});
    if query.is_empty() {
        return json!({"type": "search", "query": query, "hits": []});
    }
    let pattern = if regex { query.to_string() } else { regex_escape(query) };
    let matcher = match grep_regex::RegexMatcherBuilder::new().case_insensitive(!case).build(&pattern) {
        Ok(m) => m,
        Err(e) => return fail(e.to_string()),
    };
    let mut searcher = SearcherBuilder::new().binary_detection(BinaryDetection::quit(0)).line_number(true).build();
    let mut hits: Vec<Value> = Vec::new();
    let mut files = 0;
    let mut more = false;
    'walk: for e in walker(root).flatten() {
        if !e.file_type().is_some_and(|t| t.is_file()) || e.metadata().map(|m| m.len() > MOST_BYTES).unwrap_or(true) {
            continue;
        }
        let rel = e.path().strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        let mut found = Vec::new();
        let _ = searcher.search_path(
            &matcher,
            e.path(),
            UTF8(|line, text| {
                found.push((line, text.trim_end().chars().take(240).collect::<String>()));
                Ok(found.len() < MOST_HITS)
            }),
        );
        if !found.is_empty() {
            files += 1;
        }
        for (line, text) in found {
            if hits.len() >= MOST_HITS {
                more = true;
                break 'walk;
            }
            hits.push(json!({"path": rel, "line": line, "text": text}));
        }
    }
    json!({"type": "search", "query": query, "hits": hits, "files": files, "more": more})
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
        let r = search(d.path(), "main", false, false);
        let hits: Vec<String> = r["hits"]
            .as_array()
            .expect("hits")
            .iter()
            .map(|h| format!("{}:{}", h["path"].as_str().unwrap_or(""), h["line"]))
            .collect();
        assert_eq!(hits, ["src/lib.rs:1", "src/lib.rs:2", "src/main.rs:1"]);
        // As typed: the dot and the brackets are not a pattern; with case kept, "Main" only.
        assert_eq!(search(d.path(), "(a.b)", false, false)["hits"].as_array().map(Vec::len), Some(1));
        assert_eq!(search(d.path(), "Main", false, true)["hits"].as_array().map(Vec::len), Some(1));
        assert!(search(d.path(), "(", true, false)["error"].is_string());
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
}
