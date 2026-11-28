//! The program's log: what Code Arena says while the IDE runs, each line with its time, kept in `code-arena.log`
//! so that a problem can be looked into afterwards (the native window has no terminal to read). It is beside the
//! program when that folder can be written and the program is an installed one, else in the data folder's `logs/`.
//! It rotates at [`MAX`] bytes, keeping [`KEPT`] older files (`code-arena.1.log` is the newest of them). The
//! terminal's colours are taken out of each line, and the IDE's keys never reach it: [`hide`] blanks them.
//!
//! Only the IDE opens it ([`open`], from `main`); until then, and in every other command, [`line`] does nothing.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The log's name.
pub const NAME: &str = "code-arena.log";
/// How large the log grows before it rotates.
pub const MAX: u64 = 5 * 1024 * 1024;
/// How many older logs are kept.
pub const KEPT: usize = 3;

/// The log, once opened.
static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

/// A log file being written.
pub struct Log {
    path: PathBuf,
    file: Option<File>,
    max: u64,
    secrets: Vec<String>,
}

impl Log {
    /// The log in the first place it can be written: beside `exe` (when given), else `data/logs/`. Rotated first
    /// when it is full. None when neither place can be written.
    pub fn open(exe_dir: Option<&Path>, data: &Path, max: u64) -> Option<Log> {
        let candidates = exe_dir.map(|d| d.join(NAME)).into_iter().chain([data.join("logs").join(NAME)]);
        for path in candidates {
            let Some(dir) = path.parent() else { continue };
            if std::fs::create_dir_all(dir).is_err() {
                continue;
            }
            if std::fs::metadata(&path).is_ok_and(|m| m.len() >= max) {
                rotate(&path);
            }
            if let Ok(file) = append(&path) {
                return Some(Log { path, file: Some(file), max, secrets: Vec::new() });
            }
        }
        None
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A secret blanked in every line from now on.
    pub fn hide(&mut self, secret: &str) {
        if !secret.is_empty() && !self.secrets.iter().any(|s| s == secret) {
            self.secrets.push(secret.to_string());
        }
    }

    /// What a line becomes in the file: its time, then its words without colours or secrets.
    fn written(&self, text: &str, now: &str) -> String {
        let mut text = plain(text);
        for s in &self.secrets {
            text = text.replace(s.as_str(), "…");
        }
        let text = text.replace('\r', "");
        format!("{now} {}\n", text.trim_end_matches('\n'))
    }

    /// A line written (and the log rotated once it is full); a full disk or the file taken away ends the log.
    pub fn line(&mut self, text: &str) {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f %:z").to_string();
        let line = self.written(text, &now);
        let Some(file) = self.file.as_mut() else { return };
        let full = match file.write_all(line.as_bytes()).and_then(|()| file.metadata()) {
            Ok(m) => m.len() >= self.max,
            Err(_) => {
                self.file = None;
                return;
            }
        };
        if full {
            self.file = None;
            rotate(&self.path);
            self.file = append(&self.path).ok();
        }
    }
}

/// The log opened for the IDE: beside the program when it is an installed one (not a build folder's), else in
/// the data folder. Once; a second call keeps the first.
pub fn open(data: &Path) -> Option<PathBuf> {
    if let Some(l) = LOG.get() {
        return l.lock().ok().map(|l| l.path.clone());
    }
    let exe = std::env::current_exe().ok();
    let beside = exe.as_deref().filter(|e| !built_here(e)).and_then(Path::parent).filter(|d| writable(d));
    let log = Log::open(beside, data, MAX)?;
    let path = log.path.clone();
    let _ = LOG.set(Mutex::new(log));
    Some(path)
}

/// Where the log is written; `None` while none is kept.
pub fn path() -> Option<PathBuf> {
    LOG.get().and_then(|l| l.lock().ok().map(|l| l.path.clone()))
}

/// A line in the log, when one is kept.
pub fn line(text: &str) {
    if let Some(l) = LOG.get()
        && let Ok(mut l) = l.lock()
    {
        l.line(text);
    }
}

/// A secret (a key) blanked in every line from now on.
pub fn hide(secret: &str) {
    if let Some(l) = LOG.get()
        && let Ok(mut l) = l.lock()
    {
        l.hide(secret);
    }
}

/// A panic said in the log too, whichever thread it was on (the window's or the session's).
pub fn on_panic() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        line(&format!("panic: {info}"));
        before(info);
    }));
}

/// Whether the program runs from a cargo build folder (`target/debug`, `target/release`): no log beside it then.
pub fn built_here(exe: &Path) -> bool {
    let parts: Vec<_> = exe.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    parts.windows(2).any(|w| w[0] == "target" && (w[1] == "debug" || w[1] == "release"))
        || parts.windows(3).any(|w| w[0] == "target" && (w[2] == "debug" || w[2] == "release"))
}

/// Whether a file can be made in a folder; the one made to find out is taken away again.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".code-arena-log-{}", std::process::id()));
    let ok = File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn append(path: &Path) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// `code-arena.N.log`, beside the log.
fn numbered(path: &Path, n: usize) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "code-arena".into());
    path.with_file_name(format!("{stem}.{n}.log"))
}

/// `code-arena.log` to `code-arena.1.log`, .1 to .2 and so on; the oldest beyond [`KEPT`] goes.
fn rotate(path: &Path) {
    let _ = std::fs::remove_file(numbered(path, KEPT));
    for n in (1..KEPT).rev() {
        let _ = std::fs::rename(numbered(path, n), numbered(path, n + 1));
    }
    let _ = std::fs::rename(path, numbered(path, 1));
}

/// The terminal's colours (`ESC [ … letter`) taken out.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            while let Some(&n) = chars.peek() {
                chars.next();
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_has_its_time_and_no_colours_or_keys() {
        let dir = tempfile::tempdir().expect("dir");
        let mut log = Log::open(None, dir.path(), MAX).expect("log");
        log.hide("sk-fake-123");
        assert_eq!(
            log.written("\u{1b}[1mCode Arena\u{1b}[0m key sk-fake-123 used\n", "2026-01-02 03:04:05.006 +00:00"),
            "2026-01-02 03:04:05.006 +00:00 Code Arena key … used\n"
        );
        log.line("one");
        let text = std::fs::read_to_string(log.path()).expect("read");
        assert!(text.ends_with(" one\n"), "{text}");
        assert_eq!(log.path(), dir.path().join("logs").join(NAME));
    }

    #[test]
    fn it_rotates_when_full_and_keeps_three() {
        let dir = tempfile::tempdir().expect("dir");
        let mut log = Log::open(Some(dir.path()), dir.path(), 64).expect("log");
        assert_eq!(log.path(), dir.path().join(NAME));
        for i in 0..12 {
            log.line(&format!("line {i} with some words to fill it"));
        }
        let names: Vec<String> = {
            let mut n: Vec<String> = std::fs::read_dir(dir.path())
                .expect("dir")
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into())
                .collect();
            n.sort();
            n
        };
        assert_eq!(names, ["code-arena.1.log", "code-arena.2.log", "code-arena.3.log", "code-arena.log"]);
        let newest = std::fs::read_to_string(dir.path().join("code-arena.1.log")).expect("read");
        assert!(newest.contains("line 11"), "{newest}");
    }

    #[test]
    fn a_full_log_is_rotated_when_opened() {
        let dir = tempfile::tempdir().expect("dir");
        std::fs::write(dir.path().join(NAME), "x".repeat(100)).expect("write");
        let log = Log::open(Some(dir.path()), dir.path(), 64).expect("log");
        assert_eq!(std::fs::metadata(log.path()).expect("meta").len(), 0);
        assert!(dir.path().join("code-arena.1.log").exists());
    }

    #[test]
    fn a_build_folder_is_known() {
        assert!(built_here(Path::new("/w/target/debug/code-arena")));
        assert!(built_here(Path::new("/w/target/x86_64-unknown-linux-musl/release/code-arena")));
        assert!(!built_here(Path::new("/opt/code-arena/code-arena")));
    }
}
