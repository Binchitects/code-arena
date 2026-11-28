//! How the window's parts are joined to it without editing the files they share. This is the contract every part
//! follows; each registry's own file says the rest.
//!
//! # A part
//!
//! A part is a file of this crate that is a view, a dialog, a feature of the editor or of the chat. It has two
//! functions and is named once in `MODULES` below:
//!
//! ```ignore
//! /// What it adds to the window's registries, once at start.
//! pub fn register(ui: Ui, send: &Send) { … }
//! /// A message from the session; true to take it away from the parts after it.
//! pub fn take(ui: Ui, m: &Value, send: &Send) -> bool { … }
//! ```
//!
//! Every file the plan names is there already, declared (in `lib.rs`, or in the file whose child it is) and listed
//! in `MODULES`, with functions that do nothing until its part is written. So a part writes its own files and no
//! other: it never adds a field to `Ui`, an arm to `Ui::take`, a line to `lib.rs`, to the workbench or to this file.
//!
//! # `register`: what a part adds, and where
//!
//! `register` runs once, before the window is drawn: the workbench's first (its commands begin the palette's
//! list), then each part's in the order of `MODULES`. What is drawn from a registry is built once, from what is in
//! it then: add from `register`, not later. The registries:
//!
//! - `commands.rs`: its commands, under VS Code's ids, with their default keys, `when` and `enabled`. The palette,
//!   the menus and the keys all run a command by its id.
//! - `menus.rs`: where its commands are put: the title bar's menus, a view's title, the right-click menus.
//! - `statusbar.rs`: its items in the status bar, by side and priority.
//! - `badges.rs`: the count on a view's icon in the activity bar.
//! - `layers.rs`: what it draws over the whole window (a dialog, the toasts, an overlay) and its banner under the
//!   title bar.
//! - `editor::kinds`: its kind of editor tab that is not a file's text (a picture, a preview, the settings).
//! - `agent::slots`: what it draws around the chat's input and with a request or an answer in the thread.
//! - `chat_bridge.rs`: what the rest of the window asks of the chat's input (text put in its box, a question asked
//!   at once); the composer installs how.
//! - `keys_hook.rs`: which keys pressed in the terminal or in an editor are the workbench's.
//! - `ext::after`: what is called with every message once the window has taken it in (below).
//!
//! Every registry takes a second registration under the same id in the first's place, so a part may change what
//! another registered (its command, its menu item, its status bar item) without editing its file.
//!
//! # `take`: the session's messages
//!
//! `take` is given every message from the session, the parts in the order of `MODULES`, before the window's own
//! match in `Ui::take` (the session's state, its status, the theme). It returns **false** for nearly everything:
//! a part reads what it needs and leaves the message to the others (`state`, `status`, `tasks`, `turn_end` and the
//! like have several readers). It returns **true** only to take a message away from the parts after it and from
//! the window: the reply to its own request that another part would otherwise act on (a picture's `file`, which
//! the text editor would open as text). The parts to come are first in the list, in the order of their names; the
//! parts the window began with are last and never take anything away, so any part above them can.
//!
//! Replies go to every window and page of the session and carry no request number: a part knows its own reply by
//! the key it echoes (the path, the id, the query), never by assuming the reply is its own.
//!
//! Each message is taken once. What a `take` reads is not followed (the window takes a message in with following
//! off), so a signal read there, with `get` or with `get_untracked`, never has the message taken again when it is
//! set.
//!
//! A part that must read what the others made of a message (the thread once `state` is taken in, an answer once
//! its `turn_end` is) asks to be called after them, from its `register`: `ext::after("mine", |ui, m, send| …)`.
//!
//! # A part's own state
//!
//! It lives in the part, not in `Ui`: a `Copy` struct of signals with `State::new`, got with `ui.state::<Mine>()`
//! (one per window, made the first time it is asked for). Its signals are made with `ui.scope.create_rw_signal`
//! (and `ui.scope.create_memo`), the window's long-lived scope, never with `create_rw_signal`: that one belongs to
//! whatever effect or view is running, and goes with it. A signal is set only when its value changes (a
//! `dyn_container` builds its child again each time a signal it reads is set, even to what it held): what a
//! container is built from goes behind a memo.
//!
//! # A view
//!
//! A view is also named in `layout.rs`: its id, title, icon and keys, and its module's `READY`, `view` and
//! `actions`. A view whose `READY` is false is nowhere in the window; its part turns it to true in its own file,
//! and the view is then in its first place (`dock::HOMES`), with its "Show" and "Float" commands and its row in the
//! View menu. In this crate's tests the views in the window are the six it began with whatever `READY` says, so the
//! tests of the layout do not change when a view is written: a test of a newer view says
//! `layout::also_ready(&[View::Mine])` first.
//!
//! # The thread's items
//!
//! `model.rs` has every field a request, an answer, a call and a queued message are to carry, each read from the
//! session's JSON by a `from_json` that takes what is there. A part fills and draws its own fields; none changes an
//! item's shape.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use floem::reactive::SignalWith;
use serde_json::Value;

use crate::state::{Send, Ui};

/// A part of the window: what it registers at start, and how it takes the session's messages.
pub struct Module {
    pub name: &'static str,
    pub register: fn(Ui, &Send),
    pub take: fn(Ui, &Value, &Send) -> bool,
}

macro_rules! module {
    ($($path:ident)::+) => {
        Module {
            name: stringify!($($path)::+),
            register: crate::$($path)::+::register,
            take: crate::$($path)::+::take,
        }
    };
}

/// Every part, in the order each is given a message: the parts added to the window first, then the parts it began
/// with (which never take a message away).
pub const MODULES: &[Module] = &[
    // The parts to come: each file is there already, with functions that do nothing until its part is written.
    module!(accessibility),
    module!(accounts),
    module!(agent::actions),
    module!(agent::attach),
    module!(agent::compaction),
    module!(agent::diffs),
    module!(agent::media),
    module!(agent::mentions),
    module!(agent::quotes),
    module!(agent::rating),
    module!(agent::ring),
    module!(agent::slash),
    module!(agent::tools_picker),
    module!(assistants),
    module!(automation),
    module!(banners),
    module!(bidi),
    module!(checklists),
    module!(citations),
    module!(compare),
    module!(compare::align),
    module!(compare::ranges),
    module!(compare::toolbar),
    module!(complete),
    module!(context_panel),
    module!(debug),
    module!(debug::editor),
    module!(debug::session),
    module!(debug::toolbar),
    module!(debug_console),
    module!(decor),
    module!(diagrams),
    module!(editor::editing),
    // After the editor's commands: its Ctrl+L (Add to chat) takes Expand Line Selection's place.
    module!(agent::pieces),
    module!(editor::folding),
    module!(editor::groups),
    module!(editor::highlights),
    module!(editor::minimap),
    module!(editor::nav),
    module!(editor::options),
    module!(editor::window),
    module!(explorer_decor),
    module!(extensions_apply),
    module!(extensions_view),
    module!(format),
    module!(harness),
    module!(help),
    module!(help_text),
    module!(indent),
    module!(inline_complete),
    module!(keymap),
    module!(keymap::defaults),
    module!(keymap::editor),
    module!(keymap::when),
    module!(language),
    module!(layout_commands),
    module!(loops),
    module!(lsp),
    module!(lsp::actions),
    module!(lsp::client),
    module!(lsp::features),
    module!(lsp::navigate),
    module!(lsp::status),
    module!(lsp::sync),
    module!(manage),
    module!(mcp),
    module!(md_preview),
    module!(media),
    module!(memory_view),
    module!(merge_editor),
    module!(merge_inline),
    module!(notebook),
    module!(notebook::model),
    module!(notebook::outputs),
    module!(notify),
    module!(open_folder),
    module!(output),
    module!(peek),
    module!(plan_view),
    module!(ports),
    module!(problems),
    module!(project_tab),
    module!(projects_view),
    module!(providers),
    module!(quick_modes),
    module!(review),
    module!(run_debug),
    module!(scm),
    module!(scm::branches),
    module!(scm::history),
    module!(scm::quick_diff),
    module!(scm::repos),
    module!(scm::tree),
    module!(sessions),
    module!(sessions_arena),
    module!(settings),
    module!(settings::editor),
    module!(skills_view),
    module!(snippets),
    module!(status_items),
    module!(tasks_panel),
    module!(tasks_run),
    module!(theme_menu),
    module!(tools_dialog),
    module!(trust),
    module!(update),
    module!(vm_login),
    module!(widgets::dialog),
    module!(widgets::popup),
    module!(widgets::quick),
    module!(widgets::toast),
    module!(widgets::tree),
    module!(workspaces),
    // The parts the window began with. They never take a message away.
    module!(activity_bar),
    module!(agent::header),
    module!(agent::question),
    module!(agent::queue),
    module!(agent::answer),
    module!(agent::tools),
    module!(agent::thread),
    module!(agent::ask),
    module!(agent::plan_bar),
    module!(agent::goal_bar),
    module!(agent::jobs_bar),
    module!(agent::composer),
    module!(agent::pickers),
    module!(agent::stream),
    module!(save),
    module!(editor::tabs),
    module!(editor::breadcrumbs),
    module!(editor::find),
    module!(editor::keys),
    module!(editor::watermark),
    module!(editor::disk),
    module!(editor::kinds),
    module!(explorer),
    module!(search),
    module!(changes),
    module!(outline),
    module!(terminal),
    module!(palette),
    module!(markdown),
    module!(prose),
    module!(highlight),
];

/// Every part's `register`, once, before the window is drawn: the workbench's first (its commands begin the
/// palette's list), then each part's in the order of `MODULES`.
pub fn register(ui: Ui, send: &Send) {
    crate::workbench::register(ui, send);
    for m in MODULES {
        (m.register)(ui, send);
    }
}

/// One message from the session given to each part in turn; true when one took it away.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    MODULES.iter().any(|part| (part.take)(ui, m, send))
}

/// What is called with a message once every part and the window have taken it in.
type After = Rc<dyn Fn(Ui, &Value, &Send)>;

thread_local! {
    static AFTER: RefCell<Vec<(&'static str, After)>> = const { RefCell::new(Vec::new()) };
}

/// Adds what is called with every message of the session once every part and the window itself have taken it in
/// (the thread is made, the session's state is set): for a part that reads what others made of a message. Not
/// called for a message a part took away. One added again under the same id takes the first's place.
pub fn after(id: &'static str, then: impl Fn(Ui, &Value, &Send) + 'static) {
    AFTER.with(|a| {
        let mut all = a.borrow_mut();
        match all.iter().position(|(old, _)| *old == id) {
            Some(at) => all[at].1 = Rc::new(then),
            None => all.push((id, Rc::new(then))),
        }
    });
}

/// A message was taken in by every part and by the window: those that asked to be called after are.
pub fn taken(ui: Ui, m: &Value, send: &Send) {
    // Taken out first: what is called may add another.
    let all: Vec<After> = AFTER.with(|a| a.borrow().iter().map(|(_, then)| then.clone()).collect());
    for then in all {
        then(ui, m, send);
    }
}

/// A part's own state: made once for a window, the first time it is asked for.
pub trait State: Copy + 'static {
    fn new(ui: Ui) -> Self;
}

/// Where a window keeps its parts' states, each by its type.
#[derive(Clone, Default)]
pub struct Store(Rc<RefCell<HashMap<TypeId, Box<dyn Any>>>>);

impl Ui {
    /// A part's state for this window.
    pub fn state<T: State>(&self) -> T {
        let store = self.ext.with_untracked(Store::clone);
        if let Some(found) = store.0.borrow().get(&TypeId::of::<T>()).and_then(|s| s.downcast_ref::<T>()) {
            return *found;
        }
        // Made with the store free: making one state may ask for another.
        let made = T::new(*self);
        store.0.borrow_mut().insert(TypeId::of::<T>(), Box::new(made));
        made
    }
}

#[cfg(test)]
mod tests {
    use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
    use serde_json::json;

    use super::*;

    #[derive(Clone, Copy)]
    struct Counter(RwSignal<u32>);

    impl State for Counter {
        fn new(ui: Ui) -> Counter {
            Counter(ui.scope.create_rw_signal(0))
        }
    }

    #[derive(Clone, Copy)]
    struct Outer(Counter);

    impl State for Outer {
        fn new(ui: Ui) -> Outer {
            Outer(ui.state::<Counter>())
        }
    }

    #[test]
    fn a_parts_state_is_made_once_for_a_window_and_each_window_has_its_own() {
        let ui = Ui::new();
        ui.state::<Counter>().0.set(4);
        assert_eq!(ui.state::<Counter>().0.get_untracked(), 4);
        // One made while another is being made is the same one.
        assert_eq!(ui.state::<Outer>().0.0.get_untracked(), 4);
        assert_eq!(Ui::new().state::<Counter>().0.get_untracked(), 0);
    }

    /// The titles of a menu's rows.
    fn menu_titles(ui: Ui, menu: crate::menus::Menu) -> Vec<String> {
        crate::menus::rows(ui, menu)
            .into_iter()
            .filter_map(|r| match r {
                crate::menus::Row::Entry { title, .. } => Some(title),
                crate::menus::Row::Line => None,
            })
            .collect()
    }

    fn palette_labels(ui: Ui) -> Vec<String> {
        crate::palette::commands(ui).iter().map(|c| c.label()).collect()
    }

    #[test]
    fn a_view_that_is_not_written_is_nowhere_in_the_window() {
        use crate::layout::{VIEWS, View};
        // As the window itself: the views in it are the ones whose modules say they are written.
        crate::layout::ready_as_written();
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        register(ui, &send);
        let dock = ui.dock.get_untracked();
        let labels = palette_labels(ui);
        let rows = menu_titles(ui, crate::menus::Menu::TitleBar(crate::menus::VIEW));
        let mut not_written = 0;
        for v in VIEWS {
            let title = if v == View::Agent { "Chat" } else { v.title() };
            let floats = labels.contains(&format!("View: Float {}", v.title()));
            let in_dock = dock.group_of(v).is_some();
            if v.written() {
                assert!(ui.available(v) && in_dock && floats && rows.iter().any(|r| r == title), "{v:?}");
                continue;
            }
            not_written += 1;
            assert!(!ui.available(v), "{v:?}");
            assert!(!in_dock && !dock.floating.contains(&v), "{v:?} is in the dock");
            assert!(!ui.drawn().with_untracked(|d| d.group_of(v).is_some()), "{v:?} is drawn");
            assert!(!crate::activity_bar::listed(ui).contains(&v), "{v:?} is in the activity bar");
            assert!(!floats && !labels.contains(&format!("View: Show {}", v.title())), "{v:?} is in the palette");
            assert!(!rows.iter().any(|r| r == title), "{v:?} is in the View menu");
            assert!(!crate::commands::run(ui, &send, v.command()), "{v:?}: its command runs");
            // Its key does nothing.
            if let Some(keys) = crate::commands::parse_keys(v.keys())
                && let [key] = keys.as_slice()
            {
                let mut held = floem::keyboard::Modifiers::empty();
                held.set(floem::keyboard::Modifiers::CONTROL, key.ctrl);
                held.set(floem::keyboard::Modifiers::SHIFT, key.shift);
                held.set(floem::keyboard::Modifiers::ALT, key.alt);
                let pressed = floem::keyboard::Key::Character(key.key.as_str().into());
                assert_eq!(crate::commands::press(ui, &send, &pressed, held), crate::commands::Pressed::Free, "{v:?}");
            }
            // Revealed by name all the same (a layout kept by a later Code Arena, a part that asks): nothing.
            ui.reveal(v);
            ui.toggle_view(v);
            assert_eq!(ui.dock.get_untracked(), dock, "{v:?}");
        }
        // A layout kept with views that are not written (by a later Code Arena) is read without them.
        let all = crate::dock::Dock::of(&VIEWS).to_json();
        let read = crate::dock::Dock::from_json(&all);
        assert!(VIEWS.into_iter().all(|v| read.group_of(v).is_some() == v.written()));
        assert!(not_written == VIEWS.into_iter().filter(|v| !v.written()).count());
        crate::layout::also_ready(&[]);
    }

    #[test]
    fn a_view_written_since_is_in_its_place_with_its_command_its_key_and_its_menu_row() {
        use crate::layout::View::{Output, Problems, Scm};
        crate::layout::also_ready(&[Scm, Problems, Output]);
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        register(ui, &send);
        let labels = palette_labels(ui);
        for want in
            ["View: Show Source Control", "View: Float Source Control", "View: Show Problems", "View: Show Output"]
        {
            assert!(labels.iter().any(|l| l == want), "{want}");
        }
        assert!(!labels.iter().any(|l| l == "View: Show Memory"));
        let rows = crate::menus::rows(ui, crate::menus::Menu::TitleBar(crate::menus::VIEW));
        assert!(rows.iter().any(|r| matches!(r, crate::menus::Row::Entry { title, keys, command: Some(_), .. }
            if title == "Source Control" && keys == "Ctrl+Shift+G")));
        // Its place: the side bar, after the views that come before it; shown by its key.
        assert_eq!(ui.dock.with_untracked(|d| d.group_of(Scm)), ui.dock.with_untracked(|d| d.side_bar()));
        assert!(!ui.view_shown(Scm));
        let g = floem::keyboard::Key::Character("G".into());
        let held = floem::keyboard::Modifiers::CONTROL | floem::keyboard::Modifiers::SHIFT;
        assert_eq!(crate::commands::press(ui, &send, &g, held), crate::commands::Pressed::Ran);
        assert!(ui.view_shown(Scm));
        // Withheld while the window runs: drawn nowhere, its commands and its key gone, its place kept.
        let kept = ui.dock.get_untracked();
        ui.withhold(Scm, true);
        assert!(!ui.available(Scm) && !ui.view_shown(Scm));
        assert!(ui.drawn().with_untracked(|d| d.group_of(Scm).is_none()));
        assert!(!crate::activity_bar::listed(ui).contains(&Scm));
        assert!(!palette_labels(ui).iter().any(|l| l.ends_with("Source Control")));
        assert!(
            !menu_titles(ui, crate::menus::Menu::TitleBar(crate::menus::VIEW)).iter().any(|r| r == "Source Control")
        );
        assert_eq!(crate::commands::press(ui, &send, &g, held), crate::commands::Pressed::Free);
        assert_eq!(ui.dock.get_untracked(), kept, "the layout kept for the folder still has it");
        assert_eq!(ui.view_state()["dock"], kept.to_json());
        // Another view of its group is the one drawn meanwhile, and the others' places are as they were.
        assert!(ui.drawn().with_untracked(|d| d.side_bar() == kept.side_bar()));
        // Given back: where it was, shown as it was.
        ui.withhold(Scm, false);
        assert!(ui.available(Scm) && ui.view_shown(Scm));
        assert_eq!(ui.drawn().get_untracked(), kept);
        crate::layout::also_ready(&[]);
    }

    #[test]
    fn a_message_that_comes_in_is_taken_once_whatever_taking_it_reads() {
        use floem::reactive::create_rw_signal;

        use crate::layout::View;
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let incoming = create_rw_signal(None::<Value>);
        crate::take_incoming(ui, send, incoming);
        // A part that reads a signal while it takes a message in, as one that asks whether a view is there does.
        after("test.reads", |ui, m, _| {
            if m["type"] == "test-read" && ui.available(View::Files) {
                ui.message.update(|t| t.push('.'));
            }
        });
        incoming.set(Some(json!({"type": "test-read"})));
        assert_eq!(ui.message.get_untracked(), ".");
        // The signal it read is set: the message is not taken again.
        ui.withhold(View::Outline, true);
        assert_eq!(ui.message.get_untracked(), ".");
        // The same message said again by the session is another message.
        incoming.set(Some(json!({"type": "test-read"})));
        assert_eq!(ui.message.get_untracked(), "..");
        after("test.reads", |_, _, _| {});
    }

    #[test]
    fn every_part_is_named_once() {
        let mut names: Vec<&str> = MODULES.iter().map(|m| m.name).collect();
        let all = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all);
    }

    #[test]
    fn a_message_no_part_takes_away_reaches_the_window_and_registering_twice_adds_nothing_twice() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        assert!(!take(ui, &json!({"type": "status", "model": "m", "mode": "ask"}), &send));
        assert!(!take(ui, &json!({"type": "no-such-message"}), &send));
        // What asked to be called after is, once the window has taken the message in.
        after("test.after", |ui, m, _| {
            if m["type"] == "status" {
                ui.message.set(format!("the model is {}", ui.model.get_untracked()));
            }
        });
        ui.take(&json!({"type": "status", "model": "m2", "mode": "ask"}), &send);
        assert_eq!(ui.message.get_untracked(), "the model is m2");
        register(ui, &send);
        let commands = crate::commands::all().len();
        let rows = crate::menus::rows(ui, crate::menus::Menu::TitleBar(crate::menus::VIEW)).len();
        let items = crate::statusbar::items(crate::statusbar::Side::Left).len();
        register(ui, &send);
        assert_eq!(crate::commands::all().len(), commands);
        assert_eq!(crate::menus::rows(ui, crate::menus::Menu::TitleBar(crate::menus::VIEW)).len(), rows);
        assert_eq!(crate::statusbar::items(crate::statusbar::Side::Left).len(), items);
    }
}
