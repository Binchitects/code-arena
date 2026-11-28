//! Source Control's history: the Last Commits fold under the changes (the latest 15, each opening the files it
//! changed, each file opening its parent's text beside the commit's), and the commands Git: Open File History…
//! (the commits of the active file, followed through its renames), Git: Compare with Revision… (the file as HEAD,
//! the index, a branch or one of its last 30 commits has it, beside its editor) and Git: Open Changes.
//!
//! The commits come from the driver's `git_log`, a commit's files from `git_commit_files` (asked once a commit is
//! opened, then kept), and a revision's text from `git_show`; every request here goes with a key that starts with
//! `history.`, so its reply is told apart from the other parts'. A revision is a compare's side, `Source::Rev`
//! (`compare/mod.rs`), its text given when the compare is opened.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::CursorStyle;
use floem::views::{Decorators, empty, h_stack, label, tooltip, v_stack};
use floem::IntoView;
use serde_json::{Value, json};

use crate::compare::{self, Source};
use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::tree as wt;
use crate::widgets::{quick, toast};

/// How many commits the fold lists, how many Compare with Revision offers, how many a file's history lists.
const LAST: u64 = 15;
const REVISIONS: u64 = 30;
const FILE_HISTORY: u64 = 100;

/// What every key this part asks with begins with.
const KEY: &str = "history.";

/// A commit as `git_log` lists it; of a file's history, also the file's letter, its name then and before.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub at: u64,
    pub status: Option<char>,
    pub path_at: Option<String>,
    pub from: Option<String>,
}

impl Commit {
    fn of(v: &Value) -> Option<Commit> {
        let s = |k: &str| v[k].as_str().map(str::to_string);
        Some(Commit {
            hash: s("hash")?,
            short: s("short").unwrap_or_default(),
            subject: s("subject").unwrap_or_default(),
            author: s("author").unwrap_or_default(),
            at: v["at"].as_u64().unwrap_or(0),
            status: v["status"].as_str().and_then(|l| l.chars().next()),
            path_at: s("path_at"),
            from: s("from"),
        })
    }

    /// Its tooltip, as VS Code's: the hash, the message, who and when.
    pub fn tip(&self, now: u64) -> String {
        format!("{}\n{}\n\n{}, {}", self.hash, self.subject, self.author, ago(now, self.at))
    }
}

/// The commits of a `git_log` reply.
pub fn commits_of(m: &Value) -> Vec<Commit> {
    m["items"].as_array().map(|a| a.iter().filter_map(Commit::of).collect()).unwrap_or_default()
}

/// A file a commit changed: its path, git's letter, where it was (renamed).
#[derive(Clone, Debug, PartialEq)]
pub struct Changed {
    pub path: String,
    pub status: char,
    pub from: Option<String>,
}

/// A commit's files, as `git_commit_files` says them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Files {
    pub root: bool,
    pub files: Vec<Changed>,
    pub truncated: bool,
}

pub fn files_of(m: &Value) -> Files {
    let files = m["files"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    Some(Changed {
                        path: f["path"].as_str()?.to_string(),
                        status: f["status"].as_str().and_then(|l| l.chars().next()).unwrap_or('M'),
                        from: f["from"].as_str().map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Files {
        root: m["parents"].as_array().is_some_and(Vec::is_empty),
        files,
        truncated: m["truncated"].as_bool().unwrap_or(false),
    }
}

/// What a commit's file opens: the parent's text (where the file was before a rename) beside the commit's; an added
/// file's parent side, a deleted file's commit side, empty.
pub fn sides_of(hash: &str, root: bool, f: &Changed) -> (Source, Source) {
    let before = if root || matches!(f.status, 'A' | 'C') {
        Source::Empty
    } else {
        Source::Rev(format!("{hash}^"), f.from.clone().unwrap_or_else(|| f.path.clone()))
    };
    let after = if f.status == 'D' { Source::Empty } else { Source::Rev(hash.to_string(), f.path.clone()) };
    (before, after)
}

/// How long ago, as VS Code says it ("now", "5 mins ago", "1 hr ago", "3 days ago", "2 wks ago", "4 mos ago",
/// "1 yr ago").
pub fn ago(now: u64, at: u64) -> String {
    let s = now.saturating_sub(at);
    let (n, one, many) = match s {
        0..60 => return "now".into(),
        60..3600 => (s / 60, "min", "mins"),
        3600..86_400 => (s / 3600, "hr", "hrs"),
        86_400..604_800 => (s / 86_400, "day", "days"),
        604_800..2_592_000 => (s / 604_800, "wk", "wks"),
        2_592_000..31_536_000 => (s / 2_592_000, "mo", "mos"),
        _ => (s / 31_536_000, "yr", "yrs"),
    };
    format!("{n} {} ago", if n == 1 { one } else { many })
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// git's letter's colour; a copy is an added file.
fn colour_of(c: &crate::colors::Colors, letter: char) -> floem::peniko::Color {
    super::letter_colour(c, if letter == 'C' { 'A' } else { letter })
}

/// A row of the fold: a commit (`c:<hash>`), one of its files (`f:<hash>:<path>`), or the note that it changed
/// more files than are listed (`t:<hash>`).
enum Row<'a> {
    Commit(&'a str),
    File(&'a str, &'a str),
    More,
}

fn row_of(id: &str) -> Option<Row<'_>> {
    let (kind, rest) = id.split_once(':')?;
    match kind {
        "c" => Some(Row::Commit(rest)),
        "f" => rest.split_once(':').map(|(h, p)| Row::File(h, p)),
        "t" => Some(Row::More),
        _ => None,
    }
}

/// The fold's rows: each commit, and under one opened its files once they have come.
pub fn rows(commits: &[Commit], open: &BTreeSet<String>, files: &HashMap<String, Rc<Files>>) -> im::Vector<wt::Node> {
    let mut out = im::Vector::new();
    for c in commits {
        let opened = open.contains(&c.hash);
        out.push_back(wt::folder(&format!("c:{}", c.hash), 0, opened));
        let Some(f) = files.get(&c.hash).filter(|_| opened) else { continue };
        for file in &f.files {
            out.push_back(wt::leaf(&format!("f:{}:{}", c.hash, file.path), 1));
        }
        if f.truncated {
            out.push_back(wt::leaf(&format!("t:{}", c.hash), 1));
        }
    }
    out
}

/// A compare of revisions waiting for their texts.
#[derive(Clone)]
struct Opening {
    left: Source,
    right: Source,
    texts: (Option<String>, Option<String>),
}

/// The part's state, one per window.
#[derive(Clone, Copy)]
pub struct History {
    /// Whether the fold is open.
    open: RwSignal<bool>,
    commits: RwSignal<Rc<Vec<Commit>>>,
    /// The commits opened, by hash, and each commit's files once they have come.
    unfolded: RwSignal<BTreeSet<String>>,
    files: RwSignal<HashMap<String, Rc<Files>>>,
    tree: wt::State,
    /// The last pick or compare asked for (an older one's replies are dropped), and the compare waiting.
    asked: RwSignal<u64>,
    opening: RwSignal<Option<(u64, Opening)>>,
    /// Compare with Revision's file and its commits, while its branches are asked.
    revisions: RwSignal<Option<(String, Vec<Commit>)>>,
}

impl crate::ext::State for History {
    fn new(ui: Ui) -> History {
        History {
            open: ui.scope.create_rw_signal(true),
            commits: ui.scope.create_rw_signal(Rc::new(Vec::new())),
            unfolded: ui.scope.create_rw_signal(BTreeSet::new()),
            files: ui.scope.create_rw_signal(HashMap::new()),
            tree: wt::State::new(ui),
            asked: ui.scope.create_rw_signal(0),
            opening: ui.scope.create_rw_signal(None),
            revisions: ui.scope.create_rw_signal(None),
        }
    }
}

fn state(ui: Ui) -> History {
    ui.state::<History>()
}

/// A new number for a request whose reply only the last one asked is taken.
fn next(me: History) -> u64 {
    let n = me.asked.get_untracked() + 1;
    me.asked.set(n);
    n
}

/// The fold's commits asked again (only while it is open: shut, it shows none).
fn ask_last(ui: Ui, send: &Send) {
    if state(ui).open.get_untracked() && super::has_repo(ui) {
        send(json!({"type": "git_log", "repo": "", "limit": LAST, "key": format!("{KEY}last")}));
    }
}

/// A commit opened or shut in the fold; its files asked the first time.
fn toggle(ui: Ui, send: &Send, hash: &str) {
    let me = state(ui);
    let open = me.unfolded.with_untracked(|u| u.contains(hash));
    me.unfolded.update(|u| {
        if open {
            u.remove(hash);
        } else {
            u.insert(hash.to_string());
        }
    });
    if !open && me.files.with_untracked(|f| !f.contains_key(hash)) {
        send(json!({"type": "git_commit_files", "repo": "", "hash": hash, "key": format!("{KEY}files")}));
    }
}

/// Two sides compared, a revision's text asked of the driver first (the compare opens once all have come).
pub fn open_revs(ui: Ui, send: &Send, left: Source, right: Source) {
    let me = state(ui);
    if ui.tab(&compare::path_of(&left, &right)).is_some() {
        compare::open(ui, send, left, right);
        return;
    }
    let n = next(me);
    let text = |s: &Source| (!matches!(s, Source::Rev(..))).then(|| (matches!(s, Source::Empty)).then(String::new));
    let texts = (text(&left).flatten(), text(&right).flatten());
    for (i, s) in [&left, &right].into_iter().enumerate() {
        if let Source::Rev(rev, path) = s {
            send(json!({"type": "git_show", "repo": "", "path": path, "rev": rev, "working": false,
                "key": format!("{KEY}show.{n}.{i}")}));
        }
    }
    let o = Opening { left, right, texts };
    me.opening.set(Some((n, o.clone())));
    finish(ui, send, n);
}

/// A waiting compare opened once every revision's text has come.
fn finish(ui: Ui, send: &Send, n: u64) {
    let me = state(ui);
    let Some((at, o)) = me.opening.get_untracked() else { return };
    let ready = |s: &Source, t: &Option<String>| !matches!(s, Source::Rev(..)) || t.is_some();
    if at != n || !ready(&o.left, &o.texts.0) || !ready(&o.right, &o.texts.1) {
        return;
    }
    me.opening.set(None);
    let texts = (
        o.texts.0.filter(|_| matches!(o.left, Source::Rev(..) | Source::Empty)),
        o.texts.1.filter(|_| matches!(o.right, Source::Rev(..) | Source::Empty)),
    );
    compare::open_with(ui, send, o.left, o.right, texts);
}

/// The active editor's file, when it is one.
fn active_file(ui: Ui) -> Option<String> {
    let p = ui.active.get_untracked()?;
    ui.tab(&p).filter(|t| t.is_file()).map(|t| t.path.clone())
}

fn with_file(ui: Ui) -> bool {
    super::has_repo(ui) && active_file(ui).is_some()
}

/// A commit as a pick's row: its message, its short hash, who and when.
fn pick_row(c: &Commit, now: u64) -> quick::Item {
    quick::item(&c.hash, &c.subject)
        .icon("git-commit")
        .description(&format!("{} · {} · {}", c.short, c.author, ago(now, c.at)))
}

/// A file's commits picked from; the one picked opens its parent's text beside its own.
fn file_history(ui: Ui, send: &Send, path: &str, commits: Vec<Commit>) {
    if commits.is_empty() {
        toast::info(ui, &format!("{} has no commits", path.rsplit('/').next().unwrap_or(path)));
        return;
    }
    let t = now();
    let rows = commits.iter().map(|c| pick_row(c, t)).collect();
    let (send, path) = (send.clone(), path.to_string());
    quick::pick("Open File History").placeholder("Select a commit to open its changes").items(rows).open(
        ui,
        move |item| {
            let Some(c) = item.and_then(|i| commits.into_iter().find(|c| c.hash == i.id)) else { return };
            let f = Changed {
                path: c.path_at.clone().unwrap_or(path),
                status: c.status.unwrap_or('M'),
                from: c.from.clone(),
            };
            let (l, r) = sides_of(&c.hash, false, &f);
            open_revs(ui, &send, l, r);
        },
    );
}

/// The revisions a file is compared with: HEAD, the index, each branch and tag, its last commits.
fn revision_rows(refs: &[super::branches::Ref], commits: &[Commit], now: u64) -> Vec<quick::Item> {
    use super::branches::Kind;
    let mut rows = vec![
        quick::item("HEAD", "HEAD").icon("git-commit").description("Last commit"),
        quick::item("", "Index").icon("git-commit").description("Staged changes"),
    ];
    for r in refs {
        let (glyph, group) = match r.kind {
            Kind::Local => ("git-branch", "branches"),
            Kind::Remote => ("cloud", "remote branches"),
            Kind::Tag => ("tag", "tags"),
        };
        let short: String = r.hash.chars().take(8).collect();
        rows.push(quick::item(&format!("ref:{}", r.name), &r.name).icon(glyph).group(group).description(&short));
    }
    for c in commits {
        rows.push(pick_row(c, now).group("commits"));
    }
    rows
}

/// Compare with Revision's pick, once its file's commits and the branches have come.
fn pick_revision(ui: Ui, send: &Send, path: String, commits: Vec<Commit>, refs: Vec<super::branches::Ref>) {
    let rows = revision_rows(&refs, &commits, now());
    let send = send.clone();
    quick::pick("Compare with Revision").placeholder("Select a revision to compare the file with").items(rows).open(
        ui,
        move |item| {
            let Some(item) = item else { return };
            let rev = match item.id.strip_prefix("ref:") {
                Some(name) => Source::Rev(name.to_string(), path.clone()),
                None => match commits.iter().find(|c| c.hash == item.id) {
                    Some(c) => Source::Rev(c.hash.clone(), c.path_at.clone().unwrap_or_else(|| path.clone())),
                    None => Source::Rev(item.id.clone(), path.clone()),
                },
            };
            open_revs(ui, &send, rev, Source::Live(path));
        },
    );
}

/// Open Changes: the active file's changes tab (the working tree's, else the index's), as the view opens them.
fn open_changes(ui: Ui) {
    let Some(path) = active_file(ui) else { return };
    let f = super::state(ui).status.with_untracked(|s| {
        s.as_ref().and_then(|s| s.files.iter().find(|f| f.path == path).cloned())
    });
    match f {
        Some(f) if f.changed => crate::editor::kinds::open(ui, &format!("{}:w:{path}", super::KIND)),
        Some(f) if f.staged => crate::editor::kinds::open(ui, &format!("{}:i:{path}", super::KIND)),
        Some(f) if f.untracked => {
            toast::info(ui, &format!("{} is not tracked by git", name_of(&path)));
        }
        _ => {
            toast::info(ui, &format!("{} has no changes", name_of(&path)));
        }
    }
}

fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Its commands.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    add(command("git.openFileHistory", "Git", "Open File History...", |ui, send| {
        let Some(path) = active_file(ui) else { return };
        let n = next(state(ui));
        send(json!({"type": "git_log", "repo": "", "path": path, "limit": FILE_HISTORY, "key": format!("{KEY}file.{n}")}));
    })
    .when(with_file));
    add(command("git.compareWithRevision", "Git", "Compare with Revision...", |ui, send| {
        let Some(path) = active_file(ui) else { return };
        let n = next(state(ui));
        send(json!({"type": "git_log", "repo": "", "path": path, "limit": REVISIONS, "key": format!("{KEY}rev.{n}")}));
    })
    .when(with_file));
    add(command("git.openChange", "Git", "Open Changes", |ui, _| open_changes(ui)).when(with_file));
}

/// What a key of this part asks (`file` of `history.file.3`) and the number after it (`3`; none: empty).
fn key_of(m: &Value) -> Option<(&str, &str)> {
    let rest = m["key"].as_str()?.strip_prefix(KEY)?;
    Some(rest.split_once('.').unwrap_or((rest, "")))
}

/// The driver's replies to this part's requests (taken away), and the fold's commits asked again when the
/// repository may have changed.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let me = state(ui);
    let of = m["type"].as_str().unwrap_or("");
    if matches!(of, "git_status" | "git_committed" | "git_done") && m["repo"].as_str().unwrap_or("").is_empty() {
        ask_last(ui, send);
        return false;
    }
    if !matches!(of, "git_log" | "git_commit_files" | "git_shown" | "git_branches") {
        return false;
    }
    let Some((what, n)) = key_of(m) else { return false };
    let current = |n: &str| n == me.asked.get_untracked().to_string();
    match (of, what) {
        ("git_log", "last") => {
            let commits = commits_of(m);
            if me.commits.with_untracked(|c| **c != commits) {
                me.commits.set(Rc::new(commits));
            }
        }
        ("git_commit_files", _) => {
            let Some(hash) = m["hash"].as_str() else { return true };
            if let Some(e) = m["error"].as_str() {
                toast::error(ui, e);
                return true;
            }
            // Asked by a commit's short or long hash: kept under the one the fold knows it by.
            let known = me.commits.with_untracked(|c| {
                c.iter().find(|c| c.hash == hash || hash.starts_with(&c.hash)).map(|c| c.hash.clone())
            });
            me.files.update(|f| {
                f.insert(known.unwrap_or_else(|| hash.to_string()), Rc::new(files_of(m)));
            });
        }
        ("git_log", "file") if current(n) => match m["error"].as_str() {
            Some(e) => {
                toast::error(ui, e);
            }
            None => file_history(ui, send, m["path"].as_str().unwrap_or(""), commits_of(m)),
        },
        ("git_log", "rev") if current(n) => match m["error"].as_str() {
            Some(e) => {
                toast::error(ui, e);
            }
            None => {
                me.revisions.set(Some((m["path"].as_str().unwrap_or("").to_string(), commits_of(m))));
                send(json!({"type": "git_branches", "repo": "", "key": format!("{KEY}refs.{n}")}));
            }
        },
        ("git_branches", "refs") if current(n) => {
            let Some((path, commits)) = me.revisions.get_untracked() else { return true };
            me.revisions.set(None);
            let (refs, _) = super::branches::refs_of(m);
            pick_revision(ui, send, path, commits, refs);
        }
        ("git_shown", "show") => {
            let Some((at, side)) = n.split_once('.') else { return true };
            let Ok(at) = at.parse::<u64>() else { return true };
            let text = if m["binary"].as_bool() == Some(true) || m["too_large"].as_bool() == Some(true) {
                toast::info(ui, "The file is not shown because it is binary or too large.");
                String::new()
            } else {
                m["text"].as_str().unwrap_or("").to_string()
            };
            me.opening.update(|o| {
                if let Some((_, o)) = o.as_mut().filter(|(n, _)| *n == at) {
                    if side == "0" {
                        o.texts.0 = Some(text);
                    } else {
                        o.texts.1 = Some(text);
                    }
                }
            });
            finish(ui, send, at);
        }
        _ => {}
    }
    true
}

/// The fold under the changes: its title (LAST COMMITS, opened and shut with a click) and its commits.
pub fn view(ui: Ui, send: Send) -> floem::AnyView {
    let t = ui.colors;
    let me = state(ui);
    let shown = create_memo(move |_| super::has_repo(ui));
    let open = create_memo(move |_| me.open.get());
    let rows_now = create_memo(move |_| {
        me.commits.with(|c| me.unfolded.with(|u| me.files.with(|f| rows(c, u, f))))
    });
    let height = create_memo(move |_| rows_now.with(|r| r.len().max(1)) as f64 * wt::ROW);
    let asking = send.clone();
    let title = h_stack((
        icon("chevron-down", 16.0).style(move |s| s.apply_if(!open.get(), |s| s.hide())),
        icon("chevron-right", 16.0).style(move |s| s.apply_if(open.get(), |s| s.hide())),
        label(|| "LAST COMMITS").style(|s| s.font_size(11.0).font_bold()),
    ))
    .style(move |s| {
        let c = t.get();
        s.width_full()
            .height(22.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .gap(2.0)
            .padding_left(2.0)
            .color(c.section_fg)
            .background(c.section_bg)
            .border_top(1.0)
            .border_color(c.section_border)
            .cursor(CursorStyle::Pointer)
    })
    .on_click_stop(move |_| {
        let now_open = !me.open.get_untracked();
        me.open.set(now_open);
        if now_open {
            ask_last(ui, &asking);
        }
    });
    let (toggling, opening) = (send.clone(), send);
    let list = wt::tree(ui, me.tree, move || rows_now.get())
        .content(move |n| row_content(ui, n))
        .decoration(move |n| row_decoration(ui, n))
        .on_toggle(move |id| {
            if let Some(Row::Commit(hash)) = row_of(id) {
                toggle(ui, &toggling, hash);
            }
        })
        .on_open(move |id| match row_of(id) {
            Some(Row::Commit(hash)) => toggle(ui, &opening, hash),
            Some(Row::File(hash, path)) => {
                let file = me.files.with_untracked(|f| {
                    f.get(hash).and_then(|f| f.files.iter().find(|x| x.path == path).cloned().map(|x| (f.root, x)))
                });
                if let Some((root, f)) = file {
                    let (l, r) = sides_of(hash, root, &f);
                    open_revs(ui, &opening, l, r);
                }
            }
            _ => {}
        })
        .view()
        .style(move |s| {
            s.width_full().height(height.get()).min_height(0.0).flex_shrink(1.0_f32).apply_if(!open.get(), |s| s.hide())
        });
    v_stack((title, list))
        .style(move |s| {
            s.width_full().flex_shrink(0.0_f32).max_height_pct(45.0).min_height(0.0).apply_if(!shown.get(), |s| s.hide())
        })
        .into_any()
}

/// A row: a commit's short hash and message (its tooltip the whole of it), a file's icon, name and folder.
fn row_content(ui: Ui, node: &wt::Node) -> floem::AnyView {
    let t = ui.colors;
    let me = state(ui);
    match row_of(&node.id) {
        Some(Row::Commit(hash)) => {
            let Some(c) = me.commits.with_untracked(|cs| cs.iter().find(|c| c.hash == hash).cloned()) else {
                return empty().into_any();
            };
            let (short, subject, tip) = (c.short.clone(), c.subject.clone(), c.tip(now()));
            let row = h_stack((
                icon("git-commit", 16.0).style(move |s| s.color(t.get().icon).flex_shrink(0.0_f32)),
                label(move || subject.clone()).style(|s| s.text_ellipsis().min_width(0.0).flex_shrink(1.0_f32)),
                label(move || short.clone())
                    .style(move |s| s.font_size(12.0).color(t.get().desc).flex_shrink(0.0_f32)),
            ))
            .style(|s| s.width_full().min_width(0.0).items_center().gap(6.0));
            tooltip(row, move || {
                let tip = tip.clone();
                label(move || tip.clone())
            })
            .style(|s| s.width_full().min_width(0.0))
            .into_any()
        }
        Some(Row::File(hash, path)) => {
            let f = me.files.with_untracked(|f| f.get(hash).and_then(|f| f.files.iter().find(|x| x.path == path).cloned()));
            let Some(f) = f else { return empty().into_any() };
            let letter = f.status;
            let name = name_of(&f.path).to_string();
            let dir = match (&f.from, f.path.rsplit_once('/')) {
                (Some(from), _) => format!("{from} →"),
                (None, Some((d, _))) => d.to_string(),
                (None, None) => String::new(),
            };
            h_stack((
                icon("file", 16.0).style(move |s| s.color(t.get().icon).flex_shrink(0.0_f32)),
                label(move || name.clone()).style(move |s| s.color(colour_of(&t.get(), letter)).flex_shrink(0.0_f32)),
                label(move || dir.clone()).style(move |s| {
                    s.font_size(12.0).color(t.get().desc).text_ellipsis().min_width(0.0).flex_shrink(1.0_f32)
                }),
            ))
            .style(|s| s.width_full().min_width(0.0).items_center().gap(6.0))
            .into_any()
        }
        Some(Row::More) => label(|| "Only the first 2000 files are shown.")
            .style(move |s| s.color(t.get().desc).text_ellipsis().min_width(0.0))
            .into_any(),
        None => empty().into_any(),
    }
}

/// A file's letter at its row's right, as the changes'.
fn row_decoration(ui: Ui, node: &wt::Node) -> floem::AnyView {
    let t = ui.colors;
    let Some(Row::File(hash, path)) = row_of(&node.id) else { return empty().into_any() };
    let letter = state(ui).files.with_untracked(|f| {
        f.get(hash).and_then(|f| f.files.iter().find(|x| x.path == path).map(|x| x.status))
    });
    let Some(letter) = letter else { return empty().into_any() };
    label(move || letter.to_string())
        .style(move |s| s.font_size(12.0).width(12.0).margin_right(10.0).color(colour_of(&t.get(), letter)))
        .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::views::editor::text::Document;

    fn commit(hash: &str, subject: &str) -> Value {
        json!({"hash": hash, "short": &hash[..3], "subject": subject, "author": "Ann", "at": 1000})
    }

    #[test]
    fn a_commits_file_opens_its_parent_beside_it_with_an_empty_side_for_an_added_or_deleted_one() {
        let f = |path: &str, status: char, from: Option<&str>| Changed { path: path.into(), status, from: from.map(Into::into) };
        assert_eq!(
            sides_of("abc", false, &f("a.rs", 'M', None)),
            (Source::Rev("abc^".into(), "a.rs".into()), Source::Rev("abc".into(), "a.rs".into()))
        );
        assert_eq!(
            sides_of("abc", false, &f("new.rs", 'R', Some("old.rs"))).0,
            Source::Rev("abc^".into(), "old.rs".into()),
            "a renamed file's parent side is where it was"
        );
        assert_eq!(sides_of("abc", false, &f("a.rs", 'A', None)).0, Source::Empty);
        assert_eq!(sides_of("abc", true, &f("a.rs", 'M', None)).0, Source::Empty, "a first commit has no parent");
        assert_eq!(sides_of("abc", false, &f("a.rs", 'D', None)).1, Source::Empty);
    }

    #[test]
    fn the_fold_lists_the_commits_and_an_opened_ones_files_once_they_have_come() {
        let m = json!({"type": "git_log", "items": [commit("aaa1", "One"), commit("bbb2", "Two")]});
        let commits = commits_of(&m);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].tip(1000 + 7200), "aaa1\nOne\n\nAnn, 2 hrs ago");
        let mut open = BTreeSet::new();
        let mut files = HashMap::new();
        let ids = |r: im::Vector<wt::Node>| r.iter().map(|n| n.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(rows(&commits, &open, &files)), ["c:aaa1", "c:bbb2"]);
        open.insert("bbb2".to_string());
        assert_eq!(ids(rows(&commits, &open, &files)), ["c:aaa1", "c:bbb2"], "nothing under it until its files come");
        let reply = json!({"hash": "bbb2", "parents": ["aaa1"], "truncated": true,
            "files": [{"path": "src/a.rs", "status": "M", "from": null}, {"path": "b.rs", "status": "R", "from": "c.rs"}]});
        let f = files_of(&reply);
        assert!(!f.root && f.truncated);
        assert_eq!(f.files[1], Changed { path: "b.rs".into(), status: 'R', from: Some("c.rs".into()) });
        files.insert("bbb2".to_string(), Rc::new(f));
        let r = rows(&commits, &open, &files);
        assert_eq!(ids(r.clone()), ["c:aaa1", "c:bbb2", "f:bbb2:src/a.rs", "f:bbb2:b.rs", "t:bbb2"]);
        assert_eq!(r[1].fold, Some(true));
        assert!(matches!(row_of("f:bbb2:src/a.rs"), Some(Row::File("bbb2", "src/a.rs"))));
        assert!(files_of(&json!({"parents": [], "files": []})).root);
    }

    #[test]
    fn how_long_ago_is_said_as_vs_code_says_it() {
        assert_eq!(ago(100, 90), "now");
        assert_eq!(ago(10_000, 10_000 - 60), "1 min ago");
        assert_eq!(ago(10_000, 10_000 - 300), "5 mins ago");
        assert_eq!(ago(1_000_000, 1_000_000 - 3 * 86_400), "3 days ago");
        assert_eq!(ago(100_000_000, 100_000_000 - 2 * 604_800), "2 wks ago");
        assert_eq!(ago(100_000_000, 100_000_000 - 40_000_000), "1 yr ago");
        assert_eq!(ago(5, 10), "now", "a commit from the future is now");
    }

    #[test]
    fn compare_with_revision_offers_head_the_index_the_branches_and_the_files_commits() {
        use super::super::branches::{Kind, Ref};
        let refs = vec![
            Ref { name: "main".into(), kind: Kind::Local, hash: "0123456789".into(), ..Ref::default() },
            Ref { name: "v1".into(), kind: Kind::Tag, ..Ref::default() },
        ];
        let commits = commits_of(&json!({"items": [commit("abc9", "Fix")]}));
        let rows = revision_rows(&refs, &commits, 1000);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["HEAD", "", "ref:main", "ref:v1", "abc9"]);
        assert_eq!(rows[1].label, "Index");
        assert_eq!(rows[2].description, "01234567");
        assert_eq!(rows[4].description, "abc · Ann · now");
    }

    #[test]
    fn a_revisions_texts_open_the_compare_once_both_have_come() {
        let ui = Ui::new();
        let sent = Rc::new(std::cell::RefCell::new(Vec::<Value>::new()));
        let kept = sent.clone();
        let send: Send = Rc::new(move |v: Value| kept.borrow_mut().push(v));
        crate::compare::register(ui, &send);
        let (l, r) = (Source::Rev("abc^".into(), "a.rs".into()), Source::Rev("abc".into(), "a.rs".into()));
        open_revs(ui, &send, l.clone(), r.clone());
        let keys: Vec<String> =
            sent.borrow().iter().map(|m| format!("{} {} {}", m["type"], m["rev"], m["key"])).collect();
        assert_eq!(keys, [r#""git_show" "abc^" "history.show.1.0""#, r#""git_show" "abc" "history.show.1.1""#]);
        let path = compare::path_of(&l, &r);
        let shown = |side: &str, rev: &str, text: &str| {
            json!({"type": "git_shown", "repo": "", "path": "a.rs", "rev": rev, "working": false, "exists": true,
                "text": text, "key": format!("history.show.1.{side}")})
        };
        assert!(take(ui, &shown("0", "abc^", "one\n"), &send), "the history's text is its own");
        assert!(ui.tab(&path).is_none(), "not opened with one side still to come");
        take(ui, &shown("1", "abc", "one\ntwo\n"), &send);
        assert!(ui.tab(&path).is_some(), "opened once both have come");
        let e = compare::entry(&path).expect("the compare");
        assert_eq!(e.left.doc.text().to_string(), "one\n");
        assert_eq!(e.right.doc.text().to_string(), "one\ntwo\n");
        assert!(!e.right.editable);
    }
}
