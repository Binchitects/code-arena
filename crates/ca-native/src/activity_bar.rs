//! The activity bar, as VS Code's: the side bar's views by their icons (all the views in the window when no group
//! is at its left), each with its badge when it has a count (`badges.rs`), each dragged to dock it elsewhere; and at
//! its foot the theme button (`theme_menu.rs`) and the Accounts and Manage buttons, which run the commands of those
//! names (`ACCOUNTS`, `MANAGE`) once a part has added them; [`foot_button`] says which was clicked.

use floem::IntoView;
use floem::View as _;
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalWith};
use floem::style::CursorStyle;
use floem::views::{Decorators, container, dyn_stack, empty, h_stack, label, stack, tooltip, v_stack};
use serde_json::Value;

use crate::dock::Node;
use crate::icons::icon;
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::widgets::line;

/// The commands the foot's buttons run, under VS Code's ids for those buttons. The parts that have their menus add
/// the commands; until then a click does nothing.
pub const ACCOUNTS: &str = "workbench.actions.accounts";
pub const MANAGE: &str = "workbench.actions.manage";

thread_local! {
    /// The foot's button clicked last, for the menu its command opens beside it.
    static FOOT: std::cell::Cell<Option<floem::ViewId>> = const { std::cell::Cell::new(None) };
}

/// The foot's button clicked last (the theme button, Accounts, Manage): its command's menu opens beside it.
pub fn foot_button() -> Option<floem::ViewId> {
    FOOT.get()
}

/// The views the bar lists now: the side bar's, as the dock draws it; with no group at the window's left, every
/// view that is in the window.
pub fn listed(ui: Ui) -> Vec<View> {
    ui.drawn().with(|d| match d.side_bar().and_then(|g| d.find(g)) {
        Some(Node::Tabs { views, .. }) => views.clone(),
        _ => crate::layout::ready().into_iter().filter(|v| ui.available(*v)).collect(),
    })
}

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let item = move |v: View| {
        let lit = move || ui.view_shown(v);
        // VS Code's badge: a count at the icon's lower right, there only while there is one.
        let count = crate::badges::count(ui, v);
        let badge = label(move || crate::badges::text(count.get())).style(move |s| {
            let c = t.get();
            s.absolute()
                .inset_top(24.0)
                .inset_right(6.0)
                .height(16.0)
                .min_width(16.0)
                .padding_horiz(4.0)
                .items_center()
                .justify_center()
                .border_radius(8.0)
                .font_size(9.0)
                .background(c.badge_bg)
                .color(c.badge_fg)
                .apply_if(count.get() == 0, |s| s.hide())
        });
        let button = h_stack((
            empty().style(move |s| {
                let c = t.get();
                s.width(2.0).height_full().background(if lit() { c.activity_active_border } else { Color::TRANSPARENT })
            }),
            stack((icon(v.icon(), 24.0), badge))
                .style(|s| s.flex_grow(1.0_f32).height_full().items_center().justify_center()),
            empty().style(|s| s.width(2.0)),
        ));
        let button = crate::docking::handle(ui, v, button, move || ui.toggle_view(v)).style(move |s| {
            let c = t.get();
            s.width(48.0)
                .height(48.0)
                .cursor(CursorStyle::Pointer)
                .color(if lit() { c.activity_fg } else { c.activity_inactive })
                .hover(|s| s.color(c.activity_fg))
        });
        // VS Code's hover: the view's name and its keys.
        tooltip(button, move || {
            let keys = v.keys();
            label(move || if keys.is_empty() { v.title().to_string() } else { format!("{} ({keys})", v.title()) })
        })
    };
    let bottom = move |name: &'static str, command: &'static str| {
        let send = send.clone();
        let button = container(icon(name, 24.0));
        let id = button.id();
        button
            .on_click_stop(move |_| {
                FOOT.set(Some(id));
                crate::commands::run(ui, &send, command);
            })
            .style(move |s| {
                let c = t.get();
                s.width(48.0)
                    .height(48.0)
                    .items_center()
                    .justify_center()
                    .color(c.activity_inactive)
                    .hover(|s| s.color(c.activity_fg))
            })
    };
    v_stack((
        dyn_stack(move || listed(ui), |v: &View| *v, item).style(|s| s.flex_col()),
        empty().style(|s| s.flex_grow(1.0_f32)),
        bottom("color-mode", crate::theme_menu::THEME_BUTTON),
        bottom("account", ACCOUNTS),
        bottom("settings-gear", MANAGE),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.activity_border, 'r').width(48.0).height_full().background(c.activity_bg)
    })
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{EDITOR, Side};
    use crate::layout::also_ready;

    #[test]
    fn the_bar_lists_the_side_bars_views_that_are_in_the_window() {
        use View::*;
        let ui = Ui::new();
        assert_eq!(listed(ui), [Files, Search, Changes, Outline]);
        // A view withheld is not listed, and is listed where it was when it is given back.
        ui.withhold(Changes, true);
        assert_eq!(listed(ui), [Files, Search, Outline]);
        ui.withhold(Changes, false);
        assert_eq!(listed(ui), [Files, Search, Changes, Outline]);
        // No group at the window's left: every view in the window, and none that is not written.
        for v in [Files, Search, Changes, Outline] {
            ui.dock_view(v, EDITOR, Side::Bottom);
        }
        assert_eq!(listed(ui), crate::layout::ready());
        // A view written since is in the side bar its first place is.
        also_ready(&[Scm]);
        let ui = Ui::new();
        assert_eq!(listed(ui), [Files, Search, Scm, Changes, Outline]);
        also_ready(&[]);
    }
}
