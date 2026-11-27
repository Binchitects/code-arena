//! The window's views: what each is called, its icon, its keys, and the module that draws it. Where they are is the
//! dock tree's (`dock.rs`).
//!
//! Every view the window is to have is named here, each with a module of its own that has:
//!
//! ```ignore
//! /// Whether the view is in the window. False until the view is written.
//! pub const READY: bool = false;
//! /// The view's own content.
//! pub fn view(ui: Ui, send: Send) -> floem::AnyView { … }
//! /// The actions in its title (none: it has no action of its own).
//! pub fn actions(ui: Ui, send: Send) -> Option<floem::AnyView> { … }
//! ```
//!
//! (and the `register` and `take` every part has, see `ext.rs`). A view whose `READY` is false is nowhere: not in
//! the dock, not in the activity bar, not in a menu, not in the palette, and its keys do nothing. The part that
//! writes a view turns its `READY` to true in its own file, and that is all it takes: the view is then in its first
//! place (`dock::HOMES`), also in a layout that was kept before it was there, with its "Show" command (`command`,
//! under `keys`), its row in the View menu and its "Float" command.
//!
//! A view that is ready may still be withheld while the window runs (`ui.withhold(view, true)`: Memory, while the
//! account may not use it): it is then not drawn anywhere, its commands are not there, and it keeps its place in
//! the layout (the one kept for the folder too) for when it comes back. `ui.available(view)` says whether a view is
//! in the window now: ready and not withheld.
//!
//! In this crate's tests the views in the window are the six it began with, whatever the modules' `READY` say, so
//! a test of the layout, the palette or the menus does not change the day a view is written. A test of a view that
//! was written since says so first: `layout::also_ready(&[View::Scm])` (for that test's thread). `View::written`
//! is the module's own word, in tests too.

use floem::{AnyView, IntoView};

use crate::state::{Send, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Files,
    Search,
    Scm,
    Run,
    Extensions,
    Changes,
    Problems,
    Review,
    Plan,
    Memory,
    Skills,
    Projects,
    Automation,
    Sessions,
    Harness,
    Outline,
    Agent,
    Tasks,
    Context,
    Terminal,
    DebugConsole,
    Output,
    Ports,
}

/// Every view, the ready ones and the ones to come, in the order the View menu and the palette list them.
pub const VIEWS: [View; 23] = [
    View::Files,
    View::Search,
    View::Scm,
    View::Run,
    View::Extensions,
    View::Changes,
    View::Problems,
    View::Review,
    View::Plan,
    View::Memory,
    View::Skills,
    View::Projects,
    View::Automation,
    View::Sessions,
    View::Harness,
    View::Outline,
    View::Agent,
    View::Tasks,
    View::Context,
    View::Terminal,
    View::DebugConsole,
    View::Output,
    View::Ports,
];

/// The views the window began with: always in it.
// The parts to come ask; the window itself asks each view's module.
#[allow(dead_code)]
pub const FIRST: [View; 6] = [View::Files, View::Search, View::Changes, View::Outline, View::Agent, View::Terminal];

#[cfg(test)]
thread_local! {
    /// The views taken as ready in a test, beside the six the window began with.
    static ALSO_READY: std::cell::RefCell<Vec<View>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// In a test: these views are ready too, as when their parts are written (for this test's thread), beside the six
/// the window began with; an empty list: those six only.
#[cfg(test)]
pub fn also_ready(views: &[View]) {
    ALSO_READY.with(|a| *a.borrow_mut() = views.to_vec());
}

/// In a test: the views ready are the ones whose modules say so, as in the window itself.
#[cfg(test)]
pub fn ready_as_written() {
    also_ready(&VIEWS.into_iter().filter(|v| v.written()).collect::<Vec<_>>());
}

/// The views that are in the window: those whose module says it is ready.
pub fn ready() -> Vec<View> {
    VIEWS.into_iter().filter(|v| v.ready()).collect()
}

impl View {
    /// Its name in a kept layout: never changed once a layout may have been kept with it.
    pub fn id(self) -> &'static str {
        match self {
            View::Files => "files",
            View::Search => "search",
            View::Scm => "scm",
            View::Run => "run",
            View::Extensions => "extensions",
            View::Changes => "changes",
            View::Problems => "problems",
            View::Review => "review",
            View::Plan => "plan",
            View::Memory => "memory",
            View::Skills => "skills",
            View::Projects => "projects",
            View::Automation => "automation",
            View::Sessions => "sessions",
            View::Harness => "harness",
            View::Outline => "outline",
            View::Agent => "agent",
            View::Tasks => "tasks",
            View::Context => "context",
            View::Terminal => "terminal",
            View::DebugConsole => "debug-console",
            View::Output => "output",
            View::Ports => "ports",
        }
    }

    pub fn from_id(id: &str) -> Option<View> {
        VIEWS.into_iter().find(|v| v.id() == id)
    }

    /// Whether the view is in the window: its module's `READY` (in a test: one of the six the window began with,
    /// or one the test said is ready too).
    pub fn ready(self) -> bool {
        #[cfg(test)]
        return FIRST.contains(&self) || ALSO_READY.with(|a| a.borrow().contains(&self));
        #[cfg(not(test))]
        self.written()
    }

    /// Whether the view is written: its module's `READY`.
    pub fn written(self) -> bool {
        match self {
            View::Files | View::Search | View::Changes | View::Outline | View::Agent | View::Terminal => true,
            View::Scm => crate::scm::READY,
            View::Run => crate::run_debug::READY,
            View::Extensions => crate::extensions_view::READY,
            View::Problems => crate::problems::READY,
            View::Review => crate::review::READY,
            View::Plan => crate::plan_view::READY,
            View::Memory => crate::memory_view::READY,
            View::Skills => crate::skills_view::READY,
            View::Projects => crate::projects_view::READY,
            View::Automation => crate::automation::READY,
            View::Sessions => crate::sessions::READY,
            View::Harness => crate::harness::READY,
            View::Tasks => crate::tasks_panel::READY,
            View::Context => crate::context_panel::READY,
            View::DebugConsole => crate::debug_console::READY,
            View::Output => crate::output::READY,
            View::Ports => crate::ports::READY,
        }
    }

    /// The view's own content, from its module.
    pub fn body(self, ui: Ui, send: Send) -> AnyView {
        match self {
            View::Files => crate::explorer::view(ui, send).into_any(),
            View::Search => crate::search::view(ui, send).into_any(),
            View::Changes => crate::changes::view(ui, send).into_any(),
            View::Outline => crate::outline::view(ui, send).into_any(),
            View::Agent => crate::agent::view(ui, send).into_any(),
            View::Terminal => crate::terminal::view(ui, send).into_any(),
            View::Scm => crate::scm::view(ui, send),
            View::Run => crate::run_debug::view(ui, send),
            View::Extensions => crate::extensions_view::view(ui, send),
            View::Problems => crate::problems::view(ui, send),
            View::Review => crate::review::view(ui, send),
            View::Plan => crate::plan_view::view(ui, send),
            View::Memory => crate::memory_view::view(ui, send),
            View::Skills => crate::skills_view::view(ui, send),
            View::Projects => crate::projects_view::view(ui, send),
            View::Automation => crate::automation::view(ui, send),
            View::Sessions => crate::sessions::view(ui, send),
            View::Harness => crate::harness::view(ui, send),
            View::Tasks => crate::tasks_panel::view(ui, send),
            View::Context => crate::context_panel::view(ui, send),
            View::DebugConsole => crate::debug_console::view(ui, send),
            View::Output => crate::output::view(ui, send),
            View::Ports => crate::ports::view(ui, send),
        }
    }

    /// The actions in its title, from its module (none: it has no action of its own). The actions other parts add
    /// to a view's title are in `menus::Menu::ViewTitle`.
    pub fn actions(self, ui: Ui, send: Send) -> Option<AnyView> {
        match self {
            View::Files => Some(crate::explorer::actions(ui, send).into_any()),
            View::Terminal => Some(crate::terminal::actions(ui, send).into_any()),
            View::Agent => Some(crate::agent::header::view(ui, send).into_any()),
            View::Changes => Some(crate::changes::actions(ui, send).into_any()),
            View::Search | View::Outline => None,
            View::Scm => crate::scm::actions(ui, send),
            View::Run => crate::run_debug::actions(ui, send),
            View::Extensions => crate::extensions_view::actions(ui, send),
            View::Problems => crate::problems::actions(ui, send),
            View::Review => crate::review::actions(ui, send),
            View::Plan => crate::plan_view::actions(ui, send),
            View::Memory => crate::memory_view::actions(ui, send),
            View::Skills => crate::skills_view::actions(ui, send),
            View::Projects => crate::projects_view::actions(ui, send),
            View::Automation => crate::automation::actions(ui, send),
            View::Sessions => crate::sessions::actions(ui, send),
            View::Harness => crate::harness::actions(ui, send),
            View::Tasks => crate::tasks_panel::actions(ui, send),
            View::Context => crate::context_panel::actions(ui, send),
            View::DebugConsole => crate::debug_console::actions(ui, send),
            View::Output => crate::output::actions(ui, send),
            View::Ports => crate::ports::actions(ui, send),
        }
    }

    /// Its title as VS Code writes a view container's (the side bar's title shows it in capitals).
    pub fn title(self) -> &'static str {
        match self {
            View::Files => "Explorer",
            View::Search => "Search",
            View::Scm => "Source Control",
            View::Run => "Run and Debug",
            View::Extensions => "Extensions",
            View::Changes => "Changes",
            View::Problems => "Problems",
            View::Review => "Review",
            View::Plan => "Plan",
            View::Memory => "Memory",
            View::Skills => "Skills",
            View::Projects => "Projects",
            View::Automation => "Automation",
            View::Sessions => "Sessions",
            View::Harness => "Harness",
            View::Outline => "Outline",
            View::Agent => "Agent",
            View::Tasks => "Tasks",
            View::Context => "Context",
            View::Terminal => "Terminal",
            View::DebugConsole => "Debug Console",
            View::Output => "Output",
            View::Ports => "Ports",
        }
    }

    /// Its codicon.
    pub fn icon(self) -> &'static str {
        match self {
            View::Files => "files",
            View::Search => "search",
            View::Scm => "source-control",
            View::Run => "debug-alt",
            View::Extensions => "extensions",
            // The agent's changes had VS Code's Source Control icon before there was a Source Control view; beside
            // one, they have the old Code Arena's (two lines compared).
            View::Changes if View::Scm.ready() => "git-compare",
            View::Changes => "source-control",
            View::Problems => "warning",
            View::Review => "code-review",
            View::Plan => "checklist",
            View::Memory => "lightbulb",
            View::Skills => "wand",
            View::Projects => "project",
            View::Automation => "calendar",
            View::Sessions => "history",
            View::Harness => "tools",
            View::Outline => "symbol-class",
            View::Agent => "comment-discussion",
            View::Tasks => "tasklist",
            View::Context => "pie-chart",
            View::Terminal => "terminal",
            View::DebugConsole => "debug-console",
            View::Output => "output",
            View::Ports => "plug",
        }
    }

    /// The id of the command that shows it, for a view the window did not begin with (those it began with have
    /// their commands in `workbench.rs`, and none here): VS Code's where it has the view, else the old Code Arena's.
    pub fn command(self) -> &'static str {
        match self {
            View::Files | View::Search | View::Changes | View::Outline | View::Agent | View::Terminal => "",
            View::Scm => "workbench.view.scm",
            View::Run => "workbench.view.debug",
            View::Extensions => "workbench.view.extensions",
            View::Problems => "workbench.actions.view.problems",
            View::Review => "codeArena.view.review",
            View::Plan => "codeArena.view.plan",
            View::Memory => "codeArena.view.memory",
            View::Skills => "codeArena.view.skills",
            View::Projects => "codeArena.view.projects",
            View::Automation => "codeArena.view.automation",
            View::Sessions => "codeArena.view.sessions",
            View::Harness => "codeArena.view.harness",
            View::Tasks => "codeArena.view.tasks",
            View::Context => "codeArena.view.context",
            View::DebugConsole => "workbench.debug.action.toggleRepl",
            View::Output => "workbench.action.output.toggleOutput",
            View::Ports => "codeArena.view.ports",
        }
    }

    /// Its default key, as VS Code has it (none: an empty text).
    pub fn keys(self) -> &'static str {
        match self {
            View::Files => "Ctrl+Shift+E",
            View::Search => "Ctrl+Shift+F",
            View::Scm => "Ctrl+Shift+G",
            View::Run => "Ctrl+Shift+D",
            View::Extensions => "Ctrl+Shift+X",
            View::Problems => "Ctrl+Shift+M",
            View::Agent => "Ctrl+Alt+I",
            View::Terminal => "Ctrl+`",
            View::DebugConsole => "Ctrl+Shift+Y",
            View::Output => "Ctrl+Shift+U",
            _ => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_view_is_named_once_and_has_an_icon_the_font_has() {
        let mut ids: Vec<&str> = VIEWS.into_iter().map(View::id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), VIEWS.len());
        for v in VIEWS {
            assert_eq!(View::from_id(v.id()), Some(v));
            assert!(crate::icons::known(v.icon()), "{v:?}: {}", v.icon());
            assert!(!v.title().is_empty());
        }
        assert_eq!(View::from_id("no-such-view"), None);
        // A view the window did not begin with is shown by a command of its own, each a different one.
        let mut commands: Vec<&str> = VIEWS.into_iter().map(View::command).filter(|c| !c.is_empty()).collect();
        assert_eq!(commands.len(), VIEWS.len() - 6);
        commands.sort_unstable();
        commands.dedup();
        assert_eq!(commands.len(), VIEWS.len() - 6);
    }

    #[test]
    fn the_views_in_the_window_are_the_ready_ones() {
        use View::*;
        assert_eq!(ready(), FIRST, "the views the window began with");
        assert!(FIRST.into_iter().all(View::written));
        assert_eq!(Changes.icon(), "source-control");
        also_ready(&[Scm, Problems]);
        assert_eq!(ready(), [Files, Search, Scm, Changes, Problems, Outline, Agent, Terminal]);
        assert_eq!(Changes.icon(), "git-compare", "beside Source Control, the agent's changes have another icon");
        // As the window itself has them: the views whose modules say they are written, no other.
        ready_as_written();
        assert_eq!(ready(), VIEWS.into_iter().filter(|v| v.written()).collect::<Vec<_>>());
        also_ready(&[]);
        assert_eq!(ready(), FIRST);
    }
}
