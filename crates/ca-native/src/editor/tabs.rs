//! The editor group's tabs, as VS Code's: 35 px, the open one with its top border, a dot for changes not saved, a
//! close button; and the question asked before a tab with changes not saved closes.

use floem::event::EventListener;
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{SignalGet, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, v_stack};
use floem::{IntoView, View};

use super::{focus, live, tell};
use crate::icons::icon;
use crate::state::{Tab, Ui};
use crate::widgets::{action, button, line};

pub(super) fn tabs(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    h_stack((
        scroll(
            dyn_stack(move || ui.tabs.get(), |tab: &Tab| tab.path.clone(), move |tab: Tab| one_tab(ui, tab))
                .style(|s| s.flex_row().height_full()),
        )
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).height_full()),
        h_stack((action(t, "split-horizontal", || {}), action(t, "ellipsis", || {}))).style(move |s| {
            s.gap(2.0).padding_horiz(8.0).items_center().apply_if(ui.tabs.with(|t| t.is_empty()), |s| s.hide())
        }),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.tabs_border, 'b')
            .height(35.0)
            .width_full()
            .background(c.tabs_bg)
            .items_center()
            .apply_if(ui.tabs.with(|t| t.is_empty()), |s| s.hide())
    })
}

fn one_tab(ui: Ui, tab: Tab) -> impl IntoView {
    let t = ui.colors;
    let path = tab.path.clone();
    let here = tab.path.clone();
    let is_active = move || ui.active.with(|a| a.as_deref() == Some(here.as_str()));
    let active = is_active.clone();
    let hover = create_rw_signal(false);
    let dirty = tab.dirty;
    let shut = tab.path.clone();
    let top = is_active.clone();
    // A tab of a kind has its kind's icon before its name, and its name from its kind (a file's has none).
    let kind_icon = super::kinds::icon(&tab);
    let named = tab.clone();
    let content = h_stack((
        icon(kind_icon, 16.0).style(move |s| s.apply_if(kind_icon.is_empty(), |s| s.hide())),
        label(move || super::kinds::title(ui, &named)).style(|s| s.font_size(13.0).selectable(false)),
        dyn_container(
            move || (dirty.get(), hover.get() || is_active()),
            move |(dirty, shown)| {
                let shut = shut.clone();
                if dirty && !hover.get_untracked() {
                    icon("circle-filled", 16.0).style(|s| s.font_size(10.0)).into_any()
                } else if shown {
                    action(t, "close", move || ui.close(&shut)).into_any()
                } else {
                    empty().style(|s| s.width(22.0)).into_any()
                }
            },
        )
        .style(|s| s.width(22.0).height(22.0).items_center().justify_center()),
    ))
    .style(|s| s.flex_grow(1.0_f32).items_center().gap(4.0).padding_left(10.0).padding_right(4.0));
    v_stack((
        // The open tab's top border (tab.activeBorderTop).
        empty().style(move |s| {
            let c = t.get();
            s.height(1.0).width_full().background(if top() { c.tab_top } else { floem::peniko::Color::TRANSPARENT })
        }),
        content,
    ))
    .on_click_stop(move |_| ui.active.set(Some(path.clone())))
    .on_event_cont(floem::event::EventListener::PointerEnter, move |_| hover.set(true))
    .on_event_cont(floem::event::EventListener::PointerLeave, move |_| hover.set(false))
    .style(move |s| {
        let c = t.get();
        let s =
            s.height_full().min_width(80.0).border_right(1.0).border_color(c.tab_border).cursor(CursorStyle::Pointer);
        if active() {
            s.background(c.tab_active_bg).color(c.tab_active_fg)
        } else {
            s.background(c.tab_inactive_bg).color(c.tab_inactive_fg).hover(|s| s.background(c.tab_hover))
        }
    })
}

/// Asked before a tab with changes not saved closes, as VS Code asks: Save (it closes once saved), Don't Save,
/// Cancel; over the editor group, which takes no clicks meanwhile.
pub(super) fn close_question(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    // The file's name: a tab of a kind is asked about by its file too ("settings.json", not "user:settings.json").
    let name = move || {
        let named = |p: &str| ui.tab(p).map_or_else(|| p.rsplit('/').next().unwrap_or(p).to_string(), |tab| tab.name());
        ui.closing.with(|c| c.as_deref().map(named)).unwrap_or_default()
    };
    let dialog = v_stack((
        label(move || format!("Do you want to save the changes you made to {}?", name()))
            .style(|s| s.font_size(15.0).text_overflow(TextOverflow::Wrap)),
        label(|| "Your changes will be lost if you don't save them.")
            .style(move |s| s.margin_top(8.0).color(t.get().desc).text_overflow(TextOverflow::Wrap)),
        h_stack((
            button(t, "Save", true, move || {
                if let Some(path) = ui.closing.get_untracked() {
                    ui.close_saved.set(Some(path.clone()));
                    ui.closing.set(None);
                    tell(ui, |send| ui.save_tab(&path, send));
                }
            }),
            button(t, "Don't Save", false, move || {
                if let Some(path) = ui.closing.get_untracked() {
                    ui.close_now(&path);
                }
            }),
            button(t, "Cancel", false, move || ui.closing.set(None)),
        ))
        .style(|s| s.margin_top(20.0).gap(8.0).justify_end()),
    ))
    .on_click_stop(|_| {})
    .style(move |s| {
        let c = t.get();
        s.width(440.0)
            .max_width_pct(90.0)
            .padding(20.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(c.widget_border)
            .background(c.widget)
            .color(c.fg)
            .box_shadow_blur(16.0)
            .box_shadow_v_offset(4.0)
            .box_shadow_color(c.shadow)
    });
    let cancel = move || ui.closing.set(None);
    let over = container(dialog);
    let id = over.id();
    // The keys are the question's while it is asked (Escape cancels), and the editor's again after.
    create_effect(move |was: Option<bool>| {
        let asked = ui.closing.with(|c| c.is_some());
        if asked {
            id.request_focus();
        } else if was == Some(true)
            && let Some(editor) = ui.active.get_untracked().and_then(|p| live(ui, &p))
        {
            focus(&editor);
        }
        asked
    });
    over.keyboard_navigable()
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| cancel())
        .on_click_stop(|_| {})
        .on_event_stop(EventListener::PointerDown, |_| {})
        .on_event_stop(EventListener::PointerWheel, |_| {})
        .style(move |s| {
            s.absolute()
                .inset(0.0)
                .items_center()
                .justify_center()
                .z_index(30)
                .background(floem::peniko::Color::rgba8(0, 0, 0, 0x4D))
                .selectable(false)
                .apply_if(ui.closing.with(|c| c.is_none()), |s| s.hide())
        })
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: crate::state::Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
}
