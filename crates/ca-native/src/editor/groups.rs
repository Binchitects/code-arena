//! The editor groups, as VS Code's: the editor's place split in up to eight groups in a row (side by side) or a
//! column (one under the other), each with its own tabs and its own tab shown, a divider between two (dragged, or
//! moved 5% by the arrow keys while it has the keys; each group keeps at least 10%; a double click shares the room
//! evenly). A group whose last tab closes goes. One group has the keys at a time: the workbench's commands (Save,
//! Close Editor, Find) act on it.
//!
//! A file has one document however many groups show it (typed in one, the others follow): `ui.tabs` holds every
//! tab once, in the order the strips show them, and `ui.active` is the tab the focused group shows. Both stay what
//! the rest of the window reads and writes: a tab put in `ui.tabs` joins the focused group, a tab gone from it
//! leaves every group, and a tab made `ui.active` is shown in the focused group, or in the group that has it when
//! the focused one has not (so a kept layout's files land where they were). A group's strip shows its tabs in
//! the order of `ui.tabs`.
//!
//! Kept with the layout (`view_state`'s `groups`): the groups, their tabs, the tab each shows, their sizes, the
//! orientation, the focused group, and each tab's caret and scroll in its group; all come back after a restart.

use std::collections::HashMap;

use floem::event::{Event, EventListener};
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::editor::Editor;
use floem::views::{Decorators, dyn_stack, empty, stack};
use floem::{IntoView, View};
use serde_json::{Value, json};

use crate::commands::{self, command};
use crate::state::{Send, Ui};

/// The most groups there are.
pub const MOST: usize = 8;
/// The least share of the room a group keeps when a divider moves.
pub const LEAST: f64 = 0.10;
/// How far the arrow keys move a divider.
pub const STEP: f64 = 0.05;

/// The group to focus and the tab each group shows, of a kept layout.
type Kept = (u64, Vec<(u64, Option<String>)>);

/// Where each tab was left in each group (by its id), of a kept layout.
pub type KeptPlaces = Vec<(u64, String, Place)>;

/// One editor group: its tabs (their paths), the one it shows, its share of the room.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: u64,
    pub tabs: Vec<String>,
    pub active: Option<String>,
    pub share: f64,
}

/// The groups, what has the keys, and how they are laid out. Plain data: what changes it is tested without a
/// window.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// At least one, in the order drawn (left to right, or top to bottom).
    pub groups: Vec<Group>,
    /// The id of the group with the keys.
    pub focused: u64,
    /// One under the other (a column), else side by side (a row).
    pub vertical: bool,
    next: u64,
    /// While a kept layout comes back: the groups each tab not come yet is kept for.
    waiting: HashMap<String, Vec<u64>>,
    /// While a kept layout comes back: the group to focus and the tab each group shows, once all have come.
    kept: Option<Kept>,
}

impl Default for Model {
    fn default() -> Self {
        Model {
            groups: vec![Group { id: 1, tabs: Vec::new(), active: None, share: 1.0 }],
            focused: 1,
            vertical: false,
            next: 2,
            waiting: HashMap::new(),
            kept: None,
        }
    }
}

/// The tab shown in place of one gone from a group: the next one still there, else the one before it, else the first.
fn neighbour(old: &[String], gone: Option<&str>, now: &[String]) -> Option<String> {
    let Some(at) = gone.and_then(|g| old.iter().position(|p| p == g)) else { return now.first().cloned() };
    let has = |p: &&String| now.contains(p);
    old.iter().skip(at + 1).find(has).or_else(|| old[..at].iter().rev().find(has)).or(now.first()).cloned()
}

impl Model {
    /// Where the focused group is among the groups.
    pub fn at(&self) -> usize {
        self.groups.iter().position(|g| g.id == self.focused).unwrap_or(0)
    }

    pub fn group(&self, id: u64) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    fn group_mut(&mut self, id: u64) -> Option<&mut Group> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// The focused group.
    pub fn current(&self) -> &Group {
        &self.groups[self.at()]
    }

    /// The tab the focused group shows.
    pub fn shown(&self) -> Option<String> {
        self.current().active.clone()
    }

    /// The shares made to come to the whole room again (evenly when they make nothing).
    fn even_out(&mut self) {
        let sum: f64 = self.groups.iter().map(|g| g.share).filter(|s| s.is_finite() && *s > 0.0).sum();
        let n = self.groups.len() as f64;
        for g in &mut self.groups {
            g.share = if sum > 0.0 && g.share.is_finite() && g.share > 0.0 { g.share / sum } else { 1.0 / n };
        }
    }

    /// The room shared evenly between the groups (a divider double-clicked).
    pub fn share_evenly(&mut self) {
        let n = self.groups.len() as f64;
        for g in &mut self.groups {
            g.share = 1.0 / n;
        }
    }

    /// A new group beside the focused one (after it, else before it), showing the tab it shows, and focused; the
    /// focused group's room is halved between the two. None when there is no tab to show or eight groups already.
    pub fn split(&mut self, after: bool) -> Option<u64> {
        let shown = self.shown()?;
        if self.groups.len() >= MOST {
            return None;
        }
        let at = self.at();
        let half = self.groups[at].share / 2.0;
        self.groups[at].share = half;
        let id = self.next;
        self.next += 1;
        let g = Group { id, tabs: vec![shown.clone()], active: Some(shown), share: half };
        self.groups.insert(if after { at + 1 } else { at }, g);
        self.focused = id;
        Some(id)
    }

    /// The layout turned to one under the other (true) or side by side, when there is a single group to split.
    pub fn orient(&mut self, vertical: bool) {
        if self.groups.len() == 1 {
            self.vertical = vertical;
        }
    }

    /// Every group's tabs in the focused group, the only one left.
    pub fn join_all(&mut self, order: &[String]) {
        let at = self.at();
        let mut all: Vec<String> = Vec::new();
        for g in &self.groups {
            for p in &g.tabs {
                if !all.contains(p) {
                    all.push(p.clone());
                }
            }
        }
        all.sort_by_key(|p| order.iter().position(|o| o == p).unwrap_or(usize::MAX));
        let mut kept = self.groups.remove(at);
        kept.tabs = all;
        kept.share = 1.0;
        self.groups = vec![kept];
    }

    /// A group gone (never the last one): the group before it focused when it was, else the one after.
    pub fn remove(&mut self, id: u64) {
        if self.groups.len() < 2 {
            return;
        }
        let Some(at) = self.groups.iter().position(|g| g.id == id) else { return };
        self.groups.remove(at);
        if self.focused == id {
            self.focused = self.groups[at.saturating_sub(1)].id;
        }
        self.even_out();
    }

    /// A tab gone from one group (the document stays open in the others): the group shows its neighbour, and goes
    /// when it had no other tab.
    pub fn drop_tab(&mut self, id: u64, path: &str) {
        let Some(g) = self.group_mut(id) else { return };
        let old = g.tabs.clone();
        g.tabs.retain(|p| p != path);
        if g.active.as_deref() == Some(path) {
            g.active = neighbour(&old, Some(path), &g.tabs);
        }
        if g.tabs.is_empty() {
            self.remove(id);
        }
    }

    /// The focused group's tab moved to the next group (or the one before); moved past the last, it makes a new
    /// group there. The tab is shown and focused where it went; a group left with no tab goes.
    pub fn move_shown(&mut self, forward: bool, order: &[String]) -> bool {
        let Some(path) = self.shown() else { return false };
        let at = self.at();
        let from = self.focused;
        let to = if forward {
            match self.groups.get(at + 1) {
                Some(g) => g.id,
                None if self.groups.len() < MOST && self.current().tabs.len() > 1 => match self.split(true) {
                    Some(id) => id,
                    None => return false,
                },
                None => return false,
            }
        } else {
            match at.checked_sub(1) {
                Some(b) => self.groups[b].id,
                None => return false,
            }
        };
        if let Some(g) = self.group_mut(to) {
            if !g.tabs.contains(&path) {
                g.tabs.push(path.clone());
                g.tabs.sort_by_key(|p| order.iter().position(|o| o == p).unwrap_or(usize::MAX));
            }
            g.active = Some(path.clone());
        }
        self.focused = to;
        self.drop_tab(from, &path);
        self.focused = to;
        true
    }

    /// The n-th group (from 0) focused; one past the last splits the focused group to the right (or below).
    pub fn focus_at(&mut self, n: usize) -> bool {
        if let Some(g) = self.groups.get(n) {
            let changed = self.focused != g.id;
            self.focused = g.id;
            return changed;
        }
        n == self.groups.len() && self.split(true).is_some()
    }

    /// The next group focused (or the one before), round from the last to the first.
    pub fn focus_step(&mut self, forward: bool, round: bool) -> bool {
        let (at, n) = (self.at(), self.groups.len());
        let to = match (forward, round) {
            (true, _) if at + 1 < n => at + 1,
            (false, _) if at > 0 => at - 1,
            (true, true) => 0,
            (false, true) => n - 1,
            _ => return false,
        };
        self.focus_at(to)
    }

    /// Where the divider after the i-th group is, as a share of the room from its start.
    pub fn boundary(&self, i: usize) -> f64 {
        self.groups.iter().take(i + 1).map(|g| g.share).sum()
    }

    /// The divider after the i-th group moved to a place (a share of the room from its start); each of the two
    /// groups beside it keeps at least its least.
    pub fn drag(&mut self, i: usize, to: f64) {
        if i + 1 >= self.groups.len() || !to.is_finite() {
            return;
        }
        let start = self.boundary(i) - self.groups[i].share;
        let end = start + self.groups[i].share + self.groups[i + 1].share;
        if end - start < 2.0 * LEAST {
            return;
        }
        let b = to.clamp(start + LEAST, end - LEAST);
        self.groups[i].share = b - start;
        self.groups[i + 1].share = end - b;
    }

    /// A tab's path changed (renamed or moved, or a folder it is in): it stays in its groups.
    pub fn rename(&mut self, now: impl Fn(&str) -> Option<String>) {
        for g in &mut self.groups {
            for p in g.tabs.iter_mut().chain(g.active.iter_mut()) {
                if let Some(n) = now(p) {
                    *p = n;
                }
            }
        }
        self.waiting = self.waiting.drain().map(|(p, ids)| (now(&p).unwrap_or(p), ids)).collect();
    }

    /// The groups made to agree with the window's tabs (`ui.tabs`, in their order) and the tab shown (`ui.active`):
    /// a tab gone leaves every group (each shows its neighbour, and an empty group goes), a new one joins the
    /// focused group (or the groups a kept layout has it in), the tab shown is the focused group's (the group that
    /// has it is focused when the focused one has not). What comes back is the tab the focused group shows, what
    /// `ui.active` is to be. Doing it twice changes nothing.
    pub fn reconcile(&mut self, tabs: &[String], active: Option<&str>) -> Option<String> {
        let index = |p: &str| tabs.iter().position(|t| t == p);
        // The tabs a kept layout waited for, come.
        for p in tabs {
            if let Some(ids) = self.waiting.remove(p) {
                for id in ids {
                    if let Some(g) = self.group_mut(id)
                        && !g.tabs.contains(p)
                    {
                        g.tabs.push(p.clone());
                    }
                }
            }
        }
        let waiting = &self.waiting;
        for g in &mut self.groups {
            let old = g.tabs.clone();
            let gid = g.id;
            g.tabs.retain(|p| index(p).is_some() || waiting.get(p).is_some_and(|w| w.contains(&gid)));
            g.tabs.sort_by_key(|p| index(p).unwrap_or(usize::MAX));
            if g.active.as_ref().is_some_and(|a| index(a).is_none()) || g.active.is_none() {
                let gone = g.active.take();
                g.active = neighbour(&old, gone.as_deref(), &g.tabs).filter(|p| index(p).is_some());
            }
        }
        // New tabs join the focused group.
        let at = self.at();
        let fresh: Vec<String> =
            tabs.iter().filter(|p| !self.groups.iter().any(|g| g.tabs.contains(p))).cloned().collect();
        if !fresh.is_empty() {
            let g = &mut self.groups[at];
            g.tabs.extend(fresh);
            g.tabs.sort_by_key(|p| index(p).unwrap_or(usize::MAX));
            if g.active.is_none() {
                g.active = g.tabs.iter().find(|p| index(p).is_some()).cloned();
            }
        }
        // The tab asked to be shown: in the focused group, or in the group that has it.
        if let Some(p) = active.filter(|p| index(p).is_some()) {
            if !self.current().tabs.iter().any(|t| t == p)
                && let Some(g) = self.groups.iter().find(|g| g.tabs.iter().any(|t| t == p))
            {
                self.focused = g.id;
            }
            let at = self.at();
            self.groups[at].active = Some(p.to_string());
        }
        // A group with no tab goes (but the last, and one a kept layout waits on).
        let empty: Vec<u64> = self
            .groups
            .iter()
            .filter(|g| g.tabs.is_empty() && !self.waiting.values().any(|w| w.contains(&g.id)))
            .map(|g| g.id)
            .collect();
        for id in empty {
            self.remove(id);
        }
        self.shown()
    }

    /// A kept layout come back whole: each group shows the tab it showed and the group that had the keys has
    /// them. True when this happened now.
    pub fn settle(&mut self) -> bool {
        if !self.waiting.is_empty() {
            return false;
        }
        let Some((focused, actives)) = self.kept.take() else { return false };
        for (id, a) in actives {
            if let Some(g) = self.group_mut(id)
                && let Some(a) = a.filter(|a| g.tabs.contains(a))
            {
                g.active = Some(a);
            }
        }
        if self.group(focused).is_some() {
            self.focused = focused;
        }
        true
    }

    /// No longer waiting for a tab of a kept layout (it could not be opened).
    pub fn give_up(&mut self, path: &str) {
        self.waiting.remove(path);
    }

    /// What is kept with the layout; `places` gives a tab's caret and scroll in a group.
    pub fn to_json(&self, places: impl Fn(u64, &str) -> Option<Place>) -> Value {
        let list: Vec<Value> = self
            .groups
            .iter()
            .map(|g| {
                let kept: serde_json::Map<String, Value> = g
                    .tabs
                    .iter()
                    .filter_map(|p| places(g.id, p).map(|at| (p.clone(), json!([at.line, at.col, at.x, at.y]))))
                    .collect();
                json!({"tabs": g.tabs, "active": g.active, "share": g.share, "places": kept})
            })
            .collect();
        json!({"list": list, "focused": self.at(), "vertical": self.vertical})
    }

    /// A kept layout read (`to_json`'s): the groups are there at once, empty, and wait for their tabs (the tabs
    /// kept, `tabs`, among them). None for a layout kept with one group or none: it needs no waiting.
    pub fn from_json(v: &Value, tabs: &[String]) -> Option<(Model, KeptPlaces)> {
        let list = v["list"].as_array().filter(|l| l.len() > 1)?;
        let mut m = Model { groups: Vec::new(), vertical: v["vertical"] == true, ..Model::default() };
        let mut places = Vec::new();
        let mut actives = Vec::new();
        for (n, g) in list.iter().take(MOST).enumerate() {
            let id = n as u64 + 1;
            let mine: Vec<String> = g["tabs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|p| tabs.iter().any(|t| t == p))
                .map(str::to_string)
                .collect();
            if mine.is_empty() {
                continue;
            }
            for p in &mine {
                m.waiting.entry(p.clone()).or_default().push(id);
            }
            for (p, at) in g["places"].as_object().into_iter().flatten() {
                let n = |i: usize| at[i].as_f64().unwrap_or(0.0);
                places.push((id, p.clone(), Place { line: n(0) as usize, col: n(1) as usize, x: n(2), y: n(3) }));
            }
            actives.push((id, g["active"].as_str().map(str::to_string)));
            m.groups.push(Group { id, tabs: Vec::new(), active: None, share: g["share"].as_f64().unwrap_or(0.0) });
        }
        if m.groups.len() < 2 {
            return None;
        }
        m.next = list.len() as u64 + 1;
        let focused = v["focused"].as_u64().unwrap_or(0) as usize;
        let focused = list.get(focused).and(m.groups.iter().find(|g| g.id == focused as u64 + 1)).map(|g| g.id);
        m.focused = m.groups[0].id;
        m.kept = Some((focused.unwrap_or(m.focused), actives));
        m.even_out();
        Some((m, places))
    }
}

/// Where a tab was left in a group: its caret (line and column, from 0) and how far it was scrolled.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Place {
    pub line: usize,
    pub col: usize,
    pub x: f64,
    pub y: f64,
}

impl Place {
    /// Where an editor is now.
    pub fn of(editor: &Editor) -> Place {
        let (line, col) = editor.offset_to_line_col(editor.cursor.with_untracked(|c| c.offset()));
        let o = editor.viewport.get_untracked().origin();
        Place { line, col, x: o.x, y: o.y }
    }

    /// An editor put back where it was left: the caret at once, the scroll once it is laid out.
    /// `done` is set once the scroll is there too (until then, where the editor is is not where it was left).
    pub fn put(&self, editor: &Editor, done: RwSignal<bool>) {
        let line = self.line.min(editor.last_line());
        let at = editor.offset_of_line_col(line, self.col.min(editor.line_end_col(line, true)));
        editor.cursor.update(|c| c.set_insert(floem::views::editor::core::selection::Selection::caret(at)));
        let (e, to) = (editor.clone(), floem::kurbo::Vec2::new(self.x, self.y));
        floem::action::exec_after(std::time::Duration::from_millis(30), move |_| {
            if e.cursor.try_get_untracked().is_some() {
                e.scroll_to.set(Some(to));
                // Scrolled at the next layout.
                floem::action::exec_after(std::time::Duration::from_millis(100), move |_| {
                    if done.try_get_untracked().is_some() {
                        done.set(true);
                    }
                });
            }
        });
    }
}

/// The window's groups.
#[derive(Clone, Copy)]
pub struct Groups {
    pub model: RwSignal<Model>,
    /// The editor each group has on screen (a group showing a tab that is not a file's text has none).
    pub editors: RwSignal<HashMap<u64, (String, Editor)>>,
    /// Where each tab was left in each group.
    pub places: RwSignal<HashMap<(u64, String), Place>>,
    /// How wide (or high, one under the other) the groups' room is, in pixels.
    pub room: RwSignal<f64>,
    made: RwSignal<bool>,
}

impl crate::ext::State for Groups {
    fn new(ui: Ui) -> Self {
        Groups {
            model: ui.scope.create_rw_signal(Model::default()),
            editors: ui.scope.create_rw_signal(HashMap::new()),
            places: ui.scope.create_rw_signal(HashMap::new()),
            room: ui.scope.create_rw_signal(0.0),
            made: ui.scope.create_rw_signal(false),
        }
    }
}

/// The window's groups, following its tabs from the first time they are asked for (`Ui::new` asks).
pub fn groups(ui: Ui) -> Groups {
    let g = ui.state::<Groups>();
    if !g.made.get_untracked() {
        g.made.set(true);
        follow(ui, g);
    }
    g
}

/// The groups kept in step with `ui.tabs` and `ui.active`, whoever changes them.
fn follow(ui: Ui, g: Groups) {
    ui.scope.create_effect(move |_| {
        let tabs: Vec<String> = ui.tabs.with(|t| t.iter().map(|t| t.path.clone()).collect());
        let active = ui.active.get();
        // Read first, changed after: a change below runs other effects, and what is read after them is not followed.
        let mut m = g.model.get_untracked();
        let shown = m.reconcile(&tabs, active.as_deref());
        set_model(ui, g, m);
        if shown != active {
            ui.active.set(shown);
        }
    });
}

/// The groups changed, when they did; the layout is then kept a moment after, as any change of it is.
fn set_model(ui: Ui, g: Groups, m: Model) {
    if g.model.with_untracked(|old| *old != m) {
        g.model.set(m);
        // Only the keeping of the layout follows the sizes' map as a whole (`lib.rs`): it is told the layout changed.
        ui.sizes.update(|_| {});
    }
}

/// The groups changed by a command; `ui.active` made what the focused group shows, and the keys given to it.
pub fn change(ui: Ui, how: impl FnOnce(&mut Model, &[String])) {
    let g = groups(ui);
    let order: Vec<String> = ui.tabs.with_untracked(|t| t.iter().map(|t| t.path.clone()).collect());
    let mut m = g.model.get_untracked();
    let before = m.focused;
    how(&mut m, &order);
    let shown = m.shown();
    let focused = m.focused;
    set_model(ui, g, m);
    if ui.active.with_untracked(|a| *a != shown) {
        ui.active.set(shown);
    }
    if focused != before {
        give_keys(ui, focused);
    }
}

/// The keys given to a group's editor, once it is drawn.
fn give_keys(ui: Ui, id: u64) {
    floem::action::exec_after(std::time::Duration::from_millis(20), move |_| {
        if let Some((_, e)) = groups(ui).editors.with_untracked(|m| m.get(&id).cloned()) {
            super::focus(&e);
        }
    });
}

/// The group with the keys.
pub fn focused(ui: Ui) -> u64 {
    groups(ui).model.with_untracked(|m| m.focused)
}

/// A group given the keys (a click in it), when it has not them.
pub fn focus(ui: Ui, id: u64) {
    if focused(ui) != id {
        change(ui, |m, _| {
            if m.group(id).is_some() {
                m.focused = id;
            }
        });
    }
}

/// A tab of a group shown in it (its tab clicked), the group focused.
pub fn show(ui: Ui, id: u64, path: &str) {
    change(ui, |m, _| {
        if let Some(g) = m.group_mut(id)
            && g.tabs.iter().any(|t| t == path)
        {
            g.active = Some(path.to_string());
            m.focused = id;
        }
    });
}

/// A tab closed in one group: when another group shows its document too only this group's tab goes, else the
/// document closes (asked about first when it has changes not saved).
pub fn close_in(ui: Ui, id: u64, path: &str) {
    let elsewhere =
        groups(ui).model.with_untracked(|m| m.groups.iter().any(|g| g.id != id && g.tabs.iter().any(|t| t == path)));
    if elsewhere {
        change(ui, |m, _| m.drop_tab(id, path));
    } else {
        focus(ui, id);
        ui.close_document(path);
    }
}

/// The group a tab is closed in by the workbench (Close Editor, Ctrl+W): the focused group when it has the tab,
/// else the first group that has it.
pub fn holder(ui: Ui, path: &str) -> u64 {
    groups(ui).model.with_untracked(|m| {
        let has = |g: &Group| g.tabs.iter().any(|t| t == path);
        if has(m.current()) { m.focused } else { m.groups.iter().find(|g| has(g)).map_or(m.focused, |g| g.id) }
    })
}

/// A file opened to the side: in the group after the focused one (made when there is none), and focused there.
pub fn open_to_side(ui: Ui, path: &str, send: &Send) {
    let open = ui.tab(path).is_some();
    change(ui, |m, order| {
        if m.groups.len() == 1 {
            if m.split(true).is_none() {
                return;
            }
            // The new group shows the file asked for, not the one split.
            if open && let Some(g) = m.group_mut(m.focused) {
                g.tabs = vec![path.to_string()];
                g.active = Some(path.to_string());
            }
        } else {
            let to = (m.at() + 1).min(m.groups.len() - 1);
            let to = if to == m.at() { m.at().saturating_sub(1) } else { to };
            m.focused = m.groups[to].id;
            if open && let Some(g) = m.group_mut(m.focused) {
                if !g.tabs.iter().any(|t| t == path) {
                    g.tabs.push(path.to_string());
                    g.tabs.sort_by_key(|p| order.iter().position(|o| o == p).unwrap_or(usize::MAX));
                }
                g.active = Some(path.to_string());
            }
        }
    });
    if !open {
        // It comes as a new tab, and joins the focused group: the one just made or gone to.
        ui.open(path, send);
    }
}

/// The focused group split: after it (Right, Down) or before it (Left, Up), turned to that way when it is the only
/// group; the new group shows the same tab, where it was in the first.
pub fn split(ui: Ui, vertical: Option<bool>, after: bool) {
    let g = groups(ui);
    let from = focused(ui);
    let live = g.editors.with_untracked(|e| e.get(&from).map(|(p, e)| (p.clone(), Place::of(e))));
    change(ui, |m, _| {
        if let Some(v) = vertical {
            m.orient(v);
        }
        if let Some(id) = m.split(after)
            && let Some((path, at)) = live.clone().filter(|(p, _)| m.shown().as_ref() == Some(p))
        {
            g.places.update(|p| {
                p.insert((id, path), at);
            });
        }
    });
}

/// Where each tab was left, for the layout kept: the place kept for it, or, on screen, where it is now.
fn place_of(g: Groups, id: u64, path: &str) -> Option<Place> {
    g.editors
        .with_untracked(|e| e.get(&id).filter(|(p, _)| p == path).map(|(_, e)| Place::of(e)))
        .or_else(|| g.places.with_untracked(|p| p.get(&(id, path.to_string())).copied()))
}

/// What is kept of the groups with the layout.
pub fn kept(ui: Ui) -> Value {
    let g = groups(ui);
    g.model.with_untracked(|m| m.to_json(|id, p| place_of(g, id, p)))
}

/// The groups of a kept layout made again; the tabs (`tabs`, the layout's tabs) come after and land in their
/// groups. A layout with one group needs nothing: its tabs come to the only group.
pub fn restore(ui: Ui, v: &Value) {
    let g = groups(ui);
    let tabs: Vec<String> = v["tabs"]
        .as_array()
        .or(v["editors"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    match Model::from_json(&v["groups"], &tabs) {
        Some((mut m, places)) => {
            g.places.set(places.into_iter().map(|(id, p, at)| ((id, p), at)).collect());
            let have: Vec<String> = ui.tabs.with_untracked(|t| t.iter().map(|t| t.path.clone()).collect());
            m.reconcile(&have, None);
            set_model(ui, g, m);
            settle(ui);
        }
        None => {
            // One group: where its tabs were left.
            let id = focused(ui);
            let first = &v["groups"]["list"][0]["places"];
            let places: HashMap<(u64, String), Place> = first
                .as_object()
                .into_iter()
                .flatten()
                .map(|(p, at)| {
                    let n = |i: usize| at[i].as_f64().unwrap_or(0.0);
                    ((id, p.clone()), Place { line: n(0) as usize, col: n(1) as usize, x: n(2), y: n(3) })
                })
                .collect();
            if !places.is_empty() {
                g.places.set(places);
            }
        }
    }
}

/// Once every tab of a kept layout has come: each group shows what it showed, the focused group has the keys.
fn settle(ui: Ui) {
    let g = groups(ui);
    let mut m = g.model.get_untracked();
    if m.settle() {
        let shown = m.shown();
        set_model(ui, g, m);
        if ui.active.with_untracked(|a| *a != shown) {
            ui.active.set(shown);
        }
    }
}

/// The groups drawn: a row or a column of them, a divider between two, the question before a tab closes over all.
pub fn area(ui: Ui) -> impl IntoView {
    let g = groups(ui);
    let ids =
        floem::reactive::create_memo(move |_| g.model.with(|m| m.groups.iter().map(|g| g.id).collect::<Vec<_>>()));
    let vertical = floem::reactive::create_memo(move |_| g.model.with(|m| m.vertical));
    let row = dyn_stack(move || ids.get(), |id| *id, move |id| one(ui, id))
        .on_resize(move |r| {
            let room = if vertical.get_untracked() { r.height() } else { r.width() };
            if g.room.get_untracked() != room {
                g.room.set(room);
            }
        })
        .style(move |s| {
            let s = s.size_full().min_width(0.0).min_height(0.0);
            if vertical.get() { s.flex_col() } else { s.flex_row() }
        });
    stack((row, super::tabs::close_question(ui))).style(|s| s.size_full().min_width(0.0).min_height(0.0))
}

/// What a group's parts are drawn from: its own tabs, the tab it shows, whether it has the keys.
#[derive(Clone, Copy)]
pub struct Shown {
    pub id: u64,
    pub tabs: Memo<Vec<String>>,
    pub active: Memo<Option<String>>,
    pub focused: Memo<bool>,
}

fn one(ui: Ui, id: u64) -> impl IntoView {
    use floem::reactive::create_memo;
    let g = groups(ui);
    let shown = Shown {
        id,
        tabs: create_memo(move |_| g.model.with(|m| m.group(id).map(|g| g.tabs.clone()).unwrap_or_default())),
        active: create_memo(move |_| g.model.with(|m| m.group(id).and_then(|g| g.active.clone()))),
        focused: create_memo(move |_| g.model.with(|m| m.focused == id)),
    };
    let at = create_memo(move |_| g.model.with(|m| m.groups.iter().position(|g| g.id == id).unwrap_or(0)));
    let share = create_memo(move |_| g.model.with(|m| m.group(id).map_or(1.0, |g| g.share)));
    let vertical = create_memo(move |_| g.model.with(|m| m.vertical));
    let t = ui.colors;
    let body = super::group_view(ui, shown).style(move |s| {
        let s = s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).min_height(0.0);
        // The 1 px line between two groups (editorGroup.border).
        match (at.get() > 0, vertical.get()) {
            (false, _) => s,
            (true, false) => s.border_left(1.0).border_color(t.get().group_border),
            (true, true) => s.border_top(1.0).border_color(t.get().group_border),
        }
    });
    // The divider last: the pointer goes to the topmost part under it.
    stack((body, divider(ui, at, vertical))).on_event_cont(EventListener::PointerDown, move |_| focus(ui, id)).style(
        move |s| {
            let s = s.flex_grow(share.get() as f32).flex_basis(0.0).min_width(0.0).min_height(0.0);
            if vertical.get() { s.flex_col().width_full() } else { s.flex_row().height_full() }
        },
    )
}

/// The divider before a group (none before the first): dragged, or moved by the arrow keys while it has them;
/// a double click shares the room evenly.
fn divider(ui: Ui, at: Memo<usize>, vertical: Memo<bool>) -> impl IntoView {
    let g = groups(ui);
    let t = ui.colors;
    let down = floem::reactive::create_rw_signal(None::<(f64, f64)>);
    let lit = floem::reactive::create_rw_signal(false);
    let view = empty();
    let id = view.id();
    let pos = move |p: floem::kurbo::Point| {
        let o = id.layout_rect().origin();
        if vertical.get_untracked() { o.y + p.y } else { o.x + p.x }
    };
    // The divider after the group before this one.
    let edge = move || at.get_untracked().saturating_sub(1);
    let moved = move |to: f64| change(ui, |m, _| m.drag(edge(), to));
    let nudge = move |by: f64| change(ui, |m, _| m.drag(edge(), m.boundary(edge()) + by));
    view.keyboard_navigable()
        .on_event_stop(EventListener::PointerDown, move |e| {
            if let Event::PointerDown(p) = e {
                down.set(Some((pos(p.pos), g.model.with_untracked(|m| m.boundary(edge())))));
                id.request_active();
                id.request_focus();
            }
        })
        .on_event_stop(EventListener::PointerMove, move |e| {
            let room = g.room.get_untracked();
            if let (Event::PointerMove(p), Some((from, start))) = (e, down.get_untracked())
                && room > 0.0
            {
                moved(start + (pos(p.pos) - from) / room);
            }
        })
        .on_event_stop(EventListener::PointerUp, move |_| down.set(None))
        .on_double_click_stop(move |_| change(ui, |m, _| m.share_evenly()))
        .on_event_cont(EventListener::PointerEnter, move |_| lit.set(true))
        .on_event_cont(EventListener::PointerLeave, move |_| lit.set(false))
        .on_key_down(Key::Named(NamedKey::ArrowLeft), |_| true, move |_| nudge(-STEP))
        .on_key_down(Key::Named(NamedKey::ArrowUp), |_| true, move |_| nudge(-STEP))
        .on_key_down(Key::Named(NamedKey::ArrowRight), |_| true, move |_| nudge(STEP))
        .on_key_down(Key::Named(NamedKey::ArrowDown), |_| true, move |_| nudge(STEP))
        .style(move |s| {
            let c = t.get();
            let on = lit.get() || down.get().is_some();
            let s = if vertical.get() {
                s.height(4.0).width_full().inset_top(0.0).cursor(CursorStyle::RowResize)
            } else {
                s.width(4.0).height_full().inset_left(0.0).cursor(CursorStyle::ColResize)
            };
            let s = s.absolute();
            s.z_index(10)
                .flex_shrink(0.0_f32)
                .background(if on { c.sash } else { floem::peniko::Color::TRANSPARENT })
                .focus_visible(move |s| s.background(c.sash))
                .apply_if(at.get() == 0, |s| s.hide())
        })
}

/// Every open editor, by group ("Group 1", "Group 2"…), to pick one and show it where it is.
fn show_all(ui: Ui) {
    let g = groups(ui);
    let m = g.model.get_untracked();
    let mut items = Vec::new();
    let mut current = String::new();
    for (n, group) in m.groups.iter().enumerate() {
        for p in &group.tabs {
            let Some(tab) = ui.tab(p) else { continue };
            let key = format!("{}\u{0}{p}", group.id);
            if group.id == m.focused && group.active.as_deref() == Some(p.as_str()) {
                current = key.clone();
            }
            let dir = tab.file.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
            let name = super::kinds::title(ui, &tab);
            let item = crate::widgets::quick::item(&key, &name).description(&dir);
            items.push(if m.groups.len() > 1 { item.group(&format!("Group {}", n + 1)) } else { item });
        }
    }
    let pick = crate::widgets::quick::pick("Show All Editors By Appearance")
        .placeholder("Search open editors by name")
        .items(items);
    let pick = if current.is_empty() { pick } else { pick.active(&current) };
    pick.open(ui, move |chosen| {
        if let Some(it) = chosen
            && let Some((id, path)) = it.id.split_once('\u{0}')
            && let Ok(id) = id.parse::<u64>()
        {
            show(ui, id, path);
        }
    });
}

const ORDINALS: [&str; MOST] = ["First", "Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh", "Eighth"];

/// What it adds to the window's registries, once at start.
pub fn register(ui: Ui, _send: &Send) {
    let _ = groups(ui);
    let more = |ui: Ui| groups(ui).model.with_untracked(|m| m.groups.len() > 1);
    let shown = |ui: Ui| ui.active.with_untracked(|a| a.is_some());
    let view = "View";
    commands::add(
        command("workbench.action.splitEditor", view, "Split Editor", |ui, _| split(ui, None, true))
            .keys("Ctrl+\\")
            .enabled(shown),
    );
    commands::add(
        command("workbench.action.splitEditorOrthogonal", view, "Split Editor in Orthogonal Direction", |ui, _| {
            let v = groups(ui).model.with_untracked(|m| m.vertical);
            split(ui, Some(!v), true)
        })
        .keys("Ctrl+K Ctrl+\\")
        .enabled(shown),
    );
    for (id, title, vertical, after) in [
        ("workbench.action.splitEditorRight", "Split Editor Right", false, true),
        ("workbench.action.splitEditorLeft", "Split Editor Left", false, false),
        ("workbench.action.splitEditorDown", "Split Editor Down", true, true),
        ("workbench.action.splitEditorUp", "Split Editor Up", true, false),
    ] {
        commands::add(command(id, view, title, move |ui, _| split(ui, Some(vertical), after)).enabled(shown));
    }
    commands::add(
        command("workbench.action.joinAllGroups", view, "Join All Editor Groups", |ui, _| change(ui, Model::join_all))
            .enabled(more),
    );
    commands::add(
        command("workbench.action.closeGroup", view, "Close Editor Group", |ui, _| close_group(ui)).enabled(more),
    );
    commands::add(
        command(
            "workbench.action.toggleEditorGroupLayout",
            view,
            "Toggle Vertical/Horizontal Editor Layout",
            |ui, _| change(ui, |m, _| m.vertical = !m.vertical),
        )
        .keys("Alt+Shift+0"),
    );
    commands::add(
        command("workbench.action.moveEditorToNextGroup", view, "Move Editor into Next Group", |ui, _| {
            change(ui, |m, order| {
                m.move_shown(true, order);
            })
        })
        .keys("Ctrl+Alt+Right")
        .enabled(shown),
    );
    commands::add(
        command("workbench.action.moveEditorToPreviousGroup", view, "Move Editor into Previous Group", |ui, _| {
            change(ui, |m, order| {
                m.move_shown(false, order);
            })
        })
        .keys("Ctrl+Alt+Left")
        .enabled(shown),
    );
    commands::add(
        command("workbench.action.focusNextGroup", view, "Focus Next Editor Group", |ui, _| {
            change(ui, |m, _| {
                m.focus_step(true, true);
            })
        })
        .enabled(more),
    );
    commands::add(
        command("workbench.action.focusPreviousGroup", view, "Focus Previous Editor Group", |ui, _| {
            change(ui, |m, _| {
                m.focus_step(false, true);
            })
        })
        .enabled(more),
    );
    for (id, title, keys, vertical, forward) in [
        ("workbench.action.focusLeftGroup", "Focus Left Editor Group", "Ctrl+K Ctrl+Left", false, false),
        ("workbench.action.focusRightGroup", "Focus Right Editor Group", "Ctrl+K Ctrl+Right", false, true),
        ("workbench.action.focusAboveGroup", "Focus Editor Group Above", "Ctrl+K Ctrl+Up", true, false),
        ("workbench.action.focusBelowGroup", "Focus Editor Group Below", "Ctrl+K Ctrl+Down", true, true),
    ] {
        commands::add(
            command(id, view, title, move |ui, _| {
                change(ui, |m, _| {
                    if m.vertical == vertical {
                        m.focus_step(forward, false);
                    }
                })
            })
            .keys(keys)
            .enabled(more),
        );
    }
    for (n, nth) in ORDINALS.iter().enumerate() {
        let id = format!("workbench.action.focus{nth}EditorGroup");
        let title = format!("Focus {nth} Editor Group");
        commands::add(
            command(&id, view, &title, move |ui, _| {
                change(ui, |m, _| {
                    m.focus_at(n);
                })
            })
            .keys(&format!("Ctrl+{}", n + 1)),
        );
    }
    commands::add(
        command("workbench.action.showAllEditors", view, "Show All Editors By Appearance", |ui, _| show_all(ui))
            .keys("Ctrl+K Ctrl+P"),
    );
    // The Explorer's file (or the tab shown) opened in the group beside.
    commands::add(command("explorer.openToSide", "File", "Open to the Side", |ui, send| {
        let selected = ui.selected.get_untracked();
        let path = if selected.is_empty() || ui.tree.with_untracked(|t| t.contains_key(&selected)) {
            ui.active.get_untracked().and_then(|a| ui.tab(&a)).filter(|t| t.is_file()).map(|t| t.path)
        } else {
            Some(selected)
        };
        if let Some(p) = path {
            open_to_side(ui, &p, send);
        }
    }));
    crate::menus::add(
        crate::menus::Menu::ExplorerRow,
        crate::menus::item("explorer.openToSide").group("navigation").order(10),
    );
    // A kept layout's tabs land in their groups as they come; once all have, each group shows what it showed.
    crate::ext::after("editor.groups", |ui, m, _| {
        if m["type"] != "file" {
            return;
        }
        let g = groups(ui);
        if g.model.with_untracked(|m| m.kept.is_none()) {
            return;
        }
        if m["error"].is_string()
            && let Some(p) = m["path"].as_str()
        {
            let mut model = g.model.get_untracked();
            model.give_up(p);
            set_model(ui, g, model);
        }
        settle(ui);
    });
}

/// The focused group closed: its tabs that no other group shows close (a tab with changes not saved goes to the
/// group beside instead, so nothing is lost), and the group goes.
fn close_group(ui: Ui) {
    let g = groups(ui);
    let m = g.model.get_untracked();
    if m.groups.len() < 2 {
        return;
    }
    let here = m.current().clone();
    let only: Vec<String> =
        here.tabs.iter().filter(|p| !m.groups.iter().any(|o| o.id != here.id && o.tabs.contains(p))).cloned().collect();
    let dirty = |p: &String| ui.tab(p).is_some_and(|t| t.diff.is_none() && t.dirty.get_untracked());
    let keep: Vec<String> = only.iter().filter(|p| dirty(p)).cloned().collect();
    change(ui, |m, order| {
        let at = m.at();
        let to = m.groups[if at == 0 { 1 } else { at - 1 }].id;
        if let Some(g) = m.group_mut(to) {
            g.tabs.extend(keep.iter().cloned());
            g.tabs.sort_by_key(|p| order.iter().position(|o| o == p).unwrap_or(usize::MAX));
            g.tabs.dedup();
        }
        m.remove(here.id);
    });
    for p in only.iter().filter(|p| !keep.contains(p)) {
        ui.close_now(p);
    }
}

/// A file or a folder moved: its tabs stay in their groups under their new paths (`Ui::moved` asks, before it
/// renames the tabs).
pub fn moved(ui: Ui, from: &str, to: &str) {
    let g = groups(ui);
    let mut model = g.model.get_untracked();
    model.rename(|p| crate::state::moved_path(p, from, to));
    g.places.update(|kept| {
        *kept =
            kept.drain().map(|((id, p), at)| ((id, crate::state::moved_path(&p, from, to).unwrap_or(p)), at)).collect()
    });
    set_model(ui, g, model);
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn new_tabs_join_the_focused_group_and_gone_ones_leave_every_group() {
        let mut m = Model::default();
        assert_eq!(m.reconcile(&paths(&["a", "b"]), Some("b")).as_deref(), Some("b"));
        assert_eq!(m.groups[0].tabs, paths(&["a", "b"]));
        assert_eq!(m.split(true), Some(2));
        m.reconcile(&paths(&["a", "b", "c"]), Some("c"));
        assert_eq!(m.groups[1].tabs, paths(&["b", "c"]));
        assert_eq!(m.groups[0].tabs, paths(&["a", "b"]));
        // b closed: gone from both; the second group shows c, the first a.
        let shown = m.reconcile(&paths(&["a", "c"]), Some("c"));
        assert_eq!((m.groups[0].tabs.clone(), m.groups[1].tabs.clone()), (paths(&["a"]), paths(&["c"])));
        assert_eq!((shown.as_deref(), m.groups[0].active.as_deref()), (Some("c"), Some("a")));
        // c closed: its group goes, the first has the keys.
        assert_eq!(m.reconcile(&paths(&["a"]), Some("c")).as_deref(), Some("a"));
        assert_eq!(m.groups.len(), 1);
        assert_eq!(m.groups[0].share, 1.0);
        // Twice is once.
        let again = m.clone();
        m.reconcile(&paths(&["a"]), Some("a"));
        assert_eq!(m, again);
    }

    #[test]
    fn a_tab_shown_from_outside_is_shown_where_it_is_or_in_the_focused_group() {
        let mut m = Model::default();
        m.reconcile(&paths(&["a", "b"]), Some("a"));
        m.split(true);
        m.groups[0].tabs = paths(&["a"]);
        m.groups[1].tabs = paths(&["b"]);
        m.groups[1].active = Some("b".into());
        m.focused = 1;
        // b is only in the second group: it is shown there and that group focused.
        assert_eq!(m.reconcile(&paths(&["a", "b"]), Some("b")).as_deref(), Some("b"));
        assert_eq!(m.focused, 2);
        // The tab closed in its group, its neighbour shown, the next closed after: the group goes.
        m.drop_tab(2, "b");
        assert_eq!(m.groups.len(), 1);
        assert_eq!(m.focused, 1);
    }

    #[test]
    fn splits_stop_at_eight_and_each_halves_the_focused_room() {
        let mut m = Model::default();
        m.reconcile(&paths(&["a"]), Some("a"));
        for _ in 0..10 {
            m.split(true);
        }
        assert_eq!(m.groups.len(), MOST);
        let sum: f64 = m.groups.iter().map(|g| g.share).sum();
        assert!((sum - 1.0).abs() < 1e-9);
        assert_eq!(m.groups[0].share, 0.5);
        m.join_all(&paths(&["a"]));
        assert_eq!((m.groups.len(), m.groups[0].tabs.clone()), (1, paths(&["a"])));
    }

    #[test]
    fn a_divider_keeps_a_tenth_for_each_side_and_steps_by_a_twentieth() {
        let mut m = Model::default();
        m.reconcile(&paths(&["a"]), Some("a"));
        m.split(true);
        m.split(true);
        // 0.5, 0.25, 0.25
        m.drag(1, 0.99);
        assert!((m.groups[1].share - 0.4).abs() < 1e-9 && (m.groups[2].share - 0.1).abs() < 1e-9);
        m.drag(0, 0.0);
        assert!((m.groups[0].share - 0.1).abs() < 1e-9 && (m.groups[1].share - 0.8).abs() < 1e-9);
        let b = m.boundary(0);
        m.drag(0, b + STEP);
        assert!((m.groups[0].share - 0.15).abs() < 1e-9);
        m.share_evenly();
        assert!(m.groups.iter().all(|g| (g.share - 1.0 / 3.0).abs() < 1e-9));
    }

    #[test]
    fn moving_an_editor_past_the_last_group_makes_one_and_an_emptied_group_goes() {
        let mut m = Model::default();
        let order = paths(&["a", "b"]);
        m.reconcile(&order, Some("b"));
        assert!(m.move_shown(true, &order));
        assert_eq!((m.groups.len(), m.groups[1].tabs.clone(), m.focused), (2, paths(&["b"]), m.groups[1].id));
        assert_eq!(m.groups[0].tabs, paths(&["a"]));
        // Back: the second group is left with nothing and goes.
        assert!(m.move_shown(false, &order));
        assert_eq!((m.groups.len(), m.groups[0].tabs.clone()), (1, order.clone()));
        assert_eq!(m.shown().as_deref(), Some("b"));
        // A group of one tab is not split by moving it on.
        let mut one = Model::default();
        one.reconcile(&paths(&["a"]), Some("a"));
        assert!(!one.move_shown(true, &paths(&["a"])));
    }

    #[test]
    fn focusing_one_past_the_last_group_splits_and_next_goes_round() {
        let mut m = Model::default();
        m.reconcile(&paths(&["a"]), Some("a"));
        assert!(m.focus_at(1));
        assert_eq!(m.groups.len(), 2);
        assert!(!m.focus_at(5));
        assert!(m.focus_step(true, true));
        assert_eq!(m.at(), 0);
        assert!(!m.focus_step(false, false));
        assert!(m.focus_step(false, true));
        assert_eq!(m.at(), 1);
    }

    #[test]
    fn a_kept_layout_waits_for_its_tabs_and_shows_what_it_showed() {
        let mut m = Model::default();
        m.reconcile(&paths(&["a", "b", "c"]), Some("a"));
        m.split(true);
        m.groups[0].tabs = paths(&["a"]);
        m.groups[1].tabs = paths(&["b", "c"]);
        m.groups[1].active = Some("b".into());
        m.vertical = true;
        m.focused = m.groups[0].id;
        let at = Place { line: 3, col: 2, x: 0.0, y: 40.0 };
        let v = m.to_json(|id, p| (id == 2 && p == "c").then_some(at));
        let (mut back, places) = Model::from_json(&v, &paths(&["a", "b", "c"])).expect("two groups");
        assert_eq!(places, vec![(2, "c".to_string(), at)]);
        assert!(back.vertical);
        // The files come one by one, each said to be shown as it comes.
        back.reconcile(&paths(&["c"]), Some("c"));
        back.reconcile(&paths(&["c", "b"]), Some("b"));
        assert!(!back.settle());
        back.reconcile(&paths(&["c", "b", "a"]), Some("a"));
        assert!(back.settle());
        assert_eq!(back.groups[0].tabs, paths(&["a"]));
        assert_eq!(back.groups[1].tabs, paths(&["c", "b"]));
        assert_eq!(back.groups[1].active.as_deref(), Some("b"));
        assert_eq!(back.shown().as_deref(), Some("a"));
        // One group kept: nothing to wait for.
        assert!(Model::from_json(&Model::default().to_json(|_, _| None), &[]).is_none());
    }

    #[test]
    fn the_window_s_tabs_and_its_groups_follow_each_other_and_come_back_after_a_restart() {
        use std::rc::Rc;
        let send: Send = Rc::new(|_| {});
        let file =
            |ui: Ui, path: &str| ui.take(&json!({"type": "file", "path": path, "text": "x\n", "modified": 1}), &send);
        let ui = Ui::new();
        register(ui, &send);
        let model = |ui: Ui| groups(ui).model.get_untracked();
        file(ui, "a.rs");
        file(ui, "b.rs");
        split(ui, None, true);
        assert_eq!(model(ui).groups.len(), 2);
        assert_eq!((model(ui).focused, ui.active.get_untracked().as_deref()), (model(ui).groups[1].id, Some("b.rs")));
        // A file opened goes to the focused group; one shown from the first group's strip focuses it.
        file(ui, "c.rs");
        assert_eq!(model(ui).groups[1].tabs, paths(&["b.rs", "c.rs"]));
        let first = model(ui).groups[0].id;
        show(ui, first, "a.rs");
        assert_eq!((model(ui).focused, ui.active.get_untracked().as_deref()), (first, Some("a.rs")));
        // b.rs is in both: closed in the first group it stays open in the second.
        ui.close("b.rs");
        assert!(ui.tab("b.rs").is_some());
        assert_eq!(model(ui).groups[0].tabs, paths(&["a.rs"]));
        // Renamed, it stays where it is.
        ui.moved("c.rs", "d.rs");
        assert_eq!(model(ui).groups[1].tabs, paths(&["b.rs", "d.rs"]));
        // Kept, and made again at the next start as the files come.
        let state = ui.view_state();
        assert_eq!(state["groups"]["list"][1]["tabs"], json!(["b.rs", "d.rs"]));
        let again = Ui::new();
        again.take(&json!({"type": "state", "view_state": state}), &send);
        file(again, "d.rs");
        file(again, "a.rs");
        file(again, "b.rs");
        file(again, "a.rs");
        let m = model(again);
        assert_eq!(m.groups.len(), 2);
        assert_eq!((m.groups[0].tabs.clone(), m.groups[1].tabs.clone()), (paths(&["a.rs"]), paths(&["d.rs", "b.rs"])));
        assert_eq!(m.groups[1].active.as_deref(), Some("d.rs"));
        assert_eq!((m.at(), again.active.get_untracked().as_deref()), (0, Some("a.rs")));
        // Closing the first group's only tab: that group goes.
        again.close("a.rs");
        assert_eq!((model(again).groups.len(), again.active.get_untracked().as_deref()), (1, Some("d.rs")));
    }

    #[test]
    fn a_renamed_tab_stays_in_its_groups() {
        let mut m = Model::default();
        m.reconcile(&paths(&["src/a.rs"]), Some("src/a.rs"));
        m.split(true);
        m.rename(|p| crate::state::moved_path(p, "src", "lib"));
        assert!(m.groups.iter().all(|g| g.tabs == paths(&["lib/a.rs"]) && g.active.as_deref() == Some("lib/a.rs")));
    }
}
