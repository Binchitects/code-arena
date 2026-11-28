//! The Automation view's side of the driver: this machine's schedules, and the person's agents and scheduled
//! tasks in Arena.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn). The schedules are `schedules.rs`'s, as `code-arena
//! schedule` keeps them; those due run while the driver runs (each minute it looks), as well as by the system's timer
//! once `code-arena schedule install` set it. The agents and tasks are Arena's (8.1 on), as `/arena-agents` and
//! `/arena-tasks` have them.
//!
//! Messages, all answered while a turn runs too:
//! - `automation{key?}` [T]: answered by `automation{key, folder, schedules: [{name, prompt, folder, every,
//!   every_words, timing, at, mode, model, on, running, next, next_words, last_run, last_words, last_status}]}`.
//!   Dates are seconds since 1970 with their local words ("2026-11-28 09:00"); `folder` is the session's, where a
//!   new schedule runs unless another is given (null when the due ones' run sent it).
//! - `schedule_save{name, every, at?, mode, folder, prompt, model?}` [T]: `every` is "30m", "2h", "1d" or seconds.
//!   A `notice{text, of, key, error?}` (what was added, or why not), then `automation`.
//! - `schedule_run{name}` [T]: `automation` at once (it is running), then, when its run ends, a `notice` (how it
//!   ended and where its output is) and `automation` again.
//! - `schedule_on{name, on}`, `schedule_remove{name}` [T]: a `notice` when it cannot be, and `automation`.
//! - `arena_agents{key?}` [T]: `arena_agents{key, agents: [{id, name, description, enabled, mine, owner, trigger}],
//!   error?}`; `error` says why there are none to show (not signed in, an Arena without them).
//! - `arena_agent_run{id, text}` [T]: `arena_agent_run{key, id, run}` (the run's id), or with `error`.
//! - `arena_agent_run_get{run}` [T]: `arena_agent_run_get{key, run, workflow, status, ended, waiting, error, answer,
//!   told, steps: [{name, kind, status, output, error}]}`, or `{run, error}` when it cannot be read.
//! - `arena_tasks{key?}` [T]: `arena_tasks{key, tasks: [{id, name, prompt, cron, time_zone, when, enabled, running,
//!   next_words, last_status, last_words, last_link}], error?}`.
//! - `arena_task_run{id, name?}` [T]: a `notice` (started, running already, or why not), then `arena_tasks`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ca_arena::automation::{Agent, AgentRun, Started, Task};
use ca_arena::{ArenaClient, Feature};
use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, Shared, kind, send};
use crate::schedules::{self, New, Schedule};

/// How often the due schedules are looked for while the driver runs.
const TICK: Duration = Duration::from_secs(60);

/// Said when no Arena is signed in to.
const NO_ARENA: &str = "Sign in to Arena to see your agents and scheduled tasks there.";

/// This area in the driver.
pub struct Here;

/// Whether the due schedules are looked for already (once for the driver's life, whatever session it has).
#[derive(Default)]
struct Ticking(AtomicBool);

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("automation", Rule::Turn),
        ("schedule_save", Rule::Turn),
        ("schedule_run", Rule::Turn),
        ("schedule_on", Rule::Turn),
        ("schedule_remove", Rule::Turn),
        ("arena_agents", Rule::Turn),
        ("arena_agent_run", Rule::Turn),
        ("arena_agent_run_get", Rule::Turn),
        ("arena_tasks", Rule::Turn),
        ("arena_task_run", Rule::Turn),
    ];

    fn start(sh: &Shared) {
        if cfg!(test) || sh.kept.of::<Ticking>().0.swap(true, Ordering::SeqCst) {
            return;
        }
        let (config, out) = (sh.paths.config.clone(), sh.out.clone());
        let Ok(exe) = std::env::current_exe() else { return };
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(TICK).await;
                let (c, e) = (config.clone(), exe.clone());
                // One look at a time: the next waits for this one's runs to end.
                let ran = tokio::task::spawn_blocking(move || run_due(&c, &e)).await.unwrap_or(0);
                if ran > 0 {
                    send(&out, listed(&config, None, &Value::Null));
                }
            }
        });
    }

    fn shared(sh: &Shared, m: &Value) -> bool {
        let config = sh.paths.config.clone();
        let key = m["key"].clone();
        let name = m["name"].as_str().unwrap_or("").to_string();
        match kind(m) {
            "automation" => send(&sh.out, listed(&config, Some(&sh.folder), &key)),
            "schedule_save" => {
                let said = saved(&config, m);
                send(&sh.out, notice("schedule_save", &key, said));
                send(&sh.out, listed(&config, Some(&sh.folder), &Value::Null));
            }
            "schedule_on" | "schedule_remove" => {
                let said = if kind(m) == "schedule_on" {
                    set_on(&config, &name, m["on"].as_bool().unwrap_or(true))
                } else {
                    removed(&config, &name)
                };
                if said.is_err() {
                    send(&sh.out, notice(kind(m), &key, said));
                }
                send(&sh.out, listed(&config, Some(&sh.folder), &Value::Null));
            }
            "schedule_run" => {
                let (out, folder) = (sh.out.clone(), sh.folder.clone());
                let Ok(exe) = std::env::current_exe() else {
                    send(&out, notice("schedule_run", &key, Err("This program's own path is not known.".into())));
                    return true;
                };
                tokio::task::spawn_blocking(move || {
                    let said = run_one(&config, &exe, &name, || {
                        send(&out, listed(&config, Some(&folder), &Value::Null));
                    });
                    send(&out, notice("schedule_run", &key, said));
                    send(&out, listed(&config, Some(&folder), &Value::Null));
                });
            }
            "arena_agents" | "arena_tasks" => {
                let (out, paths, arena, k) = (sh.out.clone(), sh.paths.clone(), sh.arena.clone(), kind(m).to_string());
                tokio::spawn(async move {
                    let v = if k == "arena_agents" {
                        agents_listed(&paths, arena.as_ref()).await
                    } else {
                        tasks_listed(&paths, arena.as_ref()).await
                    };
                    send(&out, keyed(v, &key));
                });
            }
            "arena_agent_run" => {
                let (out, arena) = (sh.out.clone(), sh.arena.clone());
                let (id, text) =
                    (m["id"].as_str().unwrap_or("").to_string(), m["text"].as_str().unwrap_or("").to_string());
                tokio::spawn(async move {
                    let v = match arena {
                        None => json!({"type": "arena_agent_run", "id": id, "error": NO_ARENA}),
                        Some(a) => started(&a, &id, &text).await,
                    };
                    send(&out, keyed(v, &key));
                });
            }
            "arena_agent_run_get" => {
                let (out, arena) = (sh.out.clone(), sh.arena.clone());
                let run = m["run"].as_str().unwrap_or("").to_string();
                tokio::spawn(async move {
                    let v = match arena {
                        None => json!({"type": "arena_agent_run_get", "run": run, "error": NO_ARENA}),
                        Some(a) => match a.agent_run(&run).await {
                            Ok(r) => run_of(&r),
                            Err(e) => json!({"type": "arena_agent_run_get", "run": run, "error": e.to_string()}),
                        },
                    };
                    send(&out, keyed(v, &key));
                });
            }
            "arena_task_run" => {
                let (out, paths, arena) = (sh.out.clone(), sh.paths.clone(), sh.arena.clone());
                let id = m["id"].as_str().unwrap_or("").to_string();
                let name = if name.is_empty() { "The task".to_string() } else { name };
                tokio::spawn(async move {
                    let said = match &arena {
                        None => Err(NO_ARENA.to_string()),
                        Some(a) => match a.run_scheduled(&id).await {
                            Ok(Started::Started) => {
                                Ok(format!("{name} runs now in Arena: its answer comes to you there."))
                            }
                            Ok(Started::Running(w)) => Err(w),
                            Err(e) => Err(e.to_string()),
                        },
                    };
                    send(&out, notice("arena_task_run", &key, said));
                    send(&out, tasks_listed(&paths, arena.as_ref()).await);
                });
            }
            _ => return false,
        }
        true
    }
}

/// The key of the message answered, put in its reply.
fn keyed(mut v: Value, key: &Value) -> Value {
    if !key.is_null() {
        v["key"] = key.clone();
    }
    v
}

/// A sentence begun with a capital (the shell's errors begin small).
fn capital(text: &str) -> String {
    let mut c = text.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// What a change said, as a corner bubble's `notice`: a sentence ended with a stop.
fn notice(of: &str, key: &Value, said: Result<String, String>) -> Value {
    let (text, error) = match said {
        Ok(t) => (t, false),
        Err(e) => (e, true),
    };
    let mut text = text.trim().to_string();
    if !text.ends_with(['.', '!', '?', ')']) {
        text.push('.');
    }
    keyed(json!({"type": "notice", "text": text, "of": of, "error": error}), key)
}

/// A time (seconds since 1970) in local words: "2026-11-28 09:00".
fn words(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// An Arena time ("2026-11-27T05:30:00Z") in local words, or as it came when it is not one.
fn iso_words(t: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(t)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| t.to_string())
}

/// Whether it runs now.
fn running(s: &Schedule) -> bool {
    s.running(schedules::now())
}

/// One schedule, as the view shows it.
fn schedule_of(s: &Schedule) -> Value {
    let every = schedules::interval_words(s.every);
    let timing = match &s.at {
        Some(at) if s.every >= 86_400 => format!("every {every} at {at}"),
        _ => format!("every {every}"),
    };
    json!({
        "name": s.name,
        "prompt": s.prompt,
        "folder": s.folder.to_string_lossy(),
        "every": s.every,
        "every_words": every,
        "timing": timing,
        "at": s.at,
        "mode": s.mode,
        "model": s.model,
        "on": !s.off,
        "running": running(s),
        "next": s.next(),
        "next_words": words(s.next()),
        "last_run": s.last_run,
        "last_words": s.last_run.map(words),
        "last_status": s.last_status,
    })
}

/// The `automation` reply.
fn listed(config: &Path, folder: Option<&str>, key: &Value) -> Value {
    let all: Vec<Value> = schedules::load(config).iter().map(schedule_of).collect();
    keyed(json!({"type": "automation", "folder": folder, "schedules": all}), key)
}

/// A new schedule kept, from `schedule_save`: what was added, or why not.
fn saved(config: &Path, m: &Value) -> Result<String, String> {
    let every = match &m["every"] {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => schedules::interval(s),
        _ => None,
    }
    .ok_or("How often: every 30m, 2h, 1d.")?;
    let text = |k: &str| m[k].as_str().unwrap_or("").to_string();
    let mut all = schedules::load(config);
    let s = schedules::checked(
        &all,
        New {
            name: text("name"),
            prompt: text("prompt"),
            folder: PathBuf::from(text("folder")),
            every,
            at: m["at"].as_str().map(str::to_string),
            mode: text("mode"),
            model: m["model"].as_str().map(str::to_string),
        },
    )
    .map_err(|e| capital(&format!("{e:#}")))?;
    let said = format!(
        "Schedule {} added: it runs {}, next {}",
        s.name,
        schedule_of(&s)["timing"].as_str().unwrap_or(""),
        words(s.next())
    );
    all.push(s);
    schedules::save(config, &all).map_err(|e| e.to_string())?;
    Ok(said)
}

/// A schedule turned on or off.
fn set_on(config: &Path, name: &str, on: bool) -> Result<String, String> {
    let mut all = schedules::load(config);
    let s = all.iter_mut().find(|s| s.name == name).ok_or_else(|| format!("There is no schedule {name}"))?;
    s.off = !on;
    schedules::save(config, &all).map_err(|e| e.to_string())?;
    Ok(format!("{name} is {}", if on { "on" } else { "off" }))
}

/// A schedule removed.
fn removed(config: &Path, name: &str) -> Result<String, String> {
    let mut all = schedules::load(config);
    let before = all.len();
    all.retain(|s| s.name != name);
    if all.len() == before {
        return Err(format!("There is no schedule {name}"));
    }
    schedules::save(config, &all).map_err(|e| e.to_string())?;
    Ok(format!("Removed {name}"))
}

/// Runs one now (it blocks until its run ends); `began` once it is marked running. How it ended.
fn run_one(config: &Path, exe: &Path, name: &str, began: impl FnOnce()) -> Result<String, String> {
    let s = schedules::load(config)
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("There is no schedule {name}"))?;
    if running(&s) {
        return Err(format!("{name} is running now"));
    }
    schedules::change(config, name, |x| {
        x.last_run = Some(schedules::now());
        x.last_status = Some("running".into());
    })
    .map_err(|e| e.to_string())?;
    began();
    let code = schedules::run(config, exe, &s).map_err(|e| e.to_string())?;
    let log = config.join("schedules").join(format!("{name}.log"));
    if code == 0 {
        Ok(format!("{name} ran: ok (its output: {})", log.display()))
    } else {
        Err(format!("{name} failed (exit code {code}; its output: {})", log.display()))
    }
}

/// Runs those due now, each looked at again just before (the system's timer or another window may have run it).
/// How many ran.
fn run_due(config: &Path, exe: &Path) -> usize {
    let names: Vec<String> =
        schedules::load(config).into_iter().filter(|s| s.due(schedules::now())).map(|s| s.name).collect();
    let mut ran = 0;
    for name in names {
        let Some(s) = schedules::load(config).into_iter().find(|s| s.name == name) else { continue };
        if s.due(schedules::now()) {
            ran += 1;
            let _ = schedules::run(config, exe, &s);
        }
    }
    ran
}

/// One agent, as the view shows it.
fn agent_of(a: &Agent) -> Value {
    json!({"id": a.id, "name": a.name, "description": a.description, "enabled": a.enabled, "mine": a.mine,
           "owner": a.owner, "trigger": a.trigger})
}

/// The `arena_agents` reply: the agents, or why there are none to show.
async fn agents_listed(paths: &ca_core::Paths, arena: Option<&ArenaClient>) -> Value {
    let got = async {
        let a = arena.ok_or(NO_ARENA.to_string())?;
        crate::automation::usable(paths, a, Feature::AutomationAgents).await?;
        a.agents().await.map_err(|e| e.to_string())
    };
    match got.await {
        Ok(list) => json!({"type": "arena_agents", "agents": list.iter().map(agent_of).collect::<Vec<_>>()}),
        Err(e) => json!({"type": "arena_agents", "agents": [], "error": e}),
    }
}

/// An agent started with what it is given: the `arena_agent_run` reply.
async fn started(arena: &ArenaClient, id: &str, text: &str) -> Value {
    let text = text.trim();
    match arena.run_agent(id, (!text.is_empty()).then_some(text), &[]).await {
        Ok(run) => json!({"type": "arena_agent_run", "id": id, "run": run}),
        Err(e) => json!({"type": "arena_agent_run", "id": id, "error": e.to_string()}),
    }
}

/// A run as it stands: the `arena_agent_run_get` reply.
fn run_of(r: &AgentRun) -> Value {
    let steps: Vec<Value> = r
        .steps
        .iter()
        .map(|s| json!({"name": s.name, "kind": s.kind, "status": s.status, "output": s.output, "error": s.error}))
        .collect();
    json!({
        "type": "arena_agent_run_get",
        "run": r.id,
        "workflow": r.workflow,
        "status": r.status,
        "ended": r.ended(),
        "waiting": r.status == "waiting",
        "error": r.error,
        "answer": r.answer(),
        "told": crate::automation::told(r),
        "steps": steps,
    })
}

/// One scheduled task, as the view shows it.
fn task_of(t: &Task) -> Value {
    let when = if t.trigger.as_deref().is_some_and(|x| x != "schedule") {
        format!("on {} events", t.trigger.clone().unwrap_or_default())
    } else {
        format!("{} {}", t.cron, t.time_zone.clone().unwrap_or_default()).trim().to_string()
    };
    let last = t.last_run.as_ref();
    json!({
        "id": t.id,
        "name": t.name,
        "prompt": t.prompt,
        "cron": t.cron,
        "time_zone": t.time_zone,
        "when": when,
        "enabled": t.enabled,
        "running": t.running,
        "next_words": t.next_runs.first().map(|n| iso_words(n)),
        "last_status": last.map(|r| r.status.clone()),
        "last_words": last.and_then(|r| r.finished_at.as_deref().or(r.started_at.as_deref())).map(iso_words),
        "last_link": last.and_then(|r| r.link.clone()),
    })
}

/// The `arena_tasks` reply: the tasks, or why there are none to show.
async fn tasks_listed(paths: &ca_core::Paths, arena: Option<&ArenaClient>) -> Value {
    let got = async {
        let a = arena.ok_or(NO_ARENA.to_string())?;
        crate::automation::usable(paths, a, Feature::AutomationScheduled).await?;
        a.scheduled().await.map_err(|e| e.to_string())
    };
    match got.await {
        Ok(list) => json!({"type": "arena_tasks", "tasks": list.iter().map(task_of).collect::<Vec<_>>()}),
        Err(e) => json!({"type": "arena_tasks", "tasks": [], "error": e}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ca_core::Secret;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn save_msg(folder: &Path, name: &str, every: &str) -> Value {
        json!({"type": "schedule_save", "name": name, "every": every, "at": "09:00", "mode": "plan",
               "folder": folder.to_string_lossy(), "prompt": "Run the tests."})
    }

    #[test]
    fn a_schedule_is_saved_listed_turned_off_and_removed() {
        let (config, work) = (tempfile::tempdir().expect("config"), tempfile::tempdir().expect("work"));
        let c = config.path();
        let said = saved(c, &save_msg(work.path(), "nightly", "1d")).expect("saved");
        assert!(said.starts_with("Schedule nightly added: it runs every 1d at 09:00, next "), "{said}");
        assert!(
            saved(c, &save_msg(work.path(), "nightly", "1d"))
                .expect_err("twice")
                .starts_with("There is a schedule nightly already")
        );
        assert!(saved(c, &save_msg(work.path(), "x", "often")).expect_err("every").contains("How often"));
        let v = listed(c, Some("/w"), &json!(7));
        assert_eq!((v["key"].clone(), v["folder"].clone()), (json!(7), json!("/w")));
        let s = &v["schedules"][0];
        assert_eq!(
            (s["name"].as_str(), s["timing"].as_str(), s["on"].as_bool()),
            (Some("nightly"), Some("every 1d at 09:00"), Some(true))
        );
        assert!(s["next_words"].as_str().is_some_and(|w| w.ends_with("09:00")));
        assert!(s["last_status"].is_null() && !s["running"].as_bool().unwrap_or(true));
        set_on(c, "nightly", false).expect("off");
        assert_eq!(listed(c, None, &Value::Null)["schedules"][0]["on"], json!(false));
        assert!(set_on(c, "nope", true).is_err());
        removed(c, "nightly").expect("removed");
        assert_eq!(listed(c, None, &Value::Null)["schedules"], json!([]));
        assert!(removed(c, "nightly").is_err());
    }

    #[test]
    fn a_schedule_run_marks_it_running_then_says_how_it_ended() {
        let (config, work) = (tempfile::tempdir().expect("config"), tempfile::tempdir().expect("work"));
        let c = config.path();
        saved(c, &save_msg(work.path(), "once", "2h")).expect("saved");
        let mut seen = None;
        // No such program: its run fails, as one whose process could not start.
        let said = run_one(c, Path::new("/no/such/code-arena"), "once", || {
            seen = Some(listed(c, None, &Value::Null)["schedules"][0]["running"].clone());
        });
        assert_eq!(seen, Some(json!(true)), "running while it runs");
        assert!(said.expect_err("failed").contains("once failed (exit code -1"));
        let s = &listed(c, None, &Value::Null)["schedules"][0];
        assert_eq!(s["last_status"], json!("failed (exit code -1)"));
        assert!(s["last_words"].as_str().is_some() && s["running"] == json!(false));
        assert!(run_one(c, Path::new("/x"), "nope", || {}).is_err());
        let n = notice("schedule_run", &json!("k"), Err("once failed".into()));
        assert_eq!(
            n,
            json!({"type": "notice", "text": "once failed.", "of": "schedule_run", "error": true, "key": "k"})
        );
    }

    #[tokio::test]
    async fn an_agent_is_run_with_what_it_is_given_and_its_steps_followed() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/code-arena/agents/a1/run"))
            .and(body_json(json!({"text": "look at !12", "fields": null})))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"id": "r1"})))
            .mount(&s)
            .await;
        Mock::given(path("/api/code-arena/agents/runs/r1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "r1", "workflow": "Reviewer", "status": "done",
                "steps": [{"kind": "llm", "name": "Read", "status": "done", "output": "Looks right."}]
            })))
            .mount(&s)
            .await;
        let a = ArenaClient::new(&s.uri(), Some(Secret::new("sk-fake")), None).expect("client");
        let v = started(&a, "a1", " look at !12 ").await;
        assert_eq!(v, json!({"type": "arena_agent_run", "id": "a1", "run": "r1"}));
        let r = run_of(&a.agent_run("r1").await.expect("run"));
        assert_eq!((r["ended"].clone(), r["answer"].clone()), (json!(true), json!("Looks right.")));
        assert_eq!(r["steps"][0]["name"], json!("Read"));
        assert!(r["told"].as_str().is_some_and(|t| t.starts_with("Reviewer is done")));
        let none = agents_listed(&ca_core::Paths::under(Path::new("/tmp/no-home")), None).await;
        assert_eq!(none["error"], json!(NO_ARENA));
    }
}
