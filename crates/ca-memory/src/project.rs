//! The project's long memory, kept in the repository: `.remember/MEMORY.md` (what the project is, how it builds, tests
//! and runs, its conventions, decisions and why, where the work stands, lessons), with topic files beside it. Read at
//! each session's start (MEMORY.md whole, up to a size; the topic files listed), kept current by the agent with
//! `remember_project`, and shared with the team when the folder is committed.

use std::path::{Path, PathBuf};

pub const FOLDER: &str = ".remember";
pub const MAIN: &str = "MEMORY.md";
/// What of MEMORY.md the prompt takes, and how many topic files it lists.
const PROMPT_CHARS: usize = 24_000;
const MOST_LISTED: usize = 40;
/// The most one section may hold, and a whole file.
pub const SECTION_CHARS: usize = 20_000;
pub const FILE_CHARS: usize = 200_000;

const INSTRUCTIONS: &str = "This project keeps its long memory in .remember/, in the repository: what a session learns, the next one starts from. MEMORY.md is read at every session's start. Keep it current with remember_project as you work, not at the end: how to build, test and run it, conventions, decisions and why, where the work stands and what is next, lessons (what went wrong, what the person corrected). Keep MEMORY.md short, an index of a few hundred lines: put details in topic files (.remember/<topic>.md, e.g. architecture.md) and name them there. Write facts, not a diary; replace what is out of date (replace: true). Never secrets, keys or passwords.";

const NONE_YET: &str = "This project has no long memory yet. When you learn what a later session should know (how it builds and tests, a decision and why, where the work stands), keep it with remember_project: it starts .remember/MEMORY.md, in the repository.";

/// Where a folder's project memory is: at its repository's root (a session in a subfolder shares it), else the folder.
pub fn root(folder: &Path) -> PathBuf {
    crate::keys::git_root(folder).unwrap_or_else(|| folder.to_path_buf())
}

/// The prompt's part: MEMORY.md as it is now (cut past a size) and the topic files, or how to start it.
pub fn for_prompt(root: &Path) -> String {
    let folder = root.join(FOLDER);
    let main = folder.join(MAIN);
    let Ok(text) = std::fs::read_to_string(&main) else {
        return format!("\n# Project memory ({FOLDER}/)\n\n{NONE_YET}\n");
    };
    let mut text = text.trim().to_string();
    if text.len() > PROMPT_CHARS {
        let total = text.len();
        let cut = text.char_indices().take_while(|(i, _)| *i < PROMPT_CHARS).last().map(|(i, _)| i).unwrap_or(0);
        text.truncate(cut);
        text.push_str(&format!("\n\n[… MEMORY.md goes on ({total} characters): read_file {FOLDER}/{MAIN} for the rest, and keep it shorter: move details into topic files.]"));
    }
    let mut md = format!(
        "\n# Project memory ({FOLDER}/)\n\n{INSTRUCTIONS}\n\n## {FOLDER}/{MAIN}, as this session started\n\n{text}\n"
    );
    let topics = topics(&folder, &main);
    if !topics.is_empty() {
        md.push_str("\n## Its topic files (read_file one when it matters)\n");
        for t in topics.iter().take(MOST_LISTED) {
            md.push_str(&format!("- {t}\n"));
        }
        if topics.len() > MOST_LISTED {
            md.push_str("- … and more (list_dir .remember).\n");
        }
    }
    md
}

/// The topic files: each with its first line.
fn topics(folder: &Path, main: &Path) -> Vec<String> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "md") && p != main {
                files.push(p);
            }
        }
    }
    files.sort();
    files
        .iter()
        .map(|f| {
            let first = std::fs::read_to_string(f)
                .ok()
                .and_then(|t| {
                    t.lines().map(|l| l.trim().trim_start_matches('#').trim().to_string()).find(|l| !l.is_empty())
                })
                .unwrap_or_default();
            let rel = f.strip_prefix(folder).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            let first: String = first.chars().take(100).collect();
            format!("{FOLDER}/{rel}: {first}")
        })
        .collect()
}

/// A memory file's path inside `.remember` (MEMORY.md, or a topic file), or why it is not one.
pub fn file_of(root: &Path, name: Option<&str>) -> Result<PathBuf, String> {
    let mut file = name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(MAIN).replace('\\', "/");
    if let Some(rest) = file.strip_prefix(&format!("{FOLDER}/")) {
        file = rest.to_string();
    }
    if !file.to_ascii_lowercase().ends_with(".md") || file.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
        return Err(format!("A memory file is a Markdown file inside {FOLDER}/ (e.g. MEMORY.md, architecture.md)."));
    }
    Ok(root.join(FOLDER).join(file))
}

/// A section of a memory file added to (or replaced): the file and the section made when they are not there yet. The
/// file's text after.
pub fn keep(existing: &str, file_name: &str, section: &str, text: &str, replace: bool) -> String {
    let heading = format!("## {}", section.trim().trim_start_matches('#').trim());
    let mut lines: Vec<String> = if existing.is_empty() {
        if file_name == MAIN {
            vec![
                "# Project memory".into(),
                String::new(),
                "What sessions of Code Arena learned about this project, for the next ones (kept with remember_project; edit it freely).".into(),
            ]
        } else {
            let stem =
                Path::new(file_name).file_stem().map(|s| s.to_string_lossy().replace('-', " ")).unwrap_or_default();
            vec![format!("# {stem}")]
        }
    } else {
        existing.replace("\r\n", "\n").trim_end_matches('\n').split('\n').map(str::to_string).collect()
    };
    let body: Vec<String> = text.replace("\r\n", "\n").trim_matches('\n').split('\n').map(str::to_string).collect();
    let Some(at) = lines.iter().position(|l| l.trim().eq_ignore_ascii_case(&heading)) else {
        lines.push(String::new());
        lines.push(heading);
        lines.push(String::new());
        lines.extend(body);
        return lines.join("\n") + "\n";
    };
    // The section runs to the next heading of its level or above.
    let end = lines[at + 1..]
        .iter()
        .position(|l| l.starts_with("# ") || l.starts_with("## "))
        .map(|p| at + 1 + p)
        .unwrap_or(lines.len());
    if replace {
        let mut new = vec![String::new()];
        new.extend(body);
        new.push(String::new());
        lines.splice(at + 1..end, new);
    } else {
        // After the section's last line with words, so blank lines stay where they were.
        let mut last = end - 1;
        while last > at && lines[last].trim().is_empty() {
            last -= 1;
        }
        let mut new = Vec::new();
        if last == at {
            new.push(String::new());
        }
        new.extend(body);
        lines.splice(last + 1..last + 1, new);
    }
    lines.join("\n").trim_end_matches('\n').to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_are_made_added_to_and_replaced() {
        let t = keep("", MAIN, "How to build", "cargo build", false);
        assert!(t.starts_with("# Project memory\n"));
        assert!(t.ends_with("## How to build\n\ncargo build\n"));
        let t = keep(&t, MAIN, "Decisions", "SQLite for sessions: one file, WAL.", false);
        let t = keep(&t, MAIN, "how to build", "make ci runs the gate", false);
        assert!(t.contains("## How to build\n\ncargo build\nmake ci runs the gate\n\n## Decisions"), "{t}");
        let t = keep(&t, MAIN, "How to build", "make ci", true);
        assert!(t.contains("## How to build\n\nmake ci\n\n## Decisions"), "{t}");
        assert!(!t.contains("cargo build"));
        let topic = keep("", "architecture.md", "Crates", "ca-core first", false);
        assert!(topic.starts_with("# architecture\n"));
    }

    #[test]
    fn only_markdown_files_inside_the_folder() {
        let r = Path::new("/repo");
        assert_eq!(file_of(r, None), Ok(PathBuf::from("/repo/.remember/MEMORY.md")));
        assert_eq!(file_of(r, Some(".remember/arch.md")), Ok(PathBuf::from("/repo/.remember/arch.md")));
        assert!(file_of(r, Some("../notes.md")).is_err());
        assert!(file_of(r, Some("x.txt")).is_err());
    }

    #[test]
    fn the_prompt_reads_it_whole_and_lists_the_topics() {
        let d = tempfile::tempdir().expect("temp");
        assert!(for_prompt(d.path()).contains("no long memory yet"));
        std::fs::create_dir_all(d.path().join(FOLDER)).expect("mkdir");
        std::fs::write(d.path().join(FOLDER).join(MAIN), "# Project memory\n\n## How to build\n\nmake ci\n")
            .expect("w");
        std::fs::write(d.path().join(FOLDER).join("architecture.md"), "# The crates\n").expect("w");
        let p = for_prompt(d.path());
        assert!(p.contains("## How to build\n\nmake ci"));
        assert!(p.contains("- .remember/architecture.md: The crates"));
    }
}
