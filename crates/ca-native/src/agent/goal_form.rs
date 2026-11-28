//! The goal's form, as the old Code Arena's: "Work toward a goal" (or "Add a goal after this one" while one is worked
//! on): what the goal is (at most 4,000 characters), how it is checked (a command succeeds, a condition holds, or
//! the agent writes it down first), and its budget (until it is met, or steps 1 to 100,000, minutes 1 to 43,200 and
//! tokens 10,000 or more; 200, 60 and 5,000,000 at first). Start (Add) sends `goal_set`; the bar (`goal_bar.rs`)
//! says the rest.

use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::views::{Decorators, container, h_stack, label, v_stack};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::widgets::form::{self, Form};

/// The longest goal.
const MOST_TEXT: usize = 4_000;

/// How the goal is checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Command,
    Condition,
    Agent,
}

/// What keeps the form from being sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrong {
    /// Something is not filled in yet (Start is given up, nothing is said).
    Unready,
    /// "The budget is out of bounds."
    Bounds,
}

/// The form's fields, as typed.
#[derive(Clone, Debug, PartialEq)]
pub struct Fields {
    pub text: String,
    pub choice: Choice,
    pub command: String,
    pub condition: String,
    pub until_met: bool,
    pub steps: String,
    pub minutes: String,
    pub tokens: String,
}

impl Default for Fields {
    fn default() -> Self {
        Fields {
            text: String::new(),
            choice: Choice::Command,
            command: String::new(),
            condition: String::new(),
            until_met: false,
            steps: "200".into(),
            minutes: "60".into(),
            tokens: "5,000,000".into(),
        }
    }
}

/// A number as typed, its group marks ("5,000,000", "5_000_000", "5 000 000") left out.
fn number(s: &str) -> Option<u64> {
    let digits: String = s.chars().filter(|c| !matches!(c, ',' | '_' | ' ')).collect();
    digits.parse().ok()
}

impl Fields {
    /// Whether the budget is one a goal may have.
    pub fn in_bounds(&self) -> bool {
        self.until_met || self.budget().is_some()
    }

    /// Steps, minutes and tokens, when each is a number within its bounds.
    fn budget(&self) -> Option<(u64, u64, u64)> {
        let (s, m, t) = (number(&self.steps)?, number(&self.minutes)?, number(&self.tokens)?);
        ((1..=100_000).contains(&s) && (1..=43_200).contains(&m) && t >= 10_000).then_some((s, m, t))
    }

    /// The `goal_set` it sends, or what keeps it from being sent.
    pub fn message(&self, after: bool) -> Result<Value, Wrong> {
        if !self.in_bounds() {
            return Err(Wrong::Bounds);
        }
        let text = self.text.trim();
        if text.is_empty() || text.chars().count() > MOST_TEXT {
            return Err(Wrong::Unready);
        }
        let check = match self.choice {
            Choice::Agent => Value::Null,
            Choice::Command | Choice::Condition => {
                let (kind, words) = if self.choice == Choice::Command {
                    ("command", self.command.trim())
                } else {
                    ("condition", self.condition.trim())
                };
                if words.is_empty() {
                    return Err(Wrong::Unready);
                }
                json!({"kind": kind, "text": words})
            }
        };
        let (steps, minutes, tokens) = self.budget().unwrap_or((200, 60, 5_000_000));
        Ok(json!({"type": "goal_set", "text": text, "check": check, "after": after,
                  "budget": {"steps": steps, "minutes": minutes, "tokens": tokens, "until_met": self.until_met}}))
    }
}

/// The form's state: its fields' signals, and whether the goal waits its turn.
#[derive(Clone, Copy)]
struct GoalForm {
    form: Form,
    text: RwSignal<String>,
    choice: RwSignal<Choice>,
    command: RwSignal<String>,
    condition: RwSignal<String>,
    until_met: RwSignal<bool>,
    steps: RwSignal<String>,
    minutes: RwSignal<String>,
    tokens: RwSignal<String>,
    queued: RwSignal<bool>,
}

impl crate::ext::State for GoalForm {
    fn new(ui: Ui) -> Self {
        let f = Fields::default();
        let s = |v: String| ui.scope.create_rw_signal(v);
        GoalForm {
            form: Form::new(ui),
            text: s(f.text),
            choice: ui.scope.create_rw_signal(f.choice),
            command: s(f.command),
            condition: s(f.condition),
            until_met: ui.scope.create_rw_signal(f.until_met),
            steps: s(f.steps),
            minutes: s(f.minutes),
            tokens: s(f.tokens),
            queued: ui.scope.create_rw_signal(false),
        }
    }
}

impl GoalForm {
    /// The fields as typed (tracked: what reads it follows them).
    fn fields(self) -> Fields {
        Fields {
            text: self.text.get(),
            choice: self.choice.get(),
            command: self.command.get(),
            condition: self.condition.get(),
            until_met: self.until_met.get(),
            steps: self.steps.get(),
            minutes: self.minutes.get(),
            tokens: self.tokens.get(),
        }
    }
}

/// The form opened, with `text` as the goal when it is not empty; `queued`: a goal is worked on, so this one waits.
pub fn open(ui: Ui, text: &str, queued: bool) {
    let st = ui.state::<GoalForm>();
    if !text.is_empty() {
        st.text.set(text.to_string());
    }
    if st.queued.get_untracked() != queued {
        st.queued.set(queued);
    }
    st.form.show(None);
}

/// The goal was set: the form starts empty next time.
pub fn set(ui: Ui) {
    let st = ui.state::<GoalForm>();
    st.text.set(String::new());
    st.command.set(String::new());
    st.condition.set(String::new());
}

/// The form sent, when it is ready; it closes.
fn start(ui: Ui, send: &Send) {
    let st = ui.state::<GoalForm>();
    let fields = st.fields();
    if let Ok(m) = fields.message(st.queued.get_untracked()) {
        send(m);
        st.form.close();
        crate::chat_bridge::focus(ui);
    }
}

fn host(ui: Ui, send: Send) -> AnyView {
    let (t, st) = (ui.colors, ui.state::<GoalForm>());
    let f = st.form;
    let go = {
        let send = send.clone();
        move || start(ui, &send)
    };
    let enter = move || go.clone();
    let goal = f.named(
        ui,
        "The goal",
        f.input(ui, st.text, "The parser accepts trailing commas, with tests for them.", enter()),
    );
    let command = container(f.input(ui, st.command, "cargo test", enter()))
        .style(move |s| s.width_full().apply_if(st.choice.get() != Choice::Command, |s| s.hide()))
        .into_any();
    let condition = container(f.input(ui, st.condition, "Every public method of Parser has a test.", enter()))
        .style(move |s| s.width_full().apply_if(st.choice.get() != Choice::Condition, |s| s.hide()))
        .into_any();
    let check = v_stack((
        label(|| "How it is checked").style(|s| s.font_size(12.0).font_bold()),
        f.radio(
            ui,
            st.choice,
            Choice::Command,
            "A command succeeds",
            "Run after each turn; the goal is met on exit code 0 (the tests, a build).",
        ),
        f.radio(
            ui,
            st.choice,
            Choice::Condition,
            "A condition holds",
            "A fresh reader looks at the code after each turn and says whether it holds.",
        ),
        f.radio(
            ui,
            st.choice,
            Choice::Agent,
            "The agent writes it down first",
            "It says how it will check before it changes anything (a command asks as commands do).",
        ),
        command,
        condition,
    ))
    .style(|s| s.gap(8.0).width_full())
    .into_any();
    let numbers = h_stack((
        f.named(ui, "Steps (1 to 100,000)", f.input(ui, st.steps, "200", enter())),
        f.named(ui, "Minutes (1 to 43,200)", f.input(ui, st.minutes, "60", enter())),
        f.named(ui, "Tokens (from 10,000)", f.input(ui, st.tokens, "5,000,000", enter())),
    ))
    .style(move |s| s.gap(8.0).width_full().apply_if(st.until_met.get(), |s| s.hide()));
    let out = label(|| "The budget is out of bounds.")
        .style(move |s| s.font_size(12.0).color(t.get().error).apply_if(st.fields().in_bounds(), |s| s.hide()));
    let budget = v_stack((
        label(|| "Budget").style(|s| s.font_size(12.0).font_bold()),
        f.checkbox(
            ui,
            st.until_met,
            "Until it is met",
            "No limit of steps, time or tokens: a long session that stops only when the check passes, the agent \
             makes no progress, or you stop it.",
        ),
        numbers,
        out,
    ))
    .style(|s| s.gap(8.0).width_full())
    .into_any();
    let ready = move || st.fields().message(st.queued.get()).is_ok();
    let buttons = form::buttons(vec![
        form::button(ui, || "Cancel".into(), false, || true, move || f.close()),
        form::button(
            ui,
            move || if st.queued.get() { "Add".into() } else { "Start".into() },
            true,
            ready,
            go_button(ui, send),
        ),
    ]);
    f.host(
        ui,
        move || if st.queued.get() { "Add a goal after this one".into() } else { "Work toward a goal".into() },
        move || {
            if st.queued.get() {
                "It waits its turn: it starts once the goals before it are met, and the session carries on until \
                 every one is met."
                    .into()
            } else {
                "The agent keeps working, turn after turn, until the check passes or the budget is spent, then says \
                 how it checked."
                    .into()
            }
        },
        vec![goal, check, budget],
        buttons,
    )
}

fn go_button(ui: Ui, send: Send) -> impl Fn() + 'static {
    move || start(ui, &send)
}

/// Its layer over the window, once at start (from the goal bar's `register`).
pub fn register() {
    layers::add(Place::Over, "goal.form", 30, |ui, send| host(ui, send).into_any());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_sends_the_goal_its_check_and_its_budget() {
        let mut f = Fields { text: " fix it ".into(), command: "cargo test".into(), ..Fields::default() };
        let m = f.message(false).unwrap();
        assert_eq!(m["text"], "fix it");
        assert_eq!(m["check"], json!({"kind": "command", "text": "cargo test"}));
        assert_eq!(m["budget"], json!({"steps": 200, "minutes": 60, "tokens": 5_000_000, "until_met": false}));
        f.choice = Choice::Agent;
        assert_eq!(f.message(true).unwrap()["check"], Value::Null);
        assert_eq!(f.message(true).unwrap()["after"], true);
        f.choice = Choice::Condition;
        assert_eq!(f.message(false), Err(Wrong::Unready), "the condition is not written yet");
        f.text = " ".into();
        f.choice = Choice::Agent;
        assert_eq!(f.message(false), Err(Wrong::Unready));
    }

    #[test]
    fn a_budget_out_of_bounds_is_said_and_until_met_has_none() {
        let mut f = Fields { text: "x".into(), choice: Choice::Agent, ..Fields::default() };
        for (steps, minutes, tokens) in [
            ("0", "60", "10000"),
            ("100001", "60", "10000"),
            ("1", "43201", "10000"),
            ("1", "1", "9,999"),
            ("ten", "1", "10000"),
        ] {
            (f.steps, f.minutes, f.tokens) = (steps.into(), minutes.into(), tokens.into());
            assert_eq!(f.message(false), Err(Wrong::Bounds), "{steps} {minutes} {tokens}");
        }
        f.until_met = true;
        assert!(f.message(false).is_ok(), "no budget: until it is met");
        f.until_met = false;
        (f.steps, f.minutes, f.tokens) = ("100_000".into(), "43 200".into(), "10,000".into());
        assert_eq!(f.message(false).unwrap()["budget"]["steps"], 100_000);
    }
}
