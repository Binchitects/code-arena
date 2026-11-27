//! A menu opened at a place, as VS Code's context menu: rows of 26 px with their keys at the right, lines between
//! their groups, a check or a dot before a row that is on, a row that opens another menu beside it, and the keys
//! (the arrows, Home and End, Enter, Escape). It is drawn over the whole of the window it was asked in (a view may
//! float in a window of its own), and is the one menu of every part: a right-click's, a "…" button's, the gear's.
//!
//! ```ignore
//! // A part's own rows:
//! popup::open(ui, At::pointer(row_id, event).back(tree_id), vec![
//!     popup::entry("Rename...", move || begin_rename(ui)).keys("F2").into(),
//!     popup::line(),
//!     popup::entry("Word Wrap", move || toggle(ui)).checked(on).into(),
//!     popup::submenu("Sort By", vec![popup::entry("Name", by_name).radio(true).into()]).into(),
//! ]);
//! // The rows the window's parts registered for a place (`menus.rs`):
//! popup::open_menu(ui, &send, At::under(button_id), Menu::AgentMore);
//! ```
//!
//! A row runs after the menu has closed and the keys are back where `At::back` says. One menu is open at a time: a
//! second takes the first's place.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::rc::Rc;

use floem::action::remove_overlay;
use floem::event::{Event, EventListener};
use floem::keyboard::{Key, NamedKey};
use floem::kurbo::{Point, Rect, Size};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::{Decorators, container, dyn_stack, empty, h_stack, label, v_stack_from_iter};
use floem::{AnyView, IntoView, ViewId};
use serde_json::Value;

use crate::icons::icon;
use crate::menus::{Menu, Row};
use crate::state::{Send, Ui};
use crate::widgets::{Keep, cover, solid, text_width, window_size};

/// What is drawn before a row's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    None,
    /// A choice that is on or off: a check while it is on.
    Check(bool),
    /// One choice among several: a dot by the one in force.
    Radio(bool),
}

/// A row that does something, or opens another menu.
#[derive(Clone)]
pub struct Entry {
    pub label: String,
    /// The keys shown at its right ("Ctrl+S").
    pub keys: String,
    pub mark: Mark,
    /// Dimmed, and it does nothing, while false.
    pub enabled: bool,
    pub run: Option<Rc<dyn Fn()>>,
    /// The menu it opens beside itself; none: it runs.
    pub sub: Vec<Item>,
}

/// A row of a menu.
#[derive(Clone)]
pub enum Item {
    /// The line between two groups.
    Line,
    Entry(Entry),
}

/// A row that runs `run`.
pub fn entry(label: &str, run: impl Fn() + 'static) -> Entry {
    Entry {
        label: label.into(),
        keys: String::new(),
        mark: Mark::None,
        enabled: true,
        run: Some(Rc::new(run)),
        sub: Vec::new(),
    }
}

/// A row that opens another menu beside itself.
pub fn submenu(label: &str, items: Vec<Item>) -> Entry {
    Entry { label: label.into(), keys: String::new(), mark: Mark::None, enabled: true, run: None, sub: items }
}

/// The line between two groups.
pub fn line() -> Item {
    Item::Line
}

impl Entry {
    pub fn keys(mut self, keys: &str) -> Entry {
        self.keys = keys.into();
        self
    }

    /// A check before it while `on`.
    pub fn checked(mut self, on: bool) -> Entry {
        self.mark = Mark::Check(on);
        self
    }

    /// A dot before it while it is the one in force.
    pub fn radio(mut self, on: bool) -> Entry {
        self.mark = Mark::Radio(on);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Entry {
        self.enabled = enabled;
        self
    }
}

impl From<Entry> for Item {
    fn from(e: Entry) -> Item {
        Item::Entry(e)
    }
}

/// The menus open, one beside the other: the row lit in each (none: the menu was just opened, or the pointer is on
/// a row that cannot be chosen). The first is the menu itself; each one after is the menu of the row lit before it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lit(pub Vec<Option<usize>>);

/// The rows of the open menu that is `level` deep; none when no menu is open that deep.
fn rows_at<'a>(items: &'a [Item], lit: &Lit, level: usize) -> Option<&'a [Item]> {
    if level >= lit.0.len() {
        return None;
    }
    let mut rows = items;
    for at in lit.0.iter().take(level) {
        match rows.get((*at)?)? {
            Item::Entry(e) if e.enabled && !e.sub.is_empty() => rows = &e.sub,
            _ => return None,
        }
    }
    Some(rows)
}

/// A row the keys stop on: one that is not a line and not dimmed.
fn chosen(item: &Item) -> bool {
    matches!(item, Item::Entry(e) if e.enabled)
}

/// The row at the end of a way through the menus (the row of the first, then of the menu it opens, and so on).
fn entry_at<'a>(items: &'a [Item], path: &[usize]) -> Option<&'a Entry> {
    let (last, before) = path.split_last()?;
    let mut rows = items;
    for i in before {
        match rows.get(*i)? {
            Item::Entry(e) => rows = &e.sub,
            Item::Line => return None,
        }
    }
    match rows.get(*last)? {
        Item::Entry(e) => Some(e),
        Item::Line => None,
    }
}

/// A key pressed in a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Up,
    Down,
    First,
    Last,
    /// Right: into the lit row's menu.
    In,
    /// Left: out of the innermost menu.
    Out,
    /// Enter or Space: the lit row.
    Take,
    /// Escape: the innermost menu closed, the whole when it is the only one.
    Leave,
}

/// What a key or a click did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Did {
    Nothing,
    Close,
    /// The row to run, by its way through the menus.
    Run(Vec<usize>),
}

/// A key pressed: the lit row moved (past the lines and the dimmed rows, around the ends), a menu opened or
/// closed; and whether the whole closes or a row is to run.
pub fn press(items: &[Item], lit: &mut Lit, key: Press) -> Did {
    let Some(level) = lit.0.len().checked_sub(1) else { return Did::Close };
    let Some(rows) = rows_at(items, lit, level) else { return Did::Close };
    let at = lit.0[level];
    let stops: Vec<usize> = (0..rows.len()).filter(|i| chosen(&rows[*i])).collect();
    let into = |lit: &mut Lit, e: &Entry| lit.0.push((0..e.sub.len()).find(|i| chosen(&e.sub[*i])));
    match key {
        Press::Down => {
            lit.0[level] = stops.iter().copied().find(|i| at.is_none_or(|a| *i > a)).or(stops.first().copied())
        }
        Press::Up => {
            lit.0[level] = stops.iter().rev().copied().find(|i| at.is_none_or(|a| *i < a)).or(stops.last().copied())
        }
        Press::First => lit.0[level] = stops.first().copied(),
        Press::Last => lit.0[level] = stops.last().copied(),
        Press::In => {
            if let Some(Item::Entry(e)) = at.and_then(|a| rows.get(a))
                && e.enabled
                && !e.sub.is_empty()
            {
                into(lit, e);
            }
        }
        Press::Out => {
            if level > 0 {
                lit.0.pop();
            }
        }
        Press::Take => {
            if let Some(Item::Entry(e)) = at.and_then(|a| rows.get(a))
                && e.enabled
            {
                if !e.sub.is_empty() {
                    into(lit, e);
                } else if e.run.is_some() {
                    return Did::Run(lit.0.iter().flatten().copied().collect());
                }
            }
        }
        Press::Leave => {
            if level == 0 {
                return Did::Close;
            }
            lit.0.pop();
        }
    }
    Did::Nothing
}

/// The pointer on a row of the menu `level` deep: the row is lit, the menus deeper than its own closed, and its
/// own menu opened beside it. A dimmed row is not lit.
pub fn point(items: &[Item], lit: &Lit, level: usize, row: usize) -> Lit {
    if lit.0.get(level) == Some(&Some(row)) || level >= lit.0.len() {
        return lit.clone();
    }
    let mut to = Lit(lit.0[..=level].to_vec());
    match rows_at(items, &to, level).and_then(|rows| rows.get(row)) {
        Some(Item::Entry(e)) if e.enabled => {
            to.0[level] = Some(row);
            if !e.sub.is_empty() {
                to.0.push(None);
            }
        }
        _ => to.0[level] = None,
    }
    to
}

/// VS Code's sizes: a row, a line with the room around it, and a menu's edge (its border and the room within).
const ROW: f64 = 26.0;
const LINE: f64 = 9.0;
const EDGE: f64 = 5.0;
/// The room before a row's name (the mark is in it), and after its keys.
const BEFORE: f64 = 26.0;
const AFTER: f64 = 20.0;
/// How far a menu stays from the window's edge.
const MARGIN: f64 = 4.0;

fn height_of(item: &Item) -> f64 {
    match item {
        Item::Line => LINE,
        Item::Entry(_) => ROW,
    }
}

/// How far down in its menu a row begins.
fn top_of(rows: &[Item], row: usize) -> f64 {
    EDGE + rows[..row.min(rows.len())].iter().map(height_of).sum::<f64>()
}

/// A menu's size: as wide as its longest name and its longest keys need (`width` measures a text), as high as its
/// rows.
pub fn size_of(rows: &[Item], width: &dyn Fn(&str) -> f64) -> Size {
    let entries = || rows.iter().filter_map(|r| if let Item::Entry(e) = r { Some(e) } else { None });
    let names = entries().map(|e| width(&e.label)).fold(0.0, f64::max);
    let keys = entries().filter(|e| !e.keys.is_empty()).map(|e| width(&e.keys) + 30.0).fold(0.0, f64::max);
    let arrow = if entries().any(|e| !e.sub.is_empty()) { 16.0 } else { 0.0 };
    let wide = (2.0 * EDGE + BEFORE + names + keys + arrow + AFTER).ceil().clamp(160.0, 520.0);
    Size::new(wide, 2.0 * EDGE + rows.iter().map(height_of).sum::<f64>())
}

/// Where a menu of a size goes, kept within the window: under what it was opened by, its left at that one's left
/// (above it when there is no room under); or, `beside`, at its right with their tops level (at its left when there
/// is no room at the right).
pub fn place(by: Rect, beside: bool, size: Size, window: Size) -> Point {
    let most_x = (window.width - size.width - MARGIN).max(0.0);
    let most_y = (window.height - size.height - MARGIN).max(0.0);
    if beside {
        let x = if by.x1 + size.width <= window.width - MARGIN { by.x1 } else { by.x0 - size.width };
        return Point::new(x.clamp(0.0, most_x), by.y0.clamp(0.0, most_y));
    }
    let y = if by.y1 + size.height <= window.height - MARGIN || by.y0 - size.height < 0.0 {
        by.y1
    } else {
        by.y0 - size.height
    };
    Point::new(by.x0.clamp(0.0, most_x), y.clamp(0.0, most_y))
}

/// Where each open menu is in the window: the first by what it was opened by, each one after beside the row that
/// opened it (their first rows level).
pub fn rects(
    items: &[Item],
    lit: &Lit,
    by: Rect,
    beside: bool,
    window: Size,
    width: &dyn Fn(&str) -> f64,
) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    for level in 0..lit.0.len() {
        let Some(rows) = rows_at(items, lit, level) else { break };
        let size = size_of(rows, width);
        let at = match (level.checked_sub(1), out.last()) {
            (Some(before), Some(parent)) => {
                let row = lit.0[before].unwrap_or(0);
                let top = parent.y0 + rows_at(items, lit, before).map_or(EDGE, |r| top_of(r, row)) - EDGE;
                // A little over the menu it comes from, as VS Code's.
                place(Rect::new(parent.x0 + 3.0, top, parent.x1 - 3.0, top + ROW), true, size, window)
            }
            _ => place(by, beside, size, window),
        };
        out.push(Rect::from_origin_size(at, size));
    }
    out
}

/// Where a menu opens: the view it is asked from (the menu is drawn in that view's window), the place in that
/// window it opens by, and the view that has the keys again once it has closed.
#[derive(Clone, Copy, Debug)]
pub struct At {
    view: ViewId,
    by: Rect,
    beside: bool,
    back: Option<ViewId>,
}

impl At {
    /// At a point of the window `view` is in.
    pub fn point(view: ViewId, x: f64, y: f64) -> At {
        At { view, by: Rect::new(x, y, x, y), beside: false, back: None }
    }

    /// Where the pointer is, by an event `view` was given (a right-click: the event's point is the view's own).
    pub fn pointer(view: ViewId, e: &Event) -> At {
        let (o, p) = (view.layout_rect().origin(), e.point().unwrap_or_default());
        At::point(view, o.x + p.x, o.y + p.y)
    }

    /// Under a view (a button's menu), their lefts level.
    pub fn under(view: ViewId) -> At {
        At { view, by: view.layout_rect(), beside: false, back: None }
    }

    /// Beside a view (the activity bar's buttons), their tops level.
    pub fn beside(view: ViewId) -> At {
        At { view, by: view.layout_rect(), beside: true, back: None }
    }

    /// The view that has the keys again once the menu has closed.
    pub fn back(mut self, to: ViewId) -> At {
        self.back = Some(to);
        self
    }
}

/// The menu that is open: its layer over the window, the rows lit, and the view the keys go back to.
#[derive(Clone, Copy)]
struct Open {
    layer: RwSignal<Option<ViewId>>,
    lit: RwSignal<Lit>,
    back: RwSignal<Option<ViewId>>,
}

impl crate::ext::State for Open {
    fn new(ui: Ui) -> Open {
        Open {
            layer: ui.scope.create_rw_signal(None),
            lit: ui.scope.create_rw_signal(Lit::default()),
            back: ui.scope.create_rw_signal(None),
        }
    }
}

/// Whether a menu is open.
pub fn is_open(ui: Ui) -> bool {
    ui.state::<Open>().layer.get_untracked().is_some()
}

/// The menu closed, and the keys back where it was asked they go.
pub fn close(ui: Ui) {
    let open = ui.state::<Open>();
    if let Some(id) = open.layer.get_untracked() {
        open.layer.set(None);
        remove_overlay(id);
        if let Some(back) = open.back.get_untracked() {
            back.request_focus();
        }
    }
}

/// A row chosen: the menu closes, then the row runs.
fn run(ui: Ui, e: &Entry) {
    if let Some(run) = e.run.clone().filter(|_| e.enabled) {
        close(ui);
        run();
    }
}

/// Opens a menu of these rows (none: no menu). One that is open closes first.
pub fn open(ui: Ui, at: At, items: Vec<Item>) {
    let state = ui.state::<Open>();
    // The first's keys go where the second says, not back and forth.
    state.back.set(None);
    close(ui);
    if items.is_empty() {
        return;
    }
    ui.menu.set(None);
    ui.context.set(None);
    let window = window_size(ui, Some(at.view));
    let items = Rc::new(items);
    state.lit.set(Lit(vec![None]));
    state.back.set(at.back);
    let keyed = items.clone();
    let key = move |key: &Key, _| {
        let press_of = match key {
            Key::Named(NamedKey::ArrowDown) => Press::Down,
            Key::Named(NamedKey::ArrowUp) => Press::Up,
            Key::Named(NamedKey::Home) => Press::First,
            Key::Named(NamedKey::End) => Press::Last,
            Key::Named(NamedKey::ArrowRight) => Press::In,
            Key::Named(NamedKey::ArrowLeft) => Press::Out,
            Key::Named(NamedKey::Enter | NamedKey::Space) => Press::Take,
            Key::Named(NamedKey::Escape) => Press::Leave,
            _ => return,
        };
        let mut lit = state.lit.get_untracked();
        let did = press(&keyed, &mut lit, press_of);
        if state.lit.with_untracked(|was| *was != lit) {
            state.lit.set(lit);
        }
        match did {
            Did::Nothing => {}
            Did::Close => close(ui),
            Did::Run(path) => {
                if let Some(e) = entry_at(&keyed, &path) {
                    run(ui, e);
                }
            }
        }
    };
    let id = cover(
        ui,
        window,
        false,
        key,
        move || close(ui),
        move |keep| {
            let measured = items.clone();
            let drawn = items.clone();
            dyn_stack(
                move || {
                    let lit = state.lit.get();
                    let width = |text: &str| text_width(text, 13.0);
                    rects(&measured, &lit, at.by, at.beside, window, &width)
                        .into_iter()
                        .enumerate()
                        .map(|(level, rect)| (level, lit.0[..level].to_vec(), (rect.x0, rect.y0, rect.width())))
                        .collect::<Vec<_>>()
                },
                |(level, path, _)| (*level, path.clone()),
                move |(level, _, place)| panel(ui, state, drawn.clone(), level, place, keep.clone()),
            )
            .style(|s| s.absolute().inset(0.0).size_full())
            .into_any()
        },
    );
    state.layer.set(Some(id));
}

/// One of the open menus: its rows, at its place in the window (its left, its top, its width).
fn panel(ui: Ui, state: Open, all: Rc<Vec<Item>>, level: usize, place: (f64, f64, f64), keep: Keep) -> impl IntoView {
    let t = ui.colors;
    let rows = state.lit.with_untracked(|lit| rows_at(&all, lit, level).map(<[Item]>::to_vec)).unwrap_or_default();
    let list =
        v_stack_from_iter(rows.into_iter().enumerate().map(|(i, item)| row(ui, state, all.clone(), level, i, item)))
            .style(move |s| {
                let c = t.get();
                s.absolute()
                    .inset_left(place.0)
                    .inset_top(place.1)
                    .width(place.2)
                    .padding_vert(4.0)
                    .border(1.0)
                    .border_color(c.menu_border)
                    .border_radius(6.0)
                    .background(c.menu_bg)
                    .box_shadow_blur(8.0)
                    .box_shadow_color(c.shadow)
                    .font_size(13.0)
                    .selectable(false)
            });
    solid(list, keep)
}

/// A row of a menu: its mark, its name, its keys, and the arrow of a row that opens a menu.
fn row(ui: Ui, state: Open, all: Rc<Vec<Item>>, level: usize, i: usize, item: Item) -> AnyView {
    let t = ui.colors;
    let Item::Entry(e) = item else {
        return empty().style(move |s| s.height(1.0).margin_vert(4.0).background(t.get().menu_separator)).into_any();
    };
    let mark = match e.mark {
        Mark::Check(true) => icon("check", 16.0).into_any(),
        Mark::Radio(true) => icon("circle-small-filled", 16.0).into_any(),
        _ => empty().into_any(),
    };
    let (name, keys, enabled, opens) = (e.label.clone(), e.keys.clone(), e.enabled, !e.sub.is_empty());
    let arrow = if opens { icon("chevron-right", 16.0).into_any() } else { empty().into_any() };
    let pointed = all.clone();
    h_stack((
        container(mark).style(|s| s.width(BEFORE).flex_shrink(0.0_f32).items_center().justify_center()),
        label(move || name.clone()).style(|s| s.flex_grow(1.0_f32).min_width(0.0).text_ellipsis()),
        label(move || keys.clone()).style(|s| s.padding_left(30.0).flex_shrink(0.0_f32)),
        arrow,
    ))
    .on_event_cont(EventListener::PointerEnter, move |_| {
        let to = state.lit.with_untracked(|lit| point(&pointed, lit, level, i));
        if state.lit.with_untracked(|lit| *lit != to) {
            state.lit.set(to);
        }
    })
    .on_click_stop(move |_| {
        if !e.enabled {
            return;
        }
        if e.sub.is_empty() {
            run(ui, &e);
        } else {
            // A click on a row that opens a menu: its menu, as the pointer coming on it opens it.
            let to = state.lit.with_untracked(|lit| point(&all, lit, level, i));
            if state.lit.with_untracked(|lit| *lit != to) {
                state.lit.set(to);
            }
        }
    })
    .style(move |s| {
        let c = t.get();
        let lit = state.lit.with(|lit| lit.0.get(level).copied().flatten() == Some(i));
        let s = s
            .height(ROW)
            .items_center()
            .padding_right(if opens { 4.0 } else { AFTER })
            .border_radius(4.0)
            .margin_horiz(4.0);
        if !enabled {
            s.color(c.menu_fg.multiply_alpha(0.4))
        } else if lit {
            s.background(c.menu_selected).color(c.menu_selected_fg).cursor(CursorStyle::Pointer)
        } else {
            s.color(c.menu_fg).cursor(CursorStyle::Pointer)
        }
    })
    .into_any()
}

/// A menu's rows as the registry has them now (`menus::rows`), each running its command; one whose command may
/// not run now is dimmed.
pub fn of_rows(ui: Ui, send: &Send, rows: Vec<Row>) -> Vec<Item> {
    rows.into_iter()
        .map(|r| match r {
            Row::Line => Item::Line,
            Row::Entry { title, keys, command, .. } => {
                let (send, enabled) = (send.clone(), command.is_some());
                let id = command.unwrap_or_default();
                entry(&title, move || {
                    crate::commands::run(ui, &send, &id);
                })
                .keys(&keys)
                .enabled(enabled)
                .into()
            }
        })
        .collect()
}

/// Opens the menu the window's parts registered for a place. What a row's command is about is the window's state
/// (the row selected, the tab shown): set it before this.
pub fn open_menu(ui: Ui, send: &Send, at: At, menu: Menu) {
    open(ui, at, of_rows(ui, send, crate::menus::rows(ui, menu)));
}

/// What it adds to the window's registries, once at start: nothing (a menu is opened by the part it is of).
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in: none are.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Cut, Copy (dimmed), a line, Word Wrap (checked), Sort By (Name, Size (dimmed), a line, Kind > (A, B)).
    fn menu(ran: Rc<RefCell<Vec<&'static str>>>) -> Vec<Item> {
        let say = |what: &'static str| {
            let ran = ran.clone();
            move || ran.borrow_mut().push(what)
        };
        vec![
            entry("Cut", say("cut")).keys("Ctrl+X").into(),
            entry("Copy", say("copy")).enabled(false).into(),
            line(),
            entry("Word Wrap", say("wrap")).checked(true).into(),
            submenu(
                "Sort By",
                vec![
                    entry("Name", say("name")).radio(true).into(),
                    entry("Size", say("size")).enabled(false).into(),
                    line(),
                    submenu("Kind", vec![entry("A", say("a")).into(), entry("B", say("b")).into()]).into(),
                ],
            )
            .into(),
        ]
    }

    fn lit(at: &[Option<usize>]) -> Lit {
        Lit(at.to_vec())
    }

    #[test]
    fn the_arrows_go_past_the_lines_and_the_dimmed_rows_and_around_the_ends() {
        let items = menu(Rc::default());
        let mut at = lit(&[None]);
        let mut go = |key| {
            assert_eq!(press(&items, &mut at, key), Did::Nothing);
            at.0.clone()
        };
        assert_eq!(go(Press::Down), [Some(0)], "nothing lit: the first row");
        assert_eq!(go(Press::Down), [Some(3)], "Copy is dimmed, then a line");
        assert_eq!(go(Press::Down), [Some(4)]);
        assert_eq!(go(Press::Down), [Some(0)], "around");
        assert_eq!(go(Press::Up), [Some(4)]);
        assert_eq!(go(Press::Up), [Some(3)]);
        assert_eq!(go(Press::First), [Some(0)]);
        assert_eq!(go(Press::Last), [Some(4)]);
        let mut none = lit(&[None]);
        assert_eq!(press(&items, &mut none, Press::Up), Did::Nothing);
        assert_eq!(none, lit(&[Some(4)]), "nothing lit: Up begins at the last row");
        // A menu with no row to stop on: nothing is lit.
        let lines = vec![line(), entry("Off", || {}).enabled(false).into()];
        let mut at = lit(&[None]);
        press(&lines, &mut at, Press::Down);
        assert_eq!(at, lit(&[None]));
    }

    #[test]
    fn right_and_enter_open_a_rows_menu_and_left_and_escape_close_it() {
        let ran = Rc::new(RefCell::new(Vec::new()));
        let items = menu(ran.clone());
        let mut at = lit(&[Some(0)]);
        assert_eq!(press(&items, &mut at, Press::In), Did::Nothing);
        assert_eq!(at, lit(&[Some(0)]), "Cut opens no menu");
        at = lit(&[Some(4)]);
        press(&items, &mut at, Press::In);
        assert_eq!(at, lit(&[Some(4), Some(0)]), "its first row lit");
        press(&items, &mut at, Press::Down);
        assert_eq!(at, lit(&[Some(4), Some(3)]), "Size is dimmed");
        assert_eq!(press(&items, &mut at, Press::Take), Did::Nothing);
        assert_eq!(at, lit(&[Some(4), Some(3), Some(0)]), "Enter on a row that opens a menu opens it");
        press(&items, &mut at, Press::Down);
        assert_eq!(press(&items, &mut at, Press::Take), Did::Run(vec![4, 3, 1]));
        assert_eq!(entry_at(&items, &[4, 3, 1]).map(|e| e.label.as_str()), Some("B"));
        // Left and Escape close the innermost; Escape in the menu itself closes it all, Left does not.
        assert_eq!(press(&items, &mut at, Press::Out), Did::Nothing);
        assert_eq!(at, lit(&[Some(4), Some(3)]));
        assert_eq!(press(&items, &mut at, Press::Leave), Did::Nothing);
        assert_eq!(at, lit(&[Some(4)]));
        assert_eq!(press(&items, &mut at, Press::Out), Did::Nothing);
        assert_eq!(at, lit(&[Some(4)]));
        assert_eq!(press(&items, &mut at, Press::Leave), Did::Close);
        // Enter on a row that runs says which; on nothing, nothing.
        assert_eq!(press(&items, &mut lit(&[Some(3)]), Press::Take), Did::Run(vec![3]));
        assert_eq!(press(&items, &mut lit(&[None]), Press::Take), Did::Nothing);
        assert_eq!(press(&items, &mut lit(&[Some(1)]), Press::Take), Did::Nothing, "a dimmed row");
        assert!(ran.borrow().is_empty(), "the keys' logic runs nothing itself");
    }

    #[test]
    fn the_pointer_lights_a_row_opens_its_menu_and_closes_the_deeper_ones() {
        let items = menu(Rc::default());
        let at = point(&items, &lit(&[None]), 0, 4);
        assert_eq!(at, lit(&[Some(4), None]), "Sort By's menu opens, nothing lit in it");
        let deep = point(&items, &at, 1, 3);
        assert_eq!(deep, lit(&[Some(4), Some(3), None]));
        assert_eq!(point(&items, &deep, 1, 3), deep, "the same row again: as it was");
        assert_eq!(point(&items, &deep, 1, 0), lit(&[Some(4), Some(0)]), "another row of the middle menu");
        assert_eq!(point(&items, &deep, 0, 0), lit(&[Some(0)]));
        assert_eq!(point(&items, &deep, 0, 1), lit(&[None]), "a dimmed row is not lit");
        assert_eq!(point(&items, &deep, 0, 2), lit(&[None]), "nor a line");
        assert_eq!(point(&items, &deep, 7, 0), deep, "no menu that deep");
    }

    #[test]
    fn a_menu_is_as_wide_as_its_names_and_keys_and_stays_in_the_window() {
        let items = menu(Rc::default());
        let width = |text: &str| text.len() as f64 * 7.0;
        // "Word Wrap" (63) and "Ctrl+X" (42 + 30) with the arrow's room, and four rows and a line.
        let size = size_of(&items, &width);
        assert_eq!(size, Size::new(10.0 + 26.0 + 63.0 + 72.0 + 16.0 + 20.0, 10.0 + 4.0 * 26.0 + 9.0));
        assert_eq!(size_of(&[entry("A", || {}).into()], &width).width, 160.0, "never narrower than VS Code's");
        let window = Size::new(800.0, 600.0);
        let at = |x: f64, y: f64| Rect::new(x, y, x, y);
        assert_eq!(place(at(100.0, 100.0), false, size, window), Point::new(100.0, 100.0));
        assert_eq!(place(at(790.0, 100.0), false, size, window).x, 800.0 - size.width - 4.0);
        assert_eq!(place(at(100.0, 590.0), false, size, window).y, 590.0 - size.height, "above the pointer");
        // Under a button, or above it; beside one, or at its other side.
        let button = Rect::new(10.0, 560.0, 40.0, 590.0);
        assert_eq!(place(button, false, size, window), Point::new(10.0, 560.0 - size.height));
        assert_eq!(place(button, true, size, window), Point::new(40.0, 600.0 - size.height - 4.0));
        let right = Rect::new(760.0, 10.0, 790.0, 40.0);
        assert_eq!(place(right, true, size, window), Point::new(760.0 - size.width, 10.0));
        // A window smaller than the menu: at its top left.
        assert_eq!(place(at(5.0, 5.0), false, size, Size::new(100.0, 50.0)), Point::ZERO);
    }

    #[test]
    fn a_rows_menu_opens_beside_the_row_or_at_the_other_side_when_there_is_no_room() {
        let items = menu(Rc::default());
        let width = |text: &str| text.len() as f64 * 7.0;
        let window = Size::new(800.0, 600.0);
        let open = lit(&[Some(4), Some(3), None]);
        let all = rects(&items, &open, Rect::new(100.0, 100.0, 100.0, 100.0), false, window, &width);
        assert_eq!(all.len(), 3);
        // Sort By is the fourth row after a line: its menu's first row is level with it.
        assert_eq!(all[1].origin(), Point::new(all[0].x1 - 3.0, 100.0 + 3.0 * 26.0 + 9.0));
        assert_eq!(all[2].origin(), Point::new(all[1].x1 - 3.0, all[1].y0 + 2.0 * 26.0 + 9.0));
        // No room at the right: at the left of the menu it comes from.
        let tight = rects(&items, &open, Rect::new(560.0, 100.0, 560.0, 100.0), false, window, &width);
        assert!(tight[1].x1 <= tight[0].x0 + 3.0 || tight[1].x0 >= tight[0].x1 - 3.0);
        assert!(tight.iter().all(|r| r.x0 >= 0.0 && r.x1 <= 800.0));
        // Only the menus that are open.
        assert_eq!(rects(&items, &lit(&[None]), Rect::ZERO, false, window, &width).len(), 1);
    }

    #[test]
    fn the_registrys_rows_run_their_commands_and_dim_the_ones_that_may_not_run() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        crate::commands::add(crate::commands::command("test.popup.go", "Test", "Go", |ui, _| {
            ui.message.set("went".into())
        }));
        let rows = vec![
            Row::Entry { title: "Go".into(), keys: "F9".into(), icon: "", command: Some("test.popup.go".into()) },
            Row::Line,
            Row::Entry { title: "Later".into(), keys: String::new(), icon: "", command: None },
        ];
        let items = of_rows(ui, &send, rows);
        assert!(matches!(items[1], Item::Line));
        let Item::Entry(go) = &items[0] else { panic!("a row") };
        assert_eq!((go.label.as_str(), go.keys.as_str(), go.enabled), ("Go", "F9", true));
        let Item::Entry(later) = &items[2] else { panic!("a row") };
        assert!(!later.enabled);
        // Run as a click runs it (no menu is open: nothing to close).
        run(ui, go);
        assert_eq!(ui.message.get_untracked(), "went");
        ui.message.set(String::new());
        run(ui, later);
        assert_eq!(ui.message.get_untracked(), "");
        assert!(!is_open(ui));
    }
}
