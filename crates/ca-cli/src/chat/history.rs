//! What was sent in each folder, for ↑: a JSON Lines file per folder in the data folder, cut back to the last 1,000
//! messages once it holds 2,000. Shared with the IDE's chat in the same folder.

use std::io::Write;
use std::path::{Path, PathBuf};

const KEEP: usize = 1000;
const MOST: usize = 2000;

pub struct InputHistory {
    file: PathBuf,
}

/// A short, stable name for a folder (FNV-1a of its path).
fn folder_key(folder: &Path) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in folder.to_string_lossy().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').take(40).collect();
    format!("{name}-{h:016x}")
}

impl InputHistory {
    pub fn new(data_dir: &Path, folder: &Path) -> Self {
        Self { file: data_dir.join("history").join(format!("{}.jsonl", folder_key(folder))) }
    }

    /// Oldest first, each text once (its latest place).
    pub fn load(&self) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(&self.file) else { return vec![] };
        let mut out: Vec<String> = Vec::new();
        for line in text.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
                && let Some(t) = v.get("text").and_then(|t| t.as_str())
            {
                out.retain(|x| x != t);
                out.push(t.to_string());
            }
        }
        out
    }

    /// Adds a line; lines read from a pipe and `/exit` are not kept.
    pub fn add(&self, text: &str) {
        if text.trim().is_empty() || text.trim() == "/exit" {
            return;
        }
        if let Some(dir) = self.file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let line = serde_json::json!({"text": text, "at": chrono::Utc::now().to_rfc3339()}).to_string();
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        if let Ok(mut f) = opts.open(&self.file) {
            let _ = writeln!(f, "{line}");
        }
        self.trim();
    }

    fn trim(&self) {
        let Ok(text) = std::fs::read_to_string(&self.file) else { return };
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() < MOST {
            return;
        }
        let kept = lines[lines.len() - KEEP..].join("\n") + "\n";
        let tmp = self.file.with_extension("jsonl.tmp");
        if std::fs::write(&tmp, kept).is_ok() {
            let _ = std::fs::rename(tmp, &self.file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_per_folder_once_each_and_cut_back() {
        let d = tempfile::tempdir().expect("temp");
        let h = InputHistory::new(d.path(), Path::new("/src/shop"));
        h.add("one");
        h.add("two");
        h.add("one");
        h.add("/exit");
        assert_eq!(h.load(), vec!["two", "one"]);
        let other = InputHistory::new(d.path(), Path::new("/src/other"));
        assert!(other.load().is_empty());
        for i in 0..MOST {
            h.add(&format!("m{i}"));
        }
        let left = std::fs::read_to_string(&h.file).expect("read").lines().count();
        assert!((KEEP..=MOST).contains(&left), "{left}");
        assert_eq!(h.load().last().map(String::as_str), Some(format!("m{}", MOST - 1).as_str()));
    }
}
