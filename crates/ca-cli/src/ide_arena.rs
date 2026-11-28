//! Arena's side of a session in the IDE: the assistants it may work as, and the projects its chat may count to.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn).
//!
//! - `assistants` -> `assistants{items: [{id, name, description, model, mine}] | null, current: {id, name} | null,
//!   problem}`: answered while a turn runs too. `items` is null while nothing is signed in to Arena, and `problem`
//!   says why the list is not there.
//! - `assistant_set{id | null}`: the session works as that assistant from the next message (its instructions,
//!   tools, model, thinking and skills), or as Code Arena's own; refused while a turn runs. What is said of it is a
//!   `notice{of: "assistant_set", text, error}`, and `assistants` is said again.
//! - `projects{scope: mine | global | all, query}` -> `projects{scope, query, items: [{id, name, description,
//!   global, archived, role, mine, goals, goals_done, spent, budget}] | null, current: {id, name} | null,
//!   suggested, problem, can_create, can_global}`: the projects the person sees that are in the scope and have
//!   every word of the query, the one the session's chat counts to, and the one this folder's repository is in
//!   (`suggested`, an id). Answered while a turn runs too.
//! - `project_set{id | "none"}`, `project_new{name, global}`: kept for the turn's end. What is said of it is a
//!   `notice{of, text, error}` (with Arena's words when it refused), and `projects` is said again.
//! - `/assistant [N | NAME | none]`, `/projects`, `/project [show | N | NAME | none | new NAME]`: as in the
//!   terminal, between turns.

use std::sync::{Arc, Mutex};

use ca_arena::ArenaClient;
use ca_arena::projects::{Assistant, ProjectRow};
use ca_sync::ChatSync;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::ide_ext::{Area, Ctx, Kept, Rule, Sent, Shared, kind, say, send};

/// This area in the driver.
pub struct Here;

/// Said when nothing is signed in to Arena.
const SIGNED_OUT_ASSISTANTS: &str = "Sign in to Arena to work as one of your assistants.";
const SIGNED_OUT_PROJECTS: &str = "Sign in to Arena to see your projects and put this session in one.";

/// The assistant a session works as: the session's id, the assistant's id and name.
#[derive(Default)]
struct Working(Mutex<Option<(String, String, String)>>);

impl Working {
    /// The assistant the session works as; none when it was another session's.
    fn of(&self, session: &str) -> Option<(String, String)> {
        let w = self.0.lock().ok()?;
        w.as_ref().filter(|(s, _, _)| s == session).map(|(_, id, name)| (id.clone(), name.clone()))
    }

    fn set(&self, session: &str, assistant: Option<&Assistant>) {
        if let Ok(mut w) = self.0.lock() {
            *w = assistant.map(|a| (session.to_string(), a.id.clone(), a.name.clone()));
        }
    }
}

/// What a request of the area needs, copied off the session so it may be answered away from the driver's loop.
#[derive(Clone)]
struct Asked {
    out: broadcast::Sender<String>,
    arena: Option<ArenaClient>,
    sync: Option<Arc<ChatSync>>,
    kept: Kept,
    session: String,
    /// This folder's repository, as Arena keys it.
    key: String,
}

impl Asked {
    fn of(sh: &Shared) -> Asked {
        Asked {
            out: sh.out.clone(),
            arena: sh.arena.clone().or_else(|| sh.sync.as_ref().map(|s| s.arena().clone())),
            sync: sh.sync.clone(),
            kept: sh.kept.clone(),
            session: sh.session.clone(),
            key: ca_memory::keys::project_key(sh.workspace.root()),
        }
    }
}

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("assistants", Rule::Turn),
        ("assistant_set", Rule::Refuse("work as another assistant")),
        ("projects", Rule::Turn),
        ("project_set", Rule::Queue),
        ("project_new", Rule::Queue),
    ];

    /// The lists: asked of Arena away from the driver's loop, then said.
    fn shared(sh: &Shared, m: &Value) -> bool {
        let asked = Asked::of(sh);
        match kind(m) {
            "assistants" => {
                tokio::spawn(async move { send(&asked.out, assistants_reply(&asked).await.0) });
            }
            "projects" => {
                let (scope, query) = scope_and_query(m);
                tokio::spawn(async move { send(&asked.out, projects_reply(&asked, &scope, &query).await) });
            }
            _ => return false,
        }
        true
    }

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        let out = ctx.sh.out.clone();
        match kind(m) {
            "assistant_set" => {
                let id = m["id"].as_str().filter(|i| !i.is_empty() && *i != "none");
                let (said, error) = match set_assistant(ctx, id).await {
                    Ok(said) => (said, false),
                    Err(e) => (e, true),
                };
                notice(&out, "assistant_set", &said, error);
                send(&out, assistants_reply(&Asked::of(ctx.sh)).await.0);
            }
            "project_set" => {
                let id = m["id"].as_str().filter(|i| !i.is_empty() && *i != "none");
                let said = set_project(ctx, id).await;
                tell_project(ctx, "project_set", said).await;
            }
            "project_new" => {
                let name = m["name"].as_str().unwrap_or("").trim().to_string();
                let said = if name.is_empty() {
                    Err("Name the project.".to_string())
                } else {
                    new_project(ctx, &name, m["global"].as_bool().unwrap_or(false)).await
                };
                tell_project(ctx, "project_new", said).await;
            }
            _ => return false,
        }
        true
    }

    /// `/assistant`, `/projects` and `/project`, as in the terminal.
    async fn command(ctx: &mut Ctx<'_>, sent: &mut Sent) {
        let text = sent.model.trim().to_string();
        let (name, rest) = text.split_once(char::is_whitespace).unwrap_or((&text, ""));
        let rest = rest.trim();
        let out = ctx.sh.out.clone();
        match name {
            "/assistant" if rest.is_empty() => say(&out, &crate::projects::assistant("", ctx.p).await),
            "/assistant" => {
                let said = match chosen_assistant(ctx, rest).await {
                    Ok(id) => set_assistant(ctx, id.as_deref()).await,
                    Err(e) => Err(e),
                };
                let error = said.is_err();
                notice(&out, "assistant_set", &said.unwrap_or_else(|e| e), error);
                send(&out, assistants_reply(&Asked::of(ctx.sh)).await.0);
            }
            "/projects" => say(&out, &crate::projects::command(name, rest, ctx.p).await),
            "/project" if rest.is_empty() || rest == "show" => {
                say(&out, &crate::projects::command(name, rest, ctx.p).await)
            }
            "/project" => match rest.strip_prefix("new ").map(str::trim).filter(|w| !w.is_empty()) {
                Some(words) => {
                    let said = new_project(ctx, words, false).await;
                    tell_project(ctx, "project_new", said).await;
                }
                None => {
                    let said = match chosen_project(ctx, rest).await {
                        Ok(chosen) => set_project(ctx, chosen.as_deref()).await,
                        Err(e) => Err(e),
                    };
                    tell_project(ctx, "project_set", said).await;
                }
            },
            _ => return,
        }
        sent.done = true;
    }
}

/// A line of the area's: kept apart from the thread by the faces that show it as a bubble (`of`).
fn notice(out: &broadcast::Sender<String>, of: &str, text: &str, error: bool) {
    send(out, json!({"type": "notice", "of": of, "text": text, "error": error}));
}

/// What putting the session in a project said, then the projects again.
async fn tell_project(ctx: &mut Ctx<'_>, of: &str, said: Result<String, String>) {
    let out = ctx.sh.out.clone();
    let error = said.is_err();
    notice(&out, of, &said.unwrap_or_else(|e| e), error);
    send(&out, projects_reply(&Asked::of(ctx.sh), "all", "").await);
}

/// The scope and the query a `projects` asks for.
fn scope_and_query(m: &Value) -> (String, String) {
    let scope = match m["scope"].as_str() {
        Some(s @ ("mine" | "global")) => s.to_string(),
        _ => "all".to_string(),
    };
    (scope, m["query"].as_str().unwrap_or("").trim().to_string())
}

/// One of a list, as `/project` and `/assistant` name it: its number (from 1) or its name (any case).
fn choose<'a, T>(list: &'a [T], words: &str, name_of: impl Fn(&T) -> &str) -> Option<&'a T> {
    match words.parse::<usize>() {
        Ok(n) if (1..=list.len()).contains(&n) => Some(&list[n - 1]),
        _ => list.iter().find(|x| name_of(x).eq_ignore_ascii_case(words)),
    }
}

/// The `assistants` reply, and the list it was made of.
async fn assistants_reply(asked: &Asked) -> (Value, Vec<Assistant>) {
    let current = asked.kept.of::<Working>().of(&asked.session).map(|(id, name)| json!({"id": id, "name": name}));
    let Some(arena) = &asked.arena else {
        return (
            json!({"type": "assistants", "items": null, "current": null, "problem": SIGNED_OUT_ASSISTANTS}),
            vec![],
        );
    };
    match arena.assistants().await {
        Ok(list) => {
            let items: Vec<Value> = list
                .iter()
                .map(|a| {
                    json!({"id": a.id, "name": a.name, "description": a.description, "model": a.model,
                           "mine": a.mine})
                })
                .collect();
            (json!({"type": "assistants", "items": items, "current": current, "problem": null}), list)
        }
        Err(e) => (
            json!({"type": "assistants", "items": [], "current": current, "problem": format!("Arena did not list them: {e}")}),
            vec![],
        ),
    }
}

/// The assistant named by `/assistant`: its id, or none for Code Arena's own.
async fn chosen_assistant(ctx: &mut Ctx<'_>, words: &str) -> Result<Option<String>, String> {
    if matches!(words, "none" | "off") {
        return Ok(None);
    }
    let (_, list) = assistants_reply(&Asked::of(ctx.sh)).await;
    choose(&list, words, |a| &a.name)
        .map(|a| Some(a.id.clone()))
        .ok_or_else(|| format!("No assistant {words}: /assistant lists them."))
}

/// The session works as this assistant (by its id), or as Code Arena's own: what is said of it. The faces are told
/// the model and thinking it works with now (`status`).
async fn set_assistant(ctx: &mut Ctx<'_>, id: Option<&str>) -> Result<String, String> {
    let Some(sync) = ctx.p.sync.clone() else { return Err(SIGNED_OUT_ASSISTANTS.into()) };
    let working = ctx.sh.kept.of::<Working>();
    let session = ctx.p.session.clone();
    let said = match id {
        None => {
            working.set(&session, None);
            crate::projects::own_again(ctx.p)
        }
        Some(id) => {
            let list = sync.arena().assistants().await.map_err(|e| format!("Arena did not list them: {e}"))?;
            let chosen = list.iter().find(|a| a.id == id).ok_or("That assistant is not among yours any more.")?;
            let said = crate::projects::work_as(ctx.p, &sync, chosen);
            working.set(&session, Some(chosen));
            if let Ok(store) = ctx.p.store.lock() {
                let _ = store.set_model(&ctx.p.session, &ctx.p.agent.model, ctx.p.agent.policy.mode.name());
            }
            said
        }
    };
    send(&ctx.sh.out, crate::ide::status(ctx.p, ctx.sh));
    Ok(said)
}

/// The project named by `/project`: its id, or none for no project.
async fn chosen_project(ctx: &mut Ctx<'_>, words: &str) -> Result<Option<String>, String> {
    if matches!(words, "none" | "off") {
        return Ok(None);
    }
    let asked = Asked::of(ctx.sh);
    let Some(arena) = &asked.arena else { return Err(SIGNED_OUT_PROJECTS.into()) };
    let list = arena.projects(Some(&asked.key)).await.map_err(|e| e.to_string())?.projects;
    choose(&list, words, |p| &p.name)
        .map(|p| Some(p.id.clone()))
        .ok_or_else(|| format!("No project {words}: /projects lists them."))
}

/// The session's chat put in this project (by its id), or in none: what is said of it.
async fn set_project(ctx: &mut Ctx<'_>, id: Option<&str>) -> Result<String, String> {
    let Some(sync) = ctx.p.sync.clone() else { return Err(SIGNED_OUT_PROJECTS.into()) };
    let Some(id) = id else { return crate::projects::put_in(ctx.p, &sync, None).await };
    let key = ca_memory::keys::project_key(ctx.p.agent.ctx.workspace.root());
    let list = sync.arena().projects(Some(&key)).await.map_err(|e| e.to_string())?.projects;
    let name = list
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.name.clone())
        .ok_or("That project is not one you may put a session in (it may be archived, or you only see it).")?;
    crate::projects::put_in(ctx.p, &sync, Some((id, &name))).await
}

/// A project of the person's made, with this folder's repository and this session in it.
async fn new_project(ctx: &mut Ctx<'_>, words: &str, global: bool) -> Result<String, String> {
    let Some(sync) = ctx.p.sync.clone() else { return Err(SIGNED_OUT_PROJECTS.into()) };
    let key = ca_memory::keys::project_key(ctx.p.agent.ctx.workspace.root());
    let arena = sync.arena().clone();
    crate::projects::make_global(ctx.p, &arena, &sync, &key, words, global).await
}

/// A project as the view lists it.
fn row_json(r: &ProjectRow) -> Value {
    json!({
        "id": r.id, "name": r.name, "description": r.description, "global": r.global,
        "archived": r.status.as_deref().is_some_and(|s| s != "active"), "role": r.role, "mine": r.mine,
        "goals": r.goals, "goals_done": r.goals_done, "spent": r.spent_this_month, "budget": r.monthly_budget,
    })
}

/// Whether a project is in the scope and has every word of the query in its name or description (any case).
fn in_view(r: &ProjectRow, scope: &str, query: &str) -> bool {
    let scoped = match scope {
        "mine" => r.mine,
        "global" => r.global,
        _ => true,
    };
    let hay = format!("{} {}", r.name, r.description.as_deref().unwrap_or("")).to_lowercase();
    scoped && query.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

/// The `projects` reply for a scope and a query.
async fn projects_reply(asked: &Asked, scope: &str, query: &str) -> Value {
    let mut v = json!({"type": "projects", "scope": scope, "query": query, "items": null, "current": null,
                       "suggested": null, "problem": null, "can_create": false, "can_global": false});
    let Some(arena) = &asked.arena else {
        v["problem"] = json!(SIGNED_OUT_PROJECTS);
        return v;
    };
    let chat = asked.sync.as_ref().and_then(|s| s.chat_id());
    let (rows, open, may, work) =
        tokio::join!(arena.project_rows(), arena.projects(Some(&asked.key)), arena.may_make(), async {
            match &chat {
                Some(c) => arena.project_work(c).await.ok().flatten(),
                None => None,
            }
        });
    // An Arena whose Projects page cannot be read with the key lists those a session may be put in.
    let rows = match (rows, &open) {
        (Ok(rows), _) => Ok(rows),
        (Err(_), Ok(open)) => Ok(open
            .projects
            .iter()
            .map(|p| ProjectRow {
                id: p.id.clone(),
                name: p.name.clone(),
                description: p.description.clone(),
                global: p.global,
                status: Some("active".into()),
                mine: !p.global,
                ..ProjectRow::default()
            })
            .collect()),
        (Err(e), Err(_)) => Err(e),
    };
    match rows {
        Ok(rows) => {
            v["items"] = rows.iter().filter(|r| in_view(r, scope, query)).map(row_json).collect::<Vec<_>>().into();
        }
        Err(e) => {
            v["items"] = json!([]);
            v["problem"] = json!(format!("Arena did not list them: {e}"));
        }
    }
    if let Ok(open) = &open {
        v["suggested"] = open.suggested.clone().unwrap_or(Value::Null);
    }
    let may = may.unwrap_or(ca_arena::projects::May { create: true, global: false });
    v["can_create"] = json!(may.create);
    v["can_global"] = json!(may.create && may.global);
    if let Some(w) = work {
        v["current"] = json!({"id": w.project.id, "name": w.project.name});
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, mine: bool, global: bool) -> ProjectRow {
        ProjectRow {
            id: name.to_lowercase(),
            name: name.into(),
            description: Some("The new API".into()),
            mine,
            global,
            ..ProjectRow::default()
        }
    }

    #[test]
    fn a_project_is_in_the_view_by_its_scope_and_every_word_asked() {
        let (own, everyone) = (row("Payments", true, false), row("Handbook", false, true));
        assert!(in_view(&own, "mine", "") && !in_view(&everyone, "mine", ""));
        assert!(in_view(&everyone, "global", "") && !in_view(&own, "global", ""));
        assert!(in_view(&own, "all", "pay api") && !in_view(&own, "all", "pay web"));
    }

    #[test]
    fn one_is_chosen_by_its_number_or_its_name_in_any_case() {
        let list = [row("Payments", true, false), row("Handbook", false, true)];
        assert_eq!(choose(&list, "2", |r| &r.name).map(|r| r.id.as_str()), Some("handbook"));
        assert_eq!(choose(&list, "payments", |r| &r.name).map(|r| r.id.as_str()), Some("payments"));
        assert!(choose(&list, "3", |r| &r.name).is_none());
    }

    #[test]
    fn the_scope_asked_is_mine_global_or_all() {
        assert_eq!(scope_and_query(&json!({"scope": "mine", "query": " pay "})), ("mine".into(), "pay".into()));
        assert_eq!(scope_and_query(&json!({"scope": "else"})), ("all".into(), String::new()));
    }

    #[test]
    fn an_assistant_kept_is_the_sessions_it_was_chosen_in_only() {
        let w = Working::default();
        let a = Assistant {
            id: "a1".into(),
            name: "Reviewer".into(),
            description: None,
            instructions: None,
            model: None,
            thinking: None,
            skills: None,
            tools: None,
            mine: true,
        };
        w.set("s1", Some(&a));
        assert_eq!(w.of("s1"), Some(("a1".into(), "Reviewer".into())));
        assert_eq!(w.of("s2"), None);
    }

    #[test]
    fn an_archived_project_is_said_so_with_its_goals_and_spend() {
        let r = ProjectRow { status: Some("archived".into()), goals: 4, goals_done: 1, ..row("Payments", true, false) };
        let v = row_json(&r);
        assert_eq!((v["archived"].clone(), v["goals_done"].clone()), (json!(true), json!(1)));
    }
}
