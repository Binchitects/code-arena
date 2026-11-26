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
    pub fn new(ws: Arc<ca_workspace::Workspace>, out: broadcast::Sender<String>, shell: Option<&str>) -> Side {
        let shell = shell
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }));
        Side { ws, out, terms: Arc::default(), shell }
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
        (d, Side::new(ws, out, None), rx)
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
