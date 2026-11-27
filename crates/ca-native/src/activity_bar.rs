//! The activity bar, as VS Code's: the side bar's views by their icons (all the views in the window when no group
//! is at its left), each with its badge when it has a count (`badges.rs`), each dragged to reorder it in the bar (a
//! line shows where it goes) or to dock it elsewhere; the icons scroll when the window is short. At its foot the
//! old Code Arena's buttons: the terminal, the colour theme, Accounts and Manage (the commands of those names,
//! `ACCOUNTS` and `MANAGE`, once a part has added them, else the menus registered for them), Help and About.
//!
//! It is at the window's left (VS Code's default), in a row over the side bar (Top), or hidden; and an icon may be
//! hidden from it (the view stays where it is and comes back to the bar while it is the one shown). Both are kept
//! with the folder's layout (`kept`, under "activity_bar"). The views' menus are here too: an icon's, the bar's, a
//! panel tab's, and every other view's title (`open_view_menu`).

use std::cell::RefCell;

use floem::IntoView;
use floem::kurbo::{Point, Rect};
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::{Decorators, container, dyn_stack, empty, h_stack, label, scroll, stack, tooltip, v_stack};
use floem::{View as _, ViewId};
use serde_json::{Value, json};

use crate::dock::{EDITOR, Node, ROOT, Side};
use crate::icons::icon;
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::widgets::line;
use crate::widgets::popup::{self, At, Item, entry, submenu};

/// The commands the foot's buttons run, under VS Code's ids for those buttons. The parts that have their menus add
/// the commands; until then a click opens the menu registered for the button (`Menu::Accounts`, `Menu::Manage`).
pub const ACCOUNTS: &str = "workbench.actions.accounts";
pub const MANAGE: &str = "workbench.actions.manage";

/// VS Code's commands that place the bar, and the ones that hide or show its icons.
pub const LOCATION_DEFAULT: &str = "workbench.action.activityBarLocation.default";
pub const LOCATION_TOP: &str = "workbench.action.activityBarLocation.top";
pub const LOCATION_HIDE: &str = "workbench.action.activityBarLocation.hide";
pub const TOGGLE_VISIBILITY: &str = "workbench.action.toggleActivityBarVisibility";
pub const HIDE_VIEW: &str = "codeArena.hideFromActivityBar";
pub const SHOW_ALL: &str = "codeArena.showAllInActivityBar";
/// VS Code's commands that move the panel beside the editor.
pub const PANEL_LEFT: &str = "workbench.action.positionPanelLeft";
pub const PANEL_RIGHT: &str = "workbench.action.positionPanelRight";
pub const PANEL_BOTTOM: &str = "workbench.action.positionPanelBottom";

/// Where the bar is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    /// At the window's left, as VS Code's default.
    Default,
    /// A row over the side bar.
    Top,
    Hidden,
}

impl Location {
    fn id(self) -> &'static str {
        match self {
            Location::Default => "default",
            Location::Top => "top",
            Location::Hidden => "hidden",
        }
    }

    fn of(id: &str) -> Location {
        match id {
            "top" => Location::Top,
            "hidden" => Location::Hidden,
            _ => Location::Default,
        }
    }
}

/// The bar's own state: where it is, and the icons hidden from it.
#[derive(Clone, Copy)]
struct Bar {
    location: RwSignal<Location>,
    hidden: RwSignal<Vec<View>>,
}

impl crate::ext::State for Bar {
    fn new(ui: Ui) -> Bar {
        Bar { location: ui.scope.create_rw_signal(Location::Default), hidden: ui.scope.create_rw_signal(Vec::new()) }
    }
}

thread_local! {
    /// The bars drawn (the one at the left, the row at the top) and their icons, to find where a dragged view goes.
    static DRAWN: RefCell<Drawn> = RefCell::new(Drawn::default());
}

#[derive(Default)]
struct Drawn {
    side: Option<ViewId>,
    top: Option<ViewId>,
    side_items: Vec<(View, ViewId)>,
    top_items: Vec<(View, ViewId)>,
}

pub fn location(ui: Ui) -> Location {
    ui.state::<Bar>().location.get()
}

/// The bar put somewhere (set only when it moves, so what is drawn from it is not built again for nothing).
pub fn set_location(ui: Ui, at: Location) {
    let l = ui.state::<Bar>().location;
    if l.get_untracked() != at {
        l.set(at);
    }
}

/// Whether the bar at the window's left is drawn: where it is put, or put at the top with no side bar to hold it.
pub fn at_side(ui: Ui) -> bool {
    match location(ui) {
        Location::Default => true,
        Location::Top => ui.drawn().with(|d| d.side_bar().is_none()),
        Location::Hidden => false,
    }
}

/// Whether a view's icon is in the bar (not hidden from it).
pub fn pinned(ui: Ui, v: View) -> bool {
    !ui.state::<Bar>().hidden.with(|h| h.contains(&v))
}

/// The views the bar is for: the side bar's, as the dock draws it; with no group at the window's left, every view
/// that is in the window.
fn views(ui: Ui) -> Vec<View> {
    ui.drawn().with(|d| match d.side_bar().and_then(|g| d.find(g)) {
        Some(Node::Tabs { views, .. }) => views.clone(),
        _ => crate::layout::ready().into_iter().filter(|v| ui.available(*v)).collect(),
    })
}

/// The views the bar lists now: its views whose icons are not hidden, and one that is hidden while it is shown.
pub fn listed(ui: Ui) -> Vec<View> {
    views(ui).into_iter().filter(|v| pinned(ui, *v) || ui.view_shown(*v)).collect()
}

/// How many icons are not hidden: the last of them cannot be.
fn pinned_count(ui: Ui) -> usize {
    views(ui).into_iter().filter(|v| pinned(ui, *v)).count()
}

/// A view's icon hidden from the bar (not the last one), or given back. Hidden while it is shown in the side bar,
/// the next icon's view is shown there instead.
pub fn set_pinned(ui: Ui, v: View, on: bool) {
    let hidden = ui.state::<Bar>().hidden;
    if hidden.with_untracked(|h| h.contains(&v)) != on {
        return;
    }
    if !on && pinned_count(ui) <= 1 {
        return;
    }
    hidden.update(|h| {
        h.retain(|x| *x != v);
        if !on {
            h.push(v);
        }
    });
    if !on {
        let shown = ui.dock.with_untracked(|d| {
            d.side_bar().and_then(|g| d.find(g)).is_some_and(|n| matches!(n, Node::Tabs { active, .. } if *active == v))
        });
        if shown && let Some(other) = views(ui).into_iter().find(|x| *x != v && pinned(ui, *x)) {
            ui.dock.update(|d| *d = d.shown(other));
        }
    }
}

/// What is kept with the folder's layout.
pub fn kept(ui: Ui) -> Value {
    let bar = ui.state::<Bar>();
    json!({
        "location": bar.location.get_untracked().id(),
        "hidden": bar.hidden.with_untracked(|h| h.iter().map(|v| v.id()).collect::<Vec<_>>()),
    })
}

fn restore(ui: Ui, v: &Value) {
    set_location(ui, Location::of(v["location"].as_str().unwrap_or("")));
    let hidden: Vec<View> =
        v["hidden"].as_array().into_iter().flatten().filter_map(|x| x.as_str().and_then(View::from_id)).collect();
    let h = ui.state::<Bar>().hidden;
    if h.with_untracked(|now| *now != hidden) {
        h.set(hidden);
    }
}

/// Read by what keeps the layout, so a change here is kept too.
pub fn track(ui: Ui) {
    let bar = ui.state::<Bar>();
    bar.location.track();
    bar.hidden.track();
}

/// Where a view dragged over a bar would go: before which of its icons (none: last), and the line drawn there.
/// The icons' rects are in the bar's order; `across` is a row's (the top), else a column's.
pub fn drop_place(items: &[(View, Rect)], p: Point, across: bool) -> (Option<View>, Rect) {
    let mid = |r: &Rect| if across { r.center().x } else { r.center().y };
    let at = if across { p.x } else { p.y };
    match items.iter().find(|(_, r)| at < mid(r)) {
        Some((v, r)) if across => (Some(*v), Rect::new(r.x0 - 1.0, r.y0, r.x0 + 1.0, r.y1)),
        Some((v, r)) => (Some(*v), Rect::new(r.x0, r.y0 - 1.0, r.x1, r.y0 + 1.0)),
        None => match items.last() {
            Some((_, r)) if across => (None, Rect::new(r.x1 - 1.0, r.y0, r.x1 + 1.0, r.y1)),
            Some((_, r)) => (None, Rect::new(r.x0, r.y1 - 1.0, r.x1, r.y1 + 1.0)),
            None => (None, Rect::ZERO),
        },
    }
}

/// While a view is dragged: over a bar that is drawn, where in it it would go.
pub fn aim(ui: Ui, p: Point) -> Option<(Option<View>, Rect)> {
    let top = location(ui) == Location::Top && !at_side_untracked(ui);
    DRAWN.with(|d| {
        let d = d.borrow();
        let (bar, items) = if top { (d.top, &d.top_items) } else { (d.side, &d.side_items) };
        let r = bar?.layout_rect();
        if !r.contains(p) || r.area() <= 0.0 {
            return None;
        }
        let order = listed_untracked(ui);
        let rects: Vec<(View, Rect)> = order
            .into_iter()
            .filter_map(|v| items.iter().find(|(x, _)| *x == v).map(|(_, id)| (v, id.layout_rect())))
            .collect();
        Some(drop_place(&rects, p, top))
    })
}

fn at_side_untracked(ui: Ui) -> bool {
    floem::reactive::untrack(|| at_side(ui))
}

fn listed_untracked(ui: Ui) -> Vec<View> {
    floem::reactive::untrack(|| listed(ui))
}

/// A view dropped on a bar: in the side bar before `before` (none: last), its icon given back, and shown.
pub fn dropped(ui: Ui, v: View, before: Option<View>) {
    let side = ui.dock.with_untracked(|d| d.side_bar());
    match side {
        Some(g) => {
            ui.dock_view(v, g, Side::Center);
            let next = ui.dock.with_untracked(|d| d.reordered(v, before));
            if ui.dock.with_untracked(|d| *d != next) {
                ui.dock.set(next);
            }
        }
        None => ui.dock_view(v, ROOT, Side::Left),
    }
    set_pinned(ui, v, true);
}

/// A view moved to the panel's tabs (with no panel, a group of its own under the editor).
pub fn move_to_panel(ui: Ui, v: View) {
    match ui.dock.with_untracked(|d| d.panel()) {
        Some(p) => ui.dock_view(v, p, Side::Center),
        None => ui.dock_view(v, EDITOR, Side::Bottom),
    }
}

/// A view moved to the side bar (with none, a group of its own at the window's left).
pub fn move_to_side_bar(ui: Ui, v: View) {
    match ui.dock.with_untracked(|d| d.side_bar()) {
        Some(g) => ui.dock_view(v, g, Side::Center),
        None => ui.dock_view(v, ROOT, Side::Left),
    }
    set_pinned(ui, v, true);
}

/// The panel moved whole to a side of the editor.
pub fn move_panel(ui: Ui, side: Side) {
    let Some(p) = ui.dock.with_untracked(|d| d.panel()) else { return };
    let next = ui.dock.with_untracked(|d| d.group_moved(p, side));
    if ui.dock.with_untracked(|d| *d != next) {
        ui.dock.set(next);
    }
}

fn nudge(ui: Ui, v: View, by: i32) {
    ui.dock.update(|d| *d = d.nudged(v, by));
}

/// A group named as a menu says it: the side bar, the panel, else by the view it shows.
fn group_name(ui: Ui, g: u32) -> String {
    ui.dock.with_untracked(|d| {
        if d.side_bar() == Some(g) {
            "Primary Side Bar".to_string()
        } else if d.panel() == Some(g) {
            "Panel".to_string()
        } else {
            match d.find(g) {
                Some(Node::Tabs { active, .. }) => active.title().to_string(),
                _ => String::new(),
            }
        }
    })
}

/// The bar's own rows: a check per view, where the bar is, the views put back.
fn bar_items(ui: Ui) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let last = pinned_count(ui) <= 1;
    for v in floem::reactive::untrack(|| views(ui)) {
        let on = floem::reactive::untrack(|| pinned(ui, v));
        items.push(entry(v.title(), move || set_pinned(ui, v, !on)).checked(on).enabled(!(on && last)).into());
    }
    items.push(popup::line());
    let now = floem::reactive::untrack(|| location(ui));
    let place = |name: &str, at: Location| -> Item { entry(name, move || set_location(ui, at)).radio(now == at).into() };
    items.push(
        submenu(
            "Activity Bar Position",
            vec![place("Default", Location::Default), place("Top", Location::Top), place("Hidden", Location::Hidden)],
        )
        .into(),
    );
    items.push(popup::line());
    items.push(entry("Reset View Locations", move || ui.reset_dock()).into());
    items
}

/// Visual Studio's docking rows: beside the editor, into the tabs of any other group, in a window of its own.
fn dock_items(ui: Ui, v: View) -> Vec<Item> {
    let mut beside = Vec::new();
    for (side, name) in [
        (Side::Left, "Left of the Editor"),
        (Side::Right, "Right of the Editor"),
        (Side::Bottom, "Below the Editor"),
        (Side::Top, "Above the Editor"),
    ] {
        beside.push(entry(name, move || ui.dock_view(v, EDITOR, side)).into());
    }
    let mine = ui.dock.with_untracked(|d| d.group_of(v));
    let with: Vec<Item> = ui
        .dock
        .with_untracked(|d| d.all_groups())
        .into_iter()
        .filter(|g| Some(*g) != mine)
        .map(|g| entry(&group_name(ui, g), move || ui.dock_view(v, g, Side::Center)).into())
        .collect();
    let mut items: Vec<Item> = vec![submenu("Dock", beside).into()];
    if !with.is_empty() {
        items.push(submenu("Dock With", with).into());
    }
    items.push(entry("Float", move || ui.float(v)).into());
    items
}

/// A view's own menu: an activity bar icon's (hide it, move it to the panel, up or down, then the bar's rows), a
/// panel tab's (move it to the side bar, the panel elsewhere), any other view's (hide its group); then the
/// docking rows.
pub fn view_items(ui: Ui, v: View) -> Vec<Item> {
    let (group, side_bar, panel) = ui.dock.with_untracked(|d| (d.group_of(v), d.side_bar(), d.panel()));
    let in_bar = floem::reactive::untrack(|| views(ui)).contains(&v);
    let in_panel = !in_bar && group.is_some() && group == panel;
    let mut items: Vec<Item> = Vec::new();
    if in_bar {
        let on = floem::reactive::untrack(|| pinned(ui, v));
        let last = pinned_count(ui) <= 1;
        items.push(entry(&format!("Hide '{}'", v.title()), move || set_pinned(ui, v, false)).enabled(on && !last).into());
        items.push(entry("Move to Panel", move || move_to_panel(ui, v)).enabled(group != panel).into());
        if group.is_some() && group == side_bar {
            let order = floem::reactive::untrack(|| views(ui));
            let at = order.iter().position(|x| *x == v).unwrap_or(0);
            items.push(entry("Move Up", move || nudge(ui, v, -1)).enabled(at > 0).into());
            items.push(entry("Move Down", move || nudge(ui, v, 1)).enabled(at + 1 < order.len()).into());
        }
    } else if in_panel {
        items.push(entry("Move to Side Bar", move || move_to_side_bar(ui, v)).into());
        let now = panel.and_then(|p| ui.dock.with_untracked(|d| d.side_of_editor(p)));
        for (side, name) in
            [(Side::Bottom, "Move Panel to Bottom"), (Side::Left, "Move Panel Left"), (Side::Right, "Move Panel Right")]
        {
            items.push(entry(name, move || move_panel(ui, side)).enabled(now != Some(side)).into());
        }
        items.push(entry("Hide Panel", move || ui.toggle_group_of(v)).into());
    } else {
        items.push(entry(&format!("Hide {}", v.title()), move || ui.toggle_group_of(v)).into());
    }
    items.push(popup::line());
    items.extend(dock_items(ui, v));
    items.push(popup::line());
    if in_bar {
        items.extend(bar_items(ui));
    } else {
        items.push(entry("Reset View Locations", move || ui.reset_dock()).into());
    }
    items
}

/// A view's menu opened where it was right-clicked.
pub fn open_view_menu(ui: Ui, at: At, v: View) {
    popup::open(ui, at, view_items(ui, v));
}

/// A foot button: runs its command when a part has added it, else opens the menu registered for it beside it.
fn foot(ui: Ui, send: Send, name: &'static str, tip: &'static str, command: &'static str) -> impl IntoView {
    let t = ui.colors;
    let button = container(icon(name, 24.0));
    let id = button.id();
    let button = button
        .on_click_stop(move |_| {
            let menu = match command {
                ACCOUNTS => Some(crate::menus::Menu::Accounts),
                MANAGE => Some(crate::menus::Menu::Manage),
                _ => None,
            };
            match menu {
                Some(m) if crate::commands::find(command).is_none() => popup::open_menu(ui, &send, At::beside(id), m),
                None if command == HELP_MENU => popup::open_menu(
                    ui,
                    &send,
                    At::beside(id),
                    crate::menus::Menu::TitleBar(crate::menus::HELP),
                ),
                _ => {
                    crate::commands::run(ui, &send, command);
                }
            }
        })
        .style(move |s| {
            let c = t.get();
            s.width(48.0)
                .height(48.0)
                .items_center()
                .justify_center()
                .cursor(CursorStyle::Pointer)
                .color(c.activity_inactive)
                .hover(|s| s.color(c.activity_fg))
        });
    tooltip(button, move || {
        let keys = crate::commands::keys_of(command);
        label(move || if keys.is_empty() { tip.to_string() } else { format!("{tip} ({keys})") })
    })
}

/// What the Help button "runs": the title bar's Help menu, opened beside it.
const HELP_MENU: &str = "codeArena.activityBar.help";

/// VS Code's badge: a count at the icon's lower right, there only while there is one.
fn badge(ui: Ui, v: View, top: f64, right: f64) -> impl IntoView {
    let t = ui.colors;
    let count = crate::badges::count(ui, v);
    label(move || crate::badges::text(count.get())).style(move |s| {
        let c = t.get();
        s.absolute()
            .inset_top(top)
            .inset_right(right)
            .height(16.0)
            .min_width(16.0)
            .padding_horiz(4.0)
            .items_center()
            .justify_center()
            .border_radius(8.0)
            .font_size(9.0)
            .background(c.badge_bg)
            .color(c.badge_fg)
            .apply_if(count.get() == 0, |s| s.hide())
    })
}

/// The view's name and its keys, as VS Code's hover.
fn hover(v: View) -> impl IntoView {
    let keys = v.keys();
    label(move || if keys.is_empty() { v.title().to_string() } else { format!("{} ({keys})", v.title()) })
}

/// The bar at the window's left.
pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let item = move |v: View| {
        let lit = move || ui.view_shown(v);
        let button = h_stack((
            empty().style(move |s| {
                let c = t.get();
                s.width(2.0).height_full().background(if lit() { c.activity_active_border } else { Color::TRANSPARENT })
            }),
            stack((icon(v.icon(), 24.0), badge(ui, v, 24.0, 6.0)))
                .style(|s| s.flex_grow(1.0_f32).height_full().items_center().justify_center()),
            empty().style(|s| s.width(2.0)),
        ));
        let button = crate::docking::handle(ui, v, button, move || ui.toggle_view(v)).style(move |s| {
            let c = t.get();
            s.width(48.0)
                .height(48.0)
                .cursor(CursorStyle::Pointer)
                .color(if lit() { c.activity_fg } else { c.activity_inactive })
                .hover(|s| s.color(c.activity_fg))
        });
        let button = tooltip(button, move || hover(v));
        let id = button.id();
        DRAWN.with(|d| {
            let mut d = d.borrow_mut();
            d.side_items.retain(|(x, _)| *x != v);
            d.side_items.push((v, id));
        });
        button
    };
    let icons = scroll(dyn_stack(move || listed(ui), |v: &View| *v, item).style(|s| s.flex_col()))
        .scroll_style(|s| s.hide_bars(true))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width(48.0));
    let bar = v_stack((
        icons,
        foot(ui, send.clone(), "terminal", "Toggle Terminal", crate::workbench::TOGGLE_TERMINAL),
        foot(ui, send.clone(), "color-mode", "Color Theme", crate::workbench::SELECT_THEME),
        foot(ui, send.clone(), "account", "Accounts", ACCOUNTS),
        foot(ui, send.clone(), "settings-gear", "Manage", MANAGE),
        foot(ui, send.clone(), "question", "Help", HELP_MENU),
        foot(ui, send, "info", "About", crate::workbench::ABOUT),
    ))
    .on_secondary_click_stop(move |e| open_bar_menu(ui, e))
    .style(move |s| {
        let c = t.get();
        line(s, c.activity_border, 'r')
            .width(48.0)
            .height_full()
            .background(c.activity_bg)
            .apply_if(!at_side(ui), |s| s.hide())
    });
    let id = bar.id();
    DRAWN.with(|d| d.borrow_mut().side = Some(id));
    bar
}

fn open_bar_menu(ui: Ui, e: &floem::event::Event) {
    let at = DRAWN.with(|d| {
        let d = d.borrow();
        if location(ui) == Location::Top { d.top.or(d.side) } else { d.side.or(d.top) }
    });
    if let Some(id) = at {
        popup::open(ui, At::pointer(id, e), bar_items(ui));
    }
}

/// The bar put at the top: a row of small icons over the side bar (drawn in it, so it hides with it), with
/// Accounts and Manage at its right.
pub fn top(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let item = move |v: View| {
        let lit = move || ui.view_shown(v);
        let button = v_stack((
            stack((icon(v.icon(), 16.0), badge(ui, v, 14.0, -2.0)))
                .style(|s| s.flex_grow(1.0_f32).items_center().justify_center()),
            empty().style(move |s| {
                let c = t.get();
                s.height(1.0)
                    .width(16.0)
                    .margin_bottom(4.0)
                    .background(if lit() { c.activity_top_border } else { Color::TRANSPARENT })
            }),
        ))
        .style(|s| s.items_center());
        let button = crate::docking::handle(ui, v, button, move || ui.reveal(v)).style(move |s| {
            let c = t.get();
            s.width(32.0)
                .height_full()
                .cursor(CursorStyle::Pointer)
                .color(if lit() { c.activity_top_fg } else { c.activity_top_inactive })
                .hover(|s| s.color(c.activity_top_fg))
        });
        let button = tooltip(button, move || hover(v));
        let id = button.id();
        DRAWN.with(|d| {
            let mut d = d.borrow_mut();
            d.top_items.retain(|(x, _)| *x != v);
            d.top_items.push((v, id));
        });
        button
    };
    let small = move |name: &'static str, command: &'static str, menu: crate::menus::Menu| {
        let send = send.clone();
        let b = container(icon(name, 16.0));
        let id = b.id();
        b.on_click_stop(move |_| {
            if crate::commands::find(command).is_some() {
                crate::commands::run(ui, &send, command);
            } else {
                popup::open_menu(ui, &send, At::under(id), menu);
            }
        })
        .style(move |s| {
            let c = t.get();
            s.width(28.0)
                .height_full()
                .items_center()
                .justify_center()
                .cursor(CursorStyle::Pointer)
                .color(c.activity_top_inactive)
                .hover(|s| s.color(c.activity_top_fg))
        })
    };
    let row = h_stack((
        scroll(dyn_stack(move || listed(ui), |v: &View| *v, item).style(|s| s.height_full()))
            .scroll_style(|s| s.hide_bars(true))
            .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).height_full()),
        small("account", ACCOUNTS, crate::menus::Menu::Accounts),
        small("settings-gear", MANAGE, crate::menus::Menu::Manage),
    ))
    .on_secondary_click_stop(move |e| open_bar_menu(ui, e))
    .style(move |s| {
        let c = t.get();
        line(s, c.side_border, 'b')
            .height(36.0)
            .width_full()
            .padding_horiz(4.0)
            .items_center()
            .selectable(false)
            .apply_if(location(ui) != Location::Top, |s| s.hide())
    });
    let id = row.id();
    DRAWN.with(|d| d.borrow_mut().top = Some(id));
    row
}

/// What it adds to the window's registries, once at start: the bar's commands, the panel's, the Manage menu's
/// first rows.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    add(command(LOCATION_DEFAULT, "View", "Move Activity Bar to Side", |ui, _| set_location(ui, Location::Default)));
    add(command(LOCATION_TOP, "View", "Move Activity Bar to Top", |ui, _| set_location(ui, Location::Top)));
    add(command(LOCATION_HIDE, "View", "Hide Activity Bar", |ui, _| set_location(ui, Location::Hidden)));
    add(command(TOGGLE_VISIBILITY, "View", "Toggle Activity Bar Visibility", |ui, _| {
        let at = if location(ui) == Location::Hidden { Location::Default } else { Location::Hidden };
        set_location(ui, at);
    }));
    add(command(HIDE_VIEW, "View", "Hide the Shown View from the Activity Bar", |ui, _| {
        if let Some(v) = listed_untracked(ui).into_iter().find(|v| ui.view_shown(*v)) {
            set_pinned(ui, v, false);
        }
    }));
    add(command(SHOW_ALL, "View", "Show All Views in the Activity Bar", |ui, _| {
        let h = ui.state::<Bar>().hidden;
        if h.with_untracked(|h| !h.is_empty()) {
            h.set(Vec::new());
        }
    }));
    add(command(PANEL_LEFT, "View", "Move Panel Left", |ui, _| move_panel(ui, Side::Left)));
    add(command(PANEL_RIGHT, "View", "Move Panel Right", |ui, _| move_panel(ui, Side::Right)));
    add(command(PANEL_BOTTOM, "View", "Move Panel To Bottom", |ui, _| move_panel(ui, Side::Bottom)));
    // The gear's menu, as VS Code's begins: the parts that have settings, keys, profiles add their rows.
    use crate::menus::{Menu, add as row, item};
    row(Menu::Manage, item(crate::workbench::SHOW_COMMANDS).title("Command Palette...").group("1_commands"));
    row(Menu::Manage, item(crate::workbench::SELECT_THEME).title("Themes: Color Theme").group("5_themes"));
    row(Menu::Manage, item(crate::workbench::ABOUT).group("9_about"));
}

/// The session's messages that are this part's, taken in: the folder's layout, read back. Never taken away from
/// the rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "state" && m["view_state"].is_object() {
        restore(ui, &m["view_state"]["activity_bar"]);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::also_ready;

    #[test]
    fn the_bar_lists_the_side_bars_views_that_are_in_the_window() {
        use View::*;
        let ui = Ui::new();
        assert_eq!(listed(ui), [Files, Search, Changes, Outline]);
        // A view withheld is not listed, and is listed where it was when it is given back.
        ui.withhold(Changes, true);
        assert_eq!(listed(ui), [Files, Search, Outline]);
        ui.withhold(Changes, false);
        assert_eq!(listed(ui), [Files, Search, Changes, Outline]);
        // No group at the window's left: every view in the window, and none that is not written.
        for v in [Files, Search, Changes, Outline] {
            ui.dock_view(v, EDITOR, Side::Bottom);
        }
        assert_eq!(listed(ui), crate::layout::ready());
        // A view written since is in the side bar its first place is.
        also_ready(&[Scm]);
        let ui = Ui::new();
        assert_eq!(listed(ui), [Files, Search, Scm, Changes, Outline]);
        also_ready(&[]);
    }

    #[test]
    fn an_icon_hidden_from_the_bar_comes_back_while_its_view_is_shown_and_the_last_one_stays() {
        use View::*;
        let ui = Ui::new();
        ui.reveal(Search);
        // Hidden while it is shown: the next icon's view is shown instead, and Search is gone from the bar.
        set_pinned(ui, Search, false);
        assert!(ui.view_shown(Files));
        assert_eq!(listed(ui), [Files, Changes, Outline]);
        // Shown by its command, it is in the bar again while it is shown; not pinned back.
        ui.reveal(Search);
        assert_eq!(listed(ui), [Files, Search, Changes, Outline]);
        assert!(!pinned(ui, Search));
        ui.reveal(Files);
        assert_eq!(listed(ui), [Files, Changes, Outline]);
        // The last icon cannot be hidden.
        for v in [Files, Changes, Outline] {
            set_pinned(ui, v, false);
        }
        assert_eq!(pinned_count(ui), 1);
        assert!(pinned(ui, Outline));
        // Kept with the layout and read back.
        set_location(ui, Location::Top);
        let kept = kept(ui);
        assert_eq!(kept["location"], "top");
        let again = Ui::new();
        restore(again, &kept);
        assert_eq!(location(again), Location::Top);
        assert!(!pinned(again, Search) && pinned(again, Outline));
        // A layout kept without it: the bar as it first is.
        restore(again, &Value::Null);
        assert_eq!(location(again), Location::Default);
        assert!(pinned(again, Search));
    }

    #[test]
    fn a_view_dropped_on_the_bar_goes_where_the_line_shows() {
        use View::*;
        let rect = |i: f64| Rect::new(0.0, i * 48.0, 48.0, i * 48.0 + 48.0);
        let items = [(Files, rect(0.0)), (Search, rect(1.0)), (Changes, rect(2.0))];
        assert_eq!(drop_place(&items, Point::new(20.0, 10.0), false).0, Some(Files));
        assert_eq!(drop_place(&items, Point::new(20.0, 30.0), false).0, Some(Search));
        let (before, line) = drop_place(&items, Point::new(20.0, 140.0), false);
        assert_eq!(before, None);
        assert_eq!(line.y0, 143.0);
        // In the top row it is across.
        let row = [(Files, Rect::new(0.0, 0.0, 32.0, 36.0)), (Search, Rect::new(32.0, 0.0, 64.0, 36.0))];
        assert_eq!(drop_place(&row, Point::new(40.0, 5.0), true).0, Some(Search));
        // Dropped: in the side bar at that place, shown, and in the bar again.
        let ui = Ui::new();
        set_pinned(ui, Outline, false);
        dropped(ui, Outline, Some(Search));
        assert_eq!(listed(ui), [Files, Outline, Search, Changes]);
        assert!(ui.view_shown(Outline) && pinned(ui, Outline));
        dropped(ui, Agent, None);
        assert_eq!(listed(ui), [Files, Outline, Search, Changes, Agent]);
    }

    #[test]
    fn a_views_menu_moves_it_and_the_panel() {
        use View::*;
        also_ready(&[Output]);
        let ui = Ui::new();
        let labels = |items: &[Item]| -> Vec<String> {
            items
                .iter()
                .filter_map(|i| match i {
                    Item::Entry(e) => Some(e.label.clone()),
                    Item::Line => None,
                })
                .collect()
        };
        let run = |items: &[Item], name: &str| {
            let found = items.iter().find_map(|i| match i {
                Item::Entry(e) if e.label == name => e.run.clone(),
                _ => None,
            });
            found.expect(name)();
        };
        let icon = view_items(ui, Search);
        assert_eq!(labels(&icon)[..4], ["Hide 'Search'", "Move to Panel", "Move Up", "Move Down"]);
        assert!(labels(&icon).contains(&"Activity Bar Position".to_string()));
        // Dock With lists every other group, the side bar and the panel by those names.
        let with = icon.iter().find_map(|i| match i {
            Item::Entry(e) if e.label == "Dock With" => Some(labels(&e.sub)),
            _ => None,
        });
        assert_eq!(with.unwrap(), ["Panel", Agent.title()]);
        run(&icon, "Move Down");
        assert_eq!(listed(ui), [Files, Changes, Search, Outline]);
        run(&view_items(ui, Search), "Move to Panel");
        assert_eq!(ui.dock.with_untracked(|d| d.group_of(Search)), ui.dock.with_untracked(|d| d.panel()));
        // A panel tab's menu: back to the side bar, the panel elsewhere.
        let tab = view_items(ui, Output);
        assert_eq!(labels(&tab)[..5], ["Move to Side Bar", "Move Panel to Bottom", "Move Panel Left", "Move Panel Right", "Hide Panel"]);
        run(&tab, "Move Panel Right");
        let p = ui.dock.with_untracked(|d| d.panel()).unwrap();
        assert_eq!(ui.dock.with_untracked(|d| d.side_of_editor(p)), Some(Side::Right));
        run(&view_items(ui, Search), "Move to Side Bar");
        assert_eq!(listed(ui), [Files, Changes, Outline, Search]);
        also_ready(&[]);
    }
}
