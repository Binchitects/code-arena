//! The Explorer, as VS Code's: the folder's section (its name in capitals, 22 px), then the tree in 22 px rows, each
//! level 8 px further in with its twistie, a folder opening on a click, a file opening in the editor. Its file
//! operations are VS Code's too: New File and New Folder typed in a row of the tree, a row renamed in place (F2), a
//! file or a folder deleted after a question (Delete), a row's menu on a right-click, and the arrows to go through
//! the tree. The session does the work on disk and says what it did; the tree follows what it says.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use floem::action::{add_overlay, exec_after, remove_overlay};
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::kurbo::{Point, Rect, Size};
use floem::peniko::{Brush, Color};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{
    Decorators, container, dyn_stack, empty, h_stack, label, scroll, stack, text_input, tooltip, v_stack,
    v_stack_from_iter,
};
use floem::{AnyView, IntoView, View};
use serde_json::json;

use crate::icons::icon;
use crate::state::{Entry, Send, Ui};
use crate::widgets::{UI_FONT, action, bold, button, caps, line};

/// VS Code's row in a tree.
const ROW: f64 = 22.0;

/// What the tree's inline input is for.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// A new file or a new folder, in this folder (the project's top: empty).
    New { parent: String, dir: bool },
    /// The row of this path, renamed.
    Rename { path: String },
}

/// A move through the tree with the keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Enter,
}

#[derive(Clone, Debug, PartialEq)]
struct Row {
    depth: usize,
    path: String,
    name: String,
    dir: bool,
    open: bool,
    /// The row is the inline input: a row being renamed, or the one a new name is typed in (its path is then its
    /// folder's and a slash, which no file's is).
    input: bool,
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn name_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, n)| n)
}

/// The rows shown: each folder's entries under it while it is open, and the inline input where it belongs (a new
/// folder before its folder's entries, a new file before its folder's files).
fn rows_of(tree: &HashMap<String, Vec<Entry>>, expanded: &HashSet<String>, edit: Option<&Edit>) -> Vec<Row> {
    fn walk(
        tree: &HashMap<String, Vec<Entry>>,
        expanded: &HashSet<String>,
        edit: Option<&Edit>,
        dir: &str,
        depth: usize,
        out: &mut Vec<Row>,
    ) {
        let new = match edit {
            Some(Edit::New { parent, dir: d }) if parent == dir => Some(*d),
            _ => None,
        };
        let input =
            |d: bool| Row { depth, path: format!("{dir}/"), name: String::new(), dir: d, open: false, input: true };
        let mut placed = false;
        for e in tree.get(dir).map(Vec::as_slice).unwrap_or_default() {
            if let Some(d) = new
                && !placed
                && (d || !e.dir)
            {
                out.push(input(d));
                placed = true;
            }
            let path = join(dir, &e.name);
            let open = e.dir && expanded.contains(&path);
            let renamed = matches!(edit, Some(Edit::Rename { path: p }) if *p == path);
            out.push(Row { depth, path: path.clone(), name: e.name.clone(), dir: e.dir, open, input: renamed });
            if open {
                walk(tree, expanded, edit, &path, depth + 1, out);
            }
        }
        if let Some(d) = new
            && !placed
        {
            out.push(input(d));
        }
    }
    let mut out = Vec::new();
    walk(tree, expanded, edit, "", 0, &mut out);
    out
}

/// The rows drawn. What is typed in the input and which row is selected are not read here, so neither makes the
/// tree again.
fn rows(ui: Ui) -> Vec<Row> {
    ui.tree.with(|t| ui.expanded.with(|x| ui.explorer_edit.with(|e| rows_of(t, x, e.as_ref()))))
}

/// The rows as they are now, for the keys.
fn rows_now(ui: Ui) -> Vec<Row> {
    ui.tree.with_untracked(|t| ui.expanded.with_untracked(|x| rows_of(t, x, None)))
}

fn is_dir(ui: Ui, path: &str) -> bool {
    ui.tree.with_untracked(|t| {
        t.get(parent_of(path)).is_some_and(|es| es.iter().any(|e| e.dir && e.name == name_of(path)))
    })
}

/// The folder a new file goes in, as VS Code chooses it: the folder selected, or the one the selected file is in,
/// or the project's top.
fn folder_of_selected(ui: Ui) -> String {
    let sel = ui.selected.get_untracked();
    if sel.is_empty() || is_dir(ui, &sel) { sel } else { parent_of(&sel).to_string() }
}

/// What is wrong with a name typed, in VS Code's words; none when it can be asked of the session. A name may hold
/// folders on the way ("a/b.rs"), as VS Code's may.
fn problem(tree: &HashMap<String, Vec<Entry>>, edit: &Edit, text: &str) -> Option<String> {
    let name = text.trim();
    if name.is_empty() {
        return None;
    }
    if name.contains(['\\', ':']) || name.split('/').any(|p| p.is_empty() || p == "." || p == ".." || p == ".git") {
        return Some(format!("The name {name} is not valid as a file or folder name. Please choose a different name."));
    }
    let (parent, old) = match edit {
        Edit::New { parent, .. } => (parent.as_str(), None),
        Edit::Rename { path } => (parent_of(path), Some(name_of(path))),
    };
    if Some(name) != old && tree.get(parent).is_some_and(|es| es.iter().any(|e| e.name == name)) {
        return Some(format!(
            "A file or folder {name} already exists at this location. Please choose a different name."
        ));
    }
    None
}

/// The input shown for a new file or folder in `parent`, which is opened (and the folders it is in) so the input
/// is seen.
pub fn begin_new(ui: Ui, send: &Send, parent: &str, dir: bool) {
    let mut at = String::new();
    for part in parent.split('/').filter(|p| !p.is_empty()) {
        at = join(&at, part);
        if !ui.expanded.with_untracked(|e| e.contains(&at)) {
            ui.toggle_folder(&at, send);
        }
    }
    ui.explorer_text.set(String::new());
    ui.explorer_edit.set(Some(Edit::New { parent: parent.to_string(), dir }));
}

/// A row turned into the input, its name in it.
pub fn begin_rename(ui: Ui, path: &str) {
    if path.is_empty() {
        return;
    }
    ui.explorer_text.set(name_of(path).to_string());
    ui.explorer_edit.set(Some(Edit::Rename { path: path.to_string() }));
}

/// Enter in the input: the session is asked to make or to move it. A name that is not valid stays in the input;
/// none typed, or the name it had, is the same as Escape.
pub fn commit(ui: Ui, send: &Send) {
    let Some(edit) = ui.explorer_edit.get_untracked() else { return };
    let text = ui.explorer_text.get_untracked();
    let name = text.trim();
    if !name.is_empty() && ui.tree.with_untracked(|t| problem(t, &edit, name)).is_some() {
        return;
    }
    match &edit {
        _ if name.is_empty() => {}
        Edit::New { parent, dir } => {
            let path = join(parent, name);
            // Opened (a folder: selected) when the session says it is made.
            ui.explorer_asked.set(Some(path.clone()));
            send(json!({"type": "create", "path": path, "dir": dir}));
        }
        Edit::Rename { path } if name != name_of(path) => {
            send(json!({"type": "rename", "path": path, "to": join(parent_of(path), name)}));
        }
        Edit::Rename { .. } => {}
    }
    ui.explorer_edit.set(None);
}

/// A key in the tree: Up and Down move the selection, Left closes a folder (or goes to the folder a row is in),
/// Right opens one (or goes into it), Enter opens a file (or a folder). Says which row is selected after.
pub fn navigate(ui: Ui, send: &Send, nav: Nav) -> Option<usize> {
    let rows = rows_now(ui);
    let last = rows.len().checked_sub(1)?;
    let sel = ui.selected.get_untracked();
    let Some(i) = rows.iter().position(|r| r.path == sel) else {
        // Nothing selected (or a row no longer shown): the first row, or the last for Up.
        let i = if nav == Nav::Up { last } else { 0 };
        ui.selected.set(rows[i].path.clone());
        return Some(i);
    };
    let r = &rows[i];
    let to = match nav {
        Nav::Up => i.saturating_sub(1),
        Nav::Down => (i + 1).min(last),
        Nav::Left if r.dir && r.open => {
            ui.toggle_folder(&r.path, send);
            i
        }
        Nav::Left => rows[..i].iter().rposition(|p| p.depth < r.depth).unwrap_or(i),
        Nav::Right if r.dir && !r.open => {
            ui.toggle_folder(&r.path, send);
            i
        }
        Nav::Right if r.dir && i < last && rows[i + 1].depth > r.depth => i + 1,
        Nav::Right => i,
        Nav::Enter if r.dir => {
            ui.toggle_folder(&r.path, send);
            i
        }
        Nav::Enter => {
            ui.open(&r.path, send);
            i
        }
    };
    if to != i {
        ui.selected.set(rows[to].path.clone());
    }
    Some(to)
}

/// The tree listed again from the session: its top and each open folder.
fn refresh(ui: Ui, send: &Send) {
    ui.tree.update(|t| t.clear());
    send(json!({"type": "ls", "path": ""}));
    for p in ui.expanded.get_untracked() {
        send(json!({"type": "ls", "path": p}));
    }
}

fn focus_tree(ui: Ui) {
    if let Some(id) = ui.explorer_tree.get_untracked() {
        id.request_focus();
    }
}

fn tip(v: impl IntoView + 'static, text: &'static str) -> impl IntoView {
    tooltip(v, move || label(move || text))
}

/// The Explorer title's actions, as VS Code's: New File, New Folder, Refresh, Collapse All.
// Not in the title yet: the title is drawn with the dock, which puts them there.
pub fn actions(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let (file, folder, again) = (send.clone(), send.clone(), send);
    h_stack((
        tip(action(t, "new-file", move || begin_new(ui, &file, &folder_of_selected(ui), false)), "New File..."),
        tip(action(t, "new-folder", move || begin_new(ui, &folder, &folder_of_selected(ui), true)), "New Folder..."),
        tip(action(t, "refresh", move || refresh(ui, &again)), "Refresh Explorer"),
        tip(action(t, "collapse-all", move || ui.expanded.update(|e| e.clear())), "Collapse Folders in Explorer"),
    ))
    .style(|s| s.gap(2.0))
}

/// The window's size, from a view in it (the Explorer may float in a window of its own).
fn window_size(ui: Ui) -> Size {
    let mut top = ui.explorer_tree.get_untracked();
    while let Some(p) = top.and_then(|t| t.parent()) {
        top = Some(p);
    }
    top.and_then(|t| t.get_size()).filter(|s| s.width > 0.0 && s.height > 0.0).unwrap_or_else(|| {
        let (w, h) = ui.window.get_untracked();
        Size::new(w, h)
    })
}

/// The menu or the question closed, and the keys back with the tree.
fn close_popup(ui: Ui) {
    if let Some(id) = ui.explorer_popup.get_untracked() {
        ui.explorer_popup.set(None);
        remove_overlay(id);
        focus_tree(ui);
    }
}

/// A layer over the whole window (an overlay of the window's, so it is drawn over every part and has the pointer
/// first), with `body` in it: a press outside the body or Escape closes it, Enter runs `enter`. It is outside the
/// workbench's own view, so it sets its font and colours itself.
fn popup(ui: Ui, dim: bool, body: impl FnOnce(Size) -> AnyView + 'static, enter: Option<Rc<dyn Fn()>>) {
    close_popup(ui);
    let t = ui.colors;
    let size = window_size(ui);
    let id = add_overlay(Point::ZERO, move |_| {
        let layer = stack((
            empty()
                .on_event_stop(EventListener::PointerDown, move |_| close_popup(ui))
                .style(|s| s.absolute().inset(0.0).size_full()),
            body(size),
        ))
        .keyboard_navigable()
        // Every key stays here while it is open (none goes on to the workbench's own).
        .on_event_stop(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(k) = e {
                match (&k.key.logical_key, &enter) {
                    (Key::Named(NamedKey::Escape), _) => close_popup(ui),
                    (Key::Named(NamedKey::Enter), Some(run)) => run(),
                    _ => {}
                }
            }
        })
        .style(move |s| {
            let c = t.get();
            // A little short of the window: an overlay that reaches the window's edge is drawn moved.
            s.width(size.width - 6.0)
                .height(size.height - 6.0)
                .items_center()
                .justify_center()
                .font_family(UI_FONT.clone())
                .font_size(13.0)
                .color(c.fg)
                .apply_if(dim, |s| s.background(Color::rgba8(0, 0, 0, 0x4D)))
        });
        layer.id().request_focus();
        layer
    });
    ui.explorer_popup.set(Some(id));
}

/// A menu's item: its label, its keys, what it does ("-": a separator).
type MenuItem = (&'static str, &'static str, Rc<dyn Fn()>);

/// VS Code's menu for a row (or for the tree's empty part: `path` empty), where the pointer is in the window.
fn open_menu(ui: Ui, send: &Send, path: String, dir: bool, at: (f64, f64)) {
    let t = ui.colors;
    let folder = if dir { path.clone() } else { parent_of(&path).to_string() };
    let separator = || -> MenuItem { ("-", "", Rc::new(|| {})) };
    let new = |dir: bool| -> Rc<dyn Fn()> {
        let (send, folder) = (send.clone(), folder.clone());
        Rc::new(move || begin_new(ui, &send, &folder, dir))
    };
    let mut items: Vec<MenuItem> = vec![("New File...", "", new(false)), ("New Folder...", "", new(true))];
    if !path.is_empty() {
        let copy = |text: String| -> Rc<dyn Fn()> {
            Rc::new(move || {
                if floem::Clipboard::set_contents(text.clone()).is_err() {
                    ui.message.set("The path could not be copied.".into());
                }
            })
        };
        let (renamed, deleted, send) = (path.clone(), path.clone(), send.clone());
        let rename: Rc<dyn Fn()> = Rc::new(move || begin_rename(ui, &renamed));
        let delete: Rc<dyn Fn()> = Rc::new(move || ask_delete(ui, &send, &deleted));
        items.extend([
            separator(),
            ("Copy Path", "", copy(join(&ui.folder.get_untracked(), &path))),
            ("Copy Relative Path", "", copy(path.clone())),
            separator(),
            ("Rename...", "F2", rename),
            ("Delete", "Delete", delete),
        ]);
    }
    // Its place: where the pointer is, kept inside the window (above the pointer when there is no room under).
    let (width, height) = (260.0, items.iter().map(|i| if i.0 == "-" { 9.0 } else { 26.0 }).sum::<f64>() + 10.0);
    popup(
        ui,
        false,
        move |size| {
            let x = at.0.min(size.width - width - 8.0).max(0.0);
            let y = if at.1 + height > size.height - 8.0 { (at.1 - height).max(0.0) } else { at.1 };
            v_stack_from_iter(items.into_iter().map(move |(text, keys, run)| {
                if text == "-" {
                    return empty()
                        .style(move |s| s.height(1.0).margin_vert(4.0).background(t.get().menu_separator))
                        .into_any();
                }
                h_stack((
                    label(move || text).style(|s| s.flex_grow(1.0_f32)),
                    label(move || keys).style(|s| s.padding_left(30.0)),
                ))
                .on_click_stop(move |_| {
                    close_popup(ui);
                    run();
                })
                .style(move |s| {
                    let c = t.get();
                    s.height(26.0)
                        .items_center()
                        .padding_left(26.0)
                        .padding_right(20.0)
                        .border_radius(4.0)
                        .margin_horiz(4.0)
                        .color(c.menu_fg)
                        .hover(|s| s.background(c.menu_selected).color(c.menu_selected_fg))
                })
                .into_any()
            }))
            .style(move |s| {
                let c = t.get();
                s.absolute()
                    .inset_left(x)
                    .inset_top(y)
                    .min_width(220.0)
                    .max_width(width)
                    .padding_vert(4.0)
                    .border(1.0)
                    .border_color(c.menu_border)
                    .border_radius(6.0)
                    .background(c.menu_bg)
                    .box_shadow_blur(8.0)
                    .box_shadow_color(c.shadow)
                    .font_size(13.0)
                    .selectable(false)
            })
            .into_any()
        },
        None,
    );
}

/// VS Code's question before a delete, as its dialog: the question, what becomes of the file, the two buttons.
/// What is deleted is not gone: the session keeps it among its backups.
fn ask_delete(ui: Ui, send: &Send, path: &str) {
    if path.is_empty() {
        return;
    }
    let t = ui.colors;
    let dir = is_dir(ui, path);
    let name = name_of(path);
    let question = if dir {
        format!("Are you sure you want to delete '{name}' and its contents?")
    } else {
        format!("Are you sure you want to delete '{name}'?")
    };
    let detail = if dir {
        "You can restore this folder from the backups."
    } else {
        "You can restore this file from the backups."
    };
    let yes: Rc<dyn Fn()> = {
        let (send, path) = (send.clone(), path.to_string());
        Rc::new(move || {
            close_popup(ui);
            send(json!({"type": "delete", "path": path}));
        })
    };
    let pressed = yes.clone();
    popup(
        ui,
        true,
        move |size| {
            v_stack((
                label(move || question.clone())
                    .style(|s| s.font_size(18.0).line_height(1.25).text_overflow(TextOverflow::Wrap)),
                label(move || detail).style(|s| s.margin_top(10.0).text_overflow(TextOverflow::Wrap)),
                h_stack((
                    button(t, "Move to Backups", true, move || pressed()),
                    button(t, "Cancel", false, move || close_popup(ui)),
                ))
                .style(|s| s.margin_top(20.0).gap(8.0).justify_end()),
            ))
            .style(move |s| {
                let c = t.get();
                s.width(450.0_f64.min(size.width - 40.0))
                    .padding(20.0)
                    .border(1.0)
                    .border_color(c.widget_border)
                    .border_radius(6.0)
                    .background(c.widget)
                    .color(c.fg)
                    .box_shadow_blur(16.0)
                    .box_shadow_color(c.shadow)
                    .selectable(false)
            })
            .into_any()
        },
        Some(yes),
    );
}

type OpenMenu = Rc<dyn Fn(String, bool)>;

/// The inline input, in a row of the tree: Enter asks the session, Escape or the focus lost gives it up. What is
/// wrong with the name is said under it, as VS Code says it.
fn input_row(ui: Ui, send: Send, r: Row) -> impl IntoView {
    let t = ui.colors;
    let mine = ui.explorer_edit.get_untracked();
    let wrong = move || {
        ui.explorer_edit.with(|e| {
            e.as_ref().and_then(|e| ui.tree.with(|tree| ui.explorer_text.with(|text| problem(tree, e, text))))
        })
    };
    let input = text_input(ui.explorer_text)
        .on_event(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(k) = e
                && k.key.logical_key == Key::Named(NamedKey::Enter)
            {
                commit(ui, &send);
                if ui.explorer_edit.with_untracked(|e| e.is_none()) {
                    focus_tree(ui);
                }
            }
            // No key goes on to the workbench's own while a name is typed.
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            // Escape, or a press elsewhere: given up (unless another input has been opened since).
            if ui.explorer_edit.get_untracked() == mine {
                ui.explorer_edit.set(None);
            }
        })
        .style(move |s| {
            let c = t.get();
            s.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
                .height(20.0)
                .padding_horiz(3.0)
                .border(0.0)
                .font_size(13.0)
                .background(Color::TRANSPARENT)
                .color(c.input_fg)
                .cursor_color(Brush::Solid(c.cursor))
        });
    let id = input.id();
    // Once the row is in the tree, the input has the keys.
    exec_after(Duration::ZERO, move |_| id.request_focus());
    let twistie =
        if r.dir { icon("chevron-right", 16.0).into_any() } else { empty().style(|s| s.width(16.0)).into_any() };
    let indent = 8.0 * r.depth as f64;
    let field = container(input).style(move |s| {
        let c = t.get();
        s.flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_width(0.0)
            .height(ROW)
            .margin_left(4.0)
            .items_center()
            .border(1.0)
            .border_color(if wrong().is_some() { c.input_error } else { c.focus })
            .background(c.input_bg)
    });
    v_stack((
        h_stack((empty().style(move |s| s.width(indent)), twistie, field)).style(move |s| {
            s.height(ROW).width_full().items_center().padding_left(8.0).padding_right(12.0).color(t.get().side_fg)
        }),
        label(move || wrong().unwrap_or_default()).style(move |s| {
            let c = t.get();
            s.margin_left(indent + 28.0)
                .margin_right(12.0)
                .padding_horiz(4.0)
                .padding_vert(3.0)
                .font_size(12.0)
                .line_height(1.4)
                .border(1.0)
                .border_color(c.input_error)
                .background(c.input_error_bg)
                .color(c.fg)
                .text_overflow(TextOverflow::Wrap)
                .apply_if(wrong().is_none(), |s| s.hide())
        }),
    ))
    .style(|s| s.width_full())
}

fn row_view(ui: Ui, send: Send, r: Row, focused: RwSignal<bool>, menu: OpenMenu) -> AnyView {
    if r.input {
        return input_row(ui, send, r).into_any();
    }
    let t = ui.colors;
    let (path, shown, target) = (r.path.clone(), r.path.clone(), r.path.clone());
    h_stack((
        empty().style(move |s| s.width(8.0 * r.depth as f64)),
        if r.dir {
            icon(if r.open { "chevron-down" } else { "chevron-right" }, 16.0).into_any()
        } else {
            empty().style(|s| s.width(16.0)).into_any()
        },
        label(move || r.name.clone()).style(|s| s.padding_left(4.0).text_ellipsis()),
    ))
    .on_click_stop(move |_| {
        focus_tree(ui);
        if r.dir {
            ui.selected.set(path.clone());
            ui.toggle_folder(&path, &send);
        } else {
            ui.open(&path, &send);
        }
    })
    .on_secondary_click_stop(move |_| {
        ui.selected.set(target.clone());
        menu(target.clone(), r.dir);
    })
    .style(move |s| {
        let c = t.get();
        let selected = ui.selected.with(|x| *x == shown);
        let s = s
            .height(ROW)
            .width_full()
            .items_center()
            .padding_left(8.0)
            .font_size(13.0)
            .color(c.side_fg)
            .cursor(CursorStyle::Pointer)
            .selectable(false);
        // VS Code's selection: lit while the tree has the keys, dimmed while it has not.
        match (selected, focused.get()) {
            (true, true) => s.background(c.list_selected).color(c.list_selected_fg),
            (true, false) => s.background(c.list_inactive),
            _ => s.hover(|s| s.background(c.list_hover)),
        }
    })
    .into_any()
}

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    // The tree has the keys; the row to keep in sight after a key (how far down it is); and where the pointer is
    // in the window, for a menu opened by a right-click.
    let focused = create_rw_signal(false);
    let reveal = create_rw_signal(0.0_f64);
    let pointer = create_rw_signal((0.0_f64, 0.0_f64));
    let menu: OpenMenu = {
        let send = send.clone();
        Rc::new(move |path, dir| open_menu(ui, &send, path, dir, pointer.get_untracked()))
    };
    let header = h_stack((
        icon("chevron-down", 16.0),
        label(move || ui.folder.get().rsplit('/').next().unwrap_or("").to_uppercase()).style(|s| bold(caps(s))),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.section_border, 'b')
            .height(22.0)
            .items_center()
            .gap(2.0)
            .padding_left(2.0)
            .background(c.section_bg)
            .color(c.section_fg)
            .selectable(false)
    });
    let list = {
        let (send, menu) = (send.clone(), menu.clone());
        dyn_stack(
            move || rows(ui),
            |r: &Row| (r.path.clone(), r.open, r.input),
            move |r: Row| row_view(ui, send.clone(), r, focused, menu.clone()),
        )
        .style(|s| s.flex_col().width_full())
    };
    let tree = scroll(list)
        .ensure_visible(move || {
            let y = reveal.get();
            Rect::new(0.0, y, 1.0, y + ROW)
        })
        .keyboard_navigable()
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            if !k.modifiers.is_empty() {
                return EventPropagation::Continue;
            }
            let nav = match &k.key.logical_key {
                Key::Named(NamedKey::ArrowUp) => Nav::Up,
                Key::Named(NamedKey::ArrowDown) => Nav::Down,
                Key::Named(NamedKey::ArrowLeft) => Nav::Left,
                Key::Named(NamedKey::ArrowRight) => Nav::Right,
                Key::Named(NamedKey::Enter) => Nav::Enter,
                Key::Named(NamedKey::F2) => {
                    begin_rename(ui, &ui.selected.get_untracked());
                    return EventPropagation::Stop;
                }
                Key::Named(NamedKey::Delete) => {
                    ask_delete(ui, &send, &ui.selected.get_untracked());
                    return EventPropagation::Stop;
                }
                _ => return EventPropagation::Continue,
            };
            if let Some(i) = navigate(ui, &send, nav) {
                reveal.set(i as f64 * ROW);
            }
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| focused.set(false))
        // The tree's empty part: a click selects nothing (a new file then goes in the project's top), a right-click
        // opens the menu for the top.
        .on_click_stop(move |_| ui.selected.set(String::new()))
        .on_secondary_click_stop(move |_| menu(String::new(), true))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full());
    ui.explorer_tree.set(Some(tree.id()));
    let whole = v_stack((header, tree));
    let id = whole.id();
    whole
        .on_event_cont(EventListener::PointerMove, move |e| {
            if let Event::PointerMove(p) = e {
                // The event's point is this view's own: with the view's place in the window, the window's.
                let o = id.layout_rect().origin();
                pointer.set((o.x + p.pos.x, o.y + p.pos.y));
            }
        })
        .style(|s| s.size_full().min_height(0.0))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json::Value;

    use super::*;

    fn sender() -> (Send, Rc<RefCell<Vec<Value>>>) {
        let asked = Rc::new(RefCell::new(Vec::new()));
        let to = asked.clone();
        (Rc::new(move |v: Value| to.borrow_mut().push(v)), asked)
    }

    /// src (open: in (closed), a.rs, b.rs), docs (closed), z.txt.
    fn project() -> Ui {
        let ui = Ui::new();
        let e = |name: &str, dir: bool| Entry { name: name.into(), dir };
        ui.tree.set(
            [
                (String::new(), vec![e("docs", true), e("src", true), e("z.txt", false)]),
                ("src".to_string(), vec![e("in", true), e("a.rs", false), e("b.rs", false)]),
                ("src/in".to_string(), vec![e("deep.rs", false)]),
            ]
            .into(),
        );
        ui.expanded.set(["src".to_string()].into());
        ui
    }

    fn shown(ui: Ui) -> Vec<String> {
        rows(ui).into_iter().map(|r| if r.input { format!("[{}]", r.path) } else { r.path }).collect()
    }

    #[test]
    fn the_input_is_a_row_where_the_new_entry_will_be_or_the_row_renamed() {
        let ui = project();
        let (send, asked) = sender();
        assert_eq!(shown(ui), ["docs", "src", "src/in", "src/a.rs", "src/b.rs", "z.txt"]);
        // A new file: after its folder's folders; a new folder: before them; one level in.
        begin_new(ui, &send, "src", false);
        assert_eq!(shown(ui), ["docs", "src", "src/in", "[src/]", "src/a.rs", "src/b.rs", "z.txt"]);
        assert_eq!(rows(ui)[3].depth, 1);
        begin_new(ui, &send, "src", true);
        assert_eq!(shown(ui), ["docs", "src", "[src/]", "src/in", "src/a.rs", "src/b.rs", "z.txt"]);
        begin_new(ui, &send, "", false);
        assert_eq!(shown(ui), ["docs", "src", "src/in", "src/a.rs", "src/b.rs", "[/]", "z.txt"]);
        assert!(asked.borrow().is_empty(), "the folders were open and listed already");
        // In a closed folder inside a closed one: both are opened, and the one not listed yet is asked for.
        ui.expanded.set(HashSet::new());
        ui.tree.update(|t| {
            t.remove("src/in");
        });
        begin_new(ui, &send, "src/in", false);
        assert_eq!(shown(ui), ["docs", "src", "src/in", "[src/in/]", "src/a.rs", "src/b.rs", "z.txt"]);
        assert_eq!(*asked.borrow(), [json!({"type": "ls", "path": "src/in"})]);
        // A row renamed is the input itself, with its name in it.
        begin_rename(ui, "src/a.rs");
        assert_eq!(shown(ui), ["docs", "src", "src/in", "[src/a.rs]", "src/b.rs", "z.txt"]);
        assert_eq!(ui.explorer_text.get_untracked(), "a.rs");
    }

    #[test]
    fn enter_asks_the_session_and_a_name_that_is_not_valid_stays_in_the_input() {
        let ui = project();
        let (send, asked) = sender();
        begin_new(ui, &send, "src", false);
        ui.explorer_text.set("a.rs".into());
        commit(ui, &send);
        assert!(ui.explorer_edit.get_untracked().is_some() && asked.borrow().is_empty(), "it is there already");
        for bad in ["..", "a/../b", "/a", "a/", "a\\b", ".git/x"] {
            ui.explorer_text.set(bad.into());
            commit(ui, &send);
            assert!(ui.explorer_edit.get_untracked().is_some() && asked.borrow().is_empty(), "{bad}");
        }
        ui.explorer_text.set(" new.rs ".into());
        commit(ui, &send);
        assert_eq!(*asked.borrow(), [json!({"type": "create", "path": "src/new.rs", "dir": false})]);
        assert!(ui.explorer_edit.get_untracked().is_none());
        assert_eq!(ui.explorer_asked.get_untracked().as_deref(), Some("src/new.rs"));
        asked.borrow_mut().clear();
        // A folder, at the top, with a folder on the way.
        begin_new(ui, &send, "", true);
        ui.explorer_text.set("x/y".into());
        commit(ui, &send);
        assert_eq!(*asked.borrow(), [json!({"type": "create", "path": "x/y", "dir": true})]);
        asked.borrow_mut().clear();
        // Renamed: asked with the folder it is in; its own name or no name asks nothing.
        begin_rename(ui, "src/a.rs");
        commit(ui, &send);
        begin_rename(ui, "src/a.rs");
        ui.explorer_text.set(String::new());
        commit(ui, &send);
        assert!(asked.borrow().is_empty() && ui.explorer_edit.get_untracked().is_none());
        begin_rename(ui, "src/a.rs");
        ui.explorer_text.set("b.rs".into());
        commit(ui, &send);
        assert!(ui.explorer_edit.get_untracked().is_some(), "another's name");
        ui.explorer_text.set("c.rs".into());
        commit(ui, &send);
        assert_eq!(*asked.borrow(), [json!({"type": "rename", "path": "src/a.rs", "to": "src/c.rs"})]);
    }

    #[test]
    fn what_is_wrong_with_a_name_is_said_as_vs_code_says_it() {
        let ui = project();
        let new = Edit::New { parent: "src".into(), dir: false };
        let said = |edit: &Edit, text: &str| ui.tree.with_untracked(|t| problem(t, edit, text));
        assert_eq!(said(&new, ""), None);
        assert_eq!(said(&new, "new.rs"), None);
        assert_eq!(
            said(&new, "a.rs").as_deref(),
            Some("A file or folder a.rs already exists at this location. Please choose a different name.")
        );
        assert_eq!(
            said(&new, "..").as_deref(),
            Some("The name .. is not valid as a file or folder name. Please choose a different name.")
        );
        assert_eq!(said(&Edit::Rename { path: "src/a.rs".into() }, "a.rs"), None);
    }

    #[test]
    fn the_arrows_go_through_the_tree_and_open_and_close_its_folders() {
        let ui = project();
        let (send, asked) = sender();
        let go = |nav| {
            navigate(ui, &send, nav);
            ui.selected.get_untracked()
        };
        assert_eq!(go(Nav::Down), "docs", "nothing was selected: the first row");
        assert_eq!(go(Nav::Up), "docs", "no row before the first");
        assert_eq!(go(Nav::Down), "src");
        // Right on an open folder goes into it; on a closed one opens it (listed when it was not yet).
        assert_eq!(go(Nav::Right), "src/in");
        ui.tree.update(|t| {
            t.remove("src/in");
        });
        assert_eq!(go(Nav::Right), "src/in");
        assert!(ui.expanded.with_untracked(|e| e.contains("src/in")));
        assert_eq!(*asked.borrow(), [json!({"type": "ls", "path": "src/in"})]);
        // Left closes it, then goes to the folder it is in.
        assert_eq!(go(Nav::Left), "src/in");
        assert!(!ui.expanded.with_untracked(|e| e.contains("src/in")));
        assert_eq!(go(Nav::Left), "src");
        assert_eq!(go(Nav::Down), "src/in");
        assert_eq!(go(Nav::Down), "src/a.rs");
        assert_eq!(go(Nav::Left), "src", "a file: the folder it is in");
        // Enter opens a file in the editor, and a folder like a click.
        ui.selected.set("z.txt".into());
        asked.borrow_mut().clear();
        assert_eq!(navigate(ui, &send, Nav::Enter), Some(5));
        assert_eq!(*asked.borrow(), [json!({"type": "open", "path": "z.txt"})]);
        assert_eq!(go(Nav::Down), "z.txt", "no row after the last");
        ui.selected.set("src".into());
        navigate(ui, &send, Nav::Enter);
        assert!(!ui.expanded.with_untracked(|e| e.contains("src")));
        // A selection that is not among the rows (in a closed folder): Up starts from the last row.
        ui.selected.set("src/a.rs".into());
        assert_eq!(go(Nav::Up), "z.txt");
        assert_eq!(navigate(Ui::new(), &send, Nav::Down), None, "an empty tree");
    }

    #[test]
    fn a_new_file_goes_in_the_folder_selected_or_the_one_the_selected_file_is_in() {
        let ui = project();
        assert_eq!(folder_of_selected(ui), "");
        ui.selected.set("src".into());
        assert_eq!(folder_of_selected(ui), "src");
        ui.selected.set("src/a.rs".into());
        assert_eq!(folder_of_selected(ui), "src");
        ui.selected.set("z.txt".into());
        assert_eq!(folder_of_selected(ui), "");
    }
}
