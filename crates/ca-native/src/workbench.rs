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

use crate::icons::icon;
use crate::layout::{View, Where};
use crate::state::{Send, Ui};
use crate::widgets::{UI_FONT, action, action_lit, caps, line, sash};
use crate::{agent, changes, editor, explorer, search};

const MENUS: [&str; 8] = ["File", "Edit", "Selection", "View", "Go", "Run", "Terminal", "Help"];

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let keys = send.clone();
    let body = h_stack((
        activity_bar(ui),
        container_view(ui, send.clone(), Where::Primary),
        sash(t, true, move |d| ui.primary_width.update(|w| *w = (*w + d).clamp(170.0, 800.0)))
            .style(move |s| s.apply_if(!ui.layout.with(|l| l.shown(Where::Primary)), |s| s.hide())),
        v_stack((
            editor::view(ui),
            sash(t, false, move |d| ui.panel_height.update(|h| *h = (*h - d).clamp(100.0, 900.0)))
                .style(move |s| s.apply_if(!ui.layout.with(|l| l.shown(Where::Panel)), |s| s.hide())),
            container_view(ui, send.clone(), Where::Panel),
        ))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).height_full()),
        sash(t, true, move |d| ui.secondary_width.update(|w| *w = (*w - d).clamp(220.0, 900.0)))
            .style(move |s| s.apply_if(!ui.layout.with(|l| l.shown(Where::Secondary)), |s| s.hide())),
        container_view(ui, send.clone(), Where::Secondary),
    ))
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
    stack((
        v_stack((title_bar(ui), body, status_bar(ui))).style(|s| s.size_full()),
        menu_overlay(ui, send.clone()),
        view_menu(ui),
        resize_edges(),
    ))
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
            .font_family(UI_FONT.to_string())
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

/// VS Code's keys for the workbench.
fn key(ui: Ui, send: &Send, key: &Key, m: Modifiers) {
    let ctrl = m.contains(Modifiers::CONTROL);
    let shift = m.contains(Modifiers::SHIFT);
    let alt = m.contains(Modifiers::ALT);
    let toggle = |w: Where| ui.layout.update(|l| *l = l.toggled(w));
    let view = |v: View| ui.layout.update(|l| *l = l.revealed(v));
    match key {
        Key::Character(c) if ctrl => match (c.to_lowercase().as_str(), shift, alt) {
            ("b", false, false) => toggle(Where::Primary),
            ("b", false, true) => toggle(Where::Secondary),
            ("j", false, false) => toggle(Where::Panel),
            ("`", false, false) => ui.layout.update(|l| *l = l.view_toggled(View::Terminal)),
            ("e", true, false) => view(View::Files),
            ("f", true, false) => view(View::Search),
            ("i", false, true) => view(View::Agent),
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
        }
        _ => {}
    }
}

fn title_bar(ui: Ui) -> impl IntoView {
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
    let layout_buttons = h_stack((
        action_lit(
            t,
            move || {
                if ui.layout.with(|l| l.shown(Where::Primary)) {
                    "layout-sidebar-left"
                } else {
                    "layout-sidebar-left-off"
                }
            },
            move || ui.layout.update(|l| *l = l.toggled(Where::Primary)),
        ),
        action_lit(
            t,
            move || if ui.layout.with(|l| l.shown(Where::Panel)) { "layout-panel" } else { "layout-panel-off" },
            move || ui.layout.update(|l| *l = l.toggled(Where::Panel)),
        ),
        action_lit(
            t,
            move || {
                if ui.layout.with(|l| l.shown(Where::Secondary)) {
                    "layout-sidebar-right"
                } else {
                    "layout-sidebar-right-off"
                }
            },
            move || ui.layout.update(|l| *l = l.toggled(Where::Secondary)),
        ),
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
            ("Explorer", "Ctrl+Shift+E", Some(|ui, _| ui.layout.update(|l| *l = l.revealed(View::Files)))),
            ("Search", "Ctrl+Shift+F", Some(|ui, _| ui.layout.update(|l| *l = l.revealed(View::Search)))),
            ("Chat", "Ctrl+Alt+I", Some(|ui, _| ui.layout.update(|l| *l = l.revealed(View::Agent)))),
            ("Terminal", "Ctrl+`", Some(|ui, _| ui.layout.update(|l| *l = l.revealed(View::Terminal)))),
            ("-", "", None),
            ("Primary Side Bar", "Ctrl+B", Some(|ui, _| ui.layout.update(|l| *l = l.toggled(Where::Primary)))),
            ("Secondary Side Bar", "Ctrl+Alt+B", Some(|ui, _| ui.layout.update(|l| *l = l.toggled(Where::Secondary)))),
            ("Panel", "Ctrl+J", Some(|ui, _| ui.layout.update(|l| *l = l.toggled(Where::Panel)))),
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
fn open_view_menu(ui: Ui, v: View, _: &Event) {
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
            let here = ui.layout.with_untracked(|l| l.where_is(v));
            let mut items: Vec<(String, Box<dyn Fn()>)> = Vec::new();
            for (to, name) in [
                (Where::Primary, "Primary Side Bar"),
                (Where::Secondary, "Secondary Side Bar"),
                (Where::Panel, "Panel"),
            ] {
                if here != Some(to) {
                    items.push((
                        format!("Move to {name}"),
                        Box::new(move || ui.layout.update(|l| *l = l.moved(v, to, None))),
                    ));
                }
            }
            if let Some(w) = here {
                items.push((format!("Hide {}", v.title()), Box::new(move || ui.layout.update(|l| *l = l.toggled(w)))));
            }
            items.push((
                "Reset View Locations".into(),
                Box::new(move || ui.layout.set(crate::layout::Layout::default())),
            ));
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

fn activity_bar(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let item = move |v: View| {
        let lit = move || ui.layout.with(|l| l.shown(Where::Primary) && l.active(Where::Primary) == Some(v));
        let button = h_stack((
            empty().style(move |s| {
                let c = t.get();
                s.width(2.0).height_full().background(if lit() { c.activity_active_border } else { Color::TRANSPARENT })
            }),
            container(icon(v.icon(), 24.0))
                .style(|s| s.flex_grow(1.0_f32).height_full().items_center().justify_center()),
            empty().style(|s| s.width(2.0)),
        ))
        .on_click_stop(move |_| ui.layout.update(|l| *l = l.view_toggled(v)))
        .on_secondary_click_stop(move |e| open_view_menu(ui, v, e))
        .style(move |s| {
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
    v_stack((
        dyn_stack(move || ui.layout.with(|l| l.of(Where::Primary).to_vec()), |v: &View| *v, item)
            .style(|s| s.flex_col()),
        empty().style(|s| s.flex_grow(1.0_f32)),
        bottom("account"),
        bottom("settings-gear"),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.activity_border, 'r').width(48.0).height_full().background(c.activity_bg)
    })
}

/// A container (a side bar or the panel): its title (the view's, or the panel's tabs) and the view it shows.
fn container_view(ui: Ui, send: Send, w: Where) -> impl IntoView {
    let t = ui.colors;
    let shows = move || ui.layout.with(|l| if l.shown(w) { l.active(w) } else { None });
    let send2 = send.clone();
    let header = dyn_container(
        move || (ui.layout.with(|l| l.of(w).to_vec()), shows()),
        move |(views, active)| {
            if w == Where::Panel || views.len() > 1 && w == Where::Secondary {
                // The panel's tabs: its views' titles in capitals, the shown one underlined.
                h_stack((dyn_stack(
                    move || views.clone(),
                    |v: &View| *v,
                    move |v: View| {
                        let on = active == Some(v);
                        v_stack((
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
                        .on_click_stop(move |_| ui.layout.update(|l| *l = l.revealed(v)))
                        .on_secondary_click_stop(move |e| open_view_menu(ui, v, e))
                        .style(move |s| {
                            let c = t.get();
                            s.gap(5.0)
                                .padding_top(6.0)
                                .margin_horiz(10.0)
                                .cursor(CursorStyle::Pointer)
                                .color(if on { c.panel_title } else { c.panel_title_inactive })
                                .hover(|s| s.color(c.panel_title))
                        })
                    },
                )
                .style(|s| s.flex_row()),))
                .into_any()
            } else {
                label(move || active.map(|v| v.title().to_uppercase()).unwrap_or_default())
                    .on_secondary_click_stop(move |e| {
                        if let Some(v) = active {
                            open_view_menu(ui, v, e)
                        }
                    })
                    .style(move |s| caps(s).padding_left(20.0).color(t.get().side_title))
                    .into_any()
            }
        },
    );
    let title = h_stack((
        header.style(|s| s.flex_grow(1.0_f32)),
        dyn_container(shows, move |v| match v {
            Some(View::Files) => {
                let send = send2.clone();
                h_stack((
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
                .into_any()
            }
            Some(_) if w == Where::Panel => {
                action(t, "close", move || ui.layout.update(|l| *l = l.toggled(Where::Panel))).into_any()
            }
            _ => empty().into_any(),
        })
        .style(|s| s.padding_right(8.0)),
    ))
    .style(move |s| s.height(35.0).width_full().items_center().selectable(false));
    let body = dyn_container(shows, move |v| match v {
        Some(View::Files) => explorer::view(ui, send.clone()).into_any(),
        Some(View::Agent) => agent::view(ui, send.clone()).into_any(),
        Some(View::Search) => search::view(ui, send.clone()).into_any(),
        Some(View::Changes) => changes::view(ui, send.clone()).into_any(),
        Some(v) => not_yet(ui, v).into_any(),
        None => empty().into_any(),
    })
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
    v_stack((title, body)).style(move |s| {
        let c = t.get();
        let shown = ui.layout.with(|l| l.shown(w));
        let s = s.apply_if(!shown, |s| s.hide());
        match w {
            Where::Primary => line(s, c.side_border, 'r')
                .width(ui.primary_width.get())
                .height_full()
                .background(c.side_bg)
                .color(c.side_fg),
            Where::Secondary => line(s, c.side_border, 'l')
                .width(ui.secondary_width.get())
                .height_full()
                .background(c.side_bg)
                .color(c.side_fg),
            Where::Panel => {
                line(s, c.panel_border, 't').height(ui.panel_height.get()).width_full().background(c.panel_bg)
            }
        }
    })
}

/// A view not drawn natively yet: said so, and where it is meanwhile.
fn not_yet(ui: Ui, v: View) -> impl IntoView {
    let t = ui.colors;
    label(move || {
        format!("{} comes to the native window in a later step; `code-arena ide --browser` has it now.", v.title())
    })
    .style(move |s| {
        s.padding_horiz(20.0).padding_vert(8.0).color(t.get().desc).text_overflow(floem::style::TextOverflow::Wrap)
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
