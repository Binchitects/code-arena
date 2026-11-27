//! VS Code's empty editor: its keys, faint, in the middle.

use floem::IntoView;
use floem::reactive::SignalGet;
use floem::views::{Decorators, dyn_stack, h_stack, label, v_stack};

use crate::state::Ui;

/// VS Code's empty editor: its keys, faint, in the middle.
pub(super) fn watermark(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let row = move |what: &'static str, keys: &'static str| {
        h_stack((
            label(move || what).style(|s| s.width(170.0).justify_end().font_size(13.0)),
            h_stack((dyn_stack(
                move || keys.split('+').map(str::to_string).collect::<Vec<_>>(),
                |k: &String| k.clone(),
                move |k: String| {
                    label(move || k.clone()).style(move |s| {
                        let c = t.get();
                        s.font_size(11.0)
                            .padding_horiz(5.0)
                            .padding_vert(2.0)
                            .border(1.0)
                            .border_radius(3.0)
                            .border_color(c.widget_border)
                            .background(c.code)
                    })
                },
            )
            .style(|s| s.flex_row().gap(3.0)),))
            .style(|s| s.width(170.0)),
        ))
        .style(|s| s.gap(12.0).items_center())
    };
    v_stack((
        row("Show All Commands", "Ctrl+Shift+P"),
        row("Go to File", "Ctrl+P"),
        row("Find in Files", "Ctrl+Shift+F"),
        row("Toggle Terminal", "Ctrl+`"),
        row("Open Chat", "Ctrl+Alt+I"),
    ))
    .style(move |s| s.gap(10.0).size_full().items_center().justify_center().color(t.get().watermark).selectable(false))
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: crate::state::Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
}
