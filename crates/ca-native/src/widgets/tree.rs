//! A tree or a list of many rows, as VS Code's: rows of 22 px, each level 8 px further in, a chevron before a row
//! that folds, the row's own content, and its actions at the right while the pointer is on it. Only the rows on
//! screen are drawn, so it may have thousands. The tree is one stop of the keys and the row lit moves within it:
//! Up and Down, Home and End, Page Up and Page Down, Left (fold, or go to the row it is under), Right (unfold, or go
//! into it), Enter and Space (fold or open). With `multi`, Shift with those keys or with a click selects from the
//! row the selection began at, Ctrl with a click adds or takes away one row, Ctrl+A selects every row.
//!
//! The part that has the tree lists the rows shown now, top to bottom (a folded row's children left out: the part
//! keeps what is folded), keeps the tree's `State` with its own, and is told what was asked:
//!
//! ```ignore
//! let state = ui.state::<Mine>().tree;                       // tree::State::new(ui), made once
//! tree::tree(ui, state, move || rows_now(ui))                // im::Vector<tree::Node>
//!     .content(move |node| h_stack((icon_of(node), label_of(node))).into_any())
//!     .actions(move |node| vec![tree::act("add", "Stage Changes", { let id = node.id.clone(); move || stage(&id) })])
//!     .multi()
//!     .on_toggle(move |id| fold_or_unfold(ui, id))
//!     .on_open(move |id| open(ui, id))
//!     .on_menu(move |id, at| popup::open(ui, at, rows_for(id)))     // id none: the tree's empty part
//!     .on_key(move |key, mods| delete_or_rename(ui, key, mods))     // a key the tree has no use for; true: taken
//!     .view()
//! ```
//!
//! `state.sel` has the row lit (`focus`) and the rows selected (`picked`, in the order they were given); a part's
//! commands act on them. `tree::focus(state)` gives the tree the keys; `tree::reveal(state, id)` lights a row and
//! brings it in sight.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::rc::Rc;

use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::Rect;
use floem::peniko::Color;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo, create_rw_signal};
use floem::style::CursorStyle;
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, VirtualVector, container, empty, h_stack, h_stack_from_iter, label,
    scroll, tooltip, virtual_stack,
};
use floem::{AnyView, IntoView, View, ViewId};
use serde_json::Value;

use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::popup::At;

/// VS Code's row, how much further in each level is, and the room before the first level.
pub const ROW: f64 = 22.0;
const INDENT: f64 = 8.0;
const BEFORE: f64 = 8.0;

/// A row of the tree, as its part lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Node {
    /// What the part knows the row by (a path, an id): one row only has it.
    pub id: String,
    /// How deep it is: 0 at the top.
    pub depth: usize,
    /// A row that folds: whether it is open. None: a leaf.
    pub fold: Option<bool>,
    /// What tells the row from the one drawn before under the same id (its text changed): the row is drawn again
    /// when this is another number.
    pub stamp: u64,
}

/// A leaf, at a depth.
pub fn leaf(id: &str, depth: usize) -> Node {
    Node { id: id.into(), depth, fold: None, stamp: 0 }
}

/// A row that folds, at a depth, open or not.
pub fn folder(id: &str, depth: usize, open: bool) -> Node {
    Node { id: id.into(), depth, fold: Some(open), stamp: 0 }
}

/// The row lit, the row a selection with Shift begins at, and the rows selected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sel {
    pub focus: Option<String>,
    pub anchor: Option<String>,
    pub picked: Vec<String>,
}

impl Sel {
    /// One row lit and selected.
    pub fn one(id: &str) -> Sel {
        Sel { focus: Some(id.into()), anchor: Some(id.into()), picked: vec![id.into()] }
    }

    pub fn has(&self, id: &str) -> bool {
        self.picked.iter().any(|p| p == id)
    }
}

/// A key pressed in a tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Up,
    Down,
    First,
    Last,
    PageUp,
    PageDown,
    /// Left: an open row folded, else the row it is under lit.
    Out,
    /// Right: a folded row opened, else its first row lit.
    In,
    /// Enter or Space: a row that folds is folded or opened, a leaf opened.
    Take,
    /// Ctrl+A.
    All,
    /// Escape: only the row lit stays selected.
    Leave,
}

/// What a key asks of the part that has the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Did {
    Nothing,
    /// The row lit or the selection is another.
    Moved,
    /// Fold this row, or unfold it.
    Toggle(String),
    /// Open this leaf.
    Open(String),
}

/// The key a tree takes, of what was pressed; none: not the tree's.
pub fn press_of(key: &Key, m: Modifiers) -> Option<Press> {
    let ctrl = m.contains(Modifiers::CONTROL) || m.contains(Modifiers::META);
    if m.contains(Modifiers::ALT) {
        return None;
    }
    Some(match key {
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("a") => Press::All,
        _ if ctrl => return None,
        Key::Named(NamedKey::ArrowUp) => Press::Up,
        Key::Named(NamedKey::ArrowDown) => Press::Down,
        Key::Named(NamedKey::Home) => Press::First,
        Key::Named(NamedKey::End) => Press::Last,
        Key::Named(NamedKey::PageUp) => Press::PageUp,
        Key::Named(NamedKey::PageDown) => Press::PageDown,
        Key::Named(NamedKey::ArrowLeft) => Press::Out,
        Key::Named(NamedKey::ArrowRight) => Press::In,
        Key::Named(NamedKey::Enter | NamedKey::Space) => Press::Take,
        Key::Named(NamedKey::Escape) => Press::Leave,
        _ => return None,
    })
}

fn place(nodes: &[Node], id: &Option<String>) -> Option<usize> {
    let id = id.as_deref()?;
    nodes.iter().position(|n| n.id == id)
}

/// The rows from one place to another, whichever comes first.
fn span(nodes: &[Node], a: usize, b: usize) -> Vec<String> {
    nodes[a.min(b)..=a.max(b)].iter().map(|n| n.id.clone()).collect()
}

/// The row at `to` lit: alone selected, or (`extend`) with every row from where the selection began.
fn go(nodes: &[Node], sel: &mut Sel, to: usize, extend: bool) {
    let id = nodes[to].id.clone();
    let from = place(nodes, &sel.anchor).filter(|_| extend);
    match from {
        Some(from) => sel.picked = span(nodes, from, to),
        None => {
            sel.anchor = Some(id.clone());
            sel.picked = vec![id.clone()];
        }
    }
    sel.focus = Some(id);
}

/// A key pressed: the row lit moves (`shift` with `multi`: the selection reaches from where it began to it), and
/// what the part is asked to fold or to open. `page` is how many rows are in sight.
pub fn press(nodes: &[Node], sel: &mut Sel, key: Press, shift: bool, multi: bool, page: usize) -> Did {
    let Some(last) = nodes.len().checked_sub(1) else { return Did::Nothing };
    let was = sel.clone();
    let extend = shift && multi;
    let at = place(nodes, &sel.focus);
    let moved = |sel: &Sel| if *sel == was { Did::Nothing } else { Did::Moved };
    let page = page.max(2) - 1;
    let to = match (key, at) {
        (Press::All, _) => {
            if multi {
                sel.picked = nodes.iter().map(|n| n.id.clone()).collect();
                if at.is_none() {
                    sel.focus = Some(nodes[0].id.clone());
                }
            }
            return moved(sel);
        }
        (Press::Leave, _) => {
            sel.picked = at.map(|i| vec![nodes[i].id.clone()]).unwrap_or_default();
            sel.anchor = sel.focus.clone().filter(|_| at.is_some());
            return moved(sel);
        }
        // Nothing lit (or a row no longer shown): the first row, or the last for the keys that go up.
        (Press::Up | Press::Last | Press::PageUp, None) => last,
        (_, None) => 0,
        (Press::Up, Some(i)) => i.saturating_sub(1),
        (Press::Down, Some(i)) => (i + 1).min(last),
        (Press::First, _) => 0,
        (Press::Last, _) => last,
        (Press::PageUp, Some(i)) => i.saturating_sub(page),
        (Press::PageDown, Some(i)) => (i + page).min(last),
        (Press::Out, Some(i)) => match nodes[i].fold {
            Some(true) => return Did::Toggle(nodes[i].id.clone()),
            _ => nodes[..i].iter().rposition(|n| n.depth < nodes[i].depth).unwrap_or(i),
        },
        (Press::In, Some(i)) => match nodes[i].fold {
            Some(false) => return Did::Toggle(nodes[i].id.clone()),
            Some(true) if i < last && nodes[i + 1].depth > nodes[i].depth => i + 1,
            _ => i,
        },
        (Press::Take, Some(i)) => {
            let id = nodes[i].id.clone();
            return if nodes[i].fold.is_some() { Did::Toggle(id) } else { Did::Open(id) };
        }
    };
    // Left and Right move the row lit, they do not stretch the selection.
    go(nodes, sel, to, extend && !matches!(key, Press::Out | Press::In));
    moved(sel)
}

/// A click on a row: it is lit, and alone selected; with Ctrl (`multi`) it is added to the selection or taken out
/// of it, with Shift every row from where the selection began to it is selected.
pub fn click(nodes: &[Node], sel: &mut Sel, id: &str, ctrl: bool, shift: bool, multi: bool) {
    let Some(to) = nodes.iter().position(|n| n.id == id) else { return };
    if multi && shift {
        go(nodes, sel, to, true);
    } else if multi && ctrl {
        match sel.picked.iter().position(|p| p == id) {
            Some(at) => {
                sel.picked.remove(at);
            }
            None => sel.picked.push(id.to_string()),
        }
        sel.focus = Some(id.to_string());
        sel.anchor = Some(id.to_string());
    } else {
        go(nodes, sel, to, false);
    }
}

/// A right-click on a row: a row that is not selected becomes the only one selected (the menu is about it); one
/// that is selected leaves the selection as it is (the menu is about them all).
pub fn point(sel: &mut Sel, id: &str) {
    if !sel.has(id) {
        *sel = Sel::one(id);
    } else {
        sel.focus = Some(id.to_string());
    }
}

/// The selection kept to the rows that are still there (rows come and go under it).
pub fn keep(nodes: &[Node], sel: &Sel) -> Sel {
    let there = |id: &String| nodes.iter().any(|n| n.id == *id);
    Sel {
        focus: sel.focus.clone().filter(there),
        anchor: sel.anchor.clone().filter(there),
        picked: sel.picked.iter().filter(|p| there(p)).cloned().collect(),
    }
}

/// A tree's state, kept by the part that has it (in its own `ui.state`): the selection, whether the tree has the
/// keys, the row to bring in sight, and the tree's view.
#[derive(Clone, Copy)]
pub struct State {
    pub sel: RwSignal<Sel>,
    pub focused: RwSignal<bool>,
    reveal: RwSignal<Option<(String, u64)>>,
    view: RwSignal<Option<ViewId>>,
}

impl State {
    /// Made once, in the window's own scope.
    pub fn new(ui: Ui) -> State {
        State {
            sel: ui.scope.create_rw_signal(Sel::default()),
            focused: ui.scope.create_rw_signal(false),
            reveal: ui.scope.create_rw_signal(None),
            view: ui.scope.create_rw_signal(None),
        }
    }

    /// The tree's view once it is drawn: what a menu or a dialog gives the keys back to.
    pub fn view(&self) -> Option<ViewId> {
        self.view.get_untracked()
    }
}

/// Gives the tree the keys.
pub fn focus(state: State) {
    if let Some(id) = state.view.get_untracked() {
        id.request_focus();
    }
}

/// A row lit, alone selected, and brought in sight.
pub fn reveal(state: State, id: &str) {
    if state.sel.with_untracked(|s| *s != Sel::one(id)) {
        state.sel.set(Sel::one(id));
    }
    state.reveal.update(|r| *r = Some((id.to_string(), r.as_ref().map_or(0, |(_, n)| n + 1))));
}

/// An action of a row, drawn at its right while the pointer is on it.
#[derive(Clone)]
pub struct Action {
    pub icon: &'static str,
    pub tip: String,
    pub run: Rc<dyn Fn()>,
}

/// An action: its codicon, what it says under the pointer, what it does.
pub fn act(icon: &'static str, tip: &str, run: impl Fn() + 'static) -> Action {
    Action { icon, tip: tip.into(), run: Rc::new(run) }
}

type Content = Rc<dyn Fn(&Node) -> AnyView>;
type Actions = Rc<dyn Fn(&Node) -> Vec<Action>>;
type Told = Rc<dyn Fn(&str)>;
type Menu = Rc<dyn Fn(Option<&str>, At)>;
type OnKey = Rc<dyn Fn(&Key, Modifiers) -> bool>;

/// A tree, as it is being made.
pub struct Tree {
    ui: Ui,
    state: State,
    rows: Rc<dyn Fn() -> im::Vector<Node>>,
    content: Content,
    actions: Option<Actions>,
    multi: bool,
    on_toggle: Option<Told>,
    on_open: Option<Told>,
    on_menu: Option<Menu>,
    on_key: Option<OnKey>,
}

/// A tree of the rows `rows` lists (called again when a signal it reads is set).
pub fn tree(ui: Ui, state: State, rows: impl Fn() -> im::Vector<Node> + 'static) -> Tree {
    Tree {
        ui,
        state,
        rows: Rc::new(rows),
        content: Rc::new(|node| {
            let id = node.id.clone();
            label(move || id.clone()).into_any()
        }),
        actions: None,
        multi: false,
        on_toggle: None,
        on_open: None,
        on_menu: None,
        on_key: None,
    }
}

/// What the callbacks of a row need: shared by every row.
#[derive(Clone)]
struct Shared {
    ui: Ui,
    state: State,
    rows: Memo<im::Vector<Node>>,
    content: Content,
    actions: Option<Actions>,
    multi: bool,
    on_toggle: Option<Told>,
    on_open: Option<Told>,
    on_menu: Option<Menu>,
}

impl Shared {
    fn told(&self, did: Did) {
        match did {
            Did::Toggle(id) => {
                if let Some(told) = &self.on_toggle {
                    told(&id);
                }
            }
            Did::Open(id) => {
                if let Some(told) = &self.on_open {
                    told(&id);
                }
            }
            Did::Nothing | Did::Moved => {}
        }
    }

    fn set(&self, sel: Sel) {
        if self.state.sel.with_untracked(|was| *was != sel) {
            self.state.sel.set(sel);
        }
    }
}

impl Tree {
    /// What a row draws after its chevron (its icon, its name, its count): the row's id, when it is not said.
    pub fn content(mut self, content: impl Fn(&Node) -> AnyView + 'static) -> Tree {
        self.content = Rc::new(content);
        self
    }

    /// A row's actions, drawn at its right while the pointer is on it.
    pub fn actions(mut self, actions: impl Fn(&Node) -> Vec<Action> + 'static) -> Tree {
        self.actions = Some(Rc::new(actions));
        self
    }

    /// Several rows may be selected (Shift and Ctrl with a click, Shift with the arrows, Ctrl+A).
    pub fn multi(mut self) -> Tree {
        self.multi = true;
        self
    }

    /// Asked to fold or unfold a row (a click on it or on its chevron, Enter, Left, Right).
    pub fn on_toggle(mut self, told: impl Fn(&str) + 'static) -> Tree {
        self.on_toggle = Some(Rc::new(told));
        self
    }

    /// Asked to open a leaf (a click on it, Enter).
    pub fn on_open(mut self, told: impl Fn(&str) + 'static) -> Tree {
        self.on_open = Some(Rc::new(told));
        self
    }

    /// A right-click: on a row (it is selected by then), or on the tree's empty part (none). `At` is where the
    /// pointer is, with the keys going back to the tree: for `popup::open`.
    pub fn on_menu(mut self, told: impl Fn(Option<&str>, At) + 'static) -> Tree {
        self.on_menu = Some(Rc::new(told));
        self
    }

    /// A key the tree has no use for (F2, Delete, a letter); true: it was taken.
    pub fn on_key(mut self, told: impl Fn(&Key, Modifiers) -> bool + 'static) -> Tree {
        self.on_key = Some(Rc::new(told));
        self
    }

    /// The tree drawn. It fills the room it is given.
    pub fn view(self) -> impl IntoView + use<> {
        let Tree { ui, state, rows, content, actions, multi, on_toggle, on_open, on_menu, on_key } = self;
        let rows: Memo<im::Vector<Node>> = create_memo(move |_| rows());
        let shared = Shared { ui, state, rows, content, actions, multi, on_toggle, on_open, on_menu };
        let drawn = shared.clone();
        let list = virtual_stack(
            VirtualDirection::Vertical,
            VirtualItemSize::Fixed(Box::new(|| ROW)),
            move || rows.get().enumerate(),
            |(_, n): &(usize, Node)| n.clone(),
            move |(_, n): (usize, Node)| row_view(drawn.clone(), n),
        )
        .style(|s| s.flex_col().width_full());
        let keyed = shared.clone();
        let empty_part = shared.clone();
        let tree = scroll(list).ensure_visible(move || {
            // The row asked to be in sight (by a key, or by the part).
            let y = state
                .reveal
                .get()
                .and_then(|(id, _)| rows.with_untracked(|r| r.iter().position(|n| n.id == id)))
                .map_or(0.0, |i| i as f64 * ROW);
            Rect::new(0.0, y, 1.0, y + ROW)
        });
        let id = tree.id();
        state.view.set(Some(id));
        tree.keyboard_navigable()
            .on_event(EventListener::KeyDown, move |e| {
                let Event::KeyDown(k) = e else { return EventPropagation::Continue };
                let (key, m) = (&k.key.logical_key, k.modifiers);
                let Some(pressed) = press_of(key, m) else {
                    return match &on_key {
                        Some(told) if told(key, m) => EventPropagation::Stop,
                        _ => EventPropagation::Continue,
                    };
                };
                let nodes: Vec<Node> = rows.with_untracked(|r| r.iter().cloned().collect());
                let page = id.get_size().map_or(10, |s| (s.height / ROW) as usize);
                let mut sel = state.sel.get_untracked();
                let did = press(&nodes, &mut sel, pressed, m.contains(Modifiers::SHIFT), multi, page);
                if let Some(lit) = sel.focus.clone().filter(|_| did == Did::Moved) {
                    state.reveal.update(|r| *r = Some((lit, r.as_ref().map_or(0, |(_, n)| n + 1))));
                }
                keyed.set(sel);
                // Escape with nothing to give up is the window's (it closes what is open over it).
                if pressed == Press::Leave && did == Did::Nothing {
                    return EventPropagation::Continue;
                }
                keyed.told(did);
                EventPropagation::Stop
            })
            .on_event_cont(EventListener::FocusGained, move |_| state.focused.set(true))
            .on_event_cont(EventListener::FocusLost, move |_| state.focused.set(false))
            // The tree's empty part: a click selects nothing, a right-click opens the menu for no row.
            .on_click_stop(move |_| {
                id.request_focus();
                empty_part.set(Sel::default());
            })
            .on_secondary_click_stop(move |e| {
                id.request_focus();
                shared.set(Sel::default());
                if let Some(menu) = &shared.on_menu {
                    menu(None, At::pointer(id, e).back(id));
                }
            })
            .style(|s| s.size_full().min_height(0.0))
    }
}

/// A row: its room at the left by its depth, its chevron, its content, and its actions under the pointer.
fn row_view(shared: Shared, node: Node) -> impl IntoView {
    let Shared { ui, state, .. } = shared;
    let t = ui.colors;
    let over = create_rw_signal(false);
    let chevron: AnyView = match node.fold {
        Some(open) => {
            let (toggled, id) = (shared.clone(), node.id.clone());
            container(icon(if open { "chevron-down" } else { "chevron-right" }, 16.0))
                .on_click_stop(move |_| toggled.told(Did::Toggle(id.clone())))
                .into_any()
        }
        None => empty().style(|s| s.width(16.0)).into_any(),
    };
    let content = (shared.content)(&node);
    let actions: Vec<Action> = shared.actions.as_ref().map(|of| of(&node)).unwrap_or_default();
    let buttons = h_stack_from_iter(actions.into_iter().map(move |a| {
        let Action { icon: name, tip, run } = a;
        // The press is the button's own: else it reaches the row too, which opens or folds it.
        let button = container(icon(name, 16.0));
        let button =
            button.on_event_stop(EventListener::PointerDown, |_| {}).on_click_stop(move |_| run()).style(move |s| {
                let c = t.get();
                s.width(20.0)
                    .height(20.0)
                    .items_center()
                    .justify_center()
                    .border_radius(5.0)
                    .cursor(CursorStyle::Pointer)
                    .hover(|s| s.background(c.toolbar_hover))
            });
        tooltip(button, move || {
            let tip = tip.clone();
            label(move || tip.clone())
        })
    }))
    .style(move |s| s.gap(2.0).padding_right(8.0).flex_shrink(0.0_f32).apply_if(!over.get(), |s| s.hide()));
    let (clicked, menu_of, lit_of) = (shared.clone(), shared.clone(), node.id.clone());
    let (id, right_id, folds, depth) = (node.id.clone(), node.id.clone(), node.fold.is_some(), node.depth);
    let row = h_stack((
        empty().style(move |s| s.width(INDENT * depth as f64)),
        chevron,
        container(content).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).padding_left(4.0)),
        buttons,
    ));
    let row_id = row.id();
    row.on_event_stop(EventListener::PointerDown, move |e| {
        // The selection follows the press (as VS Code's), the row is opened on the click.
        let Event::PointerDown(p) = e else { return };
        if !p.button.is_primary() {
            return;
        }
        focus(state);
        let m = p.modifiers;
        let nodes: Vec<Node> = clicked.rows.with_untracked(|r| r.iter().cloned().collect());
        let mut sel = state.sel.get_untracked();
        let ctrl = m.contains(Modifiers::CONTROL) || m.contains(Modifiers::META);
        click(&nodes, &mut sel, &id, ctrl, m.contains(Modifiers::SHIFT), clicked.multi);
        clicked.set(sel);
        // A plain click folds a row that folds, or opens a leaf; one that selects does neither.
        if !(clicked.multi && (ctrl || m.contains(Modifiers::SHIFT))) {
            clicked.told(if folds { Did::Toggle(id.clone()) } else { Did::Open(id.clone()) });
        }
    })
    .on_click_stop(|_| {})
    .on_secondary_click_stop(move |e| {
        focus(state);
        let mut sel = state.sel.get_untracked();
        point(&mut sel, &right_id);
        menu_of.set(sel);
        if let (Some(menu), Some(tree)) = (&menu_of.on_menu, state.view.get_untracked()) {
            menu(Some(&right_id), At::pointer(row_id, e).back(tree));
        }
    })
    .on_event_cont(EventListener::PointerEnter, move |_| over.set(true))
    .on_event_cont(EventListener::PointerLeave, move |_| over.set(false))
    .style(move |s| {
        let c = t.get();
        let (selected, lit, many) =
            state.sel.with(|sel| (sel.has(&lit_of), sel.focus.as_deref() == Some(&lit_of), sel.picked.len() > 1));
        let keys = state.focused.get();
        let s = s
            .height(ROW)
            .width_full()
            .items_center()
            .padding_left(BEFORE - 1.0)
            .font_size(13.0)
            .color(c.side_fg)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            // The row lit is outlined while the tree has the keys and the selection does not say which it is.
            .border(1.0)
            .border_color(if keys && lit && (many || !selected) { c.list_outline } else { Color::TRANSPARENT });
        // VS Code's selection: lit while the tree has the keys, dimmed while it has not.
        match (selected, keys) {
            (true, true) => s.background(c.list_selected).color(c.list_selected_fg),
            (true, false) => s.background(c.list_inactive),
            _ => s.hover(|s| s.background(c.list_hover)),
        }
    })
}

/// What it adds to the window's registries, once at start: nothing (a tree is drawn by the part it is of).
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in: none are.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// src (open: main.rs, lib (folded), util.rs), docs (folded), README.md, notes.txt.
    fn nodes() -> Vec<Node> {
        vec![
            folder("src", 0, true),
            leaf("src/main.rs", 1),
            folder("src/lib", 1, false),
            leaf("src/util.rs", 1),
            folder("docs", 0, false),
            leaf("README.md", 0),
            leaf("notes.txt", 0),
        ]
    }

    fn lit(sel: &Sel) -> &str {
        sel.focus.as_deref().unwrap_or("")
    }

    fn key(sel: &mut Sel, key: Press) -> Did {
        press(&nodes(), sel, key, false, true, 4)
    }

    fn shift(sel: &mut Sel, key: Press) -> Did {
        press(&nodes(), sel, key, true, true, 4)
    }

    #[test]
    fn the_arrows_home_end_and_the_pages_move_the_row_lit_and_stop_at_the_ends() {
        let mut sel = Sel::default();
        assert_eq!(key(&mut sel, Press::Down), Did::Moved);
        assert_eq!((lit(&sel), sel.picked.clone()), ("src", vec!["src".to_string()]), "nothing lit: the first row");
        key(&mut sel, Press::Down);
        key(&mut sel, Press::Down);
        assert_eq!(sel, Sel::one("src/lib"), "the row lit is the one selected");
        assert_eq!(key(&mut sel, Press::Last), Did::Moved);
        assert_eq!(lit(&sel), "notes.txt");
        assert_eq!(key(&mut sel, Press::Down), Did::Nothing, "the end");
        assert_eq!(key(&mut sel, Press::PageUp), Did::Moved);
        assert_eq!(lit(&sel), "src/util.rs", "a page less one row");
        key(&mut sel, Press::PageUp);
        assert_eq!(lit(&sel), "src");
        assert_eq!(key(&mut sel, Press::Up), Did::Nothing, "the beginning");
        key(&mut sel, Press::PageDown);
        assert_eq!(lit(&sel), "src/util.rs");
        key(&mut sel, Press::First);
        assert_eq!(lit(&sel), "src");
        // Nothing lit: Up begins at the last row.
        let mut none = Sel::default();
        key(&mut none, Press::Up);
        assert_eq!(lit(&none), "notes.txt");
        // A row lit that is no longer shown counts as none; no rows: nothing.
        let mut gone = Sel::one("src/lib/deep.rs");
        key(&mut gone, Press::Down);
        assert_eq!(lit(&gone), "src");
        assert_eq!(press(&[], &mut gone, Press::Down, false, true, 4), Did::Nothing);
    }

    #[test]
    fn left_and_right_fold_unfold_and_go_out_of_and_into_a_row() {
        let mut sel = Sel::one("src");
        assert_eq!(key(&mut sel, Press::In), Did::Moved);
        assert_eq!(lit(&sel), "src/main.rs", "open: into it");
        assert_eq!(key(&mut sel, Press::In), Did::Nothing, "a leaf");
        assert_eq!(key(&mut sel, Press::Out), Did::Moved);
        assert_eq!(lit(&sel), "src", "to the row it is under");
        assert_eq!(key(&mut sel, Press::Out), Did::Toggle("src".into()), "open: folded");
        let mut sel = Sel::one("src/lib");
        assert_eq!(key(&mut sel, Press::In), Did::Toggle("src/lib".into()), "folded: opened");
        assert_eq!(key(&mut sel, Press::Out), Did::Moved);
        assert_eq!(lit(&sel), "src");
        let mut top = Sel::one("README.md");
        assert_eq!(key(&mut top, Press::Out), Did::Nothing, "a row at the top is under none");
        // Enter and Space: a row that folds is toggled, a leaf opened.
        assert_eq!(key(&mut Sel::one("docs"), Press::Take), Did::Toggle("docs".into()));
        assert_eq!(key(&mut Sel::one("notes.txt"), Press::Take), Did::Open("notes.txt".into()));
        assert_eq!(key(&mut Sel::default(), Press::Take), Did::Moved, "nothing lit: the first row is");
    }

    #[test]
    fn shift_selects_from_where_the_selection_began_and_ctrl_a_selects_every_row() {
        let mut sel = Sel::one("src/main.rs");
        shift(&mut sel, Press::Down);
        shift(&mut sel, Press::Down);
        assert_eq!(sel.picked, ["src/main.rs", "src/lib", "src/util.rs"]);
        assert_eq!((lit(&sel), sel.anchor.as_deref()), ("src/util.rs", Some("src/main.rs")));
        shift(&mut sel, Press::Up);
        assert_eq!(sel.picked, ["src/main.rs", "src/lib"], "back towards where it began");
        shift(&mut sel, Press::First);
        assert_eq!(sel.picked, ["src", "src/main.rs"], "and past it, the other way");
        // Without Shift the selection is the row lit again.
        key(&mut sel, Press::Down);
        assert_eq!(sel, Sel::one("src/main.rs"));
        assert_eq!(key(&mut sel, Press::All), Did::Moved);
        assert_eq!((sel.picked.len(), lit(&sel)), (7, "src/main.rs"));
        assert_eq!(key(&mut sel, Press::Leave), Did::Moved);
        assert_eq!(sel, Sel::one("src/main.rs"), "Escape: only the row lit");
        assert_eq!(key(&mut sel, Press::Leave), Did::Nothing);
        // A tree of one selection only: Shift and Ctrl+A select no more.
        let mut one = Sel::one("src");
        assert_eq!(press(&nodes(), &mut one, Press::Down, true, false, 4), Did::Moved);
        assert_eq!(one, Sel::one("src/main.rs"));
        assert_eq!(press(&nodes(), &mut one, Press::All, false, false, 4), Did::Nothing);
    }

    #[test]
    fn a_click_selects_one_row_ctrl_adds_or_takes_away_and_shift_selects_the_rows_between() {
        let nodes = nodes();
        let mut sel = Sel::default();
        click(&nodes, &mut sel, "README.md", false, false, true);
        assert_eq!(sel, Sel::one("README.md"));
        click(&nodes, &mut sel, "src/main.rs", true, false, true);
        assert_eq!(
            (sel.picked.clone(), lit(&sel)),
            (vec!["README.md".to_string(), "src/main.rs".into()], "src/main.rs")
        );
        click(&nodes, &mut sel, "README.md", true, false, true);
        assert_eq!(sel.picked, ["src/main.rs"], "taken out again");
        click(&nodes, &mut sel, "src/main.rs", true, false, true);
        click(&nodes, &mut sel, "src/util.rs", false, true, true);
        assert_eq!(sel.picked, ["src/main.rs", "src/lib", "src/util.rs"], "from the row last clicked");
        click(&nodes, &mut sel, "src", false, true, true);
        assert_eq!(sel.picked, ["src", "src/main.rs"]);
        click(&nodes, &mut sel, "nowhere", false, false, true);
        assert_eq!(sel.picked, ["src", "src/main.rs"], "a row that is not there");
        // One selection only: Ctrl and Shift are a plain click.
        click(&nodes, &mut sel, "docs", true, true, false);
        assert_eq!(sel, Sel::one("docs"));
        // A right-click on a row not selected selects it alone; on one of the selected, it leaves them.
        let mut many =
            Sel { focus: Some("src".into()), anchor: Some("src".into()), picked: vec!["src".into(), "docs".into()] };
        point(&mut many, "docs");
        assert_eq!((many.picked.len(), lit(&many)), (2, "docs"));
        point(&mut many, "notes.txt");
        assert_eq!(many, Sel::one("notes.txt"));
    }

    #[test]
    fn the_selection_keeps_only_the_rows_still_there() {
        let sel =
            Sel { focus: Some("gone".into()), anchor: Some("src".into()), picked: vec!["gone".into(), "src".into()] };
        let kept = keep(&nodes(), &sel);
        assert_eq!(kept, Sel { focus: None, anchor: Some("src".into()), picked: vec!["src".into()] });
    }

    #[test]
    fn the_keys_are_vs_codes() {
        let none = Modifiers::empty();
        let named = |k: NamedKey| Key::Named(k);
        assert_eq!(press_of(&named(NamedKey::ArrowUp), none), Some(Press::Up));
        assert_eq!(press_of(&named(NamedKey::ArrowDown), Modifiers::SHIFT), Some(Press::Down));
        assert_eq!(press_of(&named(NamedKey::Home), none), Some(Press::First));
        assert_eq!(press_of(&named(NamedKey::End), none), Some(Press::Last));
        assert_eq!(press_of(&named(NamedKey::PageUp), none), Some(Press::PageUp));
        assert_eq!(press_of(&named(NamedKey::PageDown), none), Some(Press::PageDown));
        assert_eq!(press_of(&named(NamedKey::ArrowLeft), none), Some(Press::Out));
        assert_eq!(press_of(&named(NamedKey::ArrowRight), none), Some(Press::In));
        assert_eq!(press_of(&named(NamedKey::Enter), none), Some(Press::Take));
        assert_eq!(press_of(&named(NamedKey::Space), none), Some(Press::Take));
        assert_eq!(press_of(&named(NamedKey::Escape), none), Some(Press::Leave));
        assert_eq!(press_of(&Key::Character("a".into()), Modifiers::CONTROL), Some(Press::All));
        assert_eq!(press_of(&Key::Character("a".into()), none), None, "a letter is the part's");
        assert_eq!(press_of(&named(NamedKey::F2), none), None);
        assert_eq!(press_of(&named(NamedKey::Delete), none), None);
        assert_eq!(press_of(&named(NamedKey::ArrowDown), Modifiers::CONTROL), None, "the workbench's");
        assert_eq!(press_of(&named(NamedKey::ArrowDown), Modifiers::ALT), None);
    }

    #[test]
    fn a_row_is_revealed_lit_and_alone_selected() {
        let ui = Ui::new();
        let state = State::new(ui);
        assert!(state.view().is_none());
        reveal(state, "src/util.rs");
        assert_eq!(state.sel.get_untracked(), Sel::one("src/util.rs"));
        assert_eq!(state.reveal.get_untracked(), Some(("src/util.rs".to_string(), 0)));
        reveal(state, "src/util.rs");
        assert_eq!(state.reveal.get_untracked().map(|r| r.1), Some(1), "asked again: brought in sight again");
    }
}
