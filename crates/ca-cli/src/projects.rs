//! Projects and assistants, from Arena (the managed edition): `/projects` and `/project` put the session's chat in
//! one of the person's projects (its goals and tasks then the model's to keep true to the work, with `project_plan`,
//! `project_goal` and `project_task`, on an Arena that has project work); `/assistant` works as one of theirs.

use std::sync::Arc;

use ca_arena::projects::Work;
use ca_llm::ToolDef;
use ca_sync::ChatSync;
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::{Value, json};

use crate::session::Prepared;

/// One of the project's functions, called in Arena.
pub struct ProjectTool {
    sync: Arc<ChatSync>,
    name: &'static str,
}

fn def_of(name: &str) -> ToolDef {
    let (description, parameters) = match name {
        "project_plan" => (
            "The project's plan as it is now: its goals with their states and tasks, by id.",
            json!({"type": "object", "properties": {}}),
        ),
        "project_goal" => (
            "Adds a goal of the project (no id), or changes one (its id): its title, state, description, due date. Keep each goal to an aim the person set out.",
            json!({"type": "object", "properties": {
                "id": {"type": "string", "description": "The goal's id, to change it; leave out to add one."},
                "title": {"type": "string", "description": "What it is, in a line (1 to 200 characters)."},
                "status": {"type": "string", "enum": ["planned", "doing", "done", "dropped"]},
                "description": {"type": "string", "description": "More on it: what done means, how it is checked."},
                "due": {"type": "string", "description": "When it is due, 2026-12-31."}
            }}),
        ),
        _ => (
            "Adds a task of the project (no id), under a goal or not, or changes one (its id): its title, its goal, done or not.",
            json!({"type": "object", "properties": {
                "id": {"type": "string", "description": "The task's id, to change it; leave out to add one."},
                "title": {"type": "string", "description": "What to do, in a line (1 to 300 characters)."},
                "goal": {"type": "string", "description": "The id of the goal it is part of."},
                "done": {"type": "boolean", "description": "true when it is done; false when it is not after all."}
            }}),
        ),
    };
    ToolDef { name: name.into(), description: description.into(), parameters }
}

#[async_trait::async_trait]
impl Tool for ProjectTool {
    fn def(&self) -> ToolDef {
        def_of(self.name)
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, a: &Value) -> String {
        a.get("title").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    fn group(&self) -> String {
        "Arena's project".into()
    }
    async fn run(&self, args: Value, _ctx: &Context) -> Output {
        let Some(chat) = self.sync.chat_id() else { return Output::error("This session has no chat in Arena yet.") };
        match self.sync.arena().project_call(&chat, self.name, &args).await {
            Ok((t, false)) => Output::ok(t),
            Ok((t, true)) => Output::error(t),
            Err(e) => Output::error(e.to_string()),
        }
    }
}

/// The project the session's chat is in: its tools offered and its instructions told (on an Arena that has them).
pub async fn take_up(p: &mut Prepared, said: bool) -> Option<Work> {
    let sync = p.sync.clone()?;
    let chat = sync.chat_id()?;
    let work = sync.arena().project_work(&chat).await.ok().flatten();
    p.agent.tools.retain(|t| t.group() != "Arena's project");
    if let Some(w) = &work {
        for f in &w.functions {
            let name: &'static str = match f.as_str() {
                "project_plan" => "project_plan",
                "project_goal" => "project_goal",
                "project_task" => "project_task",
                _ => continue,
            };
            p.agent.tools.push(Arc::new(ProjectTool { sync: sync.clone(), name }));
        }
        if said || !w.instructions.is_empty() {
            p.agent.note(format!("(Code Arena: {})", w.instructions));
        }
        p.agent.tags = vec![format!("chat:{chat}"), format!("project:{}", w.project.id)];
    } else {
        p.agent.tags = vec![format!("chat:{chat}")];
    }
    work
}

/// `/projects`, `/project`, `/project N|NAME`, `/project off`.
pub async fn command(name: &str, rest: &str, p: &mut Prepared) -> String {
    let Some(sync) = p.sync.clone() else { return "Projects are Arena's: sign in to it first.".into() };
    let arena = sync.arena().clone();
    let key = ca_memory::keys::project_key(p.agent.ctx.workspace.root());
    let list = match arena.projects(Some(&key)).await {
        Ok(l) => l.projects,
        Err(e) => return e.to_string(),
    };
    if name == "/projects" || (name == "/project" && rest.is_empty()) {
        let current = match sync.chat_id() {
            Some(c) => arena.project_work(&c).await.ok().flatten().map(|w| w.project.name),
            None => None,
        };
        let mut out = vec![match &current {
            Some(n) => format!("This session is in the project {n}."),
            None => "This session is in no project.".into(),
        }];
        for (i, pr) in list.iter().enumerate() {
            out.push(format!(
                "{:>3}. {}{}",
                i + 1,
                pr.name,
                pr.description.as_ref().map(|d| format!(": {d}")).unwrap_or_default()
            ));
        }
        out.push("/project N puts it in one; /project off in none.".into());
        return out.join("\n");
    }
    let chosen = if rest == "off" {
        None
    } else {
        match rest.parse::<usize>() {
            Ok(n) if (1..=list.len()).contains(&n) => Some(list[n - 1].clone()),
            _ => match list.iter().find(|x| x.name.eq_ignore_ascii_case(rest)) {
                Some(x) => Some(x.clone()),
                None => return format!("No project {rest}: /projects lists them."),
            },
        }
    };
    let Some(chat) = sync.ensure_chat().await else {
        return "This session has no chat in Arena yet: say something first.".into();
    };
    if let Err(e) = arena.move_chat(&chat, chosen.as_ref().map(|c| c.id.as_str())).await {
        return e.to_string();
    }
    let work = take_up(p, true).await;
    match (chosen, work) {
        (None, _) => "This session is in no project now.".into(),
        (Some(c), Some(w)) if !w.functions.is_empty() => format!(
            "This session is in {} now: the model keeps its goals and tasks ({}).",
            c.name,
            w.functions.join(", ")
        ),
        (Some(c), _) => format!("This session is in {} now.", c.name),
    }
}

/// `/assistant`: the person's assistants; `/assistant N|NAME` works as one (its instructions, model, thinking and
/// skills) from the next message; `/assistant off` as Code Arena's own again.
pub async fn assistant(rest: &str, p: &mut Prepared) -> String {
    let Some(sync) = p.sync.clone() else { return "Assistants are Arena's: sign in to it first.".into() };
    let list = match sync.arena().assistants().await {
        Ok(l) => l,
        Err(e) => return e.to_string(),
    };
    if rest.is_empty() {
        if list.is_empty() {
            return "You have no assistants in Arena (Chat → Assistants makes one).".into();
        }
        let mut out: Vec<String> = list
            .iter()
            .enumerate()
            .map(|(i, a)| {
                format!(
                    "{:>3}. {}{}",
                    i + 1,
                    a.name,
                    a.description.as_ref().map(|d| format!(": {d}")).unwrap_or_default()
                )
            })
            .collect();
        out.push("/assistant N works as one; /assistant off as Code Arena's own.".into());
        return out.join("\n");
    }
    if rest == "off" {
        p.agent.note("(Code Arena: the person stopped the assistant: work as Code Arena's own from now on.)".into());
        return "Working as Code Arena's own again.".into();
    }
    let chosen = match rest.parse::<usize>() {
        Ok(n) if (1..=list.len()).contains(&n) => list[n - 1].clone(),
        _ => match list.iter().find(|a| a.name.eq_ignore_ascii_case(rest)) {
            Some(a) => a.clone(),
            None => return format!("No assistant {rest}: /assistant lists them."),
        },
    };
    let mut said = vec![format!("Working as {}.", chosen.name)];
    if let Some(m) = chosen.model.clone().filter(|m| !m.is_empty()) {
        p.agent.model = m.clone();
        sync.set_model(&m);
        said.push(format!("The model is {m}."));
    }
    if let Some(t) = chosen.thinking.clone().filter(|t| !t.is_empty()) {
        p.agent.thinking = ca_llm::Thinking::parse(&t);
    }
    let skills: Vec<String> = match &chosen.skills {
        Some(Value::Array(a)) => {
            a.iter().filter_map(|x| x.as_str().or_else(|| x["name"].as_str())).map(str::to_string).collect()
        }
        _ => vec![],
    };
    p.agent.note(format!(
        "(Code Arena: from now on, work as the person's assistant \"{}\".{}{})",
        chosen.name,
        chosen.instructions.as_ref().map(|i| format!(" Its instructions:\n{i}\n")).unwrap_or_default(),
        if skills.is_empty() {
            String::new()
        } else {
            format!(" The skills it uses: {} (load them with skill).", skills.join(", "))
        }
    ));
    said.join(" ")
}
