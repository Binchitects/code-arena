//! How the window's parts are joined to it without editing the files they share. Every part (a file of this crate
//! that is a view, a dialog, a feature of the editor or of the chat) has two functions, and is named once in
//! `MODULES` below:
//!
//! ```ignore
//! /// What it adds to the window's registries, once at start.
//! pub fn register(ui: Ui, send: &Send) { … }
//! /// A message from the session; true to take it away from the parts after it.
//! pub fn take(ui: Ui, m: &Value, send: &Send) -> bool { … }
//! ```
//!
//! **`register`** runs once, before the window is drawn, after the workbench's own. It is where a part adds its
//! commands (`commands.rs`, VS Code's ids and default keys), its menu items (`menus.rs`), its status bar items
//! (`statusbar.rs`), its kind of editor tab (`editor::kinds`), what it draws around the chat's input and under an
//! answer (`agent::slots`), and installs what others call through it (`chat_bridge.rs`). Every registry takes a
//! second registration under the same id in the first's place, so a part may also change what another registered.
//!
//! **`take`** is given every message from the session, the parts in the order of `MODULES`, before the window's own
//! match in `Ui::take` (the session's state, its status, the theme). It returns **false** for nearly everything:
//! a part reads what it needs and leaves the message to the others (`state`, `status`, `tasks`, `turn_end` and the
//! like have several readers). It returns **true** only to take a message away from the parts after it: the reply
//! to its own request that another part would otherwise act on (a picture's `file`, which the text editor would
//! open as text). The parts the window began with are last in the list and never take anything away, so any part
//! above them can. Replies go to every window and page of the session and carry no request number: a part knows
//! its own reply by the key it echoes (the path, the id, the query), never by assuming the reply is its own.
//!
//! **A part's own state** lives in the part, not in `Ui`: a `Copy` struct of signals with `State::new`, got with
//! `ui.state::<Mine>()` (one per window, made the first time it is asked for). Its signals are made with
//! `ui.scope.create_rw_signal`, the window's long-lived scope, never with `create_rw_signal` (that one belongs to
//! whatever effect or view is running, and goes with it).
//!
//! **A view** is also named in `layout.rs` (its id, title, icon, keys, and its module's `READY`, `view` and
//! `actions`), see there. A view whose `READY` is false is nowhere in the window.
//!
//! A part never adds a field to `Ui`, an arm to `Ui::take`, a line to `lib.rs` or to the workbench: its file is
//! already declared and already in `MODULES`, with functions that do nothing until the part is written.

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
    module!(agent::pieces),
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
    use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
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
