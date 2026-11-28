//! Arena's assistants and projects as the session has them: the Assistant and Project submenus of the chat's More
//! menu (and their commands, a quick pick each), the status bar's project item, and what the session said of both,
//! which the Projects view (`projects_view.rs`) draws from too.
//!
//! The Assistant submenu has "None: Code Arena's own" and then the person's own assistants and those shared with
//! them, each with its description and model, a dot by the one the session works as; it is dimmed while a turn
//! runs. The Project submenu has "In no project" and the projects the session may be put in, a dot by the one its
//! chat counts to and "its repository's" by the one this folder's repository is in. Until Arena has answered a
//! submenu says "Asking Arena…", and while nothing is signed in it says how to. What the session says of a choice is
//! a corner bubble.
//!
//! The session's side is the driver's `ide_arena.rs`. The window asks for both lists when a session is taken in and
//! each time the More menu opens. Nothing of this is in the window of the edition without Arena.

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::CursorStyle;
use floem::views::{Decorators, h_stack, label, tooltip};
use serde_json::{Value, json};

use crate::commands::{self, command};
use crate::icons::icon;
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::statusbar::{self, Side};
use crate::widgets::{popup, quick, toast};

/// The commands: work as an assistant, and put the session in a project (each a quick pick).
pub const ASSISTANT: &str = "codeArena.chat.assistant";
pub const PROJECT: &str = "codeArena.chat.project";

/// Said of the session's own way of working, and of no project.
pub const OWN: &str = "None: Code Arena's own";
pub const NO_PROJECT: &str = "In no project";
/// Said until Arena has answered.
pub const ASKING: &str = "Asking Arena\u{2026}";
/// Said when the person has no assistant.
pub const NO_ASSISTANTS: &str = "You have no assistants in Arena (Chat \u{2192} Assistants makes one).";
/// Said by the project this folder's repository is in.
pub const ITS_REPOSITORY: &str = "its repository's";

/// One of the person's assistants.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Assistant {
    pub id: String,
    pub name: String,
    pub description: String,
    pub model: String,
    /// The person's own (else one shared with them).
    pub mine: bool,
}

/// What the session said of the assistants.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Assistants {
    /// None while nothing is signed in to Arena.
    pub items: Option<Vec<Assistant>>,
    /// The one the session works as: its id and name.
    pub current: Option<(String, String)>,
    pub problem: Option<String>,
}

/// One of the projects the person sees.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: String,
    pub global: bool,
    pub archived: bool,
    /// owner, contributor or viewer.
    pub role: String,
    pub mine: bool,
    pub goals: u32,
    pub goals_done: u32,
    /// This month's spend and the monthly budget, in cents.
    pub spent: u64,
    pub budget: Option<u64>,
}

impl Project {
    /// Whether a session may be put in it: an active one the person adds to.
    pub fn open(&self) -> bool {
        !self.archived && self.role != "viewer"
    }
}

/// What the session said of the projects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Projects {
    /// None while nothing is signed in to Arena.
    pub items: Option<Vec<Project>>,
    /// The one the session's chat counts to: its id and name.
    pub current: Option<(String, String)>,
    /// The one this folder's repository is in, by its id.
    pub suggested: Option<String>,
    pub problem: Option<String>,
    pub can_create: bool,
    pub can_global: bool,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn named(v: &Value) -> Option<(String, String)> {
    v["id"].as_str().map(|id| (id.to_string(), text(&v["name"])))
}

fn problem(v: &Value) -> Option<String> {
    v["problem"].as_str().filter(|p| !p.is_empty()).map(str::to_string)
}

/// Dollars as cents.
fn cents(v: &Value) -> u64 {
    (v.as_f64().unwrap_or(0.0).max(0.0) * 100.0).round() as u64
}

/// An `assistants` reply, read.
pub fn assistants_of(m: &Value) -> Assistants {
    let items = m["items"].as_array().map(|a| {
        a.iter()
            .map(|i| Assistant {
                id: text(&i["id"]),
                name: text(&i["name"]),
                description: text(&i["description"]),
                model: text(&i["model"]),
                mine: i["mine"].as_bool().unwrap_or(false),
            })
            .collect()
    });
    Assistants { items, current: named(&m["current"]), problem: problem(m) }
}

/// A `projects` reply, read.
pub fn projects_of(m: &Value) -> Projects {
    let items = m["items"].as_array().map(|a| {
        a.iter()
            .map(|i| Project {
                id: text(&i["id"]),
                name: text(&i["name"]),
                description: text(&i["description"]),
                global: i["global"].as_bool().unwrap_or(false),
                archived: i["archived"].as_bool().unwrap_or(false),
                role: text(&i["role"]),
                mine: i["mine"].as_bool().unwrap_or(false),
                goals: i["goals"].as_u64().unwrap_or(0) as u32,
                goals_done: i["goals_done"].as_u64().unwrap_or(0) as u32,
                spent: cents(&i["spent"]),
                budget: i["budget"].as_f64().filter(|b| *b > 0.0).map(|_| cents(&i["budget"])),
            })
            .collect()
    });
    Projects {
        items,
        current: named(&m["current"]),
        suggested: m["suggested"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
        problem: problem(m),
        can_create: m["can_create"].as_bool().unwrap_or(false),
        can_global: m["can_global"].as_bool().unwrap_or(false),
    }
}

/// A row of a submenu or a pick: a choice (by its id, "" for none), or words alone (dimmed, not a choice).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    /// Said after it, dimmed.
    pub note: String,
    /// Its group: choices of one group stay together, a line between two.
    pub group: &'static str,
    /// The one in force.
    pub on: bool,
    pub enabled: bool,
}

fn words(text: &str) -> Choice {
    Choice { id: String::new(), label: text.into(), note: String::new(), group: "", on: false, enabled: false }
}

/// The assistants to choose from: Code Arena's own, then the person's own, then those shared with them.
pub fn assistant_choices(got: Option<&Assistants>) -> Vec<Choice> {
    let Some(got) = got else { return vec![words(ASKING)] };
    let Some(items) = &got.items else { return vec![words(got.problem.as_deref().unwrap_or(ASKING))] };
    let current = got.current.as_ref().map(|(id, _)| id.as_str());
    let mut out = vec![Choice {
        id: String::new(),
        label: OWN.into(),
        note: String::new(),
        group: "",
        on: current.is_none(),
        enabled: true,
    }];
    for (mine, group) in [(true, "Yours"), (false, "Shared with you")] {
        out.extend(items.iter().filter(|a| a.mine == mine).map(|a| Choice {
            id: a.id.clone(),
            label: a.name.clone(),
            note: [a.description.as_str(), a.model.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" \u{b7} "),
            group,
            on: current == Some(a.id.as_str()),
            enabled: true,
        }));
    }
    if items.is_empty() {
        out.push(words(got.problem.as_deref().unwrap_or(NO_ASSISTANTS)));
    }
    out
}

/// The projects the session may be put in: none, then those open to it (the person's first).
pub fn project_choices(got: Option<&Projects>) -> Vec<Choice> {
    let Some(got) = got else { return vec![words(ASKING)] };
    let Some(items) = &got.items else { return vec![words(got.problem.as_deref().unwrap_or(ASKING))] };
    let current = got.current.as_ref().map(|(id, _)| id.as_str());
    let mut out = vec![Choice {
        id: "none".into(),
        label: NO_PROJECT.into(),
        note: String::new(),
        group: "",
        on: current.is_none(),
        enabled: true,
    }];
    let mut open: Vec<&Project> = items.iter().filter(|p| p.open() || current == Some(p.id.as_str())).collect();
    open.sort_by_key(|p| !p.mine);
    out.extend(open.into_iter().map(|p| {
        let notes: Vec<&str> = [
            (got.suggested.as_deref() == Some(p.id.as_str())).then_some(ITS_REPOSITORY),
            p.global.then_some("Global"),
        ]
        .into_iter()
        .flatten()
        .collect();
        Choice {
            id: p.id.clone(),
            label: p.name.clone(),
            note: notes.join(" \u{b7} "),
            group: "projects",
            on: current == Some(p.id.as_str()),
            enabled: true,
        }
    }));
    if let Some(problem) = &got.problem {
        out.push(words(problem));
    }
    out
}

/// What the window keeps of Arena's assistants and projects.
#[derive(Clone, Copy)]
pub struct ArenaState {
    pub assistants: RwSignal<Option<Assistants>>,
    pub projects: RwSignal<Option<Projects>>,
    /// The edition signed in to Arena (else none of this is in the window).
    pub managed: RwSignal<bool>,
    /// The session last taken in, to ask again when it is another.
    pub session: RwSignal<String>,
    /// A pick asked for before Arena had answered: opened when it does.
    pub waiting: RwSignal<Option<&'static str>>,
}

impl crate::ext::State for ArenaState {
    fn new(ui: Ui) -> Self {
        ArenaState {
            assistants: ui.scope.create_rw_signal(None),
            projects: ui.scope.create_rw_signal(None),
            managed: ui.scope.create_rw_signal(true),
            session: ui.scope.create_rw_signal(String::new()),
            waiting: ui.scope.create_rw_signal(None),
        }
    }
}

/// The window's state of them.
pub fn state(ui: Ui) -> ArenaState {
    ui.state::<ArenaState>()
}

/// Both lists asked for again.
pub fn ask(send: &Send) {
    send(json!({"type": "assistants"}));
    ask_projects(send);
}

/// The projects asked for again (all of them: the view narrows them itself, as typed).
pub fn ask_projects(send: &Send) {
    send(json!({"type": "projects", "scope": "all", "query": ""}));
}

/// An assistant chosen: the session works as it (none: as Code Arena's own).
fn work_as(send: &Send, id: &str) {
    send(json!({"type": "assistant_set", "id": if id.is_empty() { Value::Null } else { json!(id) }}));
}

/// A project chosen for the session ("none": no project).
pub fn put_in(send: &Send, id: &str) {
    send(json!({"type": "project_set", "id": id}));
}

/// A submenu's rows, of choices.
fn entries(choices: Vec<Choice>, chosen: impl Fn(&str) + Clone + 'static) -> Vec<popup::Item> {
    let mut out = Vec::new();
    let mut last = None;
    for c in choices {
        if last.is_some_and(|g| g != c.group) {
            out.push(popup::line());
        }
        last = Some(c.group);
        let (go, id) = (chosen.clone(), c.id.clone());
        // A menu is as wide as its widest row: long words (Arena's problem) are cut, the pick says them whole.
        let mut e = popup::entry(&short(&c.label, 60), move || go(&id)).keys(&short(&c.note, 48)).enabled(c.enabled);
        if c.enabled {
            e = e.radio(c.on);
        }
        out.push(e.into());
    }
    out
}

/// Words cut to at most `most` characters, with an ellipsis.
pub fn short(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.into();
    }
    let mut out: String = text.chars().take(most.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// The More menu's Assistant and Project submenus, put after its first group; both lists are asked for again.
pub fn more(ui: Ui, send: &Send, items: &mut Vec<popup::Item>) {
    let st = state(ui);
    if !st.managed.get_untracked() {
        return;
    }
    let (a, p) = (send.clone(), send.clone());
    let assistant =
        popup::submenu("Assistant", entries(assistant_choices(st.assistants.get_untracked().as_ref()), move |id| {
            work_as(&a, id)
        }))
        .enabled(!ui.running.get_untracked());
    let project = popup::submenu(
        "Project",
        entries(project_choices(st.projects.get_untracked().as_ref()), move |id| put_in(&p, id)),
    );
    let at = items.iter().position(|i| matches!(i, popup::Item::Line)).unwrap_or(items.len());
    items.splice(at..at, [popup::line(), assistant.into(), project.into()]);
    ask(send);
}

/// A quick pick of choices; words alone (signed out, none yet) are said in a bubble instead.
fn pick(ui: Ui, title: &str, placeholder: &str, choices: Vec<Choice>, chosen: impl Fn(&str) + 'static) {
    let items: Vec<quick::Item> = choices
        .iter()
        .filter(|c| c.enabled)
        .map(|c| {
            let item = quick::item(&c.id, &c.label).description(&c.note).group(c.group);
            if c.on { item.icon("check") } else { item }
        })
        .collect();
    if let Some(said) = choices.iter().find(|c| !c.enabled) {
        if items.len() <= 1 {
            toast::info(ui, &said.label);
            return;
        }
    }
    let active = choices.iter().find(|c| c.on).map(|c| c.id.clone()).unwrap_or_default();
    quick::pick(title).placeholder(placeholder).items(items).active(&active).open(ui, move |item| {
        if let Some(item) = item {
            chosen(&item.id)
        }
    });
}

/// The assistant pick: opened at once from what Arena said last, or once it answers.
pub fn pick_assistant(ui: Ui, send: &Send) {
    let st = state(ui);
    let Some(got) = st.assistants.get_untracked() else {
        st.waiting.set(Some(ASSISTANT));
        send(json!({"type": "assistants"}));
        return;
    };
    let go = send.clone();
    pick(ui, "Work as an Assistant", "Select an assistant to work as", assistant_choices(Some(&got)), move |id| {
        work_as(&go, id)
    });
    send(json!({"type": "assistants"}));
}

/// The project pick: opened at once from what Arena said last, or once it answers.
pub fn pick_project(ui: Ui, send: &Send) {
    let st = state(ui);
    let Some(got) = st.projects.get_untracked() else {
        st.waiting.set(Some(PROJECT));
        ask_projects(send);
        return;
    };
    let go = send.clone();
    pick(ui, "Put the Session in a Project", "Select the project this session counts to", project_choices(Some(&got)), move |id| {
        put_in(&go, id)
    });
    ask_projects(send);
}

/// The status bar's project item: the project the session's chat counts to.
fn status_item(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    let name = create_memo(move |_| {
        st.projects.with(|p| p.as_ref().and_then(|p| p.current.as_ref()).map(|(_, n)| n.clone()))
    });
    let shown = create_memo(move |_| st.managed.get() && name.with(Option::is_some));
    tooltip(
        h_stack((icon("project", 16.0), label(move || name.get().unwrap_or_default())))
            .on_click_stop(move |_| pick_project(ui, &send))
            .style(move |s| statusbar::item(t, s).cursor(CursorStyle::Pointer)),
        move || label(move || format!("This session counts to the Arena project {}", name.get().unwrap_or_default())),
    )
    .style(move |s| s.height_full().apply_if(!shown.get(), |s| s.hide()))
    .into_any()
}

/// What it adds to the window's registries, once at start: the two picks' commands and the project item.
pub fn register(_ui: Ui, _send: &Send) {
    commands::add(
        command(ASSISTANT, "Chat", "Work as an Assistant\u{2026}", pick_assistant)
            .when(|ui| state(ui).managed.get_untracked())
            .enabled(|ui| !ui.running.get_untracked()),
    );
    commands::add(
        command(PROJECT, "Chat", "Put the Session in a Project\u{2026}", pick_project)
            .when(|ui| state(ui).managed.get_untracked()),
    );
    statusbar::add("status.project", Side::Left, 150, status_item);
}

/// The session's messages that are this part's, taken in: the lists (taken away: the Projects view reads them
/// from here), and what was said of a choice (a bubble, not a line in the chat).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "state" => {
            let managed = m["edition"].as_str().is_none_or(|e| e == "managed");
            if st.managed.get_untracked() != managed {
                st.managed.set(managed);
            }
            ui.withhold(View::Projects, !managed);
            let session = m["session"].as_str().unwrap_or("");
            if managed && st.session.with_untracked(|s| s != session) {
                st.session.set(session.to_string());
                ask(send);
            }
            false
        }
        "assistants" => {
            let got = assistants_of(m);
            if st.assistants.with_untracked(|a| a.as_ref() != Some(&got)) {
                st.assistants.set(Some(got));
            }
            if st.waiting.get_untracked() == Some(ASSISTANT) {
                st.waiting.set(None);
                pick_assistant(ui, send);
            }
            true
        }
        "projects" => {
            let got = projects_of(m);
            if st.projects.with_untracked(|p| p.as_ref() != Some(&got)) {
                st.projects.set(Some(got));
            }
            if st.waiting.get_untracked() == Some(PROJECT) {
                st.waiting.set(None);
                pick_project(ui, send);
            }
            true
        }
        "notice" if matches!(m["of"].as_str(), Some("assistant_set" | "project_set" | "project_new")) => {
            let text = m["text"].as_str().unwrap_or("");
            if m["error"].as_bool() == Some(true) {
                toast::error(ui, text);
            } else {
                toast::info(ui, text);
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(c: &[Choice]) -> Vec<(&str, &str, bool)> {
        c.iter().map(|c| (c.label.as_str(), c.note.as_str(), c.on)).collect()
    }

    #[test]
    fn the_assistants_are_code_arenas_own_then_yours_then_those_shared_with_a_dot_by_the_one_at_work() {
        let got = assistants_of(&json!({"items": [
            {"id": "a2", "name": "Shared", "mine": false, "model": "big"},
            {"id": "a1", "name": "Reviewer", "description": "Reviews code", "model": "small", "mine": true}
        ], "current": {"id": "a1", "name": "Reviewer"}}));
        let c = assistant_choices(Some(&got));
        assert_eq!(
            labels(&c),
            [(OWN, "", false), ("Reviewer", "Reviews code \u{b7} small", true), ("Shared", "big", false)]
        );
        assert_eq!((c[1].group, c[2].group), ("Yours", "Shared with you"));
    }

    #[test]
    fn before_arena_answers_signed_out_or_with_none_the_submenu_says_so() {
        assert_eq!(labels(&assistant_choices(None)), [(ASKING, "", false)]);
        let out = assistants_of(&json!({"items": null, "problem": "Sign in to Arena."}));
        assert_eq!(labels(&assistant_choices(Some(&out))), [("Sign in to Arena.", "", false)]);
        let none = assistants_of(&json!({"items": []}));
        assert_eq!(assistant_choices(Some(&none)).last().map(|c| c.label.as_str()), Some(NO_ASSISTANTS));
    }

    #[test]
    fn the_projects_offered_are_the_open_ones_with_the_repositorys_marked_and_the_current_dotted() {
        let got = projects_of(&json!({"items": [
            {"id": "p1", "name": "Handbook", "global": true, "role": "contributor"},
            {"id": "p2", "name": "Payments", "role": "owner", "mine": true},
            {"id": "p3", "name": "Old", "role": "owner", "archived": true, "mine": true},
            {"id": "p4", "name": "Seen", "role": "viewer"}
        ], "current": {"id": "p2", "name": "Payments"}, "suggested": "p2"}));
        assert_eq!(
            labels(&project_choices(Some(&got))),
            [(NO_PROJECT, "", false), ("Payments", ITS_REPOSITORY, true), ("Handbook", "Global", false)]
        );
    }

    #[test]
    fn spend_is_read_in_cents_and_no_budget_is_none() {
        let got = projects_of(&json!({"items": [{"id": "p", "name": "P", "spent": 12.5, "budget": 0}]}));
        let p = &got.items.expect("items")[0];
        assert_eq!((p.spent, p.budget), (1250, None));
    }

    #[test]
    fn long_words_are_cut_with_an_ellipsis() {
        assert_eq!(short("abcdef", 4), "abc\u{2026}");
        assert_eq!(short("abc", 4), "abc");
    }
}
