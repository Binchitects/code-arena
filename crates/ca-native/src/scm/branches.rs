//! Source Control's branches and remotes, as VS Code's git: Checkout to… (a pick of the branches, the remote
//! branches and the tags, with "Create new branch…" and "Create new branch from…"), Create, Rename and Delete
//! Branch (the name checked as it is typed; "Delete anyway?" for a branch not merged), Undo Last Commit, Fetch,
//! Pull, Push (a branch with no upstream is published), Sync (asked first while `git.confirmSync`, rebasing while
//! `git.rebaseWhenSync`), Commit & Push and Commit & Sync. The view's "…" menu holds them all.
//!
//! The status bar's branch item opens Checkout to…, and the sync item beside it ("0↓ 1↑", or the cloud of Publish
//! Branch; hidden while `git.enableStatusBarSync` is off) syncs, its icon turning while git talks to a remote.
//! While `git.autofetch` is on the remotes are fetched every three minutes. The driver's errors are told in git's
//! own words.
//!
//! Each list the commands show is asked of the driver (`git_branches`, with a key saying what it is for) and
//! the pick is opened when the answer comes. What every part has (`register`, `take`) is said in `ext.rs`.

use std::time::Duration;

use floem::IntoView;
use floem::action::exec_after;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::CursorStyle;
use floem::views::{Decorators, h_stack, label, tooltip};
use serde_json::{Value, json};

use crate::commands;
use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::{dialog, popup, quick, toast};

use super::Scm;

/// How often the remotes are fetched while `git.autofetch` is on (VS Code's three minutes).
const AUTOFETCH: Duration = Duration::from_secs(180);
/// How often the sync item's icon turns a step.
const TICK: Duration = Duration::from_millis(100);

pub const CHECKOUT: &str = "git.checkout";
pub const BRANCH: &str = "git.branch";
pub const BRANCH_FROM: &str = "git.branchFrom";
pub const RENAME: &str = "git.renameBranch";
pub const DELETE: &str = "git.deleteBranch";
pub const UNDO: &str = "git.undoCommit";
pub const FETCH: &str = "git.fetch";
pub const FETCH_ALL: &str = "git.fetchAll";
pub const PULL: &str = "git.pull";
pub const PULL_REBASE: &str = "git.pullRebase";
pub const PUSH: &str = "git.push";
pub const PUSH_FORCE: &str = "git.pushForce";
pub const PUBLISH: &str = "git.publish";
pub const SYNC: &str = "git.sync";
pub const COMMIT_PUSH: &str = "git.commitPush";
pub const COMMIT_SYNC: &str = "git.commitSync";
pub const MORE: &str = "git.moreActions";

/// A branch, a remote branch or a tag, as the driver lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ref {
    pub name: String,
    pub kind: Kind,
    pub current: bool,
    pub hash: String,
    pub subject: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Local,
    Remote,
    Tag,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Local => "local",
            Kind::Remote => "remote",
            Kind::Tag => "tag",
        }
    }
}

/// The driver's list of refs, and its remotes.
pub fn refs_of(m: &Value) -> (Vec<Ref>, Vec<String>) {
    let refs = m["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| Ref {
            name: i["name"].as_str().unwrap_or("").to_string(),
            kind: match i["kind"].as_str() {
                Some("remote") => Kind::Remote,
                Some("tag") => Kind::Tag,
                _ => Kind::Local,
            },
            current: i["current"].as_bool().unwrap_or(false),
            hash: i["hash"].as_str().unwrap_or("").to_string(),
            subject: i["subject"].as_str().unwrap_or("").to_string(),
        })
        .filter(|r| !r.name.is_empty())
        .collect();
    let remotes =
        m["remotes"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(str::to_string)).collect();
    (refs, remotes)
}

/// A branch name as it is typed made into one git takes: VS Code's `git.branchWhitespaceChar`, a dash for each run
/// of spaces.
pub fn tidy(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join("-")
}

/// What is wrong with a new branch's name (git's `check-ref-format` rules, and a local branch already named so);
/// none: it may be taken.
pub fn name_problem(name: &str, taken: &[&str]) -> Option<String> {
    let name = tidy(name);
    if name.is_empty() {
        return Some("Please provide a branch name".into());
    }
    let bad = name.starts_with(['-', '/', '.'])
        || name.ends_with(['/', '.'])
        || name.ends_with(".lock")
        || name == "@"
        || ["..", "//", "@{", "/."].iter().any(|p| name.contains(p))
        || name.split('/').any(|c| c.ends_with(".lock"))
        || name.chars().any(|c| c.is_control() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'));
    if bad {
        return Some(format!("'{name}' is not a valid branch name"));
    }
    taken.contains(&name.as_str()).then(|| format!("A branch named '{name}' already exists"))
}

/// What the status bar's sync item and the Sync Changes button say of how far the branch is from its upstream:
/// the bar always both ("0↓ 1↑"), the button only what there is.
pub fn counts(behind: u64, ahead: u64, both: bool) -> String {
    if both {
        return format!("{behind}↓ {ahead}↑");
    }
    let mut parts = Vec::new();
    if behind > 0 {
        parts.push(format!("{behind}↓"));
    }
    if ahead > 0 {
        parts.push(format!("{ahead}↑"));
    }
    parts.join(" ")
}

/// The rows of Checkout to…: the two ways to make a branch, then the branches, the remote branches, the tags.
/// `create`: the rows that make one (false: a ref to make one from is being picked).
pub fn pick_rows(refs: &[Ref], create: bool) -> Vec<quick::Item> {
    let mut rows = Vec::new();
    if create {
        rows.push(quick::item("+new", "Create new branch...").icon("add"));
        rows.push(quick::item("+from", "Create new branch from...").icon("add"));
    }
    for r in refs {
        let (glyph, group) = match r.kind {
            Kind::Local => ("git-branch", "branches"),
            Kind::Remote => ("cloud", "remote branches"),
            Kind::Tag => ("tag", "tags"),
        };
        let what = if r.subject.is_empty() { r.hash.clone() } else { format!("{} • {}", r.hash, r.subject) };
        rows.push(
            quick::item(&format!("{}:{}", r.kind.word(), r.name), &r.name).icon(glyph).group(group).description(&what),
        );
    }
    rows
}

/// A row's id said back as a ref: its kind and its name.
fn ref_of(id: &str) -> Option<(&str, &str)> {
    id.split_once(':')
}

/// What this part keeps, one per window.
#[derive(Clone, Copy)]
pub struct Remote {
    /// The fetch, pull, push or sync git is doing (its message's type); none: none.
    busy: RwSignal<Option<String>>,
    /// The step the sync item's icon has turned to, and whether it is turning.
    tick: RwSignal<u64>,
    ticking: RwSignal<bool>,
    /// What to do once the commit being made is made: push or sync it.
    after_commit: RwSignal<Option<&'static str>>,
    /// The lists asked for, to tell their answers apart.
    asked: RwSignal<u64>,
    /// The fetch every three minutes has begun.
    fetching: RwSignal<bool>,
}

impl crate::ext::State for Remote {
    fn new(ui: Ui) -> Remote {
        Remote {
            busy: ui.scope.create_rw_signal(None),
            tick: ui.scope.create_rw_signal(0),
            ticking: ui.scope.create_rw_signal(false),
            after_commit: ui.scope.create_rw_signal(None),
            asked: ui.scope.create_rw_signal(0),
            fetching: ui.scope.create_rw_signal(false),
        }
    }
}

fn state(ui: Ui) -> Remote {
    ui.state::<Remote>()
}

fn scm(ui: Ui) -> Scm {
    ui.state::<Scm>()
}

fn has_repo(ui: Ui) -> bool {
    scm(ui).status.with(|s| s.as_ref().is_some_and(|s| s.root.is_some()))
}

/// The branch checked out and its upstream, as the last status said.
fn branch(ui: Ui) -> (Option<String>, Option<String>) {
    scm(ui).status.with_untracked(|s| s.as_ref().map(|s| (s.branch.clone(), s.upstream.clone())).unwrap_or_default())
}

/// Asks for the refs; the answer opens what `what` names (`checkout`, `from`, `create`, `rename`, `delete`,
/// `publish`).
fn ask_refs(ui: Ui, send: &Send, what: &str) {
    let me = state(ui);
    me.asked.update(|n| *n += 1);
    send(
        json!({"type": "git_branches", "repo": super::repos::selected(ui), "key": format!("{what}:{}", me.asked.get_untracked())}),
    );
}

/// A fetch, pull, push or sync asked of the driver: the sync item turns until it is done.
fn network(ui: Ui, send: &Send, mut m: Value) {
    let me = state(ui);
    let of = m["type"].as_str().unwrap_or("").to_string();
    m["repo"] = json!(super::repos::selected(ui));
    if me.busy.with_untracked(Option::is_none) {
        me.busy.set(Some(of));
        turn(me);
    }
    send(m);
}

/// Turns the sync item's icon while git talks to a remote, and stops once it is done.
fn turn(me: Remote) {
    if me.ticking.get_untracked() {
        return;
    }
    me.ticking.set(true);
    fn go(me: Remote) {
        exec_after(TICK, move |_| {
            if me.busy.with_untracked(Option::is_none) {
                me.ticking.set(false);
                return;
            }
            me.tick.update(|n| *n += 1);
            go(me);
        });
    }
    go(me);
}

fn push(ui: Ui, send: &Send, force: bool) {
    match branch(ui) {
        (None, _) => {
            toast::warning(ui, "Not on a branch: check one out to push it.");
        }
        (Some(_), None) => publish(ui, send),
        _ => network(ui, send, json!({"type": "git_push", "force": force})),
    }
}

fn push_force(ui: Ui, send: &Send) {
    let send = send.clone();
    dialog::ask("You are about to force push your changes, this can be destructive and could inadvertently overwrite changes made by others.")
        .detail("Are you sure to continue?")
        .kind(dialog::Kind::Warning)
        .button("OK")
        .open(ui, move |a| {
            if a.yes() {
                push(ui, &send, true);
            }
        });
}

fn publish(ui: Ui, send: &Send) {
    if branch(ui).0.is_none() {
        toast::warning(ui, "Not on a branch: check one out to publish it.");
        return;
    }
    ask_refs(ui, send, "publish");
}

fn sync(ui: Ui, send: &Send) {
    let (Some(name), upstream) = branch(ui) else {
        toast::warning(ui, "Not on a branch: check one out to sync it.");
        return;
    };
    let Some(upstream) = upstream else {
        let send = send.clone();
        dialog::ask(&format!("The branch \"{name}\" has no remote branch. Would you like to publish this branch?"))
            .button("Publish Branch")
            .open(ui, move |a| {
                if a.yes() {
                    publish(ui, &send);
                }
            });
        return;
    };
    let go = {
        let send = send.clone();
        move || {
            let rebase = crate::settings::now(ui, "git.rebaseWhenSync", None).as_bool().unwrap_or(false);
            network(ui, &send, json!({"type": "git_sync", "rebase": rebase}));
        }
    };
    if !crate::settings::now(ui, "git.confirmSync", None).as_bool().unwrap_or(true) {
        go();
        return;
    }
    let send = send.clone();
    dialog::ask(&format!("This action will pull and push commits from and to \"{upstream}\"."))
        .button("OK")
        .button("OK, Don't Ask Again")
        .open(ui, move |a| match a.button {
            Some(0) => go(),
            Some(1) => {
                send(json!({"type": "set_setting", "key": "git.confirmSync", "value": false}));
                go();
            }
            _ => {}
        });
}

/// Commits, then pushes or syncs what was committed.
fn commit_then(ui: Ui, send: &Send, then: &'static str) {
    let me = state(ui);
    me.after_commit.set(Some(then));
    if !super::commit(ui, send, false) {
        me.after_commit.set(None);
    }
}

fn undo(ui: Ui, send: &Send) {
    let send = send.clone();
    dialog::ask("Are you sure you want to undo the last commit?")
        .detail("Its changes are staged again and its message is put back in the box.")
        .button("Undo Last Commit")
        .open(ui, move |a| {
            if a.yes() {
                send(json!({"type": "git_undo_commit", "repo": super::repos::selected(ui)}));
            }
        });
}

/// The name of a new branch asked, then the branch made from `from` (none: what is checked out) and checked out.
fn create(ui: Ui, send: &Send, refs: &[Ref], from: Option<String>) {
    let taken: Vec<String> = refs.iter().filter(|r| r.kind == Kind::Local).map(|r| r.name.clone()).collect();
    let send = send.clone();
    quick::input("Branch name")
        .placeholder("Branch name")
        .prompt("Please provide a new branch name")
        .validate(move |v| name_problem(v, &taken.iter().map(String::as_str).collect::<Vec<_>>()))
        .open(ui, move |name| {
            if let Some(name) = name.map(|n| tidy(&n)).filter(|n| !n.is_empty()) {
                let mut m = json!({"type": "git_branch_create", "repo": super::repos::selected(ui), "name": name});
                if let Some(from) = from {
                    m["from"] = json!(from);
                }
                send(m);
            }
        });
}

fn rename(ui: Ui, send: &Send, refs: &[Ref]) {
    let Some(was) = branch(ui).0 else {
        toast::warning(ui, "Not on a branch: check one out to rename it.");
        return;
    };
    let taken: Vec<String> =
        refs.iter().filter(|r| r.kind == Kind::Local && r.name != was).map(|r| r.name.clone()).collect();
    let send = send.clone();
    quick::input("Rename Branch")
        .placeholder("Branch name")
        .prompt("Please provide a new branch name")
        .value(&was)
        .validate(move |v| name_problem(v, &taken.iter().map(String::as_str).collect::<Vec<_>>()))
        .open(ui, move |name| {
            if let Some(name) = name.map(|n| tidy(&n)).filter(|n| !n.is_empty() && *n != was) {
                send(json!({"type": "git_branch_rename", "repo": super::repos::selected(ui), "to": name}));
            }
        });
}

fn delete(ui: Ui, send: &Send, refs: &[Ref]) {
    let rows: Vec<quick::Item> = pick_rows(refs, false)
        .into_iter()
        .zip(refs)
        .filter(|(_, r)| r.kind == Kind::Local && !r.current)
        .map(|(i, _)| i)
        .collect();
    if rows.is_empty() {
        toast::info(ui, "There are no other branches to delete.");
        return;
    }
    let send = send.clone();
    quick::pick("Delete Branch").placeholder("Select a branch to delete").items(rows).open(ui, move |item| {
        if let Some((_, name)) = item.as_ref().and_then(|i| ref_of(&i.id)) {
            send(json!({"type": "git_branch_delete", "repo": super::repos::selected(ui), "name": name, "force": false, "key": name}));
        }
    });
}

fn checkout(ui: Ui, send: &Send, refs: Vec<Ref>) {
    let send = send.clone();
    quick::pick("Checkout to").placeholder("Select a branch or tag to checkout").items(pick_rows(&refs, true)).open(
        ui,
        move |item| {
            let Some(item) = item else { return };
            match item.id.as_str() {
                "+new" => create(ui, &send, &refs, None),
                "+from" => pick_from(ui, &send, refs),
                id => {
                    if let Some((kind, name)) = ref_of(id) {
                        send(json!({"type": "git_switch", "repo": super::repos::selected(ui), "name": name, "kind": kind}));
                    }
                }
            }
        },
    );
}

/// The ref a new branch is made from, picked, then its name asked.
fn pick_from(ui: Ui, send: &Send, refs: Vec<Ref>) {
    let send = send.clone();
    quick::pick("Create Branch From")
        .placeholder("Select a ref to create the branch from")
        .items(pick_rows(&refs, false))
        .open(ui, move |item| {
            if let Some((_, name)) = item.as_ref().and_then(|i| ref_of(&i.id)) {
                create(ui, &send, &refs, Some(name.to_string()));
            }
        });
}

fn publish_to(ui: Ui, send: &Send, remotes: Vec<String>) {
    let Some(name) = branch(ui).0 else { return };
    match remotes.as_slice() {
        [] => {
            toast::warning(ui, "Your repository has no remotes configured to publish to.");
        }
        [only] => network(ui, send, json!({"type": "git_push", "force": false, "remote": only})),
        _ => {
            let send = send.clone();
            let rows = remotes.iter().map(|r| quick::item(r, r).icon("cloud")).collect();
            quick::pick("Publish Branch")
                .placeholder(&format!("Select a remote to publish the branch '{name}' to:"))
                .items(rows)
                .open(ui, move |item| {
                    if let Some(item) = item {
                        network(ui, &send, json!({"type": "git_push", "force": false, "remote": item.id}));
                    }
                });
        }
    }
}

/// The rows the Commit button's menu adds after Commit and Commit (Amend).
pub fn commit_rows(ui: Ui, send: &Send) -> Vec<popup::Item> {
    vec![run_row(ui, send, COMMIT_PUSH, "Commit & Push"), run_row(ui, send, COMMIT_SYNC, "Commit & Sync")]
}

/// A row of a menu that runs a command, dimmed while it may not.
fn run_row(ui: Ui, send: &Send, id: &'static str, title: &str) -> popup::Item {
    let send = send.clone();
    let may = commands::find(id).is_some_and(|c| c.may_run(ui));
    popup::entry(title, move || {
        commands::run(ui, &send, id);
    })
    .keys(&commands::keys_of(id))
    .enabled(may)
    .into()
}

/// The view's "…" menu: VS Code's, the parts this window has.
fn more(ui: Ui, send: &Send) {
    let Some(view) = super::view_id(ui) else { return };
    let r = |id, title| run_row(ui, send, id, title);
    let rows = vec![
        r(PULL, "Pull"),
        r(PUSH, "Push"),
        r(CHECKOUT, "Checkout to..."),
        r(FETCH, "Fetch"),
        popup::line(),
        popup::submenu(
            "Commit",
            vec![
                r("git.commit", "Commit"),
                r("git.commitAmend", "Commit (Amend)"),
                r(COMMIT_PUSH, "Commit & Push"),
                r(COMMIT_SYNC, "Commit & Sync"),
                popup::line(),
                r(UNDO, "Undo Last Commit"),
            ],
        )
        .into(),
        popup::submenu(
            "Changes",
            vec![
                r("git.stageAll", "Stage All Changes"),
                r("git.unstageAll", "Unstage All Changes"),
                r("git.cleanAll", "Discard All Changes"),
            ],
        )
        .into(),
        popup::submenu(
            "Pull, Push",
            vec![
                r(SYNC, "Sync"),
                popup::line(),
                r(PULL, "Pull"),
                r(PULL_REBASE, "Pull (Rebase)"),
                popup::line(),
                r(PUSH, "Push"),
                r(PUSH_FORCE, "Push (Force with Lease)"),
                popup::line(),
                r(FETCH, "Fetch"),
                r(FETCH_ALL, "Fetch From All Remotes"),
            ],
        )
        .into(),
        popup::submenu(
            "Branch",
            vec![
                r(BRANCH, "Create Branch..."),
                r(BRANCH_FROM, "Create Branch From..."),
                popup::line(),
                r(RENAME, "Rename Branch..."),
                r(DELETE, "Delete Branch..."),
                popup::line(),
                r(PUBLISH, "Publish Branch..."),
            ],
        )
        .into(),
    ];
    // Under the title's "…", at the view's top right.
    let rect = view.layout_rect();
    popup::open(ui, popup::At::point(view, rect.x1 - 44.0, rect.y0), rows);
}

/// The button under Commit while there is nothing to commit and the branch is not where its upstream is: "Sync
/// Changes 1↓ 2↑", or "Publish Branch" for a branch that has none.
pub fn sync_button(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let me = scm(ui);
    let words = create_memo(move |_| {
        me.status.with(|s| {
            let s = s.as_ref()?;
            s.root.as_ref()?;
            if s.total > 0 {
                return None;
            }
            match (&s.branch, &s.upstream) {
                (Some(_), None) => Some(("cloud-upload", "Publish Branch".to_string())),
                (Some(_), Some(_)) if s.ahead + s.behind > 0 => {
                    Some(("sync", format!("Sync Changes {}", counts(s.behind, s.ahead, false))))
                }
                _ => None,
            }
        })
    });
    let busy = state(ui).busy;
    h_stack((
        icon("sync", 16.0)
            .style(move |s| s.apply_if(!words.with(|w| w.as_ref().is_some_and(|w| w.0 == "sync")), |s| s.hide())),
        icon("cloud-upload", 16.0).style(move |s| {
            s.apply_if(!words.with(|w| w.as_ref().is_some_and(|w| w.0 == "cloud-upload")), |s| s.hide())
        }),
        label(move || words.with(|w| w.as_ref().map(|w| w.1.clone()).unwrap_or_default())),
    ))
    .on_click_stop(move |_| {
        if busy.with_untracked(Option::is_some) {
            return;
        }
        match words.get_untracked() {
            Some(("cloud-upload", _)) => publish(ui, &send),
            Some(_) => sync(ui, &send),
            None => {}
        }
    })
    .style(move |s| {
        let c = t.get();
        s.width_full()
            .height(26.0)
            .gap(6.0)
            .items_center()
            .justify_center()
            .border_radius(2.0)
            .background(c.button_bg)
            .color(c.button_fg)
            .font_size(13.0)
            .selectable(false)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.background(c.button_hover))
            .apply_if(busy.with(Option::is_some), |s| s.color(c.button_fg.multiply_alpha(0.6)))
            .apply_if(words.with(Option::is_none), |s| s.hide())
    })
}

/// A status bar item: run on a click, said in its tooltip.
fn bar_item(
    view: impl IntoView + 'static,
    click: impl Fn() + 'static,
    tip: impl Fn() -> String + 'static,
) -> floem::AnyView {
    let tip = std::rc::Rc::new(tip);
    tooltip(view.on_click_stop(move |_| click()), move || {
        let tip = tip.clone();
        label(move || tip())
    })
    .into_any()
}

/// The status bar's branch item (the workbench's, made to open Checkout to…; "*" while there are changes) and the
/// sync item beside it.
fn register_status() {
    use crate::statusbar::{Side, add, item};
    add("status.branch", Side::Left, 100, |ui, send| {
        let t = ui.colors;
        let me = scm(ui);
        let dirty = create_memo(move |_| me.status.with(|s| s.as_ref().is_some_and(|s| s.total > 0)));
        let view = h_stack((
            icon("git-branch", 16.0),
            label(move || format!("{}{}", ui.branch.get(), if dirty.get() { "*" } else { "" })),
        ))
        .style(move |s| item(t, s).apply_if(ui.branch.with(|b| b.is_empty()), |s| s.hide()));
        bar_item(
            view,
            move || {
                commands::run(ui, &send, CHECKOUT);
            },
            move || format!("{} (Git) - Checkout Branch/Tag...", ui.branch.get()),
        )
    });
    add("status.sync", Side::Left, 110, |ui, send| {
        let t = ui.colors;
        let me = scm(ui);
        let remote = state(ui);
        // Some(true): the counts to its upstream; Some(false): Publish Branch; none: hidden.
        let shown = create_memo(move |_| {
            if !crate::settings::flag(ui, "git.enableStatusBarSync", None) {
                return None;
            }
            me.status.with(|s| {
                let s = s.as_ref()?;
                s.root.as_ref()?;
                s.branch.as_ref()?;
                Some((s.upstream.is_some(), s.behind, s.ahead))
            })
        });
        let busy = create_memo(move |_| remote.busy.with(Option::is_some));
        let view = h_stack((
            icon("sync", 16.0).style(move |s| {
                let turned = if busy.get() && !crate::accessibility::still(ui) {
                    (remote.tick.get() % 12) as f64 * std::f64::consts::TAU / 12.0
                } else {
                    0.0
                };
                s.rotate(turned).apply_if(!shown.with(|v| v.is_some_and(|v| v.0)), |s| s.hide())
            }),
            icon("cloud-upload", 16.0)
                .style(move |s| s.apply_if(!shown.with(|v| v.is_some_and(|v| !v.0)), |s| s.hide())),
            label(move || shown.get().filter(|v| v.0).map(|v| counts(v.1, v.2, true)).unwrap_or_default())
                .style(move |s| s.apply_if(!shown.with(|v| v.is_some_and(|v| v.0)), |s| s.hide())),
        ))
        .style(move |s| item(t, s).apply_if(shown.with(Option::is_none), |s| s.hide()));
        bar_item(
            view,
            move || {
                if busy.get_untracked() {
                    return;
                }
                let id = if shown.get_untracked().is_some_and(|v| v.0) { SYNC } else { PUBLISH };
                commands::run(ui, &send, id);
            },
            move || {
                if busy.get() {
                    "Synchronizing Changes...".to_string()
                } else if shown.get().is_some_and(|v| v.0) {
                    "Synchronize Changes".to_string()
                } else {
                    "Publish Branch".to_string()
                }
            },
        )
    });
}

/// What it adds to the window's registries, once at start: the commands, the "…" menu's action in the view's
/// title, the status bar's items.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::layout::View;
    use crate::menus::{Menu, item};
    // With several repositories, the one each is run on is asked first.
    let git = |id: &str, title: &str, run: fn(Ui, &Send)| {
        command(id, "Git", title, move |ui, send| super::repos::choose(ui, send, run)).when(has_repo)
    };
    add(git(CHECKOUT, "Checkout to...", |ui, send| ask_refs(ui, send, "checkout")));
    add(git(BRANCH, "Create Branch...", |ui, send| ask_refs(ui, send, "create")));
    add(git(BRANCH_FROM, "Create Branch From...", |ui, send| ask_refs(ui, send, "from")));
    add(git(RENAME, "Rename Branch...", |ui, send| ask_refs(ui, send, "rename")));
    add(git(DELETE, "Delete Branch...", |ui, send| ask_refs(ui, send, "delete")));
    add(git(UNDO, "Undo Last Commit", undo));
    add(git(FETCH, "Fetch", |ui, send| network(ui, send, json!({"type": "git_fetch"}))));
    add(git(FETCH_ALL, "Fetch From All Remotes", |ui, send| {
        network(ui, send, json!({"type": "git_fetch", "all": true}))
    }));
    add(git(PULL, "Pull", |ui, send| network(ui, send, json!({"type": "git_pull", "rebase": false}))));
    add(git(PULL_REBASE, "Pull (Rebase)", |ui, send| network(ui, send, json!({"type": "git_pull", "rebase": true}))));
    add(git(PUSH, "Push", |ui, send| push(ui, send, false)));
    add(git(PUSH_FORCE, "Push (Force with Lease)", push_force));
    add(git(PUBLISH, "Publish Branch...", publish));
    add(git(SYNC, "Sync", sync));
    add(git(COMMIT_PUSH, "Commit & Push", |ui, send| commit_then(ui, send, "push")));
    add(git(COMMIT_SYNC, "Commit & Sync", |ui, send| commit_then(ui, send, "sync")));
    add(git(MORE, "More Actions...", more).hidden());
    crate::menus::add(
        Menu::ViewTitle(View::Scm),
        item(MORE).title("More Actions...").icon("ellipsis").group("navigation").order(9),
    );
    register_status();
}

/// What the driver's answer to a branch, fetch, pull, push or sync says, told: an error in git's words, the
/// branch "not fully merged" asked again, a commit undone put back in the box.
fn done(ui: Ui, m: &Value, send: &Send) {
    let me = state(ui);
    let op = m["op"].as_str().unwrap_or("");
    if me.busy.with_untracked(|b| b.as_deref() == Some(op)) {
        me.busy.set(None);
    }
    let ok = m["ok"].as_bool().unwrap_or(false);
    if !ok {
        let error = m["error"].as_str().unwrap_or("git failed");
        if m["auto"].as_bool().unwrap_or(false) {
            return;
        }
        if op == "git_branch_delete" && m["unmerged"].as_bool().unwrap_or(false) {
            let name = m["key"].as_str().unwrap_or("").to_string();
            let send = send.clone();
            dialog::ask(&format!("The branch '{name}' is not fully merged. Delete anyway?"))
                .kind(dialog::Kind::Warning)
                .button("Delete Branch")
                .destructive()
                .open(ui, move |a| {
                    if a.yes() {
                        send(
                            json!({"type": "git_branch_delete", "repo": super::repos::selected(ui), "name": name, "force": true, "key": name}),
                        );
                    }
                });
            return;
        }
        toast::error(ui, error);
        return;
    }
    match op {
        "git_undo_commit" => {
            if let Some(message) = m["message"].as_str() {
                scm(ui).message.set(message.to_string());
            }
            toast::info(ui, "The last commit was undone: its changes are staged again.");
        }
        "git_push" if m["published"].is_string() => {
            toast::info(ui, &format!("Published the branch to '{}'.", m["published"].as_str().unwrap_or("")));
        }
        _ => {}
    }
}

/// Fetches every three minutes while `git.autofetch` is on, from the first status that has a repository.
fn autofetch(ui: Ui, send: &Send) {
    let me = state(ui);
    if me.fetching.get_untracked() {
        return;
    }
    me.fetching.set(true);
    fn go(ui: Ui, send: Send) {
        exec_after(AUTOFETCH, move |_| {
            let on = crate::settings::now(ui, "git.autofetch", None).as_bool().unwrap_or(false);
            if on && has_repo(ui) && state(ui).busy.with_untracked(Option::is_none) {
                network(ui, &send, json!({"type": "git_fetch", "auto": true}));
            }
            go(ui, send);
        });
    }
    go(ui, send.clone());
}

/// The driver's lists, answers and failures that are this part's, taken in; the push or sync after a commit.
/// Never taken away (false): Source Control's own part reads the same status and commits.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let me = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "git_status" if super::repos::is_selected(ui, m) && m["root"].is_string() => autofetch(ui, send),
        "git_branches" if super::repos::is_selected(ui, m) => {
            let Some((what, n)) = m["key"].as_str().and_then(|k| k.split_once(':')) else { return false };
            if n != me.asked.get_untracked().to_string() {
                return false;
            }
            if let Some(e) = m["error"].as_str() {
                toast::error(ui, e);
                return false;
            }
            let (refs, remotes) = refs_of(m);
            match what {
                "checkout" => checkout(ui, send, refs),
                "create" => create(ui, send, &refs, None),
                "from" => pick_from(ui, send, refs),
                "rename" => rename(ui, send, &refs),
                "delete" => delete(ui, send, &refs),
                "publish" => publish_to(ui, send, remotes),
                _ => {}
            }
        }
        "git_done" => done(ui, m, send),
        // A request the driver could not take (no such folder): the sync item stops turning.
        "git_failed" => {
            let of = m["of"].as_str().unwrap_or("");
            if me.busy.with_untracked(|b| b.as_deref() == Some(of)) {
                me.busy.set(None);
            }
        }
        "git_committed" => {
            let then = me.after_commit.get_untracked();
            if then.is_some() {
                me.after_commit.set(None);
            }
            match then {
                Some(_) if m["error"].is_string() => {}
                Some("push") => push(ui, send, false),
                Some(_) => sync(ui, send),
                None => {}
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_branch_name_is_checked_as_git_checks_it_and_against_the_branches_there_are() {
        assert_eq!(name_problem("", &[]).as_deref(), Some("Please provide a branch name"));
        assert_eq!(name_problem("  ", &[]).as_deref(), Some("Please provide a branch name"));
        assert_eq!(name_problem("my feature", &[]), None);
        assert_eq!(tidy(" my  new feature "), "my-new-feature");
        for bad in [
            "-x",
            "a..b",
            "a/",
            "/a",
            ".a",
            "a.lock",
            "a/b.lock/c",
            "a~1",
            "a^",
            "a:b",
            "a?",
            "a*",
            "a[",
            "a\\b",
            "@",
            "a@{1",
            "a//b",
            "a/.b",
            "a.",
        ] {
            assert_eq!(name_problem(bad, &[]), Some(format!("'{bad}' is not a valid branch name")), "{bad}");
        }
        assert_eq!(name_problem("feature/cart", &["main"]), None);
        assert_eq!(name_problem("main", &["main"]).as_deref(), Some("A branch named 'main' already exists"));
    }

    #[test]
    fn the_counts_say_both_ways_in_the_bar_and_only_what_there_is_on_the_button() {
        assert_eq!(counts(0, 1, true), "0↓ 1↑");
        assert_eq!(counts(2, 0, false), "2↓");
        assert_eq!(counts(2, 3, false), "2↓ 3↑");
    }

    #[test]
    fn checkout_lists_the_ways_to_make_a_branch_then_branches_remote_branches_and_tags() {
        let m = json!({"items": [
            {"name": "main", "kind": "local", "current": true, "hash": "abc1234", "subject": "First"},
            {"name": "origin/main", "kind": "remote", "hash": "abc1234", "subject": ""},
            {"name": "v1", "kind": "tag", "hash": "def5678", "subject": "Release"},
            {"name": "", "kind": "local"},
        ], "remotes": ["origin"]});
        let (refs, remotes) = refs_of(&m);
        assert_eq!((refs.len(), remotes), (3, vec!["origin".to_string()]));
        let rows = pick_rows(&refs, true);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["+new", "+from", "local:main", "remote:origin/main", "tag:v1"]);
        assert_eq!((rows[2].group.as_str(), rows[2].description.as_str()), ("branches", "abc1234 • First"));
        assert_eq!((rows[3].icon, rows[3].description.as_str()), ("cloud", "abc1234"));
        assert_eq!(ref_of("remote:origin/main"), Some(("remote", "origin/main")));
        assert_eq!(pick_rows(&refs, false).len(), 3);
    }

    #[test]
    fn commit_and_push_pushes_once_committed_and_an_answer_stops_the_turning_and_refills_the_box() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let ui = Ui::new();
        let sent: Rc<RefCell<Vec<Value>>> = Rc::default();
        let into = sent.clone();
        let send: Send = Rc::new(move |m| into.borrow_mut().push(m));
        super::super::register(ui, &send);
        register(ui, &send);
        let status = json!({"type": "git_status", "repo": "", "root": "/r", "branch": "dev", "upstream": "origin/dev",
            "total": 1, "files": [{"path": "a.rs", "x": " ", "y": "M", "changed": true}]});
        super::super::take(ui, &status, &send);
        take(ui, &status, &send);
        scm(ui).message.set("Fix".into());
        assert!(commands::run(ui, &send, COMMIT_PUSH));
        assert_eq!(sent.borrow().last().expect("commit")["type"], "git_commit");
        take(ui, &json!({"type": "git_committed", "repo": "", "hash": "abc"}), &send);
        assert_eq!(sent.borrow().last().expect("push")["type"], "git_push");
        assert_eq!(state(ui).busy.get_untracked().as_deref(), Some("git_push"));
        take(ui, &json!({"type": "git_done", "repo": "", "op": "git_push", "ok": true}), &send);
        assert_eq!(state(ui).busy.get_untracked(), None);
        commands::run(ui, &send, FETCH);
        assert_eq!(state(ui).busy.get_untracked().as_deref(), Some("git_fetch"));
        let skipped =
            json!({"type": "git_done", "repo": "", "op": "git_fetch", "ok": true, "auto": true, "skipped": true});
        take(ui, &skipped, &send);
        assert_eq!(state(ui).busy.get_untracked(), None, "a skipped fetch stops the turning too");
        // A commit that failed pushes nothing.
        scm(ui).message.set("Again".into());
        commands::run(ui, &send, COMMIT_PUSH);
        let n = sent.borrow().len();
        take(ui, &json!({"type": "git_committed", "repo": "", "error": "no"}), &send);
        assert_eq!(sent.borrow().len(), n);
        // An undo puts the commit's message back.
        take(
            ui,
            &json!({"type": "git_done", "repo": "", "op": "git_undo_commit", "ok": true, "message": "Fix"}),
            &send,
        );
        assert_eq!(scm(ui).message.get_untracked(), "Fix");
        // A list answered for an older ask is not opened.
        commands::run(ui, &send, CHECKOUT);
        let key = sent.borrow().last().expect("asked")["key"].as_str().unwrap_or("").to_string();
        assert!(key.starts_with("checkout:"));
        assert!(!quick::is_open(ui));
        take(ui, &json!({"type": "git_branches", "repo": "", "key": "checkout:0", "items": []}), &send);
        assert!(!quick::is_open(ui));
    }
}
