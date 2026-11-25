//! The project's `.code-arena/` folder, made with sensible default files the first time a folder is opened.

use std::path::{Path, PathBuf};

/// The default files, by their path in `.code-arena/`.
const FILES: &[(&str, &str)] = &[
    ("README.md", include_str!("../templates/README.md")),
    ("settings.json", include_str!("../templates/settings.json")),
    ("mcp.json", include_str!("../templates/mcp.json")),
    (".gitignore", include_str!("../templates/gitignore")),
    ("commands/explain.md", include_str!("../templates/commands/explain.md")),
    ("agents/reviewer.md", include_str!("../templates/agents/reviewer.md")),
    ("skills/explain-module/SKILL.md", include_str!("../templates/skills/explain-module/SKILL.md")),
    ("checklists/before-a-merge.md", include_str!("../templates/checklists/before-a-merge.md")),
];

/// Folders made empty, for what each person or project adds.
const FOLDERS: &[&str] = &["tools", "hooks", "local"];

/// What [`ensure_project_folder`] made: nothing when the folder was there already.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Created {
    pub folder: PathBuf,
    pub files: Vec<PathBuf>,
}

/// Makes `.code-arena/` under `root` with its default files, when it is not there. A folder that is there is left
/// as it is: a file someone deleted is not brought back, nor one changed written over.
pub fn ensure_project_folder(root: &Path) -> std::io::Result<Created> {
    let folder = root.join(".code-arena");
    let mut created = Created { folder: folder.clone(), files: vec![] };
    if folder.exists() {
        return Ok(created);
    }
    std::fs::create_dir_all(&folder)?;
    for (rel, text) in FILES {
        let path = folder.join(rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // create_new: never write over a file, even one made a moment ago by another run.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                std::io::Write::write_all(&mut f, text.as_bytes())?;
                created.files.push(PathBuf::from(rel));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    for dir in FOLDERS {
        std::fs::create_dir_all(folder.join(dir))?;
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn made_once_with_its_files() {
        let dir = tempfile::tempdir().expect("temp");
        let c = ensure_project_folder(dir.path()).expect("made");
        assert_eq!(c.files.len(), FILES.len());
        let ignore = std::fs::read_to_string(dir.path().join(".code-arena/.gitignore")).expect("gitignore");
        assert!(ignore.lines().any(|l| l == "local/"));
        assert!(dir.path().join(".code-arena/local").is_dir());
        // The settings file is JSON with comments, as settings.json is.
        let settings = std::fs::read_to_string(dir.path().join(".code-arena/settings.json")).expect("settings");
        assert!(settings.contains("\"deny\": []"));

        std::fs::write(dir.path().join(".code-arena/settings.json"), "{}").expect("change");
        std::fs::remove_file(dir.path().join(".code-arena/mcp.json")).expect("delete");
        let again = ensure_project_folder(dir.path()).expect("again");
        assert!(again.files.is_empty());
        assert_eq!(std::fs::read_to_string(dir.path().join(".code-arena/settings.json")).expect("read"), "{}");
        assert!(!dir.path().join(".code-arena/mcp.json").exists());
    }
}
