//! Source Control with several repositories, as VS Code's: the project's own and, as `git.autoRepositoryDetection`
//! says, those in its folders (down to `git.repositoryScanMaxDepth`, not in `git.repositoryScanIgnoredFolders`) and
//! those the open files are in, at most 100. With more than one, the view begins with a section of them ("2
//! repositories"): each row has the repository's name, its branch, how far it is from its upstream and its count of
//! changes, and while the pointer is on it Commit and Refresh. The one picked is the one the commit box and the
//! changes below are of; each keeps its own message. The palette's git commands ask "Repository" first, and Fetch
//! From All Repositories fetches each.
//!
//! The driver names a file of a repository from that repository's top; here every file is named from the project's
//! top (`full`), and what is sent is named again from its repository's (`split`). What every part has (`register`,
//! `take`) is said in `ext.rs`.

use std::collections::HashMap;

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo, create_rw_signal};
use floem::style::CursorStyle;
use floem::views::{Decorators, container, dyn_stack, empty, h_stack, label, tooltip, v_stack};
use serde_json::{Value, json};

use super::{Scm, Status};
use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::quick;

/// A repository Source Control shows: its folder from the project's top (empty: the project's own), its name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Repo {
    pub path: String,
    pub name: String,
}

/// What this part keeps, one per window.
#[derive(Clone, Copy)]
pub struct Repos {
    /// The repositories, the project's own first; until the driver says, the project's own alone.
    pub items: RwSignal<Vec<Repo>>,
    /// Each repository's last status, by its folder.
    pub statuses: RwSignal<HashMap<String, Status>>,
    /// The repository picked: the commit box and the changes are its.
    pub selected: RwSignal<String>,
    /// The message each repository not picked had in the commit box.
    messages: RwSignal<HashMap<String, String>>,
    /// What the last ask said (the settings and the open files): asked again only when it is another.
    asked: RwSignal<Value>,
    /// A command run from the view itself, which knows its repository: it does not ask which.
    quiet: RwSignal<bool>,
}

impl crate::ext::State for Repos {
    fn new(ui: Ui) -> Repos {
        Repos {
            items: ui.scope.create_rw_signal(vec![Repo::default()]),
            statuses: ui.scope.create_rw_signal(HashMap::new()),
            selected: ui.scope.create_rw_signal(String::new()),
            messages: ui.scope.create_rw_signal(HashMap::new()),
            asked: ui.scope.create_rw_signal(Value::Null),
            quiet: ui.scope.create_rw_signal(false),
        }
    }
}

fn state(ui: Ui) -> Repos {
    ui.state::<Repos>()
}

/// The repository picked.
pub fn selected(ui: Ui) -> String {
    state(ui).selected.get_untracked()
}

/// Whether a message of the driver is of the repository picked.
pub fn is_selected(ui: Ui, m: &Value) -> bool {
    state(ui).selected.with_untracked(|s| m["repo"].as_str().unwrap_or("") == s)
}

/// Whether there is more than one repository. Followed.
pub fn many(ui: Ui) -> bool {
    state(ui).items.with(|r| r.len() > 1)
}

/// A repository's file named from the project's top.
pub fn full(repo: &str, path: &str) -> String {
    if repo.is_empty() { path.to_string() } else { format!("{repo}/{path}") }
}

/// The repository a file of the project is in (the deepest of `repos` it is under) and the file named from that
/// repository's top.
pub fn split_of<'a>(repos: impl IntoIterator<Item = &'a str>, path: &str) -> (String, String) {
    let mut best: Option<&str> = None;
    for r in repos {
        let under = !r.is_empty() && path.strip_prefix(r).is_some_and(|rest| rest.starts_with('/'));
        if under && best.is_none_or(|b| r.len() > b.len()) {
            best = Some(r);
        }
    }
    match best {
        Some(r) => (r.to_string(), path[r.len() + 1..].to_string()),
        None => (String::new(), path.to_string()),
    }
}

/// As `split_of`, among the repositories shown.
pub fn split(ui: Ui, path: &str) -> (String, String) {
    state(ui).items.with_untracked(|items| split_of(items.iter().map(|r| r.path.as_str()), path))
}

/// Paths of the project, grouped by their repositories (in the order first met) and named from each one's top.
pub fn group(ui: Ui, paths: &[String]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for p in paths {
        let (repo, rel) = split(ui, p);
        match out.iter_mut().find(|(r, _)| *r == repo) {
            Some((_, list)) => list.push(rel),
            None => out.push((repo, vec![rel])),
        }
    }
    out
}

/// The repositories asked of the driver, as the settings and the open files say now; only when that is not what
/// was last asked (`force`: asked anyway, Refresh).
pub fn ask(ui: Ui, send: &Send, force: bool) {
    let me = state(ui);
    let setting = |k: &str| crate::settings::now(ui, k, None);
    let mut open: Vec<String> = ui.tabs.with_untracked(|tabs| {
        tabs.iter().filter(|t| t.kind.is_none() && t.diff.is_none()).map(|t| t.file.clone()).collect()
    });
    open.sort();
    open.dedup();
    let detect = match setting("git.autoRepositoryDetection") {
        Value::Null => json!(true),
        v => v,
    };
    let depth = setting("git.repositoryScanMaxDepth").as_i64().unwrap_or(1);
    let ignored = match setting("git.repositoryScanIgnoredFolders") {
        Value::Array(a) => Value::Array(a),
        _ => json!(["node_modules"]),
    };
    let m = json!({"type": "git_repositories", "repo": "", "detect": detect, "depth": depth, "ignored": ignored,
        "open": open});
    if !force && me.asked.with_untracked(|a| *a == m) {
        return;
    }
    me.asked.set(m.clone());
    send(m);
}

/// The repositories the driver said, as they are kept: the project's own alone when there is none (it offers
/// Initialize Repository).
pub fn items_of(m: &Value) -> Vec<Repo> {
    let items: Vec<Repo> = m["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some(Repo { path: r["path"].as_str()?.to_string(), name: r["name"].as_str().unwrap_or("").to_string() })
        })
        .collect();
    if items.is_empty() { vec![Repo::default()] } else { items }
}

/// A repository's status kept; the count of changes of them all, for the view's icon.
pub fn record(ui: Ui, repo: &str, s: &Status) -> usize {
    let me = state(ui);
    if me.statuses.with_untracked(|m| m.get(repo) != Some(s)) {
        me.statuses.update(|m| {
            m.insert(repo.to_string(), s.clone());
        });
    }
    let shown = me.items.with_untracked(|items| items.iter().map(|r| r.path.clone()).collect::<Vec<_>>());
    me.statuses.with_untracked(|m| shown.iter().filter_map(|r| m.get(r)).map(|s| s.total).sum())
}

/// A repository picked: the commit box keeps the message of the one before for it and takes this one's; the
/// changes and the branch shown are this one's, and its status is asked again.
pub fn select(ui: Ui, send: &Send, repo: &str) {
    let me = state(ui);
    let scm = ui.state::<Scm>();
    let was = me.selected.get_untracked();
    if was != repo {
        let message = scm.message.get_untracked();
        let next = me.messages.try_update(|m| {
            m.insert(was, message);
            m.remove(repo).unwrap_or_default()
        });
        me.selected.set(repo.to_string());
        scm.message.set(next.unwrap_or_default());
        let s = me.statuses.with_untracked(|m| m.get(repo).cloned());
        if let Some(b) = s.as_ref().and_then(|s| s.branch.clone())
            && ui.branch.with_untracked(|was| *was != b)
        {
            ui.branch.set(b);
        }
        if scm.status.with_untracked(|was| *was != s) {
            scm.status.set(s);
        }
    }
    send(json!({"type": "git_status", "repo": repo}));
}

/// The rows of the "Repository" pick: each repository's name, its folder and its branch.
pub fn pick_rows(items: &[Repo], statuses: &HashMap<String, Status>) -> Vec<quick::Item> {
    items
        .iter()
        .map(|r| {
            let branch = statuses.get(&r.path).and_then(|s| s.branch.clone()).unwrap_or_default();
            let mut item = quick::item(&r.path, &r.name).icon("repo").description(&branch);
            if !r.path.is_empty() {
                item = item.detail(&r.path);
            }
            item
        })
        .collect()
}

/// Runs a git command on a repository: with several (and not from the view itself), the one asked first.
pub fn choose(ui: Ui, send: &Send, run: impl Fn(Ui, &Send) + 'static) {
    let me = state(ui);
    if !many(ui) || me.quiet.get_untracked() {
        run(ui, send);
        return;
    }
    let rows = me.items.with_untracked(|items| me.statuses.with_untracked(|s| pick_rows(items, s)));
    let send = send.clone();
    quick::pick("Repository").placeholder("Choose a repository").items(rows).open(ui, move |item| {
        if let Some(item) = item {
            select(ui, &send, &item.id);
            run(ui, &send);
        }
    });
}

/// A command run from the view, which knows its repository (the one picked): nothing is asked.
pub fn quietly(ui: Ui, run: impl FnOnce()) {
    let me = state(ui);
    me.quiet.set(true);
    run();
    me.quiet.set(false);
}

/// Every repository's folder.
pub fn all(ui: Ui) -> Vec<String> {
    state(ui).items.with_untracked(|items| items.iter().map(|r| r.path.clone()).collect())
}

/// What a repository's row says at its right: its branch and how far it is from its upstream, and its count.
#[derive(Clone, Debug, Default, PartialEq)]
struct Line {
    branch: String,
    sync: String,
    total: usize,
}

fn line_of(s: Option<&Status>) -> Line {
    let Some(s) = s else { return Line::default() };
    Line {
        branch: s.branch.clone().unwrap_or_default(),
        sync: if s.upstream.is_some() { super::branches::counts(s.behind, s.ahead, false) } else { String::new() },
        total: s.total,
    }
}

/// A small button of a row, as a tree's.
fn tool(ui: Ui, glyph: &'static str, tip: &'static str, run: impl Fn() + 'static) -> impl IntoView {
    let t = ui.colors;
    tooltip(
        container(icon(glyph, 16.0)).on_click_stop(move |_| run()).style(move |s| {
            let c = t.get();
            s.width(22.0)
                .height(22.0)
                .items_center()
                .justify_center()
                .border_radius(5.0)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.toolbar_hover))
        }),
        move || label(move || tip),
    )
}

/// A repository's row.
fn row(ui: Ui, send: Send, repo: Repo) -> impl IntoView {
    let t = ui.colors;
    let me = state(ui);
    let path = repo.path.clone();
    let now = create_memo(move |_| me.statuses.with(|m| line_of(m.get(&path))));
    let path = repo.path.clone();
    let picked = create_memo(move |_| me.selected.with(|s| *s == path));
    let hovered = create_rw_signal(false);
    let (c1, c2, c3) = (send.clone(), send.clone(), send.clone());
    let (p1, p2, p3) = (repo.path.clone(), repo.path.clone(), repo.path.clone());
    let tools = h_stack((
        tool(ui, "check", "Commit", move || {
            select(ui, &c1, &p1);
            super::commit(ui, &c1, false);
        }),
        tool(ui, "refresh", "Refresh", move || {
            c2(json!({"type": "git_status", "repo": p2}));
        }),
    ))
    .style(move |s| s.gap(2.0).apply_if(!hovered.get(), |s| s.hide()));
    let name = repo.name.clone();
    h_stack((
        icon("repo", 16.0).style(move |s| s.color(t.get().icon).flex_shrink(0.0_f32)),
        label(move || name.clone()).style(|s| s.font_bold().text_ellipsis().min_width(0.0).flex_shrink(1.0_f32)),
        empty().style(|s| s.flex_grow(1.0_f32)),
        tools,
        h_stack((
            icon("git-branch", 14.0),
            label(move || now.with(|l| l.branch.clone())).style(|s| s.text_ellipsis().max_width(120.0)),
        ))
        .style(move |s| s.gap(3.0).items_center().apply_if(now.with(|l| l.branch.is_empty()), |s| s.hide())),
        label(move || now.with(|l| l.sync.clone()))
            .style(move |s| s.apply_if(now.with(|l| l.sync.is_empty()), |s| s.hide())),
        super::count_of(t, move || now.with(|l| l.total)),
    ))
    .on_click_stop(move |_| select(ui, &c3, &p3))
    .on_event_cont(floem::event::EventListener::PointerEnter, move |_| hovered.set(true))
    .on_event_cont(floem::event::EventListener::PointerLeave, move |_| hovered.set(false))
    .style(move |s| {
        let c = t.get();
        s.width_full()
            .height(22.0)
            .padding_left(20.0)
            .gap(6.0)
            .items_center()
            .font_size(13.0)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.background(c.list_hover))
            .apply_if(picked.get(), |s| s.background(c.list_inactive))
    })
}

/// The section of the repositories at the view's top, shown while there is more than one.
pub fn section(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let me = state(ui);
    let n = create_memo(move |_| me.items.with(Vec::len));
    let head = label(move || format!("{} repositories", n.get())).style(move |s| {
        let c = t.get();
        s.width_full().height(22.0).padding_left(12.0).items_center().font_size(11.0).font_bold().color(c.section_fg)
    });
    let rows = dyn_stack(
        move || me.items.get(),
        |r| r.path.clone(),
        move |r| row(ui, send.clone(), r),
    )
    .style(|s| s.flex_col().width_full());
    v_stack((head, rows)).style(move |s| {
        let c = t.get();
        s.width_full()
            .padding_bottom(4.0)
            .border_bottom(1.0)
            .border_color(c.section_border)
            .apply_if(n.get() < 2, |s| s.hide())
    })
}

/// What it adds to the window's registries, once at start: Fetch From All Repositories.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    add(command(FETCH_ALL, "Git", "Fetch From All Repositories", |ui, send| {
        for repo in all(ui) {
            send(json!({"type": "git_fetch", "repo": repo, "all": true}));
        }
    })
    .when(many));
}

/// The command that fetches every repository's remotes.
pub const FETCH_ALL: &str = "git.fetchAllRepositories";

/// The driver's list of the repositories, taken in: kept when it is another, the picked one kept while it is
/// still there (else the first), and each one's status asked. The list asked again when the settings may say
/// another. Never taken away (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let me = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "git_repositories" => {
            let items = items_of(m);
            if me.items.with_untracked(|was| *was != items) {
                me.statuses.update(|s| s.retain(|k, _| items.iter().any(|r| r.path == *k)));
                me.items.set(items.clone());
            }
            let picked = me.selected.get_untracked();
            if !items.iter().any(|r| r.path == picked) {
                select(ui, send, &items[0].path);
            }
            for r in items.iter().filter(|r| !r.path.is_empty()) {
                send(json!({"type": "git_status", "repo": r.path}));
            }
        }
        // The settings are taken in after this part: they are read a moment later.
        "settings" => {
            let send = send.clone();
            floem::action::exec_after(std::time::Duration::ZERO, move |_| ask(ui, &send, false));
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_named_from_its_repository() {
        let repos = ["", "lib", "lib/inner", "app"];
        assert_eq!(split_of(repos, "lib/a.rs"), ("lib".into(), "a.rs".into()));
        assert_eq!(split_of(repos, "lib/inner/b/c.rs"), ("lib/inner".into(), "b/c.rs".into()));
        assert_eq!(split_of(repos, "library/a.rs"), (String::new(), "library/a.rs".into()));
        assert_eq!(split_of(repos, "src/x.rs"), (String::new(), "src/x.rs".into()));
        assert_eq!(full("lib", "a.rs"), "lib/a.rs");
        assert_eq!(full("", "a.rs"), "a.rs");
    }

    #[test]
    fn no_repository_said_keeps_the_projects_own() {
        assert_eq!(items_of(&json!({"items": []})), vec![Repo::default()]);
        let two = items_of(&json!({"items": [{"path": "", "name": "p"}, {"path": "lib", "name": "lib"}]}));
        assert_eq!(two[1], Repo { path: "lib".into(), name: "lib".into() });
    }

    #[test]
    fn the_pick_names_each_repository_with_its_branch() {
        let items = vec![Repo { path: String::new(), name: "p".into() }, Repo { path: "lib".into(), name: "lib".into() }];
        let mut statuses = HashMap::new();
        statuses.insert("lib".to_string(), Status { branch: Some("dev".into()), ..Status::default() });
        let rows = pick_rows(&items, &statuses);
        assert_eq!((rows[1].id.as_str(), rows[1].label.as_str()), ("lib", "lib"));
        assert_eq!((rows[1].description.as_str(), rows[1].detail.as_str()), ("dev", "lib"));
    }

    #[test]
    fn a_row_says_branch_sync_and_count() {
        let s = Status { branch: Some("main".into()), upstream: Some("o/main".into()), ahead: 2, total: 3, ..Status::default() };
        assert_eq!(line_of(Some(&s)), Line { branch: "main".into(), sync: "2↑".into(), total: 3 });
        assert_eq!(line_of(None), Line::default());
    }
}
