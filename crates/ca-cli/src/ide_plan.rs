//! The plan's review (approve, another way, reject, change a step), a step undone, an earlier session's plan carried
//! on, the plan cleared; and, in `tasks`, the plan an earlier session of this folder left (`plan_left`).
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn). The work itself is the terminal's (`session::review_plan`,
//! `undo_step`, `resume_plan`): a review becomes the person's next message (`said_next`), so it runs as the next turn.
//!
//! Messages:
//! - `plan_review{action: approve | approve-ask | change | another | reject, step?, text?}` [Q]: answered by the
//!   next turn, or by `plan_review{action, error}` when there is no plan to review or a step is wrong.
//! - `plan_undo{step}`, `plan_resume`, `plan_clear` [R]: answered by `plan_undo{step, text | error}`,
//!   `plan_resume{text | error}`, `plan_clear{text}`.
//!
//! - `checklists` [T]: answered by `checklists{on, mine, repo, problem, edit}` (see `checklists_said`).
//! - `checklist_on{id | repo}`, `checklist_off`, `checklist_tick{n, done}`, `checklist_save{id?, name, items}`,
//!   `checklist_delete{id}` [Q]: answered under their own names with the keys they carried (and `error` when they
//!   could not be done), then by `checklists`. A checklist turned on is said to the model, as `/checklist` does.
//!
//! After each plan request, `tasks` (and `status`, for the mode an approval sets) are said again.

use serde_json::{Value, json};

use crate::ide_ext::{Area, Ctx, Rule, Shared, send};

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("plan_review", Rule::Queue),
        ("plan_undo", Rule::Refuse("undo a step of the plan")),
        ("plan_resume", Rule::Refuse("carry on an earlier plan")),
        ("plan_clear", Rule::Refuse("clear the plan")),
        ("checklists", Rule::Turn),
        ("checklist_on", Rule::Queue),
        ("checklist_off", Rule::Queue),
        ("checklist_tick", Rule::Queue),
        ("checklist_save", Rule::Queue),
        ("checklist_delete", Rule::Queue),
    ];

    fn shared(sh: &Shared, m: &Value) -> bool {
        if crate::ide_ext::kind(m) != "checklists" {
            return false;
        }
        let sh = sh.clone();
        tokio::spawn(async move { send(&sh.out, listing(&sh).await) });
        true
    }

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        let out = ctx.sh.out.clone();
        let kind = crate::ide_ext::kind(m);
        if kind.starts_with("checklist_") {
            let done = checklist(ctx.p, m).await;
            send(&out, answered(m, done));
            send(&out, listing(ctx.sh).await);
            return true;
        }
        match kind {
            "plan_review" => {
                let action = m["action"].as_str().unwrap_or("");
                let said = review_words(m).and_then(|(w, rest)| crate::session::review_plan(ctx.p, w, &rest));
                match said {
                    Ok(text) => ctx.p.said_next = Some(text),
                    Err(e) => send(&out, json!({"type": "plan_review", "action": action, "error": e})),
                }
            }
            "plan_undo" => {
                let step = m["step"].as_u64().unwrap_or(0);
                let reply = match u32::try_from(step).map_err(|_| "No such step.".to_string()) {
                    Ok(n) => crate::session::undo_step(ctx.p, n),
                    Err(e) => Err(e),
                };
                send(&out, said("plan_undo", Some(step), reply));
            }
            "plan_resume" => send(&out, said("plan_resume", None, crate::session::resume_plan(ctx.p))),
            "plan_clear" => {
                ctx.p.agent.ctx.plan.set(ca_tools::plan::Plan::default());
                send(&out, said("plan_clear", None, Ok("The plan is cleared.".into())));
            }
            _ => return false,
        }
        send(&out, crate::ide::tasks(ctx.sh));
        send(&out, crate::ide::status(ctx.p, ctx.sh));
        true
    }

    /// The plan an earlier session of this folder left unfinished, while this session has none of its own.
    fn tasks(sh: &Shared, v: &mut Value) {
        if !sh.plan.get().is_empty() {
            return;
        }
        let left = sh.store.lock().ok().and_then(|s| s.plan_left(&sh.folder, &sh.session).ok().flatten());
        if let Some(l) = left.and_then(|(_, _, json)| left_of(&json)) {
            v["plan_left"] = l;
        }
    }
}

/// The review's words for `session::review_plan`: its action and what follows it.
fn review_words(m: &Value) -> Result<(&'static str, String), String> {
    let text = m["text"].as_str().unwrap_or("").trim().to_string();
    match m["action"].as_str().unwrap_or("") {
        "approve" => Ok(("approve", String::new())),
        "approve-ask" => Ok(("approve", "ask".into())),
        "change" => match m["step"].as_u64() {
            Some(n) => Ok(("change", format!("{n} {text}"))),
            None => Err("Say which step to change.".into()),
        },
        "another" => Ok(("another", text)),
        "reject" => Ok(("reject", text)),
        _ => Err("A plan's review is approve, approve-ask, change, another or reject.".into()),
    }
}

/// A reply: its words, or why it could not be done.
fn said(kind: &str, step: Option<u64>, r: Result<String, String>) -> Value {
    let mut v = match r {
        Ok(text) => json!({"type": kind, "text": text}),
        Err(e) => json!({"type": kind, "error": e}),
    };
    if let Some(n) = step {
        v["step"] = json!(n);
    }
    v
}

/// The checklists, as the faces show them: the one on in the session's chat, the person's, the repository's, and
/// why there are none when the session is not kept in Arena or Arena did not answer.
async fn listing(sh: &Shared) -> Value {
    let root = sh.workspace.root().to_path_buf();
    let repo =
        tokio::task::spawn_blocking(move || ca_arena::checklists::repo_checklists(&root)).await.unwrap_or_default();
    let Some(sync) = sh.sync.clone() else {
        return checklists_said(None, &[], &repo, Some(crate::checklist::SIGN_IN.into()), false);
    };
    let arena = sync.arena();
    let mut problem = None;
    let mine = arena.checklists().await.unwrap_or_else(|e| {
        problem = Some(e.to_string());
        Vec::new()
    });
    let on = match sync.chat_id() {
        Some(chat) => arena.checklist_run(&chat).await.unwrap_or_else(|e| {
            problem.get_or_insert(e.to_string());
            None
        }),
        None => None,
    };
    let kept = ca_arena::Kept::read(&ca_arena::Kept::file(&sh.paths.servers_dir(), arena.base()));
    let edit = kept.is_none_or(|k| k.capabilities.has(ca_arena::Feature::ChecklistsEdit));
    checklists_said(on.as_ref(), &mine, &repo, problem, edit)
}

/// `checklists{on, mine, repo, problem, edit}`: `on` is `{name, checklist_id, done, total, items: [{number, text,
/// done, note, by}]}` or null; `mine` `[{id, name, items}]`; `repo` `[{name, items}]`; `edit` whether Arena makes,
/// changes and deletes them.
fn checklists_said(
    on: Option<&ca_arena::checklists::Run>,
    mine: &[ca_arena::checklists::Checklist],
    repo: &[ca_arena::checklists::RepoChecklist],
    problem: Option<String>,
    edit: bool,
) -> Value {
    let on = on.map(|r| {
        let items: Vec<Value> = r
            .items
            .iter()
            .map(|i| json!({"number": i.number, "text": i.text, "done": i.done, "note": i.note, "by": i.by}))
            .collect();
        json!({"name": r.name, "checklist_id": r.checklist_id, "done": r.done, "total": r.total, "items": items})
    });
    let mine: Vec<Value> = mine.iter().map(|c| json!({"id": c.id, "name": c.name, "items": c.items})).collect();
    let repo: Vec<Value> = repo.iter().map(|c| json!({"name": c.name, "items": c.items})).collect();
    json!({"type": "checklists", "on": on, "mine": mine, "repo": repo, "problem": problem, "edit": edit})
}

/// A checklist turned on (`id`, or the repository's by its name, `repo`), off, an item ticked (`n`, `done`), one
/// saved (`id` when it is changed, `name`, `items`) or deleted (`id`).
async fn checklist(p: &mut crate::session::Prepared, m: &Value) -> Result<(), String> {
    let sync = p.sync.clone().ok_or(crate::checklist::SIGN_IN)?;
    let arena = sync.arena().clone();
    let e = |e: ca_arena::ArenaError| e.to_string();
    let text = |k: &str| m[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let none_on = "No checklist is on in this session.";
    match crate::ide_ext::kind(m) {
        "checklist_on" => {
            let id = match (text("id"), text("repo")) {
                (Some(id), _) => id,
                (None, Some(name)) => {
                    let lists = arena.checklists().await.map_err(e)?;
                    let root = p.agent.ctx.workspace.root().to_path_buf();
                    crate::checklist::repo_as_mine(&arena, &lists, &root, &name).await?.id
                }
                _ => return Err("Say which checklist to turn on.".into()),
            };
            crate::checklist::turn_on(p, &id).await.map(|_| ())
        }
        "checklist_off" => match sync.chat_id() {
            Some(chat) => arena.stop_checklist(&chat).await.map_err(e),
            None => Ok(()),
        },
        "checklist_tick" => {
            let n = m["n"].as_u64().and_then(|n| u32::try_from(n).ok()).ok_or("Say which item to tick.")?;
            let chat = sync.chat_id().ok_or(none_on)?;
            let done = m["done"].as_bool() != Some(false);
            arena.tick(&chat, n, done, None, "person").await.map_err(e)?.map(|_| ()).ok_or(none_on.into())
        }
        "checklist_save" => {
            let (name, items) = (text("name").unwrap_or_default(), items_of(&m["items"]));
            if name.is_empty() || items.is_empty() {
                return Err("A checklist has a name and at least one item.".into());
            }
            arena.save_checklist(text("id").as_deref(), &name, &items).await.map(|_| ()).map_err(e)
        }
        "checklist_delete" => {
            arena.delete_checklist(&text("id").ok_or("Say which checklist to delete.")?).await.map_err(e)
        }
        _ => Err("Not a request of the checklists.".into()),
    }
}

/// A checklist's items as the face sent them: trimmed, the empty ones left out.
fn items_of(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|i| !i.is_empty())
        .map(str::to_string)
        .collect()
}

/// The reply to a checklist request, under its own name with the keys it carried (`id`, `repo`, `n`, `name`), and
/// why it could not be done.
fn answered(m: &Value, done: Result<(), String>) -> Value {
    let mut v = json!({"type": crate::ide_ext::kind(m)});
    for k in ["id", "repo", "n", "done", "name"] {
        if !m[k].is_null() {
            v[k] = m[k].clone();
        }
    }
    if let Err(e) = done {
        v["error"] = json!(e);
    }
    v
}

/// A plan left, as `tasks` says it: `{title, done, total}`; none when it cannot be read, is empty or is done (as
/// `session::resume_plan` would refuse it).
fn left_of(json: &str) -> Option<Value> {
    let plan: ca_tools::plan::Plan = serde_json::from_str(json).ok()?;
    let (done, total) = (plan.done(), plan.steps.len());
    (total > 0 && done < total).then(|| json!({"title": plan.title, "done": done, "total": total}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_review_is_said_in_the_terminals_words() {
        let r = |v: Value| review_words(&v);
        assert_eq!(r(json!({"action": "approve"})), Ok(("approve", String::new())));
        assert_eq!(r(json!({"action": "approve-ask"})), Ok(("approve", "ask".into())));
        assert_eq!(
            r(json!({"action": "change", "step": 2, "text": " use a map "})),
            Ok(("change", "2 use a map".into()))
        );
        assert!(r(json!({"action": "change", "text": "x"})).is_err(), "a change names its step");
        assert_eq!(r(json!({"action": "another", "text": "too slow"})), Ok(("another", "too slow".into())));
        assert_eq!(r(json!({"action": "reject"})), Ok(("reject", String::new())));
        assert!(r(json!({"action": "maybe"})).is_err());
    }

    #[test]
    fn a_plan_left_is_offered_only_while_some_of_it_is_to_do() {
        let plan = |a: &str, b: &str| {
            format!(
                r#"{{"title":"T","steps":[{{"number":1,"title":"a","status":"{a}"}},{{"number":2,"title":"b","status":"{b}"}}]}}"#
            )
        };
        assert_eq!(left_of(&plan("done", "todo")), Some(json!({"title": "T", "done": 1, "total": 2})));
        assert_eq!(left_of(&plan("done", "done")), None);
        assert_eq!(left_of(r#"{"title":"T","steps":[]}"#), None);
        assert_eq!(left_of("not json"), None);
    }

    #[test]
    fn the_checklists_are_said_with_the_one_on_and_why_there_are_none() {
        use ca_arena::checklists::{Checklist, RepoChecklist, Run};
        let run: Run = serde_json::from_value(json!({"checklistId": "k1", "name": "Before a merge", "done": 1,
            "total": 2, "items": [{"number": 1, "text": "tests", "done": true, "note": "42 passed", "by": "model"},
            {"number": 2, "text": "docs"}]}))
        .expect("run");
        let mine = [Checklist {
            id: "k1".into(),
            name: "Before a merge".into(),
            description: None,
            items: vec!["tests".into(), "docs".into()],
        }];
        let repo = [RepoChecklist { name: "Release".into(), items: vec!["tag".into()], file: "r.md".into() }];
        let v = checklists_said(Some(&run), &mine, &repo, None, true);
        assert_eq!(
            v["on"]["items"][0],
            json!({"number": 1, "text": "tests", "done": true, "note": "42 passed",
            "by": "model"})
        );
        assert_eq!((v["on"]["done"].clone(), v["on"]["checklist_id"].clone()), (json!(1), json!("k1")));
        assert_eq!(v["mine"], json!([{"id": "k1", "name": "Before a merge", "items": ["tests", "docs"]}]));
        assert_eq!(v["repo"], json!([{"name": "Release", "items": ["tag"]}]));
        let none = checklists_said(None, &[], &[], Some("sign in".into()), false);
        assert_eq!(
            (none["on"].clone(), none["problem"].clone(), none["edit"].clone()),
            (Value::Null, json!("sign in"), json!(false))
        );
    }

    #[test]
    fn a_checklist_reply_carries_its_keys_and_items_are_trimmed() {
        let m = json!({"type": "checklist_tick", "n": 2, "done": false});
        assert_eq!(answered(&m, Ok(())), json!({"type": "checklist_tick", "n": 2, "done": false}));
        let m = json!({"type": "checklist_delete", "id": "k1"});
        assert_eq!(answered(&m, Err("no".into())), json!({"type": "checklist_delete", "id": "k1", "error": "no"}));
        assert_eq!(items_of(&json!([" a ", "", "  ", "b", 3])), vec!["a".to_string(), "b".into()]);
    }

    #[test]
    fn a_reply_carries_its_step_and_its_words_or_why_not() {
        assert_eq!(said("plan_undo", Some(2), Ok("x".into())), json!({"type": "plan_undo", "step": 2, "text": "x"}));
        assert_eq!(said("plan_resume", None, Err("no".into())), json!({"type": "plan_resume", "error": "no"}));
    }
}
