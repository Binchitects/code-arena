//! The window's menus: the places a part of the window puts its commands, as VS Code's `contributes.menus`. A part
//! adds its items from its `register` (see `ext.rs`); whatever draws a menu asks for its rows:
//!
//! ```ignore
//! menus::add(Menu::TitleBar(menus::VIEW), menus::item("workbench.action.togglePanel").title("Panel").group("2_parts"));
//! let rows = menus::rows(ui, Menu::ExplorerRow);
//! ```
//!
//! The conventions:
//!
//! - An item names a command (`commands.rs`) and takes its title and its keys unless it gives its own. It is not in
//!   the menu while its own `when` or the command's is false, and is dimmed while the command may not run.
//! - Items of one `group` stay together, and a line is drawn between two groups. The groups come in the order of
//!   their names (so they are numbered: `1_open`, `2_save`), `navigation` before the others; within a group, by
//!   `order`, then as added.
//! - `menus::soon(title, keys)` is an item whose command is not there yet: drawn dimmed. A part that brings the
//!   command adds the item again under the same title, with the command, and it takes the first's place.
//! - In `Menu::ViewTitle(view)` an item with an `icon` is drawn as an action in the view's title, after the view's
//!   own actions; one without is for the title's "…" menu. The title's actions are read when the view comes to be
//!   the one its group shows: an item's `when` is asked then, and not followed after.
//! - The title bar's menus are drawn from here. The right-click menus are their parts' to draw (with the menu of
//!   `widgets::popup`): each asks for its rows when it opens. A command run from a row knows what it is about
//!   from the window's state (the Explorer's selected row, the tab shown), which the part sets before it opens
//!   its menu.
//!
//! An item added again (the same menu, the same command and title) takes the place of the first.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;

use crate::commands::{self, Test};
use crate::layout::View;
use crate::state::Ui;

/// The title bar's menus, by their place in it.
pub const FILE: usize = 0;
pub const EDIT: usize = 1;
pub const SELECTION: usize = 2;
pub const VIEW: usize = 3;
pub const GO: usize = 4;
pub const RUN: usize = 5;
pub const TERMINAL: usize = 6;
pub const HELP: usize = 7;

/// A place commands are put.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Menu {
    /// A menu of the title bar, by its place (`FILE` … `HELP`).
    TitleBar(usize),
    /// A right-click in an editor's text.
    EditorContext,
    /// A right-click on an editor's tab.
    EditorTab,
    /// A right-click on a row of the Explorer, and on its empty part.
    ExplorerRow,
    ExplorerEmpty,
    /// A right-click on a file the agent changed, and on a file of Source Control.
    ChangesRow,
    ScmRow,
    /// A right-click on a terminal's tab.
    TerminalTab,
    /// A right-click on a problem.
    ProblemsRow,
    /// The activity bar's Manage gear, and its Accounts button.
    Manage,
    Accounts,
    /// The Agent view's "…" menu.
    AgentMore,
    /// A view's title: its actions (the items with an icon) and its "…" menu.
    ViewTitle(View),
}

#[derive(Clone)]
pub struct Item {
    /// The command's id; none for an item not there yet.
    pub command: String,
    /// Its own title, in place of the command's.
    pub title: String,
    /// Its own keys, in place of the command's.
    pub keys: String,
    pub group: String,
    pub order: i32,
    /// A codicon, where the menu draws one.
    pub icon: &'static str,
    pub when: Option<Test>,
}

/// An item running a command.
pub fn item(command: &str) -> Item {
    Item {
        command: command.into(),
        title: String::new(),
        keys: String::new(),
        group: String::new(),
        order: 0,
        icon: "",
        when: None,
    }
}

/// An item whose command is not there yet: its title and its keys, dimmed.
pub fn soon(title: &str, keys: &str) -> Item {
    item("").title(title).shown_keys(keys)
}

impl Item {
    pub fn title(mut self, title: &str) -> Item {
        self.title = title.into();
        self
    }

    /// The keys shown, when they are not the command's first.
    pub fn shown_keys(mut self, keys: &str) -> Item {
        self.keys = keys.into();
        self
    }

    pub fn group(mut self, group: &str) -> Item {
        self.group = group.into();
        self
    }

    pub fn order(mut self, order: i32) -> Item {
        self.order = order;
        self
    }

    pub fn icon(mut self, icon: &'static str) -> Item {
        self.icon = icon;
        self
    }

    pub fn when(mut self, test: impl Fn(Ui) -> bool + 'static) -> Item {
        self.when = Some(std::rc::Rc::new(test));
        self
    }
}

thread_local! {
    static MENUS: RefCell<HashMap<Menu, Vec<Item>>> = RefCell::new(HashMap::new());
}

/// Adds an item to a menu. One of the same title (or, with no title of its own, the same command) takes the
/// first's place.
pub fn add(menu: Menu, item: Item) {
    MENUS.with(|m| {
        let mut m = m.borrow_mut();
        let items = m.entry(menu).or_default();
        let same = |old: &Item| {
            if item.title.is_empty() {
                old.title.is_empty() && old.command == item.command
            } else {
                old.title == item.title
            }
        };
        match items.iter().position(same) {
            Some(at) => items[at] = item,
            None => items.push(item),
        }
    });
}

/// A row of a menu as it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// The line between two groups.
    Line,
    Entry {
        title: String,
        keys: String,
        icon: &'static str,
        /// The command a click runs; none while it may not run (drawn dimmed).
        command: Option<String>,
    },
}

/// Where a group comes among the groups: `navigation` first, then by name.
fn group_order(a: &str, b: &str) -> std::cmp::Ordering {
    (a != "navigation", a).cmp(&(b != "navigation", b))
}

/// A menu's rows now: the items that are there, group after group with a line between.
pub fn rows(ui: Ui, menu: Menu) -> Vec<Row> {
    let mut items: Vec<Item> = MENUS.with(|m| m.borrow().get(&menu).cloned().unwrap_or_default());
    // A stable sort: as added, within a group and an order.
    items.sort_by(|a, b| group_order(&a.group, &b.group).then(a.order.cmp(&b.order)));
    let mut out = Vec::new();
    let mut last: Option<String> = None;
    for item in items {
        if item.when.as_ref().is_some_and(|w| !w(ui)) {
            continue;
        }
        let command = if item.command.is_empty() { None } else { commands::find(&item.command) };
        let (title, keys, run) = match &command {
            Some(c) if !c.offered(ui) => continue,
            Some(c) => (
                if item.title.is_empty() { c.title.clone() } else { item.title.clone() },
                if item.keys.is_empty() { c.shown_keys().to_string() } else { item.keys.clone() },
                c.may_run(ui).then(|| c.id.clone()),
            ),
            // No such command (yet): shown dimmed when it has a title of its own, else left out.
            None if item.title.is_empty() => continue,
            None => (item.title.clone(), item.keys.clone(), None),
        };
        if last.as_ref().is_some_and(|g| *g != item.group) {
            out.push(Row::Line);
        }
        last = Some(item.group.clone());
        out.push(Row::Entry { title, keys, icon: item.icon, command: run });
    }
    out
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use floem::reactive::{SignalGet, SignalUpdate};

    use super::*;
    use crate::commands::command;
    use crate::state::Send;

    fn titles(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Line => "-".to_string(),
                Row::Entry { title, keys, command, .. } => {
                    format!("{title}{}{}", if keys.is_empty() { "" } else { " " }, keys)
                        + if command.is_some() { "" } else { " (dimmed)" }
                }
            })
            .collect()
    }

    #[test]
    fn a_menus_rows_are_its_groups_in_order_with_a_line_between() {
        let ui = Ui::new();
        commands::add(command("test.menu.open", "Test", "Open", |_, _| {}).keys("Ctrl+Alt+O"));
        commands::add(command("test.menu.save", "Test", "Save", |_, _| {}));
        commands::add(command("test.menu.busy", "Test", "Only While It Works", |_, _| {}).when(|ui| ui.running.get()));
        commands::add(command("test.menu.off", "Test", "Off", |_, _| {}).enabled(|_| false));
        let menu = Menu::EditorTab;
        add(menu, item("test.menu.save").group("2_save"));
        add(menu, item("test.menu.open").group("1_open").order(2));
        add(menu, item("test.menu.open").title("Open First").shown_keys("F9").group("1_open").order(1));
        add(menu, item("test.menu.busy").group("1_open").order(3));
        add(menu, item("test.menu.off").group("3_more"));
        add(menu, soon("Undo", "Ctrl+Z").group("3_more"));
        add(menu, item("test.menu.nowhere").group("3_more"));
        add(menu, item("test.menu.save").title("Pin").group("navigation").icon("pinned"));
        assert_eq!(
            titles(&rows(ui, menu)),
            ["Pin", "-", "Open First F9", "Open Ctrl+Alt+O", "-", "Save", "-", "Off (dimmed)", "Undo Ctrl+Z (dimmed)"]
        );
        ui.running.set(true);
        assert!(titles(&rows(ui, menu)).contains(&"Only While It Works".to_string()));
        assert!(rows(ui, Menu::ScmRow).is_empty());
        // Added again: in the first's place. An item not there yet gets its command so.
        add(menu, item("test.menu.save").title("Undo").group("3_more"));
        let now = titles(&rows(ui, menu));
        assert_eq!(now.last().map(String::as_str), Some("Undo"));
        assert_eq!(now.iter().filter(|t| t.starts_with("Undo")).count(), 1);
        // The row's command is the one a click runs.
        let send: Send = Rc::new(|_| {});
        let Some(Row::Entry { command: Some(id), .. }) = rows(ui, menu).into_iter().next() else { panic!("a row") };
        assert!(commands::run(ui, &send, &id));
    }
}
