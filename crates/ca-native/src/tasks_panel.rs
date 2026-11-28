//! The Tasks view: the session's sub-agents (and, with "Background tasks of your own", the person's own agents in
//! the background), each a row with where it stands, its title, "yours" for one of the person's, a pencil when it
//! may change files, "as ASSISTANT · now · 12 s · 3.4k tokens · 5 tool calls" and Stop while it works; those at
//! work first, then those that ended. A row opens its detail: Back and Stop, its status line, what it was asked, its
//! tools, each step's thinking, words and calls, what it is doing, its report and its errors.
//!
//! In the thread a `task` call is one line (its status, its description, "Working…" or its report's first line, its
//! time, tokens and model) that opens it here, and two or more in a row are one group ("3 sub-agents · 2 working").
//! The session's side is the driver's `ide_agents.rs`: `agent_tasks`, `agent_task{id}`, `agent_task_stop{id}` and
//! its `agent` events.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use floem::action::exec_after;
use floem::peniko::Color;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, tooltip, v_stack};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::colors::Colors;
use crate::icons::icon;
use crate::layout::View;
use crate::model::Tool;
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, Theme, action, bold, button, caps, dialog, toast};

/// Whether the view is in the window.
pub const READY: bool = true;

/// The view, while the session has started no task.
const NONE_YET: &str = "No tasks in this session yet.";
/// What the help says.
const HELP: &str = "When the agent hands a piece of work to a sub-agent, the sub-agent works here in a context of its \
                    own and reports back; the agent reads its report. Several run at once. Open one to see what it \
                    was asked, its steps and its report, or stop it.";
/// How often a working task's seconds and spinner move.
const SPIN: Duration = Duration::from_millis(100);
/// How long an open task waits before its detail is asked for again as it changes.
const DETAIL_EVERY: Duration = Duration::from_millis(700);

/// A task as its row shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub assistant: String,
    /// "working", "done", "failed" or "stopped".
    pub status: String,
    /// What it is doing ("": it ended).
    pub now: String,
    /// When it began, in milliseconds since 1970.
    pub began: u64,
    /// How long it had worked when the session said it.
    pub ms: u64,
    pub tokens: u64,
    pub calls: u64,
    pub model: String,
    pub edits: bool,
    pub yours: bool,
    /// When the session said it.
    pub heard: Instant,
}

impl Default for Task {
    fn default() -> Self {
        Task {
            id: String::new(),
            title: String::new(),
            assistant: String::new(),
            status: String::new(),
            now: String::new(),
            began: 0,
            ms: 0,
            tokens: 0,
            calls: 0,
            model: String::new(),
            edits: false,
            yours: false,
            heard: Instant::now(),
        }
    }
}

fn text(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

impl Task {
    /// A task as the session says it (an `agent` event, an `agent_tasks` item, an `agent_task`).
    pub fn from_json(v: &Value) -> Task {
        Task {
            id: text(v, "id"),
            title: text(v, "title"),
            assistant: text(v, "assistant"),
            status: text(v, "status"),
            now: text(v, "now"),
            began: v["began"].as_u64().unwrap_or(0),
            ms: v["ms"].as_u64().unwrap_or(0),
            tokens: v["tokens"].as_u64().unwrap_or(0),
            calls: v["calls"].as_u64().unwrap_or(0),
            model: text(v, "model"),
            edits: v["edits"] == true,
            yours: v["yours"] == true,
            heard: Instant::now(),
        }
    }

    pub fn working(&self) -> bool {
        self.status == "working"
    }

    /// How long it has worked: still counting while it works.
    pub fn ms_now(&self) -> u64 {
        if self.working() { self.ms + self.heard.elapsed().as_millis() as u64 } else { self.ms }
    }
}

/// A piece of a task's step.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Thinking(String),
    Text(String),
    Notice(String),
    Call { name: String, subject: String, done: bool, ok: bool, ms: u64, text: String },
}

/// A task opened: its row, and what it was asked, did and said.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Detail {
    pub task: Task,
    pub prompt: String,
    pub tools: Vec<String>,
    pub report: String,
    pub error: Option<String>,
    pub steps: Vec<Vec<Piece>>,
    /// The session kept its work (false: read from the thread's call alone, as one from before a restart).
    pub kept: bool,
}

fn piece_of(p: &Value) -> Option<Piece> {
    let t = text(p, "text");
    Some(match p["kind"].as_str()? {
        "thinking" => Piece::Thinking(t),
        "text" => Piece::Text(t),
        "notice" => Piece::Notice(t),
        "tool" => Piece::Call {
            name: text(p, "name"),
            subject: text(p, "subject"),
            done: p["done"] == true,
            ok: p["ok"] != false,
            ms: p["ms"].as_u64().unwrap_or(0),
            text: t,
        },
        _ => return None,
    })
}

impl Detail {
    /// An `agent_task` reply (none: it says the session has no such task).
    pub fn from_json(v: &Value) -> Option<Detail> {
        v.get("status")?;
        let mut steps: Vec<Vec<Piece>> = v["work"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| s.as_array().into_iter().flatten().filter_map(piece_of).collect::<Vec<_>>())
            .filter(|s: &Vec<Piece>| !s.is_empty())
            .collect();
        // Its last words are its report, shown under its own heading: not twice.
        let report = text(v, "report");
        if !report.trim().is_empty()
            && let Some(last) = steps.last_mut()
            && matches!(last.last(), Some(Piece::Text(w)) if w.trim() == report.trim())
        {
            last.pop();
            if last.is_empty() {
                steps.pop();
            }
        }
        Some(Detail {
            task: Task::from_json(v),
            prompt: text(v, "prompt"),
            tools: v["tools"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect(),
            report,
            error: v["error"].as_str().filter(|e| !e.is_empty()).map(str::to_string),
            steps,
            kept: true,
        })
    }

    /// A task read from its call in the thread: what it was asked and what it reported.
    pub fn from_tool(t: &Tool) -> Detail {
        let status = call_status(t);
        let failed = status == "failed" || status == "stopped";
        Detail {
            task: Task {
                id: t.id.clone(),
                title: description(t),
                assistant: t.arguments["agent"].as_str().filter(|a| !a.is_empty()).unwrap_or("sub-agent").to_string(),
                status: status.to_string(),
                ms: t.ms,
                heard: t.began.unwrap_or_else(Instant::now),
                ..Task::default()
            },
            prompt: t.arguments["prompt"].as_str().unwrap_or("").to_string(),
            report: if t.done && !failed { report_of(&t.text).to_string() } else { String::new() },
            error: (t.done && failed && !t.text.is_empty()).then(|| t.text.clone()),
            ..Detail::default()
        }
    }
}

/// Where a `task` call stands, in the session's words for a task.
pub fn call_status(t: &Tool) -> &'static str {
    match (t.done, t.ok, t.status.as_str()) {
        (false, _, _) => "working",
        (true, true, _) => "done",
        (true, false, "denied" | "skipped") => "stopped",
        (true, false, _) if t.text.starts_with("The person stopped") => "stopped",
        _ => "failed",
    }
}

/// A `task` call's description (its subject, else what it was asked).
pub fn description(t: &Tool) -> String {
    let d = t.arguments["description"].as_str().unwrap_or("").trim();
    if !d.is_empty() {
        d.to_string()
    } else if !t.subject.is_empty() {
        t.subject.clone()
    } else {
        "Sub-agent".to_string()
    }
}

/// A `task` call's result without the line the tool puts first ("NAME reports (…, N steps):").
pub fn report_of(result: &str) -> &str {
    match result.split_once('\n') {
        Some((first, rest)) if first.contains(" reports (") && first.ends_with("):") => rest,
        _ => result,
    }
}

/// The first line of a report that has words, without its Markdown heading's marks.
pub fn first_line(report: &str) -> String {
    report.lines().map(|l| l.trim().trim_start_matches('#').trim()).find(|l| !l.is_empty()).unwrap_or("").to_string()
}

/// "12 s", "1 min 5 s", "2 h 3 min".
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min {} s", s / 60, s % 60),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

/// "800 tokens", "3.4k tokens", "1.2M tokens".
pub fn tokens(n: u64) -> String {
    match n {
        0..1000 => format!("{n} tokens"),
        1000..1_000_000 => format!("{:.1}k tokens", n as f64 / 1000.0).replace(".0k", "k"),
        _ => format!("{:.1}M tokens", n as f64 / 1_000_000.0).replace(".0M", "M"),
    }
}

/// When it began, from how long ago: "now", "5 min ago", "2 h ago", "3 d ago".
pub fn ago(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A row's second line: "as ASSISTANT · now · 12 s · 3.4k tokens · 5 tool calls" (`since`: how long ago it began).
pub fn meta(t: &Task, since: Option<u64>, ms: u64) -> String {
    let mut parts = vec![format!("as {}", if t.assistant.is_empty() { "sub-agent" } else { &t.assistant })];
    if let Some(since) = since {
        parts.push(ago(since));
    }
    parts.push(duration(ms));
    if t.tokens > 0 {
        parts.push(tokens(t.tokens));
    }
    parts.push(if t.calls == 1 { "1 tool call".into() } else { format!("{} tool calls", t.calls) });
    parts.join(" · ")
}

/// A status in words.
pub fn status_word(status: &str) -> &'static str {
    match status {
        "working" => "Working",
        "done" => "Done",
        "stopped" => "Stopped",
        _ => "Failed",
    }
}

/// A status's codicon and colour (none: the text's own).
pub fn mark(status: &str, c: &Colors) -> (&'static str, Option<Color>) {
    match status {
        "working" => ("loading", None),
        "done" => ("check", Some(c.ok)),
        "stopped" => ("circle-slash", None),
        _ => ("error", Some(c.error)),
    }
}

/// A group of task calls: "3 sub-agents · 2 working", "3 sub-agents · done".
pub fn crew_line(statuses: &[&str]) -> String {
    let working = statuses.iter().filter(|s| **s == "working").count();
    let failed = statuses.iter().filter(|s| **s == "failed").count();
    let n = statuses.len();
    if working > 0 {
        format!("{n} sub-agents · {working} working")
    } else if failed > 0 {
        format!("{n} sub-agents · {failed} failed")
    } else {
        format!("{n} sub-agents · done")
    }
}

/// The view's state in a window.
#[derive(Clone, Copy)]
pub struct TasksState {
    /// The session's tasks, newest first.
    pub tasks: RwSignal<Vec<Task>>,
    /// The task opened, by id.
    pub open: RwSignal<Option<String>>,
    pub detail: RwSignal<Option<Detail>>,
    /// The session the tasks are of.
    session: RwSignal<String>,
    /// Moves while a task works.
    spin: RwSignal<u64>,
    spinning: RwSignal<bool>,
    /// The open task's detail is to be asked for again.
    asking: RwSignal<bool>,
}

impl crate::ext::State for TasksState {
    fn new(ui: Ui) -> Self {
        TasksState {
            tasks: ui.scope.create_rw_signal(Vec::new()),
            open: ui.scope.create_rw_signal(None),
            detail: ui.scope.create_rw_signal(None),
            session: ui.scope.create_rw_signal(String::new()),
            spin: ui.scope.create_rw_signal(0),
            spinning: ui.scope.create_rw_signal(false),
            asking: ui.scope.create_rw_signal(false),
        }
    }
}

pub fn state(ui: Ui) -> TasksState {
    ui.state::<TasksState>()
}

impl TasksState {
    fn working(&self) -> bool {
        self.tasks.with_untracked(|ts| ts.iter().any(Task::working))
            || self.detail.with_untracked(|d| d.as_ref().is_some_and(|d| d.task.working()))
    }

    /// The clock moves while a task works, and stands still otherwise.
    fn spin_on(self) {
        if self.spinning.get_untracked() || !self.working() {
            return;
        }
        self.spinning.set(true);
        fn go(st: TasksState) {
            exec_after(SPIN, move |_| {
                if st.working() {
                    st.spin.update(|n| *n += 1);
                    go(st);
                } else {
                    st.spinning.set(false);
                }
            });
        }
        go(self);
    }

    /// The open task's detail, asked for again once its news has settled a moment.
    fn ask_detail(self, send: &Send) {
        if self.asking.get_untracked() {
            return;
        }
        self.asking.set(true);
        let send = send.clone();
        exec_after(DETAIL_EVERY, move |_| {
            self.asking.set(false);
            if let Some(id) = self.open.get_untracked() {
                send(json!({"type": "agent_task", "id": id}));
            }
        });
    }
}

/// Opens a task: in the view, from its call in the thread when the session has no more of it.
pub fn open(ui: Ui, send: &Send, id: &str, from: Option<Detail>) {
    let st = state(ui);
    if st.open.with_untracked(|o| o.as_deref() != Some(id)) {
        st.open.set(Some(id.to_string()));
        st.detail.set(from);
    }
    ui.reveal(View::Tasks);
    send(json!({"type": "agent_task", "id": id}));
}

fn back(ui: Ui) {
    let st = state(ui);
    st.open.set(None);
    st.detail.set(None);
}

fn stop(send: &Send, id: &str) {
    send(json!({"type": "agent_task_stop", "id": id}));
}

/// The view's own content: the list, or the task opened.
pub fn view(ui: Ui, send: Send) -> AnyView {
    let (t, st) = (ui.colors, state(ui));
    let opened = create_memo(move |_| st.open.with(Option::is_some));
    dyn_container(
        move || opened.get(),
        move |o| if o { detail(ui, send.clone()).into_any() } else { list(ui, send.clone()).into_any() },
    )
    .style(move |s| s.size_full().min_height(0.0).color(t.get().side_fg))
    .into_any()
}

/// Wrapped words, in the description's colour.
fn desc(t: Theme, words: String) -> impl IntoView {
    label(move || words.clone()).style(move |s| {
        s.font_size(12.0).line_height(1.4).color(t.get().desc).text_overflow(TextOverflow::Wrap).min_width(0.0)
    })
}

/// A status's icon: a spinner that turns while it works.
fn status_icon(ui: Ui, status: Memo<String>) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    dyn_container(
        move || status.get(),
        move |s| {
            let working = s == "working";
            icon(mark(&s, &t.get_untracked()).0, 16.0)
                .style(move |st_| {
                    let c = t.get();
                    let st_ = match mark(&s, &c).1 {
                        Some(colour) => st_.color(colour),
                        None => st_,
                    };
                    st_.flex_shrink(0.0_f32)
                        .apply_if(working, |s| s.rotate((st.spin.get() % 12) as f64 * std::f64::consts::TAU / 12.0))
                })
                .into_any()
        },
    )
}

/// The list: those at work, then those that ended.
fn list(ui: Ui, send: Send) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let any = create_memo(move |_| st.tasks.with(|ts| !ts.is_empty()));
    dyn_container(
        move || any.get(),
        move |any| {
            if !any {
                return desc(t, NONE_YET.into()).style(|s| s.padding_horiz(20.0).padding_vert(10.0)).into_any();
            }
            let at_work = create_memo(move |_| {
                st.tasks.with(|ts| ts.iter().filter(|t| t.working()).map(|t| t.id.clone()).collect::<Vec<_>>())
            });
            let ended = create_memo(move |_| {
                st.tasks.with(|ts| ts.iter().filter(|t| !t.working()).map(|t| t.id.clone()).collect::<Vec<_>>())
            });
            scroll(
                v_stack((section(ui, send.clone(), "At work", at_work), section(ui, send.clone(), "Ended", ended)))
                    .style(|s| s.width_full().padding_bottom(10.0)),
            )
            .style(|s| s.size_full().min_height(0.0))
            .into_any()
        },
    )
    .style(|s| s.size_full().min_height(0.0))
}

fn section(ui: Ui, send: Send, title: &'static str, ids: Memo<Vec<String>>) -> impl IntoView {
    let t = ui.colors;
    v_stack((
        label(move || title).style(move |s| bold(caps(s)).color(t.get().desc).padding_horiz(20.0).padding_vert(6.0)),
        dyn_stack(move || ids.get(), String::clone, move |id| row(ui, send.clone(), id))
            .style(|s| s.flex_col().width_full()),
    ))
    .style(move |s| s.width_full().apply_if(ids.with(Vec::is_empty), |s| s.hide()))
}

/// A task's row: its status, title, "yours", pencil, its line, and Stop while it works.
fn row(ui: Ui, send: Send, id: String) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let key = id.clone();
    let me = create_memo(move |_| st.tasks.with(|ts| ts.iter().find(|t| t.id == key).cloned()).unwrap_or_default());
    let status = create_memo(move |_| me.with(|m| m.status.clone()));
    let line = create_memo(move |_| {
        st.spin.with(|_| ());
        me.with(|m| meta(m, Some(unix_ms().saturating_sub(m.began)), m.ms_now()))
    });
    let title = label(move || me.with(|m| m.title.clone()))
        .style(move |s| s.color(t.get().fg).text_ellipsis().min_width(0.0).flex_shrink(1.0_f32));
    let yours = label(|| "yours").style(move |s| {
        let c = t.get();
        caps(s)
            .padding_horiz(5.0)
            .border(1.0)
            .border_radius(3.0)
            .border_color(c.widget_border)
            .color(c.desc)
            .apply_if(!me.with(|m| m.yours), |s| s.hide())
    });
    let pencil = tooltip(icon("edit", 14.0).style(move |s| s.color(t.get().desc)), || label(|| "May change files"))
        .style(move |s| s.apply_if(!me.with(|m| m.edits), |s| s.hide()));
    let (stop_send, open_send, open_id, stop_id) = (send.clone(), send, id.clone(), id);
    let stop_button = tooltip(action(t, "debug-stop", move || stop(&stop_send, &stop_id)), || label(|| "Stop"))
        .style(move |s| s.apply_if(status.get() != "working", |s| s.hide()));
    h_stack((
        status_icon(ui, status).style(|s| s.margin_top(2.0)),
        v_stack((
            h_stack((title, yours, pencil)).style(|s| s.gap(6.0).items_center().min_width(0.0)),
            label(move || line.get())
                .style(move |s| s.font_size(12.0).color(t.get().desc).text_ellipsis().min_width(0.0)),
        ))
        .style(|s| s.gap(2.0).flex_grow(1.0_f32).min_width(0.0)),
        stop_button,
    ))
    .on_click_stop(move |_| open(ui, &open_send, &open_id, None))
    .style(move |s| {
        let c = t.get();
        s.gap(8.0)
            .width_full()
            .padding_horiz(20.0)
            .padding_vert(5.0)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.background(c.list_hover))
    })
}

/// A heading of the detail.
fn heading(t: Theme, words: &'static str) -> impl IntoView {
    label(move || words).style(move |s| bold(s).font_size(12.0).color(t.get().fg).margin_top(10.0))
}

/// The task opened: Back, its title and Stop; its status line; what it was asked and its tools; its steps; what it
/// is doing; its report and its error.
fn detail(ui: Ui, send: Send) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let d = move || st.detail;
    let status = create_memo(move |_| d().with(|d| d.as_ref().map(|d| d.task.status.clone()).unwrap_or_default()));
    let title = label(move || d().with(|d| d.as_ref().map(|d| d.task.title.clone()).unwrap_or_default()))
        .style(move |s| bold(s).color(t.get().fg).text_ellipsis().min_width(0.0).flex_grow(1.0_f32));
    let stop_send = send.clone();
    let stop_button = button(t, "Stop", false, move || {
        if let Some(id) = st.open.get_untracked() {
            stop(&stop_send, &id);
        }
    });
    let head = h_stack((
        tooltip(action(t, "arrow-left", move || back(ui)), || label(|| "Back")),
        title,
        container(stop_button).style(move |s| s.apply_if(status.get() != "working", |s| s.hide())),
    ))
    .style(|s| s.gap(6.0).items_center().width_full().padding_horiz(12.0).padding_vert(6.0));
    let line = create_memo(move |_| {
        st.spin.with(|_| ());
        d().with(|d| {
            d.as_ref()
                .map(|d| {
                    let since = (d.task.began > 0).then(|| unix_ms().saturating_sub(d.task.began));
                    let mut words =
                        format!("{} · {}", status_word(&d.task.status), meta(&d.task, since, d.task.ms_now()));
                    if !d.task.model.is_empty() {
                        words.push_str(&format!(" · {}", d.task.model));
                    }
                    words
                })
                .unwrap_or_default()
        })
    });
    let status_line = h_stack((
        status_icon(ui, status),
        label(move || line.get()).style(move |s| {
            s.font_size(12.0).line_height(1.4).color(t.get().desc).text_overflow(TextOverflow::Wrap).min_width(0.0)
        }),
    ))
    .style(|s| s.gap(6.0).items_center().width_full());
    let prompt = create_memo(move |_| d().with(|d| d.as_ref().map(|d| d.prompt.clone()).unwrap_or_default()));
    let asked = v_stack((
        heading(t, "What it was asked"),
        label(move || prompt.get()).style(move |s| {
            s.font_size(12.0).line_height(1.4).color(t.get().fg).text_overflow(TextOverflow::Wrap).min_width(0.0)
        }),
    ))
    .style(move |s| s.gap(4.0).width_full().apply_if(prompt.with(String::is_empty), |s| s.hide()));
    let tools = create_memo(move |_| d().with(|d| d.as_ref().map(|d| d.tools.join(", ")).unwrap_or_default()));
    let tools_line = label(move || format!("Tools: {}", tools.get())).style(move |s| {
        s.font_size(12.0)
            .line_height(1.4)
            .color(t.get().desc)
            .text_overflow(TextOverflow::Wrap)
            .min_width(0.0)
            .apply_if(tools.with(String::is_empty), |s| s.hide())
    });
    let steps = create_memo(move |_| d().with(|d| d.as_ref().map(|d| d.steps.clone()).unwrap_or_default()));
    let cards = dyn_stack(
        move || steps.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, s): &(usize, Vec<Piece>)| {
            (*i, format!("{s:?}").len(), s.iter().filter(|p| matches!(p, Piece::Call { done: true, .. })).count())
        },
        move |(i, s)| step_card(ui, i, s),
    )
    .style(|s| s.flex_col().gap(6.0).width_full());
    let now = create_memo(move |_| {
        d().with(|d| d.as_ref().filter(|d| d.task.working()).map(|d| d.task.now.clone()).unwrap_or_default())
    });
    let doing = label(move || format!("{}…", now.get().trim_end_matches('…'))).style(move |s| {
        s.font_size(12.0)
            .color(t.get().desc)
            .text_ellipsis()
            .min_width(0.0)
            .apply_if(now.with(String::is_empty), |s| s.hide())
    });
    let report = create_memo(move |_| d().with(|d| d.as_ref().map(|d| d.report.clone()).unwrap_or_default()));
    let report_view = dyn_container(
        move || report.get(),
        move |r| {
            if r.trim().is_empty() {
                return empty().into_any();
            }
            let text = floem::reactive::create_rw_signal(r);
            v_stack((heading(t, "Its report"), crate::prose::answer(ui, text)))
                .style(|s| s.gap(4.0).width_full().min_width(0.0))
                .into_any()
        },
    )
    .style(|s| s.width_full().min_width(0.0));
    let error = create_memo(move |_| d().with(|d| d.as_ref().and_then(|d| d.error.clone()).unwrap_or_default()));
    let error_line = label(move || error.get()).style(move |s| {
        s.font_size(12.0)
            .color(t.get().error)
            .text_overflow(TextOverflow::Wrap)
            .min_width(0.0)
            .apply_if(error.with(String::is_empty), |s| s.hide())
    });
    let not_kept = desc(t, "Its steps are not kept: it ran before Code Arena last started.".into())
        .style(move |s| s.apply_if(d().with(|d| d.as_ref().is_none_or(|d| d.kept || d.task.working())), |s| s.hide()));
    v_stack((
        head,
        scroll(
            v_stack((status_line, asked, tools_line, not_kept, cards, doing, report_view, error_line))
                .style(|s| s.gap(6.0).width_full().padding_horiz(20.0).padding_bottom(12.0)),
        )
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full().min_height(0.0)),
    ))
    .style(|s| s.size_full().min_height(0.0))
}

/// One step of a task: its thinking (opened on a click), its words, its notices and its calls.
fn step_card(ui: Ui, i: usize, pieces: Vec<Piece>) -> impl IntoView {
    let t = ui.colors;
    let views: Vec<AnyView> = pieces.into_iter().map(|p| piece(ui, p)).collect();
    v_stack((
        label(move || format!("Step {}", i + 1)).style(move |s| caps(s).color(t.get().desc)),
        floem::views::stack_from_iter(views).style(|s| s.flex_col().gap(4.0).width_full().min_width(0.0)),
    ))
    .style(move |s| {
        s.gap(4.0)
            .width_full()
            .min_width(0.0)
            .padding(8.0)
            .border(1.0)
            .border_radius(4.0)
            .border_color(t.get().widget_border)
    })
}

fn piece(ui: Ui, p: Piece) -> AnyView {
    let t = ui.colors;
    let wrap = move |s: floem::style::Style| {
        s.font_size(12.0).line_height(1.4).text_overflow(TextOverflow::Wrap).min_width(0.0)
    };
    match p {
        Piece::Thinking(words) => {
            let open = floem::reactive::create_rw_signal(false);
            v_stack((
                h_stack((
                    dyn_container(move || open.get(), |o| icon(if o { "chevron-down" } else { "chevron-right" }, 14.0)),
                    label(|| "Thinking"),
                ))
                .on_click_stop(move |_| open.update(|o| *o = !*o))
                .style(move |s| {
                    s.gap(4.0).items_center().color(t.get().desc).cursor(CursorStyle::Pointer).selectable(false)
                }),
                label(move || words.clone())
                    .style(move |s| wrap(s).color(t.get().desc).apply_if(!open.get(), |s| s.hide())),
            ))
            .style(|s| s.gap(2.0).width_full().min_width(0.0))
            .into_any()
        }
        Piece::Text(words) => label(move || words.clone()).style(move |s| wrap(s).color(t.get().fg)).into_any(),
        Piece::Notice(words) => label(move || words.clone()).style(move |s| wrap(s).color(t.get().desc)).into_any(),
        Piece::Call { name, subject, done, ok, ms, text } => {
            let open = floem::reactive::create_rw_signal(false);
            let status = if !done {
                "working"
            } else if ok {
                "done"
            } else {
                "failed"
            };
            let (mark_name, colour) = mark(status, &t.get_untracked());
            let first: String = subject.lines().next().unwrap_or("").to_string();
            let took = if done && ms > 0 { duration(ms) } else { String::new() };
            v_stack((
                h_stack((
                    icon(mark_name, 14.0).style(move |s| match colour {
                        Some(c) => s.color(c).flex_shrink(0.0_f32),
                        None => s.flex_shrink(0.0_f32),
                    }),
                    label(move || name.clone()).style(move |s| s.color(t.get().fg).flex_shrink(0.0_f32)),
                    label(move || first.clone()).style(|s| {
                        s.font_family(MONO_FONT.clone())
                            .font_size(12.0)
                            .text_ellipsis()
                            .min_width(0.0)
                            .flex_shrink(1.0_f32)
                    }),
                    empty().style(|s| s.flex_grow(1.0_f32)),
                    label(move || took.clone()).style(|s| s.font_size(12.0).flex_shrink(0.0_f32)),
                ))
                .on_click_stop(move |_| open.update(|o| *o = !*o))
                .style(move |s| {
                    s.gap(6.0)
                        .items_center()
                        .width_full()
                        .min_width(0.0)
                        .color(t.get().desc)
                        .cursor(CursorStyle::Pointer)
                }),
                label(move || text.clone()).style(move |s| {
                    s.font_family(MONO_FONT.clone())
                        .font_size(11.0)
                        .color(t.get().desc)
                        .text_overflow(TextOverflow::Wrap)
                        .min_width(0.0)
                        .padding_left(20.0)
                        .apply_if(!open.get(), |s| s.hide())
                }),
            ))
            .style(|s| s.gap(2.0).width_full().min_width(0.0))
            .into_any()
        }
    }
}

/// A `task` call in the thread: one line that opens it in the Tasks view.
pub fn line(ui: Ui, send: Send, tools: RwSignal<Vec<Tool>>, n: usize) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let me = create_memo(move |_| tools.with(|ts| ts.get(n).cloned().unwrap_or_default()));
    let live = create_memo(move |_| {
        let id = me.with(|m| m.id.clone());
        st.tasks.with(|ts| ts.iter().find(|t| !id.is_empty() && t.id == id).cloned())
    });
    let status = create_memo(move |_| {
        let own = me.with(|m| call_status(m).to_string());
        // The call's own end is the last word: a task the session has not told of ending ends with its call.
        if own == "working" { live.with(|l| l.as_ref().map(|l| l.status.clone())).unwrap_or(own) } else { own }
    });
    let words = create_memo(move |_| {
        let s = status.get();
        me.with(|m| match s.as_str() {
            "working" => "Working…".to_string(),
            "done" => first_line(report_of(&m.text)),
            _ => first_line(&m.text),
        })
    });
    let facts = create_memo(move |_| {
        st.spin.with(|_| ());
        let working = status.get() == "working";
        let live = live.get();
        let ms = match &live {
            Some(l) if working || l.ms > 0 => l.ms_now(),
            _ => me.with(|m| if working { m.began.map(|b| b.elapsed().as_millis() as u64).unwrap_or(0) } else { m.ms }),
        };
        let mut parts = vec![duration(ms)];
        if let Some(l) = &live {
            if l.tokens > 0 {
                parts.push(tokens(l.tokens));
            }
            if !l.model.is_empty() {
                parts.push(l.model.clone());
            }
        }
        parts.join(" · ")
    });
    let title = create_memo(move |_| me.with(description));
    h_stack((
        status_icon(ui, status),
        label(move || title.get()).style(move |s| {
            s.color(t.get().fg).text_ellipsis().min_width(0.0).flex_shrink(0.0_f32).max_width_pct(50.0)
        }),
        label(move || words.get()).style(|s| s.text_ellipsis().min_width(0.0).flex_shrink(1.0_f32)),
        empty().style(|s| s.flex_grow(1.0_f32)),
        label(move || facts.get()).style(|s| s.font_size(12.0).flex_shrink(0.0_f32)),
        icon("chevron-right", 14.0).style(|s| s.flex_shrink(0.0_f32)),
    ))
    .on_click_stop(move |_| {
        let tool = me.get_untracked();
        if !tool.id.is_empty() {
            open(ui, &send, &tool.id, Some(Detail::from_tool(&tool)));
        }
    })
    .style(move |s| {
        let c = t.get();
        s.gap(6.0)
            .items_center()
            .width_full()
            .min_width(0.0)
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .border(1.0)
            .border_radius(4.0)
            .border_color(c.widget_border)
            .color(c.desc)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.list_hover))
    })
}

/// Two or more `task` calls in a row: their line ("3 sub-agents · 2 working") over each one's line.
pub fn crew(ui: Ui, send: Send, tools: RwSignal<Vec<Tool>>, start: usize, len: Memo<usize>) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let head = create_memo(move |_| {
        let n = len.get();
        st.tasks.with(|live| {
            tools.with(|ts| {
                let statuses: Vec<&str> = ts
                    .iter()
                    .skip(start)
                    .take(n)
                    .map(|c| match call_status(c) {
                        "working" => live
                            .iter()
                            .find(|l| l.id == c.id)
                            .map(|l| if l.working() { "working" } else { "done" })
                            .unwrap_or("working"),
                        s => s,
                    })
                    .collect();
                crew_line(&statuses)
            })
        })
    });
    v_stack((
        h_stack((icon("organization", 14.0), label(move || head.get())))
            .style(move |s| s.gap(6.0).items_center().color(t.get().desc)),
        dyn_stack(
            move || (start..start + len.get()).collect::<Vec<_>>(),
            |n: &usize| *n,
            move |n| line(ui, send.clone(), tools, n),
        )
        .style(|s| s.flex_col().gap(4.0).width_full().min_width(0.0)),
    ))
    .style(|s| s.gap(4.0).width_full().min_width(0.0))
}

/// The actions in its title: how many are at work, and the help.
pub fn actions(ui: Ui, _send: Send) -> Option<AnyView> {
    let (t, st) = (ui.colors, state(ui));
    let working = create_memo(move |_| st.tasks.with(|ts| ts.iter().filter(|t| t.working()).count()));
    let count = label(move || format!("{} at work", working.get())).style(move |s| {
        s.font_size(11.0).color(t.get().desc).margin_right(4.0).apply_if(working.get() == 0, |s| s.hide())
    });
    let help = tooltip(
        action(t, "question", move || {
            dialog::ask("Tasks").detail(HELP).kind(dialog::Kind::Info).button("OK").no_cancel().open(ui, |_| {})
        }),
        || label(|| "About Tasks"),
    );
    Some(h_stack((count, help)).style(|s| s.items_center()).into_any())
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    match m["type"].as_str().unwrap_or("") {
        // Another session: its own tasks.
        "state" => {
            let session = m["session"].as_str().unwrap_or("");
            if !session.is_empty() && st.session.with_untracked(|s| s != session) {
                st.session.set(session.to_string());
                if st.tasks.with_untracked(|t| !t.is_empty()) {
                    st.tasks.set(Vec::new());
                }
                if st.open.with_untracked(Option::is_some) {
                    back(ui);
                }
                send(json!({"type": "agent_tasks"}));
            }
            false
        }
        "agent" => {
            let task = Task::from_json(m);
            let id = task.id.clone();
            st.tasks.update(|ts| match ts.iter_mut().find(|t| t.id == id) {
                Some(t) => *t = task.clone(),
                None => ts.insert(0, task.clone()),
            });
            if st.open.with_untracked(|o| o.as_deref() == Some(id.as_str())) {
                st.detail.update(|d| {
                    if let Some(d) = d {
                        d.task = task;
                    }
                });
                st.ask_detail(send);
            }
            st.spin_on();
            true
        }
        "agent_tasks" => {
            let tasks: Vec<Task> = m["items"].as_array().into_iter().flatten().map(Task::from_json).collect();
            st.tasks.set(tasks);
            st.spin_on();
            true
        }
        "agent_task" => {
            let id = m["id"].as_str().unwrap_or("");
            if st.open.with_untracked(|o| o.as_deref() == Some(id))
                && let Some(d) = Detail::from_json(m)
                && st.detail.with_untracked(|was| was.as_ref() != Some(&d))
            {
                st.detail.set(Some(d));
                st.spin_on();
            }
            true
        }
        "agent_task_stop" => {
            if let Some(e) = m["error"].as_str() {
                toast::error(ui, e);
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(done: bool, ok: bool, text: &str) -> Tool {
        Tool {
            name: "task".into(),
            id: "c1".into(),
            done,
            ok,
            text: text.into(),
            arguments: json!({"description": "Review the parser", "prompt": "Read it.", "agent": "reviewer"}),
            ..Tool::default()
        }
    }

    #[test]
    fn a_rows_line_says_who_when_how_long_and_how_much() {
        let t = Task { assistant: "reviewer".into(), tokens: 3400, calls: 5, ..Task::default() };
        assert_eq!(meta(&t, Some(2_000), 12_400), "as reviewer · now · 12 s · 3.4k tokens · 5 tool calls");
        let one = Task { calls: 1, ..Task::default() };
        assert_eq!(meta(&one, None, 61_000), "as sub-agent · 1 min 1 s · 1 tool call");
        assert_eq!(
            [ago(59_000), ago(300_000), ago(7_200_000), ago(200_000_000)],
            ["now", "5 min ago", "2 h ago", "2 d ago"]
        );
        assert_eq!([tokens(800), tokens(3000), tokens(1_250_000)], ["800 tokens", "3k tokens", "1.2M tokens"]);
        assert_eq!(duration(7_380_000), "2 h 3 min");
    }

    #[test]
    fn a_report_loses_the_tools_first_line() {
        let r = "reviewer reports (Review the parser, 3 steps):\n## Findings\nIt is fine.";
        assert_eq!(report_of(r), "## Findings\nIt is fine.");
        assert_eq!(first_line(report_of(r)), "Findings");
        assert_eq!(report_of("plain words"), "plain words");
        assert_eq!(first_line("\n\n  "), "");
    }

    #[test]
    fn a_task_call_says_where_it_stands() {
        assert_eq!(call_status(&tool(false, true, "")), "working");
        assert_eq!(call_status(&tool(true, true, "x")), "done");
        assert_eq!(call_status(&tool(true, false, "reviewer could not finish")), "failed");
        assert_eq!(call_status(&tool(true, false, "The person stopped reviewer before it reported.")), "stopped");
        assert_eq!(call_status(&Tool { status: "skipped".into(), ..tool(true, false, "") }), "stopped");
        assert_eq!(description(&tool(true, true, "")), "Review the parser");
    }

    #[test]
    fn a_call_from_before_a_restart_opens_with_what_it_said() {
        let d = Detail::from_tool(&tool(true, true, "reviewer reports (R, 2 steps):\nAll good."));
        assert_eq!(
            (d.task.title.as_str(), d.task.assistant.as_str(), d.task.status.as_str()),
            ("Review the parser", "reviewer", "done")
        );
        assert_eq!((d.prompt.as_str(), d.report.as_str(), d.kept), ("Read it.", "All good.", false));
        let f = Detail::from_tool(&tool(true, false, "reviewer could not finish: gone"));
        assert_eq!((f.report.as_str(), f.error.as_deref()), ("", Some("reviewer could not finish: gone")));
    }

    #[test]
    fn a_detail_is_read_with_its_steps() {
        let v = json!({"type": "agent_task", "id": "c1", "status": "working", "title": "T", "assistant": "sub-agent",
            "now": "grep fn", "ms": 900, "tokens": 0, "calls": 1, "prompt": "Find it.", "tools": ["grep"],
            "report": "", "error": null, "work": [
                [{"kind": "thinking", "text": "Hmm"}, {"kind": "tool", "id": "g", "name": "grep", "subject": "fn",
                  "done": true, "ok": false, "ms": 4, "text": "none"}],
                []]});
        let d = Detail::from_json(&v).expect("a detail");
        assert_eq!(d.steps.len(), 1, "an empty step is no step");
        assert_eq!(d.steps[0][0], Piece::Thinking("Hmm".into()));
        assert!(matches!(&d.steps[0][1], Piece::Call { ok: false, ms: 4, .. }));
        assert_eq!((d.tools.clone(), d.error, d.task.now.as_str()), (vec!["grep".to_string()], None, "grep fn"));
        assert!(
            Detail::from_json(&json!({"type": "agent_task", "id": "c1", "error": "There is no such task."})).is_none()
        );
    }

    #[test]
    fn its_last_words_are_shown_once_as_its_report() {
        let v = json!({"type": "agent_task", "id": "c1", "status": "done", "report": "All good.", "work": [
            [{"kind": "thinking", "text": "Hmm"}], [{"kind": "text", "text": "All good. "}]]});
        let d = Detail::from_json(&v).expect("a detail");
        assert_eq!(d.steps, [vec![Piece::Thinking("Hmm".into())]]);
        assert_eq!(d.report, "All good.");
    }

    #[test]
    fn a_group_of_task_calls_says_how_many_work() {
        assert_eq!(crew_line(&["working", "done", "working"]), "3 sub-agents · 2 working");
        assert_eq!(crew_line(&["done", "done"]), "2 sub-agents · done");
        assert_eq!(crew_line(&["done", "failed"]), "2 sub-agents · 1 failed");
    }
}
