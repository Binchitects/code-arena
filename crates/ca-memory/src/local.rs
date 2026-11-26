//! The memory files on this machine, for a session without Arena: the person's (`memory.md` in the config folder, and
//! `~/.code-arena/memory.md`), and this project's (in the data folder, never in the repository). Each memory is a line
//! `- …` the person may edit; they go to Arena when it next answers.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The most a memory file gives the prompt.
pub const MAX_CHARS: usize = 12_000;

#[derive(Debug, Clone)]
pub struct Files {
    pub person: PathBuf,
    pub home: Option<PathBuf>,
    pub project: PathBuf,
    /// Arena's block, as it last gave it: for a session without Arena.
    pub cache: PathBuf,
}

impl Files {
    pub fn new(config: &Path, data: &Path, home: Option<&Path>, project_root: &Path) -> Self {
        let key = crate::keys::local_key(project_root);
        Files {
            person: config.join("memory.md"),
            home: home.map(|h| h.join(".code-arena").join("memory.md")).filter(|h| *h != config.join("memory.md")),
            project: data.join("memory").join(format!("{key}.md")),
            cache: data.join("memory").join(format!("{key}.arena.json")),
        }
    }

    fn read_file(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .map(|t| {
                t.lines()
                    .filter_map(|l| l.trim_start().strip_prefix("- "))
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The memories, in order: the person's (both files), or this project's.
    pub fn read(&self, person: bool) -> Vec<String> {
        if !person {
            return Self::read_file(&self.project);
        }
        let mut all = Self::read_file(&self.person);
        if let Some(h) = &self.home {
            for m in Self::read_file(h) {
                if !all.contains(&m) {
                    all.push(m);
                }
            }
        }
        all
    }

    /// Adds a memory (one line); `false` when the same is there already.
    pub fn add(&self, person: bool, text: &str) -> std::io::Result<bool> {
        let line = text.replace(['\n', '\r'], " ").trim().to_string();
        if line.is_empty() || self.read(person).iter().any(|m| m.eq_ignore_ascii_case(&line)) {
            return Ok(false);
        }
        let file = if person { &self.person } else { &self.project };
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut text = std::fs::read_to_string(file).unwrap_or_else(|_| {
            if person {
                "# What Code Arena remembers about you (every project)\n\n".into()
            } else {
                "# What Code Arena remembers about this project\n\n".into()
            }
        });
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("- {line}\n"));
        std::fs::write(file, text)?;
        Ok(true)
    }

    /// Both as the prompt gives them; empty when there are none.
    pub fn for_prompt(&self) -> String {
        let mut out = String::new();
        for (person, title) in [
            (true, "What you remember about the person (every project)"),
            (false, "What you remember about this project"),
        ] {
            let all = self.read(person);
            if all.is_empty() {
                continue;
            }
            let mut text = all.iter().map(|m| format!("- {m}")).collect::<Vec<_>>().join("\n");
            if text.len() > MAX_CHARS {
                let cut = text.char_indices().take_while(|(i, _)| *i < MAX_CHARS).last().map(|(i, _)| i).unwrap_or(0);
                text.truncate(cut);
                text.push_str("\n… (cut: the file is longer)");
            }
            out.push_str(&format!("\n# {title}\n\n{text}\n"));
        }
        out
    }

    /// The files, gone to Arena: kept beside as `.imported`.
    pub fn imported(&self) {
        for f in [Some(&self.person), self.home.as_ref(), Some(&self.project)].into_iter().flatten() {
            if !f.exists() {
                continue;
            }
            let done = f.with_extension("md.imported");
            let before = std::fs::read_to_string(&done).unwrap_or_default();
            let now = std::fs::read_to_string(f).unwrap_or_default();
            if std::fs::write(&done, before + &now).is_ok() {
                let _ = std::fs::remove_file(f);
            }
        }
    }

    /// The memories that share words with a question, best first (what recall is without Arena).
    pub fn find(&self, query: &str, besides: &[String], most: usize) -> Vec<String> {
        let q = words(query);
        let mut scored: Vec<(f64, String)> = self
            .read(true)
            .into_iter()
            .chain(self.read(false))
            .filter(|m| !besides.contains(m))
            .map(|m| (score(&q, &words(&m)), m))
            .filter(|(s, _)| *s >= 0.2)
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().take(most).map(|(_, m)| m).collect()
    }
}

const COMMON: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "what", "which", "how", "are", "was", "were", "has", "have",
    "its", "into", "about", "when", "where", "who", "why", "does", "did", "can", "not", "but", "you", "your", "they",
    "their", "there", "our", "all", "use", "please",
];

/// The words that carry meaning, in lower case.
pub fn words(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .filter(|w| w.chars().count() >= 3 && !COMMON.contains(w))
        .map(str::to_string)
        .collect()
}

fn score(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    a.iter().filter(|w| b.contains(*w)).count() as f64 / ((a.len() * b.len()) as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_once_read_back_found_by_words_and_imported() {
        let d = tempfile::tempdir().expect("temp");
        let f = Files::new(&d.path().join("config"), &d.path().join("data"), None, &d.path().join("repo"));
        assert!(f.add(false, "The tests run with cargo nextest, not cargo test").expect("add"));
        assert!(!f.add(false, "the tests run with cargo nextest, not cargo test").expect("again"));
        assert!(f.add(true, "Answers in short sentences").expect("add"));
        assert_eq!(f.read(false), vec!["The tests run with cargo nextest, not cargo test"]);
        let p = f.for_prompt();
        assert!(p.contains("# What you remember about the person (every project)\n\n- Answers in short sentences"));
        assert!(p.contains("# What you remember about this project"));
        assert_eq!(f.find("how do I run the tests?", &[], 5), vec!["The tests run with cargo nextest, not cargo test"]);
        assert!(f.find("deploy the website", &[], 5).is_empty());
        f.imported();
        assert!(f.read(false).is_empty() && f.read(true).is_empty());
        assert!(f.project.with_extension("md.imported").exists());
    }
}
