//! The breadcrumbs under the tabs, as VS Code's: the shown file's path, part by part.

use floem::IntoView;
use floem::reactive::SignalGet;
use floem::views::{Decorators, dyn_container, dyn_stack, h_stack, label};

use crate::icons::icon;
use crate::state::Ui;

pub(super) fn breadcrumbs(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    dyn_container(
        move || ui.active.get(),
        move |path| {
            let file = path.and_then(|p| ui.tab(&p)).map(|t| t.file).unwrap_or_default();
            let parts: Vec<String> = file.split('/').map(str::to_string).collect();
            let last = parts.len().saturating_sub(1);
            h_stack((dyn_stack(
                move || parts.clone().into_iter().enumerate().collect::<Vec<_>>(),
                |(n, p): &(usize, String)| (*n, p.clone()),
                move |(n, p): (usize, String)| {
                    h_stack((
                        label(move || p.clone()),
                        icon("chevron-right", 16.0).style(move |s| s.apply_if(n == last, |s| s.hide())),
                    ))
                    .style(|s| s.items_center().gap(2.0))
                },
            )
            .style(|s| s.flex_row().gap(2.0)),))
            .into_any()
        },
    )
    .style(move |s| {
        let c = t.get();
        s.height(22.0)
            .width_full()
            .items_center()
            .padding_left(12.0)
            .font_size(13.0)
            .color(c.breadcrumb)
            .background(c.editor_bg)
            .selectable(false)
            .apply_if(!super::kinds::text_shown(ui), |s| s.hide())
    })
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: crate::state::Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
}
