//! The editor group, as VS Code's: its 35 px tabs (the open one with its top border, a dot for changes not saved, a
//! close button), the breadcrumbs, then the editor (line numbers, the current line, the caret, VS Code's font); with
//! nothing open, VS Code's watermark of keys.

use floem::IntoView;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith, create_rw_signal};
use floem::style::CursorStyle;
use floem::text::FamilyOwned;
use floem::views::editor::text::SimpleStyling;
use floem::views::{Decorators, dyn_container, dyn_stack, empty, h_stack, label, scroll, text_editor, v_stack};

use crate::icons::icon;
use crate::state::{Tab, Ui};
use crate::widgets::{MONO_FONT, action, line};

pub fn view(ui: Ui) -> impl IntoView {
    v_stack((tabs(ui), breadcrumbs(ui), body(ui))).style(|s| s.size_full().min_width(0.0).min_height(0.0))
}

fn tabs(ui: Ui) -> impl IntoView {
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
    let content = h_stack((
        label({
            let n = tab.name();
            move || n.clone()
        })
        .style(|s| s.font_size(13.0).selectable(false)),
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

fn breadcrumbs(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    dyn_container(
        move || ui.active.get(),
        move |path| {
            let parts: Vec<String> = path.unwrap_or_default().split('/').map(str::to_string).collect();
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
            .apply_if(ui.active.with(|a| a.is_none()), |s| s.hide())
    })
}

fn body(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    dyn_container(
        move || (ui.active.get(), t.get()),
        move |(path, c)| match path.and_then(|p| ui.tab(&p)) {
            None => watermark(ui).into_any(),
            Some(tab) => {
                let styling = SimpleStyling::builder()
                    .font_size(14)
                    .line_height(19.0)
                    .font_family(FamilyOwned::parse_list(MONO_FONT).collect())
                    .build();
                let caret = ui.caret;
                let ed = text_editor("")
                    .use_doc(tab.doc.clone())
                    .styling(styling)
                    .editor_style(move |s| {
                        s.gutter_dim_color(c.line_number)
                            .gutter_accent_color(c.line_number_active)
                            .gutter_current_color(c.editor_bg)
                            .gutter_left_padding(18.0)
                            .gutter_right_padding(22.0)
                            .current_line_color(c.line_highlight)
                            .selection_color(c.selection)
                            .cursor_color(c.cursor)
                            .indent_guide_color(c.indent_guide)
                            .visible_whitespace(c.whitespace)
                            .phantom_color(c.desc)
                            .placeholder_color(c.placeholder)
                    })
                    .style(move |s| {
                        s.size_full()
                            .background(c.editor_bg)
                            .set(floem::views::editor::Foreground, c.editor_fg)
                            .color(c.editor_fg)
                    });
                let editor = ed.editor().clone();
                let cursor = editor.cursor;
                floem::reactive::create_effect(move |_| {
                    let offset = cursor.with(|c| c.offset());
                    let (line, col) = editor.offset_to_line_col(offset);
                    caret.set((line + 1, col + 1));
                });
                ed.into_any()
            }
        },
    )
    .style(move |s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full().background(t.get().editor_bg))
}

/// VS Code's empty editor: its keys, faint, in the middle.
fn watermark(ui: Ui) -> impl IntoView {
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
