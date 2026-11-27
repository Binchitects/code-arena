//! The Agent view, drawn as VS Code's chat view: the requests in bubbles on the right, the answers in full width
//! with their calls as cards that open, the agent's questions as cards, and the input box at the bottom with its
//! toolbar (the mode, the model and the thinking, each a picker; send or stop).
//!
//! Each file here is one part of the view. What a part draws in another's place (a chip in the box, a footer under
//! an answer, a bar over the box) it adds to that place (`slots.rs`), and does not edit the other's file. The
//! thread's items and all they carry are in `model.rs`; the session's messages reach each part through its `take`
//! (`ext.rs`).

use floem::kurbo::Rect;
use floem::reactive::{SignalGet, create_rw_signal};
use floem::style::Style;
use floem::views::{Decorators, stack, text_input, v_stack_from_iter};
use floem::{IntoView, View};

use crate::state::{Send, Ui};
use crate::widgets::Theme;

pub mod actions;
pub mod answer;
pub mod calls;
pub mod ask;
pub mod attach;
pub mod compaction;
pub mod composer;
pub mod diffs;
pub mod goal_bar;
pub mod header;
pub mod jobs_bar;
pub mod media;
pub mod mentions;
pub mod pickers;
pub mod pieces;
pub mod plan_bar;
pub mod question;
pub mod queue;
pub mod quotes;
pub mod rating;
pub mod ring;
pub mod slash;
pub mod slots;
pub mod stream;
pub mod thread;
pub mod tools;
pub mod tools_picker;

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    // Where the pill that opened a picker is in the window (its menu is placed over it), and the input, which the
    // keys go back to when the menu closes.
    let anchor = create_rw_signal(Rect::ZERO);
    let field = text_input(ui.draft);
    let back = field.id();
    let mut parts = vec![header::view(ui, send.clone()).into_any(), thread::thread(ui, send.clone()).into_any()];
    parts.extend(bars(ui, &send));
    parts.push(composer::input(ui, send.clone(), field, anchor).into_any());
    parts.extend(slots::column(ui, &send, slots::Slot::BoxFoot));
    let column = v_stack_from_iter(parts);
    // The view's place in the window: the menu's layer fills the same.
    let frame = column.id();
    stack((column.style(|s| s.size_full().min_height(0.0)), pickers::menu(ui, send, anchor, frame, back)))
        .style(|s| s.size_full().min_height(0.0))
}

/// A bar over the input (the plan's, the goal's, a command's with no time limit): its look.
pub fn bar(t: Theme, s: Style) -> Style {
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

/// The bars over the input, as their place holds them: the plan, the goal, the commands running with no time
/// limit, and what other parts add.
fn bars(ui: Ui, send: &Send) -> Option<floem::AnyView> {
    slots::column(ui, send, slots::Slot::Bars).map(|bars| bars.style(|s| s.gap(6.0).padding_horiz(12.0)).into_any())
}
