//! The dock tree drawn: each split a row or a column of its parts with a sash between them, each group its title
//! (or its tabs) and the view it shows, the editor where the tree puts it. A view is dragged by its icon, title or
//! tab: past 5 px, Visual Studio's guide shows on the part under the pointer and at the window's edges, the place it
//! would take is shaded, and on release it docks there (Escape gives up).

use std::cell::RefCell;
use std::collections::HashMap;

use floem::event::{Event, EventListener, EventPropagation};
use floem::kurbo::{Point, Rect};
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, Position};
use floem::views::{
    Decorators, container, dyn_container, empty, h_stack, h_stack_from_iter, label, v_stack, v_stack_from_iter,
};
use floem::{IntoView, View as _, ViewId};

use crate::dock::{self, Dir, Dock, EDITOR, Node, ROOT, Side};
use crate::icons::icon;
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::widgets::{action, caps, line};

thread_local! {
    /// Each drawn part's view, by its number (the window's dock area is `ROOT`), to find what the pointer is over.
    static PARTS: RefCell<HashMap<u32, ViewId>> = RefCell::new(HashMap::new());
    /// The floating views' windows.
    static WINDOWS: RefCell<HashMap<View, floem::window::WindowId>> = RefCell::new(HashMap::new());
    /// How the window speaks to the session (a floating view's window draws its view with it).
    static SEND: RefCell<Option<Send>> = const { RefCell::new(None) };
}

pub fn set_send(send: Send) {
    SEND.with(|s| *s.borrow_mut() = Some(send));
}

fn rect_of(id: u32) -> Option<Rect> {
    PARTS.with(|p| p.borrow().get(&id).map(|v| v.layout_rect()))
}

fn keep(id: u32, view: ViewId) {
    PARTS.with(|p| p.borrow_mut().insert(id, view));
}

/// A drag going on: the view, where the pointer is, the part whose guide shows, where it would dock, what is shaded.
#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub view: View,
    pub at: Point,
    pub over: Option<(u32, Rect)>,
    pub drop: Option<(u32, Side)>,
    pub shade: Option<Rect>,
}

/// The least a part may be: a side bar's 170 px across, the panel's 77 down; the editor's 220 and 70.
fn least(dir: Dir) -> f64 {
    if dir == Dir::Row { 170.0 } else { 77.0 }
}
fn editor_least(dir: Dir) -> f64 {
    if dir == Dir::Row { 220.0 } else { 70.0 }
}

impl Ui {
    /// A part's size (across in a row, down in a column), its first one until it is resized.
    pub fn size(&self, id: u32) -> RwSignal<f64> {
        if let Some(s) = self.sizes.with_untracked(|m| m.get(&id).copied()) {
            return s;
        }
        let first = dock::FIRST_SIZES.iter().find(|(i, _)| *i == id).map(|(_, s)| *s).unwrap_or(dock::NEW_SIZE);
        let s = self.scope.create_rw_signal(first);
        self.sizes.update(|m| {
            m.insert(id, s);
        });
        s
    }

    pub fn hidden(&self, group: u32) -> bool {
        self.hidden.with(|h| h.contains(&group))
    }

    /// Whether the view is on screen: its group shown, and it the one shown in it.
    pub fn view_shown(&self, v: View) -> bool {
        self.dock.with(|d| {
            d.group_of(v).is_some_and(|g| {
                !self.hidden(g) && matches!(d.find(g), Some(Node::Tabs { active, .. }) if *active == v)
            })
        })
    }

    /// The view on screen: its group shown, it shown in it.
    pub fn reveal(&self, v: View) {
        self.dock.update(|d| *d = d.shown(v));
        if let Some(g) = self.dock.with_untracked(|d| d.group_of(v)) {
            self.hidden.update(|h| {
                h.remove(&g);
            });
        }
    }

    /// An activity bar's click (and a view's key): shown, or its group hidden when it is shown already.
    pub fn toggle_view(&self, v: View) {
        if self.view_shown(v) {
            self.hide_group_of(v);
        } else {
            self.reveal(v);
        }
    }

    fn hide_group_of(&self, v: View) {
        if let Some(g) = self.dock.with_untracked(|d| d.group_of(v)) {
            self.hidden.update(|h| {
                h.insert(g);
            });
        }
    }

    /// Ctrl+B, Ctrl+Alt+B, Ctrl+J: the group holding the Explorer, the Agent, the Terminal hidden or shown.
    pub fn toggle_group_of(&self, v: View) {
        let Some(g) = self.dock.with_untracked(|d| d.group_of(v)) else { return };
        self.hidden.update(|h| {
            if !h.remove(&g) {
                h.insert(g);
            }
        });
    }

    pub fn group_shown(&self, v: View) -> bool {
        self.dock.with(|d| d.group_of(v)).is_some_and(|g| !self.hidden(g))
    }

    pub fn dock_view(&self, v: View, target: u32, side: Side) {
        self.dock.update(|d| *d = d.docked(v, target, side));
        self.reveal(v);
        // Docked from a window of its own: the window goes.
        if let Some(w) = WINDOWS.with(|w| w.borrow_mut().remove(&v)) {
            floem::close_window(w);
        }
    }

    /// The view in a window of its own, as Visual Studio floats a tool window.
    pub fn float(&self, v: View) {
        self.dock.update(|d| *d = d.floated(v));
        open_window(*self, v);
    }

    /// The floating views' windows opened (after the layout is read back).
    pub fn open_floating(&self) {
        for v in self.dock.with_untracked(|d| d.floating.clone()) {
            open_window(*self, v);
        }
    }

    pub fn reset_dock(&self) {
        self.dock.set(Dock::default());
        self.hidden.update(|h| h.clear());
        for (_, w) in WINDOWS.with(|w| std::mem::take(&mut *w.borrow_mut())) {
            floem::close_window(w);
        }
    }
}

/// The dock area: the tree, drawn again only when its shape changes (a view docked), not when a tab is chosen or
/// a part resized or hidden.
pub fn tree(ui: Ui, send: Send) -> impl IntoView {
    let shape = create_memo(move |_| ui.dock.with(|d| shape_of(&d.root)));
    let area = dyn_container(
        move || shape.get(),
        move |_| {
            let root = ui.dock.get_untracked().root;
            node_view(ui, send.clone(), root, Place::Whole).into_any()
        },
    )
    .style(|s| s.size_full());
    let id = area.id();
    keep(ROOT, id);
    area
}

/// The tree's shape: its parts and their views, not which view each shows.
fn shape_of(n: &Node) -> String {
    match n {
        Node::Editor => "E".into(),
        Node::Tabs { id, views, .. } => {
            format!("T{id}[{}]", views.iter().map(|v| v.id()).collect::<Vec<_>>().join(","))
        }
        Node::Split { id, dir, kids } => {
            format!("S{id}{:?}({})", dir, kids.iter().map(shape_of).collect::<Vec<_>>().join(" "))
        }
    }
}

/// Whether a part is drawn: a group not hidden, a split with any part drawn, the editor always.
fn drawn(ui: Ui, n: &Node) -> impl Fn() -> bool + Clone + 'static {
    let mut groups = Vec::new();
    n.groups(&mut groups);
    let editor = n.has_editor();
    move || editor || groups.iter().any(|g| !ui.hidden(*g))
}

/// Where a part is against the part that takes the room (the editor's side): before it or after it, across or down;
/// it gives the part's border side and its look (a panel's under or over the editor, a side bar's beside it).
#[derive(Clone, Copy, PartialEq)]
enum Place {
    Before(Dir),
    After(Dir),
    Whole,
}

fn node_view(ui: Ui, send: Send, n: Node, place: Place) -> impl IntoView {
    match n {
        Node::Editor => {
            let v = container(crate::editor::view(ui)).style(|s| s.size_full().min_width(0.0).min_height(0.0));
            keep(EDITOR, v.id());
            v.into_any()
        }
        Node::Tabs { id, .. } => group_view(ui, send, id, place).into_any(),
        Node::Split { id, dir, kids } => split_view(ui, send, id, dir, kids).into_any(),
    }
}

fn split_view(ui: Ui, send: Send, id: u32, dir: Dir, kids: Vec<Node>) -> impl IntoView {
    let t = ui.colors;
    // The part that takes the room left: the one holding the editor, else the last.
    let flex = kids.iter().position(Node::has_editor).unwrap_or(kids.len().saturating_sub(1));
    let flex_has_editor = kids.get(flex).is_some_and(Node::has_editor);
    let shown: Vec<_> = kids.iter().map(|k| drawn(ui, k)).collect();
    let ids: Vec<u32> = kids.iter().map(Node::id).collect();
    let mut parts = Vec::new();
    for (i, k) in kids.into_iter().enumerate() {
        if i > 0 {
            // The sash resizes the part on the editor's far side of it.
            let (part, sign) = if i <= flex { (i - 1, 1.0) } else { (i, -1.0) };
            let (before, here) = (shown[..i].to_vec(), shown[i].clone());
            let size = ui.size(ids[part]);
            let resized = ids[part];
            let shown_all = shown.clone();
            let ids_all = ids.clone();
            let s = crate::widgets::sash(t, dir == Dir::Row, sign, size, move |want| {
                let others: f64 = ids_all
                    .iter()
                    .enumerate()
                    .filter(|(j, x)| *j != flex && **x != resized && shown_all[*j]())
                    .map(|(_, x)| ui.size(*x).get_untracked())
                    .sum();
                resize(ui, id, resized, dir, flex_has_editor, others, want);
            })
            .style(move |s| s.apply_if(!(here() && before.iter().any(|b| b())), |s| s.hide()));
            parts.push(s.into_any());
        }
        let here = shown[i].clone();
        let size = (i != flex).then(|| ui.size(ids[i]));
        let place = if i < flex { Place::Before(dir) } else { Place::After(dir) };
        let part = container(node_view(ui, send.clone(), k, place)).style(move |s| {
            let s = s.apply_if(!here(), |s| s.hide()).min_width(0.0).min_height(0.0);
            match (size, dir) {
                (None, _) => s.flex_grow(1.0_f32).flex_basis(0.0).size_full(),
                (Some(z), Dir::Row) => s.width(z.get()).height_full().flex_shrink(0.0_f32),
                (Some(z), Dir::Column) => s.height(z.get()).width_full().flex_shrink(0.0_f32),
            }
        });
        parts.push(part.into_any());
    }
    let view = match dir {
        Dir::Row => h_stack_from_iter(parts).into_any(),
        Dir::Column => v_stack_from_iter(parts).into_any(),
    }
    .style(|s| s.size_full());
    keep(id, view.id());
    view
}

/// A part resized by its sash: at least its least (under half of it, a group closes, as VS Code snaps), at most
/// what its split has after the others and the editor's least; a double-click gives it its first size.
fn resize(ui: Ui, split: u32, part: u32, dir: Dir, editor: bool, others: f64, want: f64) {
    let size = ui.size(part);
    if want.is_nan() {
        let first = dock::FIRST_SIZES.iter().find(|(i, _)| *i == part).map(|(_, s)| *s).unwrap_or(dock::NEW_SIZE);
        size.set(first);
        return;
    }
    let is_group = ui.dock.with_untracked(|d| matches!(d.find(part), Some(Node::Tabs { .. })));
    if want < least(dir) / 2.0 && is_group {
        ui.hidden.update(|h| {
            h.insert(part);
        });
        return;
    }
    let extent = rect_of(split).map(|r| if dir == Dir::Row { r.width() } else { r.height() }).unwrap_or(f64::MAX);
    let room = if editor { editor_least(dir) } else { least(dir) };
    let most = (extent - others - room - 4.0).max(least(dir));
    size.set(want.clamp(least(dir), most));
}

/// The parts fitted to their splits after the window is resized: in each split, the parts beside the one that
/// takes the room give it back, the last first, down to their least.
pub fn fit(ui: Ui) {
    fn walk(ui: Ui, n: &Node) {
        let Node::Split { id, dir, kids } = n else { return };
        let flex = kids.iter().position(Node::has_editor).unwrap_or(kids.len().saturating_sub(1));
        let room = if kids[flex].has_editor() { editor_least(*dir) } else { least(*dir) };
        if let Some(r) = rect_of(*id).filter(|r| r.area() > 0.0) {
            let extent = if *dir == Dir::Row { r.width() } else { r.height() };
            let drawn_fixed: Vec<u32> =
                kids.iter().enumerate().filter(|(i, k)| *i != flex && drawn(ui, k)()).map(|(_, k)| k.id()).collect();
            let mut over: f64 = drawn_fixed.iter().map(|k| ui.size(*k).get_untracked()).sum::<f64>() + room - extent;
            for k in drawn_fixed.iter().rev() {
                if over <= 0.0 {
                    break;
                }
                let s = ui.size(*k);
                let now = s.get_untracked();
                let to = (now - over).max(least(*dir));
                over -= now - to;
                s.set(to);
            }
        }
        kids.iter().for_each(|k| walk(ui, k));
    }
    let root = ui.dock.get_untracked().root;
    walk(ui, &root);
}

fn group_view(ui: Ui, send: Send, id: u32, place: Place) -> impl IntoView {
    let t = ui.colors;
    let views = move || {
        ui.dock.with(|d| match d.find(id) {
            Some(Node::Tabs { views, active, .. }) => (views.clone(), Some(*active)),
            _ => (Vec::new(), None),
        })
    };
    let side_bar = ui.dock.with_untracked(|d| d.side_bar()) == Some(id);
    let header = dyn_container(views, move |(views, active)| {
        let panel = matches!(place, Place::Before(Dir::Column) | Place::After(Dir::Column));
        if side_bar || (views.len() == 1 && !panel) {
            // A side bar's title (the activity bar switches its views), as VS Code's: the shown view's name.
            let Some(v) = active else { return empty().into_any() };
            let title = label(move || v.title().to_uppercase())
                .style(move |s| caps(s).padding_left(20.0).height_full().items_center().color(t.get().side_title));
            handle(ui, v, title, move || {}).into_any()
        } else {
            h_stack_from_iter(views.into_iter().map(|v| {
                let on = active == Some(v);
                let tab = v_stack((
                    label(move || v.title().to_uppercase()).style(caps),
                    empty().style(move |s| {
                        let c = t.get();
                        s.height(1.0).width_full().background(if on {
                            c.panel_title_border
                        } else {
                            Color::TRANSPARENT
                        })
                    }),
                ))
                .style(move |s| {
                    let c = t.get();
                    s.gap(5.0)
                        .padding_top(6.0)
                        .margin_horiz(10.0)
                        .cursor(CursorStyle::Pointer)
                        .color(if on { c.panel_title } else { c.panel_title_inactive })
                        .hover(|s| s.color(c.panel_title))
                });
                handle(ui, v, tab, move || ui.reveal(v))
            }))
            .style(|s| s.height_full())
            .into_any()
        }
    });
    let actions = {
        let send = send.clone();
        dyn_container(
            move || views().1,
            move |active| {
                let send = send.clone();
                let close = action(t, "close", move || {
                    ui.hidden.update(|h| {
                        h.insert(id);
                    })
                });
                match active {
                    Some(View::Files) => h_stack((
                        action(t, "refresh", move || {
                            ui.tree.update(|t| t.clear());
                            send(serde_json::json!({"type": "ls", "path": ""}));
                            for p in ui.expanded.get_untracked() {
                                send(serde_json::json!({"type": "ls", "path": p}));
                            }
                        }),
                        action(t, "collapse-all", move || ui.expanded.update(|e| e.clear())),
                    ))
                    .style(|s| s.gap(2.0))
                    .into_any(),
                    _ if !side_bar => close.into_any(),
                    _ => empty().into_any(),
                }
            },
        )
        .style(|s| s.padding_right(8.0))
    };
    let title = h_stack((header.style(|s| s.flex_grow(1.0_f32).height_full()), actions))
        .style(|s| s.height(35.0).width_full().items_center().selectable(false));
    let body = dyn_container(
        move || views().1,
        move |v| match v {
            Some(v) => view_body(ui, send.clone(), v).into_any(),
            None => empty().into_any(),
        },
    )
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
    // Its look by where it is: a panel's under or over the editor, a side bar's beside it, its border on the
    // editor's side.
    let view = v_stack((title, body)).style(move |s| {
        let c = t.get();
        let s = s.size_full();
        match place {
            Place::After(Dir::Column) => line(s, c.panel_border, 't').background(c.panel_bg),
            Place::Before(Dir::Column) => line(s, c.panel_border, 'b').background(c.panel_bg),
            Place::After(Dir::Row) => line(s, c.side_border, 'l').background(c.side_bg).color(c.side_fg),
            _ => line(s, c.side_border, 'r').background(c.side_bg).color(c.side_fg),
        }
    });
    keep(id, view.id());
    view
}

/// A view's own content.
fn view_body(ui: Ui, send: Send, v: View) -> floem::AnyView {
    match v {
        View::Files => crate::explorer::view(ui, send).into_any(),
        View::Agent => crate::agent::view(ui, send).into_any(),
        View::Search => crate::search::view(ui, send).into_any(),
        View::Changes => crate::changes::view(ui, send).into_any(),
        View::Terminal => crate::terminal::view(ui, send).into_any(),
        View::Outline => crate::outline::view(ui, send).into_any(),
    }
}

/// A floating view's window: its title with a button that docks it back, and the view, in the theme; closed, the
/// view docks back beside the editor.
fn open_window(ui: Ui, v: View) {
    let Some(send) = SEND.with(|s| s.borrow().clone()) else { return };
    if WINDOWS.with(|w| w.borrow().contains_key(&v)) {
        return;
    }
    let config =
        floem::window::WindowConfig::default().title(format!("{} — Code Arena", v.title())).size((460.0, 620.0));
    floem::new_window(
        move |wid| {
            WINDOWS.with(|w| w.borrow_mut().insert(v, wid));
            let t = ui.colors;
            let title = h_stack((
                label(move || v.title().to_uppercase())
                    .style(move |s| caps(s).padding_left(20.0).color(t.get().side_title)),
                empty().style(|s| s.flex_grow(1.0_f32)),
                action(t, "layout-sidebar-right", move || ui.dock_view(v, EDITOR, Side::Right)),
            ))
            .style(|s| s.height(35.0).width_full().items_center().padding_right(8.0).selectable(false));
            v_stack((title, view_body(ui, send, v).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0))))
                .on_event_cont(EventListener::WindowClosed, move |_| {
                    // Closed by its frame: back in the tree (unless docked already, which closed it).
                    if WINDOWS.with(|w| w.borrow_mut().remove(&v)).is_some() {
                        ui.dock.update(|d| *d = d.docked(v, EDITOR, Side::Right));
                        ui.reveal(v);
                    }
                })
                .style(move |s| {
                    let c = t.get();
                    s.size_full()
                        .font_family(crate::widgets::UI_FONT.clone())
                        .font_size(13.0)
                        .background(c.side_bg)
                        .color(c.side_fg)
                })
        },
        Some(config),
    );
}

/// What a view is dragged by (its icon, title or tab): a click (no move past 5 px) does `click`; a drag docks it.
pub fn handle<V: IntoView + 'static>(ui: Ui, v: View, view: V, click: impl Fn() + 'static) -> impl IntoView {
    let start = RwSignal::new(None::<Point>);
    let view = view.into_view();
    let id = view.id();
    let at = move |p: Point| {
        let o = id.layout_rect().origin();
        Point::new(o.x + p.x, o.y + p.y)
    };
    view.on_event(EventListener::PointerDown, move |e| {
        if let Event::PointerDown(p) = e
            && p.button.is_primary()
        {
            start.set(Some(at(p.pos)));
            id.request_active();
        }
        EventPropagation::Continue
    })
    .on_event_stop(EventListener::PointerMove, move |e| {
        let (Event::PointerMove(p), Some(from)) = (e, start.get_untracked()) else { return };
        let now = at(p.pos);
        if ui.drag.with_untracked(|d| d.is_none()) {
            if (now - from).hypot() < 5.0 {
                return;
            }
            ui.menu.set(None);
            ui.context.set(None);
        }
        ui.drag.set(Some(aim(ui, v, now)));
    })
    .on_event_stop(EventListener::PointerUp, move |_| {
        let was = start.get_untracked();
        start.set(None);
        match ui.drag.get_untracked() {
            Some(d) => {
                ui.drag.set(None);
                match d.drop {
                    Some((target, side)) => ui.dock_view(d.view, target, side),
                    // Released away from every guide, as in Visual Studio: in a window of its own.
                    None if was.is_some_and(|w| (d.at - w).hypot() > 60.0) => ui.float(d.view),
                    None => {}
                }
            }
            None if was.is_some() => click(),
            None => {}
        }
    })
    .on_secondary_click_stop(move |_| crate::workbench::open_view_menu(ui, v))
}

/// What the pointer is over while a view is dragged: the window's edge guides first, then the part under it and
/// its guide.
fn aim(ui: Ui, v: View, p: Point) -> Drag {
    let mut d = Drag { view: v, at: p, over: None, drop: None, shade: None };
    let Some(area) = rect_of(ROOT) else { return d };
    if let Some(side) = dock::zone(&dock::edge_guides(area), p) {
        d.drop = Some((ROOT, side));
        d.shade = Some(dock::preview(area, side, true));
    }
    let mut parts = vec![EDITOR];
    ui.dock.with_untracked(|dk| dk.root.groups(&mut parts));
    let under = parts
        .into_iter()
        .filter(|g| *g == EDITOR || !ui.hidden.with_untracked(|h| h.contains(g)))
        .filter_map(|g| rect_of(g).filter(|r| r.contains(p) && r.area() > 0.0).map(|r| (g, r)))
        .min_by(|a, b| a.1.area().total_cmp(&b.1.area()));
    if let Some((g, r)) = under {
        d.over = Some((g, r));
        if d.drop.is_none()
            && let Some(side) = dock::zone(&dock::guide(r), p)
            && !(side == Side::Center && g == EDITOR)
        {
            d.drop = Some((g, side));
            d.shade = Some(dock::preview(r, side, false));
        }
    }
    d
}

/// While a view is dragged: the shade where it would go, Visual Studio's guides (on the part under the pointer and
/// at the window's edges), and the view's name under the pointer.
pub fn overlay(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let square = move |side: Side, r: Rect, edge: bool| {
        let name = match side {
            Side::Left => "arrow-left",
            Side::Right => "arrow-right",
            Side::Top => "arrow-up",
            Side::Bottom => "arrow-down",
            Side::Center => "window",
        };
        container(icon(name, 16.0)).style(move |s| {
            let c = t.get();
            let lit = ui
                .drag
                .with(|d| d.as_ref().and_then(|d| d.drop).is_some_and(|(g, sd)| sd == side && (g == ROOT) == edge));
            s.position(Position::Absolute)
                .inset_left(r.x0)
                .inset_top(r.y0)
                .width(r.width())
                .height(r.height())
                .items_center()
                .justify_center()
                .border(1.0)
                .border_radius(4.0)
                .border_color(c.focus)
                .background(if lit { c.focus } else { c.widget })
                .color(if lit { c.button_fg } else { c.focus })
                .box_shadow_blur(6.0)
                .box_shadow_color(c.shadow)
        })
    };
    dyn_container(
        move || ui.drag.with(|d| d.as_ref().map(|d| (d.over.map(|o| o.1), d.shade, d.view))),
        move |state| {
            let Some((over, shade, _)) = state else { return empty().into_any() };
            let mut parts: Vec<floem::AnyView> = Vec::new();
            if let Some(r) = shade {
                parts.push(
                    empty()
                        .style(move |s| {
                            let c = t.get();
                            s.position(Position::Absolute)
                                .inset_left(r.x0)
                                .inset_top(r.y0)
                                .width(r.width())
                                .height(r.height())
                                .background(c.focus.multiply_alpha(0.25))
                                .border(2.0)
                                .border_color(c.focus)
                        })
                        .into_any(),
                );
            }
            if let Some(r) = over {
                for (side, sq) in dock::guide(r) {
                    parts.push(square(side, sq, false).into_any());
                }
            }
            if let Some(area) = rect_of(ROOT) {
                for (side, sq) in dock::edge_guides(area) {
                    parts.push(square(side, sq, true).into_any());
                }
            }
            stack_of(parts).into_any()
        },
    )
    .style(move |s| {
        s.position(Position::Absolute).inset(0.0).z_index(300).apply_if(ui.drag.with(|d| d.is_none()), |s| s.hide())
    })
}

fn stack_of(parts: Vec<floem::AnyView>) -> impl IntoView {
    floem::views::stack_from_iter(parts).style(|s| s.position(Position::Absolute).inset(0.0))
}

/// The dragged view's name under the pointer.
pub fn ghost(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    label(move || ui.drag.with(|d| d.as_ref().map(|d| d.view.title().to_string()).unwrap_or_default())).style(
        move |s| {
            let c = t.get();
            let at = ui.drag.with(|d| d.as_ref().map(|d| d.at)).unwrap_or_default();
            s.position(Position::Absolute)
                .inset_left(at.x + 12.0)
                .inset_top(at.y + 12.0)
                .padding_horiz(8.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .background(c.list_selected)
                .color(c.list_selected_fg)
                .font_size(12.0)
                .z_index(310)
                .apply_if(ui.drag.with(|d| d.is_none()), |s| s.hide())
        },
    )
}
