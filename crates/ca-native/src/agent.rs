//! The Agent view, drawn as VS Code's chat view: the requests in bubbles on the right, the answers in full width
//! with their calls in a line that opens, the agent's questions as cards, and the input box at the bottom with its
//! toolbar (the mode, the model and the thinking, each a picker; send or stop).

use std::rc::Rc;

use floem::event::EventListener;
use floem::keyboard::{Key, NamedKey};
use floem::kurbo::Rect;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::style::{CursorStyle, Position, TextOverflow};
use floem::views::{
    Decorators, TextInput, dyn_container, dyn_stack, empty, h_stack, label, scroll, stack, text_input, v_stack,
};
use floem::{IntoView, View, ViewId};
use serde_json::json;

use crate::icons::icon;
use crate::model::{Item, Kind, Tool, summary};
use crate::state::{Picker, Send, Ui};
use crate::widgets::{MONO_FONT, action, bold, button};

/// A menu's least width, as VS Code's.
const MENU_WIDTH: f64 = 160.0;

/// The modes the session takes (what runs without asking), and how much it may think.
const MODES: &[&str] = &["ask", "auto-edit", "plan", "yolo"];
const THINKING: &[&str] = &["default", "off", "low", "medium", "high", "xhigh"];

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    // Where the pill that opened a picker is in the window (its menu is placed over it), and the input, which the
    // keys go back to when the menu closes.
    let anchor = create_rw_signal(Rect::ZERO);
    let field = text_input(ui.draft);
    let back = field.id();
    let column = v_stack((thread(ui, send.clone()), bars(ui, send.clone()), input(ui, send.clone(), field, anchor)));
    // The view's place in the window: the menu's layer fills the same.
    let frame = column.id();
    stack((column.style(|s| s.size_full().min_height(0.0)), menu(ui, send, anchor, frame, back)))
        .style(|s| s.size_full().min_height(0.0))
}

fn welcome(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    v_stack((
        icon("comment-discussion", 48.0).style(move |s| s.color(t.get().desc).margin_bottom(12.0)),
        label(|| "Code Arena").style(move |s| bold(s).font_size(20.0).color(t.get().fg)),
        label(|| "Ask about this folder, or have the agent change it.")
            .style(move |s| s.color(t.get().desc).margin_top(6.0).text_overflow(TextOverflow::Wrap)),
    ))
    .style(|s| s.items_center().justify_center().size_full().padding(20.0))
}

fn thread(ui: Ui, send: Send) -> impl IntoView {
    let follow = create_rw_signal(0.0f32);
    create_effect(move |_| {
        ui.items.with(|l| l.len());
        follow.set(100.0);
    });
    dyn_container(
        move || ui.items.with(|l| l.is_empty()),
        move |empty| {
            if empty {
                welcome(ui).into_any()
            } else {
                let send = send.clone();
                scroll(
                    dyn_stack(move || ui.items.get(), |i: &Item| i.id, move |i: Item| item(ui, i, send.clone()))
                        .style(|s| s.flex_col().width_full().padding_horiz(16.0).padding_vert(12.0).gap(16.0)),
                )
                .scroll_to_percent(move || follow.get())
                .style(|s| s.size_full())
                .into_any()
            }
        },
    )
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full())
}

fn item(ui: Ui, i: Item, send: Send) -> impl IntoView {
    let t = ui.colors;
    match i.kind {
        Kind::User(text) | Kind::Queued(text) => h_stack((
            empty().style(|s| s.flex_grow(1.0_f32).min_width(40.0)),
            label(move || text.clone()).style(move |s| {
                let c = t.get();
                s.padding_horiz(12.0)
                    .padding_vert(8.0)
                    .border_radius(8.0)
                    .background(c.chat_request)
                    .color(c.fg)
                    .line_height(1.5)
                    .text_overflow(TextOverflow::Wrap)
                    .max_width_pct(85.0)
            }),
        ))
        .style(|s| s.width_full())
        .into_any(),
        Kind::Assistant { text, tools, open } => v_stack((
            h_stack((
                dyn_container(move || open.get(), |o| icon(if o { "chevron-down" } else { "chevron-right" }, 16.0)),
                label(move || summary(&tools.get())),
            ))
            .on_click_stop(move |_| open.update(|o| *o = !*o))
            .style(move |s| {
                s.gap(4.0)
                    .items_center()
                    .color(t.get().desc)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .apply_if(tools.with(|t| t.is_empty()), |s| s.hide())
            }),
            dyn_stack(
                move || if open.get() { tools.get().into_iter().enumerate().collect::<Vec<_>>() } else { vec![] },
                |(n, t): &(usize, Tool)| (*n, t.done),
                move |(_, tool): (usize, Tool)| {
                    let failed = !tool.ok;
                    let out = tool.text.clone();
                    v_stack((
                        h_stack((
                            icon(
                                if !tool.done {
                                    "loading"
                                } else if failed {
                                    "error"
                                } else {
                                    "check"
                                },
                                16.0,
                            )
                            .style(move |s| {
                                let c = t.get();
                                s.color(if failed { c.error } else { c.desc })
                            }),
                            label(move || format!("{}  {}", tool.name, tool.subject))
                                .style(move |s| s.font_family(MONO_FONT.clone()).font_size(12.0).color(t.get().desc)),
                        ))
                        .style(|s| s.gap(6.0).items_center()),
                        label(move || out.clone()).style(move |s| {
                            let c = t.get();
                            s.font_family(MONO_FONT.clone())
                                .font_size(12.0)
                                .padding(8.0)
                                .border_radius(4.0)
                                .background(c.code)
                                .text_overflow(TextOverflow::Wrap)
                                .apply_if(tool.text.is_empty(), |s| s.hide())
                        }),
                    ))
                    .style(|s| s.gap(4.0).padding_left(20.0))
                },
            )
            .style(|s| s.flex_col().gap(6.0)),
            crate::prose::answer(ui, text),
        ))
        .style(|s| s.gap(8.0).width_full())
        .into_any(),
        Kind::Notice(text) => label(move || text.clone())
            .style(move |s| s.color(t.get().desc).font_size(12.0).text_overflow(TextOverflow::Wrap))
            .into_any(),
        Kind::Error(text) => h_stack((
            icon("error", 16.0).style(move |s| s.color(t.get().error)),
            label(move || text.clone()).style(move |s| s.color(t.get().error).text_overflow(TextOverflow::Wrap)),
        ))
        .style(|s| s.gap(6.0))
        .into_any(),
        Kind::Ask { id, tool, subject, answer } => {
            let reply = move |a: &'static str| {
                let send = send.clone();
                move || send(json!({"type": "answer", "id": id, "answer": a}))
            };
            v_stack((
                label(move || format!("Run {tool}?")).style(move |s| bold(s).color(t.get().fg)),
                label(move || subject.clone()).style(move |s| {
                    let c = t.get();
                    s.font_family(MONO_FONT.clone())
                        .font_size(12.0)
                        .padding(8.0)
                        .border_radius(4.0)
                        .background(c.code)
                        .text_overflow(TextOverflow::Wrap)
                }),
                h_stack((
                    button(t, "Allow", true, reply("yes")),
                    button(t, "Always, this session", false, reply("always")),
                    button(t, "Deny", false, reply("no")),
                ))
                .style(move |s| s.gap(8.0).apply_if(answer.get().is_some(), |s| s.hide())),
                label(move || match answer.get().as_deref() {
                    Some("no") => "Denied".to_string(),
                    Some("always") => "Allowed for this session".to_string(),
                    Some("yes") => "Allowed".to_string(),
                    Some("gone") => "No longer asked: the turn ended".to_string(),
                    _ => String::new(),
                })
                .style(move |s| s.color(t.get().desc).apply_if(answer.get().is_none(), |s| s.hide())),
            ))
            .style(move |s| {
                let c = t.get();
                s.gap(10.0)
                    .padding(12.0)
                    .border(1.0)
                    .border_color(c.widget_border)
                    .border_radius(6.0)
                    .background(c.widget)
            })
            .into_any()
        }
    }
}

fn bars(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let bar = move |s: floem::style::Style| {
        let c = t.get();
        s.padding_horiz(10.0)
            .padding_vert(5.0)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(6.0)
            .background(c.widget)
            .font_size(12.0)
            .color(c.fg)
    };
    v_stack((
        label(move || ui.plan.get().map(|(p, d, n)| format!("Plan  {p} · {d} of {n} done")).unwrap_or_default())
            .style(move |s| bar(s).apply_if(ui.plan.get().is_none(), |s| s.hide())),
        label(move || ui.goal.get().map(|g| format!("Goal  {g}")).unwrap_or_default())
            .style(move |s| bar(s).apply_if(ui.goal.get().is_none(), |s| s.hide())),
        dyn_stack(
            move || ui.jobs.get(),
            |(id, _): &(u64, String)| *id,
            move |(id, cmd): (u64, String)| {
                let send = send.clone();
                h_stack((
                    icon("terminal", 16.0),
                    label(move || cmd.clone()).style(|s| s.flex_grow(1.0_f32).font_family(MONO_FONT.clone())),
                    action(t, "debug-stop", move || send(json!({"type": "stop_job", "id": id}))),
                ))
                .style(move |s| bar(s).gap(6.0).items_center())
            },
        )
        .style(|s| s.flex_col().gap(6.0)),
    ))
    .style(|s| s.gap(6.0).padding_horiz(12.0))
}

/// The input as VS Code's chat input: a box with the text and, under it, its toolbar.
fn input(ui: Ui, send: Send, field: TextInput, anchor: RwSignal<Rect>) -> impl IntoView {
    let t = ui.colors;
    let focused = create_rw_signal(false);
    let submit = {
        let send = send.clone();
        move || {
            let text = ui.draft.get_untracked().trim().to_string();
            if !text.is_empty() {
                send(json!({"type": "send", "text": text}));
                ui.draft.set(String::new());
            }
        }
    };
    let enter = submit.clone();
    let stop = send.clone();
    let send_or_stop = send.clone();
    // A turn that begins closes a picker left open.
    create_effect(move |_| {
        if ui.running.get() {
            ui.picker.set(None);
        }
    });
    v_stack((
        field
            .placeholder("Ask Code Arena")
            .on_key_down(Key::Named(NamedKey::Enter), |m| m.is_empty(), move |_| enter())
            .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| stop(json!({"type": "stop"})))
            .on_event_cont(floem::event::EventListener::FocusGained, move |_| focused.set(true))
            .on_event_cont(floem::event::EventListener::FocusLost, move |_| focused.set(false))
            .style(move |s| {
                let c = t.get();
                s.width_full()
                    .padding_horiz(2.0)
                    .padding_vert(4.0)
                    .border(0.0)
                    .background(floem::peniko::Color::TRANSPARENT)
                    .color(c.input_fg)
                    .font_size(13.0)
                    .cursor_color(floem::peniko::Brush::Solid(c.cursor))
            }),
        h_stack((
            pill(ui, Picker::Mode, anchor),
            pill(ui, Picker::Model, anchor),
            pill(ui, Picker::Thinking, anchor),
            empty().style(|s| s.flex_grow(1.0_f32)),
            dyn_container(
                move || ui.running.get(),
                move |running| {
                    let send = send_or_stop.clone();
                    let submit = submit.clone();
                    if running {
                        action(t, "debug-stop", move || send(json!({"type": "stop"}))).into_any()
                    } else {
                        action(t, "send", submit).into_any()
                    }
                },
            ),
        ))
        .style(|s| s.gap(2.0).items_center().width_full().min_width(0.0)),
    ))
    .style(move |s| {
        let c = t.get();
        s.margin(12.0)
            .margin_top(6.0)
            .padding(6.0)
            .gap(4.0)
            .border(1.0)
            .border_color(if focused.get() { c.focus } else { c.input_border })
            .border_radius(6.0)
            .background(c.input_bg)
    })
}

/// A name with its first letter a capital, as a menu shows it.
fn capital(s: &str) -> String {
    let mut rest = s.chars();
    rest.next().map(|c| c.to_uppercase().chain(rest).collect()).unwrap_or_default()
}

/// What a picker has in force.
fn current(ui: Ui, which: Picker) -> String {
    match which {
        Picker::Mode => ui.mode.get(),
        Picker::Model => ui.model.get(),
        Picker::Thinking => ui.thinking.get(),
    }
}

/// What a picker offers. The model in force is listed even when the session did not name it among its models.
fn options(ui: Ui, which: Picker) -> Vec<String> {
    match which {
        Picker::Mode => MODES.iter().map(|m| m.to_string()).collect(),
        Picker::Thinking => THINKING.iter().map(|m| m.to_string()).collect(),
        Picker::Model => {
            let (mut all, now) = (ui.models.get(), ui.model.get());
            if !now.is_empty() && !all.contains(&now) {
                all.insert(0, now);
            }
            all
        }
    }
}

/// An option as the menu names it: a model by its id, the others with a capital.
fn named(which: Picker, v: &str) -> String {
    if which == Picker::Model { v.to_string() } else { capital(v) }
}

/// What tells the session a picker's choice.
fn chosen(which: Picker, v: &str) -> serde_json::Value {
    match which {
        Picker::Mode => json!({"type": "mode", "mode": v}),
        Picker::Model => json!({"type": "model", "model": v}),
        Picker::Thinking => json!({"type": "thinking", "level": v}),
    }
}

/// One of the input's pickers, as VS Code's chat input has them: what is in force and a chevron, opening its menu
/// over itself. While a turn runs the session takes none of the three, so they are dimmed and do not open.
fn pill(ui: Ui, which: Picker, anchor: RwSignal<Rect>) -> impl IntoView {
    let t = ui.colors;
    let view = h_stack((
        label(move || {
            let now = current(ui, which);
            match which {
                Picker::Mode if now.is_empty() => "Agent".to_string(),
                Picker::Thinking if now.is_empty() => "Thinking".to_string(),
                Picker::Thinking => format!("Thinking: {now}"),
                _ => named(which, &now),
            }
        })
        .style(|s| s.min_width(0.0).text_overflow(TextOverflow::Ellipsis)),
        icon("chevron-down", 12.0),
    ));
    let id = view.id();
    view.on_event_stop(EventListener::PointerDown, move |_| {
        if ui.running.get_untracked() {
            return;
        }
        let now = current(ui, which);
        ui.picker_at.set(options(ui, which).iter().position(|o| *o == now).unwrap_or(0));
        anchor.set(id.layout_rect());
        ui.picker.set(Some(which));
    })
    .style(move |s| {
        let c = t.get();
        let s = s
            .gap(2.0)
            .items_center()
            .min_width(0.0)
            .padding_horiz(6.0)
            .height(22.0)
            .border_radius(5.0)
            .font_size(12.0)
            .selectable(false)
            .apply_if(which != Picker::Model, |s| s.flex_shrink(0.0_f32));
        if ui.running.get() {
            s.color(c.desc.multiply_alpha(0.5))
        } else {
            let lit = ui.picker.get() == Some(which);
            s.color(c.desc)
                .cursor(CursorStyle::Pointer)
                .apply_if(lit, |s| s.background(c.toolbar_hover))
                .hover(|s| s.background(c.toolbar_hover))
        }
    })
}

/// The open picker's menu, as VS Code's: 26 px items, a check by the one in force, in a layer over the whole view
/// (so every item is within the view's bounds and takes the pointer), placed over the pill that opened it. The
/// menu takes the keys while it is open (the arrows, Enter, Escape) and closes on a press anywhere else: in the
/// view, on the layer; elsewhere in the window, by losing the keys. `frame` is the view, whose place in the window
/// is the layer's (a hidden layer has none), and `back` the input, which the keys go back to.
fn menu(ui: Ui, send: Send, anchor: RwSignal<Rect>, frame: ViewId, back: ViewId) -> impl IntoView {
    let t = ui.colors;
    let close = move || {
        ui.picker.set(None);
        back.request_focus();
    };
    let choose: Rc<dyn Fn(Picker, String)> = Rc::new(move |which, v| {
        close();
        send(chosen(which, &v));
    });
    let items = {
        let choose = choose.clone();
        dyn_stack(
            move || {
                let which = ui.picker.get();
                let all = which.map(|w| options(ui, w)).unwrap_or_default();
                all.into_iter().enumerate().filter_map(|(n, v)| Some((n, which?, v))).collect::<Vec<_>>()
            },
            |item: &(usize, Picker, String)| item.clone(),
            move |(n, which, v): (usize, Picker, String)| {
                let (choose, name, value) = (choose.clone(), named(which, &v), v.clone());
                h_stack((
                    icon("check", 16.0)
                        .style(move |s| s.apply_if(current(ui, which) != value, |s| s.color(Color::TRANSPARENT))),
                    label(move || name.clone()),
                ))
                // On the press: a press takes the keys from the menu, and so would close it before a click.
                .on_event_stop(EventListener::PointerDown, move |_| choose(which, v.clone()))
                .on_event_cont(EventListener::PointerEnter, move |_| ui.picker_at.set(n))
                .style(move |s| {
                    let c = t.get();
                    s.height(26.0)
                        .items_center()
                        .gap(6.0)
                        .padding_left(6.0)
                        .padding_right(20.0)
                        .margin_horiz(4.0)
                        .border_radius(4.0)
                        .color(c.menu_fg)
                        .cursor(CursorStyle::Pointer)
                        .apply_if(ui.picker_at.get() == n, |s| s.background(c.menu_selected).color(c.menu_selected_fg))
                })
            },
        )
        .style(|s| s.flex_col().min_width(MENU_WIDTH).padding_vert(4.0))
    };
    let step = move |by: isize| {
        let n = ui.picker.get_untracked().map(|w| options(ui, w).len()).unwrap_or(0) as isize;
        if n > 0 {
            ui.picker_at.update(|at| *at = (*at as isize + by).rem_euclid(n) as usize);
        }
    };
    let behind = empty().on_event_stop(EventListener::PointerDown, move |_| close()).style(|s| s.size_full());
    let list = scroll(items)
        .keyboard_navigable()
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| close())
        .on_key_down(Key::Named(NamedKey::ArrowDown), |_| true, move |_| step(1))
        .on_key_down(Key::Named(NamedKey::ArrowUp), |_| true, move |_| step(-1))
        .on_key_down(
            Key::Named(NamedKey::Enter),
            |_| true,
            move |_| {
                if let Some(which) = ui.picker.get_untracked()
                    && let Some(v) = options(ui, which).get(ui.picker_at.get_untracked())
                {
                    choose(which, v.clone());
                }
            },
        )
        .on_event_cont(EventListener::FocusLost, move |_| ui.picker.set(None))
        .style(move |s| {
            let c = t.get();
            // Over the pill, its left at the pill's, and within the view on every side.
            let (pill, view) = (anchor.get(), frame.layout_rect());
            let left = (pill.x0 - view.x0).min(view.width() - MENU_WIDTH - 12.0).max(4.0);
            s.position(Position::Absolute)
                .inset_left(left)
                .inset_bottom(view.y1 - pill.y0 + 4.0)
                .max_height((pill.y0 - view.y0 - 12.0).clamp(60.0, 320.0))
                .border(1.0)
                .border_color(c.menu_border)
                .border_radius(6.0)
                .background(c.menu_bg)
                .box_shadow_blur(8.0)
                .box_shadow_color(c.shadow)
                .font_size(13.0)
                .selectable(false)
        });
    let id = list.id();
    create_effect(move |_| {
        if ui.picker.get().is_some() {
            id.request_focus();
        }
    });
    stack((behind, list)).style(move |s| {
        s.position(Position::Absolute).inset(0.0).z_index(100).apply_if(ui.picker.get().is_none(), |s| s.hide())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menus_names_begin_with_a_capital() {
        assert_eq!(capital("auto-edit"), "Auto-edit");
        assert_eq!(capital("xhigh"), "Xhigh");
        assert_eq!(capital(""), "");
    }
}
