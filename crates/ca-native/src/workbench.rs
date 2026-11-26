//! VS Code's workbench, region by region at its sizes: the 35 px title bar (the menu, the command center, the layout
//! buttons, the window's buttons), the 48 px activity bar, the side bars with their 35 px titles, the editor group,
//! the panel with its tabs, and the 22 px status bar. Each part is resized by its sash.

use floem::IntoView;
use floem::action::{minimize_window, toggle_window_maximized};
use floem::event::{Event, EventListener};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, Position};
use floem::views::{
    Decorators, container, drag_resize_window_area, drag_window_area, dyn_container, dyn_stack, empty, h_stack, label,
    stack, tooltip, v_stack,
};
use floem::window::ResizeDirection;

use crate::dock::{EDITOR, Node, Side};
use crate::icons::icon;
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::widgets::{UI_FONT, action_lit, line};

const MENUS: [&str; 8] = ["File", "Edit", "Selection", "View", "Go", "Run", "Terminal", "Help"];

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let keys = send.clone();
    let body = h_stack((activity_bar(ui), crate::docking::tree(ui, send.clone())))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
    stack((
        v_stack((title_bar(ui, send.clone()), body, status_bar(ui))).style(|s| s.size_full()),
        menu_overlay(ui, send.clone()),
        view_menu(ui),
        crate::docking::overlay(ui),
        crate::docking::ghost(ui),
        crate::palette::view(ui, send.clone()),
        resize_edges(),
    ))
    .on_resize(move |r| {
        ui.window.set((r.width(), r.height()));
        // Once the parts are laid out at the window's new size, they are fitted into it.
        floem::action::exec_after(std::time::Duration::ZERO, move |_| crate::docking::fit(ui));
    })
    .on_event_cont(EventListener::PointerMove, move |e| {
        if let Event::PointerMove(p) = e {
            ui.pointer.set((p.pos.x, p.pos.y));
        }
    })
    .on_event_cont(EventListener::KeyDown, move |e| {
        if let Event::KeyDown(k) = e {
            key(ui, &keys, &k.key.logical_key, k.modifiers);
        }
    })
    .style(move |s| {
        let c = t.get();
        s.size_full()
            .font_family(UI_FONT.clone())
            .font_size(13.0)
            .color(c.fg)
            .background(c.editor_bg)
            // VS Code's hovers and placeholders, in place of Floem's own light ones.
            .class(floem::views::TooltipClass, |s| {
                s.border(1.0)
                    .border_color(c.widget_border)
                    .border_radius(4.0)
                    .padding_horiz(8.0)
                    .padding_vert(4.0)
                    .margin(6.0)
                    .background(c.widget)
                    .color(c.fg)
                    .font_size(12.0)
                    .box_shadow_blur(8.0)
                    .box_shadow_h_offset(0.0)
                    .box_shadow_v_offset(2.0)
                    .box_shadow_color(c.shadow)
            })
            .class(floem::views::PlaceholderTextClass, |s| s.color(c.placeholder).font_size(13.0))
    })
}

/// The palette's commands that act on the layout, by VS Code's ids.
pub fn run_command(ui: Ui, _: &Send, id: &str) {
    use crate::palette as p;
    match id {
        p::TOGGLE_PRIMARY => ui.toggle_group_of(View::Files),
        p::TOGGLE_SECONDARY => ui.toggle_group_of(View::Agent),
        p::TOGGLE_PANEL => ui.toggle_group_of(View::Terminal),
        p::FOCUS_EXPLORER => ui.reveal(View::Files),
        p::FOCUS_SEARCH => ui.reveal(View::Search),
        p::FOCUS_CHANGES => ui.reveal(View::Changes),
        p::FOCUS_TERMINAL => ui.reveal(View::Terminal),
        p::FOCUS_AGENT => ui.reveal(View::Agent),
        _ => {}
    }
}

thread_local! {
    /// Ctrl+K pressed: the next key ends VS Code's chord (Ctrl+K Ctrl+T: the theme pick).
    static CHORD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// VS Code's keys for the workbench.
pub(crate) fn key(ui: Ui, send: &Send, key: &Key, m: Modifiers) {
    let ctrl = m.contains(Modifiers::CONTROL);
    let shift = m.contains(Modifiers::SHIFT);
    let alt = m.contains(Modifiers::ALT);
    let chord = CHORD.with(|c| c.replace(false));
    if let (true, Key::Character(c)) = (chord, key)
        && ctrl
        && c.eq_ignore_ascii_case("t")
    {
        crate::palette::open(ui, send, crate::palette::Mode::Themes);
        return;
    }
    match key {
        Key::Character(c) if ctrl => match (c.to_lowercase().as_str(), shift, alt) {
            ("k", false, false) => CHORD.with(|c| c.set(true)),
            ("f", false, false) => crate::editor::find_show(ui),
            ("p", false, false) => crate::palette::open(ui, send, crate::palette::Mode::Files),
            ("p", true, false) => crate::palette::open(ui, send, crate::palette::Mode::Commands),
            ("b", false, false) => ui.toggle_group_of(View::Files),
            ("b", false, true) => ui.toggle_group_of(View::Agent),
            ("j", false, false) => ui.toggle_group_of(View::Terminal),
            ("`", false, false) => ui.toggle_view(View::Terminal),
            ("e", true, false) => ui.reveal(View::Files),
            ("f", true, false) => ui.reveal(View::Search),
            ("i", false, true) => ui.reveal(View::Agent),
            ("s", false, false) => ui.save(send),
            ("w", false, false) => {
                if let Some(p) = ui.active.get_untracked() {
                    ui.close(&p);
                }
            }
            _ => {}
        },
        Key::Named(NamedKey::Escape) => {
            ui.menu.set(None);
            ui.context.set(None);
            // A drag given up: the view stays where it was.
            ui.drag.set(None);
        }
        _ => {}
    }
}

fn title_bar(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let menus = h_stack((dyn_stack(
        || MENUS.into_iter().enumerate().collect::<Vec<_>>(),
        |(n, _)| *n,
        move |(n, name)| {
            label(move || name)
                .on_click_stop(move |_| ui.menu.update(|m| *m = if *m == Some(n) { None } else { Some(n) }))
                .on_event_cont(EventListener::PointerEnter, move |_| {
                    if ui.menu.get_untracked().is_some() {
                        ui.menu.set(Some(n));
                    }
                })
                .style(move |s| {
                    let c = t.get();
                    let open = ui.menu.get() == Some(n);
                    s.padding_horiz(8.0)
                        .height(22.0)
                        .items_center()
                        .border_radius(4.0)
                        .font_size(13.0)
                        .selectable(false)
                        .hover(|s| s.background(c.toolbar_hover))
                        .apply_if(open, |s| s.background(c.toolbar_hover))
                })
        },
    )
    .style(|s| s.flex_row().items_center()),));
    let command_center = h_stack((
        icon("search", 16.0),
        label(move || ui.folder.get().rsplit('/').next().unwrap_or("").to_string()).style(|s| s.font_size(12.0)),
    ))
    // VS Code's command center: a click is Quick Open.
    .on_click_stop(move |_| crate::palette::open(ui, &send, crate::palette::Mode::Files))
    .style(move |s| {
        let c = t.get();
        s.gap(6.0)
            .items_center()
            .justify_center()
            .height(22.0)
            .width_pct(38.0)
            .max_width(600.0)
            .min_width(160.0)
            .border(1.0)
            .border_color(c.command_border)
            .border_radius(6.0)
            .background(c.command_bg)
            .color(c.command_fg)
            .selectable(false)
            .hover(|s| s.background(c.command_hover))
    });
    // VS Code's layout buttons: the groups holding the Explorer, the Terminal and the Agent shown or hidden.
    let toggle = move |on: &'static str, off: &'static str, v: View| {
        action_lit(t, move || if ui.group_shown(v) { on } else { off }, move || ui.toggle_group_of(v))
    };
    let layout_buttons = h_stack((
        toggle("layout-sidebar-left", "layout-sidebar-left-off", View::Files),
        toggle("layout-panel", "layout-panel-off", View::Terminal),
        toggle("layout-sidebar-right", "layout-sidebar-right-off", View::Agent),
    ))
    .style(|s| s.gap(2.0).items_center().padding_right(8.0));
    let window = |name: &'static str, close: bool, on: fn()| {
        container(icon(name, 16.0)).on_click_stop(move |_| on()).style(move |s| {
            let c = t.get();
            s.width(46.0).height_full().items_center().justify_center().color(c.title_fg).hover(move |s| {
                if close {
                    s.background(Color::rgb8(0xE8, 0x11, 0x23)).color(Color::WHITE)
                } else {
                    s.background(c.toolbar_hover)
                }
            })
        })
    };
    h_stack((
        container(icon("sparkle", 16.0))
            .style(move |s| s.width(35.0).height_full().items_center().justify_center().color(t.get().link)),
        menus,
        drag_window_area(empty()).style(|s| s.flex_grow(1.0_f32).height_full()),
        command_center,
        drag_window_area(empty()).style(|s| s.flex_grow(1.0_f32).height_full()),
        layout_buttons,
        window("chrome-minimize", false, minimize_window),
        window("chrome-maximize", false, toggle_window_maximized),
        window("chrome-close", true, floem::quit_app),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.title_border, 'b').height(35.0).width_full().items_center().background(c.title_bg).color(c.title_fg)
    })
}

/// A menu's items: its label, its keys, what it does (none: drawn dimmed, not there yet).
type MenuItem = (&'static str, &'static str, Option<fn(Ui, &Send)>);

fn menu_items(n: usize) -> Vec<MenuItem> {
    match n {
        0 => vec![
            ("Save", "Ctrl+S", Some(|ui, send| ui.save(send))),
            ("-", "", None),
            (
                "Close Editor",
                "Ctrl+W",
                Some(|ui, _| {
                    if let Some(p) = ui.active.get_untracked() {
                        ui.close(&p)
                    }
                }),
            ),
            ("-", "", None),
            ("Exit", "", Some(|_, _| floem::quit_app())),
        ],
        1 => vec![("Undo", "Ctrl+Z", None), ("Redo", "Ctrl+Y", None), ("-", "", None), ("Find", "Ctrl+F", None)],
        2 => vec![("Select All", "Ctrl+A", None)],
        3 => vec![
            ("Explorer", "Ctrl+Shift+E", Some(|ui, _| ui.reveal(View::Files))),
            ("Search", "Ctrl+Shift+F", Some(|ui, _| ui.reveal(View::Search))),
            ("Changes", "", Some(|ui, _| ui.reveal(View::Changes))),
            ("Outline", "", Some(|ui, _| ui.reveal(View::Outline))),
            ("Chat", "Ctrl+Alt+I", Some(|ui, _| ui.reveal(View::Agent))),
            ("Terminal", "Ctrl+`", Some(|ui, _| ui.reveal(View::Terminal))),
            ("-", "", None),
            ("Primary Side Bar", "Ctrl+B", Some(|ui, _| ui.toggle_group_of(View::Files))),
            ("Secondary Side Bar", "Ctrl+Alt+B", Some(|ui, _| ui.toggle_group_of(View::Agent))),
            ("Panel", "Ctrl+J", Some(|ui, _| ui.toggle_group_of(View::Terminal))),
            ("-", "", None),
            ("Reset View Locations", "", Some(|ui, _| ui.reset_dock())),
        ],
        4 => vec![("Go to File...", "Ctrl+P", None)],
        5 => vec![("Start Debugging", "F5", None)],
        6 => vec![("New Terminal", "Ctrl+Shift+`", None)],
        _ => vec![("About", "", Some(|ui, _| ui.message.set(format!("Code Arena {}", env!("CARGO_PKG_VERSION")))))],
    }
}

/// The open menu, under its title, and a layer behind it that closes it on a click elsewhere.
fn menu_overlay(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    dyn_container(
        move || ui.menu.get(),
        move |open| {
            let Some(n) = open else { return empty().into_any() };
            let send = send.clone();
            // Where the menu's title is: the icon (35) and the titles before it, each its text and 16 of padding.
            let left = 35.0 + MENUS[..n].iter().map(|m| m.len() as f64 * 7.4 + 16.0).sum::<f64>();
            let items = dyn_stack(
                move || menu_items(n).into_iter().enumerate().collect::<Vec<_>>(),
                |(i, _)| *i,
                move |(_, (text, keys, run)): (usize, MenuItem)| {
                    let send = send.clone();
                    if text == "-" {
                        return empty()
                            .style(move |s| {
                                s.height(1.0).margin_vert(4.0).margin_horiz(0.0).background(t.get().menu_separator)
                            })
                            .into_any();
                    }
                    h_stack((
                        label(move || text).style(|s| s.flex_grow(1.0_f32)),
                        label(move || keys).style(|s| s.padding_left(30.0)),
                    ))
                    .on_click_stop(move |_| {
                        if let Some(run) = run {
                            ui.menu.set(None);
                            run(ui, &send);
                        }
                    })
                    .style(move |s| {
                        let c = t.get();
                        let s = s
                            .height(26.0)
                            .items_center()
                            .padding_left(26.0)
                            .padding_right(20.0)
                            .border_radius(4.0)
                            .margin_horiz(4.0);
                        if run.is_some() {
                            s.color(c.menu_fg).hover(|s| s.background(c.menu_selected).color(c.menu_selected_fg))
                        } else {
                            s.color(c.menu_fg.multiply_alpha(0.4))
                        }
                    })
                    .into_any()
                },
            )
            .style(move |s| {
                let c = t.get();
                s.flex_col()
                    .position(Position::Absolute)
                    .inset_left(left)
                    .inset_top(31.0)
                    .min_width(220.0)
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
            stack((empty().on_click_stop(move |_| ui.menu.set(None)).style(|s| s.size_full()), items))
                .style(|s| s.position(Position::Absolute).inset(0.0).size_full())
                .into_any()
        },
    )
    .style(move |s| {
        s.position(Position::Absolute).inset(0.0).apply_if(ui.menu.with(|m| m.is_none()), |s| s.hide()).z_index(100)
    })
}

/// A view's menu opened where it was right-clicked (the event's point is the clicked view's own, so the place is
/// the pointer's as the window's root last saw it).
pub fn open_view_menu(ui: Ui, v: View) {
    let (x, y) = ui.pointer.get_untracked();
    ui.context.set(Some((v, x, y)));
    ui.menu.set(None);
}

type ViewMenuItem = (usize, String, std::rc::Rc<dyn Fn()>);

/// A view's own menu, as VS Code's: move it to another container, hide it, put every view back.
fn view_menu(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    dyn_container(
        move || ui.context.get(),
        move |open| {
            let Some((v, x, y)) = open else { return empty().into_any() };
            // Visual Studio's Dock: beside the editor on any side, or into the tabs of another group.
            let mut items: Vec<(String, Box<dyn Fn()>)> = Vec::new();
            for (side, name) in [
                (Side::Left, "Dock Left of the Editor"),
                (Side::Right, "Dock Right of the Editor"),
                (Side::Bottom, "Dock Below the Editor"),
                (Side::Top, "Dock Above the Editor"),
            ] {
                items.push((name.into(), Box::new(move || ui.dock_view(v, EDITOR, side))));
            }
            let mine = ui.dock.with_untracked(|d| d.group_of(v));
            for other in [View::Files, View::Agent, View::Terminal] {
                let to = ui.dock.with_untracked(|d| d.group_of(other));
                if let Some(g) = to.filter(|g| Some(*g) != mine) {
                    items.push((
                        format!("Dock with {}", other.title()),
                        Box::new(move || ui.dock_view(v, g, Side::Center)),
                    ));
                }
            }
            items.push(("Float".into(), Box::new(move || ui.float(v))));
            items.push((format!("Hide {}", v.title()), Box::new(move || ui.toggle_group_of(v))));
            items.push(("Reset View Locations".into(), Box::new(move || ui.reset_dock())));
            let items: Vec<ViewMenuItem> =
                items.into_iter().enumerate().map(|(i, (s, f))| (i, s, std::rc::Rc::from(f))).collect();
            let list = dyn_stack(
                move || items.clone(),
                |(i, _, _)| *i,
                move |(_, text, run)| {
                    label(move || text.clone())
                        .on_click_stop(move |_| {
                            ui.context.set(None);
                            run();
                        })
                        .style(move |s| {
                            let c = t.get();
                            s.height(26.0)
                                .items_center()
                                .padding_left(26.0)
                                .padding_right(26.0)
                                .border_radius(4.0)
                                .margin_horiz(4.0)
                                .color(c.menu_fg)
                                .hover(|s| s.background(c.menu_selected).color(c.menu_selected_fg))
                        })
                },
            )
            .style(move |s| {
                let c = t.get();
                s.flex_col()
                    .position(Position::Absolute)
                    .inset_left(x)
                    .inset_top(y)
                    .min_width(220.0)
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
            stack((empty().on_click_stop(move |_| ui.context.set(None)).style(|s| s.size_full()), list))
                .style(|s| s.position(Position::Absolute).inset(0.0).size_full())
                .into_any()
        },
    )
    .style(move |s| {
        s.position(Position::Absolute).inset(0.0).apply_if(ui.context.with(|m| m.is_none()), |s| s.hide()).z_index(100)
    })
}

/// The window's edges: it has no frame of the system's, so it is resized by these (each a strip of its own, so the
/// rest of the window gets its clicks).
fn resize_edges() -> impl IntoView {
    let edge = |d: ResizeDirection, f: fn(floem::style::Style) -> floem::style::Style| {
        drag_resize_window_area(d, empty()).style(move |s| f(s.position(Position::Absolute).z_index(200)))
    };
    (
        edge(ResizeDirection::North, |s| s.inset_top(0.0).inset_left(0.0).width_full().height(3.0)),
        edge(ResizeDirection::South, |s| s.inset_bottom(0.0).inset_left(0.0).width_full().height(3.0)),
        edge(ResizeDirection::West, |s| s.inset_left(0.0).inset_top(0.0).height_full().width(3.0)),
        edge(ResizeDirection::East, |s| s.inset_right(0.0).inset_top(0.0).height_full().width(3.0)),
        edge(ResizeDirection::SouthEast, |s| s.inset_right(0.0).inset_bottom(0.0).width(8.0).height(8.0)),
    )
}

/// The activity bar: the side bar's views (all of them when no group is at the window's left), each dragged to
/// dock it elsewhere.
fn activity_bar(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let item = move |v: View| {
        let lit = move || ui.view_shown(v);
        let button = h_stack((
            empty().style(move |s| {
                let c = t.get();
                s.width(2.0).height_full().background(if lit() { c.activity_active_border } else { Color::TRANSPARENT })
            }),
            container(icon(v.icon(), 24.0))
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
        // VS Code's hover: the view's name and its keys.
        tooltip(button, move || {
            let keys = v.keys();
            label(move || if keys.is_empty() { v.title().to_string() } else { format!("{} ({keys})", v.title()) })
        })
    };
    let bottom = move |name: &'static str| {
        container(icon(name, 24.0)).style(move |s| {
            let c = t.get();
            s.width(48.0)
                .height(48.0)
                .items_center()
                .justify_center()
                .color(c.activity_inactive)
                .hover(|s| s.color(c.activity_fg))
        })
    };
    let listed = move || {
        ui.dock.with(|d| match d.side_bar().and_then(|g| d.find(g)) {
            Some(Node::Tabs { views, .. }) => views.clone(),
            _ => crate::layout::VIEWS.to_vec(),
        })
    };
    v_stack((
        dyn_stack(listed, |v: &View| *v, item).style(|s| s.flex_col()),
        empty().style(|s| s.flex_grow(1.0_f32)),
        bottom("account"),
        bottom("settings-gear"),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.activity_border, 'r').width(48.0).height_full().background(c.activity_bg)
    })
}

fn status_bar(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let item = move |s: floem::style::Style| {
        let c = t.get();
        s.height_full()
            .items_center()
            .gap(4.0)
            .padding_horiz(5.0)
            .margin_horiz(2.0)
            .hover(|s| s.background(c.status_hover))
    };
    h_stack((
        container(icon("remote", 16.0)).style(move |s| {
            let c = t.get();
            s.height_full().padding_horiz(10.0).items_center().background(c.remote_bg).color(c.remote_fg)
        }),
        h_stack((icon("git-branch", 16.0), label(move || ui.branch.get())))
            .style(move |s| item(s).apply_if(ui.branch.with(|b| b.is_empty()), |s| s.hide())),
        h_stack((icon("error", 16.0), label(|| "0"), icon("warning", 16.0), label(|| "0"))).style(item),
        label(move || ui.message.get())
            .style(move |s| item(s).apply_if(ui.message.with(|m| m.is_empty()), |s| s.hide())),
        empty().style(|s| s.flex_grow(1.0_f32)),
        label(move || {
            let (l, c) = ui.caret.get();
            format!("Ln {l}, Col {c}")
        })
        .style(move |s| item(s).apply_if(ui.active.with(|a| a.is_none()), |s| s.hide())),
        label(|| "UTF-8").style(move |s| item(s).apply_if(ui.active.with(|a| a.is_none()), |s| s.hide())),
        label(move || ui.active.get().and_then(|p| ui.tab(&p)).map(|t| t.colours.language()).unwrap_or_default())
            .style(move |s| item(s).apply_if(ui.active.with(|a| a.is_none()), |s| s.hide())),
        h_stack((
            dyn_container(move || ui.running.get(), |r| icon(if r { "loading" } else { "sparkle" }, 16.0)),
            label(move || ui.model.get()),
        ))
        .style(item),
        container(icon("bell", 16.0)).style(item),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.status_border, 't')
            .height(22.0)
            .width_full()
            .items_center()
            .font_size(12.0)
            .background(c.status_bg)
            .color(c.status_fg)
            .selectable(false)
    })
}
