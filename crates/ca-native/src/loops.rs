//! Loops, as the old Code Arena's: a prompt run as a background agent every so often while the session is open
//! (`/loop`), its report reaching the agent with the person's next message. "Loops…" (the command palette, or the
//! bar over the input while one runs) opens the form: the loops listed ("1 · every 1h · may change files · 3 runs,
//! 1 skipped · last run ok"), each with its stop, and one added (what each run does, how often, whether it may change
//! files). The loops themselves are the driver's (`loops`, `loop_add`, `loop_stop`).

use std::time::Duration;

use floem::action::exec_after;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, dyn_stack, h_stack, h_stack_from_iter, label, tooltip, v_stack};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::commands::{self, command};
use crate::icons::icon;
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::widgets::form::{self, Form};

/// "Loops…".
pub const LOOPS: &str = "codeArena.chat.loops";

/// How often a loop may run, as the form offers it.
const EVERY: [(&str, &str); 5] =
    [("15m", "15 minutes"), ("30m", "30 minutes"), ("1h", "1 hour"), ("4h", "4 hours"), ("1d", "1 day")];

/// A loop as the driver lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loop {
    pub id: u64,
    pub every: String,
    pub prompt: String,
    pub edits: bool,
    pub runs: u64,
    pub skipped: u64,
    /// How its last run ended: "ok", "failed", or none yet.
    pub last: Option<String>,
}

impl Loop {
    pub fn of(v: &Value) -> Loop {
        let n = |k: &str| v[k].as_u64().unwrap_or(0);
        Loop {
            id: n("id"),
            every: v["every_words"].as_str().unwrap_or("").to_string(),
            prompt: v["prompt"].as_str().unwrap_or("").to_string(),
            edits: v["edits"].as_bool().unwrap_or(false),
            runs: n("runs"),
            skipped: n("skipped"),
            last: v["last"].as_str().map(str::to_string),
        }
    }

    /// "1 · every 1h · may change files · 3 runs, 1 skipped · last run ok".
    pub fn line(&self) -> String {
        let mut s = format!("{} · every {}", self.id, self.every);
        if self.edits {
            s.push_str(" · may change files");
        }
        s.push_str(&format!(" · {} {}", self.runs, if self.runs == 1 { "run" } else { "runs" }));
        if self.skipped > 0 {
            s.push_str(&format!(", {} skipped", self.skipped));
        }
        if let Some(last) = &self.last {
            s.push_str(&format!(" · last run {last}"));
        }
        s
    }
}

/// The loops listed, and the form's fields.
#[derive(Clone, Copy)]
struct Loops {
    items: RwSignal<Vec<Loop>>,
    form: Form,
    prompt: RwSignal<String>,
    every: RwSignal<&'static str>,
    edits: RwSignal<bool>,
}

impl crate::ext::State for Loops {
    fn new(ui: Ui) -> Self {
        Loops {
            items: ui.scope.create_rw_signal(Vec::new()),
            form: Form::new(ui),
            prompt: ui.scope.create_rw_signal(String::new()),
            every: ui.scope.create_rw_signal("1h"),
            edits: ui.scope.create_rw_signal(false),
        }
    }
}

/// The form opened, the loops asked for again (and every few seconds while it is open, for their runs).
fn open(ui: Ui, send: &Send) {
    let st = ui.state::<Loops>();
    send(json!({"type": "loops"}));
    let was = st.form.open.get_untracked();
    st.form.show(None);
    if !was {
        refresh(ui, send.clone());
    }
}

fn refresh(ui: Ui, send: Send) {
    exec_after(Duration::from_secs(5), move |_| {
        if ui.state::<Loops>().form.open.get_untracked() {
            send(json!({"type": "loops"}));
            refresh(ui, send);
        }
    });
}

/// The loop typed added.
fn add(ui: Ui, send: &Send) {
    let st = ui.state::<Loops>();
    let prompt = st.prompt.get_untracked().trim().to_string();
    if prompt.is_empty() {
        return;
    }
    send(
        json!({"type": "loop_add", "every": st.every.get_untracked(), "prompt": prompt, "edits": st.edits.get_untracked()}),
    );
}

/// A loop in the form's list: its prompt, its line under it, and its stop.
fn row(ui: Ui, send: Send, l: Loop) -> impl IntoView {
    let t = ui.colors;
    let (prompt, line, id) = (l.prompt.clone(), l.line(), l.id);
    h_stack((
        icon("sync", 16.0).style(move |s| s.color(t.get().desc).margin_top(1.0).flex_shrink(0.0_f32)),
        v_stack((
            label(move || prompt.clone()).style(|s| s.text_overflow(TextOverflow::Wrap)),
            label(move || line.clone()).style(move |s| s.font_size(12.0).color(t.get().desc)),
        ))
        .style(|s| s.gap(2.0).flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0)),
        tooltip(crate::widgets::action(t, "close", move || send(json!({"type": "loop_stop", "id": id}))), move || {
            label(move || format!("Stop loop {id}"))
        }),
    ))
    .style(move |s| {
        s.gap(8.0)
            .items_start()
            .width_full()
            .padding_horiz(10.0)
            .padding_vert(8.0)
            .border(1.0)
            .border_radius(4.0)
            .border_color(t.get().widget_border)
    })
}

fn host(ui: Ui, send: Send) -> AnyView {
    let (t, st) = (ui.colors, ui.state::<Loops>());
    let f = st.form;
    let list = {
        let send = send.clone();
        dyn_stack(
            move || st.items.get(),
            |l: &Loop| format!("{}|{}|{}|{:?}", l.id, l.runs, l.skipped, l.last),
            move |l| row(ui, send.clone(), l),
        )
        .style(move |s| s.flex_col().gap(6.0).width_full().apply_if(st.items.with(Vec::is_empty), |s| s.hide()))
        .into_any()
    };
    let go = {
        let send = send.clone();
        move || add(ui, &send)
    };
    let prompt = f.named(
        ui,
        "Each run",
        f.input(ui, st.prompt, "Run the tests; if one fails, find why and say so.", go.clone()),
    );
    let chips = h_stack_from_iter(EVERY.iter().map(|&(value, words)| {
        label(move || words)
            .on_click_stop(move |_| {
                if st.every.get_untracked() != value {
                    st.every.set(value);
                }
            })
            .style(move |s| {
                let c = t.get();
                let on = st.every.get() == value;
                s.padding_horiz(8.0)
                    .padding_vert(3.0)
                    .border(1.0)
                    .border_radius(4.0)
                    .font_size(12.0)
                    .selectable(false)
                    .cursor(CursorStyle::Pointer)
                    .border_color(if on { c.focus } else { c.input_border })
                    .background(if on { c.button_bg } else { c.input_bg })
                    .color(if on { c.button_fg } else { c.input_fg })
            })
    }))
    .style(|s| s.gap(4.0).flex_wrap(floem::style::FlexWrap::Wrap));
    let every = h_stack((label(|| "Every").style(|s| s.font_size(12.0)), chips)).style(|s| s.gap(8.0).items_center());
    let edits = f.checkbox(
        ui,
        st.edits,
        "It may change files",
        "Its commands are refused all the same: nobody is there to approve them.",
    );
    let ready = move || st.prompt.with(|p| !p.trim().is_empty());
    let buttons = form::buttons(vec![
        form::button(ui, || "Close".into(), false, || true, move || f.close()),
        form::button(ui, || "Add the loop".into(), true, ready, go),
    ]);
    f.host(
        ui,
        || "Loops".into(),
        || {
            "A prompt run as a background agent every so often while this session is open: its report reaches the \
             agent with your next message."
                .into()
        },
        vec![list, prompt, every.into_any(), edits],
        buttons,
    )
}

/// The bar over the input while loops run: how many, and a click opens the form.
fn bar(ui: Ui, send: Send) -> impl IntoView {
    let (t, st) = (ui.colors, ui.state::<Loops>());
    let n = create_memo(move |_| st.items.with(Vec::len));
    h_stack((
        icon("sync", 14.0),
        label(move || if n.get() == 1 { "1 loop runs".to_string() } else { format!("{} loops run", n.get()) })
            .style(|s| s.font_size(12.0).flex_grow(1.0_f32)),
        label(|| "Loops…").style(move |s| s.font_size(12.0).color(t.get().link)),
    ))
    .on_click_stop(move |_| open(ui, &send))
    .style(move |s| {
        crate::agent::bar(t, s)
            .gap(6.0)
            .items_center()
            .cursor(CursorStyle::Pointer)
            .apply_if(n.get() == 0, |s| s.hide())
    })
}

/// What it adds to the window's registries, once at start: "Loops…", its form over the window, and its bar over
/// the input.
pub fn register(_ui: Ui, _send: &Send) {
    commands::add(command(LOOPS, "Chat", "Loops…", open));
    layers::add(Place::Over, "loops.form", 30, |ui, send| host(ui, send).into_any());
    crate::agent::slots::add(crate::agent::slots::Slot::Bars, "loops", 40, |ui, send| bar(ui, send).into_any());
}

/// The session's messages that are this part's, taken in: the loops listed, and what adding or stopping one said.
/// True takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let st = ui.state::<Loops>();
    match m["type"].as_str().unwrap_or("") {
        "loops" => {
            let items: Vec<Loop> = m["items"].as_array().map(|a| a.iter().map(Loop::of).collect()).unwrap_or_default();
            if st.items.with_untracked(|i| *i != items) {
                st.items.set(items);
            }
            true
        }
        "loop_add" | "loop_stop" => {
            if let Some(e) = m["error"].as_str() {
                crate::widgets::toast::error(ui, e);
            } else if m["type"] == "loop_add" {
                st.prompt.set(String::new());
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loop_says_how_often_what_it_may_do_and_how_its_runs_went() {
        let l =
            Loop::of(&json!({"id": 2, "every": 3600, "every_words": "1h", "prompt": "run the tests", "edits": true,
                                 "runs": 3, "skipped": 1, "last": "ok"}));
        assert_eq!(l.line(), "2 · every 1h · may change files · 3 runs, 1 skipped · last run ok");
        let first = Loop::of(&json!({"id": 1, "every_words": "15m", "runs": 1, "last": null}));
        assert_eq!(first.line(), "1 · every 15m · 1 run");
    }
}
