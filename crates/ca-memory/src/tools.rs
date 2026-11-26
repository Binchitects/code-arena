//! The memory's tools: `remember`, `recall`, `remember_project`, `search_history`, `open_chat` and `skill_save`.

use std::sync::Arc;

use ca_llm::ToolDef;
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::{Value, json};

use crate::{Memory, project};

/// The tools a session has: those of Arena's memory only when it may use them.
pub fn tools(memory: Arc<Memory>) -> Vec<Arc<dyn Tool>> {
    let mut t: Vec<Arc<dyn Tool>> =
        vec![Arc::new(RememberProject { memory: memory.clone() }), Arc::new(SkillSave { memory: memory.clone() })];
    if !memory.denied() {
        t.push(Arc::new(Remember { memory: memory.clone() }));
        t.push(Arc::new(Recall { memory: memory.clone() }));
    }
    if memory.arena().is_some() {
        t.push(Arc::new(SearchHistory { memory: memory.clone() }));
        t.push(Arc::new(OpenChat { memory }));
    }
    t
}

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

struct Remember {
    memory: Arc<Memory>,
}

#[async_trait::async_trait]
impl Tool for Remember {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "remember".into(),
            description: "Keep something for later sessions: about this project (how to build or test it, a convention, a decision and why, a lesson from something that went wrong), or about the person (a preference for every project, with scope person). One or two short sentences each; not secrets, not what is in the code already. The person sees it and can edit or forget it (/memory, and Memory in Arena).".into(),
            parameters: json!({"type": "object", "properties": {
                "memory": {"type": "string", "description": "One or two sentences, as they should be read in a later session."},
                "scope": {"type": "string", "enum": ["project", "person"], "description": "project (the default): this project only; person: every project."},
                "kind": {"type": "string", "enum": ["preference", "fact", "decision", "howto", "lesson"], "description": "preference (how the person likes things), fact (the default), decision (and why), howto (how something is done here), lesson (what went wrong once, or what the person corrected)."}
            }, "required": ["memory"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, a: &Value) -> String {
        s(a, "memory").unwrap_or_default().chars().take(100).collect()
    }
    async fn run(&self, a: Value, _ctx: &Context) -> Output {
        let person = s(&a, "scope") == Some("person");
        let kind =
            s(&a, "kind").filter(|k| matches!(*k, "preference" | "decision" | "howto" | "lesson")).unwrap_or("fact");
        match self.memory.remember(person, s(&a, "memory").unwrap_or_default(), kind).await {
            Ok((true, place)) => Output::ok(format!("Remembered {place}.")),
            Ok((false, place)) => Output::ok(format!("Already remembered {place}.")),
            Err(e) => Output::error(e),
        }
    }
}

struct Recall {
    memory: Arc<Memory>,
}

#[async_trait::async_trait]
impl Tool for Recall {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "recall".into(),
            description: "Search what is remembered (the person's, this project's and the company's memories) for what bears on a question, when what came with the message is not enough: the nearest by meaning.".into(),
            parameters: json!({"type": "object", "properties": {"query": {"type": "string", "description": "What to look for, in a few words or a question."}}, "required": ["query"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, a: &Value) -> String {
        s(a, "query").unwrap_or_default().to_string()
    }
    async fn run(&self, a: Value, _ctx: &Context) -> Output {
        match self.memory.recall(s(&a, "query").unwrap_or_default(), 6_000).await {
            Some(note) => {
                // The memories without the note's heading and its closing bracket.
                let body = note.split_once('\n').map(|(_, b)| b).unwrap_or(&note);
                Output::ok(body.strip_suffix(')').unwrap_or(body).to_string())
            }
            None => Output::ok("Nothing remembered bears on that."),
        }
    }
}

struct RememberProject {
    memory: Arc<Memory>,
}

#[async_trait::async_trait]
impl Tool for RememberProject {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "remember_project".into(),
            description: "Keep something in the project's long memory (.remember/, in the repository), which every later session reads at its start: add to a section of MEMORY.md (or of a topic file), or replace the section when what it says is out of date. For how to build, test and run; conventions; decisions and why; where the work stands and what is next; lessons. Short facts about the person go to remember instead.".into(),
            parameters: json!({"type": "object", "properties": {
                "section": {"type": "string", "description": "The section's heading, e.g. \"How to build, test and run\", \"Decisions\", \"State and next steps\", \"Lessons\"."},
                "text": {"type": "string", "description": "What to keep: facts, in Markdown."},
                "file": {"type": "string", "description": "A topic file inside .remember/ (default MEMORY.md), e.g. architecture.md."},
                "replace": {"type": "boolean", "description": "Replace the section (what it says is out of date) instead of adding to it."}
            }, "required": ["section", "text"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Edit
    }
    fn subject(&self, a: &Value) -> String {
        format!(".remember/{} § {}", s(a, "file").unwrap_or(project::MAIN), s(a, "section").unwrap_or_default())
    }
    async fn run(&self, a: Value, ctx: &Context) -> Output {
        let (Some(section), Some(text)) = (s(&a, "section"), s(&a, "text")) else {
            return Output::error("Give the section and the text.");
        };
        if section.trim().is_empty() || text.trim().is_empty() {
            return Output::error("Give the section and the text.");
        }
        if text.len() > project::SECTION_CHARS {
            return Output::error(format!(
                "A section holds at most {} characters: put details in a topic file.",
                project::SECTION_CHARS
            ));
        }
        let file = match project::file_of(&self.memory.root, s(&a, "file")) {
            Ok(f) => f,
            Err(e) => return Output::error(e),
        };
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let existing = std::fs::read_to_string(&file).unwrap_or_default();
        let after =
            project::keep(&existing, &name, section, text, a.get("replace").and_then(Value::as_bool) == Some(true));
        if after.len() > project::FILE_CHARS {
            return Output::error(format!(
                "{name} would pass {} characters: move details into a topic file.",
                project::FILE_CHARS
            ));
        }
        ctx.checkpoints.before_write(&file);
        if let Some(d) = file.parent()
            && let Err(e) = std::fs::create_dir_all(d)
        {
            return Output::error(e.to_string());
        }
        if let Err(e) = std::fs::write(&file, after) {
            return Output::error(e.to_string());
        }
        let shown = ctx.workspace.display(&file);
        let mut o = Output::ok(format!("Kept in {shown} § {}.", section.trim()));
        o.wrote.push(file);
        o
    }
}

struct SearchHistory {
    memory: Arc<Memory>,
}

#[async_trait::async_trait]
impl Tool for SearchHistory {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "search_history".into(),
            description: "Find the person's past chats and Code Arena sessions (on every machine, and in the web chat) by what they were about: what was asked, done and decided, the nearest first, each with its id. For \"as we decided last month\" or \"the script from that session\".".into(),
            parameters: json!({"type": "object", "properties": {"query": {"type": "string", "description": "What the earlier work was about, in a few words or a question."}}, "required": ["query"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, a: &Value) -> String {
        s(a, "query").unwrap_or_default().to_string()
    }
    async fn run(&self, a: Value, _ctx: &Context) -> Output {
        let Some(arena) = self.memory.arena() else {
            return Output::error("Past chats are Arena's: this session is not signed in to Arena.");
        };
        match arena.search_history(s(&a, "query").unwrap_or_default()).await {
            Ok(r) if r.error => Output::error(r.text),
            Ok(r) => Output::ok(r.text),
            Err(e) => Output::error(e.to_string()),
        }
    }
}

struct OpenChat {
    memory: Arc<Memory>,
}

#[async_trait::async_trait]
impl Tool for OpenChat {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "open_chat".into(),
            description: "Read one of the person's past chats or sessions, by the id search_history gave: its digest, and with query the messages that hold those words (with their numbers), or with from (and to) its messages by number.".into(),
            parameters: json!({"type": "object", "properties": {
                "chat": {"type": "string", "description": "The chat's id, as search_history gives it."},
                "query": {"type": "string", "description": "Words to find in it (optional)."},
                "from": {"type": "integer", "description": "The first message to read, by number (optional)."},
                "to": {"type": "integer", "description": "The last (optional)."}
            }, "required": ["chat"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, a: &Value) -> String {
        s(a, "chat").unwrap_or_default().to_string()
    }
    async fn run(&self, a: Value, _ctx: &Context) -> Output {
        let Some(arena) = self.memory.arena() else {
            return Output::error("Past chats are Arena's: this session is not signed in to Arena.");
        };
        let from = a.get("from").and_then(Value::as_u64);
        let to = a.get("to").and_then(Value::as_u64);
        match arena.open_history(s(&a, "chat").unwrap_or_default(), s(&a, "query"), from, to).await {
            Ok(r) if r.error => Output::error(r.text),
            Ok(r) => Output::ok(r.text),
            Err(e) => Output::error(e.to_string()),
        }
    }
}

struct SkillSave {
    memory: Arc<Memory>,
}

fn good_name(n: &str) -> bool {
    let mut c = n.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase())
        && (2..=40).contains(&n.len())
        && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[async_trait::async_trait]
impl Tool for SkillSave {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "skill_save".into(),
            description: "Keep a way of doing a task as a skill, to do it the same way next time: after working out a procedure worth repeating (several steps: how to release, how to add an endpoint here, how to debug this kind of failure), or a better version of a skill that went wrong. Instructions to an assistant, general enough to reuse (no one-off details). For this project (its .code-arena/skills/, in the repository), or for the person (their skills in Arena, for every project and the web chat).".into(),
            parameters: json!({"type": "object", "properties": {
                "name": {"type": "string", "description": "Its slash name: lowercase letters, digits and dashes (e.g. release-a-version)."},
                "description": {"type": "string", "description": "When to use it, in a sentence: the model reads it to choose the skill."},
                "instructions": {"type": "string", "description": "The steps, as instructions to an assistant."},
                "scope": {"type": "string", "enum": ["project", "person"], "description": "project (default): this repository's; person: the person's, everywhere."},
                "why": {"type": "string", "description": "What it was learned from, or what was wrong with the version before."}
            }, "required": ["name", "description", "instructions"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Edit
    }
    fn subject(&self, a: &Value) -> String {
        format!("/{} ({})", s(a, "name").unwrap_or_default(), s(a, "scope").unwrap_or("project"))
    }
    async fn run(&self, a: Value, ctx: &Context) -> Output {
        let name = s(&a, "name").unwrap_or_default().trim().trim_start_matches('/').to_string();
        let description = s(&a, "description").unwrap_or_default().trim().to_string();
        let instructions = s(&a, "instructions").unwrap_or_default().trim().to_string();
        if !good_name(&name) {
            return Output::error(
                "The name is lowercase letters, digits and dashes, 2 to 40 of them (e.g. release-a-version).",
            );
        }
        if description.is_empty() || instructions.len() < 40 {
            return Output::error("Say when to use it, and give its steps (at least a few lines).");
        }
        if s(&a, "scope") == Some("person") {
            let Some(arena) = self.memory.arena() else {
                return Output::error(
                    "The person's skills are Arena's: this session is not signed in to Arena. Keep it for the project instead.",
                );
            };
            let why = s(&a, "why").unwrap_or("Worked out in a Code Arena session");
            return match arena.learn_skill(&name, &description, &instructions, why).await {
                Ok(r) if r.error => Output::error(r.text),
                Ok(r) => Output::ok(r.text),
                Err(e) => Output::error(e.to_string()),
            };
        }
        let file = self.memory.root.join(".code-arena").join("skills").join(&name).join("SKILL.md");
        let replaced = file.exists();
        ctx.checkpoints.before_write(&file);
        if let Some(d) = file.parent()
            && let Err(e) = std::fs::create_dir_all(d)
        {
            return Output::error(e.to_string());
        }
        let body = format!(
            "---\nname: {name}\ndescription: {}\nlearned: {}, by Code Arena\n---\n\n{instructions}\n",
            description.replace('\n', " "),
            chrono::Utc::now().format("%Y-%m-%d")
        );
        if let Err(e) = std::fs::write(&file, body) {
            return Output::error(e.to_string());
        }
        let shown = ctx.workspace.display(&file);
        let mut o = Output::ok(format!(
            "{} /{name} in {shown}: it is used from the next message (commit it to share it with the team).",
            if replaced { "Replaced" } else { "Kept" }
        ));
        o.wrote.push(file);
        o
    }
}
