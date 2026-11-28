//! File: Open Folder… (Ctrl+K Ctrl+O) and Open Recent… (Ctrl+R), as VS Code's simple folder dialog and its recent
//! pick. What every part has (`register`, `take`) is said in `ext.rs`.
//!
//! Open Folder is a quick pick over one folder: "Open <folder>" first, ".." for the one above, its folders (Enter
//! goes in, the folder is listed by the session: `folders{path, files: false, key}`), then the folders opened
//! before; a whole path typed (`/`, `~/`, `C:\`) is offered as "Go to <path>". Open Recent lists the folders opened
//! before (`recent_folders{key}`), the one worked in left out, ten at first and "More…" for the rest.
//!
//! Opening asks about the tabs with changes not saved (Save All, Don't Save), keeps this folder's layout, covers the
//! window with "Opening <folder>…" and sends `folder_open{path, key}`. The session begins a new session there and
//! sends its `state`: when that names another folder, the tabs, the groups, the Explorer's tree and the terminals
//! are let go of before the window takes it in, and the folder's own layout is restored from it. A
//! `folder_opened{error}` says why nothing changed.

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use serde_json::{Value, json};

use crate::state::{Send, Ui};
use crate::widgets::{dialog, quick, toast};

/// The commands, VS Code's ids.
pub const OPEN_FOLDER: &str = "workbench.action.files.openFolder";
pub const OPEN_RECENT: &str = "workbench.action.openRecent";
pub const CLEAR_RECENT: &str = "workbench.action.clearRecentFiles";

/// The key the dialog's listings and the recent folders are asked with.
const KEY: &str = "open_folder";
/// The key Open Recent asks with: its reply opens the pick.
const RECENT_KEY: &str = "open_recent";
/// How many recent folders Open Recent lists before "More…".
const FIRST: usize = 10;

#[derive(Clone, Copy)]
struct OpenFolder {
    /// The folder the dialog asked to be listed (empty: none asked).
    asked: RwSignal<String>,
    /// The last folder that could be listed: the dialog goes back to it when another cannot be.
    good: RwSignal<String>,
    /// The folders opened before, the latest first.
    recent: RwSignal<Vec<String>>,
}

impl crate::ext::State for OpenFolder {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        OpenFolder {
            asked: s.create_rw_signal(String::new()),
            good: s.create_rw_signal(String::new()),
            recent: s.create_rw_signal(Vec::new()),
        }
    }
}

fn state(ui: Ui) -> OpenFolder {
    ui.state::<OpenFolder>()
}

/// This computer's home folder, for `~` in what is shown.
fn home() -> Option<String> {
    std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).ok().filter(|h| !h.is_empty())
}

/// A path as shown: the home folder as `~`.
pub fn tilde(path: &str, home: Option<&str>) -> String {
    match home.map(|h| h.trim_end_matches(['/', '\\'])) {
        Some(h) if !h.is_empty() && path == h => "~".into(),
        Some(h) if !h.is_empty() && path.starts_with(h) && path[h.len()..].starts_with(['/', '\\']) => {
            format!("~{}", &path[h.len()..])
        }
        _ => path.to_string(),
    }
}

/// A folder's own name (the path itself for the top).
fn name_of(path: &str) -> &str {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or(path)
}

/// A folder within another, as a path.
fn join(dir: &str, name: &str) -> String {
    if dir.ends_with(['/', '\\']) { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

/// Whether what is typed is a whole path to go to: `/…`, `~`, `~/…`, `C:\…`, `C:/…`.
pub fn is_path(typed: &str) -> bool {
    let b = typed.as_bytes();
    typed.starts_with('/')
        || typed == "~"
        || typed.starts_with("~/")
        || typed.starts_with("~\\")
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'\\' || b[2] == b'/'))
}

/// The dialog's rows for a folder listed: Open it, `..`, its folders, then the recent folders not already there.
pub fn rows(dir: &str, parent: Option<&str>, folders: &[String], recent: &[String], home: Option<&str>) -> Vec<quick::Item> {
    let mut rows = vec![
        quick::item(&format!("open:{dir}"), &format!("Open {}", name_of(dir)))
            .description(&tilde(dir, home))
            .icon("folder-opened"),
    ];
    if let Some(p) = parent {
        rows.push(quick::item(&format!("go:{p}"), "..").icon("folder"));
    }
    for name in folders {
        rows.push(quick::item(&format!("go:{}", join(dir, name)), name).icon("folder"));
    }
    for f in recent.iter().filter(|f| f.as_str() != dir) {
        rows.push(
            quick::item(&format!("go:{f}"), name_of(f))
                .description(&tilde(f, home))
                .icon("history")
                .group("recently opened"),
        );
    }
    rows
}

/// Open Recent's rows: the folders opened before but the one worked in, `FIRST` of them unless `all`, "More…"
/// for the rest.
pub fn recent_rows(recent: &[String], current: &str, all: bool, home: Option<&str>) -> Vec<quick::Item> {
    let others: Vec<&String> = recent.iter().filter(|f| f.as_str() != current).collect();
    let shown = if all { others.len() } else { others.len().min(FIRST) };
    let mut rows: Vec<quick::Item> = others[..shown]
        .iter()
        .map(|f| quick::item(&format!("open:{f}"), name_of(f)).description(&tilde(f, home)).icon("folder").group("folders"))
        .collect();
    if shown < others.len() {
        rows.push(quick::item("more", "More...").icon("ellipsis"));
    }
    rows
}

/// Open Folder…: the folder worked in listed, then the dialog over it.
pub fn open_folder(ui: Ui, send: &Send) {
    if busy(ui) {
        return;
    }
    send(json!({"type": "recent_folders", "key": KEY}));
    let start = ui.folder.get_untracked();
    go(ui, send, if start.is_empty() { home().unwrap_or_else(|| "/".into()) } else { start });
}

/// A folder asked to be listed for the dialog.
fn go(ui: Ui, send: &Send, dir: String) {
    state(ui).asked.set(dir.clone());
    send(json!({"type": "folders", "path": dir, "files": false, "key": KEY}));
}

/// The dialog over a folder the session listed.
fn dialog_over(ui: Ui, send: &Send, m: &Value) {
    let st = state(ui);
    let dir = m["dir"].as_str().unwrap_or("").to_string();
    if let Some(e) = m["error"].as_str() {
        toast::error(ui, &format!("{}: {e}", tilde(&dir, home().as_deref())));
        let good = st.good.get_untracked();
        if !good.is_empty() && good != dir && st.asked.get_untracked() != good {
            go(ui, send, good);
        }
        return;
    }
    st.good.set(dir.clone());
    let folders: Vec<String> = m["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["dir"] == true)
        .filter_map(|e| e["name"].as_str().map(str::to_string))
        .collect();
    let h = home();
    let items = st.recent.with_untracked(|r| rows(&dir, m["parent"].as_str(), &folders, r, h.as_deref()));
    let send = send.clone();
    quick::pick("Open Folder")
        .placeholder(&format!("{}: type to narrow, or a whole path", tilde(&dir, h.as_deref())))
        .items(items)
        .free(|typed| is_path(typed).then(|| quick::item(&format!("go:{typed}"), &format!("Go to {typed}")).icon("arrow-right")))
        .most(500)
        .open(ui, move |item| {
            let Some(item) = item else { return };
            if let Some(path) = item.id.strip_prefix("open:") {
                open(ui, &send, path.to_string());
            } else if let Some(path) = item.id.strip_prefix("go:") {
                go(ui, &send, path.to_string());
            }
        });
}

/// Open Recent…: the recent folders asked for; the pick opens when they come.
pub fn open_recent(ui: Ui, send: &Send) {
    if !busy(ui) {
        send(json!({"type": "recent_folders", "key": RECENT_KEY}));
    }
}

fn recent_pick(ui: Ui, send: &Send, all: bool) {
    let h = home();
    let items = state(ui).recent.with_untracked(|r| recent_rows(r, &ui.folder.get_untracked(), all, h.as_deref()));
    if items.is_empty() {
        toast::info(ui, "No folder was opened before this one.");
        return;
    }
    let send = send.clone();
    quick::pick("").placeholder("Select to open").items(items).open(ui, move |item| match item {
        Some(item) if item.id == "more" => recent_pick(ui, &send, true),
        Some(item) => {
            if let Some(path) = item.id.strip_prefix("open:") {
                open(ui, &send, path.to_string());
            }
        }
        None => {}
    });
}

/// Nothing is opened while an answer is written: said, and true.
fn busy(ui: Ui) -> bool {
    let running = ui.running.get_untracked();
    if running {
        toast::info(ui, "Wait for the answer to end, or stop it, to open another folder.");
    }
    running
}

/// A folder opened in this window, once the tabs with changes not saved are answered for.
fn open(ui: Ui, send: &Send, path: String) {
    if busy(ui) {
        return;
    }
    let dirty: Vec<String> = ui.tabs.with_untracked(|t| t.iter().filter(|t| t.dirty.get_untracked()).map(|t| t.name()).collect());
    let send = send.clone();
    if dirty.is_empty() {
        really_open(ui, &send, &path);
        return;
    }
    dialog::ask(&format!("Do you want to save the changes you made to {}?", crate::editor::tabs::names(&dirty)))
        .kind(dialog::Kind::Warning)
        .detail("Your changes will be lost if you don't save them.")
        .button("Save All")
        .button("Don't Save")
        .open(ui, move |a| match a.button {
            Some(0) => {
                crate::save::save_all(ui, &send);
                really_open(ui, &send, &path);
            }
            Some(1) => {
                let paths: Vec<String> = ui.tabs.with_untracked(|t| {
                    t.iter().filter(|t| t.dirty.get_untracked()).map(|t| t.path.clone()).collect()
                });
                for p in paths {
                    crate::save::discarded(ui, &p, &send);
                    if let Some(tab) = ui.tab(&p) {
                        tab.dirty.set(false);
                    }
                }
                really_open(ui, &send, &path);
            }
            _ => {}
        });
}

fn really_open(ui: Ui, send: &Send, path: &str) {
    if ui.loaded.get_untracked() {
        send(json!({"type": "keep_view_state", "state": ui.view_state()}));
    }
    crate::banners::opening(ui, &tilde(path, home().as_deref()));
    send(json!({"type": "folder_open", "path": path, "key": KEY}));
}

/// Whether a `state` is of another folder than the one the window shows (not the first one).
pub fn another_folder(shown: &str, loaded: bool, m: &Value) -> bool {
    loaded && !shown.is_empty() && m["folder"].as_str().is_some_and(|f| f != shown)
}

/// What was the folder's let go of before another's `state` is taken in: the tabs (their changes were answered
/// for), the groups, the Explorer's tree and the terminals. The window then restores the new folder's layout.
fn let_go(ui: Ui) {
    ui.loaded.set(false);
    let paths: Vec<String> = ui.tabs.with_untracked(|t| t.iter().map(|t| t.path.clone()).collect());
    for p in &paths {
        // Not sent to the new folder as let go of: a tab of the same name there may have its own.
        if let Some(tab) = ui.tab(p) {
            tab.dirty.set(false);
        }
        ui.close_now(p);
    }
    crate::editor::groups::change(ui, |m, _| *m = crate::editor::groups::Model::default());
    ui.tree.set(Default::default());
    ui.expanded.set(Default::default());
    ui.selected.set(String::new());
    crate::terminal::forget_all(ui);
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{FILE, Menu, item};
    add(command(OPEN_FOLDER, "File", "Open Folder...", open_folder).keys("Ctrl+K Ctrl+O"));
    add(command(OPEN_RECENT, "File", "Open Recent...", open_recent).keys("Ctrl+R"));
    add(command(CLEAR_RECENT, "File", "Clear Recently Opened...", |ui, send| {
        let send = send.clone();
        dialog::ask("Do you want to clear all recently opened folders?")
            .kind(dialog::Kind::Warning)
            .detail("This action is irreversible!")
            .button("Clear")
            .open(ui, move |a| {
                if a.yes() {
                    send(json!({"type": "recent_clear", "key": KEY}));
                }
            });
    }));
    crate::menus::add(Menu::TitleBar(FILE), item(OPEN_FOLDER).group("1_open").order(10));
    crate::menus::add(Menu::TitleBar(FILE), item(OPEN_RECENT).group("1_open").order(20));
    crate::menus::add(Menu::TitleBar(FILE), item(CLEAR_RECENT).group("1_open").order(30));
    // The Explorer's "…": another folder in its place.
    let explorer = Menu::ViewTitle(crate::layout::View::Files);
    crate::menus::add(explorer, item(OPEN_FOLDER).group("9_folder").order(10));
    crate::menus::add(explorer, item(OPEN_RECENT).group("9_folder").order(20));
}

/// The session's messages that are this part's, taken in. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "state" => {
            if another_folder(&ui.folder.get_untracked(), ui.loaded.get_untracked(), m) {
                let_go(ui);
            }
        }
        "folders" if m["key"] == KEY => {
            if st.asked.with_untracked(|a| m["path"] == a.as_str()) {
                st.asked.set(String::new());
                dialog_over(ui, send, m);
            }
        }
        "recent_folders" => {
            let list: Vec<String> =
                m["items"].as_array().into_iter().flatten().filter_map(|i| i["path"].as_str().map(str::to_string)).collect();
            if st.recent.with_untracked(|r| *r != list) {
                st.recent.set(list);
            }
            if m["key"] == RECENT_KEY {
                recent_pick(ui, send, false);
            }
        }
        "folder_opened" => {
            if let Some(e) = m["error"].as_str() {
                crate::banners::not_opening(ui);
                toast::error(ui, e);
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
    fn the_home_folder_is_shown_as_a_tilde() {
        assert_eq!(tilde("/home/me/src/a", Some("/home/me")), "~/src/a");
        assert_eq!(tilde("/home/me", Some("/home/me/")), "~");
        assert_eq!(tilde("/home/meow", Some("/home/me")), "/home/meow");
        assert_eq!(tilde("/x", None), "/x");
    }

    #[test]
    fn whole_paths_are_gone_to_and_names_are_narrowed() {
        for p in ["/", "/etc", "~", "~/src", "C:\\", "c:/Users", "D:"] {
            assert!(is_path(p), "{p}");
        }
        for p in ["src", "~src", "1:", "..", "C"] {
            assert!(!is_path(p), "{p}");
        }
    }

    #[test]
    fn the_dialog_opens_the_folder_goes_up_and_in_and_lists_the_recent_ones_after() {
        let r = rows("/w/repo", Some("/w"), &["src".into(), ".git".into()], &["/w/repo".into(), "/x/y".into()], None);
        let ids: Vec<&str> = r.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["open:/w/repo", "go:/w", "go:/w/repo/src", "go:/w/repo/.git", "go:/x/y"]);
        assert_eq!(r[0].label, "Open repo");
        assert_eq!(r[1].label, "..");
        assert_eq!(r[4].group, "recently opened");
        // The top has no "..", and its folders are not given a second "/".
        let top = rows("/", None, &["etc".into()], &[], None);
        assert_eq!(top.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["open:/", "go:/etc"]);
    }

    #[test]
    fn open_recent_leaves_out_the_folder_worked_in_and_offers_more_after_ten() {
        let recent: Vec<String> = (0..13).map(|i| format!("/f{i}")).collect();
        let first = recent_rows(&recent, "/f0", false, None);
        assert_eq!(first.len(), FIRST + 1);
        assert_eq!(first[0].id, "open:/f1");
        assert_eq!(first.last().map(|i| i.id.as_str()), Some("more"));
        let all = recent_rows(&recent, "/f0", true, None);
        assert_eq!(all.len(), 12);
        assert!(recent_rows(&["/f0".into()], "/f0", false, None).is_empty());
    }

    #[test]
    fn only_a_state_of_another_folder_after_the_first_lets_the_window_go() {
        let m = json!({"type": "state", "folder": "/b"});
        assert!(another_folder("/a", true, &m));
        assert!(!another_folder("/b", true, &m));
        assert!(!another_folder("", true, &m));
        assert!(!another_folder("/a", false, &m));
        assert!(!another_folder("/a", true, &json!({"type": "state"})));
    }
}
