//! The Skills view: every skill the agent reads, under This project, Yours, From Arena and Built in, each group
//! folding and saying the folders its skills are in. A click on a skill opens its SKILL.md to edit when it is the
//! project's or yours on this computer, else puts `/name` in the chat's box; while the pointer is on a row, Export
//! (.zip) and Use in the Chat. The title has New Skill (for this project, in Arena or on this computer, then its
//! name: Enter makes it and opens it), Refresh and Collapse All; its "…" menu, Import Skills… (a path, what it
//! holds, where it comes in, and Replace, Rename or Skip for a name there already), Export the Project's and
//! Yours…, Export All… and Built-in Skills (the setting `skills.builtIn`).
//!
//! The skills' side is the driver's `ide_skills.rs`. What every part has (`register`, `take`) is said in `ext.rs`;
//! what a view has (`READY`, `view`, `actions`), in `layout.rs`.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::views::{Decorators, container, h_stack, label, tooltip};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::widgets::tree::{self, Node};
use crate::widgets::{quick, toast};

/// Whether the view is in the window.
pub const READY: bool = true;

/// The commands of the view's title and its "…" menu.
pub const NEW: &str = "codeArena.skills.new";
pub const REFRESH: &str = "codeArena.skills.refresh";
pub const COLLAPSE: &str = "codeArena.skills.collapseAll";
pub const IMPORT: &str = "codeArena.skills.import";
pub const EXPORT_OWN: &str = "codeArena.skills.exportOwn";
pub const EXPORT_ALL: &str = "codeArena.skills.exportAll";
pub const BUILT_IN: &str = "codeArena.skills.toggleBuiltIn";
pub const MORE: &str = "codeArena.skills.more";

/// The setting that turns the built-in skills on and off.
const BUILT_IN_KEY: &str = "skills.builtIn";
/// What a name may be, said when it is not.
const NAME_RULE: &str = "Lower-case letters, digits, - and _, from a letter or a digit (at most 64).";
/// Where a path the person types is.
const PATH_RULE: &str = "A path inside the project, or from / or ~/";

/// A skill as a row shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The folder it is in, as the row says it.
    pub origin: String,
    pub path: String,
    /// How it opens to edit: a path inside the project, or one of the person's files; neither: it is not theirs.
    pub open: Open,
}

/// How a skill's SKILL.md is opened to edit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Open {
    Project(String),
    User(String),
    #[default]
    None,
}

/// A group of skills: This project, Yours, From Arena, Built in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub id: String,
    pub title: String,
    pub skills: Vec<Skill>,
}

/// The `skills` message as the view keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct List {
    pub groups: Vec<Group>,
    pub can_arena: bool,
    pub builtin: bool,
}

/// A row of the view, by its id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Group(String),
    Skill(String, String),
    Note,
}

/// A row's id: `g:GROUP`, `s:GROUP:NAME`, `note`.
pub fn row_of(id: &str) -> Option<Row> {
    if id == "note" {
        return Some(Row::Note);
    }
    if let Some(g) = id.strip_prefix("g:") {
        return Some(Row::Group(g.to_string()));
    }
    let (g, name) = id.strip_prefix("s:")?.split_once(':')?;
    Some(Row::Skill(g.to_string(), name.to_string()))
}

/// The list in a `skills` message.
pub fn list_of(m: &Value) -> List {
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    let groups = m["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| Group {
            id: s(g, "id"),
            title: s(g, "title"),
            skills: g["skills"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|k| Skill {
                    name: s(k, "name"),
                    description: s(k, "description"),
                    origin: s(k, "origin"),
                    path: s(k, "path"),
                    open: match (k["open"]["project"].as_str(), k["open"]["user"].as_str()) {
                        (Some(p), _) => Open::Project(p.into()),
                        (_, Some(u)) => Open::User(u.into()),
                        _ => Open::None,
                    },
                })
                .collect(),
        })
        .collect();
    List { groups, can_arena: m["can_arena"] == true, builtin: m["builtin"] != false }
}

/// The rows shown: each group that has skills, its skills under it unless it is folded; a note when there is none.
pub fn rows(list: &List, folded: &BTreeSet<String>) -> Vec<Node> {
    let mut out = Vec::new();
    for g in list.groups.iter().filter(|g| !g.skills.is_empty()) {
        let open = !folded.contains(&g.id);
        let mut head = tree::folder(&format!("g:{}", g.id), 0, open);
        head.stamp = g.skills.len() as u64;
        out.push(head);
        if open {
            for (i, k) in g.skills.iter().enumerate() {
                let mut row = tree::leaf(&format!("s:{}:{}", g.id, k.name), 1);
                // Drawn again when what it says changed.
                row.stamp = stamp(&(i, &k.description, &k.origin, &k.path));
                out.push(row);
            }
        }
    }
    if out.is_empty() {
        out.push(tree::leaf("note", 0));
    }
    out
}

fn stamp(v: &impl std::hash::Hash) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one(v)
}

/// The folders a group's skills are in, as its row says them (not when it is the group's own title: Built in).
pub fn origins(g: &Group) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for k in &g.skills {
        if !seen.contains(&k.origin.as_str()) && !k.origin.eq_ignore_ascii_case(&g.title) {
            seen.push(&k.origin);
        }
    }
    seen.join(", ")
}

/// Whether a skill's name is one a folder may have (the driver's `good_skill_name`).
pub fn good_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The names of the skills in the groups given (all of them when none is).
pub fn names_in(list: &List, groups: &[&str]) -> Vec<String> {
    list.groups
        .iter()
        .filter(|g| groups.is_empty() || groups.contains(&g.id.as_str()))
        .flat_map(|g| g.skills.iter().map(|k| k.name.clone()))
        .collect()
}

/// A name not taken by a skill coming in: `NAME-2`, `NAME-3`…
pub fn free_name(name: &str, taken: &[String]) -> String {
    (2..).map(|n| format!("{name}-{n}")).find(|n| !taken.contains(n)).unwrap_or_default()
}

/// What an import's preview said, kept for the steps after it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preview {
    pub path: String,
    /// Each skill's name and the scopes that have one of that name already.
    pub skills: Vec<(String, Vec<String>)>,
    pub left_out: Vec<String>,
    /// Skills that had a file left out for looking like it holds a secret.
    pub secret: Vec<String>,
}

/// The preview in a `skill_import_preview` message.
pub fn preview_of(m: &Value) -> Preview {
    let strings = |v: &Value| -> Vec<String> {
        v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect()
    };
    let list = m["skills"].as_array().cloned().unwrap_or_default();
    Preview {
        path: m["path"].as_str().unwrap_or("").into(),
        skills: list.iter().map(|k| (k["name"].as_str().unwrap_or("").to_string(), strings(&k["taken"]))).collect(),
        left_out: strings(&m["left_out"]),
        secret: list
            .iter()
            .filter(|k| k["secret"] == true)
            .filter_map(|k| k["name"].as_str().map(str::to_string))
            .collect(),
    }
}

/// The view's state in a window.
#[derive(Clone, Copy)]
struct Skills {
    list: RwSignal<Option<List>>,
    folded: RwSignal<BTreeSet<String>>,
    tree: tree::State,
}

impl crate::ext::State for Skills {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        Skills {
            list: s.create_rw_signal(None),
            folded: s.create_rw_signal(BTreeSet::new()),
            tree: tree::State::new(ui),
        }
    }
}

fn state(ui: Ui) -> Skills {
    ui.state::<Skills>()
}

/// A skill by its group and name.
fn skill_of(ui: Ui, group: &str, name: &str) -> Option<Skill> {
    state(ui).list.with_untracked(|l| {
        l.as_ref()?.groups.iter().find(|g| g.id == group)?.skills.iter().find(|k| k.name == name).cloned()
    })
}

/// Asks for the list (with Arena's read again when `fresh`).
fn ask(send: &Send, fresh: bool) {
    send(json!({"type": "skills", "fresh": fresh}));
}

/// A skill opened: its SKILL.md to edit, or `/name` in the chat's box when it is not the person's to edit.
fn open(ui: Ui, send: &Send, k: &Skill) {
    match &k.open {
        Open::Project(p) => ui.open(p, send),
        Open::User(u) => crate::settings::open_user(ui, send, u),
        Open::None => use_in_chat(ui, k),
    }
}

/// `/name ` put at the start of the chat's box.
fn use_in_chat(ui: Ui, k: &Skill) {
    crate::chat_bridge::insert_at_start(ui, &format!("/{} ", k.name));
}

/// Asks where a `.zip` of the skills named goes, then sends them out.
fn export(ui: Ui, send: &Send, names: Vec<String>, default: &str, title: &str) {
    if names.is_empty() {
        toast::info(ui, "There is no skill to export.");
        return;
    }
    let send = send.clone();
    quick::input(title)
        .value(default)
        .prompt(&format!("Where to save the .zip: {}", PATH_RULE.to_lowercase()))
        .validate(|v| v.trim().is_empty().then(|| "A path is needed.".into()))
        .open(ui, move |to| {
            if let Some(to) = to.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
                send(json!({"type": "skill_export", "names": names, "to": to}));
            }
        });
}

/// New Skill: where it is made, then its name.
fn new_skill(ui: Ui, send: &Send) {
    let can_arena = state(ui).list.with_untracked(|l| l.as_ref().is_some_and(|l| l.can_arena));
    let mut items = vec![quick::item("project", "For This Project").icon("repo").description(".code-arena/skills")];
    if can_arena {
        items.push(
            quick::item("arena", "In Arena").icon("cloud").description("yours, in every project and Arena's chat"),
        );
    }
    items.push(quick::item("yours", "On This Computer").icon("account").description("yours, in every project here"));
    let send = send.clone();
    quick::pick("New Skill").placeholder("Where the skill is made").items(items).open(ui, move |chosen| {
        let Some(scope) = chosen.map(|c| c.id) else { return };
        quick::input("New Skill")
            .placeholder("Name")
            .prompt("Its name, as /name in the chat: lower-case, with dashes. Enter makes it and opens it")
            .validate(|v| (!good_name(v.trim())).then(|| NAME_RULE.into()))
            .open(ui, move |name| {
                if let Some(name) = name.map(|n| n.trim().to_string()).filter(|n| good_name(n)) {
                    send(json!({"type": "skill_new", "name": name, "scope": scope}));
                }
            });
    });
}

/// Import Skills…: the path of what comes in; the rest follows the preview.
fn import(ui: Ui, send: &Send) {
    let send = send.clone();
    quick::input("Import Skills")
        .placeholder("skills.zip")
        .prompt(&format!("A .zip, a folder or a SKILL.md: {}", PATH_RULE.to_lowercase()))
        .validate(|v| v.trim().is_empty().then(|| "A path is needed.".into()))
        .open(ui, move |path| {
            if let Some(path) = path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
                send(json!({"type": "skill_import_preview", "path": path}));
            }
        });
}

/// The preview come: where the skills come in, then a choice for each name there already.
fn after_preview(ui: Ui, send: &Send, p: Preview) {
    if p.skills.is_empty() {
        let why = if p.left_out.is_empty() { String::new() } else { format!(" Left out: {}.", p.left_out.join("; ")) };
        toast::warning(ui, &format!("No skill was found in {}.{why}", p.path));
        return;
    }
    if !p.secret.is_empty() {
        toast::warning(
            ui,
            &format!("A file of {} looks like it holds a secret: it does not come in.", p.secret.join(", ")),
        );
    }
    let can_arena = state(ui).list.with_untracked(|l| l.as_ref().is_some_and(|l| l.can_arena));
    let names: Vec<&str> = p.skills.iter().map(|(n, _)| n.as_str()).collect();
    let all = names.join(", ");
    let taken = |scope: &str| p.skills.iter().filter(|(_, t)| t.iter().any(|s| s == scope)).count();
    let there = |scope: &str| match taken(scope) {
        0 => String::new(),
        1 => "1 there already".to_string(),
        n => format!("{n} there already"),
    };
    let mut items = vec![
        quick::item("project", "This Project").icon("repo").description(".code-arena/skills").detail(&there("project")),
        quick::item("yours", "Yours, on This Computer").icon("account").detail(&there("yours")),
    ];
    if can_arena {
        items.push(quick::item("arena", "Yours, in Arena").icon("cloud").detail(&there("arena")));
    }
    let title = if names.len() == 1 { format!("Import {all}") } else { format!("Import {} Skills", names.len()) };
    let send = send.clone();
    quick::pick(&title).placeholder(&format!("Where {all} come in")).items(items).open(ui, move |chosen| {
        let Some(scope) = chosen.map(|c| c.id) else { return };
        let clashes: Vec<String> =
            p.skills.iter().filter(|(_, t)| t.contains(&scope)).map(|(n, _)| n.clone()).collect();
        let names: Vec<String> = p.skills.iter().map(|(n, _)| n.clone()).collect();
        let job = Rc::new(Import { path: p.path.clone(), scope, names, choices: RefCell::new(Vec::new()) });
        choose(ui, send, job, clashes, 0);
    });
}

/// An import being chosen: what comes in where, and what was chosen for the names there already.
struct Import {
    path: String,
    scope: String,
    names: Vec<String>,
    choices: RefCell<Vec<Value>>,
}

/// Replace, Rename or Skip for each name there already, one after the other; then the import is sent.
fn choose(ui: Ui, send: Send, job: Rc<Import>, clashes: Vec<String>, at: usize) {
    let Some(name) = clashes.get(at).cloned() else {
        send(json!({
            "type": "skill_import",
            "path": job.path,
            "scope": job.scope,
            "choices": job.choices.borrow().clone(),
        }));
        return;
    };
    let items = vec![
        quick::item("replace", "Replace").icon("replace").description("the one there goes"),
        quick::item("rename", "Rename…").icon("edit").description("brought in under another name"),
        quick::item("skip", "Skip").icon("close").description("the one there stays"),
    ];
    quick::pick(&format!("A skill named {name} is there already")).items(items).open(ui, move |chosen| {
        let Some(action) = chosen.map(|c| c.id) else { return };
        if action != "rename" {
            job.choices.borrow_mut().push(json!({"name": name, "action": action}));
            choose(ui, send, job, clashes, at + 1);
            return;
        }
        let mut taken = job.names.clone();
        taken.extend(state(ui).list.with_untracked(|l| l.as_ref().map(|l| names_in(l, &[])).unwrap_or_default()));
        quick::input(&format!("Rename {name}"))
            .value(&free_name(&name, &taken))
            .prompt("The name it comes in under")
            .validate(move |v| {
                let v = v.trim();
                if !good_name(v) {
                    Some(NAME_RULE.into())
                } else if taken.iter().any(|t| t == v) {
                    Some(format!("A skill named {v} is there already."))
                } else {
                    None
                }
            })
            .open(ui, move |to| {
                let Some(to) = to.map(|t| t.trim().to_string()).filter(|t| good_name(t)) else { return };
                job.choices.borrow_mut().push(json!({"name": name, "action": "rename", "as": to}));
                choose(ui, send, job, clashes, at + 1);
            });
    });
}

/// The view's own content.
pub fn view(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    if st.list.with_untracked(Option::is_none) {
        ask(&send, true);
    }
    let shown = create_memo(move |_| {
        let nodes = st.list.with(|l| st.folded.with(|f| l.as_ref().map(|l| rows(l, f))));
        nodes.unwrap_or_default().into_iter().collect::<im::Vector<_>>()
    });
    let (acting, opening) = (send.clone(), send);
    tree::tree(ui, st.tree, move || shown.get())
        .content(move |node| row_view(ui, node))
        .actions(move |node| {
            let Some(Row::Skill(g, name)) = row_of(&node.id) else { return vec![] };
            let Some(k) = skill_of(ui, &g, &name) else { return vec![] };
            let (send, use_k) = (acting.clone(), k.clone());
            vec![
                tree::act("export", "Export (.zip)", move || {
                    export(ui, &send, vec![k.name.clone()], &format!("{}.zip", k.name), "Export Skill")
                }),
                tree::act("comment-discussion", "Use in the Chat", move || use_in_chat(ui, &use_k)),
            ]
        })
        .on_toggle(move |id| {
            if let Some(Row::Group(g)) = row_of(id) {
                st.folded.update(|f| {
                    if !f.remove(&g) {
                        f.insert(g);
                    }
                });
            }
        })
        .on_open(move |id| {
            if let Some(Row::Skill(g, name)) = row_of(id)
                && let Some(k) = skill_of(ui, &g, &name)
            {
                open(ui, &opening, &k);
            }
        })
        .view()
        .style(move |s| s.size_full().color(t.get().side_fg))
        .into_any()
}

/// What a row shows: a group's title, how many it has and their folders; a skill's name and what it does.
fn row_view(ui: Ui, node: &Node) -> floem::AnyView {
    let t = ui.colors;
    let desc = move |text: String| {
        label(move || text.clone())
            .style(move |s| s.font_size(12.0).color(t.get().desc).text_ellipsis().min_width(0.0).flex_shrink(1.0_f32))
    };
    let group = |id: &str| state(ui).list.with_untracked(|l| l.as_ref()?.groups.iter().find(|g| g.id == id).cloned());
    match row_of(&node.id) {
        Some(Row::Group(id)) => {
            let Some(g) = group(&id) else { return label(|| "").into_any() };
            let (title, n, from) = (g.title.clone(), g.skills.len(), origins(&g));
            h_stack((
                label(move || title.clone()).style(|s| s.font_size(11.0).font_bold().flex_shrink(0.0_f32)),
                count_badge(ui, n),
                desc(from),
            ))
            .style(|s| s.items_center().gap(6.0).min_width(0.0))
            .into_any()
        }
        Some(Row::Skill(g, name)) => {
            let Some(k) = skill_of(ui, &g, &name) else { return label(|| "").into_any() };
            let said = if k.description.is_empty() { k.origin.clone() } else { k.description.clone() };
            let tip = match &k.open {
                Open::None => {
                    format!("{} ({})\n{}\nA click puts /{} in the chat's box.", k.name, k.origin, k.path, k.name)
                }
                _ => format!("{} ({})\n{}", k.name, k.origin, k.path),
            };
            let (title, description) = (k.name.clone(), k.description.clone());
            let line = h_stack((
                icon("wand", 16.0).style(|s| s.flex_shrink(0.0_f32)),
                label(move || title.clone()).style(|s| s.font_size(13.0).flex_shrink(0.0_f32)),
                desc(said),
            ))
            .style(|s| s.items_center().gap(6.0).min_width(0.0));
            tooltip(line, move || {
                let tip = if description.is_empty() { tip.clone() } else { format!("{description}\n\n{tip}") };
                label(move || tip.clone())
            })
            .into_any()
        }
        Some(Row::Note) | None => {
            let note = if state(ui).list.with_untracked(Option::is_none) {
                "Reading the skills…"
            } else {
                "No skills yet: New Skill makes one."
            };
            container(label(move || note)).style(move |s| s.color(t.get().desc)).into_any()
        }
    }
}

fn count_badge(ui: Ui, n: usize) -> impl IntoView {
    let t = ui.colors;
    label(move || n.to_string()).style(move |s| {
        let c = t.get();
        s.font_size(11.0)
            .padding_horiz(5.0)
            .height(16.0)
            .min_width(16.0)
            .items_center()
            .justify_center()
            .border_radius(8.0)
            .background(c.badge_bg)
            .color(c.badge_fg)
            .flex_shrink(0.0_f32)
    })
}

/// The title's "…" menu: the items of the view's title that are not drawn as its actions.
pub fn more_rows(rows: Vec<crate::menus::Row>) -> Vec<crate::menus::Row> {
    use crate::menus::Row as M;
    let mut out: Vec<M> = Vec::new();
    for r in rows {
        match r {
            M::Entry { icon, .. } if !icon.is_empty() => {}
            M::Line if out.is_empty() || matches!(out.last(), Some(M::Line)) => {}
            r => out.push(r),
        }
    }
    if matches!(out.last(), Some(M::Line)) {
        out.pop();
    }
    out
}

/// Opens the title's "…" menu at the top right of the list.
fn more(ui: Ui, send: &Send) {
    use crate::widgets::popup;
    let Some(view) = state(ui).tree.view() else { return };
    let r = view.layout_rect();
    let rows = more_rows(crate::menus::rows(ui, crate::menus::Menu::ViewTitle(View::Skills)));
    popup::open(ui, popup::At::point(view, r.x1 - 8.0, r.y0), popup::of_rows(ui, send, rows));
}

/// The actions in its title: none of its own (they are the title menu's).
pub fn actions(_ui: Ui, _send: Send) -> Option<floem::AnyView> {
    None
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{Menu, item};
    let list = |ui: Ui| state(ui).list.get_untracked().unwrap_or_default();
    add(command(NEW, "Skills", "New Skill...", new_skill));
    add(command(REFRESH, "Skills", "Refresh", |_, send| ask(send, true)));
    add(command(COLLAPSE, "Skills", "Collapse All", move |ui, _| {
        let all: BTreeSet<String> = list(ui).groups.iter().map(|g| g.id.clone()).collect();
        state(ui).folded.set(all);
    }));
    add(command(IMPORT, "Skills", "Import Skills...", import));
    add(command(EXPORT_OWN, "Skills", "Export the Project's and Yours...", move |ui, send| {
        export(ui, send, names_in(&list(ui), &["project", "yours"]), "skills.zip", "Export the Project's and Yours")
    }));
    add(command(EXPORT_ALL, "Skills", "Export All...", move |ui, send| {
        export(ui, send, names_in(&list(ui), &[]), "all-skills.zip", "Export All Skills")
    }));
    add(command(MORE, "Skills", "More Actions...", more).hidden());
    add(command(BUILT_IN, "Skills", "Built-in Skills", move |ui, send| {
        let on = list(ui).builtin;
        send(json!({"type": "set_setting", "key": BUILT_IN_KEY, "value": !on}));
        ask(send, false);
    }));
    let title = |id: &str, icon: &'static str, order: i32| item(id).icon(icon).group("navigation").order(order);
    let at = Menu::ViewTitle(View::Skills);
    crate::menus::add(at, title(NEW, "add", 1));
    crate::menus::add(at, title(REFRESH, "refresh", 2));
    crate::menus::add(at, title(COLLAPSE, "collapse-all", 3));
    crate::menus::add(at, title(MORE, "ellipsis", 4));
    crate::menus::add(at, item(IMPORT).group("1_transfer").order(1));
    crate::menus::add(at, item(EXPORT_OWN).group("1_transfer").order(2));
    crate::menus::add(at, item(EXPORT_ALL).group("1_transfer").order(3));
    crate::menus::add(at, item(BUILT_IN).group("2_built_in").checked(move |ui| list(ui).builtin));
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    let said = |m: &Value| {
        m["left_out"].as_array().filter(|l| !l.is_empty()).map(|l| {
            let l: Vec<&str> = l.iter().filter_map(Value::as_str).collect();
            format!(" Left out: {}.", l.join("; "))
        })
    };
    match m["type"].as_str().unwrap_or("") {
        "skills" => {
            let list = list_of(m);
            if st.list.with_untracked(|l| l.as_ref() != Some(&list)) {
                st.list.set(Some(list));
            }
            true
        }
        "skill_made" => {
            if let Some(e) = m["error"].as_str() {
                toast::error(ui, e);
                return true;
            }
            let name = m["name"].as_str().unwrap_or("");
            match (m["open"]["project"].as_str(), m["open"]["user"].as_str()) {
                (Some(p), _) => ui.open(p, send),
                (_, Some(u)) => crate::settings::open_user(ui, send, u),
                _ => {
                    toast::info(ui, &format!("/{name} is made in Arena: write it on Arena's Skills page."));
                }
            }
            ask(send, false);
            true
        }
        "skill_exported" => {
            match m["error"].as_str() {
                Some(e) => toast::error(ui, e),
                None => toast::info(
                    ui,
                    &format!("Exported to {}.{}", m["to"].as_str().unwrap_or(""), said(m).unwrap_or_default()),
                ),
            };
            true
        }
        "skill_import_preview" => {
            match m["error"].as_str() {
                Some(e) => {
                    toast::error(ui, e);
                }
                None => after_preview(ui, send, preview_of(m)),
            }
            true
        }
        "skill_imported" => {
            if let Some(e) = m["error"].as_str() {
                toast::error(ui, e);
                return true;
            }
            let names =
                |k: &str| -> Vec<&str> { m[k].as_array().into_iter().flatten().filter_map(Value::as_str).collect() };
            let (done, kept) = (names("done"), names("kept"));
            let mut text = if done.is_empty() {
                "No skill came in.".to_string()
            } else {
                format!("Imported {}: used from the next message.", done.join(", "))
            };
            if !kept.is_empty() {
                text.push_str(&format!(" Kept as they were: {}.", kept.join(", ")));
            }
            text.push_str(&said(m).unwrap_or_default());
            toast::info(ui, &text);
            ask(send, false);
            true
        }
        // A skill may be written by the agent, or changed on disk, while it answers.
        "turn_end" | "state" => {
            if st.list.with_untracked(Option::is_some) {
                ask(send, false);
            }
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> List {
        list_of(&json!({
            "type": "skills",
            "can_arena": true,
            "groups": [
                {"id": "project", "title": "This project", "skills": [
                    {"name": "release", "description": "Tag it", "origin": ".code-arena/skills",
                     "path": ".code-arena/skills/release/SKILL.md", "open": {"project": ".code-arena/skills/release/SKILL.md"}},
                    {"name": "review", "description": "", "origin": ".arena/skills", "path": "p", "open": {"project": "p"}}
                ]},
                {"id": "yours", "title": "Yours", "skills": [
                    {"name": "tests", "description": "Write tests", "origin": "~/.config/code-arena/skills", "path": "t",
                     "open": {"user": "skills/tests/SKILL.md"}}
                ]},
                {"id": "arena", "title": "From Arena", "skills": []},
                {"id": "builtin", "title": "Built in", "skills": [
                    {"name": "explain", "description": "Explain", "origin": "Built in", "path": "x", "open": null}
                ]}
            ]
        }))
    }

    #[test]
    fn the_list_is_read_with_how_each_skill_opens() {
        let l = list();
        assert!(l.can_arena && l.builtin);
        assert_eq!(l.groups[0].skills[0].open, Open::Project(".code-arena/skills/release/SKILL.md".into()));
        assert_eq!(l.groups[1].skills[0].open, Open::User("skills/tests/SKILL.md".into()));
        assert_eq!(l.groups[3].skills[0].open, Open::None);
        assert_eq!(origins(&l.groups[0]), ".code-arena/skills, .arena/skills");
        assert_eq!(names_in(&l, &["project", "yours"]), ["release", "review", "tests"]);
        assert_eq!(names_in(&l, &[]).len(), 4);
    }

    #[test]
    fn the_rows_are_the_groups_with_skills_and_a_folded_group_hides_its_own() {
        let l = list();
        let ids = |f: &BTreeSet<String>| rows(&l, f).into_iter().map(|n| n.id).collect::<Vec<_>>();
        assert_eq!(
            ids(&BTreeSet::new()),
            [
                "g:project",
                "s:project:release",
                "s:project:review",
                "g:yours",
                "s:yours:tests",
                "g:builtin",
                "s:builtin:explain"
            ]
        );
        let folded: BTreeSet<String> = ["project".to_string()].into();
        assert_eq!(ids(&folded), ["g:project", "g:yours", "s:yours:tests", "g:builtin", "s:builtin:explain"]);
        assert_eq!(rows(&l, &folded)[0].fold, Some(false));
        assert_eq!(rows(&List::default(), &BTreeSet::new())[0].id, "note");
        assert_eq!(row_of("s:yours:tests"), Some(Row::Skill("yours".into(), "tests".into())));
        assert_eq!(row_of("g:arena"), Some(Row::Group("arena".into())));
    }

    #[test]
    fn a_name_is_lower_case_with_dashes_and_a_rename_finds_one_free() {
        assert!(good_name("write-tests_2"));
        for bad in ["", "Tests", "-x", "a b", "a/b", &"x".repeat(65)] {
            assert!(!good_name(bad), "{bad:?}");
        }
        assert_eq!(free_name("tests", &["tests".into(), "tests-2".into()]), "tests-3");
    }

    #[test]
    fn a_preview_says_what_comes_in_what_is_there_and_what_held_a_secret() {
        let p = preview_of(&json!({
            "type": "skill_import_preview", "path": "x.zip",
            "skills": [
                {"name": "a", "exists": true, "taken": ["project"], "secret": true},
                {"name": "b", "exists": false, "taken": [], "secret": false}
            ],
            "left_out": ["a/env.md: it looks like it holds a key"]
        }));
        assert_eq!(p.skills, [("a".to_string(), vec!["project".to_string()]), ("b".to_string(), vec![])]);
        assert_eq!(p.secret, ["a"]);
        assert_eq!(p.left_out.len(), 1);
    }

    #[test]
    fn the_more_menu_has_the_titles_items_without_an_icon_and_no_stray_line() {
        use crate::menus::Row as M;
        let entry = |title: &str, icon: &'static str| M::Entry {
            title: title.into(),
            keys: String::new(),
            icon,
            command: Some(title.into()),
            checked: false,
        };
        let got = more_rows(vec![
            entry("New", "add"),
            M::Line,
            entry("Import", ""),
            M::Line,
            M::Line,
            entry("Built-in", ""),
            M::Line,
            entry("Help", "question"),
        ]);
        let titles: Vec<String> = got
            .iter()
            .map(|r| match r {
                M::Entry { title, .. } => title.clone(),
                M::Line => "-".into(),
            })
            .collect();
        assert_eq!(titles, ["Import", "-", "Built-in"]);
    }
}
