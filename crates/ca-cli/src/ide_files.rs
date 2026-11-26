//! Coder mode's side of the IDE's stream: the project's files (a folder listed, a file opened and saved) and its
//! terminals. They answer while a turn runs too. Paths come relative to the project only, each part resolved as the
//! system resolves it (a link that leads out is refused); only plain files are read or written, never a pipe or a
//! device, and not a binary file or one over 5 MB.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::broadcast;

/// The largest file the editor opens.
const MOST_BYTES: u64 = 5 * 1024 * 1024;

/// The most entries a folder lists.
const MOST_ENTRIES: usize = 5000;

struct Term {
    pty: ca_pty::Pty,
    writer: Box<dyn Write + Send>,
}

/// What Coder mode needs, apart from the session: the workspace, the terminals, the stream.
pub struct Side {
    ws: Arc<ca_workspace::Workspace>,
    checkpoints: Arc<ca_tools::checkpoints::Checkpoints>,
    out: broadcast::Sender<String>,
    terms: Arc<Mutex<HashMap<u64, Term>>>,
    shell: PathBuf,
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
    ) -> Side {
        let shell = shell
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }));
        Side { ws, checkpoints, out, terms: Arc::default(), shell }
    }

    /// The real path a page's path names: relative to the project, inside it.
    fn real(&self, rel: &str) -> Result<PathBuf, String> {
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

    fn shown(&self, real: &Path) -> String {
        real.strip_prefix(self.ws.root()).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default()
    }

    /// A message of Coder mode's, answered; false when it is none of them.
    pub fn handle(&self, m: &Value) -> bool {
        let path = m["path"].as_str().unwrap_or("");
        match m["type"].as_str().unwrap_or("") {
            "ls" => send(&self.out, self.ls(path)),
            "open" => send(&self.out, self.open(path)),
            "save" => send(&self.out, self.save(path, m["text"].as_str().unwrap_or(""))),
            "stat" => send(&self.out, self.stat(m)),
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
        if let Err(e) = std::fs::write(&real, text) {
            return fail(&e.to_string());
        }
        let m = std::fs::metadata(&real).map(|m| modified(&m)).unwrap_or(0);
        json!({"type": "saved", "path": self.shown(&real), "modified": m})
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
        if let Ok(mut t) = self.terms.lock() {
            t.insert(id, Term { pty, writer });
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
        (d, Side::new(ws, Arc::default(), out, None), rx)
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
        let s = Side::new(ws.clone(), cps.clone(), out, None);
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
}
