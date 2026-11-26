//! The Agent view, drawn as VS Code's chat view: the requests in bubbles on the right, the answers in full width
//! with their calls in a line that opens, the agent's questions as cards, and the input box at the bottom with its
//! toolbar (the mode, the model and the thinking, each a picker; send or stop).

use floem::kurbo::Rect;
use floem::reactive::{SignalGet, create_rw_signal};
use floem::style::Style;
use floem::views::{Decorators, stack, text_input, v_stack};
use floem::{IntoView, View};

use crate::state::{Send, Ui};
use crate::widgets::Theme;

mod answer;
mod ask;
mod composer;
mod goal_bar;
mod header;
mod jobs_bar;
mod pickers;
mod plan_bar;
mod question;
mod queue;
pub mod stream;
mod thread;
mod tools;

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    // Where the pill that opened a picker is in the window (its menu is placed over it), and the input, which the
    // keys go back to when the menu closes.
    let anchor = create_rw_signal(Rect::ZERO);
    let field = text_input(ui.draft);
    let back = field.id();
    let column = v_stack((
        header::view(ui, send.clone()),
        thread::thread(ui, send.clone()),
        bars(ui, send.clone()),
        composer::input(ui, send.clone(), field, anchor),
    ));
    // The view's place in the window: the menu's layer fills the same.
    let frame = column.id();
    stack((column.style(|s| s.size_full().min_height(0.0)), pickers::menu(ui, send, anchor, frame, back)))
        .style(|s| s.size_full().min_height(0.0))
}

/// A bar over the input (the plan's, the goal's, a command's with no time limit): its look.
fn bar(t: Theme, s: Style) -> Style {
    let c = t.get();
    s.padding_horiz(10.0)
        .padding_vert(5.0)
        .border(1.0)
        .border_color(c.widget_border)
        .border_radius(6.0)
        .background(c.widget)
        .font_size(12.0)
        .color(c.fg)
}

/// The bars over the input: the plan, the goal, the commands running with no time limit.
fn bars(ui: Ui, send: Send) -> impl IntoView {
    v_stack((plan_bar::view(ui), goal_bar::view(ui), jobs_bar::view(ui, send)))
        .style(|s| s.gap(6.0).padding_horiz(12.0))
}
