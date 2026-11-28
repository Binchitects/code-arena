//! The commands with no time limit (and those in the background) above the box, at most a quarter of the window:
//! a header that says how many there are and how many run ("2 commands, 1 running") and folds them all, then each
//! job on a line of its own: a spinner while it runs or a dot after, "job N", its command, how it stands ("running,
//! no time limit", "running in the background", "exit code 0 after 3m 12s") and "Stop job N" while it runs. A job
//! opened shows the last 200 lines of its output, following the end while it runs; the newest one running is open.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use floem::IntoView;
use floem::action::exec_after;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, dyn_container, dyn_stack, empty, h_stack, label, scroll, v_stack};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, bold};

/// How often the spinners turn; the output of an open job that runs is asked for every [`ASK_EVERY`] ticks.
const TICK: Duration = Duration::from_millis(100);
const ASK_EVERY: u64 = 10;

/// A job as the session's tasks say it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Job {
    pub id: u64,
    pub command: String,
    pub background: bool,
    /// "running", "ended" or "stopped".
    pub status: String,
    pub exit: Option<i64>,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
}

impl Job {
    pub fn of(v: &Value) -> Job {
        let running = v["running"] == true;
        let status = v["status"].as_str().map(str::to_string);
        Job {
            id: v["id"].as_u64().unwrap_or(0),
            command: v["command"].as_str().unwrap_or("").to_string(),
            background: v["background"] == true,
            status: status.unwrap_or_else(|| if running { "running" } else { "ended" }.to_string()),
            exit: v["exit"].as_i64(),
            started_ms: v["started_ms"].as_u64().unwrap_or(0),
            ended_ms: v["ended_ms"].as_u64(),
        }
    }

    pub fn running(&self) -> bool {
        self.status == "running"
    }
}

/// The bar's own state: the jobs, whether all are folded, which are open, which were asked to stop, their output.
#[derive(Clone, Copy)]
struct Bar {
    jobs: RwSignal<Vec<Job>>,
    folded: RwSignal<bool>,
    open: RwSignal<BTreeSet<u64>>,
    stopping: RwSignal<BTreeSet<u64>>,
    output: RwSignal<HashMap<u64, String>>,
    /// The highest job seen, and the one opened by itself as the newest running (closed when a newer one starts).
    seen: RwSignal<u64>,
    auto: RwSignal<Option<u64>>,
    /// The spinners' count, and whether its timer runs.
    tick: RwSignal<u64>,
    ticking: RwSignal<bool>,
}

impl crate::ext::State for Bar {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        Bar {
            jobs: s.create_rw_signal(Vec::new()),
            folded: s.create_rw_signal(false),
            open: s.create_rw_signal(BTreeSet::new()),
            stopping: s.create_rw_signal(BTreeSet::new()),
            output: s.create_rw_signal(HashMap::new()),
            seen: s.create_rw_signal(0),
            auto: s.create_rw_signal(None),
            tick: s.create_rw_signal(0),
            ticking: s.create_rw_signal(false),
        }
    }
}

/// How tall the jobs' list may be in a window this tall: the bar is a quarter of it at most, its header included.
pub fn list_height(window: f64) -> f64 {
    (window * 0.25 - 40.0).max(60.0)
}

/// The header's words: "2 commands, 1 running".
pub fn header(jobs: &[Job]) -> String {
    let n = jobs.len();
    let running = jobs.iter().filter(|j| j.running()).count();
    format!("{n} command{}, {running} running", if n == 1 { "" } else { "s" })
}

/// How long a job took: "12s", "3m 12s", "1h 4m".
pub fn took(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

/// How a job stands, after its command.
pub fn standing(j: &Job) -> String {
    if j.running() {
        return if j.background { "running in the background" } else { "running, no time limit" }.to_string();
    }
    let after = j.ended_ms.map(|e| format!(" after {}", took(e.saturating_sub(j.started_ms)))).unwrap_or_default();
    match (j.status.as_str(), j.exit) {
        ("stopped", _) => format!("stopped{after}"),
        (_, Some(c)) => format!("exit code {c}{after}"),
        (_, None) => format!("ended by a signal{after}"),
    }
}

/// The job to open by itself: the newest one running that is newer than any seen before.
pub fn fresh(seen: u64, jobs: &[Job]) -> Option<u64> {
    jobs.iter().filter(|j| j.running() && j.id > seen).map(|j| j.id).max()
}

/// Asks for a job's output.
fn ask(send: &Send, id: u64) {
    send(json!({"type": "job_output", "id": id}));
}

/// Turns the spinners while a job runs, and asks for the output of those open and running every second.
fn clock(bar: Bar, send: Send) {
    if bar.ticking.get_untracked() {
        return;
    }
    bar.ticking.set(true);
    fn go(bar: Bar, send: Send) {
        exec_after(TICK, move |_| {
            if !bar.jobs.with_untracked(|js| js.iter().any(Job::running)) {
                bar.ticking.set(false);
                return;
            }
            bar.tick.update(|n| *n += 1);
            if bar.tick.get_untracked() % ASK_EVERY == 0 {
                let open = bar.open.get_untracked();
                let asked: Vec<u64> = bar.jobs.with_untracked(|js| {
                    js.iter().filter(|j| j.running() && open.contains(&j.id)).map(|j| j.id).collect()
                });
                if !bar.folded.get_untracked() {
                    for id in asked {
                        ask(&send, id);
                    }
                }
            }
            go(bar, send);
        });
    }
    go(bar, send);
}

pub(super) fn view(ui: Ui, send: Send) -> impl IntoView {
    let (t, bar) = (ui.colors, ui.state::<Bar>());
    let folded = create_memo(move |_| bar.folded.get());
    let chevron = dyn_container(
        move || folded.get(),
        |f| icon(if f { "chevron-right" } else { "chevron-down" }, 14.0).into_any(),
    );
    let head = h_stack((
        chevron,
        label(|| "Commands with no time limit").style(move |s| bold(s).font_size(12.0).flex_shrink(0.0_f32)),
        label(move || bar.jobs.with(|js| format!(" · {}", header(js))))
            .style(move |s| s.font_size(12.0).color(t.get().desc).min_width(0.0).text_overflow(TextOverflow::Ellipsis)),
    ))
    .style(|s| s.gap(5.0).items_center().width_full().cursor(CursorStyle::Pointer))
    .on_click_stop(move |_| bar.folded.update(|f| *f = !*f));
    let rows = dyn_container(
        move || folded.get(),
        move |f| {
            if f {
                return empty().into_any();
            }
            let send = send.clone();
            let list = dyn_stack(
                // The newest first: the one open by itself is at the top.
                move || bar.jobs.with(|js| js.iter().rev().cloned().collect::<Vec<_>>()),
                |j: &Job| (j.id, j.status.clone()),
                move |j| row(ui, bar, send.clone(), j),
            )
            .style(|s| s.flex_col().gap(2.0).width_full().min_width(0.0));
            scroll(list)
                .style(move |s| s.width_full().margin_top(4.0).max_height(list_height(ui.window.get().1)))
                .into_any()
        },
    );
    v_stack((head, rows)).style(move |s| super::bar(t, s).apply_if(bar.jobs.with(Vec::is_empty), |s| s.hide()))
}

/// A job's line, and its output under it while it is open.
fn row(ui: Ui, bar: Bar, send: Send, j: Job) -> impl IntoView {
    let (t, id, running) = (ui.colors, j.id, j.running());
    let is_open = create_memo(move |_| bar.open.with(|o| o.contains(&id)));
    let chevron = dyn_container(
        move || is_open.get(),
        |o| icon(if o { "chevron-down" } else { "chevron-right" }, 14.0).into_any(),
    );
    let status = j.status.clone();
    let mark = if running { icon("loading", 14.0) } else { icon("circle-filled", 10.0) }.style(move |s| {
        let c = t.get();
        if running {
            if crate::accessibility::still(ui) {
                return s;
            }
            return s.rotate((bar.tick.get() % 12) as f64 * std::f64::consts::TAU / 12.0);
        }
        let ok = status == "ended" && j.exit == Some(0);
        s.color(if ok {
            c.ok
        } else if status == "stopped" {
            c.desc
        } else {
            c.error
        })
    });
    let mark = h_stack((mark,)).style(|s| s.width(14.0).height(14.0).items_center().justify_center());
    let name = format!("job {id}");
    let (command, words) = (j.command.clone(), standing(&j));
    let tip = j.command.clone();
    let stop =
        {
            let send = send.clone();
            label(move || {
                if bar.stopping.with(|s| s.contains(&id)) {
                    "Stopping…".to_string()
                } else {
                    format!("Stop job {id}")
                }
            })
            .on_click_stop(move |_| {
                if !bar.stopping.with_untracked(|s| s.contains(&id)) {
                    bar.stopping.update(|s| {
                        s.insert(id);
                    });
                    send(json!({"type": "stop_job", "id": id}));
                }
            })
            .style(move |s| {
                let c = t.get();
                s.padding_horiz(8.0)
                    .padding_vert(1.0)
                    .border_radius(3.0)
                    .font_size(12.0)
                    .flex_shrink(0.0_f32)
                    .background(c.button2_bg)
                    .color(c.button2_fg)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .hover(|s| s.background(c.button2_hover))
                    .apply_if(!running, |s| s.hide())
            })
        };
    let line = h_stack((
        chevron,
        mark,
        label(move || name.clone()).style(move |s| bold(s).font_size(12.0).flex_shrink(0.0_f32)),
        floem::views::tooltip(
            label(move || command.clone()).style(|s| {
                s.font_size(12.0).font_family(MONO_FONT.clone()).min_width(0.0).text_overflow(TextOverflow::Ellipsis)
            }),
            move || {
                let tip = tip.clone();
                label(move || tip.clone())
            },
        )
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0)),
        label(move || words.clone()).style(move |s| {
            s.font_size(12.0)
                .color(t.get().desc)
                .min_width(0.0)
                .flex_shrink(1.0_f32)
                .text_overflow(TextOverflow::Ellipsis)
        }),
        stop,
    ))
    .style(|s| s.gap(6.0).items_center().width_full().min_width(0.0).cursor(CursorStyle::Pointer))
    .on_click_stop(move |_| {
        let opening = !is_open.get_untracked();
        bar.open.update(|o| {
            if opening {
                o.insert(id);
            } else {
                o.remove(&id);
            }
        });
        if opening {
            ask(&send, id);
        }
    });
    let text = create_memo(move |_| bar.output.with(|o| o.get(&id).cloned()));
    let out = dyn_container(
        move || is_open.get(),
        move |o| {
            if !o {
                return empty().into_any();
            }
            let shown = label(move || match text.get() {
                Some(t) if !t.is_empty() => t.trim_end().to_string(),
                _ => "No output yet.".to_string(),
            })
            .style(move |s| {
                let none = text.with(|t| t.as_deref().is_none_or(str::is_empty));
                s.font_family(MONO_FONT.clone())
                    .font_size(12.0)
                    .line_height(1.4)
                    .apply_if(none, |s| s.color(t.get().desc))
            });
            scroll(shown)
                .scroll_to_percent(move || {
                    // The end in view, as a terminal follows what comes.
                    text.with(|t| t.as_ref().map(String::len));
                    100.0
                })
                .style(move |s| {
                    let c = t.get();
                    s.max_height((list_height(ui.window.get().1) - 24.0).clamp(40.0, 180.0))
                        .width_full()
                        .min_width(0.0)
                        .margin_left(19.0)
                        .margin_vert(2.0)
                        .padding_horiz(8.0)
                        .padding_vert(4.0)
                        .border_left(1.0)
                        .border_color(c.widget_border)
                })
                .into_any()
        },
    );
    v_stack((line, out)).style(|s| s.width_full().min_width(0.0))
}

/// What it adds to the window's registries, once at start: its bar over the input.
pub fn register(_ui: Ui, _send: &Send) {
    super::slots::add(super::slots::Slot::Bars, "jobs", 30, |ui, send| view(ui, send).into_any());
}

/// The session's tasks, their jobs taken in, and a job's output (`job_output{id, text}`, taken away).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str() {
        Some("tasks") => {
            let jobs: Vec<Job> = m["jobs"].as_array().into_iter().flatten().map(Job::of).collect();
            let running: Vec<(u64, String)> =
                jobs.iter().filter(|j| j.running()).map(|j| (j.id, j.command.clone())).collect();
            if ui.jobs.with_untracked(|j| *j != running) {
                ui.jobs.set(running);
            }
            let bar = ui.state::<Bar>();
            let seen = jobs.iter().map(|j| j.id).max().unwrap_or(0);
            // Another session's jobs (counted from 1 again): what was open or shown was the last one's.
            if seen < bar.seen.get_untracked() {
                bar.seen.set(0);
                bar.auto.set(None);
                bar.open.set(BTreeSet::new());
                bar.output.set(HashMap::new());
            }
            let open = bar.open.get_untracked();
            let was = bar.jobs.get_untracked();
            // One open that ended: its output's end, once more.
            for j in jobs.iter().filter(|j| !j.running() && open.contains(&j.id)) {
                if was.iter().any(|w| w.id == j.id && w.running()) {
                    ask(send, j.id);
                }
            }
            if bar.stopping.with_untracked(|s| s.iter().any(|id| !jobs.iter().any(|j| j.id == *id && j.running()))) {
                bar.stopping.update(|s| s.retain(|id| jobs.iter().any(|j| j.id == *id && j.running())));
            }
            if let Some(newest) = fresh(bar.seen.get_untracked(), &jobs) {
                let before = bar.auto.get_untracked();
                bar.open.update(|o| {
                    if let Some(b) = before {
                        o.remove(&b);
                    }
                    o.insert(newest);
                });
                bar.auto.set(Some(newest));
                ask(send, newest);
            }
            if seen > bar.seen.get_untracked() {
                bar.seen.set(seen);
            }
            let any = jobs.iter().any(Job::running);
            if was != jobs {
                bar.jobs.set(jobs);
            }
            if any {
                clock(bar, send.clone());
            }
            false
        }
        Some("job_output") => {
            let bar = ui.state::<Bar>();
            let id = m["id"].as_u64().unwrap_or(0);
            let text = m["text"].as_str().unwrap_or("");
            if bar.output.with_untracked(|o| o.get(&id).map(String::as_str) != Some(text)) {
                bar.output.update(|o| {
                    o.insert(id, text.to_string());
                });
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: u64, status: &str) -> Job {
        Job { id, command: "cargo test".into(), status: status.into(), ..Job::default() }
    }

    #[test]
    fn the_header_says_how_many_commands_and_how_many_run() {
        assert_eq!(header(&[job(1, "running")]), "1 command, 1 running");
        assert_eq!(header(&[job(1, "ended"), job(2, "running"), job(3, "stopped")]), "3 commands, 1 running");
    }

    #[test]
    fn the_bar_takes_a_quarter_of_the_window_at_most() {
        assert_eq!(list_height(800.0), 160.0);
        assert_eq!(list_height(100.0), 60.0);
    }

    #[test]
    fn a_jobs_time_is_said_in_seconds_minutes_or_hours() {
        assert_eq!(took(12_400), "12s");
        assert_eq!(took(192_000), "3m 12s");
        assert_eq!(took(3_840_000), "1h 4m");
    }

    #[test]
    fn each_job_says_how_it_stands() {
        assert_eq!(standing(&job(1, "running")), "running, no time limit");
        assert_eq!(standing(&Job { background: true, ..job(1, "running") }), "running in the background");
        let ended = Job { exit: Some(0), started_ms: 1_000, ended_ms: Some(193_000), ..job(1, "ended") };
        assert_eq!(standing(&ended), "exit code 0 after 3m 12s");
        assert_eq!(standing(&Job { status: "stopped".into(), exit: None, ..ended.clone() }), "stopped after 3m 12s");
        assert_eq!(standing(&Job { exit: None, ..ended }), "ended by a signal after 3m 12s");
    }

    #[test]
    fn the_newest_job_running_is_opened_once() {
        let jobs = [job(1, "running"), job(2, "running"), job(3, "ended")];
        assert_eq!(fresh(0, &jobs), Some(2));
        assert_eq!(fresh(3, &jobs), None);
    }

    #[test]
    fn a_job_is_read_from_the_tasks_and_from_a_session_without_its_status() {
        let j = Job::of(&json!({"id": 4, "command": "make", "running": false, "exit": 2, "background": false,
                                "status": "ended", "started_ms": 10, "ended_ms": 20}));
        assert_eq!((j.id, j.exit, j.running(), j.ended_ms), (4, Some(2), false, Some(20)));
        assert!(Job::of(&json!({"id": 1, "running": true})).running());
    }
}
