//! VS Code's workbench, region by region at its sizes: the 35 px title bar (the menu, the command center, the layout
//! buttons, the window's buttons), the 48 px activity bar, the side bars with their 35 px titles, the editor group,
//! the panel with its tabs, and the 22 px status bar. Each part is resized by its sash.

use floem::IntoView;
use floem::action::{minimize_window, toggle_window_maximized};
use floem::event::{Event, EventListener};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::{
    Decorators, container, drag_resize_window_area, drag_window_area, dyn_container, dyn_stack, empty, h_stack,
    h_stack_from_iter, label, stack, stack_from_iter, v_stack_from_iter,
};
use floem::window::ResizeDirection;

use crate::icons::icon;
use crate::layout::View;
use crate::menus::Row;
use crate::state::{Send, Ui};
use crate::widgets::{UI_FONT, action_lit, line};

const MENUS: [&str; 8] = ["File", "Edit", "Selection", "View", "Go", "Run", "Terminal", "Help"];

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let keys = send.clone();
    // The activity bar, the title bar and the status bar are hidden in Zen mode and full screen (`layout_commands`).
    let bars = move |s: floem::style::Style| s.apply_if(!crate::layout_commands::bars_shown(ui), |s| s.hide());
    let activity = container(crate::activity_bar::view(ui, send.clone())).style(move |s| bars(s.height_full()));
    let body = h_stack((activity, crate::docking::tree(ui, send.clone())))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
    // The title bar, the parts' banners under it (`layers.rs`; none: no row), the views, the status bar.
    let title = container(title_bar(ui, send.clone()))
        .style(move |s| s.width_full().apply_if(!crate::layout_commands::title_bar_shown(ui), |s| s.hide()));
    let mut rows = vec![title.into_any()];
    rows.extend(crate::layers::views(ui, &send, crate::layers::Place::Banner));
    rows.push(body.into_any());
    rows.push(container(status_bar(ui, send.clone())).style(move |s| bars(s.width_full())).into_any());
    // Over them: the window's own layers, then the parts', then the strips the window is resized by.
    let mut all = vec![
        v_stack_from_iter(rows).style(|s| s.size_full()).into_any(),
        menu_overlay(ui, send.clone()).into_any(),
        crate::docking::overlay(ui).into_any(),
        crate::docking::ghost(ui).into_any(),
        crate::palette::view(ui, send.clone()).into_any(),
    ];
    all.extend(crate::layers::views(ui, &send, crate::layers::Place::Over));
    all.push(resize_edges().into_any());
    stack_from_iter(all)
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

/// The ids of the workbench's own commands that other parts name (VS Code's, and the old Code Arena's where only it
/// has the command).
pub const SAVE: &str = "workbench.action.files.save";
pub const CLOSE_EDITOR: &str = "workbench.action.closeActiveEditor";
pub const QUICK_OPEN: &str = "workbench.action.quickOpen";
pub const SHOW_COMMANDS: &str = "workbench.action.showCommands";
pub const TOGGLE_PRIMARY: &str = "workbench.action.toggleSidebarVisibility";
pub const TOGGLE_SECONDARY: &str = "workbench.action.toggleAuxiliaryBar";
pub const TOGGLE_PANEL: &str = "workbench.action.togglePanel";
pub const SHOW_EXPLORER: &str = "workbench.view.explorer";
pub const SHOW_SEARCH: &str = "workbench.view.search";
pub const SHOW_CHANGES: &str = "codeArena.view.changes";
pub const SHOW_OUTLINE: &str = "outline.focus";
pub const SHOW_AGENT: &str = "workbench.action.chat.open";
pub const FOCUS_TERMINAL: &str = "workbench.action.terminal.focus";
pub const TOGGLE_TERMINAL: &str = "workbench.action.terminal.toggleTerminal";
pub const NEW_TERMINAL: &str = "workbench.action.terminal.new";
pub const KILL_TERMINAL: &str = "workbench.action.terminal.kill";
pub const CLEAR_TERMINAL: &str = "workbench.action.terminal.clear";
pub const SELECT_THEME: &str = "workbench.action.selectTheme";
pub const ABOUT: &str = "workbench.action.showAboutDialog";
pub const RESET_VIEWS: &str = "workbench.action.resetViewLocations";
pub const FIND: &str = "actions.find";
pub const QUIT: &str = "workbench.action.quit";

/// The command that puts a view in a window of its own.
pub fn float_command(v: View) -> String {
    format!("codeArena.view.float.{}", v.id())
}

/// The workbench's own commands, menus and status bar items: what the window began with, in the order the palette
/// lists them. Each view's "Show" and "Float" are there only while the view is.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::palette::{Mode, open};
    add(command(SAVE, "File", "Save", |ui, send| ui.save(send)).keys("Ctrl+S"));
    add(command(CLOSE_EDITOR, "View", "Close Editor", |ui, _| {
        if let Some(p) = ui.active.get_untracked() {
            ui.close(&p);
        }
    })
    .keys("Ctrl+W"));
    add(command(QUICK_OPEN, "", "Go to File…", |ui, send| open(ui, send, Mode::Files)).keys("Ctrl+P"));
    let toggle = |v: View| move |ui: Ui, _: &Send| ui.toggle_group_of(v);
    let reveal = |v: View| move |ui: Ui, _: &Send| ui.reveal(v);
    add(command(TOGGLE_PRIMARY, "View", "Toggle Primary Side Bar Visibility", toggle(View::Files)).keys("Ctrl+B"));
    add(command(TOGGLE_SECONDARY, "View", "Toggle Secondary Side Bar Visibility", toggle(View::Agent))
        .keys("Ctrl+Alt+B"));
    add(command(TOGGLE_PANEL, "View", "Toggle Panel Visibility", toggle(View::Terminal)).keys("Ctrl+J"));
    add(command(SHOW_EXPLORER, "View", "Show Explorer", reveal(View::Files)).keys("Ctrl+Shift+E"));
    add(command(SHOW_SEARCH, "View", "Show Search", reveal(View::Search)).keys("Ctrl+Shift+F"));
    add(command(SHOW_CHANGES, "View", "Show the Agent's Changes", reveal(View::Changes)));
    add(command(SHOW_OUTLINE, "View", "Show Outline", reveal(View::Outline)));
    add(command(FOCUS_TERMINAL, "Terminal", "Focus Terminal", reveal(View::Terminal)).keys("Ctrl+`"));
    add(command(NEW_TERMINAL, "Terminal", "Create New Terminal", |ui, send| {
        ui.reveal(View::Terminal);
        crate::terminal::new_terminal(ui, send);
    })
    .keys("Ctrl+Shift+`"));
    add(command(KILL_TERMINAL, "Terminal", "Kill the Active Terminal Instance", crate::terminal::kill));
    add(command(CLEAR_TERMINAL, "Terminal", "Clear", |ui, _| crate::terminal::clear(ui)));
    add(command(SHOW_AGENT, "View", "Show Agent", reveal(View::Agent)).keys("Ctrl+Alt+I"));
    add(command(SELECT_THEME, "Preferences", "Color Theme", |ui, send| open(ui, send, Mode::Themes))
        .keys("Ctrl+K Ctrl+T"));
    add(command(ABOUT, "Help", "About", |ui, _| ui.message.set(format!("Code Arena {}", env!("CARGO_PKG_VERSION")))));
    // The dock's own commands, as Visual Studio's Window menu has them.
    add(command(RESET_VIEWS, "View", "Reset View Locations", |ui, _| ui.reset_dock()));
    for v in crate::layout::VIEWS {
        add(command(&float_command(v), "View", &format!("Float {}", v.title()), move |ui, _| ui.float(v))
            .when(move |ui| ui.available(v)));
    }
    // Each view the window did not begin with: shown by its command and its key, once it is there.
    for v in crate::layout::VIEWS.into_iter().filter(|v| !v.command().is_empty()) {
        let mut show = command(v.command(), "View", &format!("Show {}", v.title()), reveal(v));
        if !v.keys().is_empty() {
            show = show.keys(v.keys());
        }
        add(show.when(move |ui| ui.available(v)));
    }
    // Commands of a key or of a menu only: not in the palette.
    add(command(SHOW_COMMANDS, "View", "Show All Commands", |ui, send| open(ui, send, Mode::Commands))
        .keys("Ctrl+Shift+P")
        .keys("F1")
        .hidden());
    add(command(FIND, "", "Find", |ui, _| crate::editor::find_show(ui)).keys("Ctrl+F").hidden());
    // After Focus Terminal, so the key they share is this one's: it hides the terminal when it is the view shown.
    add(command(TOGGLE_TERMINAL, "View", "Toggle Terminal", |ui, _| ui.toggle_view(View::Terminal))
        .keys("Ctrl+`")
        .hidden());
    add(command(QUIT, "File", "Exit", |_, _| floem::quit_app()).hidden());
    register_menus();
    register_status();
}

/// The title bar's menus as the window began with them. What is not there yet is dimmed.
fn register_menus() {
    use crate::menus::{EDIT, FILE, GO, HELP, Menu, RUN, SELECTION, TERMINAL, VIEW, add, item, soon};
    let bar = Menu::TitleBar;
    add(bar(FILE), item(SAVE).group("1_save"));
    add(bar(FILE), item(CLOSE_EDITOR).group("2_close"));
    add(bar(FILE), item(QUIT).group("3_exit"));
    add(bar(EDIT), soon("Undo", "Ctrl+Z").group("1_undo"));
    add(bar(EDIT), soon("Redo", "Ctrl+Y").group("1_undo"));
    add(bar(EDIT), soon("Find", "Ctrl+F").group("2_find"));
    add(bar(SELECTION), soon("Select All", "Ctrl+A"));
    // The views: the ones the window began with under the names its menu gave them, the others under their own.
    for (n, v) in crate::layout::VIEWS.into_iter().enumerate() {
        let (command, title) = match v {
            View::Files => (SHOW_EXPLORER, "Explorer"),
            View::Search => (SHOW_SEARCH, "Search"),
            View::Changes => (SHOW_CHANGES, "Changes"),
            View::Outline => (SHOW_OUTLINE, "Outline"),
            View::Agent => (SHOW_AGENT, "Chat"),
            View::Terminal => (FOCUS_TERMINAL, "Terminal"),
            other => (other.command(), other.title()),
        };
        add(bar(VIEW), item(command).title(title).group("1_views").order(n as i32).when(move |ui| ui.available(v)));
    }
    add(bar(VIEW), item(TOGGLE_PRIMARY).title("Primary Side Bar").group("2_parts").order(1));
    add(bar(VIEW), item(TOGGLE_SECONDARY).title("Secondary Side Bar").group("2_parts").order(2));
    add(bar(VIEW), item(TOGGLE_PANEL).title("Panel").group("2_parts").order(3));
    add(bar(VIEW), item(RESET_VIEWS).group("3_reset"));
    add(bar(GO), item(QUICK_OPEN).title("Go to File...").group("3_file_nav"));
    add(bar(RUN), soon("Start Debugging", "F5"));
    add(bar(TERMINAL), soon("New Terminal", "Ctrl+Shift+`"));
    add(bar(HELP), item(ABOUT));
}

/// VS Code's keys for the workbench: the command a key (or a chord of two) is bound to runs; Escape, bound to
/// none, closes what is open over the window.
pub(crate) fn key(ui: Ui, send: &Send, key: &Key, m: Modifiers) {
    crate::layout_commands::key_seen(ui, key, m);
    if crate::layout_commands::zen_escape(ui, key, m) {
        return;
    }
    // An editing command's key does nothing where no editor has the keys (the editor runs it itself).
    if crate::editor::editing::editor_only(key, m) {
        return;
    }
    if crate::commands::press(ui, send, key, m) != crate::commands::Pressed::Free {
        return;
    }
    if matches!(key, Key::Named(NamedKey::Escape)) {
        ui.menu.set(None);
        ui.context.set(None);
        // A drag given up: the view stays where it was.
        ui.drag.set(None);
    }
}

fn title_bar(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let menus = h_stack((dyn_stack(
        || MENUS.into_iter().enumerate().collect::<Vec<_>>(),
        |(n, _)| *n,
        move |(n, name)| {
            label(move || name)
                .on_move(move |p| crate::layout_commands::title_at(n, p.x))
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
    let window = |name: &'static str, close: bool, on: std::rc::Rc<dyn Fn()>| {
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
        window("chrome-minimize", false, std::rc::Rc::new(minimize_window)),
        window("chrome-maximize", false, std::rc::Rc::new(toggle_window_maximized)),
        // As File > Exit: what is not saved is kept aside or asked about first (`save.rs`).
        window(
            "chrome-close",
            true,
            std::rc::Rc::new(move || match ui.out.get_untracked() {
                Some(send) => {
                    crate::commands::run(ui, &send, QUIT);
                }
                None => floem::quit_app(),
            }),
        ),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.title_border, 'b').height(35.0).width_full().items_center().background(c.title_bg).color(c.title_fg)
    })
}

/// The open menu, under its title, and a layer behind it that closes it on a click elsewhere. It has the keys while
/// it is open (`layout_commands::menu_key`: the arrows, Enter, Left and Right to the menus beside it).
fn menu_overlay(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let lit = ui.scope.create_rw_signal(None::<usize>);
    dyn_container(
        move || ui.menu.get(),
        move |open| {
            let Some(n) = open else {
                crate::layout_commands::menu_closed(ui);
                return empty().into_any();
            };
            if lit.get_untracked().is_some() {
                lit.set(None);
            }
            let send = send.clone();
            let keys = send.clone();
            let shown = crate::menus::rows(ui, crate::menus::Menu::TitleBar(n));
            let live: Vec<bool> = shown.iter().map(|r| matches!(r, Row::Entry { command: Some(_), .. })).collect();
            // Where the menu's title is, as laid out (before that: the icon's 35 and the titles before it, each its
            // text and 16 of padding).
            let left = crate::layout_commands::title_x(n)
                .unwrap_or_else(|| 35.0 + MENUS[..n].iter().map(|m| m.len() as f64 * 7.4 + 16.0).sum::<f64>());
            let rows = crate::menus::rows(ui, crate::menus::Menu::TitleBar(n));
            let items = dyn_stack(
                move || rows.clone().into_iter().enumerate().collect::<Vec<_>>(),
                |(i, _)| *i,
                move |(i, row): (usize, Row)| {
                    let send = send.clone();
                    let Row::Entry { title, keys, command, .. } = row else {
                        return empty()
                            .style(move |s| {
                                s.height(1.0).margin_vert(4.0).margin_horiz(0.0).background(t.get().menu_separator)
                            })
                            .into_any();
                    };
                    let live = command.is_some();
                    h_stack((
                        label(move || title.clone()).style(|s| s.flex_grow(1.0_f32)),
                        label(move || keys.clone()).style(|s| s.padding_left(30.0)),
                    ))
                    .on_click_stop(move |_| {
                        if let Some(id) = &command {
                            ui.menu.set(None);
                            crate::commands::run(ui, &send, id);
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
                        if live {
                            s.color(c.menu_fg)
                                .hover(|s| s.background(c.menu_selected).color(c.menu_selected_fg))
                                .apply_if(lit.get() == Some(i), |s| {
                                    s.background(c.menu_selected).color(c.menu_selected_fg)
                                })
                        } else {
                            s.color(crate::widgets::mix(c.menu_fg, c.menu_bg, 0.6))
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
            // Over the window while the menu is open, as VS Code's menu bar: a click on another menu's title opens
            // that one, the pointer going onto one switches to it, a click on its own title or anywhere else closes
            // it.
            let catcher = empty()
                .on_event_stop(EventListener::PointerDown, move |e| {
                    let Event::PointerDown(p) = e else { return };
                    let open = ui.menu.get_untracked();
                    match title_under(p.pos.x, p.pos.y) {
                        Some(m) if open != Some(m) => ui.menu.set(Some(m)),
                        _ => ui.menu.set(None),
                    }
                })
                .on_event_cont(EventListener::PointerMove, move |e| {
                    if let Event::PointerMove(p) = e
                        && let Some(m) = title_under(p.pos.x, p.pos.y)
                        && ui.menu.get_untracked().is_some_and(|open| open != m)
                    {
                        ui.menu.set(Some(m));
                    }
                })
                .style(|s| s.size_full());
            let layer = stack((catcher, items))
                .keyboard_navigable()
                .on_event_stop(EventListener::KeyDown, move |e| {
                    let Event::KeyDown(k) = e else { return };
                    use crate::layout_commands::MenuKey;
                    match crate::layout_commands::menu_key(&live, lit.get_untracked(), &k.key.logical_key) {
                        MenuKey::Lit(to) => lit.set(to),
                        MenuKey::Switch(by) => {
                            ui.menu.set(Some((n as isize + by).rem_euclid(MENUS.len() as isize) as usize))
                        }
                        MenuKey::Run(row) => {
                            // The command run says where the keys go.
                            crate::layout_commands::menu_keys_kept();
                            ui.menu.set(None);
                            if let Some(Row::Entry { command: Some(id), .. }) = shown.get(row) {
                                crate::commands::run(ui, &keys, id);
                            }
                        }
                        MenuKey::Close => ui.menu.set(None),
                        MenuKey::Nothing => {}
                    }
                })
                .style(|s| s.position(Position::Absolute).inset(0.0).size_full());
            // It has the keys while it is open (by a click too: the click took them from the editor), and gives
            // them back when it closes.
            crate::layout_commands::menu_took_keys();
            let id = floem::View::id(&layer);
            floem::action::exec_after(std::time::Duration::ZERO, move |_| id.request_focus());
            layer.into_any()
        },
    )
    .style(move |s| {
        s.position(Position::Absolute).inset(0.0).apply_if(ui.menu.with(|m| m.is_none()), |s| s.hide()).z_index(100)
    })
}

/// The title bar menu whose title is at `x`, `y` across the window (none: no title is there). A title spans from
/// where it is laid out to where the next begins; the last, its text and padding.
fn title_under(x: f64, y: f64) -> Option<usize> {
    let starts: Vec<Option<f64>> = (0..MENUS.len()).map(crate::layout_commands::title_x).collect();
    title_at_point(&starts, x, y)
}

fn title_at_point(starts: &[Option<f64>], x: f64, y: f64) -> Option<usize> {
    if !(0.0..30.0).contains(&y) {
        return None;
    }
    let n = starts.iter().rposition(|s| s.is_some_and(|s| s <= x))?;
    let end = match starts.get(n + 1).copied().flatten() {
        Some(next) => next,
        None => starts[n]? + MENUS[n].len() as f64 * 7.4 + 16.0,
    };
    (x < end).then_some(n)
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

/// The status bar's items as the window began with them (see `statusbar.rs` for the priorities).
fn register_status() {
    use crate::statusbar::{Side, add, item};
    add("status.remote", Side::Left, 90, |ui, _| {
        let t = ui.colors;
        container(icon("remote", 16.0))
            .style(move |s| {
                let c = t.get();
                s.height_full().padding_horiz(10.0).items_center().background(c.remote_bg).color(c.remote_fg)
            })
            .into_any()
    });
    add("status.branch", Side::Left, 100, |ui, _| {
        let t = ui.colors;
        h_stack((icon("git-branch", 16.0), label(move || ui.branch.get())))
            .style(move |s| item(t, s).apply_if(ui.branch.with(|b| b.is_empty()), |s| s.hide()))
            .into_any()
    });
    add("status.problems", Side::Left, 120, |ui, _| {
        let t = ui.colors;
        h_stack((icon("error", 16.0), label(|| "0"), icon("warning", 16.0), label(|| "0")))
            .style(move |s| item(t, s))
            .into_any()
    });
    add("status.message", Side::Left, 170, |ui, _| {
        let t = ui.colors;
        label(move || ui.message.get())
            .style(move |s| item(t, s).apply_if(ui.message.with(|m| m.is_empty()), |s| s.hide()))
            .into_any()
    });
    add("status.selection", Side::Right, 200, |ui, send| {
        let t = ui.colors;
        label(move || {
            let (l, c) = ui.caret.get();
            format!("Ln {l}, Col {c}")
        })
        .on_click_stop(move |_| crate::palette::open(ui, &send, crate::palette::Mode::Line))
        .style(move |s| item(t, s).apply_if(!crate::editor::kinds::text_shown(ui), |s| s.hide()))
        .into_any()
    });
    add("status.encoding", Side::Right, 220, |ui, _| {
        let t = ui.colors;
        label(|| "UTF-8")
            .style(move |s| item(t, s).apply_if(!crate::editor::kinds::text_shown(ui), |s| s.hide()))
            .into_any()
    });
    add("status.language", Side::Right, 240, |ui, _| {
        let t = ui.colors;
        label(move || ui.active.get().and_then(|p| ui.tab(&p)).map(|t| t.colours.language()).unwrap_or_default())
            .style(move |s| item(t, s).apply_if(!crate::editor::kinds::text_shown(ui), |s| s.hide()))
            .into_any()
    });
    add("status.model", Side::Right, 291, |ui, _| {
        let t = ui.colors;
        h_stack((
            dyn_container(move || ui.running.get(), |r| icon(if r { "loading" } else { "sparkle" }, 16.0)),
            label(move || ui.model.get()),
        ))
        .style(move |s| item(t, s))
        .into_any()
    });
}

/// The status bar: the left items, the room between, the right items, as their registry has them at start.
fn status_bar(ui: Ui, send: Send) -> impl IntoView {
    use crate::statusbar::{Side, items};
    let t = ui.colors;
    let side = |side: Side| {
        let send = send.clone();
        h_stack_from_iter(items(side).into_iter().map(move |i| (i.view)(ui, send.clone()))).style(|s| s.height_full())
    };
    h_stack((side(Side::Left), empty().style(|s| s.flex_grow(1.0_f32)), side(Side::Right))).style(move |s| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_under_the_pointer_spans_to_the_next_and_the_last_to_its_text() {
        let starts =
            [Some(40.0), Some(80.0), Some(120.0), Some(200.0), Some(240.0), Some(280.0), Some(320.0), Some(390.0)];
        assert_eq!(title_at_point(&starts, 85.0, 15.0), Some(1));
        assert_eq!(title_at_point(&starts, 39.0, 15.0), None);
        assert_eq!(title_at_point(&starts, 400.0, 15.0), Some(7));
        assert_eq!(title_at_point(&starts, 460.0, 15.0), None, "past Help");
        assert_eq!(title_at_point(&starts, 85.0, 200.0), None, "below the title bar");
        assert_eq!(title_at_point(&[None; 8], 85.0, 15.0), None, "not laid out yet");
    }
}
