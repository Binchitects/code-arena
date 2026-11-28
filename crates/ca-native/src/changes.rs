//! The Changes view, as VS Code's Source Control list: each file the agent changed with its folder dimmed and its
//! letter (M, A, D) in VS Code's colours, as a list or under its folders; a click opens its changes as an inline
//! diff, the removed lines red, the added green, each with its numbers before and after. On a row, under the
//! pointer, Open File, Revert and Accept; its right-click menu has the rest (the chat's Add, Explain and Fix, its
//! path). Accept keeps the agent's changes (the file is no longer one of them until the agent writes it again);
//! Revert puts the file back as it was, or deletes it when the agent made it. The status bar counts the files and
//! the activity bar's icon has the count as its badge.

use std::collections::BTreeSet;
use std::rc::Rc;

use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::CursorStyle;
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, dyn_container, empty, h_stack, label, scroll, tooltip, v_stack,
    virtual_stack,
};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::badges;
use crate::chat_bridge;
use crate::commands::command;
use crate::diff::{Line, Side};
use crate::ext::State;
use crate::icons::icon;
use crate::layout::View;
use crate::menus::Menu;
use crate::scm::tree::{self as scm_tree, Which};
use crate::state::{Change, Send, Ui};
use crate::widgets::tree::{self, Node};
use crate::widgets::{MONO_FONT, action, dialog, popup, toast};

/// Accept All Changes: every file the agent changed kept as it is.
pub const ACCEPT_ALL: &str = "codeArena.acceptAllChanges";

/// The ids of a folder's row (a file's is its path).
const FOLDER: &str = "folder:";

/// VS Code's colours for a changed file's letter (gitDecoration's).
fn letter_colour(letter: &str, dark: bool) -> Color {
    match (letter, dark) {
        ("A", true) => Color::rgb8(0x73, 0xC9, 0x91),
        ("A", false) => Color::rgb8(0x58, 0x7C, 0x0C),
        ("D", true) => Color::rgb8(0xC7, 0x4E, 0x39),
        ("D", false) => Color::rgb8(0xAD, 0x0B, 0x0B),
        (_, true) => Color::rgb8(0xE2, 0xC0, 0x8D),
        (_, false) => Color::rgb8(0x89, 0x5B, 0x03),
    }
}

/// The view's own state: its tree, and whether the changes were asked for and not answered yet (and asked again
/// meanwhile). Whether it shows a tree, and the folders folded, are `scm::tree`'s (kept with the folder).
#[derive(Clone, Copy)]
struct Review {
    tree: tree::State,
    asked: RwSignal<bool>,
    again: RwSignal<bool>,
}

impl State for Review {
    fn new(ui: Ui) -> Review {
        Review {
            tree: tree::State::new(ui),
            asked: ui.scope.create_rw_signal(false),
            again: ui.scope.create_rw_signal(false),
        }
    }
}

/// The changes asked for again; while an answer is awaited, once more when it comes (each answer carries every
/// file's text before and after, so a turn of many writes asks for them a few times, not once a write).
pub fn refresh(ui: Ui, send: &Send) {
    let r = ui.state::<Review>();
    if r.asked.get_untracked() {
        r.again.set(true);
    } else {
        r.asked.set(true);
        send(json!({"type": "changes"}));
    }
}

/// What tells a file's row from the one drawn before: its letter.
fn stamp(c: &Change) -> u64 {
    c.letter().as_bytes()[0] as u64
}

/// The rows: each file under its folders (a tree, as `scm::tree` lays it out: a folder with only one folder in it
/// on one row with it when `compact`), or each file alone, its folder written after its name (a list). A folder
/// comes before the files beside it; a folded one's rows are left out. A folder's stamp is how many names its row
/// shows; a file's says the way it is shown too, so its row is drawn again when that changes.
fn rows(changes: &[Change], as_tree: bool, compact: bool, folded: &BTreeSet<String>) -> im::Vector<Node> {
    if !as_tree {
        return changes.iter().map(|c| Node { stamp: stamp(c), ..tree::leaf(&c.path, 0) }).collect();
    }
    let paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
    scm_tree::layout(&paths, compact, &|p| !folded.contains(p))
        .into_iter()
        .map(|e| match e {
            scm_tree::Entry::Folder { path, span, depth, open } => {
                Node { stamp: span as u64, ..tree::folder(&format!("{FOLDER}{path}"), depth, open) }
            }
            scm_tree::Entry::File { index, depth } => {
                let c = &changes[index];
                Node { stamp: stamp(c) | 1 << 8, ..tree::leaf(&c.path, depth) }
            }
        })
        .collect()
}

/// The rows shown now. Followed.
fn rows_now(ui: Ui) -> im::Vector<Node> {
    let as_tree = scm_tree::as_tree(ui, Which::Changes);
    let compact = as_tree && scm_tree::compact(ui);
    let folded = if as_tree { scm_tree::folded(ui, Which::Changes) } else { BTreeSet::new() };
    ui.changes.with(|c| rows(c, as_tree, compact, &folded))
}

/// The change of a row (none: a folder's, or a file no longer changed).
fn change_of(ui: Ui, id: &str) -> Option<Change> {
    ui.changes.with_untracked(|cs| cs.iter().find(|c| c.path == id).cloned())
}

/// What a revert does to the file, as said under the pointer and in the question.
fn revert_says(c: &Change) -> &'static str {
    if c.letter() == "A" {
        "The agent made this file: reverting deletes it."
    } else {
        "The file goes back to how it was before the agent changed it."
    }
}

fn accept(send: &Send, path: &str) {
    send(json!({"type": "changes_accept", "path": path}));
}

/// VS Code's question before changes are discarded, then the revert.
fn ask_revert(ui: Ui, send: &Send, c: &Change) {
    let name = c.path.rsplit('/').next().unwrap_or(&c.path).to_string();
    let (send, path) = (send.clone(), c.path.clone());
    let mut asked = dialog::ask(&format!("Are you sure you want to revert the agent's changes to '{name}'?"))
        .detail(revert_says(c))
        .button("Revert")
        .destructive();
    if let Some(v) = ui.state::<Review>().tree.view() {
        asked = asked.back(v);
    }
    asked.open(ui, move |answer| {
        if answer.yes() {
            send(json!({"type": "changes_revert", "path": path}));
        }
    });
}

/// The path a file is mentioned by in the chat's box, as the box's own mentions are written.
fn mention(path: &str) -> String {
    crate::agent::mentions::mention(path, "", true).trim_end().to_string()
}

/// The row's menu: the file's changes and the file, the chat, accept and revert, its path; then the items the
/// window's parts registered for the place.
fn open_menu(ui: Ui, send: &Send, c: Change, at: popup::At) {
    let copy = |text: String| {
        move || {
            if floem::Clipboard::set_contents(text.clone()).is_err() {
                toast::error(ui, "The path could not be copied.");
            }
        }
    };
    let folder = ui.folder.get_untracked();
    let full = if folder.is_empty() { c.path.clone() } else { format!("{}/{}", folder.trim_end_matches('/'), c.path) };
    let named = mention(&c.path);
    let (shown, opened, opener) = (c.clone(), c.path.clone(), send.clone());
    let (said, explained, explainer) = (named.clone(), named.clone(), send.clone());
    let (fixed, fixer) = (named, send.clone());
    let (kept, keeper) = (c.path.clone(), send.clone());
    let (undone, undoer) = (c.clone(), send.clone());
    let mut items: Vec<popup::Item> = vec![
        popup::entry("Open Changes", move || ui.open_changes(&shown)).into(),
        popup::entry("Open File", move || ui.open(&opened, &opener)).enabled(c.after.is_some()).into(),
        popup::line(),
        popup::entry("Add to Chat", move || {
            // Before what is typed there, as a mention typed in the box: the draft is kept.
            chat_bridge::insert_at_start(ui, &format!("{said} "));
            chat_bridge::focus(ui);
        })
        .into(),
        popup::entry("Explain These Changes", move || {
            chat_bridge::ask_now(ui, &explainer, &format!("Explain the changes you made to {explained}."))
        })
        .into(),
        popup::entry("Fix These Changes", move || {
            chat_bridge::ask_now(
                ui,
                &fixer,
                &format!("Find what is wrong in the changes you made to {fixed}, and fix it."),
            )
        })
        .into(),
        popup::line(),
        popup::entry("Accept", move || accept(&keeper, &kept)).into(),
        popup::entry("Revert", move || ask_revert(ui, &undoer, &undone)).into(),
        popup::line(),
        popup::entry("Copy Path", copy(full)).into(),
        popup::entry("Copy Relative Path", copy(c.path.clone())).into(),
    ];
    let more = popup::of_rows(ui, send, crate::menus::rows(ui, Menu::ChangesRow));
    if !more.is_empty() {
        items.push(popup::line());
        items.extend(more);
    }
    popup::open(ui, at, items);
}

/// A file's letter at its row's right, right of the actions so that it stays in place under the pointer; none for
/// a folder.
fn row_letter(ui: Ui, node: &Node) -> AnyView {
    let t = ui.colors;
    if node.id.starts_with(FOLDER) {
        return empty().into_any();
    }
    let letter = change_of(ui, &node.id).map(|c| c.letter()).unwrap_or("M");
    label(move || letter)
        .style(move |s| s.font_size(12.0).width(12.0).margin_right(10.0).color(letter_colour(letter, t.get().dark)))
        .into_any()
}

/// A row: a file's name in its letter's colour and its folder dimmed (in a list); or a folder's name (the names it
/// spans, as its stamp says).
fn row(ui: Ui, node: &Node) -> AnyView {
    let t = ui.colors;
    let as_tree = scm_tree::as_tree_now(ui, Which::Changes);
    if let Some(path) = node.id.strip_prefix(FOLDER) {
        let name = scm_tree::label_of(path, node.stamp as usize);
        return label(move || name.clone()).style(|s| s.font_size(13.0)).into_any();
    }
    let path = node.id.clone();
    let letter = change_of(ui, &path).map(|c| c.letter()).unwrap_or("M");
    let (dir, name) = match path.rsplit_once('/') {
        Some((d, n)) => (d.to_string(), n.to_string()),
        None => (String::new(), path.clone()),
    };
    h_stack((
        label(move || name.clone()).style(move |s| {
            let c = t.get();
            s.flex_shrink(0.0_f32).color(letter_colour(letter, c.dark)).apply_if(letter == "D", |s| s.color(c.desc))
        }),
        label(move || dir.clone()).style(move |s| {
            s.font_size(12.0)
                .color(t.get().desc)
                .text_ellipsis()
                .min_width(0.0)
                .flex_shrink(1.0_f32)
                .apply_if(as_tree, |s| s.hide())
        }),
    ))
    .style(|s| s.width_full().items_center().gap(6.0).font_size(13.0).min_width(0.0))
    .into_any()
}

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let r = ui.state::<Review>();
    // Asked again each time the view is shown.
    r.asked.set(false);
    refresh(ui, &send);
    let (acting, menu_send) = (send.clone(), send);
    let list = tree::tree(ui, r.tree, move || rows_now(ui))
        .content(move |node| row(ui, node))
        .decoration(move |node| row_letter(ui, node))
        .actions(move |node| {
            // A folder's act on every file under it.
            if let Some(folder) = node.id.strip_prefix(FOLDER) {
                let (folder, undoer, send) = (folder.to_string(), acting.clone(), acting.clone());
                let undone = folder.clone();
                return vec![
                    tree::act("discard", "Revert", move || ask_revert_folder(ui, &undoer, &undone)),
                    tree::act("check", "Accept (keep the agent's changes)", move || {
                        for c in changes_under(ui, &folder) {
                            accept(&send, &c.path);
                        }
                    }),
                ];
            }
            let Some(c) = change_of(ui, &node.id) else { return vec![] };
            let mut out = Vec::new();
            if c.after.is_some() {
                let (path, send) = (c.path.clone(), acting.clone());
                out.push(tree::act("go-to-file", "Open File", move || ui.open(&path, &send)));
            }
            let (undone, undoer) = (c.clone(), acting.clone());
            let says = format!("Revert ({})", revert_says(&c).trim_end_matches('.'));
            out.push(tree::act("discard", &says, move || ask_revert(ui, &undoer, &undone)));
            let (path, send) = (c.path.clone(), acting.clone());
            out.push(tree::act("check", "Accept (keep the agent's changes)", move || accept(&send, &path)));
            out
        })
        .on_toggle(move |id| {
            if let Some(path) = id.strip_prefix(FOLDER) {
                scm_tree::toggle(ui, Which::Changes, path);
            }
        })
        .on_open(move |id| {
            if let Some(c) = change_of(ui, id) {
                ui.open_changes(&c);
            }
        })
        .on_menu(move |id, at| {
            if let Some(c) = id.and_then(|id| change_of(ui, id)) {
                open_menu(ui, &menu_send, c, at);
            }
        })
        .view();
    v_stack((
        label(|| "The agent has changed no file in this session.").style(move |s| {
            s.padding_horiz(20.0)
                .line_height(1.4)
                .padding_vert(6.0)
                .color(t.get().desc)
                .apply_if(ui.changes.with(|c| !c.is_empty()), |s| s.hide())
        }),
        list.style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full().min_height(0.0)),
    ))
    .style(|s| s.size_full().min_height(0.0))
}

fn tip(v: impl IntoView + 'static, text: &'static str) -> AnyView {
    tooltip(v, move || label(move || text)).into_any()
}

/// The actions in the view's title, as Source Control's: Accept All, Refresh, View as Tree or List, Collapse All.
pub fn actions(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let (all, again) = (send.clone(), send);
    let as_tree = create_memo(move |_| scm_tree::as_tree(ui, Which::Changes));
    h_stack((
        tip(
            action(t, "check-all", move || {
                crate::commands::run(ui, &all, ACCEPT_ALL);
            }),
            "Accept All Changes",
        ),
        tip(action(t, "refresh", move || refresh(ui, &again)), "Refresh"),
        dyn_container(
            move || as_tree.get(),
            move |shown| {
                if shown {
                    tip(action(t, "list-flat", move || scm_tree::set_tree(ui, Which::Changes, false)), "View as List")
                } else {
                    tip(action(t, "list-tree", move || scm_tree::set_tree(ui, Which::Changes, true)), "View as Tree")
                }
            },
        ),
        tip(action(t, "collapse-all", move || collapse_all(ui)), "Collapse All")
            .style(move |s| s.apply_if(!as_tree.get(), |s| s.hide())),
    ))
    .style(|s| s.gap(2.0))
}

/// Every folder of the tree folded.
fn collapse_all(ui: Ui) {
    let compact = scm_tree::compact(ui);
    let all: BTreeSet<String> = ui.changes.with_untracked(|cs| {
        let paths: Vec<&str> = cs.iter().map(|c| c.path.as_str()).collect();
        scm_tree::folders(&paths, compact).into_iter().collect()
    });
    scm_tree::fold_only(ui, Which::Changes, all);
}

/// The changes of the files under a folder.
fn changes_under(ui: Ui, folder: &str) -> Vec<Change> {
    ui.changes.with_untracked(|cs| cs.iter().filter(|c| scm_tree::under(folder, &c.path)).cloned().collect())
}

/// The question before every file under a folder is reverted, then the reverts.
fn ask_revert_folder(ui: Ui, send: &Send, folder: &str) {
    let under = changes_under(ui, folder);
    match under.as_slice() {
        [] => return,
        [one] => return ask_revert(ui, send, one),
        _ => {}
    }
    let name = folder.rsplit('/').next().unwrap_or(folder);
    let send = send.clone();
    let mut asked = dialog::ask(&format!(
        "Are you sure you want to revert the agent's changes to the {} files in '{name}'?",
        under.len()
    ))
    .detail("Each file goes back to how it was before the agent changed it; a file the agent made is deleted.")
    .button("Revert")
    .destructive();
    if let Some(v) = ui.state::<Review>().tree.view() {
        asked = asked.back(v);
    }
    asked.open(ui, move |answer| {
        if answer.yes() {
            for c in &under {
                send(json!({"type": "changes_revert", "path": c.path}));
            }
        }
    });
}

/// "Agent changes: 3 files", as the status bar writes it.
fn count_text(n: usize) -> String {
    if n == 1 { "Agent changes: 1 file".into() } else { format!("Agent changes: {n} files") }
}

/// A file's changes, as VS Code's inline diff.
pub fn diff_view(ui: Ui, lines: Rc<Vec<Line>>) -> impl IntoView {
    let t = ui.colors;
    let all: im::Vector<(usize, Line)> = lines.iter().cloned().enumerate().collect();
    let number = move |n: Option<usize>| {
        label(move || n.map(|n| n.to_string()).unwrap_or_default())
            .style(move |s| s.width(44.0).justify_end().padding_right(8.0).color(t.get().line_number))
    };
    scroll(
        virtual_stack(
            VirtualDirection::Vertical,
            VirtualItemSize::Fixed(Box::new(|| 19.0)),
            move || all.clone(),
            |(i, _): &(usize, Line)| *i,
            move |(_, l): (usize, Line)| {
                let side = l.side;
                let mark = match side {
                    Side::Same => " ",
                    Side::Removed => "-",
                    Side::Added => "+",
                };
                h_stack((
                    number(l.old),
                    number(l.new),
                    label(move || mark).style(|s| s.width(16.0)),
                    label(move || l.text.replace('\t', "    ")),
                ))
                .style(move |s| {
                    let c = t.get();
                    let bg = match (side, c.dark) {
                        (Side::Removed, _) => Color::rgba8(0xFF, 0x00, 0x00, 0x33),
                        (Side::Added, true) => Color::rgba8(0x9B, 0xB9, 0x55, 0x33),
                        (Side::Added, false) => Color::rgba8(0x9C, 0xCC, 0x2C, 0x40),
                        (Side::Same, _) => Color::TRANSPARENT,
                    };
                    s.height(19.0).min_width_full().items_center().background(bg).color(c.editor_fg)
                })
            },
        )
        .style(|s| s.flex_col().min_width_full()),
    )
    .style(move |s| {
        let c = t.get();
        s.size_full().font_family(MONO_FONT.clone()).font_size(14.0).background(c.editor_bg).selectable(false)
    })
}

/// What it adds to the window's registries, once at start: Accept All Changes, and the status bar's count of the
/// files the agent changed (a click shows them).
pub fn register(_ui: Ui, _send: &Send) {
    crate::commands::add(
        command(ACCEPT_ALL, "Changes", "Accept All Changes", |_, send| send(json!({"type": "changes_accept"})))
            .enabled(|ui| ui.changes.with(|c| !c.is_empty())),
    );
    crate::statusbar::add("status.agentChanges", crate::statusbar::Side::Left, 130, |ui, send| {
        let t = ui.colors;
        let n = create_memo(move |_| ui.changes.with(Vec::len));
        tooltip(
            h_stack((icon("git-compare", 16.0), label(move || count_text(n.get()))))
                .style(|s| s.gap(4.0))
                .on_click_stop(move |_| {
                    crate::commands::run(ui, &send, crate::workbench::SHOW_CHANGES);
                }),
            || label(|| "Show the Agent's Changes"),
        )
        .style(move |s| crate::statusbar::item(t, s).cursor(CursorStyle::Pointer).apply_if(n.get() == 0, |s| s.hide()))
        .into_any()
    });
}

/// The session's messages this part follows, taken in (never taken away): the files the agent changed (the view,
/// its badge and the status bar drawn again only when they changed, and the tab of a file's changes closed once it
/// is no longer changed: accepted or reverted here or elsewhere), and a write by the agent, after which they are
/// asked for again.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str() {
        Some("changes") => {
            let text = |v: &Value| v.as_str().map(str::to_string);
            let now: Vec<Change> = m["files"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| Change {
                    path: f["path"].as_str().unwrap_or("").to_string(),
                    before: text(&f["before"]),
                    after: text(&f["after"]),
                })
                .collect();
            badges::set(ui, View::Changes, now.len());
            let gone: Vec<String> = ui.tabs.with_untracked(|tabs| {
                tabs.iter()
                    .filter_map(|t| crate::compare::before_of(&t.path).map(|p| (t.path.clone(), p)))
                    .filter(|(_, p)| !now.iter().any(|c| c.path == *p))
                    .map(|(id, _)| id)
                    .collect()
            });
            // The compares of the agent's changes show what it wrote since.
            crate::compare::changes_said(ui, &now);
            if ui.changes.with_untracked(|was| *was != now) {
                ui.changes.set(now);
            }
            for id in gone {
                ui.close_document(&id);
            }
            let r = ui.state::<Review>();
            r.asked.set(false);
            if r.again.get_untracked() {
                r.again.set(false);
                refresh(ui, send);
            }
        }
        Some("tool_end") if matches!(m["name"].as_str(), Some("write_file" | "edit_file")) => refresh(ui, send),
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str, before: Option<&str>, after: Option<&str>) -> Change {
        Change { path: path.into(), before: before.map(str::to_string), after: after.map(str::to_string) }
    }

    fn ids(rows: &im::Vector<Node>) -> Vec<(String, usize)> {
        rows.iter().map(|n| (n.id.clone(), n.depth)).collect()
    }

    #[test]
    fn the_files_are_listed_alone_or_under_their_folders_the_folders_first() {
        let cs = [
            change("README.md", Some("a"), Some("b")),
            change("src/main.rs", None, Some("new")),
            change("src/ui/view.rs", Some("a"), None),
        ];
        let list = rows(&cs, false, true, &BTreeSet::new());
        assert_eq!(ids(&list), [("README.md".into(), 0), ("src/main.rs".into(), 0), ("src/ui/view.rs".into(), 0)]);
        assert_eq!(list.iter().map(|n| n.stamp).collect::<Vec<_>>(), [b'M' as u64, b'A' as u64, b'D' as u64]);
        let tree = rows(&cs, true, true, &BTreeSet::new());
        assert_eq!(
            ids(&tree),
            [
                ("folder:src".into(), 0),
                ("folder:src/ui".into(), 1),
                ("src/ui/view.rs".into(), 2),
                ("src/main.rs".into(), 1),
                ("README.md".into(), 0),
            ]
        );
        let folded = rows(&cs, true, true, &BTreeSet::from(["src/ui".to_string()]));
        assert_eq!(folded.len(), 4);
        assert_eq!(folded[1].fold, Some(false));
        // A file's row in a tree is drawn again from its row in a list.
        assert_ne!(tree[4], list[0]);
        // A folder with only one folder in it shares its row with it; its stamp is the names it spans.
        let deep = [change("tests/unit/a.rs", Some("a"), Some("b"))];
        let compact = rows(&deep, true, true, &BTreeSet::new());
        assert_eq!(ids(&compact), [("folder:tests/unit".into(), 0), ("tests/unit/a.rs".into(), 1)]);
        assert_eq!(compact[0].stamp, 2);
        assert_eq!(rows(&deep, true, false, &BTreeSet::new()).len(), 3);
    }

    #[test]
    fn the_status_bar_counts_files_and_a_revert_says_what_it_does() {
        assert_eq!(count_text(1), "Agent changes: 1 file");
        assert_eq!(count_text(3), "Agent changes: 3 files");
        assert!(revert_says(&change("a", None, Some("x"))).contains("deletes it"));
        assert!(revert_says(&change("a", Some("x"), Some("y"))).contains("goes back"));
        assert_eq!(mention("a b.rs"), "@\"a b.rs\"");
    }

    #[test]
    fn a_turn_of_many_writes_asks_for_the_changes_once_at_a_time() {
        let ui = Ui::new();
        let said = Rc::new(std::cell::RefCell::new(Vec::<Value>::new()));
        let kept = said.clone();
        let send: Send = Rc::new(move |v: Value| kept.borrow_mut().push(v));
        let write = json!({"type": "tool_end", "name": "edit_file"});
        for _ in 0..5 {
            take(ui, &write, &send);
        }
        assert_eq!(said.borrow().len(), 1, "one asked, the rest wait for its answer");
        take(ui, &json!({"type": "changes", "files": [{"path": "a.rs", "before": "a", "after": "b"}]}), &send);
        assert_eq!(said.borrow().len(), 2, "asked once more for what was written meanwhile");
        assert_eq!(ui.changes.get_untracked().len(), 1);
        assert_eq!(badges::count(ui, View::Changes).get_untracked(), 1);
        take(ui, &json!({"type": "changes", "files": []}), &send);
        assert_eq!(said.borrow().len(), 2);
        assert_eq!(badges::count(ui, View::Changes).get_untracked(), 0);
    }

    #[test]
    fn a_write_reaches_the_view_past_the_parts_before_it() {
        let ui = Ui::new();
        let said = Rc::new(std::cell::RefCell::new(Vec::<Value>::new()));
        let kept = said.clone();
        let send: Send = Rc::new(move |v: Value| kept.borrow_mut().push(v));
        ui.take(&json!({"type": "tool_end", "id": "1", "name": "write_file", "ok": true}), &send);
        assert!(said.borrow().iter().any(|v| v["type"] == "changes"), "{:?}", said.borrow());
    }

    #[test]
    fn the_changes_of_a_file_no_longer_changed_close() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let (a, b) = (change("a.rs", Some("a"), Some("b")), change("b.rs", None, Some("new")));
        crate::compare::register(ui, &send);
        ui.open_changes(&a);
        ui.open_changes(&b);
        // Accepted or reverted elsewhere: a.rs is no longer listed.
        take(ui, &json!({"type": "changes", "files": [{"path": "b.rs", "before": null, "after": "new"}]}), &send);
        let open: Vec<String> = ui.tabs.with_untracked(|t| t.iter().map(|t| t.path.clone()).collect());
        assert_eq!(open, ["compare:before:b.rs ↔ file:b.rs"]);
        assert_eq!(mention("src/my cart.rs"), "@\"src/my cart.rs\"");
        assert_eq!(mention("a.rs"), "@a.rs");
    }
}
