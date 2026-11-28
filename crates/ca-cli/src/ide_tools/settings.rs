//! Configure Tools in the driver: each tool's own settings read from where they are kept, applied to the session's
//! tools, and saved.
//!
//! - `tool_settings{key?}` [T] → `tool_settings{key, tools:[{name, group, group_title, kind, description,
//!   arguments:[{name, type, description}], enabled, group_on, asks, person, here, project, locked}], own, servers,
//!   project_file}`: every tool of the session as it is defined, with its settings at each level (`person`: theirs
//!   for every project; `here`: theirs for this project; `project`: the project's file, off and ask only). `asks`
//!   is how it asks now: ask, allow (never), mode (as the mode says of its kind) or never (it only reads). `locked`
//!   says why it may not run unasked, or is null. `own` and `servers` are the project's own tools and MCP servers
//!   waiting to be enabled: none yet, as Code Arena reads neither from a project.
//! - `tool_settings_set{name, level: 'person'|'here', settings:{enabled, approval, description, defaults, fixed},
//!   key?}` [Q] → `tool_settings{key: name, name, ...}`, or `refused{of, key: name, reason}`. Empty settings forget
//!   the tool's at that level.
//! - `tool_enable{key, on}` [Q] → `refused{of, key, reason}`: there is nothing of the project's to enable yet.
//!
//! The person's for every project are `tools.settings` in their settings.json; theirs for this project are kept in
//! the data folder's `tool-settings.json` by folder, never in the project; the project's are `tools.settings` in
//! its `.code-arena/settings.json`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ca_agent::tool_settings::{self as ts, Approval, Approvals, Choice, Levels};
use ca_tools::{Kind, Tool};
use serde_json::{Value, json};

use super::{Kept, OWN, Off, Row, ServerRow, server_rows};
use crate::ide_ext::{Ctx, Shared, refuse, send};
use crate::session::Prepared;

/// The setting the person's and the project's are kept under.
fn key() -> &'static str {
    ca_settings::schema::TOOL_SETTINGS.key
}

/// The file in the data folder the person's settings for each folder are kept in: `{"folders": {"/abs": {…}}}`.
const HERE_FILE: &str = "tool-settings.json";

/// The project's settings file, as the dialog names it.
const PROJECT_FILE: &str = ".code-arena/settings.json";

/// Where each level is kept.
struct Places {
    person: PathBuf,
    here: PathBuf,
    project: PathBuf,
    folder: String,
}

impl Places {
    fn of(paths: &ca_core::Paths, root: &Path) -> Places {
        Places {
            person: paths.settings_file(),
            here: paths.data.join(HERE_FILE),
            project: root.join(".code-arena").join("settings.json"),
            folder: root.to_string_lossy().into_owned(),
        }
    }

    fn here_all(&self) -> Value {
        read_json(&self.here).unwrap_or_else(|| json!({"folders": {}}))
    }

    /// The settings at the three levels, read now.
    fn levels(&self) -> Levels {
        let at = |f: &Path| read_json(f).and_then(|v| v.get(key()).cloned()).unwrap_or_else(|| json!({}));
        let here = self.here_all()["folders"].get(&self.folder).cloned().unwrap_or_else(|| json!({}));
        Levels::new(&at(&self.person), &here, &at(&self.project))
    }

    /// Keeps one tool's settings at a level, the others as they are.
    fn save(&self, level: &str, name: &str, choice: &Choice) -> Result<(), String> {
        let failed = |e: std::io::Error| format!("Not saved: {e}");
        match level {
            "person" => {
                let all = read_json(&self.person).and_then(|v| v.get(key()).cloned()).unwrap_or_else(|| json!({}));
                ca_settings::settings::keep(&self.person, key(), &ts::with(&all, name, choice)).map_err(failed)
            }
            "here" => {
                let mut all = self.here_all();
                if !all["folders"].is_object() {
                    all["folders"] = json!({});
                }
                let mine = all["folders"].get(&self.folder).cloned().unwrap_or_else(|| json!({}));
                let mine = ts::with(&mine, name, choice);
                if let Some(f) = all["folders"].as_object_mut() {
                    if mine.as_object().is_some_and(|o| o.is_empty()) {
                        f.remove(&self.folder);
                    } else {
                        f.insert(self.folder.clone(), mine);
                    }
                }
                if let Some(dir) = self.here.parent() {
                    std::fs::create_dir_all(dir).map_err(failed)?;
                }
                let text = serde_json::to_string_pretty(&all).unwrap_or_default();
                std::fs::write(&self.here, text + "\n").map_err(failed)
            }
            _ => {
                Err("Tool settings are kept for you (person: every project) or for you in this project (here).".into())
            }
        }
    }
}

/// A settings file read, its comments left out; none when it is not there or not JSON.
fn read_json(file: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(file).ok()?;
    ca_settings::jsonc::parse(&text).ok().filter(Value::is_object)
}

/// Why a tool may not run unasked, or none: one that asks every time, or one of Arena's that asks first.
fn lock(name: &str, group: &str, kind: Kind, servers: &[ServerRow]) -> Option<&'static str> {
    if name == "stop_command" {
        return Some(ts::ALWAYS_ASKS);
    }
    let arena_asks = group == "arena" && kind != Kind::Read && !servers.iter().any(|s| s.name == "arena" && s.trusted);
    arena_asks.then_some(ts::ARENA_ASKS)
}

/// The session's tool lists, as settling them needs them.
struct Lists<'a> {
    pub tools: &'a mut Vec<Arc<dyn Tool>>,
    pub deferred: &'a mut Vec<Arc<dyn Tool>>,
    pub disabled: &'a mut Vec<Arc<dyn Tool>>,
}

/// What settling tells the session: how each tool asks, and which tools are off by their own settings.
struct Settled {
    pub approvals: Approvals,
    pub off: BTreeSet<String>,
}

/// Puts the settings in force on the session's tools: a tool turned off goes out of what the model is sent, one
/// turned back on (that this turned off) comes back unless its group or an assistant keeps it off, and the rest
/// are told of and given their arguments as the settings say.
fn settle(
    l: Lists<'_>,
    levels: &Levels,
    groups_off: &BTreeSet<String>,
    held: &[String],
    was_off: &BTreeSet<String>,
    locked: &dyn Fn(&dyn Tool) -> bool,
) -> Settled {
    let name = |t: &Arc<dyn Tool>| t.def().name;
    let off = |t: &Arc<dyn Tool>| levels.effective(&name(t), false).enabled == Some(false);
    let (back, still): (Vec<_>, Vec<_>) = l.disabled.drain(..).partition(|t| {
        let n = name(t);
        was_off.contains(&n) && !off(t) && !groups_off.contains(&t.group()) && !held.contains(&n)
    });
    *l.disabled = still;
    l.tools.extend(back);
    for list in [&mut *l.tools, &mut *l.deferred] {
        let (gone, kept): (Vec<_>, Vec<_>) = list.drain(..).partition(|t| off(t));
        *list = kept;
        l.disabled.extend(gone);
    }
    let mut approvals = Approvals::new();
    for t in l.tools.iter_mut().chain(l.deferred.iter_mut()) {
        let n = name(t);
        let c = levels.effective(&n, locked(t.as_ref()));
        if let Some(a) = c.approval {
            approvals.insert(n, a);
        }
        *t = ts::configure(t, &c);
    }
    let off = l.disabled.iter().filter(|t| off(t)).map(name).collect();
    Settled { approvals, off }
}

/// Settles the session's tools with the settings as they are kept now.
pub fn apply(sh: &Shared, p: &mut Prepared) {
    let levels = Places::of(&sh.paths, sh.workspace.root()).levels();
    let servers = server_rows(&sh.servers);
    let groups_off = sh.kept.of::<Off>().of(&sh.session);
    let was_off: BTreeSet<String> = p.tools_off.iter().cloned().collect();
    let locked = |t: &dyn Tool| lock(&t.def().name, &t.group(), t.kind(), &servers).is_some();
    let lists = Lists { tools: &mut p.agent.tools, deferred: &mut p.agent.deferred, disabled: &mut p.disabled };
    let s = settle(lists, &levels, &groups_off, &p.assistant_off, &was_off, &locked);
    p.agent.policy.approvals = s.approvals;
    p.tools_off = s.off.into_iter().collect();
    // The sub-agents start from the tools and the policy as they are now.
    crate::session::refresh_crew(p, p.ui);
}

/// Settles a session's tools as it starts, before any face asks: the terminal's too.
pub fn first(p: &mut Prepared, paths: &ca_core::Paths, root: &Path) {
    let levels = Places::of(paths, root).levels();
    let locked = |t: &dyn Tool| lock(&t.def().name, &t.group(), t.kind(), &[]).is_some();
    let lists = Lists { tools: &mut p.agent.tools, deferred: &mut p.agent.deferred, disabled: &mut p.disabled };
    let s = settle(lists, &levels, &BTreeSet::new(), &p.assistant_off, &BTreeSet::new(), &locked);
    p.agent.policy.approvals = s.approvals;
    p.tools_off = s.off.into_iter().collect();
}

fn kind_word(k: Kind) -> &'static str {
    match k {
        Kind::Read => "read",
        Kind::Edit => "edit",
        Kind::Shell => "shell",
    }
}

/// How a tool asks now, as the dialog says it.
fn asks(name: &str, kind: Kind, c: &Choice) -> &'static str {
    match (name, c.approval, kind) {
        ("stop_command", _, _) => "ask",
        (_, Some(Approval::Ask), _) => "ask",
        (_, Some(Approval::Allow), Kind::Read) | (_, None, Kind::Read) => "never",
        (_, Some(Approval::Allow), _) => "allow",
        _ => "mode",
    }
}

/// One tool as the dialog lists it.
fn entry(r: &Row, levels: &Levels, servers: &[ServerRow], groups_off: &BTreeSet<String>) -> Value {
    let locked = lock(&r.name, &r.group, r.kind, servers);
    let c = levels.effective(&r.name, locked.is_some());
    let (person, here, project) = levels.of(&r.name);
    let title = servers.iter().find(|s| s.name == r.group).map(|s| s.title.clone()).unwrap_or_else(|| r.group.clone());
    json!({
        "name": r.name,
        "group": r.group,
        "group_title": title,
        "kind": kind_word(r.kind),
        "description": r.about,
        "arguments": r.args.iter().map(|(n, t, d)| json!({"name": n, "type": t, "description": d})).collect::<Vec<_>>(),
        "enabled": c.enabled != Some(false),
        "group_on": r.group == OWN || !groups_off.contains(&r.group),
        "asks": asks(&r.name, r.kind, &c),
        "person": person.to_value(),
        "here": here.to_value(),
        "project": project.to_value(),
        "locked": locked,
    })
}

/// Every tool's entry, Code Arena's own first, then by group and name.
fn entries(rows: &[Row], levels: &Levels, servers: &[ServerRow], groups_off: &BTreeSet<String>) -> Vec<Value> {
    let mut rows: Vec<&Row> = rows.iter().collect();
    rows.sort_by_key(|r| (r.group != OWN, r.group.to_lowercase(), r.name.clone()));
    rows.dedup_by(|a, b| a.name == b.name);
    rows.into_iter().map(|r| entry(r, levels, servers, groups_off)).collect()
}

/// `tool_settings`: the tools as the session last stood still, with their settings read now.
pub fn message(sh: &Shared, key: &Value) -> Value {
    let rows = sh.kept.of::<Kept>().0.lock().ok().filter(|g| g.0 == sh.session).map(|g| g.1.clone());
    let places = Places::of(&sh.paths, sh.workspace.root());
    let groups_off = sh.kept.of::<Off>().of(&sh.session);
    let tools = entries(&rows.unwrap_or_default(), &places.levels(), &server_rows(&sh.servers), &groups_off);
    json!({
        "type": "tool_settings", "key": key, "tools": tools, "own": [], "servers": [],
        "project_file": places.project.exists().then_some(PROJECT_FILE),
    })
}

/// `tool_settings_set`: one tool's settings kept at a level, then put on the session's tools.
pub fn set(ctx: &mut Ctx<'_>, m: &Value) {
    let name = m["name"].as_str().unwrap_or("").to_string();
    let asked = json!({"type": "tool_settings_set", "key": name});
    let level = m["level"].as_str().unwrap_or("person");
    let choice = Choice::from_value(&m["settings"]);
    let p = &*ctx.p;
    let tool =
        p.agent.tools.iter().chain(p.agent.deferred.iter()).chain(p.disabled.iter()).find(|t| t.def().name == name);
    let Some(tool) = tool.map(ts::base) else {
        refuse(&ctx.sh.out, &asked, &format!("No tool {name}."));
        return;
    };
    let locked = lock(&name, &tool.group(), tool.kind(), &server_rows(&ctx.sh.servers));
    if let Some(why) = ts::refusal(&name, &choice, &tool.def(), locked) {
        refuse(&ctx.sh.out, &asked, &why);
        return;
    }
    let places = Places::of(&ctx.sh.paths, ctx.sh.workspace.root());
    if choice.enabled == Some(true) && places.levels().of(&name).2.enabled == Some(false) {
        refuse(&ctx.sh.out, &asked, &format!("Turned off by the project ({PROJECT_FILE})."));
        return;
    }
    if let Err(why) = places.save(level, &name, &choice) {
        refuse(&ctx.sh.out, &asked, &why);
        return;
    }
    apply(ctx.sh, ctx.p);
    super::keep(ctx.sh, ctx.p);
    let mut v = message(ctx.sh, &json!(name));
    v["name"] = json!(name);
    send(&ctx.sh.out, v);
}

/// `tool_enable`: the project's own tools and MCP servers, of which Code Arena reads none yet.
pub fn enable(ctx: &mut Ctx<'_>, m: &Value) {
    let asked = json!({"type": "tool_enable", "key": m["key"]});
    refuse(&ctx.sh.out, &asked, "The project has no tools or MCP servers of its own to enable.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use ca_llm::ToolDef;
    use ca_tools::{Context, Output};

    struct T(&'static str, &'static str, Kind);

    #[async_trait::async_trait]
    impl Tool for T {
        fn def(&self) -> ToolDef {
            ToolDef {
                name: self.0.into(),
                description: "d".into(),
                parameters: json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}),
            }
        }
        fn kind(&self) -> Kind {
            self.2
        }
        fn subject(&self, _a: &Value) -> String {
            String::new()
        }
        async fn run(&self, _a: Value, _c: &Context) -> Output {
            Output::ok("")
        }
        fn group(&self) -> String {
            self.1.into()
        }
    }

    fn tool(name: &'static str, group: &'static str) -> Arc<dyn Tool> {
        Arc::new(T(name, group, Kind::Edit))
    }

    fn names(v: &[Arc<dyn Tool>]) -> Vec<String> {
        v.iter().map(|t| t.def().name).collect()
    }

    #[test]
    fn a_tool_turned_off_goes_and_comes_back_unless_its_group_is_off() {
        let (mut tools, mut deferred, mut disabled) = (vec![tool("a", OWN), tool("b", "srv")], vec![], vec![]);
        let off = Levels::new(
            &json!({"a": {"enabled": false}, "b": {"enabled": false, "approval": "ask"}}),
            &json!({}),
            &json!({}),
        );
        let none = BTreeSet::new();
        let l = Lists { tools: &mut tools, deferred: &mut deferred, disabled: &mut disabled };
        let s = settle(l, &off, &none, &[], &none, &|_| false);
        assert!(tools.is_empty());
        assert_eq!(s.off, ["a".to_string(), "b".to_string()].into());
        // Turned on again while its group "srv" is off: only "a" comes back.
        let on = Levels::new(&json!({"b": {"approval": "ask"}}), &json!({}), &json!({}));
        let groups: BTreeSet<String> = ["srv".to_string()].into();
        let l = Lists { tools: &mut tools, deferred: &mut deferred, disabled: &mut disabled };
        let s = settle(l, &on, &groups, &[], &s.off, &|_| false);
        assert_eq!(names(&tools), ["a"]);
        assert_eq!(names(&disabled), ["b"]);
        assert!(s.off.is_empty());
    }

    #[test]
    fn a_tool_off_from_the_start_comes_back_when_turned_on() {
        let (mut tools, mut deferred, mut disabled) = (vec![tool("a", OWN)], vec![], vec![]);
        let off = Levels::new(&json!({"a": {"enabled": false}}), &json!({}), &json!({}));
        let none = BTreeSet::new();
        let l = Lists { tools: &mut tools, deferred: &mut deferred, disabled: &mut disabled };
        let first = settle(l, &off, &none, &[], &none, &|_| false);
        let l = Lists { tools: &mut tools, deferred: &mut deferred, disabled: &mut disabled };
        settle(l, &Levels::default(), &none, &[], &first.off, &|_| false);
        assert_eq!(names(&tools), ["a"]);
        assert!(disabled.is_empty());
    }

    #[test]
    fn a_tool_off_for_another_reason_is_not_brought_back() {
        let (mut tools, mut deferred, mut disabled) = (vec![], vec![], vec![tool("x", OWN), tool("y", OWN)]);
        let was: BTreeSet<String> = ["x".to_string(), "y".to_string()].into();
        let l = Lists { tools: &mut tools, deferred: &mut deferred, disabled: &mut disabled };
        settle(l, &Levels::default(), &BTreeSet::new(), &["y".to_string()], &was, &|_| false);
        assert_eq!(names(&tools), ["x"], "y is kept off by an assistant");
        let (mut tools, mut deferred, mut disabled) = (vec![], vec![], vec![tool("z", OWN)]);
        let l = Lists { tools: &mut tools, deferred: &mut deferred, disabled: &mut disabled };
        settle(l, &Levels::default(), &BTreeSet::new(), &[], &BTreeSet::new(), &|_| false);
        assert!(tools.is_empty(), "turned off with /tools, not by its settings");
    }

    #[test]
    fn settling_says_how_each_asks_and_a_locked_tool_never_runs_unasked() {
        let (mut tools, mut deferred, mut disabled) = (vec![tool("a", OWN)], vec![tool("b", OWN)], vec![]);
        let l = Levels::new(
            &json!({"a": {"approval": "allow", "description": "A."}, "b": {"approval": "allow"}}),
            &json!({}),
            &json!({}),
        );
        let lists = Lists { tools: &mut tools, deferred: &mut deferred, disabled: &mut disabled };
        let s = settle(lists, &l, &BTreeSet::new(), &[], &BTreeSet::new(), &|t| t.def().name == "b");
        assert_eq!(s.approvals.get("a"), Some(&Approval::Allow));
        assert_eq!(s.approvals.get("b"), None);
        assert_eq!(tools[0].def().description, "A.");
        assert_eq!(deferred.len(), 1);
    }

    #[test]
    fn locks_and_what_asks_says() {
        let arena =
            ServerRow { name: "arena".into(), title: "Arena's tools".into(), trusted: false, connected: Some(1) };
        assert_eq!(lock("stop_command", OWN, Kind::Shell, &[]), Some(ts::ALWAYS_ASKS));
        assert_eq!(lock("post", "arena", Kind::Edit, std::slice::from_ref(&arena)), Some(ts::ARENA_ASKS));
        assert_eq!(lock("look", "arena", Kind::Read, &[arena]), None);
        let c = |v: Value| Choice::from_value(&v);
        assert_eq!(asks("read_file", Kind::Read, &c(json!({}))), "never");
        assert_eq!(asks("read_file", Kind::Read, &c(json!({"approval": "ask"}))), "ask");
        assert_eq!(asks("run_shell", Kind::Shell, &c(json!({}))), "mode");
        assert_eq!(asks("run_shell", Kind::Shell, &c(json!({"approval": "allow"}))), "allow");
        assert_eq!(asks("stop_command", Kind::Shell, &c(json!({}))), "ask");
    }

    #[test]
    fn settings_are_kept_per_level_and_here_is_not_in_the_project() {
        let d = tempfile::tempdir().expect("temp");
        let root = d.path().join("proj");
        std::fs::create_dir_all(root.join(".code-arena")).expect("dir");
        std::fs::write(
            root.join(".code-arena/settings.json"),
            r#"{"tools.settings": {"git": {"enabled": false, "approval": "allow"}}}"#,
        )
        .expect("w");
        let places = Places {
            person: d.path().join("config/settings.json"),
            here: d.path().join("data").join(HERE_FILE),
            project: root.join(".code-arena/settings.json"),
            folder: root.to_string_lossy().into_owned(),
        };
        let c = Choice::from_value(&json!({"approval": "ask", "fixed": {"a": 1}}));
        places.save("person", "run_shell", &c).expect("person");
        places.save("here", "write_file", &Choice::from_value(&json!({"enabled": false}))).expect("here");
        let l = places.levels();
        assert_eq!(l.of("run_shell").0, c);
        assert_eq!(l.of("write_file").1.enabled, Some(false));
        assert_eq!(
            l.of("git").2,
            Choice { enabled: Some(false), ..Choice::default() },
            "the project's allow is not read"
        );
        let project = std::fs::read_to_string(root.join(".code-arena/settings.json")).expect("r");
        assert!(!project.contains("write_file"));
        places.save("here", "write_file", &Choice::default()).expect("forget");
        assert_eq!(read_json(&places.here).expect("here")["folders"], json!({}));
        assert!(places.save("team", "x", &c).is_err());
    }
}
