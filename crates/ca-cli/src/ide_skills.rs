//! The skills, for the Skills view: listed by whose they are, made, sent out as a `.zip` and brought in.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn). All of them are answered while a turn runs too, off the
//! driver's loop; every reply says again the `key` its request carried.
//!
//! - `skills{fresh?}` → `skills{groups: [{id, title, skills: [{name, description, origin, path, open}]}], can_arena,
//!   builtin}`: the groups `project` (This project), `yours` (Yours), `arena` (From Arena: your groups' and the
//!   company's, and the plugins') and `builtin` (Built in), each skill once, as the session reads them. `origin` is
//!   the folder it is in as the row says it; `open` is how a row opens its SKILL.md to edit: `{project: PATH}` (a
//!   path inside the project) or `{user: "skills/NAME/SKILL.md"}` (one of the person's files, `user_file`), else
//!   null. The built-in skills are written again first as `skills.builtIn` says; `fresh` reads Arena's again too.
//!   `can_arena`: the Arena signed in to keeps skills (feature `skills`).
//! - `skill_new{name, scope}` (scope `project`, `yours` or `arena`) → `skill_made{name, scope, open}` or
//!   `{name, scope, error}`: a SKILL.md to fill, in `.code-arena/skills` of the project, the settings folder's
//!   `skills`, or Arena.
//! - `skill_export{names, to}` → `skill_exported{to, names, left_out}` or `{error}`: the skills' folders as one
//!   `.zip` (`to` inside the project unless absolute), a file that looks like it holds a secret left out.
//! - `skill_import_preview{path}` → `skill_import_preview{path, skills: [{name, exists, taken, secret}], left_out}`
//!   or `{path, error}`: what a `.zip`, a folder or a SKILL.md holds. `taken`: the scopes that have a skill of that
//!   name already; `secret`: one of its files was left out for looking like it holds a secret.
//! - `skill_import{path, scope, choices: [{name, action: replace|rename|skip, as?}]}` → `skill_imported{scope, done,
//!   kept, left_out}` or `{scope, error}`: brought in; a name there already without a choice is kept as it was.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ca_agent::library::{Library, Skill, Source, good_skill_name, load};
use ca_agent::skill_transfer as st;
use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, Shared, send};

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("skills", Rule::Turn),
        ("skill_new", Rule::Turn),
        ("skill_export", Rule::Turn),
        ("skill_import_preview", Rule::Turn),
        ("skill_import", Rule::Turn),
    ];

    fn shared(sh: &Shared, m: &Value) -> bool {
        let (sh, m) = (sh.clone(), m.clone());
        tokio::spawn(async move {
            let mut reply = answer(&sh, &m).await;
            if let Some(key) = Some(&m["key"]).filter(|k| k.is_string() || k.is_number()) {
                reply["key"] = key.clone();
            }
            send(&sh.out, reply);
        });
        true
    }
}

/// Where a request's skills are, as the area needs them: the project, the settings folder, the data folder.
#[derive(Clone, Debug)]
struct Places {
    root: PathBuf,
    config: PathBuf,
    data: PathBuf,
    home: Option<PathBuf>,
}

impl Places {
    fn of(sh: &Shared) -> Places {
        Places {
            root: sh.workspace.root().to_path_buf(),
            config: sh.paths.config.clone(),
            data: sh.paths.data.clone(),
            home: sh.home.clone(),
        }
    }

    /// The folder a scope's skills are written into.
    fn dir(&self, scope: &str) -> Option<PathBuf> {
        match scope {
            "project" => Some(self.root.join(".code-arena").join("skills")),
            "yours" => Some(self.config.join("skills")),
            _ => None,
        }
    }

    /// A path the person typed: inside the project unless absolute (`~/` is the home folder).
    fn typed(&self, path: &str) -> PathBuf {
        let path = path.trim();
        if let (Some(rest), Some(home)) = (path.strip_prefix("~/"), &self.home) {
            return home.join(rest);
        }
        let p = Path::new(path);
        if p.is_absolute() { p.to_path_buf() } else { self.root.join(p) }
    }

    /// A path as the person reads it: inside the project from its top, in the home folder from `~`.
    fn shown(&self, path: &Path) -> String {
        if let Ok(rel) = path.strip_prefix(&self.root) {
            return rel.to_string_lossy().replace('\\', "/");
        }
        if let Some(rest) = self.home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
            return format!("~/{}", rest.to_string_lossy().replace('\\', "/"));
        }
        path.to_string_lossy().into_owned()
    }
}

/// The reply to one request.
async fn answer(sh: &Shared, m: &Value) -> Value {
    let places = Places::of(sh);
    let text = |k: &str| m[k].as_str().unwrap_or("").trim().to_string();
    match crate::ide_ext::kind(m) {
        "skills" => {
            if m["fresh"] == true && can_arena(sh) {
                refresh_arena(sh).await;
            }
            let builtin = sh.settings.bool(&ca_settings::schema::BUILTIN_SKILLS);
            let (sources, can) = (sh.sources.clone(), can_arena(sh));
            blocking(move || {
                let _ = ca_agent::library::keep_builtin(&places.data, &crate::session::arena_off(&places.data), builtin);
                let library = load(&sources);
                json!({"type": "skills", "groups": groups(&library, &sources, &places), "can_arena": can, "builtin": builtin})
            })
            .await
        }
        "skill_new" => {
            let (name, scope) = (text("name"), text("scope"));
            let made = |name: &str, scope: &str, r: Result<Value, String>| match r {
                Ok(open) => json!({"type": "skill_made", "name": name, "scope": scope, "open": open}),
                Err(e) => json!({"type": "skill_made", "name": name, "scope": scope, "error": e}),
            };
            let fail = |e: String| made(&name, &scope, Err(e));
            if !good_skill_name(&name) {
                return fail(NAME_RULE.into());
            }
            if scope == "arena" {
                let Some(arena) = sh.arena.clone().filter(|_| can_arena(sh)) else {
                    return fail("The Arena signed in to does not keep skills.".into());
                };
                if let Err(e) = arena.make_skill(&name, DESCRIBE, &instructions(&name)).await {
                    return fail(e.to_string());
                }
                refresh_arena(sh).await;
                return made(&name, &scope, Ok(Value::Null));
            }
            let (n, sc) = (name.clone(), scope.clone());
            let r = blocking(move || make(&places, &n, &sc)).await;
            made(&name, &scope, r)
        }
        "skill_export" => {
            let names: Vec<String> =
                m["names"].as_array().into_iter().flatten().filter_map(|n| n.as_str().map(str::to_string)).collect();
            let (to, sources) = (text("to"), sh.sources.clone());
            blocking(move || export(&places, &load(&sources), &names, &to)).await
        }
        "skill_import_preview" => {
            let path = text("path");
            let mine = arena_mine(&places.data);
            blocking(move || match st::read(&places.typed(&path)) {
                Ok(found) => preview(&places, &found, &mine, &path),
                Err(e) => json!({"type": "skill_import_preview", "path": path, "error": e}),
            })
            .await
        }
        "skill_import" => {
            let (path, scope) = (text("path"), text("scope"));
            let choices = choices_of(&m["choices"]);
            let fail = |e: String| json!({"type": "skill_imported", "scope": scope, "error": e});
            let found = {
                let at = places.typed(&path);
                match blocking(move || st::read(&at)).await {
                    Ok(f) => f,
                    Err(e) => return fail(e),
                }
            };
            if scope == "arena" {
                let Some(arena) = sh.arena.clone().filter(|_| can_arena(sh)) else {
                    return fail("The Arena signed in to does not keep skills.".into());
                };
                return import_arena(sh, &arena, &found, &choices).await;
            }
            blocking(move || import_local(&places, &scope, &found, &choices)).await
        }
        _ => json!({"type": "notice", "text": "Not a request of the Skills view."}),
    }
}

/// Runs file work off the driver's loop.
async fn blocking<T: std::marker::Send + 'static>(work: impl FnOnce() -> T + std::marker::Send + 'static) -> T {
    match tokio::task::spawn_blocking(work).await {
        Ok(v) => v,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
}

/// What a skill's name may be, said when it is not.
const NAME_RULE: &str = "A skill's name is lower-case letters, digits, - and _ (from a letter or a digit), at most 64.";
/// What a new skill says it does until it is written.
const DESCRIBE: &str = "What this skill does, and when to use it.";

/// A new skill's instructions, to be written.
fn instructions(name: &str) -> String {
    format!("# {name}\n\nWhat to do, step by step, when this skill is used.\n")
}

/// A new skill's SKILL.md.
fn template(name: &str) -> String {
    format!("---\nname: {name}\ndescription: {DESCRIBE}\n---\n\n{}", instructions(name))
}

/// Whether the session's Arena keeps skills: the managed edition, signed in, and its feature `skills` there.
fn can_arena(sh: &Shared) -> bool {
    sh.edition == ca_settings::Edition::Managed
        && sh.arena.as_ref().is_some_and(|a| {
            ca_arena::Kept::read(&ca_arena::Kept::file(&sh.paths.servers_dir(), a.base()))
                .is_some_and(|k| k.capabilities.has(ca_arena::Feature::Skills))
        })
}

/// Arena's skills read again and kept for the session.
async fn refresh_arena(sh: &Shared) {
    if let Some(arena) = sh.arena.clone() {
        let builtin = sh.settings.bool(&ca_settings::schema::BUILTIN_SKILLS);
        crate::session::keep_arena_skills(arena, sh.paths.data.clone(), builtin, sh.notices.clone()).await;
    }
}

/// The names of Arena's skills that are the person's own, as Arena last said.
fn arena_mine(data: &Path) -> Vec<String> {
    std::fs::read_to_string(ca_agent::library::arena_dir(data).join("mine.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// The group a source's skills are in, and the folder its rows say.
fn group_of(source: &Source, places: &Places) -> (&'static str, String) {
    let label = source.label.as_str();
    let inside = || label.split_once('(').and_then(|(_, r)| r.strip_suffix(')')).map(|f| format!("{f}/skills"));
    if label.starts_with("this project") {
        ("project", inside().unwrap_or_else(|| "this project".into()))
    } else if label.starts_with("yours") {
        ("yours", inside().unwrap_or_else(|| places.shown(&source.dir.join("skills"))))
    } else if label == "built in" {
        ("builtin", "Built in".into())
    } else if let Some(plugin) = label.strip_prefix("the plugin ") {
        ("arena", format!("the plugin {plugin}"))
    } else {
        ("arena", "Arena".into())
    }
}

/// The view's groups, from what the session reads.
fn groups(library: &Library, sources: &[Source], places: &Places) -> Vec<Value> {
    let mine = arena_mine(&places.data);
    let mut by: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for k in &library.skills {
        let Some(source) = sources.iter().find(|s| s.label == k.source) else { continue };
        let (mut group, mut origin) = group_of(source, places);
        if source.label == "Arena's" && mine.contains(&k.name) {
            (group, origin) = ("yours", "Arena".into());
        }
        by.entry(group).or_default().push(json!({
            "name": k.name,
            "description": k.description,
            "origin": origin,
            "path": places.shown(&k.path),
            "open": open_of(k, source, places),
        }));
    }
    [("project", "This project"), ("yours", "Yours"), ("arena", "From Arena"), ("builtin", "Built in")]
        .into_iter()
        .map(|(id, title)| json!({"id": id, "title": title, "skills": by.remove(id).unwrap_or_default()}))
        .collect()
}

/// How a row opens its SKILL.md to edit: inside the project, or as one of the person's files; none for another's.
fn open_of(k: &Skill, source: &Source, places: &Places) -> Value {
    if let Ok(rel) = k.path.strip_prefix(&places.root) {
        return json!({"project": rel.to_string_lossy().replace('\\', "/")});
    }
    let dir = k.path.parent().and_then(|d| d.file_name()).map(|d| d.to_string_lossy().into_owned());
    match dir {
        Some(dir) if source.dir == places.config && good_skill_name(&dir) => {
            json!({"user": format!("skills/{dir}/SKILL.md")})
        }
        _ => Value::Null,
    }
}

/// A new skill's folder and SKILL.md written in a scope's folder; gives how it is opened.
fn make(places: &Places, name: &str, scope: &str) -> Result<Value, String> {
    let dir = places.dir(scope).ok_or_else(|| format!("A skill is made for project, yours or arena, not {scope}."))?;
    let folder = dir.join(name);
    if folder.exists() {
        return Err(format!("A skill named {name} is in {} already.", places.shown(&dir)));
    }
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", places.shown(&folder)))?;
    let file = folder.join(st::SKILL_MD);
    std::fs::write(&file, template(name)).map_err(|e| format!("{}: {e}", places.shown(&file)))?;
    Ok(if scope == "project" {
        json!({"project": places.shown(&file)})
    } else {
        json!({"user": format!("skills/{name}/SKILL.md")})
    })
}

/// The skills named, sent out as one `.zip`.
fn export(places: &Places, library: &Library, names: &[String], to: &str) -> Value {
    let fail = |e: String| json!({"type": "skill_exported", "names": names, "error": e});
    if names.is_empty() {
        return fail("There is no skill to export.".into());
    }
    let mut folders = Vec::new();
    for name in names {
        let Some(k) = library.skills.iter().find(|k| &k.name == name) else {
            return fail(format!("No skill named {name}."));
        };
        let Some(dir) = k.path.parent() else { return fail(format!("The skill {name} has no folder.")) };
        folders.push((dir.to_path_buf(), name.clone()));
    }
    let to =
        if to.is_empty() { format!("{}.zip", if names.len() == 1 { &names[0] } else { "skills" }) } else { to.into() };
    let mut at = places.typed(&to);
    if at.extension().is_none_or(|e| !e.eq_ignore_ascii_case("zip")) {
        at.set_extension("zip");
    }
    if let Some(d) = at.parent()
        && let Err(e) = std::fs::create_dir_all(d)
    {
        return fail(format!("{}: {e}", places.shown(d)));
    }
    match st::export_many(&folders, &at) {
        Ok(left) => json!({"type": "skill_exported", "to": places.shown(&at), "names": names, "left_out": left}),
        Err(e) => fail(e),
    }
}

/// The scopes that have a skill of this name already.
fn taken(places: &Places, mine: &[String], name: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    for scope in ["project", "yours"] {
        if places.dir(scope).is_some_and(|d| d.join(name).exists()) {
            out.push(scope);
        }
    }
    if mine.iter().any(|m| m == name) {
        out.push("arena");
    }
    out
}

/// What an import would bring in.
fn preview(places: &Places, found: &st::Found, mine: &[String], path: &str) -> Value {
    let skills: Vec<Value> = found
        .skills
        .iter()
        .map(|s| {
            let taken = taken(places, mine, &s.name);
            let secret = found
                .left_out
                .iter()
                .any(|l| l.contains("secrets never come in") && l.contains(&format!("{}/", s.name)));
            json!({"name": s.name, "exists": !taken.is_empty(), "taken": taken, "secret": secret})
        })
        .collect();
    json!({"type": "skill_import_preview", "path": path, "skills": skills, "left_out": found.left_out})
}

/// What the person chose for a name there already.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Choice {
    Replace,
    Rename(String),
    Skip,
}

/// The choices of a `skill_import`, by name.
fn choices_of(v: &Value) -> BTreeMap<String, Choice> {
    let mut out = BTreeMap::new();
    for c in v.as_array().into_iter().flatten() {
        let name = c["name"].as_str().unwrap_or("").to_string();
        let choice = match c["action"].as_str().unwrap_or("") {
            "replace" => Choice::Replace,
            "rename" => Choice::Rename(c["as"].as_str().unwrap_or("").trim().to_string()),
            _ => Choice::Skip,
        };
        out.insert(name, choice);
    }
    out
}

/// The skills to write, after the choices: those that replace what is there, and the rest.
fn chosen(
    found: &st::Found,
    choices: &BTreeMap<String, Choice>,
) -> Result<(Vec<st::Folder>, Vec<st::Folder>, Vec<String>), String> {
    let (mut replace, mut add, mut skipped) = (Vec::new(), Vec::new(), Vec::new());
    for s in &found.skills {
        match choices.get(&s.name) {
            Some(Choice::Replace) => replace.push(s.clone()),
            Some(Choice::Rename(to)) if !good_skill_name(to) => return Err(format!("{to}: {NAME_RULE}")),
            Some(Choice::Rename(to)) => add.push(st::renamed(s, to)),
            Some(Choice::Skip) => skipped.push(s.name.clone()),
            None => add.push(s.clone()),
        }
    }
    Ok((replace, add, skipped))
}

/// Skills brought into the project's folder or the person's.
fn import_local(places: &Places, scope: &str, found: &st::Found, choices: &BTreeMap<String, Choice>) -> Value {
    let fail = |e: String| json!({"type": "skill_imported", "scope": scope, "error": e});
    let Some(dir) = places.dir(scope) else {
        return fail(format!("Skills come in for project, yours or arena, not {scope}."));
    };
    let (replace, add, mut kept) = match chosen(found, choices) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let mut done = Vec::new();
    for (list, over) in [(replace, true), (add, false)] {
        match st::write_into(&dir, &list, over) {
            Ok((d, k)) => {
                done.extend(d);
                kept.extend(k);
            }
            Err(e) => return fail(format!("{}: {e}", places.shown(&dir))),
        }
    }
    json!({"type": "skill_imported", "scope": scope, "done": done, "kept": kept, "left_out": found.left_out})
}

/// Skills brought into Arena as the person's own, with their choices, then Arena's read again.
async fn import_arena(
    sh: &Shared,
    arena: &ca_arena::ArenaClient,
    found: &st::Found,
    choices: &BTreeMap<String, Choice>,
) -> Value {
    let folders: Vec<ca_arena::skills::Folder> = found
        .skills
        .iter()
        .map(|s| ca_arena::skills::Folder { folder: s.name.clone(), files: s.files.clone() })
        .collect();
    let said: Vec<Value> = choices
        .iter()
        .map(|(name, c)| match c {
            Choice::Replace => json!({"folder": name, "action": "replace"}),
            Choice::Rename(to) => json!({"folder": name, "action": "rename", "name": to}),
            Choice::Skip => json!({"folder": name, "action": "skip"}),
        })
        .collect();
    match arena.import_skills(&folders, &said, false).await {
        Ok(v) => {
            let names = |k: &str| -> Vec<String> {
                v[k].as_array().into_iter().flatten().filter_map(|x| x["name"].as_str().map(str::to_string)).collect()
            };
            refresh_arena(sh).await;
            json!({"type": "skill_imported", "scope": "arena", "done": names("imported"), "kept": names("skipped"), "left_out": found.left_out})
        }
        Err(e) => json!({"type": "skill_imported", "scope": "arena", "error": e.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(d: &Path) -> Places {
        Places { root: d.join("repo"), config: d.join("config"), data: d.join("data"), home: Some(d.join("home")) }
    }

    fn skill(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir.join(name)).expect("mkdir");
        std::fs::write(
            dir.join(name).join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Does {name}\n---\nDo it."),
        )
        .expect("write");
    }

    fn sources(p: &Places) -> Vec<Source> {
        ca_agent::library::sources(&p.root, &p.config, p.home.as_deref(), Some(&p.data), &Default::default())
    }

    #[test]
    fn the_skills_are_grouped_by_whose_they_are_with_their_folder_and_how_they_open() {
        let d = tempfile::tempdir().expect("temp");
        let p = places(d.path());
        skill(&p.root.join(".code-arena/skills"), "release");
        skill(&p.root.join(".arena/skills"), "review");
        skill(&p.config.join("skills"), "tests");
        skill(&p.home.clone().expect("home").join(".code-arena/skills"), "notes");
        skill(&ca_agent::library::arena_dir(&p.data).join("skills"), "company");
        skill(&ca_agent::library::arena_dir(&p.data).join("skills"), "mine");
        std::fs::write(ca_agent::library::arena_dir(&p.data).join("mine.json"), "[\"mine\"]").expect("mine");
        skill(&ca_agent::library::builtin_dir(&p.data).join("skills"), "explain");
        let sources = sources(&p);
        let got = groups(&load(&sources), &sources, &p);
        let ids: Vec<&str> = got.iter().filter_map(|g| g["id"].as_str()).collect();
        assert_eq!(ids, ["project", "yours", "arena", "builtin"]);
        let names = |i: usize| -> Vec<&str> {
            got[i]["skills"].as_array().expect("skills").iter().filter_map(|k| k["name"].as_str()).collect()
        };
        assert_eq!(names(0), ["release", "review"]);
        assert_eq!(names(1), ["tests", "notes", "mine"]);
        assert_eq!(names(2), ["company"]);
        assert_eq!(names(3), ["explain"]);
        let row = |i: usize, j: usize| got[i]["skills"][j].clone();
        assert_eq!(row(0, 0)["origin"], ".code-arena/skills");
        assert_eq!(row(0, 1)["origin"], ".arena/skills");
        assert_eq!(row(0, 0)["open"], json!({"project": ".code-arena/skills/release/SKILL.md"}));
        assert_eq!(row(1, 0)["open"], json!({"user": "skills/tests/SKILL.md"}));
        assert_eq!(row(1, 1)["origin"], "~/.code-arena/skills");
        assert_eq!(row(1, 1)["open"], Value::Null);
        assert_eq!(row(1, 2)["origin"], "Arena");
        assert_eq!(row(3, 0)["origin"], "Built in");
        assert_eq!(row(0, 0)["description"], "Does release");
    }

    #[test]
    fn a_new_skill_is_written_to_fill_and_never_over_one_there() {
        let d = tempfile::tempdir().expect("temp");
        let p = places(d.path());
        assert_eq!(make(&p, "deploy", "project"), Ok(json!({"project": ".code-arena/skills/deploy/SKILL.md"})));
        let text = std::fs::read_to_string(p.root.join(".code-arena/skills/deploy/SKILL.md")).expect("made");
        assert!(text.starts_with("---\nname: deploy\n"), "{text}");
        assert!(make(&p, "deploy", "project").is_err_and(|e| e.contains("already")));
        assert_eq!(make(&p, "deploy", "yours"), Ok(json!({"user": "skills/deploy/SKILL.md"})));
        assert!(p.config.join("skills/deploy/SKILL.md").is_file());
        assert!(make(&p, "deploy", "elsewhere").is_err());
    }

    #[test]
    fn skills_go_out_as_a_zip_and_come_in_after_a_preview_with_the_choices() {
        let d = tempfile::tempdir().expect("temp");
        let p = places(d.path());
        skill(&p.root.join(".code-arena/skills"), "release");
        skill(&p.config.join("skills"), "tests");
        let sources = sources(&p);
        let out = export(&p, &load(&sources), &["release".into(), "tests".into()], "out/both");
        assert_eq!(out["to"], "out/both.zip", "{out}");
        assert!(export(&p, &load(&sources), &["nothing".into()], "x.zip")["error"].is_string());
        let found = st::read(&p.root.join("out/both.zip")).expect("read");
        let seen = preview(&p, &found, &[], "out/both.zip");
        assert_eq!(
            seen["skills"][0],
            json!({"name": "release", "exists": true, "taken": ["project"], "secret": false})
        );
        assert_eq!(seen["skills"][1]["taken"], json!(["yours"]));
        // Into the project: release renamed, tests new there.
        let choices = choices_of(&json!([{"name": "release", "action": "rename", "as": "release-2"}]));
        let got = import_local(&p, "project", &found, &choices);
        assert_eq!(got["done"], json!(["release-2", "tests"]), "{got}");
        let (fm, _) = ca_agent::library::front_matter(
            &std::fs::read_to_string(p.root.join(".code-arena/skills/release-2/SKILL.md")).expect("r"),
        );
        assert_eq!(fm["name"], "release-2");
        // Again: one skipped, one replaced, none other written.
        let choices =
            choices_of(&json!([{"name": "release", "action": "skip"}, {"name": "tests", "action": "replace"}]));
        let got = import_local(&p, "project", &found, &choices);
        assert_eq!(got["done"], json!(["tests"]));
        assert_eq!(got["kept"], json!(["release"]));
        // A name there already with no choice is kept; a bad new name is refused.
        assert_eq!(import_local(&p, "project", &found, &BTreeMap::new())["kept"], json!(["release", "tests"]));
        let bad = choices_of(&json!([{"name": "release", "action": "rename", "as": "Bad Name"}]));
        assert!(import_local(&p, "project", &found, &bad)["error"].is_string());
    }

    #[test]
    fn a_typed_path_is_inside_the_project_unless_it_is_absolute_or_from_home() {
        let d = tempfile::tempdir().expect("temp");
        let p = places(d.path());
        assert_eq!(p.typed("a.zip"), p.root.join("a.zip"));
        assert_eq!(p.typed("~/x.zip"), d.path().join("home/x.zip"));
        assert_eq!(p.typed(&d.path().join("y.zip").to_string_lossy()), d.path().join("y.zip"));
        assert_eq!(p.shown(&d.path().join("home/s")), "~/s");
    }
}
