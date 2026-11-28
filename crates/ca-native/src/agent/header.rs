//! The Agent view's header: the old Code Arena's buttons, in the view's title at its right as a VS Code view
//! title's actions (`View::actions`, in the dock and in a window of its own). The goal (lit while one is worked toward), the tasks (with how many are at work), this folder's
//! sessions, and "…", the chat's More menu (`menus::Menu::AgentMore`: what every part adds to it, under the
//! window's own Open in a New Window and Hide the Chat).
//!
//! Each button runs a command, and is there only while its command is offered: the goal's is goal mode's to add
//! (`GOAL`); the tasks' and the sessions' are here, offered while their views are in the window.

use floem::reactive::{SignalGet, SignalWith};
use floem::style::Position;
use floem::views::{Decorators, container, h_stack, label, stack, tooltip};
use floem::{IntoView, View as _};
use serde_json::Value;

use crate::commands::{self, command};
use crate::icons::icon;
use crate::layout::View;
use crate::menus::{self, Menu};
use crate::state::{Send, Ui};
use crate::widgets::{action, popup};

/// The goal's command: "Work Toward a Goal…", goal mode's.
pub const GOAL: &str = "codeArena.chat.goal";
/// The tasks and sub-agents at work, in the Tasks view.
pub const TASKS: &str = "codeArena.chat.showTasks";
/// This folder's sessions, in the Sessions view.
pub const HISTORY: &str = "codeArena.chat.history";
/// The Agent view hidden.
pub const HIDE: &str = "codeArena.chat.hide";
/// The earlier part of the conversation summed up (`/compact`), between turns.
pub const COMPACT: &str = "codeArena.chat.compact";

/// How many are at work, as the tasks button's badge says it.
fn at_work(n: usize) -> String {
    format!("{n} at work")
}

/// A header button: its command's action, with its words when the pointer rests on it, there while the command
/// is offered.
fn button(ui: Ui, send: &Send, id: &'static str, name: &'static str, tip: &'static str) -> impl IntoView + use<> {
    let send = send.clone();
    let shown = move || commands::find(id).is_some_and(|c| c.offered(ui));
    tooltip(
        action(ui.colors, name, move || {
            commands::run(ui, &send, id);
        }),
        move || label(move || tip),
    )
    .style(move |s| s.apply_if(!shown(), |s| s.hide()))
}

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    // The goal's icon in the accent while one is worked toward.
    let goal = {
        let send = send.clone();
        let target = container(icon("target", 16.0)).on_click_stop(move |_| {
            commands::run(ui, &send, GOAL);
        });
        tooltip(target, || label(|| "Work Toward a Goal…")).style(move |s| {
            let c = t.get();
            s.width(22.0)
                .height(22.0)
                .items_center()
                .justify_center()
                .border_radius(5.0)
                .color(if ui.goal.with(Option::is_some) { c.focus } else { c.icon })
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(c.toolbar_hover))
                .apply_if(!commands::find(GOAL).is_some_and(|c| c.offered(ui)), |s| s.hide())
        })
    };
    let busy = move || ui.jobs.with(Vec::len);
    let tasks = stack((
        button(ui, &send, TASKS, "tasklist", "Show Tasks and Sub-agents"),
        label(move || busy().to_string()).style(move |s| {
            let c = t.get();
            s.position(Position::Absolute)
                .inset_right(-2.0)
                .inset_top(-2.0)
                .min_width(13.0)
                .height(13.0)
                .padding_horiz(3.0)
                .border_radius(7.0)
                .font_size(9.0)
                .justify_center()
                .items_center()
                .background(c.badge_bg)
                .color(c.badge_fg)
                .apply_if(busy() == 0, |s| s.hide())
        }),
    ));
    let tasks = tooltip(tasks, move || {
        label(move || if busy() == 0 { "Show Tasks and Sub-agents".into() } else { at_work(busy()) })
    })
    .style(move |s| s.apply_if(!commands::find(TASKS).is_some_and(|c| c.offered(ui)), |s| s.hide()));
    let history = button(ui, &send, HISTORY, "history", "This Folder's Sessions");
    let more = {
        let send = send.clone();
        let button = action(t, "ellipsis", || {}).into_view();
        let id = button.id();
        tooltip(
            button.on_click_stop(move |_| popup::open_menu(ui, &send, popup::At::under(id), Menu::AgentMore)),
            || label(|| "More Actions…"),
        )
    };
    h_stack((crate::sessions_arena::mark(ui), goal, tasks, history, more)).style(|s| s.items_center().gap(2.0))
}

/// What it adds to the window's registries, once at start: the tasks' and the sessions' commands, and the
/// window's own rows of the More menu.
pub fn register(_ui: Ui, _send: &Send) {
    commands::add(
        command(TASKS, "Chat", "Show Tasks and Sub-agents", |ui, _| ui.reveal(View::Tasks))
            .when(|ui| ui.available(View::Tasks)),
    );
    commands::add(
        command(HISTORY, "Chat", "Show This Folder's Sessions", |ui, _| ui.reveal(View::Sessions))
            .when(|ui| ui.available(View::Sessions)),
    );
    commands::add(
        command(HIDE, "Chat", "Hide the Chat", |ui, _| {
            if ui.view_shown(View::Agent) {
                ui.toggle_view(View::Agent);
            }
        })
        .when(|ui| ui.view_shown(View::Agent)),
    );
    commands::add(
        command(COMPACT, "Chat", "Compact the Conversation", super::compaction::compact_now)
            .enabled(|ui| !ui.running.get_untracked()),
    );
    let float = crate::workbench::float_command(View::Agent);
    menus::add(Menu::AgentMore, menus::item(&float).title("Open in a New Window").group("8_window").order(10));
    menus::add(Menu::AgentMore, menus::item(HIDE).group("8_window").order(20));
}

/// The session's messages that are this part's, taken in: none (it reads what the window has). Never taken
/// away (false).
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tasks_badge_says_how_many_are_at_work() {
        assert_eq!(at_work(3), "3 at work");
    }
}
