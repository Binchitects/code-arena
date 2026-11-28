//! An editor in a window of its own, as VS Code's Move Editor into New Window: the tab's menu (or the command)
//! moves the tab into an OS window that is an editor group of its own, with its tab strip, breadcrumbs and editor.
//! It is the same document (what is typed in one place is typed in the other, unsaved text and all), and files
//! opened while that window has the keys open in it, as in VS Code. Move back into the IDE (the window's button,
//! its tab's menu, or the command) gives its tabs back to the editor group the keys were last in; closing the
//! window does the same, and a window whose last tab closes goes. The window's group is in `groups::Model`'s
//! `apart` (what is tested there); the windows are not kept with the layout: after a restart their tabs are in the
//! editor.

use std::cell::RefCell;
use std::collections::HashMap;

use floem::event::{Event, EventListener};
use floem::reactive::{SignalGet, SignalWith, create_memo};
use floem::views::{Decorators, empty, h_stack, label, tooltip, v_stack};
use floem::window::WindowId;
use serde_json::Value;

use super::groups;
use crate::commands::{self, command};
use crate::menus::{self, Menu, item};
use crate::state::{Send, Ui};
use crate::widgets::action;

/// VS Code's command that moves the tab into a new window, and the one that moves it back.
pub const MOVE_TO_WINDOW: &str = "workbench.action.moveEditorToNewWindow";
pub const MOVE_BACK: &str = "workbench.action.moveEditorToMainWindow";

thread_local! {
    /// Each window's group (by its id) and the window.
    static WINDOWS: RefCell<HashMap<u64, WindowId>> = RefCell::new(HashMap::new());
    /// The editor group the keys were last in in the IDE's window: where the tabs go back to, and the group that
    /// has the keys again once a window loses them.
    static BACK: RefCell<u64> = const { RefCell::new(1) };
    static SEND: RefCell<Option<Send>> = const { RefCell::new(None) };
}

/// What it adds to the window's registries, once at start: the two commands, the tab's menu items, and the
/// windows closed when their group goes.
pub fn register(ui: Ui, send: &Send) {
    SEND.with(|s| *s.borrow_mut() = Some(send.clone()));
    let apart = |ui: Ui| groups::groups(ui).model.with_untracked(|m| m.is_apart(m.focused));
    let shown = |ui: Ui| groups::groups(ui).model.with_untracked(|m| m.shown().is_some());
    commands::add(
        command(MOVE_TO_WINDOW, "View", "Move Editor into New Window", |ui, _| move_to_window(ui))
            .enabled(move |ui| shown(ui) && !apart(ui)),
    );
    commands::add(
        command(MOVE_BACK, "View", "Move Editor back into the IDE", |ui, _| {
            let id = groups::focused(ui);
            move_back(ui, id);
        })
        .enabled(apart),
    );
    menus::add(
        Menu::EditorTab,
        item(MOVE_TO_WINDOW).title("Move into New Window").group("7_window").order(10).when(move |ui| !apart(ui)),
    );
    menus::add(
        Menu::EditorTab,
        item(MOVE_BACK).title("Move back into the IDE").group("7_window").order(10).when(apart),
    );
    // A window whose group went (its last tab closed, or the groups joined) closes.
    let g = groups::groups(ui);
    let ids = create_memo(move |_| g.model.with(|m| m.apart.clone()));
    ui.scope.create_effect(move |_| {
        let ids = ids.get();
        let gone: Vec<WindowId> = WINDOWS.with(|w| {
            let mut w = w.borrow_mut();
            let gone: Vec<u64> = w.keys().filter(|id| !ids.contains(id)).copied().collect();
            gone.iter().filter_map(|id| w.remove(id)).collect()
        });
        for w in gone {
            floem::close_window(w);
        }
    });
}

/// The session's messages that are this part's, taken in: none. Never taken away (false).
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

/// The focused group's tab moved into a window of its own.
fn move_to_window(ui: Ui) {
    let (from, path) = groups::groups(ui).model.with_untracked(|m| (m.focused, m.shown()));
    let Some(path) = path else { return };
    BACK.with(|b| *b.borrow_mut() = from);
    let mut made = None;
    groups::change(ui, |m, _| made = m.detach(from, &path));
    if let Some(id) = made {
        open(ui, id);
    }
}

/// A window's tabs given back to the IDE's editor; the window closes.
fn move_back(ui: Ui, id: u64) {
    let to = BACK.with(|b| *b.borrow());
    groups::change(ui, |m, order| m.attach(id, to, order));
}

/// The window of a group: a bar with its title and the button that moves it back, then the group, in the theme.
fn open(ui: Ui, id: u64) {
    let Some(send) = SEND.with(|s| s.borrow().clone()) else { return };
    let g = groups::groups(ui);
    let title = create_memo(move |_| {
        let shown = g.model.with(|m| m.group(id).and_then(|g| g.active.clone()));
        shown.map(|p| p.rsplit('/').next().unwrap_or(&p).to_string()).unwrap_or_default()
    });
    let config = floem::window::WindowConfig::default().title("Code Arena").size((900.0, 640.0));
    floem::new_window(
        move |wid| {
            WINDOWS.with(|w| w.borrow_mut().insert(id, wid));
            let t = ui.colors;
            let bar = h_stack((
                label(move || title.get()).style(move |s| s.padding_left(12.0).color(t.get().side_fg)),
                empty().style(|s| s.flex_grow(1.0_f32)),
                tooltip(action(t, "layout-sidebar-left", move || move_back(ui, id)), || {
                    label(|| "Move back into the IDE")
                }),
            ))
            .style(move |s| {
                s.height(30.0)
                    .width_full()
                    .items_center()
                    .padding_right(8.0)
                    .background(t.get().title_bg)
                    .border_bottom(1.0)
                    .border_color(t.get().group_border)
                    .selectable(false)
            });
            let keys = send.clone();
            let body = super::group_view(ui, groups::shown_of(ui, id))
                .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
            v_stack((bar, body))
                .window_title(move || {
                    let name = title.get();
                    if name.is_empty() { "Code Arena".to_string() } else { format!("{name} — Code Arena") }
                })
                .on_event_cont(EventListener::PointerDown, move |_| groups::focus(ui, id))
                .on_event_cont(EventListener::KeyDown, move |e| {
                    if let Event::KeyDown(k) = e {
                        let pad = crate::accessibility::numpad(&k.key.logical_key, k.key.location);
                        crate::workbench::key(ui, &keys, pad.as_ref().unwrap_or(&k.key.logical_key), k.modifiers);
                    }
                })
                .on_event_cont(EventListener::WindowGotFocus, move |_| got_focus(ui, id))
                .on_event_cont(EventListener::WindowLostFocus, move |_| lost_focus(ui, id))
                .on_event_cont(EventListener::WindowClosed, move |_| {
                    // Closed by its frame: its tabs back in the IDE (unless its group went, which closed it).
                    if WINDOWS.with(|w| w.borrow_mut().remove(&id)).is_some() {
                        move_back(ui, id);
                    }
                })
                .style(move |s| {
                    let c = t.get();
                    s.size_full()
                        .font_family(crate::widgets::UI_FONT.clone())
                        .font_size(13.0)
                        .color(c.fg)
                        .background(c.editor_bg)
                })
        },
        Some(config),
    );
}

/// The window come to the front: it has the keys, and the IDE's group that had them is where they go back.
fn got_focus(ui: Ui, id: u64) {
    let (focused, main) = groups::groups(ui).model.with_untracked(|m| (m.focused, !m.is_apart(m.focused)));
    if main {
        BACK.with(|b| *b.borrow_mut() = focused);
    }
    groups::focus(ui, id);
}

/// Another window come to the front: the keys back in the IDE's group they were in (a file opened from the
/// explorer then opens there, not in this window).
fn lost_focus(ui: Ui, id: u64) {
    let back = BACK.with(|b| *b.borrow());
    let to = groups::groups(ui).model.with_untracked(|m| {
        (m.focused == id).then(|| if m.group(back).is_some() && !m.is_apart(back) { back } else { m.groups[0].id })
    });
    if let Some(to) = to {
        groups::focus(ui, to);
    }
}
