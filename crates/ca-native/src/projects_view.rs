//! The Projects view: the Arena projects the person sees. Mine, Global and All I See narrow the list to the person's
//! own (and those they are in), the company's global ones, or every one; a box finds a project by its words. Each row
//! has its name (bold for the one the session counts to), whether it is global or archived, the person's role in
//! it, and a line of how far it is: how much of its goals are done, this month's spend against its budget, whether
//! this session counts to it and whether this folder's repository is in it. The pointer on a row the session may go
//! in shows "Put this session in it". New Project (global when the person may make one so) and Refresh are in the
//! view's title.
//!
//! What the session said of the projects is `assistants.rs`'s, which takes the replies in; this view only draws it.
//! It is not in the window of the edition without Arena.

use floem::IntoView;
use floem::event::EventListener;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{
    Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, text_input, v_stack,
};
use serde_json::{Value, json};

use crate::assistants::{self, Project, Projects};
use crate::icons::icon;
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::widgets::{Theme, action, bold, quick};

/// Whether the view is in the window.
pub const READY: bool = true;

/// The view's title commands.
pub const NEW: &str = "codeArena.projects.new";
const REFRESH: &str = "codeArena.projects.refresh";

/// Said when the person has no project in the scope, and on a row the session may go in.
const EMPTY: &str = "No project here yet. New Project makes one.";
const PUT: &str = "Put this session in it";

/// Which projects are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Mine,
    Global,
    All,
}

impl Scope {
    pub fn title(self) -> &'static str {
        match self {
            Scope::Mine => "Mine",
            Scope::Global => "Global",
            Scope::All => "All I See",
        }
    }
}

/// Whether a project is in the scope and has every word typed in its name or description (any case).
pub fn shown(p: &Project, scope: Scope, find: &str) -> bool {
    let scoped = match scope {
        Scope::Mine => p.mine,
        Scope::Global => p.global,
        Scope::All => true,
    };
    let hay = format!("{} {}", p.name, p.description).to_lowercase();
    scoped && find.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

/// Cents as dollars: whole ones without cents.
pub fn money(cents: u64) -> String {
    if cents % 100 == 0 { format!("${}", cents / 100) } else { format!("${}.{:02}", cents / 100, cents % 100) }
}

/// What is said after a project's name: global, archived, the person's role.
pub fn tags(p: &Project) -> String {
    let role = match p.role.as_str() {
        "owner" => "Owner",
        "contributor" => "Contributor",
        "viewer" => "Viewer",
        _ => "",
    };
    [p.global.then_some("Global"), p.archived.then_some("Archived"), (!role.is_empty()).then_some(role)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

/// The line under a project's name: its goals done, its spend, and how this session and folder are in it.
pub fn progress(p: &Project, current: bool, suggested: bool) -> String {
    let goals = (p.goals > 0).then(|| format!("{}% of its goals", p.goals_done * 100 / p.goals));
    let spend = match p.budget {
        Some(b) => Some(format!("{} this month of {}", money(p.spent), money(b))),
        None if p.spent > 0 => Some(format!("{} this month", money(p.spent))),
        None => None,
    };
    [
        goals,
        spend,
        current.then(|| "This session counts to it".to_string()),
        suggested.then(|| "This folder's repository is in it".to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" \u{b7} ")
}

/// A row as it is drawn: the project, and whether the session counts to it and the folder's repository is in it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Row {
    pub project: Project,
    pub current: bool,
    pub suggested: bool,
}

/// The rows shown, the session's project first.
pub fn rows(got: &Projects, scope: Scope, find: &str) -> Vec<Row> {
    let current = got.current.as_ref().map(|(id, _)| id.as_str());
    let mut out: Vec<Row> = got
        .items
        .iter()
        .flatten()
        .filter(|p| shown(p, scope, find))
        .map(|p| Row {
            project: p.clone(),
            current: current == Some(p.id.as_str()),
            suggested: got.suggested.as_deref() == Some(p.id.as_str()),
        })
        .collect();
    out.sort_by_key(|r| !r.current);
    out
}

/// What the body shows.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Body {
    /// The session has not said yet.
    Waiting,
    /// Words alone: signed out, a problem, none in the scope, none found.
    Words(String),
    List,
}

/// The view's state in a window.
#[derive(Clone, Copy)]
pub struct ProjectsState {
    pub scope: RwSignal<Scope>,
    pub find: RwSignal<String>,
}

impl crate::ext::State for ProjectsState {
    fn new(ui: Ui) -> Self {
        ProjectsState { scope: ui.scope.create_rw_signal(Scope::All), find: ui.scope.create_rw_signal(String::new()) }
    }
}

/// A new project: its name asked, then who sees it when the person may make a global one.
fn new_project(ui: Ui, send: &Send) {
    let got = assistants::state(ui).projects.get_untracked().unwrap_or_default();
    if !got.can_create {
        crate::widgets::toast::info(ui, "Making projects is not among what you may do in Arena.");
        return;
    }
    let send = send.clone();
    quick::input("New Project")
        .placeholder("The project's name (NAME: DESCRIPTION adds what it is for)")
        .validate(|v| v.trim().is_empty().then(|| "Name the project.".into()))
        .open(ui, move |name| {
            let Some(name) = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) else { return };
            if !got.can_global {
                send(json!({"type": "project_new", "name": name, "global": false}));
                return;
            }
            quick::pick("New Project: Who Sees It")
                .items(vec![
                    quick::item("own", "Only you and those you add").icon("person"),
                    quick::item("global", "Everyone in the company").description("Global").icon("globe"),
                ])
                .open(ui, move |who| {
                    if let Some(who) = who {
                        send(json!({"type": "project_new", "name": name, "global": who.id == "global"}));
                    }
                });
        });
}

/// The view's own content.
pub fn view(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st, got) = (ui.colors, ui.state::<ProjectsState>(), assistants::state(ui).projects);
    if got.with_untracked(Option::is_none) {
        assistants::ask_projects(&send);
    }
    let scopes = h_stack((
        scope_tab(t, st.scope, Scope::Mine),
        scope_tab(t, st.scope, Scope::Global),
        scope_tab(t, st.scope, Scope::All),
    ))
    .style(|s| s.gap(14.0).padding_horiz(12.0).padding_top(4.0));
    let find = find_box(t, st.find);
    let shown =
        create_memo(move |_| got.with(|g| st.scope.with(|s| st.find.with(|f| g.as_ref().map(|g| rows(g, *s, f))))));
    let body = create_memo(move |_| {
        got.with(|g| {
            let Some(g) = g else { return Body::Waiting };
            if g.items.is_none() || (g.problem.is_some() && g.items.as_ref().is_some_and(Vec::is_empty)) {
                return Body::Words(g.problem.clone().unwrap_or_default());
            }
            if shown.with(|r| r.as_ref().is_some_and(|r| !r.is_empty())) {
                return Body::List;
            }
            let typed = st.find.with(|f| f.trim().to_string());
            Body::Words(if typed.is_empty() {
                EMPTY.into()
            } else {
                format!("No project has \u{201c}{typed}\u{201d}.")
            })
        })
    });
    let main = dyn_container(
        move || body.get(),
        move |b| match b {
            Body::Waiting => empty().into_any(),
            Body::Words(w) => container(label(move || w.clone()).style(move |s| {
                s.font_size(12.0).line_height(1.4).color(t.get().desc).text_overflow(TextOverflow::Wrap).min_width(0.0)
            }))
            .style(|s| s.padding_horiz(12.0).padding_top(6.0).width_full().min_width(0.0))
            .into_any(),
            Body::List => {
                let send = send.clone();
                scroll(
                    dyn_stack(move || shown.get().unwrap_or_default(), Clone::clone, move |r| row(ui, send.clone(), r))
                        .style(|s| s.flex_col().width_full().padding_bottom(10.0)),
                )
                .style(|s| s.size_full().min_height(0.0))
                .into_any()
            }
        },
    )
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full().min_height(0.0));
    v_stack((scopes, find, main)).style(move |s| s.size_full().min_height(0.0).color(t.get().side_fg)).into_any()
}

/// One of the scopes, drawn as a panel's tab: lit and underlined while it is the one listed.
fn scope_tab(t: Theme, scope: RwSignal<Scope>, me: Scope) -> impl IntoView {
    let on = create_memo(move |_| scope.get() == me);
    label(move || me.title().to_uppercase())
        .on_click_stop(move |_| {
            if scope.get_untracked() != me {
                scope.set(me);
            }
        })
        .style(move |s| {
            let c = t.get();
            let s =
                s.font_size(11.0).padding_vert(4.0).border_bottom(1.0).cursor(CursorStyle::Pointer).selectable(false);
            if on.get() {
                s.color(c.fg).border_color(c.panel_title_border)
            } else {
                s.color(c.panel_title_inactive).border_color(floem::peniko::Color::TRANSPARENT).hover(|s| s.color(c.fg))
            }
        })
}

/// The box that finds a project by its words.
fn find_box(t: Theme, value: RwSignal<String>) -> impl IntoView {
    let focused = create_rw_signal(false);
    container(
        text_input(value)
            .placeholder("Find a project")
            .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
            .on_event_cont(EventListener::FocusLost, move |_| focused.set(false))
            .style(move |s| {
                let c = t.get();
                let s = s
                    .width_full()
                    .min_width(0.0)
                    .height(26.0)
                    .padding_horiz(6.0)
                    .border(1.0)
                    .border_radius(2.0)
                    .border_color(if focused.get() { c.focus } else { c.input_border })
                    .background(c.input_bg)
                    .color(c.input_fg)
                    .cursor_color(floem::peniko::Brush::Solid(c.cursor));
                crate::widgets::steady_input(s, c.input_bg, Some(c.focus))
            }),
    )
    .style(|s| s.width_full().padding_horiz(12.0).padding_vert(8.0))
}

/// A project's row: its icon and name, its tags, its progress line, and "Put this session in it" on hover.
fn row(ui: Ui, send: Send, r: Row) -> floem::AnyView {
    let t = ui.colors;
    let over = create_rw_signal(false);
    let Row { project: p, current, suggested } = r;
    let (name, tags, line) = (p.name.clone(), tags(&p), progress(&p, current, suggested));
    let (no_tags, no_line, may) = (tags.is_empty(), line.is_empty(), p.open() && !current);
    let head = h_stack((
        icon("project", 16.0).style(move |s| s.color(t.get().icon).flex_shrink(0.0_f32)),
        label(move || name.clone()).style(move |s| {
            let s = s.font_size(13.0).color(t.get().fg).text_overflow(TextOverflow::Ellipsis).min_width(0.0);
            if current { bold(s) } else { s }
        }),
        label(move || tags.clone()).style(move |s| {
            s.font_size(12.0).color(t.get().desc).flex_shrink(0.0_f32).margin_left(4.0).apply_if(no_tags, |s| s.hide())
        }),
    ))
    .style(|s| s.items_center().gap(6.0).min_width(0.0).height(22.0));
    let progress = label(move || line.clone()).style(move |s| {
        s.font_size(12.0)
            .line_height(1.4)
            .color(t.get().desc)
            .text_overflow(TextOverflow::Wrap)
            .min_width(0.0)
            .padding_left(22.0)
            .apply_if(no_line, |s| s.hide())
    });
    let words = v_stack((head, progress)).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0));
    let id = p.id.clone();
    let go = send.clone();
    let put = action(t, "arrow-right", move || assistants::put_in(&go, &id));
    let put = container(floem::views::tooltip(put, || label(|| PUT)))
        .style(move |s| s.flex_shrink(0.0_f32).apply_if(!may || !over.get(), |s| s.hide()));
    h_stack((words, put))
        .on_event_cont(EventListener::PointerEnter, move |_| over.set(true))
        .on_event_cont(EventListener::PointerLeave, move |_| over.set(false))
        .style(move |s| {
            s.gap(6.0)
                .width_full()
                .padding_left(12.0)
                .padding_right(8.0)
                .padding_vert(2.0)
                .items_start()
                .hover(|s| s.background(t.get().list_hover))
        })
        .into_any()
}

/// The actions in its title: none of its own (New Project and Refresh are the title menu's).
pub fn actions(_ui: Ui, _send: Send) -> Option<floem::AnyView> {
    None
}

/// What it adds to the window's registries, once at start: New Project and Refresh, in its title.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{Menu, item};
    add(command(NEW, "Projects", "New Project\u{2026}", new_project)
        .when(|ui| assistants::state(ui).managed.get_untracked()));
    add(command(REFRESH, "Projects", "Refresh", |_, send| assistants::ask_projects(send)).hidden());
    crate::menus::add(Menu::ViewTitle(View::Projects), item(NEW).icon("add").group("navigation").order(1));
    crate::menus::add(Menu::ViewTitle(View::Projects), item(REFRESH).icon("refresh").group("navigation").order(2));
}

/// The session's messages that are this part's, taken in: none (`assistants.rs` takes the projects in).
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> Project {
        Project { id: name.to_lowercase(), name: name.into(), description: "The new API".into(), ..Project::default() }
    }

    #[test]
    fn the_scopes_and_the_find_box_narrow_the_list() {
        let own = Project { mine: true, ..project("Payments") };
        let everyone = Project { global: true, ..project("Handbook") };
        assert!(shown(&own, Scope::Mine, "") && !shown(&everyone, Scope::Mine, ""));
        assert!(shown(&everyone, Scope::Global, "") && !shown(&own, Scope::Global, ""));
        assert!(shown(&own, Scope::All, "PAY api") && !shown(&own, Scope::All, "pay web"));
    }

    #[test]
    fn a_rows_line_says_its_goals_its_spend_and_how_the_session_is_in_it() {
        let p = Project { goals: 4, goals_done: 1, spent: 1250, budget: Some(10000), ..project("P") };
        assert_eq!(
            progress(&p, true, true),
            "25% of its goals \u{b7} $12.50 this month of $100 \u{b7} This session counts to it \u{b7} This \
             folder's repository is in it"
        );
        assert_eq!(progress(&Project { spent: 300, ..project("P") }, false, false), "$3 this month");
        assert_eq!(progress(&project("P"), false, false), "");
    }

    #[test]
    fn a_rows_tags_are_global_archived_and_the_role() {
        let p = Project { global: true, archived: true, role: "viewer".into(), ..project("P") };
        assert_eq!(tags(&p), "Global \u{b7} Archived \u{b7} Viewer");
    }

    #[test]
    fn the_sessions_project_comes_first() {
        let got = Projects {
            items: Some(vec![project("A"), project("B")]),
            current: Some(("b".into(), "B".into())),
            suggested: Some("a".into()),
            ..Projects::default()
        };
        let r = rows(&got, Scope::All, "");
        assert_eq!(
            r.iter().map(|r| (r.project.id.as_str(), r.current, r.suggested)).collect::<Vec<_>>(),
            [("b", true, false), ("a", false, true)]
        );
    }
}
