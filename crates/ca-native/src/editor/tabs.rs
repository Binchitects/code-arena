//! An editor group's tabs, as VS Code's: 35 px, the open one with its top border (in the focused group), a dot for
//! changes not saved, a close button; the group's actions (Split Editor Right, its "…" menu); and the question asked
//! before a tab with changes not saved closes.
//!
//! - A file opened by a click in the Explorer comes as a preview (its name in italics): the next one opened so in
//!   its group takes its place. A double click on its tab, an edit, a drag or Keep Open (Ctrl+K Enter) keeps it.
//!   `workbench.editor.enablePreview` false: none comes as a preview.
//! - A tab pinned (Ctrl+K Shift+Enter, or its menu's Pin) comes first in its group, has a pin for its close
//!   button (a click unpins it), and is not closed by a middle click nor by the commands that close several.
//! - A middle click closes a tab; a right click shows it and opens its menu (`Menu::EditorTab`).
//! - A tab dragged is put where it is dropped: before or after another tab, in its own group or another, in the
//!   middle of a group (moved there), or at a group's edge (a new group there, as Split does).
//! - Close Others, Close to the Right, Close Saved, Close All (in the group, and in every group) close their tabs
//!   with one question for those with changes not saved: Save, Don't Save, or Cancel (nothing closes).
//!
//! The order of a group's tabs is the window's (`ui.tabs`, which the groups follow), pinned tabs first: moving a
//! tab moves it there.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::kurbo::{Point, Rect};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, stack, v_stack};
use floem::{IntoView, View, ViewId};
use serde_json::Value;

use super::groups::{self, Model, Shown};
use super::{focus, tell};
use crate::commands::{self, command};
use crate::icons::icon;
use crate::menus::{self, Menu, item};
use crate::state::{Send, Tab, Ui};
use crate::widgets::{action, button, line, popup, toast};

/// The tabs' own state: each group's preview, the pinned tabs, the files asked for as previews, the tabs to close
/// once saved, the tab dragged.
#[derive(Clone, Copy)]
pub struct Tabs {
    pub preview: RwSignal<HashMap<u64, String>>,
    pub pinned: RwSignal<HashSet<String>>,
    asked: RwSignal<HashSet<String>>,
    closing: RwSignal<HashSet<String>>,
    drag: RwSignal<Option<Dragged>>,
}

impl crate::ext::State for Tabs {
    fn new(ui: Ui) -> Self {
        Tabs {
            preview: ui.scope.create_rw_signal(HashMap::new()),
            pinned: ui.scope.create_rw_signal(HashSet::new()),
            asked: ui.scope.create_rw_signal(HashSet::new()),
            closing: ui.scope.create_rw_signal(HashSet::new()),
            drag: ui.scope.create_rw_signal(None),
        }
    }
}

fn state(ui: Ui) -> Tabs {
    ui.state::<Tabs>()
}

/// A tab being dragged: from which group, where the pointer is (in the window), where it would go.
#[derive(Clone, Debug, PartialEq)]
struct Dragged {
    from: u64,
    path: String,
    at: Point,
    aim: Option<(Drop, Rect)>,
}

/// Where a tab dragged goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Drop {
    /// Next to a tab of a group: after it (true) or before it.
    By(u64, String, bool),
    /// After a group's last tab (its strip's empty part).
    End(u64),
    /// Into a group, shown there.
    Into(u64),
    /// A new group at a group's edge: after it (right, below) or before; one under the other (true) or beside.
    Split(u64, bool, bool),
}

thread_local! {
    /// The strips drawn, by group, and their tabs, by group and path: where a tab dragged is over.
    static STRIPS: RefCell<HashMap<u64, ViewId>> = RefCell::new(HashMap::new());
    static TAB_VIEWS: RefCell<HashMap<(u64, String), ViewId>> = RefCell::new(HashMap::new());
}

// ---- The order of the tabs, and which close ----

/// A group's tabs as its strip shows them: the pinned ones first, each part in the window's order.
pub fn in_order(tabs: &[String], pinned: &HashSet<String>) -> Vec<String> {
    let (mut first, rest): (Vec<String>, Vec<String>) = tabs.iter().cloned().partition(|p| pinned.contains(p));
    first.extend(rest);
    first
}

/// The window's tabs with those of `wanted` in its order, each in a place one of them had.
pub fn arrange(all: &[String], wanted: &[String]) -> Vec<String> {
    let mut next = wanted.iter().filter(|p| all.contains(p));
    all.iter()
        .map(|p| if wanted.contains(p) { next.next().cloned().unwrap_or_else(|| p.clone()) } else { p.clone() })
        .collect()
}

/// `order` with `path` put at `to` (among the others).
pub fn put_at(order: &[String], path: &str, to: usize) -> Vec<String> {
    let mut out: Vec<String> = order.iter().filter(|p| *p != path).cloned().collect();
    out.insert(to.min(out.len()), path.to_string());
    out
}

/// What a close command closes of a group's tabs (in its strip's order), from the tab it is about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Close {
    Others,
    Right,
    Saved,
    All,
}

/// The tabs `how` closes: never a pinned one.
pub fn to_close(
    order: &[String],
    at: &str,
    how: Close,
    pinned: &HashSet<String>,
    dirty: impl Fn(&str) -> bool,
) -> Vec<String> {
    let i = order.iter().position(|p| p == at);
    order
        .iter()
        .enumerate()
        .filter(|(j, p)| match how {
            Close::Others => *p != at,
            Close::Right => i.is_some_and(|i| *j > i),
            Close::Saved => !dirty(p),
            Close::All => true,
        })
        .map(|(_, p)| p.clone())
        .filter(|p| !pinned.contains(p))
        .collect()
}

/// Names said in a question: "a", "a and b", "a, b and c".
pub fn names(list: &[String]) -> String {
    match list {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A tab dragged from a group put where it is dropped, in the groups (its place among the tabs is the window's
/// order's, set after). False when nothing changes.
pub fn put(m: &mut Model, from: u64, path: &str, to: &Drop, order: &[String]) -> bool {
    let join = |m: &mut Model, id: u64| {
        if let Some(g) = m.groups.iter_mut().find(|g| g.id == id) {
            if !g.tabs.iter().any(|t| t == path) {
                g.tabs.push(path.to_string());
                g.tabs.sort_by_key(|p| order.iter().position(|o| o == p).unwrap_or(usize::MAX));
            }
            g.active = Some(path.to_string());
        }
        m.focused = id;
    };
    match *to {
        Drop::By(id, _, _) | Drop::End(id) | Drop::Into(id) => {
            if m.group(id).is_none() {
                return false;
            }
            join(m, id);
            if id != from {
                m.drop_tab(from, path);
                m.focused = id;
            }
            true
        }
        Drop::Split(id, vertical, after) => {
            if m.group(id).is_none() || (m.groups.len() > 1 && m.vertical != vertical) {
                return false;
            }
            m.orient(vertical);
            m.focused = id;
            let Some(made) = m.split(after) else { return false };
            if let Some(g) = m.groups.iter_mut().find(|g| g.id == made) {
                g.tabs = vec![path.to_string()];
                g.active = Some(path.to_string());
            }
            m.drop_tab(from, path);
            m.focused = made;
            true
        }
    }
}

// ---- Preview and pinned ----

/// Whether new tabs come as previews (`workbench.editor.enablePreview`, true unless said).
fn previews(ui: Ui) -> bool {
    crate::settings::now(ui, "workbench.editor.enablePreview", None).as_bool().unwrap_or(true)
}

/// A file opened as a preview (a click in the Explorer): shown when it is open, else asked for, and its group's
/// preview when it comes.
pub fn open_preview(ui: Ui, path: &str, send: &Send) {
    if ui.tab(path).is_none() && previews(ui) {
        state(ui).asked.update(|a| {
            a.insert(path.to_string());
        });
    }
    ui.open(path, send);
}

/// A tab kept open: no longer a preview in any group.
pub fn keep(ui: Ui, path: &str) {
    let st = state(ui);
    if st.preview.with_untracked(|m| m.values().any(|p| p == path)) {
        st.preview.update(|m| m.retain(|_, p| p != path));
    }
}

fn is_pinned(ui: Ui, path: &str) -> bool {
    state(ui).pinned.with_untracked(|p| p.contains(path))
}

fn is_preview(ui: Ui, group: u64, path: &str) -> bool {
    state(ui).preview.with_untracked(|m| m.get(&group).is_some_and(|p| p == path))
}

/// A group's tabs in its strip's order.
fn order_of(ui: Ui, group: u64) -> Vec<String> {
    let tabs = groups::groups(ui).model.with_untracked(|m| m.group(group).map(|g| g.tabs.clone())).unwrap_or_default();
    state(ui).pinned.with_untracked(|p| in_order(&tabs, p))
}

/// The window's tabs put in the order a group's are wanted in.
fn set_order(ui: Ui, wanted: &[String]) {
    let all: Vec<String> = ui.tabs.with_untracked(|t| t.iter().map(|t| t.path.clone()).collect());
    let now = arrange(&all, wanted);
    if now != all {
        ui.tabs.update(|t| t.sort_by_key(|tab| now.iter().position(|p| *p == tab.path).unwrap_or(usize::MAX)));
    }
}

/// A tab pinned (last of its group's pinned ones) or unpinned (first of the others).
fn pin(ui: Ui, group: u64, path: &str, on: bool) {
    let st = state(ui);
    if st.pinned.with_untracked(|p| p.contains(path)) == on {
        return;
    }
    keep(ui, path);
    let order = order_of(ui, group);
    let pinned_before = order.iter().filter(|p| *p != path && is_pinned(ui, p)).count();
    st.pinned.update(|p| {
        if on {
            p.insert(path.to_string());
        } else {
            p.remove(path);
        }
    });
    set_order(ui, &put_at(&order, path, pinned_before));
}

/// The tab a command is about: the one the focused group shows.
fn target(ui: Ui) -> Option<(u64, String)> {
    let g = groups::groups(ui).model.with_untracked(|m| (m.focused, m.shown()));
    g.1.map(|p| (g.0, p))
}

// ---- Closing several ----

/// Tabs closed, each in its group: those whose document no other group keeps close (asked about once when they
/// have changes not saved); the others leave their group only.
pub fn close_many(ui: Ui, list: Vec<(u64, String)>) {
    if list.is_empty() {
        return;
    }
    let m = groups::groups(ui).model.get_untracked();
    let closes_doc = |path: &str| {
        m.groups
            .iter()
            .filter(|g| g.tabs.iter().any(|t| t == path))
            .all(|g| list.iter().any(|(id, p)| *id == g.id && p == path))
    };
    let mut docs: Vec<String> = Vec::new();
    let mut drops: Vec<(u64, String)> = Vec::new();
    for (id, p) in &list {
        if closes_doc(p) {
            if !docs.contains(p) {
                docs.push(p.clone());
            }
        } else {
            drops.push((*id, p.clone()));
        }
    }
    let dirty: Vec<String> = docs
        .iter()
        .filter(|p| ui.tab(p).is_some_and(|t| t.diff.is_none() && t.dirty.get_untracked()))
        .cloned()
        .collect();
    let done = move |ui: Ui, save: bool| finish(ui, &drops, &docs, save);
    if dirty.is_empty() {
        done(ui, false);
        return;
    }
    let shown: Vec<String> = dirty.iter().map(|p| ui.tab(p).map_or_else(|| p.clone(), |t| t.name())).collect();
    crate::widgets::dialog::ask(&format!("Do you want to save the changes you made to {}?", names(&shown)))
        .kind(crate::widgets::dialog::Kind::Warning)
        .detail("Your changes will be lost if you don't save them.")
        .button("Save")
        .button("Don't Save")
        .open(ui, move |a| match a.button {
            Some(0) => done(ui, true),
            Some(1) => done(ui, false),
            _ => {}
        });
}

/// Tabs closed once answered: those that leave a group only, and the documents (saved first, when asked to, and
/// closed once saved).
fn finish(ui: Ui, drops: &[(u64, String)], docs: &[String], save: bool) {
    for (id, p) in drops {
        if groups::groups(ui).model.with_untracked(|m| m.group(*id).is_some()) {
            groups::change(ui, |m, _| m.drop_tab(*id, p));
        }
    }
    for p in docs {
        let dirty = ui.tab(p).is_some_and(|t| t.diff.is_none() && t.dirty.get_untracked());
        if save && dirty {
            state(ui).closing.update(|c| {
                c.insert(p.clone());
            });
            tell(ui, |send| ui.save_tab(p, send));
        } else {
            ui.close_now(p);
        }
    }
}

/// The focused group's tabs a close command closes, from the tab it shows.
fn close_in_group(ui: Ui, how: Close) {
    let Some((group, at)) = target(ui).or_else(|| {
        let id = groups::focused(ui);
        order_of(ui, id).first().cloned().map(|p| (id, p))
    }) else {
        return;
    };
    let order = order_of(ui, group);
    let dirty = |p: &str| ui.tab(p).is_some_and(|t| t.dirty.get_untracked());
    let list = state(ui).pinned.with_untracked(|pinned| to_close(&order, &at, how, pinned, dirty));
    close_many(ui, list.into_iter().map(|p| (group, p)).collect());
}

// ---- Going between tabs ----

/// Every group's tabs, the groups in their order.
fn all_tabs(ui: Ui) -> Vec<(u64, String)> {
    let ids: Vec<u64> = groups::groups(ui).model.with_untracked(|m| m.groups.iter().map(|g| g.id).collect());
    ids.into_iter().flat_map(|id| order_of(ui, id).into_iter().map(move |p| (id, p))).collect()
}

/// The next tab (or the one before), in the focused group only or through every group; round at the ends.
fn step(ui: Ui, forward: bool, in_group: bool) {
    let Some(at) = target(ui) else { return };
    let list = if in_group { order_of(ui, at.0).into_iter().map(|p| (at.0, p)).collect() } else { all_tabs(ui) };
    let Some(i) = list.iter().position(|t| *t == at) else { return };
    let n = list.len();
    let (id, p) = &list[if forward { (i + 1) % n } else { (i + n - 1) % n }];
    groups::show(ui, *id, p);
}

/// The focused group's tab shown moved one place left or right, among the pinned or the others.
fn move_in_group(ui: Ui, right: bool) {
    let Some((group, path)) = target(ui) else { return };
    let order = order_of(ui, group);
    let Some(i) = order.iter().position(|p| *p == path) else { return };
    let j = if right { i + 1 } else { i.wrapping_sub(1) };
    let Some(other) = order.get(j) else { return };
    if is_pinned(ui, other) != is_pinned(ui, &path) {
        return;
    }
    keep(ui, &path);
    set_order(ui, &put_at(&order, &path, j));
}

/// The n-th tab (from 1) of the focused group shown; 0: the last.
fn show_at(ui: Ui, n: usize) {
    let id = groups::focused(ui);
    let order = order_of(ui, id);
    let found = if n == 0 { order.last() } else { order.get(n - 1) };
    if let Some(p) = found.cloned() {
        groups::show(ui, id, &p);
    }
}

// ---- The path of the tab shown ----

/// The file of the tab shown, its path from the folder; none for a tab that is no file.
fn file_of(ui: Ui) -> Option<String> {
    let path = ui.active.get_untracked()?;
    ui.tab(&path).filter(|t| t.kind.is_none()).map(|t| t.file)
}

fn full(ui: Ui, file: &str) -> String {
    let folder = ui.folder.get_untracked();
    if file.starts_with('/') || folder.is_empty() {
        file.to_string()
    } else {
        format!("{}/{file}", folder.trim_end_matches('/'))
    }
}

fn copy(ui: Ui, text: String) {
    if floem::Clipboard::set_contents(text).is_err() {
        toast::error(ui, "The path could not be copied.");
    }
}

/// The file of the tab shown lit in the Explorer, its folders opened.
fn reveal(ui: Ui, send: &Send) {
    let Some(file) = file_of(ui) else { return };
    ui.reveal(crate::layout::View::Files);
    let mut dir = String::new();
    for part in file.split('/').collect::<Vec<_>>().split_last().map(|(_, d)| d.to_vec()).unwrap_or_default() {
        dir = if dir.is_empty() { part.to_string() } else { format!("{dir}/{part}") };
        if !ui.expanded.with_untracked(|e| e.contains(&dir)) {
            ui.toggle_folder(&dir, send);
        }
    }
    ui.selected.set(file);
}

// ---- The strip ----

pub(super) fn tabs(ui: Ui, group: Shown) -> impl IntoView {
    let t = ui.colors;
    let st = state(ui);
    let order = create_memo(move |_| {
        let tabs = group.tabs.get();
        st.pinned.with(|p| in_order(&tabs, p))
    });
    let tabs = move || order.get().iter().filter_map(|p| ui.tab(p)).collect::<Vec<_>>();
    let empty = move || group.tabs.with(|t| t.is_empty());
    let more = container(icon("ellipsis", 16.0));
    let more_id = more.id();
    let strip = h_stack((
        scroll(
            dyn_stack(tabs, |tab: &Tab| tab.path.clone(), move |tab: Tab| one_tab(ui, group, tab))
                .style(|s| s.flex_row().height_full()),
        )
        // As narrow as the group: the tabs that do not fit scroll under it, instead of the group being made as
        // wide as all its tabs (which pushed the parts beside the editor out of the window).
        .scroll_style(|s| s.hide_bars(true))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).height_full()),
        h_stack((
            // An SVG's text has its preview a click away (Open Preview, as a Markdown file's has in VS Code).
            action(t, "open-preview", move || {
                if let Some(path) = group.active.get_untracked().filter(|p| svg_text(ui, p)) {
                    groups::focus(ui, group.id);
                    tell(ui, |send| crate::media::preview(ui, &path, send));
                }
            })
            .style(move |s| {
                s.apply_if(!group.active.with(|a| a.as_deref().is_some_and(|p| svg_text(ui, p))), |s| s.hide())
            }),
            // Split Editor Right (Ctrl+\), as VS Code's: the group's tab shown again in a group beside it.
            action(t, "split-horizontal", move || {
                groups::focus(ui, group.id);
                groups::split(ui, None, true);
            }),
            // The group's "…": its close commands.
            more.on_click_stop(move |_| {
                groups::focus(ui, group.id);
                let run = |id: &'static str| {
                    move || {
                        tell(ui, |send| {
                            commands::run(ui, send, id);
                        })
                    }
                };
                let keys = |id: &str| commands::find(id).map(|c| c.shown_keys().to_string()).unwrap_or_default();
                popup::open(
                    ui,
                    popup::At::under(more_id),
                    vec![
                        popup::entry("Close All", run(CLOSE_GROUP)).keys(&keys(CLOSE_GROUP)).into(),
                        popup::entry("Close Saved", run(CLOSE_SAVED)).keys(&keys(CLOSE_SAVED)).into(),
                    ],
                );
            })
            .style(move |s| {
                let c = t.get();
                s.width(22.0)
                    .height(22.0)
                    .items_center()
                    .justify_center()
                    .border_radius(5.0)
                    .cursor(CursorStyle::Pointer)
                    .hover(|s| s.background(c.toolbar_hover))
            }),
        ))
        .style(move |s| s.gap(2.0).padding_horiz(8.0).items_center().apply_if(empty(), |s| s.hide())),
    ));
    let id = strip.id();
    STRIPS.with(|s| s.borrow_mut().insert(group.id, id));
    strip
        .on_cleanup(move || {
            STRIPS.with(|s| {
                let mut s = s.borrow_mut();
                if s.get(&group.id) == Some(&id) {
                    s.remove(&group.id);
                }
            })
        })
        .style(move |s| {
            let c = t.get();
            line(s, c.tabs_border, 'b')
                .height(35.0)
                .width_full()
                .background(c.tabs_bg)
                .items_center()
                .apply_if(empty(), |s| s.hide())
        })
}

/// Whether a tab is an SVG file's text.
fn svg_text(ui: Ui, path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".svg") && ui.tab(path).is_some_and(|t| t.is_file())
}

/// What a tab says when the pointer rests on it: its file's whole path, or its name.
fn tip(ui: Ui, tab: &Tab) -> String {
    if tab.kind.is_none() { full(ui, &tab.file) } else { super::kinds::title(ui, tab) }
}

fn one_tab(ui: Ui, group: Shown, tab: Tab) -> impl IntoView {
    let t = ui.colors;
    let st = state(ui);
    let path = tab.path.clone();
    let here = tab.path.clone();
    let is_active = create_memo(move |_| group.active.with(|a| a.as_deref() == Some(here.as_str())));
    let hover = create_rw_signal(false);
    let dirty = tab.dirty;
    let p = tab.path.clone();
    let pinned = create_memo(move |_| st.pinned.with(|s| s.contains(&p)));
    let p = tab.path.clone();
    let preview = create_memo(move |_| st.preview.with(|m| m.get(&group.id) == Some(&p)));
    // An edit keeps a preview.
    {
        let p = tab.path.clone();
        create_effect(move |_| {
            if dirty.get() {
                keep(ui, &p);
            }
        });
    }
    // The open tab's top border is the focused group's only (tab.unfocusedActiveBorderTop is none).
    let top = move || is_active.get() && group.focused.get();
    // A tab of a kind has its kind's icon before its name, and its name from its kind (a file's has none).
    let kind_icon = super::kinds::icon(&tab);
    let named = tab.clone();
    // What its button is: the pin (a click unpins), the dot of changes not saved, the close button, or nothing.
    let slot = create_memo(move |_| {
        let shown = hover.get() || is_active.get();
        match (pinned.get(), dirty.get()) {
            (true, d) => {
                if d {
                    1
                } else {
                    0
                }
            }
            (false, true) if !hover.get() => 2,
            _ if shown => 3,
            _ => 4,
        }
    });
    let shut = tab.path.clone();
    let content = h_stack((
        icon(kind_icon, 16.0).style(move |s| s.apply_if(kind_icon.is_empty(), |s| s.hide())),
        label(move || super::kinds::title(ui, &named)).style(move |s| {
            s.font_size(13.0)
                .selectable(false)
                .apply_if(preview.get() && *crate::widgets::ITALIC, |s| s.font_style(floem::text::Style::Italic))
        }),
        dyn_container(
            move || slot.get(),
            move |slot| {
                let shut = shut.clone();
                match slot {
                    0 | 1 => action(t, if slot == 0 { "pinned" } else { "pinned-dirty" }, move || {
                        pin(ui, group.id, &shut, false)
                    })
                    .into_any(),
                    2 => icon("circle-filled", 16.0).style(|s| s.font_size(10.0)).into_any(),
                    3 => action(t, "close", move || groups::close_in(ui, group.id, &shut)).into_any(),
                    _ => empty().style(|s| s.width(22.0)).into_any(),
                }
            },
        )
        // A press on its button is not one on the tab (it is not shown first, nor dragged).
        .on_event_stop(EventListener::PointerDown, |_| {})
        .style(|s| s.width(22.0).height(22.0).items_center().justify_center()),
    ))
    .style(|s| s.flex_grow(1.0_f32).items_center().gap(4.0).padding_left(10.0).padding_right(4.0));
    let view = v_stack((
        // The open tab's top border (tab.activeBorderTop).
        empty().style(move |s| {
            let c = t.get();
            s.height(1.0).width_full().background(if top() { c.tab_top } else { floem::peniko::Color::TRANSPARENT })
        }),
        content,
    ));
    let id = view.id();
    let key = (group.id, tab.path.clone());
    TAB_VIEWS.with(|v| v.borrow_mut().insert(key.clone(), id));
    let start = create_rw_signal(None::<Point>);
    let window_at = move |p: Point| {
        let o = id.layout_rect().origin();
        Point::new(o.x + p.x, o.y + p.y)
    };
    let (down, up, dbl, menu) = (path.clone(), path.clone(), path.clone(), path.clone());
    let tip_tab = tab.clone();
    let view = view
        .on_event(EventListener::PointerDown, move |e| {
            if let Event::PointerDown(p) = e
                && p.button.is_primary()
            {
                groups::show(ui, group.id, &down);
                start.set(Some(window_at(p.pos)));
                id.request_active();
            }
            EventPropagation::Continue
        })
        .on_event_stop(EventListener::PointerMove, move |e| {
            let (Event::PointerMove(p), Some(from)) = (e, start.get_untracked()) else { return };
            let now = window_at(p.pos);
            if st.drag.with_untracked(|d| d.is_none()) && (now - from).hypot() < 5.0 {
                return;
            }
            let aim = aim(ui, group.id, &path, now);
            st.drag.set(Some(Dragged { from: group.id, path: path.clone(), at: now, aim }));
        })
        .on_event_stop(EventListener::PointerUp, move |e| {
            if let Event::PointerUp(p) = e
                && p.button.is_auxiliary()
            {
                // A middle click closes it (not a pinned one).
                if !is_pinned(ui, &up) {
                    groups::close_in(ui, group.id, &up);
                }
                return;
            }
            start.set(None);
            if let Some(d) = st.drag.get_untracked() {
                st.drag.set(None);
                if let Some((to, _)) = d.aim {
                    dropped(ui, d.from, &d.path, to);
                }
            }
        })
        .on_double_click_stop(move |_| keep(ui, &dbl))
        .on_secondary_click_stop(move |e| {
            groups::show(ui, group.id, &menu);
            let at = popup::At::pointer(id, e);
            tell(ui, |send| popup::open(ui, at, tab_menu(ui, send)));
        })
        .on_cleanup(move || {
            TAB_VIEWS.with(|v| {
                let mut v = v.borrow_mut();
                if v.get(&key) == Some(&id) {
                    v.remove(&key);
                }
            })
        })
        .style(move |s| {
            let c = t.get();
            let s = s
                .height_full()
                .min_width(80.0)
                .border_right(1.0)
                .border_color(c.tab_border)
                .cursor(CursorStyle::Pointer);
            if is_active.get() {
                // In a group without the keys, dimmer (tab.unfocusedActiveForeground).
                s.background(c.tab_active_bg).color(if group.focused.get() {
                    c.tab_active_fg
                } else {
                    c.tab_unfocused_active_fg
                })
            } else {
                s.background(c.tab_inactive_bg).color(c.tab_inactive_fg).hover(|s| s.background(c.tab_hover))
            }
        });
    floem::views::tooltip(view, move || {
        let tab = tip_tab.clone();
        label(move || tip(ui, &tab))
    })
    .on_event_cont(EventListener::PointerEnter, move |_| hover.set(true))
    .on_event_cont(EventListener::PointerLeave, move |_| hover.set(false))
}

/// A tab's menu: the rows registered for `Menu::EditorTab`, about the tab shown (a right click shows it first).
fn tab_menu(ui: Ui, send: &Send) -> Vec<popup::Item> {
    popup::of_rows(ui, send, menus::rows(ui, Menu::EditorTab))
}

// ---- Dragging ----

/// Where a tab dragged from a group would go with the pointer at `p` (in the window), and what is lit for it.
fn aim(ui: Ui, from: u64, path: &str, p: Point) -> Option<(Drop, Rect)> {
    let strips: Vec<(u64, ViewId)> = STRIPS.with(|s| s.borrow().iter().map(|(g, v)| (*g, *v)).collect());
    let (count, vertical) = groups::groups(ui).model.with_untracked(|m| (m.groups.len(), m.vertical));
    for (gid, strip) in strips {
        let r = strip.layout_rect();
        if r.contains(p) && r.area() > 0.0 {
            let mut last = r.x0;
            for tab in order_of(ui, gid) {
                let Some(v) = TAB_VIEWS.with(|t| t.borrow().get(&(gid, tab.clone())).copied()) else { continue };
                let tr = v.layout_rect();
                last = last.max(tr.x1);
                if tr.contains(p) {
                    if tab == path && gid == from {
                        return None;
                    }
                    return Some((Drop::By(gid, tab, p.x > tr.center().x), tr));
                }
            }
            return Some((Drop::End(gid), Rect::new(last, r.y0, r.x1.max(last), r.y1)));
        }
        let Some(body) = strip.parent() else { continue };
        let b = body.layout_rect();
        if !b.contains(p) || b.area() <= 0.0 {
            continue;
        }
        // Under the strip: the group's editor.
        let b = Rect::new(b.x0, r.y1.max(b.y0), b.x1, b.y1);
        let (fx, fy) = ((p.x - b.x0) / b.width().max(1.0), (p.y - b.y0) / b.height().max(1.0));
        let edge = if fx < 0.25 {
            Some((false, false))
        } else if fx > 0.75 {
            Some((false, true))
        } else if fy < 0.25 {
            Some((true, false))
        } else if fy > 0.75 {
            Some((true, true))
        } else {
            None
        };
        let alone = gid == from && order_of(ui, gid).len() < 2;
        let can_split = |v: bool| count < groups::MOST && (count == 1 || vertical == v) && !alone;
        return match edge {
            Some((v, after)) if can_split(v) => {
                let half = match (v, after) {
                    (false, false) => Rect::new(b.x0, b.y0, b.center().x, b.y1),
                    (false, true) => Rect::new(b.center().x, b.y0, b.x1, b.y1),
                    (true, false) => Rect::new(b.x0, b.y0, b.x1, b.center().y),
                    (true, true) => Rect::new(b.x0, b.center().y, b.x1, b.y1),
                };
                Some((Drop::Split(gid, v, after), half))
            }
            _ if gid == from => None,
            _ => Some((Drop::Into(gid), b)),
        };
    }
    None
}

/// A tab dropped: kept, put in its group (or a new one) and in its place there.
fn dropped(ui: Ui, from: u64, path: &str, to: Drop) {
    keep(ui, path);
    // A tab dropped among the pinned ones is pinned, among the others unpinned.
    if let Drop::By(_, other, _) = &to {
        let want = is_pinned(ui, other);
        if want != is_pinned(ui, path) {
            state(ui).pinned.update(|p| {
                if want {
                    p.insert(path.to_string());
                } else {
                    p.remove(path);
                }
            });
        }
    }
    let target = match &to {
        Drop::By(id, ..) | Drop::End(id) => Some(*id),
        _ => None,
    };
    let path_s = path.to_string();
    let t2 = to.clone();
    groups::change(ui, move |m, order| {
        put(m, from, &path_s, &t2, order);
    });
    // Its place among the group's tabs.
    if let Some(id) = target {
        let order = order_of(ui, id);
        let without: Vec<String> = order.iter().filter(|p| *p != path).cloned().collect();
        let at = match &to {
            Drop::By(_, other, after) => {
                without.iter().position(|p| p == other).map_or(without.len(), |i| i + usize::from(*after))
            }
            _ => without.iter().filter(|p| is_pinned(ui, p) || !is_pinned(ui, path)).count(),
        };
        set_order(ui, &put_at(&order, path, at));
    }
}

/// What is drawn over the editor groups: the question asked before a tab with changes not saved closes, and while
/// a tab is dragged, where it would go (editorGroup.dropBackground).
pub(super) fn close_question(ui: Ui) -> impl IntoView {
    let st = state(ui);
    let t = ui.colors;
    let shade = create_memo(move |_| st.drag.with(|d| d.as_ref().and_then(|d| d.aim.as_ref().map(|a| a.1))));
    let over = empty();
    let area = over.id();
    let drawn = empty().style(move |s| {
        let o = area.parent().map(|p| p.layout_rect().origin()).unwrap_or_default();
        match shade.get() {
            Some(r) => s
                .absolute()
                .inset_left(r.x0 - o.x)
                .inset_top(r.y0 - o.y)
                .width(r.width())
                .height(r.height())
                .background(t.get().drop_bg),
            None => s.hide(),
        }
    });
    let busy = move || ui.closing.with(|c| c.is_some()) || st.drag.with(|d| d.is_some());
    stack((question(ui), over.style(|s| s.hide()), drawn))
        .style(move |s| s.absolute().inset(0.0).z_index(30).apply_if(!busy(), |s| s.hide()))
}

/// Asked before a tab with changes not saved closes, as VS Code asks: Save (it closes once saved), Don't Save,
/// Cancel; over the editor group, which takes no clicks meanwhile.
fn question(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    // The file's name: a tab of a kind is asked about by its file too ("settings.json", not "user:settings.json").
    let name = move || {
        let named = |p: &str| ui.tab(p).map_or_else(|| p.rsplit('/').next().unwrap_or(p).to_string(), |tab| tab.name());
        ui.closing.with(|c| c.as_deref().map(named)).unwrap_or_default()
    };
    let dialog = v_stack((
        label(move || format!("Do you want to save the changes you made to {}?", name()))
            .style(|s| s.font_size(15.0).text_overflow(TextOverflow::Wrap)),
        label(|| "Your changes will be lost if you don't save them.")
            .style(move |s| s.margin_top(8.0).color(t.get().desc).text_overflow(TextOverflow::Wrap)),
        h_stack((
            button(t, "Save", true, move || {
                if let Some(path) = ui.closing.get_untracked() {
                    ui.close_saved.set(Some(path.clone()));
                    ui.closing.set(None);
                    tell(ui, |send| ui.save_tab(&path, send));
                }
            }),
            button(t, "Don't Save", false, move || {
                if let Some(path) = ui.closing.get_untracked() {
                    ui.close_now(&path);
                }
            }),
            button(t, "Cancel", false, move || ui.closing.set(None)),
        ))
        .style(|s| s.margin_top(20.0).gap(8.0).justify_end()),
    ))
    .on_click_stop(|_| {})
    .style(move |s| {
        let c = t.get();
        s.width(440.0)
            .max_width_pct(90.0)
            .padding(20.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(c.widget_border)
            .background(c.widget)
            .color(c.fg)
            .box_shadow_blur(16.0)
            .box_shadow_v_offset(4.0)
            .box_shadow_color(c.shadow)
    });
    let cancel = move || ui.closing.set(None);
    let over = container(dialog);
    let id = over.id();
    // The keys are the question's while it is asked (Escape cancels), and the editor's again after.
    create_effect(move |was: Option<bool>| {
        let asked = ui.closing.with(|c| c.is_some());
        if asked {
            id.request_focus();
        } else if was == Some(true)
            && let Some(editor) = ui.live.get_untracked().map(|(_, e)| e)
        {
            focus(&editor);
        }
        asked
    });
    over.keyboard_navigable()
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| cancel())
        .on_click_stop(|_| {})
        .on_event_stop(EventListener::PointerDown, |_| {})
        .on_event_stop(EventListener::PointerWheel, |_| {})
        .style(move |s| {
            s.absolute()
                .inset(0.0)
                .items_center()
                .justify_center()
                .background(floem::peniko::Color::rgba8(0, 0, 0, 0x4D))
                .selectable(false)
                .apply_if(ui.closing.with(|c| c.is_none()), |s| s.hide())
        })
}

// ---- Commands ----

const CLOSE_OTHERS: &str = "workbench.action.closeOtherEditors";
const CLOSE_RIGHT: &str = "workbench.action.closeEditorsToTheRight";
const CLOSE_SAVED: &str = "workbench.action.closeUnmodifiedEditors";
const CLOSE_GROUP: &str = "workbench.action.closeEditorsInGroup";
const CLOSE_ALL: &str = "workbench.action.closeAllEditors";
const KEEP: &str = "workbench.action.keepEditor";
const PIN: &str = "workbench.action.pinEditor";
const UNPIN: &str = "workbench.action.unpinEditor";
const COPY_PATH: &str = "workbench.action.files.copyPathOfActiveFile";
const COPY_RELATIVE: &str = "workbench.action.files.copyRelativePathOfActiveFile";
const REVEAL: &str = "workbench.files.action.showActiveFileInExplorer";

/// What it adds to the window's registries, once at start: the tabs' commands, their keys, the tab's menu, and what
/// makes a file asked for as a preview its group's preview, and closes a tab once saved.
pub fn register(ui: Ui, _send: &Send) {
    let view = "View";
    let shown = |ui: Ui| target(ui).is_some();
    let add = commands::add;
    add(command(CLOSE_OTHERS, view, "Close Other Editors in Group", |ui, _| close_in_group(ui, Close::Others))
        .keys("Ctrl+Alt+W")
        .enabled(shown));
    add(command(CLOSE_RIGHT, view, "Close Editors to the Right in Group", |ui, _| close_in_group(ui, Close::Right))
        .enabled(shown));
    add(command(CLOSE_SAVED, view, "Close Saved Editors in Group", |ui, _| close_in_group(ui, Close::Saved))
        .keys("Ctrl+K U"));
    add(command(CLOSE_GROUP, view, "Close All Editors in Group", |ui, _| close_in_group(ui, Close::All))
        .keys("Ctrl+K W"));
    add(command(CLOSE_ALL, view, "Close All Editors", |ui, _| {
        let pinned = state(ui).pinned.get_untracked();
        close_many(ui, all_tabs(ui).into_iter().filter(|(_, p)| !pinned.contains(p)).collect());
    })
    .keys("Ctrl+K Ctrl+W"));
    add(command(KEEP, view, "Keep Editor", |ui, _| {
        if let Some((_, p)) = target(ui) {
            keep(ui, &p);
        }
    })
    .keys("Ctrl+K Enter")
    .enabled(|ui| target(ui).is_some_and(|(g, p)| is_preview(ui, g, &p))));
    add(command(PIN, view, "Pin Editor", |ui, _| {
        if let Some((g, p)) = target(ui) {
            pin(ui, g, &p, true);
        }
    })
    .keys("Ctrl+K Shift+Enter")
    .when(|ui| target(ui).is_some_and(|(_, p)| !is_pinned(ui, &p))));
    add(command(UNPIN, view, "Unpin Editor", |ui, _| {
        if let Some((g, p)) = target(ui) {
            pin(ui, g, &p, false);
        }
    })
    .keys("Ctrl+K Shift+Enter")
    .when(|ui| target(ui).is_some_and(|(_, p)| is_pinned(ui, &p))));
    add(command("workbench.action.nextEditor", view, "Open Next Editor", |ui, _| step(ui, true, false))
        .keys("Ctrl+PageDown"));
    add(command("workbench.action.previousEditor", view, "Open Previous Editor", |ui, _| step(ui, false, false))
        .keys("Ctrl+PageUp"));
    add(command("workbench.action.nextEditorInGroup", view, "Open Next Editor in Group", |ui, _| step(ui, true, true))
        .keys("Ctrl+K Ctrl+PageDown")
        .keys("Ctrl+Alt+PageDown"));
    add(command("workbench.action.previousEditorInGroup", view, "Open Previous Editor in Group", |ui, _| {
        step(ui, false, true)
    })
    .keys("Ctrl+K Ctrl+PageUp")
    .keys("Ctrl+Alt+PageUp"));
    add(command("workbench.action.moveEditorLeftInGroup", view, "Move Editor Left", |ui, _| move_in_group(ui, false))
        .keys("Ctrl+Shift+PageUp")
        .enabled(shown));
    add(command("workbench.action.moveEditorRightInGroup", view, "Move Editor Right", |ui, _| move_in_group(ui, true))
        .keys("Ctrl+Shift+PageDown")
        .enabled(shown));
    for n in 1..=9 {
        let id = format!("workbench.action.openEditorAtIndex{n}");
        let title = format!("Open Editor at Index {n}");
        add(command(&id, view, &title, move |ui, _| show_at(ui, n)).keys(&format!("Alt+{n}")).hidden());
    }
    add(command("workbench.action.lastEditorInGroup", view, "Open Last Editor in Group", |ui, _| show_at(ui, 0))
        .keys("Alt+0"));
    let file = |ui: Ui| file_of(ui).is_some();
    add(command(COPY_PATH, "File", "Copy Path of Active File", |ui, _| {
        if let Some(f) = file_of(ui) {
            copy(ui, full(ui, &f));
        }
    })
    .keys("Ctrl+K P")
    .enabled(file));
    add(command(COPY_RELATIVE, "File", "Copy Relative Path of Active File", |ui, _| {
        if let Some(f) = file_of(ui) {
            copy(ui, f);
        }
    })
    .keys("Ctrl+K Ctrl+Shift+C")
    .enabled(file));
    add(command(REVEAL, "File", "Reveal Active File in Explorer View", reveal).enabled(file));

    // The tab's menu, as VS Code's.
    let tab = |id: &str, title: &str, group: &str, order: i32| {
        menus::add(Menu::EditorTab, item(id).title(title).group(group).order(order));
    };
    tab(crate::workbench::CLOSE_EDITOR, "Close", "1_close", 10);
    tab(CLOSE_OTHERS, "Close Others", "1_close", 20);
    tab(CLOSE_RIGHT, "Close to the Right", "1_close", 30);
    tab(CLOSE_SAVED, "Close Saved", "1_close", 40);
    tab(CLOSE_GROUP, "Close All", "1_close", 50);
    tab(COPY_PATH, "Copy Path", "2_copy", 10);
    tab(COPY_RELATIVE, "Copy Relative Path", "2_copy", 20);
    tab(REVEAL, "Reveal in Explorer View", "3_reveal", 10);
    menus::add(
        Menu::EditorTab,
        item(KEEP)
            .title("Keep Open")
            .group("4_keep")
            .order(10)
            .when(|ui| target(ui).is_some_and(|(g, p)| is_preview(ui, g, &p))),
    );
    tab(PIN, "Pin", "4_keep", 20);
    tab(UNPIN, "Unpin", "4_keep", 30);
    tab("workbench.action.splitEditorRight", "Split Right", "5_split", 10);
    tab("workbench.action.splitEditorDown", "Split Down", "5_split", 20);
    tab("workbench.action.moveEditorToNextGroup", "Move to Next Group", "6_move", 10);
    tab("workbench.action.moveEditorToPreviousGroup", "Move to Previous Group", "6_move", 20);

    // A file asked for as a preview, come: its group's preview, in place of the one before (unless that one was
    // kept meanwhile); a tab closed once it is saved.
    crate::ext::after("editor.tabs", |ui, m, _| {
        let st = state(ui);
        let path = m["path"].as_str().unwrap_or("");
        match m["type"].as_str().unwrap_or("") {
            "file" if st.asked.with_untracked(|a| a.contains(path)) => {
                st.asked.update(|a| {
                    a.remove(path);
                });
                if m["error"].is_string() || ui.tab(path).is_none() {
                    return;
                }
                let group = groups::holder(ui, path);
                let old = st.preview.with_untracked(|p| p.get(&group).cloned());
                st.preview.update(|p| {
                    p.insert(group, path.to_string());
                });
                if let Some(old) = old.filter(|o| o != path)
                    && ui.tab(&old).is_some_and(|t| !t.dirty.get_untracked())
                    && groups::groups(ui)
                        .model
                        .with_untracked(|m| m.group(group).is_some_and(|g| g.tabs.contains(&old)))
                {
                    groups::close_in(ui, group, &old);
                    groups::show(ui, group, path);
                }
            }
            "saved" if st.closing.with_untracked(|c| c.contains(path)) => {
                st.closing.update(|c| {
                    c.remove(path);
                });
                if m["error"].is_null() {
                    ui.close_now(path);
                }
            }
            _ => {}
        }
    });
    // A tab closed is no longer pinned nor a preview.
    ui.scope.create_effect(move |_| {
        let open: HashSet<String> = ui.tabs.with(|t| t.iter().map(|t| t.path.clone()).collect());
        let st = state(ui);
        if st.pinned.with_untracked(|p| p.iter().any(|p| !open.contains(p))) {
            st.pinned.update(|p| p.retain(|p| open.contains(p)));
        }
        if st.preview.with_untracked(|m| m.values().any(|p| !open.contains(p))) {
            st.preview.update(|m| m.retain(|_, p| open.contains(p)));
        }
    });
}

/// The session's messages that are this part's: none is taken here (what it follows is in `ext::after`).
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use serde_json::json;

    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|p| p.to_string()).collect()
    }

    fn set(v: &[&str]) -> HashSet<String> {
        v.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn pinned_tabs_come_first_and_a_move_keeps_the_others_in_their_places() {
        assert_eq!(in_order(&s(&["a", "b", "c", "d"]), &set(&["c"])), ["c", "a", "b", "d"]);
        // The group has b, d of a b c d: d before b.
        assert_eq!(arrange(&s(&["a", "b", "c", "d"]), &s(&["d", "b"])), ["a", "d", "c", "b"]);
        assert_eq!(put_at(&s(&["a", "b", "c"]), "c", 0), ["c", "a", "b"]);
        assert_eq!(put_at(&s(&["a", "b", "c"]), "a", 9), ["b", "c", "a"]);
    }

    #[test]
    fn the_close_commands_take_their_tabs_and_never_a_pinned_one() {
        let order = s(&["p", "a", "b", "c"]);
        let pinned = set(&["p"]);
        let dirty = |p: &str| p == "b";
        assert_eq!(to_close(&order, "a", Close::Others, &pinned, dirty), ["b", "c"]);
        assert_eq!(to_close(&order, "a", Close::Right, &pinned, dirty), ["b", "c"]);
        assert_eq!(to_close(&order, "a", Close::Saved, &pinned, dirty), ["a", "c"]);
        assert_eq!(to_close(&order, "p", Close::All, &pinned, dirty), ["a", "b", "c"]);
        assert_eq!(names(&s(&["a"])), "a");
        assert_eq!(names(&s(&["a", "b", "c"])), "a, b and c");
    }

    #[test]
    fn a_tab_dropped_goes_to_its_group_or_a_new_one_at_the_edge() {
        let mut m = Model::default();
        let order = s(&["a", "b"]);
        m.reconcile(&order, Some("a"));
        let first = m.focused;
        // Dropped at the right edge of its own group: a group of its own there.
        assert!(put(&mut m, first, "b", &Drop::Split(first, false, true), &order));
        assert_eq!(m.groups.len(), 2);
        let second = m.focused;
        assert_eq!(m.group(second).map(|g| g.tabs.clone()), Some(s(&["b"])));
        assert_eq!(m.group(first).map(|g| g.tabs.clone()), Some(s(&["a"])));
        // Above or below, when the groups are side by side: no.
        assert!(!put(&mut m, second, "b", &Drop::Split(first, true, true), &order));
        // Dropped in the first: the second, left with nothing, goes.
        assert!(put(&mut m, second, "b", &Drop::By(first, "a".into(), false), &order));
        assert_eq!(m.groups.len(), 1);
        assert_eq!(m.current().tabs, s(&["a", "b"]));
        assert_eq!(m.shown().as_deref(), Some("b"));
    }

    fn window() -> (Ui, Send, Rc<RefCell<Vec<Value>>>) {
        let ui = Ui::new();
        let asked = Rc::new(RefCell::new(Vec::<Value>::new()));
        let send: Send = {
            let asked = asked.clone();
            Rc::new(move |v| asked.borrow_mut().push(v))
        };
        ui.out.set(Some(send.clone()));
        register(ui, &send);
        (ui, send, asked)
    }

    fn file(ui: Ui, send: &Send, path: &str) {
        let m = json!({"type": "file", "path": path, "text": "x\n", "modified": 1});
        ui.take(&m, send);
    }

    fn open(ui: Ui) -> Vec<String> {
        ui.tabs.with_untracked(|t| t.iter().map(|t| t.path.clone()).collect())
    }

    #[test]
    fn a_preview_takes_the_place_of_the_one_before_until_it_is_kept() {
        let (ui, send, _) = window();
        open_preview(ui, "a.rs", &send);
        file(ui, &send, "a.rs");
        open_preview(ui, "b.rs", &send);
        file(ui, &send, "b.rs");
        assert_eq!(open(ui), ["b.rs"]);
        assert_eq!(ui.active.get_untracked().as_deref(), Some("b.rs"));
        keep(ui, "b.rs");
        open_preview(ui, "c.rs", &send);
        file(ui, &send, "c.rs");
        assert_eq!(open(ui), ["b.rs", "c.rs"]);
        // The new one is the group's preview.
        state(ui).preview.with_untracked(|p| assert!(p.values().any(|p| p == "c.rs")));
    }

    #[test]
    fn files_that_come_without_being_asked_as_previews_all_stay() {
        let (ui, send, _) = window();
        for p in ["a.rs", "b.rs", "c.rs"] {
            file(ui, &send, p);
        }
        assert_eq!(open(ui), ["a.rs", "b.rs", "c.rs"]);
        assert!(state(ui).preview.with_untracked(HashMap::is_empty));
    }

    #[test]
    fn pinning_puts_a_tab_first_and_the_close_commands_leave_it() {
        let (ui, send, _) = window();
        for p in ["a.rs", "b.rs", "c.rs"] {
            file(ui, &send, p);
        }
        groups::show(ui, groups::focused(ui), "c.rs");
        commands::run(ui, &send, PIN);
        assert_eq!(order_of(ui, groups::focused(ui)), ["c.rs", "a.rs", "b.rs"]);
        commands::run(ui, &send, "workbench.action.moveEditorRightInGroup");
        assert_eq!(order_of(ui, groups::focused(ui)), ["c.rs", "a.rs", "b.rs"]);
        groups::show(ui, groups::focused(ui), "a.rs");
        commands::run(ui, &send, "workbench.action.moveEditorRightInGroup");
        assert_eq!(order_of(ui, groups::focused(ui)), ["c.rs", "b.rs", "a.rs"]);
        commands::run(ui, &send, CLOSE_GROUP);
        assert_eq!(open(ui), ["c.rs"]);
    }

    #[test]
    fn closing_several_with_changes_asks_once_and_saves_before_closing() {
        let (ui, send, asked) = window();
        for p in ["a.rs", "b.rs"] {
            file(ui, &send, p);
        }
        ui.tab("a.rs").expect("open").dirty.set(true);
        close_many(ui, vec![(groups::focused(ui), "a.rs".into()), (groups::focused(ui), "b.rs".into())]);
        // Asked: nothing closed yet. Save: b closes, a once saved.
        assert_eq!(open(ui).len(), 2);
        finish(ui, &[], &s(&["a.rs", "b.rs"]), true);
        assert_eq!(open(ui), ["a.rs"]);
        assert_eq!(asked.borrow().last().and_then(|m| m["type"].as_str()), Some("save"));
        let saved = json!({"type": "saved", "path": "a.rs", "modified": 2});
        ui.take(&saved, &send);
        assert!(open(ui).is_empty());
    }

    #[test]
    fn the_keys_bound_here_are_read() {
        for k in [
            "Ctrl+K Enter",
            "Ctrl+K Shift+Enter",
            "Ctrl+K P",
            "Ctrl+K U",
            "Ctrl+K Ctrl+Shift+C",
            "Alt+1",
            "Ctrl+Shift+PageUp",
            "Ctrl+Alt+PageDown",
            "Ctrl+K Ctrl+W",
            "Ctrl+Alt+W",
        ] {
            assert!(commands::parse_keys(k).is_some(), "{k}");
        }
    }
}
