//! The goal's bar over the input, as the old Code Arena's: what the session is working towards and how it stands
//! ("Goal · Working: …"), what it spent of its budget ("turn 2 · 40 of 200 steps · …") with a bar of the same (in
//! the warning colour from 90%), how it is checked, how it ended; Pause or Resume and Cancel while it is worked on,
//! Put away once it ended. Under it the goals waiting, each with its drop, and Start the next once the one before
//! ended. Its commands ("Work Toward a Goal…", "Add a Goal After This One…", pause, resume, cancel) and its form
//! (`goal_form.rs`) are here too; the goal itself is the driver's (`tasks.goal`, `tasks.next`).

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, dyn_container, dyn_stack, empty, h_stack, label, tooltip, v_stack};
use serde_json::{Value, json};

use crate::commands::{self, command};
use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::{bold, mix};

/// "Add a Goal After This One…": the form, for a goal that waits its turn.
pub const NEXT: &str = "codeArena.chat.goalNext";
pub const PAUSE: &str = "codeArena.chat.goalPause";
pub const RESUME: &str = "codeArena.chat.goalResume";
pub const CANCEL: &str = "codeArena.chat.goalCancel";

/// A goal as the bar shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Goal {
    pub text: String,
    pub status: String,
    pub turns: u64,
    pub steps: u64,
    pub tokens: u64,
    pub ms: u64,
    /// Its budget: steps, minutes, tokens, and whether it has none (until it is met).
    pub budget: (u64, u64, u64, bool),
    /// How it is checked: kind ("command", "condition"), the words, and by whom ("person", "agent").
    pub check: Option<(String, String, String)>,
    pub passed: Option<bool>,
    pub outcome: Option<String>,
}

impl Goal {
    /// The goal as `tasks` carries it.
    pub fn of(v: &Value) -> Option<Goal> {
        let text = v["text"].as_str()?.to_string();
        let n = |k: &str| v[k].as_u64().unwrap_or(0);
        let b = &v["budget"];
        Some(Goal {
            text,
            status: v["status"].as_str().unwrap_or("").to_string(),
            turns: n("turns"),
            steps: n("steps"),
            tokens: n("tokens"),
            ms: n("ms"),
            budget: (
                b["steps"].as_u64().unwrap_or(0),
                b["minutes"].as_u64().unwrap_or(0),
                b["tokens"].as_u64().unwrap_or(0),
                b["until_met"].as_bool().unwrap_or(false),
            ),
            check: v["check"]["text"].as_str().map(|t| {
                let s = |k: &str| v["check"][k].as_str().unwrap_or("").to_string();
                (s("kind"), t.to_string(), s("by"))
            }),
            passed: v["passed"].as_bool(),
            outcome: v["outcome"].as_str().map(str::to_string),
        })
    }

    /// Worked on or paused: not ended.
    pub fn active(&self) -> bool {
        self.status == "working" || self.status == "paused"
    }

    /// The share of its budget the most spent of steps, time and tokens has used (0 to 100).
    pub fn most(&self) -> f64 {
        let (steps, minutes, tokens, until) = self.budget;
        if until {
            return 0.0;
        }
        let part = |p: u64, w: u64| if w == 0 { 0.0 } else { (100.0 * p as f64 / w as f64).min(100.0) };
        part(self.steps, steps).max(part(self.ms, minutes * 60_000)).max(part(self.tokens, tokens))
    }

    /// "turn 2 · 40 of 200 steps · 3 min 5 s of 60 min · 120k of 5M tokens".
    pub fn usage(&self) -> String {
        let (steps, minutes, tokens, until) = self.budget;
        let turn = format!("turn {}", self.turns.max(1));
        if until {
            return [
                turn,
                format!("{} steps", grouped(self.steps)),
                time(self.ms),
                format!("{} tokens", short(self.tokens)),
                "until it is met".into(),
            ]
            .join(" · ");
        }
        [
            turn,
            format!("{} of {} steps", grouped(self.steps), grouped(steps)),
            format!("{} of {minutes} min", time(self.ms)),
            format!("{} of {} tokens", short(self.tokens), short(tokens)),
        ]
        .join(" · ")
    }

    /// "Check: cargo test (written down by the agent) · not passing yet".
    pub fn check_words(&self) -> String {
        match &self.check {
            None => "The agent writes down how it will check it first.".into(),
            Some((_, text, by)) => format!(
                "Check: {text}{}{}",
                if by == "agent" { " (written down by the agent)" } else { "" },
                if self.passed == Some(false) { " · not passing yet" } else { "" }
            ),
        }
    }

    /// What tells this goal, as it stands, from another (a goal put away stays away until it changes).
    fn key(&self) -> String {
        format!("{}|{}|{}", self.text, self.status, self.turns)
    }
}

/// The words for a goal's status.
pub fn status_words(status: &str) -> &'static str {
    match status {
        "working" => "Working",
        "paused" => "Paused",
        "met" => "Met",
        "budget" => "Budget spent",
        "stuck" => "Stopped: no progress",
        "cancelled" => "Cancelled",
        "queued" => "Waiting",
        _ => "Set",
    }
}

/// 12345 as "12,345".
pub fn grouped(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Tokens as a few figures: 950, 12.5k, 5M.
pub fn short(n: u64) -> String {
    let trim = |x: f64, unit: &str| {
        let s = format!("{x:.1}");
        format!("{}{unit}", s.strip_suffix(".0").unwrap_or(&s))
    };
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        trim(n as f64 / 1_000.0, "k")
    } else {
        trim(n as f64 / 1_000_000.0, "M")
    }
}

/// A time spent: "0 s", "45 s", "3 min 5 s", "2 h 10 min".
pub fn time(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min {} s", s / 60, s % 60),
        _ => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

/// The goal as the driver last said it, the goals waiting, and the goal put away (its key).
#[derive(Clone, Copy)]
pub struct GoalState {
    pub goal: RwSignal<Option<Goal>>,
    pub next: RwSignal<Vec<String>>,
    away: RwSignal<Option<String>>,
}

impl crate::ext::State for GoalState {
    fn new(ui: Ui) -> Self {
        GoalState {
            goal: ui.scope.create_rw_signal(None),
            next: ui.scope.create_rw_signal(Vec::new()),
            away: ui.scope.create_rw_signal(None),
        }
    }
}

/// Whether a goal is worked on or paused now.
pub fn active(ui: Ui) -> bool {
    ui.state::<GoalState>().goal.with_untracked(|g| g.as_ref().is_some_and(Goal::active))
}

/// A small button of the bar: its codicon, with its words when the pointer rests on it.
fn act(ui: Ui, name: &'static str, tip: &'static str, on: impl Fn() + 'static) -> impl IntoView {
    tooltip(crate::widgets::action(ui.colors, name, on), move || label(move || tip))
}

pub(super) fn view(ui: Ui, send: Send) -> impl IntoView {
    let (t, st) = (ui.colors, ui.state::<GoalState>());
    let goal = move || st.goal.get().unwrap_or_default();
    let shown = move || st.goal.with(|g| g.as_ref().is_some_and(|g| st.away.with(|a| a.as_deref() != Some(&g.key()))));
    let status = create_memo(move |_| st.goal.with(|g| g.as_ref().map(|g| g.status.clone()).unwrap_or_default()));
    let mark = dyn_container(
        move || status.get(),
        move |s| {
            let c = t.get_untracked();
            let (name, colour) = match s.as_str() {
                "met" => ("pass-filled", c.charts_green),
                "working" | "paused" => ("target", c.focus),
                _ => ("circle-slash", c.desc),
            };
            icon(name, 16.0).style(move |s| s.color(colour)).into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32).margin_top(1.0));
    let words = h_stack((
        label(move || format!("Goal · {}:", status_words(&status.get())))
            .style(|s| bold(s).font_size(12.0).flex_shrink(0.0_f32)),
        label(move || st.goal.with(|g| g.as_ref().map(|g| g.text.clone()).unwrap_or_default()))
            .style(|s| s.font_size(12.0).line_height(1.4).min_width(0.0).text_overflow(TextOverflow::Wrap)),
    ))
    .style(|s| s.gap(4.0).flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).items_start());
    let say = {
        let send = send.clone();
        move |kind: &'static str| {
            let send = send.clone();
            move || send(json!({"type": kind}))
        }
    };
    let pause = act(ui, "debug-pause", "Pause after this turn", say("goal_pause"))
        .style(move |s| s.apply_if(status.get() != "working", |s| s.hide()));
    let resume = act(ui, "play", "Carry on toward it", say("goal_resume"))
        .style(move |s| s.apply_if(status.get() != "paused", |s| s.hide()));
    let cancel = {
        let send = send.clone();
        act(ui, "close", "Give it up (the turn working on it stops)", move || cancel_goal(ui, &send))
            .style(move |s| s.apply_if(!matches!(status.get().as_str(), "working" | "paused"), |s| s.hide()))
    };
    let put_away = act(ui, "close", "Put it away", move || {
        let key = st.goal.with_untracked(|g| g.as_ref().map(Goal::key));
        st.away.set(key);
    })
    .style(move |s| s.apply_if(matches!(status.get().as_str(), "working" | "paused"), |s| s.hide()));
    let top = h_stack((mark, words, pause, resume, cancel, put_away)).style(|s| s.gap(6.0).items_start().width_full());
    let usage = label(move || goal().usage())
        .style(move |s| s.font_size(11.0).line_height(1.4).color(t.get().desc).text_overflow(TextOverflow::Wrap));
    let fill = empty().style(move |s| {
        let most = st.goal.with(|g| g.as_ref().map_or(0.0, Goal::most));
        let c = t.get();
        s.height_full().width_pct(most).border_radius(2.0).background(if most >= 90.0 { c.warning } else { c.progress })
    });
    let track = h_stack((fill,)).style(move |s| {
        let c = t.get();
        let off = st.goal.with(|g| g.as_ref().is_none_or(|g| !g.active() || g.budget.3));
        s.height(4.0).width_full().border_radius(2.0).background(mix(c.widget, c.fg, 0.15)).apply_if(off, |s| s.hide())
    });
    let check = label(move || goal().check_words())
        .style(move |s| s.font_size(11.0).line_height(1.4).color(t.get().desc).text_overflow(TextOverflow::Wrap));
    let outcome = label(move || goal().outcome.unwrap_or_default()).style(move |s| {
        s.font_size(11.0)
            .line_height(1.4)
            .text_overflow(TextOverflow::Wrap)
            .apply_if(st.goal.with(|g| g.as_ref().is_none_or(|g| g.outcome.is_none())), |s| s.hide())
    });
    v_stack((top, usage, track, check, outcome, waiting(ui, send.clone()))).style(move |s| {
        let c = t.get();
        let border = match status.get().as_str() {
            "working" => mix(c.focus, c.widget, 0.5),
            "met" => mix(c.charts_green, c.widget, 0.6),
            _ => c.widget_border,
        };
        super::bar(t, s).gap(5.0).border_color(border).apply_if(!shown(), |s| s.hide())
    })
}

/// The goals waiting after this one: each with its drop, and Start the next once the one before ended.
fn waiting(ui: Ui, send: Send) -> impl IntoView {
    let (t, st) = (ui.colors, ui.state::<GoalState>());
    let count = create_memo(move |_| st.next.with(Vec::len));
    let ended = create_memo(move |_| st.goal.with(|g| g.as_ref().is_none_or(|g| !g.active())));
    let start = {
        let send = send.clone();
        crate::widgets::form::button(
            ui,
            || "Start the next".into(),
            false,
            || true,
            move || send(json!({"type": "goal_next"})),
        )
        .style(move |s| s.apply_if(!ended.get(), |s| s.hide()))
    };
    let head = h_stack((
        label(move || format!("Next, each when the one before is met ({})", count.get()))
            .style(|s| bold(s).font_size(11.0).flex_grow(1.0_f32)),
        start,
    ))
    .style(|s| s.items_center().width_full());
    let rows = dyn_stack(
        move || st.next.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, text)| format!("{i}|{text}"),
        move |(i, text)| {
            let send = send.clone();
            h_stack((
                label(move || format!("{}.", i + 1)).style(move |s| s.color(t.get().desc)),
                label(move || text.clone()).style(|s| {
                    s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).text_overflow(TextOverflow::Ellipsis)
                }),
                act(ui, "trash", "Drop it", move || send(json!({"type": "goal_drop", "place": i + 1}))),
            ))
            .style(|s| s.gap(6.0).items_center().width_full().font_size(11.0))
        },
    )
    .style(|s| s.flex_col().gap(1.0).width_full());
    v_stack((head, rows)).style(move |s| {
        let c = t.get();
        s.gap(3.0)
            .width_full()
            .padding_top(5.0)
            .border_top(1.0)
            .border_color(c.widget_border)
            .cursor(CursorStyle::Default)
            .apply_if(count.get() == 0, |s| s.hide())
    })
}

/// The goal given up; the turn working on it stops.
fn cancel_goal(ui: Ui, send: &Send) {
    send(json!({"type": "goal_cancel"}));
    if ui.running.get_untracked() {
        commands::run(ui, send, super::composer::STOP);
    }
}

/// What it adds to the window's registries, once at start: its bar over the input, its form over the window, and
/// its commands.
pub fn register(_ui: Ui, _send: &Send) {
    super::slots::add(super::slots::Slot::Bars, "goal", 20, |ui, send| view(ui, send).into_any());
    super::goal_form::register();
    commands::add(command(super::header::GOAL, "Chat", "Work Toward a Goal…", |ui, _| {
        super::goal_form::open(ui, "", active(ui))
    }));
    commands::add(
        command(NEXT, "Chat", "Add a Goal After This One…", |ui, _| super::goal_form::open(ui, "", true)).when(active),
    );
    let is = |status: &'static str| {
        move |ui: Ui| ui.state::<GoalState>().goal.with_untracked(|g| g.as_ref().is_some_and(|g| g.status == status))
    };
    commands::add(
        command(PAUSE, "Chat", "Pause the Goal", |_, send| send(json!({"type": "goal_pause"}))).when(is("working")),
    );
    commands::add(
        command(RESUME, "Chat", "Resume the Goal", |_, send| send(json!({"type": "goal_resume"}))).when(is("paused")),
    );
    commands::add(command(CANCEL, "Chat", "Cancel the Goal", cancel_goal).when(active));
}

/// The session's message of its tasks, the goal's part taken in; a goal message's error is said. Never taken away
/// (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let st = ui.state::<GoalState>();
    match m["type"].as_str().unwrap_or("") {
        "tasks" => {
            let goal = Goal::of(&m["goal"]);
            let next: Vec<String> = m["next"]
                .as_array()
                .map(|n| n.iter().filter_map(|g| g["text"].as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            // The header's target is lit while a goal is worked on or paused.
            let lit = goal.as_ref().filter(|g| g.active()).map(|g| g.text.clone());
            if ui.goal.with_untracked(|g| *g != lit) {
                ui.goal.set(lit);
            }
            if st.goal.with_untracked(|g| *g != goal) {
                st.goal.set(goal);
            }
            if st.next.with_untracked(|n| *n != next) {
                st.next.set(next);
            }
        }
        "goal_set" | "goal_pause" | "goal_resume" | "goal_cancel" | "goal_next" | "goal_drop" => {
            if let Some(e) = m["error"].as_str() {
                crate::widgets::toast::error(ui, e);
            } else if m["type"] == "goal_set" {
                super::goal_form::set(ui);
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal() -> Goal {
        Goal::of(&json!({"text": "fix it", "status": "working", "turns": 2, "steps": 40, "tokens": 120_000,
                         "ms": 185_000, "budget": {"steps": 200, "minutes": 60, "tokens": 5_000_000, "until_met": false},
                         "check": {"kind": "command", "text": "cargo test", "by": "agent"}, "passed": false}))
        .expect("as written")
    }

    #[test]
    fn the_bar_says_what_the_goal_spent_of_its_budget() {
        let g = goal();
        assert_eq!(g.usage(), "turn 2 · 40 of 200 steps · 3 min 5 s of 60 min · 120k of 5M tokens");
        assert_eq!(g.most(), 20.0);
        let mut u = g.clone();
        u.budget.3 = true;
        assert_eq!(u.usage(), "turn 2 · 40 steps · 3 min 5 s · 120k tokens · until it is met");
        assert_eq!(u.most(), 0.0);
        let mut spent = g.clone();
        spent.steps = 190;
        assert!(spent.most() >= 90.0, "the bar turns to the warning colour");
        let mut first = g;
        first.turns = 0;
        assert!(first.usage().starts_with("turn 1 · "));
    }

    #[test]
    fn the_bar_says_how_the_goal_is_checked_and_how_it_stands() {
        let mut g = goal();
        assert_eq!(g.check_words(), "Check: cargo test (written down by the agent) · not passing yet");
        g.check = None;
        assert_eq!(g.check_words(), "The agent writes down how it will check it first.");
        assert_eq!(status_words("stuck"), "Stopped: no progress");
        assert_eq!(status_words("budget"), "Budget spent");
        assert!(g.active());
        g.status = "met".into();
        assert!(!g.active());
        assert!(Goal::of(&Value::Null).is_none());
    }

    #[test]
    fn numbers_are_said_short() {
        assert_eq!((grouped(5), grouped(1234), grouped(5_000_000)), ("5".into(), "1,234".into(), "5,000,000".into()));
        assert_eq!((short(950), short(12_500), short(5_000_000)), ("950".into(), "12.5k".into(), "5M".into()));
        assert_eq!((time(0), time(45_000), time(7_800_000)), ("0 s".into(), "45 s".into(), "2 h 10 min".into()));
    }
}
