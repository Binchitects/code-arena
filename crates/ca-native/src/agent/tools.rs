//! An answer's calls, each a card that opens, as VS Code's chat draws a tool's run: its tool's icon, its title and
//! main argument, and where it stands ("Waiting for you", "Running" with its seconds and a bar, "Not allowed",
//! "Failed", "Not run", or its count and time). Opened, it says what it was asked with and what it gave. Ended calls
//! in a row fold into one line ("Used 5 tools · read_file ×3, grep, run_shell"), and a read or an edit has a chip of
//! its file under it. What each says is worked out in `calls.rs`.

use std::time::{Duration, Instant};

use floem::AnyView;
use floem::IntoView;
use floem::action::exec_after;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, dyn_container, dyn_stack, empty, h_stack, label, v_stack, v_stack_from_iter};
use serde_json::Value;

use super::calls::{self, Chip, Group, Shown, State};
use crate::icons::icon;
use crate::model::{Kind, Progress, Tool};
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, bold, caps};

/// How often a running call's seconds, spinner and bar move.
const TICK: Duration = Duration::from_millis(100);

/// After how long a call that says nothing of its progress shows a bar that moves.
const BAR_AFTER: Duration = Duration::from_millis(1500);

/// The most lines of a result's places or rows drawn.
const MOST_ROWS: usize = 200;

/// An answer's calls: their groups, each a card or a folded line.
pub(super) fn list(ui: Ui, send: Send, tools: RwSignal<Vec<Tool>>) -> impl IntoView {
    let running = ui.running;
    let groups = create_memo(move |_| tools.with(|ts| calls::groups(ts, running.get())));
    let tick = clock(ui, tools);
    dyn_stack(
        move || groups.get(),
        |g: &Group| match g {
            Group::One(n) => (*n, false),
            Group::Fold { start, .. } => (*start, true),
        },
        move |g| match g {
            Group::One(n) => card(ui, send.clone(), tools, n, tick).into_any(),
            Group::Fold { start, .. } => fold(ui, send.clone(), tools, groups, start, tick).into_any(),
        },
    )
    .style(|s| s.flex_col().gap(6.0).width_full().min_width(0.0))
}

/// A count that moves while one of the calls runs, and stands still otherwise. Its signals are the window's, as
/// are the calls', so a timer still pending after the answer is gone finds them.
fn clock(ui: Ui, tools: RwSignal<Vec<Tool>>) -> RwSignal<u64> {
    let tick = ui.scope.create_rw_signal(0u64);
    let generation = ui.scope.create_rw_signal(0u64);
    let busy = move || {
        ui.running.get_untracked() && tools.with_untracked(|ts| ts.iter().any(|t| t.began.is_some() && !t.done))
    };
    fn go(tick: RwSignal<u64>, generation: RwSignal<u64>, g: u64, busy: impl Fn() -> bool + Copy + 'static) {
        exec_after(TICK, move |_| {
            if generation.get_untracked() == g && busy() {
                tick.update(|n| *n += 1);
                go(tick, generation, g, busy);
            }
        });
    }
    create_effect(move |was: Option<bool>| {
        let now = ui.running.get() && tools.with(|ts| ts.iter().any(|t| t.began.is_some() && !t.done));
        if now && was != Some(true) {
            generation.update(|g| *g += 1);
            go(tick, generation, generation.get_untracked(), busy);
        }
        now
    });
    tick
}

/// A folded run of ended calls: its line, and its cards when opened.
fn fold(
    ui: Ui,
    send: Send,
    tools: RwSignal<Vec<Tool>>,
    groups: Memo<Vec<Group>>,
    start: usize,
    tick: RwSignal<u64>,
) -> impl IntoView {
    let t = ui.colors;
    let open = create_rw_signal_here(false);
    let len = create_memo(move |_| {
        groups.with(|gs| {
            gs.iter()
                .find_map(|g| match g {
                    Group::Fold { start: s, len } if *s == start => Some(*len),
                    _ => None,
                })
                .unwrap_or(0)
        })
    });
    let line = create_memo(move |_| {
        let n = len.get();
        tools.with(|ts| ts.get(start..start + n).map(calls::fold_line).unwrap_or_default())
    });
    let failed = create_memo(move |_| {
        let n = len.get();
        tools.with(|ts| ts.iter().skip(start).take(n).any(|t| t.done && !t.ok))
    });
    v_stack((
        h_stack((
            dyn_container(move || open.get(), |o| icon(if o { "chevron-down" } else { "chevron-right" }, 16.0)),
            icon("tools", 14.0),
            label(move || line.get()).style(|s| s.text_ellipsis().min_width(0.0)),
            icon("error", 14.0).style(move |s| s.color(t.get().error).apply_if(!failed.get(), |s| s.hide())),
        ))
        .on_click_stop(move |_| open.update(|o| *o = !*o))
        .style(move |s| {
            s.gap(6.0).items_center().color(t.get().desc).cursor(CursorStyle::Pointer).selectable(false).min_width(0.0)
        }),
        dyn_stack(
            move || if open.get() { (start..start + len.get()).collect::<Vec<_>>() } else { vec![] },
            |n: &usize| *n,
            move |n| card(ui, send.clone(), tools, n, tick),
        )
        .style(|s| s.flex_col().gap(6.0).padding_left(20.0).width_full().min_width(0.0)),
    ))
    .style(|s| s.gap(6.0).width_full().min_width(0.0))
}

/// A signal of the view being built (gone with it).
fn create_rw_signal_here<T: 'static>(v: T) -> RwSignal<T> {
    floem::reactive::create_rw_signal(v)
}

/// One call's card: its head, its bar while it runs, what it was asked with and gave when opened, and its file's
/// chip under it.
fn card(ui: Ui, send: Send, tools: RwSignal<Vec<Tool>>, n: usize, tick: RwSignal<u64>) -> impl IntoView {
    let t = ui.colors;
    let running = ui.running;
    let me = create_memo(move |_| tools.with(|ts| ts.get(n).cloned().unwrap_or_default()));
    let st = create_memo(move |_| me.with(|m| calls::state(m, running.get())));
    let open = create_rw_signal_here(false);
    let shown = create_memo(move |_| open.get() || st.get() == State::Waiting);
    let elapsed = move || me.with(|m| m.began.map(|b| b.elapsed()).unwrap_or_default());
    let status = create_memo(move |_| {
        let s = st.get();
        if s == State::Running {
            tick.with(|_| ());
        }
        me.with(|m| calls::status(m, s, elapsed().as_secs()))
    });
    let title = create_memo(move |_| me.with(|m| (m.name.clone(), calls::main_argument(m))));
    let head = h_stack((
        dyn_container(move || shown.get(), |o| icon(if o { "chevron-down" } else { "chevron-right" }, 16.0)),
        dyn_container(move || me.with(|m| calls::icon_of(&m.name)), |name| icon(name, 14.0)),
        label(move || title.get().0).style(move |s| s.color(t.get().fg).flex_shrink(0.0_f32)),
        label(move || title.get().1).style(|s| {
            s.font_family(MONO_FONT.clone()).font_size(12.0).text_ellipsis().min_width(0.0).flex_shrink(1.0_f32)
        }),
        empty().style(|s| s.flex_grow(1.0_f32)),
        label(move || status.get()).style(|s| s.flex_shrink(0.0_f32).font_size(12.0)),
        state_icon(ui, st, tick),
    ))
    .on_click_stop(move |_| open.update(|o| *o = !*o))
    .style(move |s| {
        let c = t.get();
        s.gap(6.0)
            .items_center()
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .width_full()
            .min_width(0.0)
            .color(c.desc)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.list_hover))
    });
    // The body is made again when it is opened or closed and when the call ends: not as the others' calls change.
    let body_key = create_memo(move |_| (shown.get(), me.with(|m| (m.done, m.status.clone(), m.text.len()))));
    let body_send = send.clone();
    let body = dyn_container(
        move || body_key.get(),
        move |(open, _)| {
            if open { body(ui, body_send.clone(), me.get_untracked(), st.get_untracked()) } else { empty().into_any() }
        },
    )
    .style(|s| s.width_full().min_width(0.0));
    // An edit with its diff has the diff's path in place of the chip; an answer's one edit, the diff alone.
    let chip_of = create_memo(move |_| me.with(|m| if m.ok && m.diff.is_some() { None } else { Chip::of(m) }));
    let lone = create_memo(move |_| tools.with(|ts| super::diffs::lone(ts)));
    let diff_send = send.clone();
    v_stack((
        v_stack((head, bar(ui, me, st, tick), body)).style(move |s| {
            let c = t.get();
            s.width_full()
                .min_width(0.0)
                .border(1.0)
                .border_color(c.widget_border)
                .border_radius(4.0)
                .apply_if(lone.get(), |s| s.hide())
        }),
        dyn_container(
            move || chip_of.get(),
            move |c| match c {
                Some(c) => chip(ui, send.clone(), c).into_any(),
                None => empty().into_any(),
            },
        ),
        super::diffs::under(ui, diff_send, me),
    ))
    .style(|s| s.gap(4.0).width_full().min_width(0.0))
}

/// The icon at a card's end: a spinner while it runs, a check, a cross, a "no".
fn state_icon(ui: Ui, st: Memo<State>, tick: RwSignal<u64>) -> impl IntoView {
    let t = ui.colors;
    dyn_container(
        move || st.get(),
        move |s| {
            let name = match s {
                State::Waiting => "question",
                State::Running => "loading",
                State::Done => "check",
                State::Failed => "error",
                State::Denied | State::NotRun => "circle-slash",
            };
            icon(name, 14.0)
                .style(move |st| {
                    let c = t.get();
                    let st = match s {
                        State::Done => st.color(c.ok),
                        State::Failed => st.color(c.error),
                        _ => st,
                    };
                    // A spinner turns, a twelfth of a turn each tick.
                    st.apply_if(s == State::Running, |st| {
                        st.rotate((tick.get() % 12) as f64 * std::f64::consts::TAU / 12.0)
                    })
                })
                .into_any()
        },
    )
}

/// The bar under a running call's head: how far it has got when it says, else one that moves once it has run a
/// while.
fn bar(ui: Ui, me: Memo<Tool>, st: Memo<State>, tick: RwSignal<u64>) -> impl IntoView {
    let t = ui.colors;
    let part = move || -> Option<(f64, f64)> {
        if st.get() != State::Running {
            return None;
        }
        me.with(|m| match &m.progress {
            Some(Progress { done, total, .. }) if *total > 0 => {
                Some((0.0, (*done as f64 / *total as f64).min(1.0) * 100.0))
            }
            _ if m.began.is_some_and(|b| b.elapsed() >= BAR_AFTER) => {
                tick.with(|_| ());
                // A third of the width, going across and back in from the left.
                let at = (tick.get_untracked() % 20) as f64 / 20.0;
                Some((at * 130.0 - 30.0, 30.0))
            }
            _ => {
                tick.with(|_| ());
                None
            }
        })
    };
    let part = create_memo(move |_| part());
    empty().style(move |s| {
        let c = t.get();
        match part.get() {
            Some((left, width)) => s
                .height(2.0)
                .width_pct(width.min(100.0 - left.max(0.0)) + left.min(0.0))
                .margin_left(floem::unit::PxPct::Pct(left.max(0.0)))
                .background(c.progress),
            None => s.hide(),
        }
    })
}

/// The chip under a read or an edit: its file and lines, which open them.
fn chip(ui: Ui, send: Send, c: Chip) -> impl IntoView {
    let t = ui.colors;
    let words = c.text();
    h_stack((icon("file", 14.0), label(move || words.clone()).style(|s| s.font_size(12.0).text_ellipsis())))
        .on_click_stop(move |_| {
            if c.first > 0 {
                ui.open_at(&c.path, c.first, &send);
            } else {
                ui.open(&c.path, &send);
            }
        })
        .style(move |s| {
            let c = t.get();
            s.gap(4.0)
                .items_center()
                .padding_horiz(6.0)
                .padding_vert(1.0)
                .margin_left(8.0)
                .border(1.0)
                .border_color(c.widget_border)
                .border_radius(4.0)
                .color(c.link)
                .cursor(CursorStyle::Pointer)
                .max_width_full()
                .hover(|s| s.background(c.list_hover))
        })
}

/// A card opened: the question when it waits, what it was asked with, and what it gave.
fn body(ui: Ui, send: Send, tool: Tool, st: State) -> AnyView {
    let t = ui.colors;
    let heading =
        move |words: &'static str| label(move || words).style(move |s| caps(s).font_size(11.0).color(t.get().desc));
    let mut parts: Vec<AnyView> = Vec::new();
    if st == State::Waiting {
        let ask = format!("Allow {} to run with these arguments?", tool.name);
        parts.push(label(move || ask.clone()).style(move |s| bold(s).color(t.get().fg)).into_any());
    }
    let asked = calls::asked(&tool.arguments);
    if asked != calls::Asked::default() {
        parts.push(heading("Asked with").into_any());
        for (k, v) in asked.short {
            parts.push(
                h_stack((
                    label(move || k.clone()).style(move |s| s.color(t.get().desc).flex_shrink(0.0_f32)),
                    label(move || v.clone()).style(|s| {
                        s.font_family(MONO_FONT.clone())
                            .font_size(12.0)
                            .text_overflow(TextOverflow::Wrap)
                            .min_width(0.0)
                    }),
                ))
                .style(|s| s.gap(8.0).min_width(0.0))
                .into_any(),
            );
        }
        for (k, v) in asked.code {
            let lang = calls::lang_of(&tool, &k);
            parts.push(label(move || k.clone()).style(move |s| s.color(t.get().desc)).into_any());
            parts.push(crate::prose::code(ui, &lang, "", move || v.clone()).into_any());
        }
        if let Some(json) = asked.rest {
            parts.push(crate::prose::code(ui, "json", "", move || json.clone()).into_any());
        }
    }
    if tool.done && !tool.text.is_empty() {
        parts.push(heading("Result").into_any());
        parts.push(result(ui, send, &tool));
        if tool.cut {
            parts.push(label(|| "…").style(move |s| s.color(t.get().desc)).into_any());
        }
    }
    v_stack_from_iter(parts)
        .style(move |s| {
            let c = t.get();
            s.gap(6.0).padding(8.0).width_full().min_width(0.0).border_top(1.0).border_color(c.widget_border)
        })
        .into_any()
}

/// A call's result, by what it holds.
fn result(ui: Ui, send: Send, tool: &Tool) -> AnyView {
    let t = ui.colors;
    let mono = move |text: String, console: bool| {
        label(move || text.clone()).style(move |s| {
            let c = t.get();
            s.font_family(MONO_FONT.clone())
                .font_size(12.0)
                .padding(8.0)
                .border_radius(4.0)
                .width_full()
                .min_width(0.0)
                .background(c.code)
                .color(if console { c.fg } else { c.desc })
                .text_overflow(TextOverflow::Wrap)
        })
    };
    match calls::shown(tool) {
        Shown::File { text, lang, .. } => crate::prose::code(ui, &lang, "", move || text.clone()).into_any(),
        Shown::Console(text) => mono(text, true).into_any(),
        Shown::Raw(text) => mono(text, false).into_any(),
        Shown::Locations(all) => v_stack_from_iter(all.into_iter().take(MOST_ROWS).map(|l| {
            let send = send.clone();
            let place = if l.line > 0 { format!("{}:{}", l.path, l.line) } else { l.path.clone() };
            let text = l.text.clone();
            h_stack((
                label(move || place.clone()).style(move |s| s.color(t.get().link).flex_shrink(0.0_f32)),
                label(move || text.clone())
                    .style(|s| s.font_family(MONO_FONT.clone()).font_size(12.0).text_ellipsis().min_width(0.0)),
            ))
            .on_click_stop(move |_| {
                if l.line > 0 {
                    ui.open_at(&l.path, l.line, &send);
                } else {
                    ui.open(&l.path, &send);
                }
            })
            .style(move |s| {
                s.gap(8.0).min_width(0.0).cursor(CursorStyle::Pointer).hover(|s| s.background(t.get().list_hover))
            })
        }))
        .style(|s| s.width_full().min_width(0.0))
        .into_any(),
        Shown::Table(table) => rows(ui, table).into_any(),
    }
}

/// A result's records as a table whose columns sort it: once up, again down.
fn rows(ui: Ui, table: calls::Table) -> impl IntoView {
    let t = ui.colors;
    let sort = create_rw_signal_here(None::<(usize, bool)>);
    let columns = table.columns.clone();
    let all = table.rows;
    let cell = move |text: String, head: bool| {
        label(move || text.clone()).style(move |s| {
            let s = s.width(140.0).padding_horiz(6.0).padding_vert(2.0).text_ellipsis();
            if head { bold(s) } else { s.font_family(MONO_FONT.clone()).font_size(12.0) }
        })
    };
    let head = floem::views::h_stack_from_iter(columns.iter().enumerate().map(|(i, name)| {
        let name = name.clone();
        let mark = move || match sort.get() {
            Some((c, down)) if c == i => {
                if down {
                    " ↓"
                } else {
                    " ↑"
                }
            }
            _ => "",
        };
        label(move || format!("{name}{}", mark()))
            .on_click_stop(move |_| {
                sort.update(|s| {
                    *s = Some(match *s {
                        Some((c, false)) if c == i => (i, true),
                        _ => (i, false),
                    })
                })
            })
            .style(move |s| {
                bold(s).width(140.0).padding_horiz(6.0).padding_vert(2.0).text_ellipsis().cursor(CursorStyle::Pointer)
            })
    }))
    .style(move |s| s.border_bottom(1.0).border_color(t.get().widget_border));
    let body = dyn_container(
        move || sort.get(),
        move |by| {
            let list = match by {
                Some((c, down)) => calls::sorted(&all, c, down),
                None => all.clone(),
            };
            v_stack_from_iter(
                list.into_iter()
                    .take(MOST_ROWS)
                    .map(|r| floem::views::h_stack_from_iter(r.into_iter().map(|v| cell(v, false)))),
            )
            .into_any()
        },
    );
    floem::views::scroll(v_stack((head, body))).style(|s| s.width_full().min_width(0.0))
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The call of an id, changed where it is: in the answer being written, or in one before the agent's question
/// that it waited on; false when there is none of it.
fn change(ui: Ui, id: &str, f: impl FnOnce(&mut Tool)) -> bool {
    if id.is_empty() {
        return false;
    }
    let held = ui.items.with_untracked(|l| {
        l.iter().rev().find_map(|i| match &i.kind {
            Kind::Assistant(a) if a.tools.with_untracked(|ts| ts.iter().any(|t| t.id == id && !t.done)) => {
                Some(a.tools)
            }
            _ => None,
        })
    });
    let Some(tools) = held else { return false };
    tools.update(|ts| {
        if let Some(t) = ts.iter_mut().rev().find(|t| t.id == id && !t.done) {
            f(t);
        }
    });
    true
}

/// The session's messages that are a call's (waiting, begun, going, ended), taken in. Never taken away from the
/// rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let id = m["id"].as_str().unwrap_or("");
    match m["type"].as_str().unwrap_or("") {
        "tool_waiting" => {
            if !change(ui, id, |t| t.status = "waiting".into())
                && let Kind::Assistant(answer) = ui.current().kind
            {
                let waiting =
                    Tool { status: "waiting".into(), ok: true, text: String::new(), ..Tool::from_json(m, false) };
                answer.tools.update(|t| t.push(waiting));
            }
        }
        "tool_start" => {
            let said = Tool::from_json(m, false);
            let began = Some(Instant::now());
            let found = change(ui, id, |t| {
                t.status = "running".into();
                t.began = began;
                if !said.arguments.is_null() {
                    t.arguments = said.arguments.clone();
                }
                if !said.subject.is_empty() {
                    t.subject = said.subject.clone();
                }
            });
            if !found && let Kind::Assistant(answer) = ui.current().kind {
                let started = Tool { ok: true, text: String::new(), status: "running".into(), began, ..said };
                answer.tools.update(|t| t.push(started));
            }
        }
        "tool_progress" => {
            let progress = Progress {
                done: m["progress"].as_f64().unwrap_or(0.0).max(0.0) as u64,
                total: m["total"].as_f64().unwrap_or(0.0).max(0.0) as u64,
                message: m["message"].as_str().unwrap_or("").to_string(),
            };
            change(ui, id, |t| t.progress = Some(progress));
        }
        "tool_end" => {
            // The call as it ended: what the session says of it now, over what it said when it began (its
            // arguments kept when the end says none).
            let ended = Tool { ok: m["ok"] == true, ..Tool::from_json(m, true) };
            let end = |t: &mut Tool| {
                let arguments = std::mem::take(&mut t.arguments);
                *t = ended.clone();
                if t.arguments.is_null() {
                    t.arguments = arguments;
                }
            };
            // The call it ends: the one of its id, wherever it is; for a session that does not number its calls,
            // the last one of its name and subject in the answer being written that has not ended.
            if !change(ui, id, end)
                && let Kind::Assistant(answer) = ui.current().kind
            {
                answer.tools.update(|ts| {
                    let same =
                        |t: &Tool| id.is_empty() && !t.done && t.name == ended.name && t.subject == ended.subject;
                    match ts.iter_mut().rev().find(|t| same(t)) {
                        Some(t) => end(t),
                        None => ts.push(ended.clone()),
                    }
                });
            }
        }
        "turn_end" => ui.items.with_untracked(|l| {
            for i in l {
                if let Kind::Assistant(a) = &i.kind
                    && a.tools.with_untracked(|ts| ts.iter().any(|t| !t.done))
                {
                    a.tools.update(|ts| ts.iter_mut().for_each(settle));
                }
            }
        }),
        _ => {}
    }
    false
}

/// A call not ended when its turn ended, said as one that did not run: so the next turn's start does not make it
/// run again.
fn settle(t: &mut Tool) {
    if !t.done {
        t.done = true;
        t.ok = false;
        t.status = "skipped".into();
        t.began = None;
    }
}

/// A kept step's calls, as the session says them: each with what it kept of it (`tool_calls`), else by their
/// names alone (`tools`).
/// One that has not ended in a turn that has (`running` false) did not run.
pub(super) fn kept(msg: &Value, running: bool) -> Vec<Tool> {
    match msg["tool_calls"].as_array() {
        Some(all) => all
            .iter()
            .map(|v| {
                let mut t = Tool::from_json(v, v["done"] == true);
                if !running {
                    settle(&mut t);
                }
                t
            })
            .collect(),
        None => msg["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t.as_str())
            .map(|n| Tool { name: n.into(), done: true, ok: true, ..Tool::default() })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_call_waits_then_runs_goes_and_ends_in_one_card() {
        let ui = Ui::new();
        let send: Send = std::rc::Rc::new(|_| {});
        ui.push(Kind::assistant(ui.scope));
        let said = |v: Value| take(ui, &v, &send);
        said(
            json!({"type": "tool_waiting", "id": "c1", "name": "run_shell", "subject": "ls", "arguments": {"command": "ls"}}),
        );
        said(
            json!({"type": "tool_start", "id": "c1", "name": "run_shell", "subject": "ls", "arguments": {"command": "ls"}}),
        );
        said(json!({"type": "tool_progress", "id": "c1", "progress": 2.0, "total": 5.0, "message": ""}));
        let Kind::Assistant(answer) = ui.current().kind else { panic!("an answer") };
        let now = answer.tools.get_untracked();
        assert_eq!(now.len(), 1);
        assert!(now[0].began.is_some() && now[0].status == "running");
        assert_eq!(now[0].progress.as_ref().map(|p| (p.done, p.total)), Some((2, 5)));
        said(
            json!({"type": "tool_end", "id": "c1", "name": "run_shell", "subject": "ls", "ok": true, "text": "a", "ms": 40, "status": "done"}),
        );
        let now = answer.tools.get_untracked();
        assert_eq!(
            (now.len(), now[0].done, now[0].ms, now[0].arguments["command"].clone()),
            (1, true, 40, json!("ls"))
        );
    }

    #[test]
    fn a_call_asked_about_runs_and_ends_in_its_card_though_the_question_came_after_it() {
        let ui = Ui::new();
        let send: Send = std::rc::Rc::new(|_| {});
        let said = |v: Value| take(ui, &v, &send);
        said(json!({"type": "tool_waiting", "id": "c1", "name": "edit_file", "subject": "a.rs"}));
        ui.push(Kind::Notice("the question".into()));
        said(json!({"type": "tool_start", "id": "c1", "name": "edit_file", "subject": "a.rs"}));
        said(json!({"type": "tool_end", "id": "c1", "name": "edit_file", "subject": "a.rs", "ok": true, "text": "ok"}));
        let answers: Vec<Vec<Tool>> = ui.items.with_untracked(|l| {
            l.iter()
                .filter_map(|i| if let Kind::Assistant(a) = &i.kind { Some(a.tools.get_untracked()) } else { None })
                .collect()
        });
        assert_eq!(answers.len(), 1, "no answer is begun for the call's start");
        assert!(answers[0][0].done && answers[0][0].ok);
    }

    #[test]
    fn a_kept_step_reads_its_calls_and_an_older_one_their_names() {
        let step = json!({"tools": ["grep"], "tool_calls": [
            {"id": "a", "name": "grep", "arguments": {"pattern": "x"}, "done": true, "ok": true, "text": "a:1: x", "ms": 5},
            {"id": "b", "name": "read_file", "done": false},
        ]});
        let calls = kept(&step, true);
        assert_eq!((calls[0].ms, calls[0].done, calls[1].done), (5, true, false));
        // In a turn that ended, one not ended did not run, and stays so when another turn runs.
        let calls = kept(&step, false);
        assert_eq!(calls::state(&calls[1], true), State::NotRun);
        assert_eq!(calls::state(&calls[0], true), State::Done);
        assert_eq!(kept(&json!({"tools": ["grep"]}), false)[0].name, "grep");
    }

    #[test]
    fn a_call_cut_off_by_its_turns_end_did_not_run_and_an_id_used_again_is_a_call_of_its_own() {
        let ui = Ui::new();
        let send: Send = std::rc::Rc::new(|_| {});
        let said = |v: Value| take(ui, &v, &send);
        said(json!({"type": "tool_start", "id": "call_0", "name": "grep", "subject": "x"}));
        said(json!({"type": "tool_end", "id": "call_0", "name": "grep", "subject": "x", "ok": true, "text": "a"}));
        said(json!({"type": "tool_start", "id": "call_0", "name": "run_shell", "subject": "make"}));
        said(json!({"type": "turn_end", "ok": false, "stopped": true}));
        let Kind::Assistant(answer) = ui.current().kind else { panic!("an answer") };
        let now = answer.tools.get_untracked();
        assert_eq!(now.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["grep", "run_shell"]);
        assert_eq!((now[0].text.as_str(), calls::state(&now[0], true)), ("a", State::Done));
        assert_eq!((calls::state(&now[1], true), now[1].began), (State::NotRun, None));
    }
}
