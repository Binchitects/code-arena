//! The Automation view: what runs on its own, as the old Code Arena's has it, in three sections.
//!
//! - **Schedules here**: a prompt Code Arena runs in a folder every so often, as a session of its own. Each shows
//!   its name (struck through while it is off), how often, "mode plan · next 2026-11-28 09:00", its prompt and
//!   "Last 2026-11-28 08:00: ok"; while the pointer is on it, Run it now, Turn off (or on) and Remove. **New
//!   schedule** (the section's +) opens a form: its name, how often (every 30 minutes to every 7 days), the time of
//!   day for a day's, its mode, its folder and what it does. A note says they run while this IDE is open, and with
//!   it closed once `code-arena schedule install` set the system's timer.
//! - **Agents in Arena**: the person's Agent Studio agents and those shared with them. Run opens a form for what it
//!   is given; the run is then followed step by step under the list until it ends.
//! - **Scheduled tasks in Arena**: each with its cron, its last run and its next, and Run it now in Arena.
//!
//! The session's side is the driver's `ide_automation.rs`; what a change said comes as a corner bubble.

use std::time::Duration;

use floem::action::exec_after;
use floem::event::EventListener;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo, create_rw_signal};
use floem::style::{CursorStyle, FlexWrap, Position, TextOverflow};
use floem::views::{
    Decorators, container, dyn_container, dyn_stack, empty, h_stack, h_stack_from_iter, label, scroll, stack, tooltip,
    v_stack,
};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layers::{self, Place};
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::widgets::form::{self, Form};
use crate::widgets::{Theme, action, bold, caps, dialog, line, toast};

/// Whether the view is in the window.
pub const READY: bool = true;

/// The command that asks for everything again.
const REFRESH: &str = "codeArena.automation.refresh";
/// "New Schedule…".
const NEW: &str = "codeArena.automation.newSchedule";

/// How often a schedule may run, as the form offers it: its value for the driver, and its words.
pub const EVERY: [(&str, &str); 7] = [
    ("30m", "30 minutes"),
    ("1h", "1 hour"),
    ("2h", "2 hours"),
    ("6h", "6 hours"),
    ("12h", "12 hours"),
    ("1d", "1 day"),
    ("7d", "7 days"),
];

/// The modes a schedule runs in: its value, its words, and what it means.
const MODES: [(&str, &str, &str); 4] = [
    ("plan", "Reads only", "It reads and says what it found or would do; it changes nothing."),
    ("ask", "Asks first", "Nobody is there to allow a change: each one is refused."),
    ("auto-edit", "Edits files", "It changes files in its folder; its commands are refused."),
    ("yolo", "Does anything", "Every change and command is allowed. Only for a folder you can lose."),
];

/// The note under the schedules.
const NOTE: &str = "Schedules run while this IDE is open, and with it closed once `code-arena schedule install` has \
                    set up a timer of the system's.";

/// How long an agent's run is followed here before it is left to Arena (it goes on there).
const FOLLOW: Duration = Duration::from_secs(600);
/// How often a run followed is asked for.
const EVERY_RUN: Duration = Duration::from_secs(2);

/// A schedule, as the driver lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Sched {
    pub name: String,
    pub prompt: String,
    pub folder: String,
    /// "every 2h", "every 1d at 09:00".
    pub timing: String,
    pub mode: String,
    pub on: bool,
    pub running: bool,
    pub next_words: String,
    pub last_words: Option<String>,
    pub last_status: Option<String>,
}

impl Sched {
    pub fn of(v: &Value) -> Sched {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        Sched {
            name: s("name"),
            prompt: s("prompt"),
            folder: s("folder"),
            timing: s("timing"),
            mode: s("mode"),
            on: v["on"].as_bool().unwrap_or(true),
            running: v["running"].as_bool().unwrap_or(false),
            next_words: s("next_words"),
            last_words: v["last_words"].as_str().map(str::to_string),
            last_status: v["last_status"].as_str().map(str::to_string),
        }
    }

    /// "mode plan · next 2026-11-28 09:00" (no next while it is off).
    pub fn mode_line(&self) -> String {
        if self.on && !self.next_words.is_empty() {
            format!("mode {} \u{b7} next {}", self.mode, self.next_words)
        } else {
            format!("mode {} \u{b7} off", self.mode)
        }
    }

    /// "Last 2026-11-28 08:00: ok", "Running now…", or "Not run yet".
    pub fn last_line(&self) -> String {
        if self.running {
            return "Running now\u{2026}".into();
        }
        match (&self.last_words, &self.last_status) {
            (Some(w), Some(s)) => format!("Last {w}: {s}"),
            (Some(w), None) => format!("Last {w}"),
            _ => "Not run yet".into(),
        }
    }
}

/// An agent in Arena.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub mine: bool,
    pub owner: String,
}

impl Agent {
    pub fn of(v: &Value) -> Agent {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        Agent {
            id: s("id"),
            name: s("name"),
            description: s("description"),
            enabled: v["enabled"].as_bool().unwrap_or(true),
            mine: v["mine"].as_bool().unwrap_or(true),
            owner: s("owner"),
        }
    }

    /// What is said under its name: its description, whose it is when it is shared, and whether it is off.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if !self.description.is_empty() {
            parts.push(self.description.clone());
        }
        if !self.mine {
            parts.push(format!("Shared by {}", if self.owner.is_empty() { "someone" } else { &self.owner }));
        }
        if !self.enabled {
            parts.push("Off".into());
        }
        parts.join(" \u{b7} ")
    }
}

/// A scheduled task in Arena.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub prompt: String,
    /// Its cron and time zone, or the events it runs on.
    pub when: String,
    pub enabled: bool,
    pub running: bool,
    pub next_words: Option<String>,
    pub last_status: Option<String>,
    pub last_words: Option<String>,
}

impl Task {
    pub fn of(v: &Value) -> Task {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        let o = |k: &str| v[k].as_str().filter(|x| !x.is_empty()).map(str::to_string);
        Task {
            id: s("id"),
            name: s("name"),
            prompt: s("prompt"),
            when: s("when"),
            enabled: v["enabled"].as_bool().unwrap_or(true),
            running: v["running"].as_bool().unwrap_or(false),
            next_words: o("next_words"),
            last_status: o("last_status"),
            last_words: o("last_words"),
        }
    }

    /// "0 9 * * * Asia/Tehran · last run done 2026-11-27 09:00 · next 2026-11-28 09:00".
    pub fn line(&self) -> String {
        let mut s = self.when.clone();
        if self.running {
            s.push_str(" \u{b7} running now");
        } else if let Some(st) = &self.last_status {
            s.push_str(&format!(" \u{b7} last run {st}"));
            if let Some(w) = &self.last_words {
                s.push_str(&format!(" {w}"));
            }
        }
        if !self.enabled {
            s.push_str(" \u{b7} off");
        } else if let Some(n) = &self.next_words {
            s.push_str(&format!(" \u{b7} next {n}"));
        }
        s
    }
}

/// One step of a run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Step {
    pub name: String,
    pub status: String,
    /// Its output, or its error when it failed.
    pub said: String,
    pub failed: bool,
}

/// An agent's run, followed here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    pub agent: String,
    pub run: String,
    pub status: String,
    pub ended: bool,
    pub waiting: bool,
    /// How it ended, in a line (once it ended or waits).
    pub told: String,
    pub error: Option<String>,
    pub steps: Vec<Step>,
    /// How many times it was asked for.
    pub asked: u32,
}

impl Run {
    /// An `arena_agent_run_get` reply taken into the run followed.
    pub fn took(&mut self, v: &Value) {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        if let Some(e) = v["error"].as_str().filter(|_| v["status"].is_null()) {
            self.error = Some(e.to_string());
            self.ended = true;
            return;
        }
        self.status = s("status");
        self.ended = v["ended"].as_bool().unwrap_or(false);
        self.waiting = v["waiting"].as_bool().unwrap_or(false);
        self.told = s("told");
        self.steps = v["steps"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|st| {
                        let error = st["error"].as_str().filter(|e| !e.is_empty());
                        Step {
                            name: st["name"].as_str().or(st["kind"].as_str()).unwrap_or("Step").to_string(),
                            status: st["status"].as_str().unwrap_or("").to_string(),
                            said: error.or(st["output"].as_str()).unwrap_or("").trim().to_string(),
                            failed: error.is_some(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
    }

    /// Whether to ask for it again: it goes on, and it was not followed too long.
    pub fn again(&self) -> bool {
        !self.ended
            && !self.waiting
            && self.error.is_none()
            && self.asked * (EVERY_RUN.as_secs() as u32) < FOLLOW.as_secs() as u32
    }

    /// Its line over the steps: "Reviewer: running", or how it ended.
    pub fn head(&self) -> String {
        if let Some(e) = &self.error {
            return format!("{}: {e}", self.agent);
        }
        if self.ended || self.waiting {
            return self.told.clone();
        }
        if !self.again() {
            return format!("{} goes on in Arena; its runs are there.", self.agent);
        }
        format!("{}: {}\u{2026}", self.agent, if self.status.is_empty() { "starting" } else { &self.status })
    }
}

/// A step's codicon by its status.
pub fn step_icon(status: &str) -> &'static str {
    match status {
        "done" | "ok" | "succeeded" => "pass",
        "failed" | "error" => "error",
        "running" => "loading",
        "skipped" | "stopped" => "circle-slash",
        _ => "circle-large",
    }
}

/// What a new schedule sends, from the form's fields; why not, when a field is missing or wrong.
pub fn save_message(
    name: &str,
    every: &str,
    at: &str,
    mode: &str,
    folder: &str,
    prompt: &str,
) -> Result<Value, &'static str> {
    if name.trim().is_empty() {
        return Err("Give the schedule a name.");
    }
    if prompt.trim().is_empty() {
        return Err("Say what it does.");
    }
    if folder.trim().is_empty() {
        return Err("Say the folder it runs in.");
    }
    let daily = every.ends_with('d');
    let at = at.trim();
    if daily && !at.is_empty() && !is_hhmm(at) {
        return Err("The time of day is like 09:00.");
    }
    let mut m = json!({"type": "schedule_save", "name": name.trim(), "every": every, "mode": mode,
                       "folder": folder.trim(), "prompt": prompt.trim()});
    if daily && !at.is_empty() {
        m["at"] = json!(at);
    }
    Ok(m)
}

/// "09:00".
fn is_hhmm(at: &str) -> bool {
    at.split_once(':').is_some_and(|(h, m)| {
        h.len() <= 2 && m.len() == 2 && h.parse::<u32>().is_ok_and(|h| h < 24) && m.parse::<u32>().is_ok_and(|m| m < 60)
    })
}

/// A list from Arena: its items, or why there are none to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listed<T> {
    pub items: Vec<T>,
    pub error: Option<String>,
}

/// The view's state in a window.
#[derive(Clone, Copy)]
struct Auto {
    schedules: RwSignal<Option<Vec<Sched>>>,
    /// The session's folder, where a new schedule runs unless another is said.
    folder: RwSignal<String>,
    agents: RwSignal<Option<Listed<Agent>>>,
    tasks: RwSignal<Option<Listed<Task>>>,
    /// Which sections are open: Schedules here, Agents in Arena, Scheduled tasks in Arena.
    open: [RwSignal<bool>; 3],
    run: RwSignal<Option<Run>>,
    form: Form,
    name: RwSignal<String>,
    every: RwSignal<&'static str>,
    at: RwSignal<String>,
    mode: RwSignal<&'static str>,
    dir: RwSignal<String>,
    prompt: RwSignal<String>,
    /// The form for what an agent is given, and the agent it runs.
    give: Form,
    given: RwSignal<String>,
    agent: RwSignal<Option<Agent>>,
}

impl crate::ext::State for Auto {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        Auto {
            schedules: s.create_rw_signal(None),
            folder: s.create_rw_signal(String::new()),
            agents: s.create_rw_signal(None),
            tasks: s.create_rw_signal(None),
            open: [s.create_rw_signal(true), s.create_rw_signal(true), s.create_rw_signal(true)],
            run: s.create_rw_signal(None),
            form: Form::new(ui),
            name: s.create_rw_signal(String::new()),
            every: s.create_rw_signal("1d"),
            at: s.create_rw_signal("09:00".to_string()),
            mode: s.create_rw_signal("plan"),
            dir: s.create_rw_signal(String::new()),
            prompt: s.create_rw_signal(String::new()),
            give: Form::new(ui),
            given: s.create_rw_signal(String::new()),
            agent: s.create_rw_signal(None),
        }
    }
}

fn state(ui: Ui) -> Auto {
    ui.state::<Auto>()
}

/// Everything asked for again.
fn refresh(send: &Send) {
    send(json!({"type": "automation"}));
    send(json!({"type": "arena_agents"}));
    send(json!({"type": "arena_tasks"}));
}

/// The New schedule form opened, its folder the session's.
fn new_schedule(ui: Ui) {
    let st = state(ui);
    let folder = st.folder.get_untracked();
    if st.dir.with_untracked(String::is_empty) && !folder.is_empty() {
        st.dir.set(folder);
    }
    st.form.show(None);
}

/// The schedule typed sent; the form closes when the driver says it was added.
fn save(ui: Ui, send: &Send) {
    let st = state(ui);
    let m = save_message(
        &st.name.get_untracked(),
        st.every.get_untracked(),
        &st.at.get_untracked(),
        st.mode.get_untracked(),
        &st.dir.get_untracked(),
        &st.prompt.get_untracked(),
    );
    match m {
        Ok(m) => send(m),
        Err(why) => {
            toast::error(ui, why);
        }
    }
}

/// The agent run with what it is given.
fn run_agent(ui: Ui, send: &Send) {
    let st = state(ui);
    let Some(a) = st.agent.get_untracked() else { return };
    send(json!({"type": "arena_agent_run", "id": a.id, "text": st.given.get_untracked().trim()}));
    st.run.set(Some(Run { agent: a.name.clone(), ..Run::default() }));
    st.given.set(String::new());
    st.give.close();
}

/// Removes one, once the person said so.
fn remove(ui: Ui, send: &Send, name: &str) {
    let (send, n) = (send.clone(), name.to_string());
    dialog::ask(&format!("Remove the schedule \u{201c}{name}\u{201d}?"))
        .detail("It does not run again. Its last output stays in the config folder's schedules folder.")
        .kind(dialog::Kind::Warning)
        .button("Remove")
        .destructive()
        .open(ui, move |a| {
            if a.yes() {
                send(json!({"type": "schedule_remove", "name": n}));
            }
        });
}

/// Wrapped words, in the description's colour.
fn desc(t: Theme, words: impl Fn() -> String + 'static) -> impl IntoView {
    label(words).style(move |s| {
        s.font_size(12.0)
            .line_height(1.4)
            .color(t.get().desc)
            .text_overflow(TextOverflow::Wrap)
            .min_width(0.0)
            .max_width_full()
    })
}

/// A section's head, as VS Code's: its chevron, its title in capitals, and its actions at the right.
fn head(ui: Ui, title: &'static str, open: RwSignal<bool>, acts: AnyView) -> impl IntoView {
    let t = ui.colors;
    let over = create_rw_signal(false);
    h_stack((
        container(dyn_container(move || open.get(), |o| icon(if o { "chevron-down" } else { "chevron-right" }, 16.0)))
            .style(|s| s.flex_shrink(0.0_f32)),
        label(move || title.to_uppercase()).style(|s| bold(caps(s)).flex_grow(1.0_f32).min_width(0.0)),
        container(acts).style(move |s| s.flex_shrink(0.0_f32).apply_if(!over.get() || !open.get(), |s| s.hide())),
    ))
    .on_click_stop(move |_| open.update(|o| *o = !*o))
    .on_event_cont(EventListener::PointerEnter, move |_| over.set(true))
    .on_event_cont(EventListener::PointerLeave, move |_| over.set(false))
    .style(move |s| {
        let c = t.get();
        line(s, c.section_border, 't')
            .width_full()
            .height(22.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .gap(2.0)
            .padding_left(2.0)
            .padding_right(8.0)
            .background(c.section_bg)
            .color(c.section_fg)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
    })
}

/// An action with its tooltip.
fn act(t: Theme, name: &'static str, tip: &'static str, on: impl Fn() + 'static) -> AnyView {
    tooltip(action(t, name, on), move || label(move || tip)).into_any()
}

/// A row's name, struck through while it is off (and dimmed).
fn name_of(t: Theme, name: String, off: bool) -> impl IntoView {
    stack((
        label(move || name.clone()).style(move |s| {
            let c = t.get();
            s.font_size(13.0)
                .line_height(1.4)
                .color(if off { c.desc } else { c.fg })
                .text_overflow(TextOverflow::Ellipsis)
        }),
        empty().style(move |s| {
            s.position(Position::Absolute)
                .inset_left(0.0)
                .inset_right(0.0)
                .inset_top(9.0)
                .height(1.0)
                .background(t.get().desc)
                .apply_if(!off, |s| s.hide())
        }),
    ))
    .style(|s| s.min_width(0.0).flex_shrink(1.0_f32))
}

/// A row of a section: a codicon, its name with its actions while the pointer is on it, and its lines under.
fn item(t: Theme, glyph: &'static str, top: AnyView, acts: Vec<AnyView>, lines: Vec<AnyView>) -> impl IntoView {
    let over = create_rw_signal(false);
    let acts = h_stack_from_iter(acts).style(move |s| s.flex_shrink(0.0_f32).apply_if(!over.get(), |s| s.hide()));
    let top = h_stack((top, empty().style(|s| s.flex_grow(1.0_f32)), acts))
        .style(|s| s.width_full().min_width(0.0).items_center().min_height(22.0));
    let body = v_stack((top, floem::views::v_stack_from_iter(lines).style(|s| s.gap(1.0).width_full().min_width(0.0))))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0));
    h_stack((icon(glyph, 16.0).style(move |s| s.color(t.get().icon).margin_top(3.0).flex_shrink(0.0_f32)), body))
        .on_event_cont(EventListener::PointerEnter, move |_| over.set(true))
        .on_event_cont(EventListener::PointerLeave, move |_| over.set(false))
        .style(move |s| {
            s.gap(6.0)
                .width_full()
                .items_start()
                .padding_left(20.0)
                .padding_right(8.0)
                .padding_vert(3.0)
                .hover(|s| s.background(t.get().list_hover))
        })
}

/// A schedule's row.
fn schedule_row(ui: Ui, send: Send, s: Sched) -> AnyView {
    let t = ui.colors;
    let (n1, n2, n3) = (s.name.clone(), s.name.clone(), s.name.clone());
    let (s1, s2, s3) = (send.clone(), send.clone(), send);
    let on = s.on;
    let acts = vec![
        act(t, "play", "Run it now", move || s1(json!({"type": "schedule_run", "name": n1}))),
        if on {
            act(t, "circle-slash", "Turn off", move || s2(json!({"type": "schedule_on", "name": n2, "on": false})))
        } else {
            act(t, "pass", "Turn on", move || s2(json!({"type": "schedule_on", "name": n2, "on": true})))
        },
        act(t, "trash", "Remove", move || remove(ui, &s3, &n3)),
    ];
    let (timing, mode, prompt, last, folder) =
        (s.timing.clone(), s.mode_line(), s.prompt.clone(), s.last_line(), s.folder.clone());
    let failed = s.last_status.as_deref().is_some_and(|x| x.starts_with("failed"));
    let lines = vec![
        desc(t, move || timing.clone()).into_any(),
        desc(t, move || mode.clone()).into_any(),
        tooltip(
            label(move || prompt.clone()).style(move |s| {
                s.font_size(12.0).line_height(1.4).color(t.get().fg).text_overflow(TextOverflow::Wrap).min_width(0.0)
            }),
            move || {
                label({
                    let f = folder.clone();
                    move || format!("In {f}")
                })
            },
        )
        .into_any(),
        label(move || last.clone())
            .style(move |s| {
                let c = t.get();
                s.font_size(12.0).line_height(1.4).color(if failed { c.error } else { c.desc }).min_width(0.0)
            })
            .into_any(),
    ];
    let glyph = if s.running { "loading" } else { "calendar" };
    item(t, glyph, name_of(t, s.name.clone(), !on).into_any(), acts, lines).into_any()
}

/// An agent's row.
fn agent_row(ui: Ui, a: Agent) -> AnyView {
    let t = ui.colors;
    let st = state(ui);
    let line = a.line();
    let no_line = line.is_empty();
    let shown = a.clone();
    let acts = vec![act(t, "play", "Run…", move || {
        if st.agent.with_untracked(|x| x.as_ref() != Some(&shown)) {
            st.agent.set(Some(shown.clone()));
        }
        st.give.show(None);
    })];
    let lines = vec![desc(t, move || line.clone()).style(move |s| s.apply_if(no_line, |s| s.hide())).into_any()];
    item(t, "hubot", name_of(t, a.name.clone(), !a.enabled).into_any(), acts, lines).into_any()
}

/// A scheduled task's row.
fn task_row(ui: Ui, send: Send, k: Task) -> AnyView {
    let t = ui.colors;
    let (id, name) = (k.id.clone(), k.name.clone());
    let acts = vec![act(t, "play", "Run it now in Arena", move || {
        send(json!({"type": "arena_task_run", "id": id, "name": name}))
    })];
    let (line, prompt) = (k.line(), k.prompt.clone());
    let no_prompt = prompt.is_empty();
    let lines = vec![
        desc(t, move || line.clone()).into_any(),
        label(move || prompt.clone())
            .style(move |s| {
                s.font_size(12.0)
                    .line_height(1.4)
                    .color(t.get().fg)
                    .text_overflow(TextOverflow::Wrap)
                    .min_width(0.0)
                    .apply_if(no_prompt, |s| s.hide())
            })
            .into_any(),
    ];
    let glyph = if k.running { "loading" } else { "history" };
    item(t, glyph, name_of(t, k.name.clone(), !k.enabled).into_any(), acts, lines).into_any()
}

/// The run followed: its head line and its steps.
fn run_view(ui: Ui) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let head = create_memo(move |_| st.run.with(|r| r.as_ref().map(Run::head)));
    let steps = create_memo(move |_| st.run.with(|r| r.as_ref().map(|r| r.steps.clone()).unwrap_or_default()));
    let close = act(t, "close", "Close", move || st.run.set(None));
    let top = h_stack((
        label(move || head.get().unwrap_or_default()).style(move |s| {
            s.font_size(12.0)
                .line_height(1.4)
                .color(t.get().fg)
                .text_overflow(TextOverflow::Wrap)
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
        close,
    ))
    .style(|s| s.gap(4.0).items_start().width_full());
    let list = dyn_stack(
        move || steps.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, s): &(usize, Step)| (*i, s.clone()),
        move |(_, s)| {
            let (name, said, failed) = (s.name.clone(), s.said.clone(), s.failed);
            let quiet = said.is_empty();
            h_stack((
                icon(step_icon(&s.status), 14.0).style(move |st| {
                    let c = t.get();
                    st.margin_top(2.0).flex_shrink(0.0_f32).color(if failed { c.error } else { c.icon })
                }),
                v_stack((
                    label(move || name.clone()).style(|s| s.font_size(12.0).line_height(1.4)),
                    desc(t, move || said.chars().take(400).collect()).style(move |s| s.apply_if(quiet, |s| s.hide())),
                ))
                .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0)),
            ))
            .style(|s| s.gap(6.0).items_start().width_full())
        },
    )
    .style(|s| s.flex_col().gap(4.0).width_full());
    v_stack((top, list)).style(move |s| {
        let c = t.get();
        s.gap(6.0)
            .margin_left(20.0)
            .margin_right(8.0)
            .margin_vert(4.0)
            .padding(8.0)
            .border(1.0)
            .border_radius(4.0)
            .border_color(c.widget_border)
            .apply_if(head.get().is_none(), |s| s.hide())
    })
}

/// What a section from Arena shows while it has nothing to list.
fn waiting_or(t: Theme, said: impl Fn() -> Option<String> + 'static) -> impl IntoView {
    let said = create_memo(move |_| said());
    desc(t, move || said.get().unwrap_or_default()).style(move |s| {
        s.padding_left(22.0).padding_right(8.0).padding_vert(4.0).apply_if(said.get().is_none(), |s| s.hide())
    })
}

/// The view's own content.
pub fn view(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    if st.schedules.with_untracked(Option::is_none) {
        send(json!({"type": "automation"}));
    }
    if st.agents.with_untracked(Option::is_none) {
        send(json!({"type": "arena_agents"}));
    }
    if st.tasks.with_untracked(Option::is_none) {
        send(json!({"type": "arena_tasks"}));
    }
    let [o1, o2, o3] = st.open;

    let schedules = {
        let send = send.clone();
        let rows = create_memo(move |_| st.schedules.get().unwrap_or_default());
        dyn_stack(move || rows.get(), Clone::clone, move |s| schedule_row(ui, send.clone(), s))
            .style(|s| s.flex_col().width_full())
    };
    let none = create_memo(move |_| st.schedules.with(|s| s.as_ref().is_some_and(Vec::is_empty)));
    let sched_body = v_stack((
        schedules,
        desc(t, || "No schedules yet.".into()).style(move |s| s.padding_left(22.0).apply_if(!none.get(), |s| s.hide())),
        container(crate::widgets::button(t, "New Schedule\u{2026}", true, move || new_schedule(ui)))
            .style(|s| s.padding_left(22.0).padding_top(6.0)),
        desc(t, || NOTE.into()).style(|s| s.padding_left(22.0).padding_right(8.0).padding_vert(6.0)),
    ))
    .style(move |s| s.width_full().apply_if(!o1.get(), |s| s.hide()));
    let sched_head = head(ui, "Schedules here", o1, act(t, "add", "New Schedule\u{2026}", move || new_schedule(ui)));

    let agents = {
        let rows = create_memo(move |_| st.agents.with(|a| a.as_ref().map(|a| a.items.clone()).unwrap_or_default()));
        dyn_stack(move || rows.get(), Clone::clone, move |a| agent_row(ui, a)).style(|s| s.flex_col().width_full())
    };
    let agents_said = waiting_or(t, move || {
        st.agents.with(|a| match a {
            None => None,
            Some(a) if a.error.is_some() => a.error.clone(),
            Some(a) if a.items.is_empty() => Some("You have no agents in Arena yet (Agent Studio makes them).".into()),
            Some(_) => None,
        })
    });
    let agents_body =
        v_stack((agents, agents_said, run_view(ui))).style(move |s| s.width_full().apply_if(!o2.get(), |s| s.hide()));
    let again = send.clone();
    let agents_head =
        head(ui, "Agents in Arena", o2, act(t, "refresh", "Refresh", move || again(json!({"type": "arena_agents"}))));

    let tasks = {
        let send = send.clone();
        let rows = create_memo(move |_| st.tasks.with(|a| a.as_ref().map(|a| a.items.clone()).unwrap_or_default()));
        dyn_stack(move || rows.get(), Clone::clone, move |k| task_row(ui, send.clone(), k))
            .style(|s| s.flex_col().width_full())
    };
    let tasks_said = waiting_or(t, move || {
        st.tasks.with(|a| match a {
            None => None,
            Some(a) if a.error.is_some() => a.error.clone(),
            Some(a) if a.items.is_empty() => Some("You have no scheduled tasks in Arena yet.".into()),
            Some(_) => None,
        })
    });
    let tasks_body = v_stack((tasks, tasks_said)).style(move |s| s.width_full().apply_if(!o3.get(), |s| s.hide()));
    let again = send.clone();
    let tasks_head = head(
        ui,
        "Scheduled tasks in Arena",
        o3,
        act(t, "refresh", "Refresh", move || again(json!({"type": "arena_tasks"}))),
    );

    scroll(
        v_stack((sched_head, sched_body, agents_head, agents_body, tasks_head, tasks_body))
            .style(|s| s.width_full().min_width(0.0).padding_bottom(10.0)),
    )
    .style(move |s| s.size_full().min_height(0.0).color(t.get().side_fg))
    .into_any()
}

/// The New schedule form over the window.
fn schedule_form(ui: Ui, send: Send) -> AnyView {
    let (t, st) = (ui.colors, state(ui));
    let f = st.form;
    let go = {
        let send = send.clone();
        move || save(ui, &send)
    };
    let name = f.named(ui, "Name", f.input(ui, st.name, "nightly-tests", go.clone()));
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
    .style(|s| s.gap(4.0).flex_wrap(FlexWrap::Wrap));
    let every = f.named(ui, "Every", chips.into_any());
    let at = container(f.named(ui, "At", f.input(ui, st.at, "09:00", go.clone())))
        .style(move |s| s.width_full().apply_if(!st.every.get().ends_with('d'), |s| s.hide()))
        .into_any();
    let modes = floem::views::v_stack_from_iter(
        MODES.iter().map(|&(value, words, hint)| f.radio(ui, st.mode, value, words, hint)),
    )
    .style(|s| s.gap(6.0));
    let mode = f.named(ui, "Mode", modes.into_any());
    let dir = f.named(ui, "Folder", f.input(ui, st.dir, "The folder it runs in", go.clone()));
    let prompt = f.named(
        ui,
        "What to do",
        f.input(ui, st.prompt, "Run the tests; if one fails, find why and say so.", go.clone()),
    );
    let ready = move || st.name.with(|n| !n.trim().is_empty()) && st.prompt.with(|p| !p.trim().is_empty());
    let buttons = form::buttons(vec![
        form::button(ui, || "Cancel".into(), false, || true, move || f.close()),
        form::button(ui, || "Add the Schedule".into(), true, ready, go),
    ]);
    f.host(
        ui,
        || "New Schedule".into(),
        || {
            "A prompt Code Arena runs on its own in a folder, as a session of its own (synced to Arena as any is)."
                .into()
        },
        vec![name, every, at, mode, dir, prompt],
        buttons,
    )
}

/// The form for what an agent is given.
fn give_form(ui: Ui, send: Send) -> AnyView {
    let st = state(ui);
    let f = st.give;
    let go = move || run_agent(ui, &send);
    let given =
        f.named(ui, "What it is given", f.input(ui, st.given, "A text for the agent (it may need none)", go.clone()));
    let buttons = form::buttons(vec![
        form::button(ui, || "Cancel".into(), false, || true, move || f.close()),
        form::button(ui, || "Run".into(), true, || true, go),
    ]);
    f.host(
        ui,
        move || st.agent.with(|a| a.as_ref().map(|a| format!("Run {}", a.name)).unwrap_or_default()),
        move || {
            st.agent.with(|a| {
                a.as_ref()
                    .map(|a| {
                        if a.description.is_empty() {
                            "It runs in Arena; how it goes is shown here step by step.".to_string()
                        } else {
                            a.description.clone()
                        }
                    })
                    .unwrap_or_default()
            })
        },
        vec![given],
        buttons,
    )
}

/// The actions in its title: none of its own (Refresh is the title menu's).
pub fn actions(_ui: Ui, _send: Send) -> Option<floem::AnyView> {
    None
}

/// What it adds to the window's registries, once at start: Refresh in its title, New Schedule…, and its forms.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{Menu, item};
    add(command(REFRESH, "Automation", "Refresh", |_, send| refresh(send)).hidden());
    add(command(NEW, "Automation", "New Schedule\u{2026}", |ui, _| new_schedule(ui)));
    crate::menus::add(Menu::ViewTitle(View::Automation), item(NEW).icon("add").group("navigation").order(1));
    crate::menus::add(Menu::ViewTitle(View::Automation), item(REFRESH).icon("refresh").group("navigation").order(2));
    layers::add(Place::Over, "automation.schedule", 30, |ui, send| schedule_form(ui, send));
    layers::add(Place::Over, "automation.give", 30, |ui, send| give_form(ui, send));
}

/// The next ask for the run followed, while it goes on.
fn follow(ui: Ui, send: Send, run: String) {
    exec_after(EVERY_RUN, move |_| {
        let st = state(ui);
        if st.run.with_untracked(|r| r.as_ref().is_some_and(|r| r.run == run && r.again())) {
            st.run.update(|r| {
                if let Some(r) = r {
                    r.asked += 1;
                }
            });
            send(json!({"type": "arena_agent_run_get", "run": run}));
        }
    });
}

/// A list from Arena, read.
fn listed<T>(m: &Value, key: &str, of: impl Fn(&Value) -> T) -> Listed<T> {
    Listed {
        items: m[key].as_array().map(|a| a.iter().map(of).collect()).unwrap_or_default(),
        error: m["error"].as_str().map(str::to_string),
    }
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "automation" => {
            let got: Vec<Sched> =
                m["schedules"].as_array().map(|a| a.iter().map(Sched::of).collect()).unwrap_or_default();
            if st.schedules.with_untracked(|s| s.as_ref() != Some(&got)) {
                st.schedules.set(Some(got));
            }
            if let Some(f) = m["folder"].as_str()
                && st.folder.with_untracked(|x| x != f)
            {
                st.folder.set(f.to_string());
            }
            true
        }
        "arena_agents" => {
            let got = listed(m, "agents", Agent::of);
            if st.agents.with_untracked(|a| a.as_ref() != Some(&got)) {
                st.agents.set(Some(got));
            }
            true
        }
        "arena_tasks" => {
            let got = listed(m, "tasks", Task::of);
            if st.tasks.with_untracked(|a| a.as_ref() != Some(&got)) {
                st.tasks.set(Some(got));
            }
            true
        }
        "arena_agent_run" => {
            match (m["run"].as_str(), m["error"].as_str()) {
                (Some(run), _) => {
                    st.run.update(|r| {
                        if let Some(r) = r {
                            r.run = run.to_string();
                        }
                    });
                    send(json!({"type": "arena_agent_run_get", "run": run}));
                }
                (None, e) => {
                    let e = e.unwrap_or("The agent did not start.").to_string();
                    st.run.update(|r| {
                        if let Some(r) = r {
                            r.error = Some(e.clone());
                        }
                    });
                    toast::error(ui, &e);
                }
            }
            true
        }
        "arena_agent_run_get" => {
            let run = m["run"].as_str().unwrap_or("").to_string();
            // A reply for a run no longer followed is dropped.
            if !st.run.with_untracked(|r| r.as_ref().is_some_and(|r| r.run == run)) {
                return true;
            }
            st.run.update(|r| {
                if let Some(r) = r {
                    r.took(m);
                }
            });
            if st.run.with_untracked(|r| r.as_ref().is_some_and(Run::again)) {
                follow(ui, send.clone(), run);
            }
            true
        }
        // What a change said: a corner bubble, not a line in the chat.
        "notice" if m["of"].as_str().is_some_and(|o| o.starts_with("schedule_") || o == "arena_task_run") => {
            let text = m["text"].as_str().unwrap_or("");
            if m["error"].as_bool() == Some(true) {
                toast::error(ui, text);
            } else {
                toast::info(ui, text);
                if m["of"] == "schedule_save" {
                    st.form.close();
                    st.name.set(String::new());
                    st.prompt.set(String::new());
                }
            }
            true
        }
        // Another session, or the window come again: the schedules may have run meanwhile.
        "state" => {
            send(json!({"type": "automation"}));
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_schedule_says_its_mode_next_and_last_run() {
        let mut s = Sched::of(&json!({"name": "nightly", "timing": "every 1d at 09:00", "mode": "plan", "on": true,
            "next_words": "2026-11-29 09:00", "last_words": "2026-11-28 09:00", "last_status": "ok"}));
        assert_eq!(s.mode_line(), "mode plan \u{b7} next 2026-11-29 09:00");
        assert_eq!(s.last_line(), "Last 2026-11-28 09:00: ok");
        s.on = false;
        assert_eq!(s.mode_line(), "mode plan \u{b7} off");
        s.running = true;
        assert_eq!(s.last_line(), "Running now\u{2026}");
        assert_eq!(Sched::of(&json!({"name": "x"})).last_line(), "Not run yet");
    }

    #[test]
    fn a_new_schedule_needs_its_fields_and_a_time_only_for_a_day() {
        let m = save_message(" n ", "1d", "09:00", "plan", "/w", " go ").expect("ok");
        assert_eq!(
            m,
            json!({"type": "schedule_save", "name": "n", "every": "1d", "mode": "plan", "folder": "/w", "prompt": "go", "at": "09:00"})
        );
        assert!(save_message("n", "2h", "09:00", "plan", "/w", "go").expect("hours")["at"].is_null());
        assert!(save_message("", "2h", "", "plan", "/w", "go").is_err());
        assert!(save_message("n", "2h", "", "plan", "/w", " ").is_err());
        assert!(save_message("n", "2h", "", "plan", "", "go").is_err());
        assert!(save_message("n", "1d", "9:5", "plan", "/w", "go").is_err());
        assert!(save_message("n", "7d", "", "plan", "/w", "go").expect("no time")["at"].is_null());
    }

    #[test]
    fn agents_and_tasks_say_whose_and_when() {
        let a = Agent::of(
            &json!({"id": "a", "name": "Reviewer", "description": "Reviews an MR.", "enabled": false, "mine": false, "owner": "Sara"}),
        );
        assert_eq!(a.line(), "Reviews an MR. \u{b7} Shared by Sara \u{b7} Off");
        let k = Task::of(&json!({"id": "t", "name": "Morning", "when": "0 9 * * * Asia/Tehran", "enabled": true,
            "next_words": "2026-11-28 09:00", "last_status": "done", "last_words": "2026-11-27 09:00"}));
        assert_eq!(
            k.line(),
            "0 9 * * * Asia/Tehran \u{b7} last run done 2026-11-27 09:00 \u{b7} next 2026-11-28 09:00"
        );
    }

    #[test]
    fn a_run_is_followed_until_it_ends_or_waits() {
        let mut r = Run { agent: "Reviewer".into(), run: "r1".into(), ..Run::default() };
        assert!(r.again());
        assert_eq!(r.head(), "Reviewer: starting\u{2026}");
        r.took(&json!({"run": "r1", "status": "running", "ended": false, "steps": [
            {"name": "Read", "status": "done", "output": "Looks right."}, {"kind": "llm", "status": "failed", "error": "No key"}]}));
        assert_eq!(r.head(), "Reviewer: running\u{2026}");
        assert_eq!(
            r.steps[1],
            Step { name: "llm".into(), status: "failed".into(), said: "No key".into(), failed: true }
        );
        assert_eq!(step_icon(&r.steps[0].status), "pass");
        r.took(&json!({"run": "r1", "status": "done", "ended": true, "told": "Reviewer is done: Looks right.", "steps": []}));
        assert!(!r.again());
        assert_eq!(r.head(), "Reviewer is done: Looks right.");
        let mut gone = Run { agent: "A".into(), ..Run::default() };
        gone.took(&json!({"run": "r1", "error": "Not found"}));
        assert!(!gone.again() && gone.head() == "A: Not found");
        let long = Run { agent: "A".into(), asked: 1000, ..Run::default() };
        assert!(!long.again() && long.head().contains("goes on in Arena"));
    }
}
