//! Output styles: how the agent talks, in the common format. A style is a Markdown file `output-styles/NAME.md`
//! (the project's, yours, a plugin's) with front matter `name`, `description` and `keep-coding-instructions`; its
//! text goes into the system prompt. Without `keep-coding-instructions: true` it takes the place of Code Arena's own
//! coding instructions (a style for other work); with it, it is added to them. Three are built in: default,
//! explanatory and learning.

use std::path::PathBuf;

use crate::library::{Source, front_matter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    pub name: String,
    pub description: String,
    pub text: String,
    /// Code Arena's coding instructions kept, the style's added to them.
    pub keep_coding: bool,
    /// Whose: "built in", or the source's label.
    pub from: String,
    pub file: Option<PathBuf>,
}

const EXPLANATORY: &str = "While you work, teach a little about the code as you go. Before and after a change, add a short \
\"Insight\" (two or three points) on why the code is as it is: the choices made in this codebase, the patterns it uses, \
what a change here affects elsewhere. Keep them about this code, not general lessons, and keep the work itself as \
brief as before.";

const LEARNING: &str = "Work with the person so they learn by doing. Do the larger part yourself, but where a piece of the \
change is small and teaches something (a function's core, a condition, a test case: five to ten lines), leave it to \
them: write the code around it, mark the place with a `TODO(human)` comment saying what is wanted, and stop to ask \
them to write it. Add a short \"Insight\" on the choices in this codebase as you go. Never leave more than one \
TODO(human) at a time.";

/// The built-in styles: default (Code Arena as it is), explanatory, learning.
pub fn builtin() -> Vec<Style> {
    let b = |name: &str, description: &str, text: &str| Style {
        name: name.into(),
        description: description.into(),
        text: text.into(),
        keep_coding: true,
        from: "built in".into(),
        file: None,
    };
    vec![
        b("default", "Code Arena as it is: does the work, says briefly what it did.", ""),
        b("explanatory", "Does the work and explains the codebase's choices as it goes (Insights).", EXPLANATORY),
        b(
            "learning",
            "Works with you: leaves small pieces for you to write (TODO(human)) and explains as it goes.",
            LEARNING,
        ),
    ]
}

/// Every style there is, best first: the sources' (the project's win over yours), then the built-in ones. A name is
/// taken once, by the first that has it.
pub fn all(sources: &[Source]) -> Vec<Style> {
    let mut out: Vec<Style> = Vec::new();
    for src in sources {
        let Ok(rd) = std::fs::read_dir(src.dir.join("output-styles")) else { continue };
        let mut files: Vec<PathBuf> =
            rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
        files.sort();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            let (meta, body) = front_matter(&text);
            let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let name = meta.get("name").cloned().filter(|n| !n.is_empty()).unwrap_or(stem);
            if out.iter().any(|s| s.name.eq_ignore_ascii_case(&name)) {
                continue;
            }
            out.push(Style {
                name,
                description: meta.get("description").cloned().unwrap_or_default(),
                text: body.trim().to_string(),
                keep_coding: meta.get("keep-coding-instructions").is_some_and(|v| v.trim() == "true"),
                from: src.label.clone(),
                file: Some(f),
            });
        }
    }
    for b in builtin() {
        if !out.iter().any(|s| s.name.eq_ignore_ascii_case(&b.name)) {
            out.push(b);
        }
    }
    out
}

/// The style of that name (any case), if there is one.
pub fn find(sources: &[Source], name: &str) -> Option<Style> {
    all(sources).into_iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_projects_style_wins_and_the_built_in_ones_are_there() {
        let d = tempfile::tempdir().expect("temp");
        let project = d.path().join("p");
        let yours = d.path().join("y");
        for dir in [&project, &yours] {
            std::fs::create_dir_all(dir.join("output-styles")).expect("dir");
        }
        std::fs::write(
            project.join("output-styles/terse.md"),
            "---\nname: Terse\ndescription: One line.\n---\nAnswer in one line.\n",
        )
        .expect("w");
        std::fs::write(yours.join("output-styles/terse.md"), "---\nname: terse\n---\nMine.\n").expect("w");
        std::fs::write(
            yours.join("output-styles/explanatory.md"),
            "---\ndescription: My own.\nkeep-coding-instructions: true\n---\nExplain more.\n",
        )
        .expect("w");
        let sources =
            vec![Source { label: "this project".into(), dir: project }, Source { label: "yours".into(), dir: yours }];
        let all = all(&sources);
        let terse = all.iter().find(|s| s.name == "Terse").expect("terse");
        assert_eq!(
            (terse.text.as_str(), terse.keep_coding, terse.from.as_str()),
            ("Answer in one line.", false, "this project")
        );
        assert_eq!(all.iter().filter(|s| s.name.eq_ignore_ascii_case("terse")).count(), 1);
        let ex = find(&sources, "Explanatory").expect("ex");
        assert_eq!((ex.text.as_str(), ex.keep_coding), ("Explain more.", true));
        assert!(find(&sources, "learning").is_some_and(|s| s.from == "built in"));
        assert!(find(&sources, "default").is_some());
    }
}
