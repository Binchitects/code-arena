//! VS Code's empty editor. The only group, empty, is the window's welcome: "No file is open", Open Folder… and
//! its keys, faint, in the middle. Another empty group says "Drag a tab here, or open a file."

use floem::IntoView;
use floem::reactive::{SignalGet, SignalWith, create_memo};
use floem::views::{Decorators, dyn_container, dyn_stack, h_stack, label, v_stack};

use crate::state::Ui;

/// VS Code's empty editor: the welcome when its group is the only one, else what a tab can be put here by.
pub(super) fn watermark(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let only = create_memo(move |_| super::groups::groups(ui).model.with(|m| m.groups.len() <= 1));
    dyn_container(
        move || only.get(),
        move |only| {
            if only {
                welcome(ui).into_any()
            } else {
                label(|| "Drag a tab here, or open a file.")
                    .style(move |s| s.font_size(13.0).color(t.get().watermark))
                    .into_any()
            }
        },
    )
    .style(|s| s.size_full().items_center().justify_center().selectable(false))
}

/// The window's welcome: what is not open, Open Folder…, the keys.
fn welcome(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let row = move |what: &'static str, keys: &'static str| {
        h_stack((
            label(move || what).style(|s| s.width(170.0).justify_end().font_size(13.0)),
            h_stack((dyn_stack(
                move || {
                    keys.split(' ')
                        .flat_map(|chord| chord.split('+'))
                        .map(str::to_string)
                        .enumerate()
                        .collect::<Vec<_>>()
                },
                |(i, _): &(usize, String)| *i,
                move |(_, k): (usize, String)| {
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
    let open = crate::widgets::button(t, "Open Folder...", true, move || {
        if let Some(send) = ui.out.get_untracked() {
            crate::commands::run(ui, &send, crate::open_folder::OPEN_FOLDER);
        }
    });
    v_stack((
        label(|| "No file is open").style(|s| s.font_size(15.0).margin_bottom(2.0)),
        open.style(|s| s.margin_bottom(14.0)),
        row("Show All Commands", "Ctrl+Shift+P"),
        row("Go to File", "Ctrl+P"),
        row("Open Folder", "Ctrl+K Ctrl+O"),
        row("Open Recent", "Ctrl+R"),
        row("Find in Files", "Ctrl+Shift+F"),
        row("Toggle Terminal", "Ctrl+`"),
        row("Open Chat", "Ctrl+Alt+I"),
    ))
    .style(move |s| s.gap(10.0).items_center().color(t.get().watermark))
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: crate::state::Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
}
