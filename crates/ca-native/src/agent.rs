//! The Agent view, drawn as VS Code's chat view: the requests in bubbles on the right, the answers in full width
//! with their calls in a line that opens, the agent's questions as cards, and the input box at the bottom with its
//! toolbar (the mode, the model, send or stop).

use floem::IntoView;
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{SignalGet, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, dyn_container, dyn_stack, empty, h_stack, label, scroll, text_input, v_stack};
use serde_json::json;

use crate::icons::icon;
use crate::model::{Item, Kind, Tool, summary};
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, action, bold, button};

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    v_stack((thread(ui, send.clone()), bars(ui, send.clone()), input(ui, send)))
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
            label(move || text.get()).style(move |s| {
                s.line_height(1.5)
                    .color(t.get().fg)
                    .text_overflow(TextOverflow::Wrap)
                    .apply_if(text.with(|t| t.is_empty()), |s| s.hide())
            }),
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
fn input(ui: Ui, send: Send) -> impl IntoView {
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
    let pill = move |text: Box<dyn Fn() -> String>| {
        h_stack((label(text),)).style(move |s| {
            let c = t.get();
            s.gap(2.0)
                .items_center()
                .padding_horiz(6.0)
                .height(22.0)
                .border_radius(5.0)
                .font_size(12.0)
                .color(c.desc)
                .selectable(false)
        })
    };
    v_stack((
        text_input(ui.draft)
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
            pill(Box::new(move || {
                let m = ui.mode.get();
                if m.is_empty() { "Agent".to_string() } else { m[..1].to_uppercase() + &m[1..] }
            })),
            pill(Box::new(move || ui.model.get())),
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
        .style(|s| s.gap(2.0).items_center().width_full()),
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
