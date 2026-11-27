//! The breadcrumbs under a group's tabs, as VS Code's: the path of the file the group shows, part by part.

use floem::IntoView;
use floem::reactive::{SignalGet, SignalWith};
use floem::views::{Decorators, dyn_container, dyn_stack, h_stack, label};

use crate::icons::icon;
use crate::state::Ui;

pub(super) fn breadcrumbs(ui: Ui, active: floem::reactive::Memo<Option<String>>) -> impl IntoView {
    let t = ui.colors;
    // Shown for a file's text (or its changes), not for a tab of a kind, while `breadcrumbs.enabled` is on.
    let text = move || {
        active
            .with(|a| a.as_ref().is_some_and(|p| ui.tabs.with(|t| t.iter().any(|t| t.path == *p && t.kind.is_none()))))
    };
    dyn_container(
        move || active.get(),
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
            .apply_if(!text() || !crate::settings::flag(ui, "breadcrumbs.enabled", None), |s| s.hide())
    })
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: crate::state::Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
}
