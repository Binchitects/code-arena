//! The instructions every session reads, and the system prompt built around them.

use std::path::{Path, PathBuf};

/// The names read in each folder, the first one there counting: a project written for another agent is read as
/// it is.
pub const NAMES: &[&str] = &["ARENA.md", "AGENTS.md", "CLAUDE.md", "QWEN.md"];

/// Each file is read up to this many characters.
pub const MOST_CHARS: usize = 20_000;

/// One file of instructions, as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    pub path: PathBuf,
    pub text: String,
    pub cut: bool,
}

/// Where the person's own instructions are looked for, in order: the config folder's `ARENA.md`, then the first of
/// the names in `~/.code-arena`, else `~/.claude/CLAUDE.md`. With `CODE_ARENA_HOME` set, the home's are not read.
pub fn person_candidates(config_dir: &Path, home: Option<&Path>) -> Vec<Vec<PathBuf>> {
    let mut groups = vec![vec![config_dir.join("ARENA.md")]];
    if let Some(h) = home {
        let mut g: Vec<PathBuf> = NAMES.iter().map(|n| h.join(".code-arena").join(n)).collect();
        g.push(h.join(".claude").join("CLAUDE.md"));
        groups.push(g);
    }
    groups
}

/// The person's, the repository root's and the working directory's (when it is below the root).
pub fn read_all(person: &[Vec<PathBuf>], root: &Path, cwd: &Path) -> Vec<Instruction> {
    let mut out = Vec::new();
    for group in person {
        if let Some(i) = first_of(group) {
            out.push(i);
        }
    }
    let in_folder = |dir: &Path| first_of(&NAMES.iter().map(|n| dir.join(n)).collect::<Vec<_>>());
    if let Some(i) = in_folder(root) {
        out.push(i);
    }
    if cwd != root
        && let Some(i) = in_folder(cwd)
    {
        out.push(i);
    }
    out
}

fn first_of(paths: &[PathBuf]) -> Option<Instruction> {
    paths.iter().find_map(|p| {
        let text = std::fs::read_to_string(p).ok()?;
        let cut = text.chars().count() > MOST_CHARS;
        let text = if cut { text.chars().take(MOST_CHARS).collect() } else { text };
        Some(Instruction { path: p.clone(), text, cut })
    })
}

/// What the model is told of where it works.
#[derive(Debug, Clone)]
pub struct Environment {
    pub os: String,
    pub shell: String,
    pub cwd: String,
    pub branch: Option<String>,
    pub date: String,
    pub mode: String,
    /// Whether a person can answer questions (false in `-p`).
    pub interactive: bool,
}

/// The system prompt: who it is, how it works, the instructions and where it is.
pub fn system_prompt(env: &Environment, instructions: &[Instruction]) -> String {
    let mut s = String::from(
        "You are Code Arena, Farzan Arena's coding agent. You work in the person's project on their machine: you read and change \
its code, and run commands, with the tools given.\n\n\
How to work:\n\
- Read before you change: find the code with grep and glob, read what matters with read_file.\n\
- Make the smallest change that does what was asked, in the project's own style. Change files with edit_file; write_file \
only for new files or a whole rewrite.\n\
- Check your work: run the project's tests or build with run_shell when there are some, and say what you ran and what it gave.\n\
- Calls that only read can go together in one answer; calls that change things run in the order given.\n\
- When a tool is refused, do not try to get around it: say what you would have done, or find another way.\n\
- Cite code as path:line. Be brief: say what you did and what is left, not every step.\n",
    );
    if !env.interactive {
        s.push_str("- Nobody can answer a question in this run: do the task as asked, and end with the answer.\n");
    }
    for i in instructions {
        s.push_str(&format!("\n<instructions from=\"{}\">\n{}\n", i.path.display(), i.text.trim_end()));
        if i.cut {
            s.push_str(&format!("(cut at {MOST_CHARS} characters)\n"));
        }
        s.push_str("</instructions>\n");
    }
    s.push_str(&format!(
        "\n<environment>\nSystem: {}\nShell: {}\nWorking directory: {}\n{}Date: {}\nMode: {}\n</environment>\n",
        env.os,
        env.shell,
        env.cwd,
        env.branch.as_ref().map(|b| format!("Git branch: {b}\n")).unwrap_or_default(),
        env.date,
        env.mode
    ));
    s
}

/// The branch checked out, read from `.git/HEAD` (no git run for it).
pub fn git_branch(root: &Path) -> Option<String> {
    let git = root.join(".git");
    let head_file = if git.is_file() {
        // A worktree: `.git` names the real folder.
        let text = std::fs::read_to_string(&git).ok()?;
        PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim()).join("HEAD")
    } else {
        git.join("HEAD")
    };
    let head = std::fs::read_to_string(head_file).ok()?;
    match head.trim().strip_prefix("ref: refs/heads/") {
        Some(b) => Some(b.to_string()),
        None => Some(format!("(detached at {})", head.trim().chars().take(12).collect::<String>())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_name_in_each_folder_counts() {
        let d = tempfile::tempdir().expect("temp");
        let cfg = d.path().join("cfg");
        let home = d.path().join("home");
        let root = d.path().join("repo");
        let sub = root.join("pkg");
        for p in [&cfg, &home.join(".claude"), &sub] {
            std::fs::create_dir_all(p).expect("mkdir");
        }
        std::fs::write(home.join(".claude/CLAUDE.md"), "home rules").expect("w");
        std::fs::write(root.join("AGENTS.md"), "agents").expect("w");
        std::fs::write(root.join("CLAUDE.md"), "claude").expect("w");
        std::fs::write(sub.join("ARENA.md"), "x".repeat(MOST_CHARS + 5)).expect("w");
        let got = read_all(&person_candidates(&cfg, Some(&home)), &root, &sub);
        let names: Vec<_> =
            got.iter().map(|i| i.path.strip_prefix(d.path()).expect("in").display().to_string()).collect();
        assert_eq!(names, vec!["home/.claude/CLAUDE.md", "repo/AGENTS.md", "repo/pkg/ARENA.md"]);
        assert!(got[2].cut);
        assert_eq!(got[2].text.len(), MOST_CHARS);
    }

    #[test]
    fn the_prompt_carries_instructions_and_environment() {
        let env = Environment {
            os: "linux".into(),
            shell: "bash".into(),
            cwd: "/w".into(),
            branch: Some("main".into()),
            date: "2026-11-26".into(),
            mode: "ask".into(),
            interactive: false,
        };
        let s = system_prompt(
            &env,
            &[Instruction { path: "/w/ARENA.md".into(), text: "Run make test.".into(), cut: false }],
        );
        assert!(s.contains("<instructions from=\"/w/ARENA.md\">\nRun make test.\n</instructions>"));
        assert!(s.contains("Git branch: main\n"));
        assert!(s.contains("Nobody can answer"));
    }

    #[test]
    fn the_branch_from_head() {
        let d = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(d.path().join(".git")).expect("mkdir");
        std::fs::write(d.path().join(".git/HEAD"), "ref: refs/heads/feature/x\n").expect("w");
        assert_eq!(git_branch(d.path()).as_deref(), Some("feature/x"));
    }
}
