//! Commands, skills and sub-agents written as Markdown files: the project's (`.code-arena/`, `.arena/`, and
//! Claude Code's `.claude/` and Qwen Code's `.qwen/`) and the person's (the config folder, `~/.code-arena`,
//! `~/.claude`). The project's win over the person's of the same name. Read again at each turn, so one added or
//! changed works from the next message.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ca_llm::ToolDef;
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::{Value, json};

/// Whose a file is, as `/commands` and `/skills` say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub label: String,
    pub dir: PathBuf,
}

/// The folders read, best first.
pub fn sources(repo_root: &Path, config_dir: &Path, home: Option<&Path>) -> Vec<Source> {
    let mut s = Vec::new();
    for name in [".code-arena", ".arena", ".claude", ".qwen"] {
        s.push(Source { label: format!("this project ({name})"), dir: repo_root.join(name) });
    }
    s.push(Source { label: "yours".into(), dir: config_dir.to_path_buf() });
    if let Some(h) = home {
        s.push(Source { label: "yours (~/.code-arena)".into(), dir: h.join(".code-arena") });
        s.push(Source { label: "yours (~/.claude)".into(), dir: h.join(".claude") });
    }
    s
}

/// A file's front matter (`---` lines) as keys, and the text after it.
pub fn front_matter(text: &str) -> (BTreeMap<String, String>, String) {
    let mut map = BTreeMap::new();
    let t = text.trim_start_matches('\u{feff}');
    let Some(rest) = t.strip_prefix("---") else { return (map, t.to_string()) };
    let rest = rest.trim_start_matches(['\r', '\n']);
    let Some(end) = rest.find("\n---") else { return (map, t.to_string()) };
    let head = &rest[..end];
    let body = rest[end + 4..].trim_start_matches(|c: char| c != '\n').trim_start_matches('\n');
    let mut last_key: Option<String> = None;
    for line in head.lines() {
        if let Some(item) = line.trim_start().strip_prefix("- ")
            && let Some(k) = &last_key
        {
            // A YAML list under the last key: kept as `a, b`.
            let entry = map.entry(k.clone()).or_insert_with(String::new);
            if !entry.is_empty() {
                entry.push_str(", ");
            }
            entry.push_str(item.trim().trim_matches(['"', '\'']));
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().trim_matches(['"', '\'']);
            let v = v.strip_prefix('[').and_then(|x| x.strip_suffix(']')).unwrap_or(v);
            map.insert(k.trim().to_string(), v.to_string());
            last_key = Some(k.trim().to_string());
        }
    }
    (map, body.to_string())
}

/// `/NAME`: a prompt in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub argument_hint: String,
    pub prompt: String,
    pub path: PathBuf,
    pub source: String,
}

impl Command {
    /// The prompt with what was typed after the name: `$ARGUMENTS` is all of it, `$1`, `$2` its words; a prompt with
    /// neither gets it after.
    pub fn expand(&self, args: &str) -> String {
        expand(&self.prompt, args)
    }
}

pub fn expand(prompt: &str, args: &str) -> String {
    let words: Vec<&str> = args.split_whitespace().collect();
    let mut out = prompt.replace("$ARGUMENTS", args);
    let mut used = out != prompt;
    for i in (1..=9).rev() {
        let key = format!("${i}");
        if out.contains(&key) {
            used = true;
            out = out.replace(&key, words.get(i - 1).copied().unwrap_or(""));
        }
    }
    if !used && !args.trim().is_empty() {
        out.push_str("\n\n");
        out.push_str(args.trim());
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubAgent {
    pub name: String,
    pub description: String,
    /// The tools it may use, of those its task gives it (empty: as given).
    pub tools: Vec<String>,
    pub model: Option<String>,
    pub instructions: String,
    pub path: PathBuf,
    pub source: String,
}

/// Everything found, by name, the best source first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Library {
    pub commands: Vec<Command>,
    pub skills: Vec<Skill>,
    pub agents: Vec<SubAgent>,
}

fn md_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            md_files(&p, &format!("{prefix}{name}:"), out);
        } else if let Some(stem) = name.strip_suffix(".md") {
            out.push((format!("{prefix}{stem}"), p));
        }
    }
}

/// Reads every source; a file of more than 200 KB is not read.
pub fn load(sources: &[Source]) -> Library {
    let mut lib = Library::default();
    let read = |p: &Path| {
        std::fs::metadata(p).ok().filter(|m| m.len() <= 200_000).and_then(|_| std::fs::read_to_string(p).ok())
    };
    for src in sources {
        let mut cmds = Vec::new();
        md_files(&src.dir.join("commands"), "", &mut cmds);
        for (name, path) in cmds {
            if lib.commands.iter().any(|c| c.name == name) {
                continue;
            }
            let Some(text) = read(&path) else { continue };
            let (fm, body) = front_matter(&text);
            lib.commands.push(Command {
                name,
                description: fm.get("description").cloned().unwrap_or_default(),
                argument_hint: fm.get("argument-hint").cloned().unwrap_or_default(),
                prompt: body.trim().to_string(),
                path,
                source: src.label.clone(),
            });
        }
        if let Ok(rd) = std::fs::read_dir(src.dir.join("skills")) {
            let mut dirs: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.join("SKILL.md").is_file()).collect();
            dirs.sort();
            for d in dirs {
                let path = d.join("SKILL.md");
                let Some(text) = read(&path) else { continue };
                let (fm, _) = front_matter(&text);
                let name = fm
                    .get("name")
                    .cloned()
                    .unwrap_or_else(|| d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                if name.is_empty() || lib.skills.iter().any(|s| s.name == name) {
                    continue;
                }
                lib.skills.push(Skill {
                    name,
                    description: fm.get("description").cloned().unwrap_or_default(),
                    path,
                    source: src.label.clone(),
                });
            }
        }
        let mut agents = Vec::new();
        md_files(&src.dir.join("agents"), "", &mut agents);
        for (file_name, path) in agents {
            let Some(text) = read(&path) else { continue };
            let (fm, body) = front_matter(&text);
            let name = fm.get("name").cloned().unwrap_or(file_name);
            if lib.agents.iter().any(|a| a.name == name) {
                continue;
            }
            let tools = fm
                .get("tools")
                .map(|t| t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
                .unwrap_or_default();
            lib.agents.push(SubAgent {
                name,
                description: fm.get("description").cloned().unwrap_or_default(),
                tools,
                model: fm.get("model").cloned().filter(|m| !m.is_empty() && m != "inherit"),
                instructions: body.trim().to_string(),
                path,
                source: src.label.clone(),
            });
        }
    }
    lib
}

impl Library {
    /// The note the model reads of the skills: a line for each, to load with `skill` when a task needs it.
    pub fn skills_note(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }
        let mut s = String::from(
            "Skills: ways of doing tasks, written down. When a task matches one, load it with the skill tool first and follow it.\n",
        );
        for k in &self.skills {
            s.push_str(&format!("- {}: {}\n", k.name, k.description.lines().next().unwrap_or("")));
        }
        Some(s)
    }

    /// `/NAME words`: a command (it wins over a skill of the same name), or a skill used by the person.
    pub fn slash(&self, line: &str) -> Option<String> {
        let rest = line.strip_prefix('/')?;
        let (name, args) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
        if let Some(c) = self.commands.iter().find(|c| c.name == name) {
            return Some(c.expand(args));
        }
        let k = self.skills.iter().find(|k| k.name == name)?;
        let text = std::fs::read_to_string(&k.path).ok()?;
        let (_, body) = front_matter(&text);
        let mut out = format!(
            "Use the skill {name}: follow its instructions below for this request.\n\n<skill name=\"{name}\">\n{}\n</skill>",
            expand(body.trim(), args)
        );
        if !args.is_empty() && !body.contains("$ARGUMENTS") {
            out.push_str(&format!("\n\nThe request: {args}"));
        }
        Some(out)
    }
}

/// The `skill` tool: a skill's instructions, and the files beside it, one at a time.
pub struct SkillTool {
    pub sources: Vec<Source>,
}

#[async_trait::async_trait]
impl Tool for SkillTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "skill".into(),
            description: "Loads a skill's instructions by its name (the skills are listed in the system prompt); with `file`, one of the files beside its SKILL.md.".into(),
            parameters: json!({"type": "object", "properties": {"name": {"type": "string"}, "file": {"type": "string", "description": "A file beside SKILL.md, by its path in the skill's folder."}}, "required": ["name"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        args.get("name").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    async fn run(&self, args: Value, _ctx: &Context) -> Output {
        let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
        let lib = load(&self.sources);
        let Some(k) = lib.skills.iter().find(|k| k.name == name) else {
            let names: Vec<&str> = lib.skills.iter().map(|k| k.name.as_str()).collect();
            return Output::error(format!("No skill named {name}. The skills: {}.", names.join(", ")));
        };
        let dir = k.path.parent().unwrap_or(Path::new("."));
        if let Some(file) = args.get("file").and_then(Value::as_str) {
            if file.split(['/', '\\']).any(|p| p == "..") || Path::new(file).is_absolute() {
                return Output::error("A skill's file is named by its path in the skill's folder.");
            }
            return match std::fs::read_to_string(dir.join(file)) {
                Ok(t) => Output::ok(t),
                Err(e) => Output::error(format!("{file}: {e}")),
            };
        }
        let Ok(text) = std::fs::read_to_string(&k.path) else {
            return Output::error(format!("{} could not be read", k.path.display()));
        };
        let (_, body) = front_matter(&text);
        let mut others = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n != "SKILL.md" {
                    others.push(n);
                }
            }
        }
        others.sort();
        let mut out = body.trim().to_string();
        if !others.is_empty() {
            out.push_str(&format!("\n\n(Files beside it, to read with skill and file: {}.)", others.join(", ")));
        }
        Output::ok(out)
    }
}

pub fn skill_tool(sources: Vec<Source>) -> Arc<dyn Tool> {
    Arc::new(SkillTool { sources })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, text).expect("write");
    }

    #[test]
    fn front_matter_with_lists() {
        let (fm, body) = front_matter(
            "---\nname: reviewer\ndescription: \"Reviews\"\ntools: [read_file, grep]\nmodel: inherit\nlist:\n  - a\n  - b\n---\nYou review.\n",
        );
        assert_eq!(fm["name"], "reviewer");
        assert_eq!(fm["description"], "Reviews");
        assert_eq!(fm["tools"], "read_file, grep");
        assert_eq!(fm["list"], "a, b");
        assert_eq!(body, "You review.\n");
        let (none, all) = front_matter("no front matter");
        assert!(none.is_empty());
        assert_eq!(all, "no front matter");
    }

    #[test]
    fn arguments_go_where_the_prompt_says() {
        assert_eq!(expand("Fix $ARGUMENTS now", "the parser"), "Fix the parser now");
        assert_eq!(expand("Compare $1 with $2", "a b"), "Compare a with b");
        assert_eq!(expand("Review this branch.", "carefully"), "Review this branch.\n\ncarefully");
        assert_eq!(expand("Review.", ""), "Review.");
    }

    #[test]
    fn the_projects_win_over_the_persons_and_folders_make_names() {
        let d = tempfile::tempdir().expect("temp");
        let repo = d.path().join("repo");
        let cfg = d.path().join("cfg");
        write(&repo.join(".code-arena/commands/review.md"), "---\ndescription: the project's\n---\nReview it.");
        write(&repo.join(".claude/commands/git/commit.md"), "Commit $ARGUMENTS");
        write(&cfg.join("commands/review.md"), "mine");
        write(
            &repo.join(".claude/skills/release/SKILL.md"),
            "---\nname: release\ndescription: How to release\n---\nStep 1: tag.",
        );
        write(&repo.join(".claude/skills/release/checklist.md"), "- tests");
        write(
            &cfg.join("agents/reviewer.md"),
            "---\nname: reviewer\ntools: read_file, grep\nmodel: qwen-small\n---\nYou review.",
        );
        let lib = load(&sources(&repo, &cfg, None));
        let names: Vec<&str> = lib.commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["review", "git:commit"]);
        assert_eq!(lib.commands[0].description, "the project's");
        assert_eq!(lib.slash("/git:commit the fix").as_deref(), Some("Commit the fix"));
        assert!(lib.slash("/release 2.0").expect("skill").contains("Step 1: tag."));
        assert_eq!(lib.agents[0].tools, vec!["read_file", "grep"]);
        assert_eq!(lib.agents[0].model.as_deref(), Some("qwen-small"));
        assert!(lib.skills_note().expect("note").contains("- release: How to release"));
        assert!(lib.slash("/nothing").is_none());
    }

    #[tokio::test]
    async fn the_skill_tool_reads_a_skill_and_its_files() {
        let d = tempfile::tempdir().expect("temp");
        write(&d.path().join(".code-arena/skills/release/SKILL.md"), "---\nname: release\n---\nTag it.");
        write(&d.path().join(".code-arena/skills/release/notes.md"), "Notes.");
        let t = SkillTool { sources: sources(d.path(), &d.path().join("cfg"), None) };
        let ws = ca_workspace::Workspace::open(d.path(), &[]).expect("ws");
        let ctx = Context::new(Arc::new(ws), None, std::time::Duration::from_secs(5));
        let o = t.run(json!({"name": "release"}), &ctx).await;
        assert_eq!(o.text, "Tag it.\n\n(Files beside it, to read with skill and file: notes.md.)");
        assert_eq!(t.run(json!({"name": "release", "file": "notes.md"}), &ctx).await.text, "Notes.");
        assert!(t.run(json!({"name": "release", "file": "../../x"}), &ctx).await.is_error);
    }
}
