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

/// `/project new NAME [: DESCRIPTION]`: a project of the person's in Arena with this repository in it, and this session
/// in it.
async fn make(
    p: &mut Prepared,
    arena: &ca_arena::ArenaClient,
    sync: &Arc<ca_sync::ChatSync>,
    key: &str,
    words: &str,
) -> String {
    make_global(p, arena, sync, key, words, false).await.unwrap_or_else(|e| e)
}

/// As `/project new`, and global when asked: what is said of it, or (as an error) why Arena did not make it.
pub async fn make_global(
    p: &mut Prepared,
    arena: &ca_arena::ArenaClient,
    sync: &ca_sync::ChatSync,
    key: &str,
    words: &str,
    global: bool,
) -> Result<String, String> {
    let (name, description) = match words.split_once(':') {
        Some((n, d)) => (n.trim(), Some(d.trim()).filter(|d| !d.is_empty())),
        None => (words, None),
    };
    let repo = ca_arena::projects::repository_of(key);
    let id = match arena
        .make_project_for(name, description, repo.as_ref().map(|(a, b)| (a.as_str(), b.as_str())), global)
        .await
    {
        Ok(id) => id,
        Err(e) => return Err(format!("Arena did not make it: {e}")),
    };
    let mut said = vec![format!(
        "Made the project {name}{} in Arena.",
        repo.as_ref().map(|(path, _)| format!(" with {path} in it")).unwrap_or_default()
    )];
    match sync.ensure_chat().await {
        Some(chat) => match arena.move_chat(&chat, Some(&id)).await {
            Ok(()) => {
                take_up(p, true).await;
                said.push("This session is in it now: /project show shows it.".into());
            }
            Err(e) => said.push(format!("This session could not go in it: {e}")),
        },
        None => said.push("Say something first, then /project N puts this session in it.".into()),
    }
    Ok(said.join(" "))
}

/// A project as its page has it, in lines.
fn shown(v: &ca_arena::projects::ProjectView) -> String {
    let mut out = vec![format!(
        "{}{}{}",
        v.name,
        v.status.as_ref().map(|s| format!(" ({s})")).unwrap_or_default(),
        v.due_on.as_ref().map(|d| format!(", due {d}")).unwrap_or_default()
    )];
    if let Some(d) = v.description.as_ref().filter(|d| !d.is_empty()) {
        out.push(format!("  {d}"));
    }
    let repos: Vec<String> =
        v.repositories.iter().filter_map(|r| r.get("path").and_then(Value::as_str).map(str::to_string)).collect();
    if !repos.is_empty() {
        out.push(format!("  Repositories: {}", repos.join(", ")));
    }
    if !v.members.is_empty() {
        out.push(format!("  {} people in it, you as {}", v.members.len(), v.role.clone().unwrap_or_default()));
    }
    out.push(format!("  Goals ({}):", v.goals.len()));
    for g in &v.goals {
        let done = g.status.as_deref() == Some("done");
        out.push(format!("    {} {}", if done { "✓" } else { "○" }, g.title));
        for t in v.tasks.iter().filter(|t| t.goal_id.as_deref() == Some(g.id.as_str())) {
            out.push(task_line(t));
        }
    }
    let loose: Vec<String> = v.tasks.iter().filter(|t| t.goal_id.is_none()).map(task_line).collect();
    if !loose.is_empty() {
        out.push("  Tasks of no goal:".into());
        out.extend(loose);
    }
    out.join("\n")
}

fn task_line(t: &ca_arena::projects::ViewTask) -> String {
    format!(
        "      {} {}{}",
        if t.done { "✓" } else { "·" },
        t.title,
        t.assignee.as_ref().map(|a| format!(" ({a})")).unwrap_or_default()
    )
}

/// `/projects`, `/project`, `/project N|NAME`, `/project off`.
pub async fn command(name: &str, rest: &str, p: &mut Prepared) -> String {
    let Some(sync) = p.sync.clone() else { return "Projects are Arena's: sign in to it first.".into() };
    let arena = sync.arena().clone();
    let key = ca_memory::keys::project_key(p.agent.ctx.workspace.root());
    if name == "/project"
        && let Some(words) = rest.strip_prefix("new ").map(str::trim).filter(|w| !w.is_empty())
    {
        return make(p, &arena, &sync, &key, words).await;
    }
    if name == "/project" && rest == "show" {
        let Some(chat) = sync.chat_id() else {
            return "This session is in no project (it has no chat in Arena yet).".into();
        };
        return match arena.project_work(&chat).await {
            Ok(Some(w)) => match arena.project_view(&w.project.id).await {
                Ok(v) => shown(&v),
                Err(e) => e.to_string(),
            },
            Ok(None) => "This session is in no project: /projects lists them, /project new NAME makes one.".into(),
            Err(e) => e.to_string(),
        };
    }
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
    let chosen = if rest == "off" || rest == "none" {
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
    put_in(p, &sync, chosen.as_ref().map(|c| (c.id.as_str(), c.name.as_str()))).await.unwrap_or_else(|e| e)
}

/// The session's chat put in this project (its id and name), or in none: what is said of it, or why it could not.
pub async fn put_in(p: &mut Prepared, sync: &ca_sync::ChatSync, chosen: Option<(&str, &str)>) -> Result<String, String> {
    let Some(chat) = sync.ensure_chat().await else {
        return Err("This session has no chat in Arena yet: say something first.".into());
    };
    sync.arena().move_chat(&chat, chosen.map(|c| c.0)).await.map_err(|e| e.to_string())?;
    let work = take_up(p, true).await;
    Ok(match (chosen, work) {
        (None, _) => "This session is in no project now.".into(),
        (Some((_, name)), Some(w)) if !w.functions.is_empty() => format!(
            "This session is in {name} now: the model keeps its goals and tasks ({}).",
            w.functions.join(", ")
        ),
        (Some((_, name)), _) => format!("This session is in {name} now."),
    })
}

type Tools = Vec<Arc<dyn Tool>>;

/// The tools kept, and Arena's of other groups than these; Code Arena's own and other servers' are always kept.
fn outside_groups(tools: Tools, groups: &[String]) -> (Tools, Tools) {
    let (aside, keep): (Tools, Tools) =
        tools.into_iter().partition(|t| t.arena_tool().is_some_and(|g| !groups.contains(&g)));
    (keep, aside)
}

/// An assistant's tool groups: Arena's tools of other groups set aside (with `/tools`' own), the last assistant's put
/// back first; `None` puts them all back. How many were put back.
pub fn tool_groups(p: &mut Prepared, groups: Option<&Vec<String>>) -> usize {
    let names = std::mem::take(&mut p.assistant_off);
    let (back, keep): (Vec<_>, Vec<_>) = p.disabled.drain(..).partition(|t| names.contains(&t.def().name));
    p.disabled = keep;
    let n = back.len();
    p.agent.tools.extend(back);
    let Some(groups) = groups else { return n };
    let (tools, aside) = outside_groups(std::mem::take(&mut p.agent.tools), groups);
    p.agent.tools = tools;
    let (deferred, aside_later) = outside_groups(std::mem::take(&mut p.agent.deferred), groups);
    p.agent.deferred = deferred;
    for t in aside.into_iter().chain(aside_later) {
        p.assistant_off.push(t.def().name);
        p.disabled.push(t);
    }
    n
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
    if rest == "off" || rest == "none" {
        return own_again(p);
    }
    let chosen = match rest.parse::<usize>() {
        Ok(n) if (1..=list.len()).contains(&n) => list[n - 1].clone(),
        _ => match list.iter().find(|a| a.name.eq_ignore_ascii_case(rest)) {
            Some(a) => a.clone(),
            None => return format!("No assistant {rest}: /assistant lists them."),
        },
    };
    work_as(p, &sync, &chosen)
}

/// Code Arena's own again, with all of Arena's tools an assistant set aside: what is said of it.
pub fn own_again(p: &mut Prepared) -> String {
    let back = tool_groups(p, None);
    p.agent.note("(Code Arena: the person stopped the assistant: work as Code Arena's own from now on.)".into());
    if back > 0 {
        format!("Working as Code Arena's own again, with all of Arena's tools ({back} back).")
    } else {
        "Working as Code Arena's own again.".into()
    }
}

/// The session works as this assistant from the next message (its instructions, tools, model, thinking and
/// skills): what is said of it.
pub fn work_as(p: &mut Prepared, sync: &ca_sync::ChatSync, chosen: &ca_arena::projects::Assistant) -> String {
    let mut said = vec![format!("Working as {}.", chosen.name)];
    if let Some(groups) = &chosen.tools {
        tool_groups(p, Some(groups));
        said.push(if groups.is_empty() {
            "It uses none of Arena's tools.".to_string()
        } else {
            format!("Arena's tools it uses: {}.", groups.join(", "))
        });
    } else {
        tool_groups(p, None);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(&'static str, Option<&'static str>);
    #[async_trait::async_trait]
    impl Tool for Fake {
        fn def(&self) -> ToolDef {
            ToolDef { name: self.0.into(), description: String::new(), parameters: json!({}) }
        }
        fn kind(&self) -> Kind {
            Kind::Read
        }
        fn subject(&self, _args: &Value) -> String {
            String::new()
        }
        async fn run(&self, _args: Value, _ctx: &Context) -> Output {
            Output::ok("")
        }
        fn arena_tool(&self) -> Option<String> {
            self.1.map(str::to_string)
        }
    }

    #[test]
    fn an_assistants_groups_keep_its_arena_tools_and_all_others() {
        let tools: Tools = vec![
            Arc::new(Fake("read_file", None)),
            Arc::new(Fake("mcp__arena__web_search", Some("web"))),
            Arc::new(Fake("mcp__arena__run_python", Some("python"))),
            Arc::new(Fake("mcp__mine__thing", None)),
        ];
        let (keep, aside) = outside_groups(tools, &["web".to_string()]);
        let names = |t: &Tools| t.iter().map(|x| x.def().name).collect::<Vec<_>>();
        assert_eq!(names(&keep), ["read_file", "mcp__arena__web_search", "mcp__mine__thing"]);
        assert_eq!(names(&aside), ["mcp__arena__run_python"]);
        let (keep, aside) = outside_groups(keep, &[]);
        assert_eq!(
            (names(&keep), names(&aside)),
            (vec!["read_file".to_string(), "mcp__mine__thing".to_string()], vec!["mcp__arena__web_search".to_string()])
        );
        assert_eq!(
            ca_arena::projects::repository_of("gitlab.corp/payments/api"),
            Some(("payments/api".into(), "gitlab.corp".into()))
        );
        assert_eq!(ca_arena::projects::repository_of("local/abc123"), None);
    }
}
