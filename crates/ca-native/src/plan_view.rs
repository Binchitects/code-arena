//! The Plan view: the agent's working plan. Its title, how many of its steps are done and its goal; then each step
//! with its mark (Done, Doing, Blocked, To do), what it waits for ("after 1, 2"), when it is done ("Done when:
//! `check` · passed", the check's output when the pointer rests on it), its files, the symbols it changes, what a
//! change to them reaches and its notes. A step begun can be undone (its files as they were before it began).
//!
//! A plan the agent proposed in plan mode waits for the person's review: what a second reading said of it, then
//! Approve, Approve, Asking Before Each Change, Another Way… and Reject… (each with why, asked for), and Change Step
//! on each step (what to change, asked for). The review is said to the agent as the person's next message.
//!
//! With no plan, the view says how one comes, and offers to carry on the plan an earlier session of this folder left
//! unfinished. The session's side is the driver's `ide_plan.rs`.

use floem::IntoView;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, tooltip, v_stack};
use serde_json::{Value, json};

use crate::colors::Colors;
use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, Theme, action, bold, button, dialog, quick, toast};

/// Whether the view is in the window.
pub const READY: bool = true;

/// What a click says while an answer is written.
const BUSY: &str = "Wait for the answer, or stop it, to change the plan.";
/// The view, while the session has no plan.
const NONE_YET: &str = "There is no plan yet. In plan mode the agent proposes one for your review; in the other modes \
                        it keeps one for work of several steps.";

/// A step as the view shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub number: u64,
    pub title: String,
    pub status: String,
    pub check: Option<String>,
    /// "passed" or "failed", since the step began.
    pub check_result: Option<String>,
    pub check_output: Option<String>,
    pub files: Vec<String>,
    pub symbols: Vec<String>,
    pub depends_on: Vec<u64>,
    pub notes: Option<String>,
    pub impact: Option<String>,
}

/// The plan as the view shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub title: String,
    pub goal: Option<String>,
    pub steps: Vec<Step>,
    /// "proposed", "approved", or none (made outside plan mode).
    pub review: Option<String>,
    pub critique: Option<String>,
}

impl Plan {
    pub fn done(&self) -> usize {
        self.steps.iter().filter(|s| s.status == "done").count()
    }

    pub fn proposed(&self) -> bool {
        self.review.as_deref() == Some("proposed")
    }
}

/// The plan an earlier session of this folder left unfinished.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Left {
    pub title: String,
    pub done: u64,
    pub total: u64,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(text).collect()
}

/// The plan in a `tasks` message (none: the session has none).
pub fn plan_of(m: &Value) -> Option<Plan> {
    let p = m["plan"].as_object()?;
    let steps: Vec<Step> = p
        .get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|s| Step {
            number: s["number"].as_u64().unwrap_or(0),
            title: s["title"].as_str().unwrap_or("").to_string(),
            status: s["status"].as_str().unwrap_or("todo").to_string(),
            check: text(&s["check"]),
            check_result: text(&s["checkResult"]),
            check_output: text(&s["checkOutput"]),
            files: strings(&s["files"]),
            symbols: strings(&s["symbols"]),
            depends_on: s["dependsOn"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect(),
            notes: text(&s["notes"]),
            impact: text(&s["impact"]),
        })
        .collect();
    let at = |k: &str| p.get(k).and_then(text);
    Some(Plan {
        title: p.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
        goal: at("goal"),
        steps,
        review: at("review"),
        critique: p.get("critique").and_then(Value::as_str).map(|c| c.trim().to_string()),
    })
}

/// The plan left in a `tasks` message.
pub fn left_of(m: &Value) -> Option<Left> {
    let l = m["plan_left"].as_object()?;
    Some(Left {
        title: l.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
        done: l.get("done").and_then(Value::as_u64).unwrap_or(0),
        total: l.get("total").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// A step's status in words.
pub fn status_word(status: &str) -> &'static str {
    match status {
        "done" => "Done",
        "doing" => "Doing",
        "blocked" => "Blocked",
        _ => "To do",
    }
}

/// A step's mark: its codicon and colour.
pub fn mark(status: &str, c: &Colors) -> (&'static str, Color) {
    match status {
        "done" => ("pass-filled", c.ok),
        "doing" => ("circle-large-filled", c.progress),
        "blocked" => ("error", c.error),
        _ => ("circle-large", c.desc),
    }
}

/// When the step is done: "Done when: `check` · passed".
pub fn check_line(s: &Step) -> Option<String> {
    let check = s.check.as_ref()?;
    Some(match &s.check_result {
        Some(r) => format!("Done when: `{check}` · {r}"),
        None => format!("Done when: `{check}`"),
    })
}

/// What the step waits for: "after 1, 2".
pub fn after_line(s: &Step) -> Option<String> {
    (!s.depends_on.is_empty())
        .then(|| format!("after {}", s.depends_on.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")))
}

/// "N/M done".
pub fn progress(p: &Plan) -> String {
    format!("{}/{} done", p.done(), p.steps.len())
}

/// What the list knows a step by: another key draws it again.
fn key(s: &Step) -> String {
    format!("{s:?}")
}

/// The view's state in a window.
#[derive(Clone, Copy)]
pub struct PlanState {
    pub plan: RwSignal<Option<Plan>>,
    pub left: RwSignal<Option<Left>>,
}

impl crate::ext::State for PlanState {
    fn new(ui: Ui) -> Self {
        PlanState { plan: ui.scope.create_rw_signal(None), left: ui.scope.create_rw_signal(None) }
    }
}

/// The view's state: the plan as the session last said it (the bar over the input reads it too).
pub fn state(ui: Ui) -> PlanState {
    ui.state::<PlanState>()
}

/// Sends a request that changes the plan, unless an answer is being written (then says why).
fn between_turns(ui: Ui, send: &Send, m: Value) {
    if ui.running.get_untracked() {
        toast::info(ui, BUSY);
    } else {
        send(m);
    }
}

/// The person's review of the plan proposed.
fn review(send: &Send, action: &str, step: Option<u64>, text: &str) {
    let mut m = json!({"type": "plan_review", "action": action});
    if let Some(n) = step {
        m["step"] = json!(n);
    }
    if !text.trim().is_empty() {
        m["text"] = json!(text.trim());
    }
    send(m);
}

/// A review that says why (Another Way, Reject): asked for first, not required.
fn review_why(ui: Ui, send: &Send, action: &'static str, title: &str, prompt: &str) {
    let send = send.clone();
    quick::input(title).placeholder("Why (optional)").prompt(prompt).open(ui, move |why| {
        if let Some(why) = why {
            review(&send, action, None, &why);
        }
    });
}

/// Change Step N: what is to change, required.
fn change_step(ui: Ui, send: &Send, n: u64) {
    let send = send.clone();
    quick::input(&format!("Change Step {n}"))
        .placeholder("What should change in this step")
        .prompt("Tell the agent what should change")
        .validate(|v| v.trim().is_empty().then(|| "Say what should change.".to_string()))
        .open(ui, move |what| {
            if let Some(what) = what {
                review(&send, "change", Some(n), &what);
            }
        });
}

/// Undo Step N, once the person said so.
fn undo_step(ui: Ui, send: &Send, n: u64) {
    if ui.running.get_untracked() {
        toast::info(ui, BUSY);
        return;
    }
    let send = send.clone();
    dialog::ask(&format!("Undo step {n} of the plan?"))
        .detail(
            "Its files go back to how they were before it began, with every step begun after it; those steps are to \
             do again. What commands changed is not undone.",
        )
        .kind(dialog::Kind::Warning)
        .button("Undo")
        .open(ui, move |a| {
            if a.yes() {
                between_turns(ui, &send, json!({"type": "plan_undo", "step": n}));
            }
        });
}

/// The view's own content.
pub fn view(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    let has = create_memo(move |_| st.plan.with(Option::is_some));
    dyn_container(
        move || has.get(),
        move |has| if has { body(ui, send.clone()).into_any() } else { nothing(ui, send.clone()).into_any() },
    )
    .style(move |s| s.size_full().min_height(0.0).color(t.get().side_fg))
    .into_any()
}

/// Wrapped words, in the description's colour.
fn desc(t: Theme, words: String) -> impl IntoView {
    label(move || words.clone()).style(move |s| {
        s.font_size(12.0)
            .line_height(1.4)
            .color(t.get().desc)
            .text_overflow(TextOverflow::Wrap)
            .min_width(0.0)
            .max_width_full()
    })
}

/// No plan: how one comes, and the plan an earlier session left.
fn nothing(ui: Ui, send: Send) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let left = create_memo(move |_| st.left.get());
    let offer = dyn_container(
        move || left.get(),
        move |l| match l {
            None => empty().into_any(),
            Some(l) => {
                let send = send.clone();
                let words = format!(
                    "An earlier session left a plan in progress here: \u{201c}{}\u{201d}, {} of {} steps done.",
                    l.title, l.done, l.total
                );
                v_stack((
                    desc(t, words),
                    button(t, "Carry it on here", true, move || {
                        between_turns(ui, &send, json!({"type": "plan_resume"}))
                    }),
                ))
                .style(|s| s.gap(8.0).margin_top(12.0).items_start().width_full())
                .into_any()
            }
        },
    );
    v_stack((desc(t, NONE_YET.into()), offer)).style(|s| s.padding_horiz(20.0).padding_vert(10.0).width_full())
}

/// The plan: its head, the review while it waits for one, its steps.
fn body(ui: Ui, send: Send) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let title = label(move || st.plan.with(|p| p.as_ref().map(|p| p.title.clone()).unwrap_or_default()))
        .style(move |s| bold(s).font_size(13.0).color(t.get().fg).text_overflow(TextOverflow::Wrap).min_width(0.0));
    let count = label(move || st.plan.with(|p| p.as_ref().map(progress).unwrap_or_default()))
        .style(move |s| s.font_size(12.0).color(t.get().desc).flex_shrink(0.0_f32));
    let goal =
        label(move || st.plan.with(|p| p.as_ref().and_then(|p| p.goal.clone()).unwrap_or_default())).style(move |s| {
            let none = st.plan.with(|p| p.as_ref().is_none_or(|p| p.goal.is_none()));
            s.font_size(12.0).color(t.get().desc).text_overflow(TextOverflow::Wrap).apply_if(none, |s| s.hide())
        });
    let head = v_stack((
        h_stack((title.style(|s| s.flex_grow(1.0_f32)), count)).style(|s| s.gap(8.0).width_full().items_center()),
        goal,
    ))
    .style(|s| s.gap(4.0).width_full().padding_horiz(20.0).padding_vert(8.0));
    let proposed = create_memo(move |_| st.plan.with(|p| p.as_ref().is_some_and(Plan::proposed)));
    let review_send = send.clone();
    let review = dyn_container(
        move || proposed.get(),
        move |p| if p { review_card(ui, review_send.clone()).into_any() } else { empty().into_any() },
    );
    let steps = dyn_stack(
        move || st.plan.with(|p| p.as_ref().map(|p| p.steps.clone()).unwrap_or_default()),
        key,
        move |s| step_row(ui, send.clone(), s, proposed),
    )
    .style(|s| s.flex_col().width_full().padding_bottom(10.0));
    scroll(v_stack((head, review, steps)).style(|s| s.width_full()))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full().min_height(0.0))
}

/// The review of a plan proposed: what a second reading said of it, and the person's answers.
fn review_card(ui: Ui, send: Send) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let critique = label(move || {
        st.plan.with(|p| match p.as_ref().and_then(|p| p.critique.as_deref()) {
            Some("") => "A second reading found nothing to change.".to_string(),
            Some(c) => format!("A second reading: {c}"),
            None => String::new(),
        })
    })
    .style(move |s| {
        let none = st.plan.with(|p| p.as_ref().is_none_or(|p| p.critique.is_none()));
        s.font_size(12.0)
            .line_height(1.4)
            .color(t.get().desc)
            .text_overflow(TextOverflow::Wrap)
            .min_width(0.0)
            .max_width_full()
            .apply_if(none, |s| s.hide())
    });
    let (a, b, c, d) = (send.clone(), send.clone(), send.clone(), send);
    v_stack((
        label(|| "This plan waits for your review.")
            .style(move |s| bold(s).font_size(12.0).line_height(1.4).color(t.get().fg)),
        critique,
        h_stack((
            button(t, "Approve", true, move || review(&a, "approve", None, "")),
            wide(t, "Approve, asking before each change", false, move || review(&b, "approve-ask", None, "")),
            button(t, "Another Way\u{2026}", false, move || {
                review_why(ui, &c, "another", "Another Way", "Why another way?")
            }),
            button(t, "Reject\u{2026}", false, move || {
                review_why(ui, &d, "reject", "Reject the Plan", "Why reject it?")
            }),
        ))
        .style(|s| s.gap(6.0).flex_wrap(floem::style::FlexWrap::Wrap).row_gap(6.0).min_width(0.0).max_width_full()),
        desc(t, "Or change a step: Change Step on the step.".into()),
    ))
    .style(move |s| {
        let c = t.get();
        s.gap(8.0)
            .min_width(0.0)
            .margin_horiz(12.0)
            .margin_bottom(8.0)
            .padding(10.0)
            .border(1.0)
            .border_radius(4.0)
            .border_color(c.widget_border)
            .background(c.widget)
    })
}

/// A button whose words wrap when the side bar is narrow: the primary one or the secondary.
fn wide(t: Theme, text: &'static str, primary: bool, on: impl Fn() + 'static) -> impl IntoView {
    label(move || text).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        let (bg, fg, hover) = if primary {
            (c.button_bg, c.button_fg, c.button_hover)
        } else {
            (c.button2_bg, c.button2_fg, c.button2_hover)
        };
        s.padding_horiz(14.0)
            .padding_vert(4.0)
            .border_radius(4.0)
            .font_size(13.0)
            .line_height(1.4)
            .max_width_full()
            .text_overflow(TextOverflow::Wrap)
            .background(bg)
            .color(fg)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(hover))
    })
}

/// A small chip: a file of the step.
fn chip(t: Theme, words: String) -> impl IntoView {
    label(move || words.clone()).style(move |s| {
        let c = t.get();
        s.font_family(MONO_FONT.clone())
            .font_size(11.0)
            .padding_horiz(5.0)
            .padding_vert(1.0)
            .border_radius(3.0)
            .background(c.preformat_bg)
            .color(c.fg)
    })
}

/// A link under a step: Change Step N, Undo Step N.
fn link(t: Theme, words: String, on: impl Fn() + 'static) -> impl IntoView {
    label(move || words.clone()).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        s.font_size(12.0).color(c.link).cursor(CursorStyle::Pointer).hover(|s| s.color(c.link_active))
    })
}

fn step_row(ui: Ui, send: Send, s: Step, proposed: floem::reactive::Memo<bool>) -> impl IntoView {
    let t = ui.colors;
    let (n, status) = (s.number, s.status.clone());
    let mark_status = status.clone();
    let marked = icon(mark(&status, &t.get_untracked()).0, 16.0)
        .style(move |st| st.color(mark(&mark_status, &t.get()).1).flex_shrink(0.0_f32).margin_top(1.0));
    let word = status_word(&status);
    let title = format!("{n}. {}", crate::bidi::isolate(&s.title));
    let done = status == "done";
    let head = h_stack((
        label(move || title.clone()).style(move |s| {
            s.font_size(13.0)
                .color(t.get().fg)
                .text_overflow(TextOverflow::Wrap)
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .apply_if(done, |s| s.color(t.get().desc))
        }),
        label(move || word).style(move |s| s.font_size(11.0).color(t.get().desc).flex_shrink(0.0_f32)),
    ))
    .style(|s| s.gap(8.0).width_full());
    let mut lines: Vec<floem::AnyView> = Vec::new();
    if let Some(after) = after_line(&s) {
        lines.push(desc(t, after).into_any());
    }
    if let Some(check) = check_line(&s) {
        let failed = s.check_result.as_deref() == Some("failed");
        let passed = s.check_result.as_deref() == Some("passed");
        let line = label(move || check.clone()).style(move |s| {
            let c = t.get();
            s.font_size(12.0).text_overflow(TextOverflow::Wrap).min_width(0.0).color(if failed {
                c.error
            } else if passed {
                c.ok
            } else {
                c.desc
            })
        });
        lines.push(match s.check_output.clone() {
            Some(out) => tooltip(line, move || {
                let out = out.clone();
                label(move || out.clone()).style(|s| s.font_family(MONO_FONT.clone()).font_size(11.0).max_width(480.0))
            })
            .into_any(),
            None => line.into_any(),
        });
    }
    if !s.files.is_empty() {
        let files = s.files.iter().cloned().map(|f| chip(t, f).into_any()).collect::<Vec<_>>();
        lines.push(
            floem::views::stack_from_iter(files)
                .style(|s| s.flex_row().flex_wrap(floem::style::FlexWrap::Wrap).gap(4.0).row_gap(4.0))
                .into_any(),
        );
    }
    if !s.symbols.is_empty() {
        lines.push(desc(t, format!("Changes: {}", s.symbols.join(", "))).into_any());
    }
    if let Some(impact) = s.impact.clone() {
        lines.push(desc(t, impact).into_any());
    }
    if let Some(notes) = s.notes.clone() {
        lines.push(desc(t, notes).into_any());
    }
    let (change_send, undo_send) = (send.clone(), send);
    let begun = status != "todo";
    let links = h_stack((
        container(link(t, format!("Change step {n}"), move || change_step(ui, &change_send, n)))
            .style(move |s| s.apply_if(!proposed.get(), |s| s.hide())),
        container(link(t, format!("Undo step {n}"), move || undo_step(ui, &undo_send, n)))
            .style(move |s| s.apply_if(proposed.get() || !begun, |s| s.hide())),
    ))
    .style(move |s| s.gap(12.0).apply_if(!proposed.get() && !begun, |s| s.hide()));
    lines.push(links.into_any());
    h_stack((
        marked,
        v_stack((head, floem::views::stack_from_iter(lines).style(|s| s.flex_col().gap(3.0).width_full())))
            .style(|s| s.gap(3.0).flex_grow(1.0_f32).min_width(0.0)),
    ))
    .style(move |s| {
        s.gap(8.0).width_full().padding_horiz(20.0).padding_vert(5.0).hover(|s| s.background(t.get().list_hover))
    })
}

/// The actions in its title: Clear Plan, while there is one.
pub fn actions(ui: Ui, send: Send) -> Option<floem::AnyView> {
    let (t, st) = (ui.colors, state(ui));
    let clear = action(t, "clear-all", move || between_turns(ui, &send, json!({"type": "plan_clear"})));
    let clear = tooltip(clear, || label(|| "Clear Plan"));
    Some(container(clear).style(move |s| s.apply_if(st.plan.with(Option::is_none), |s| s.hide())).into_any())
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it. `tasks` is
/// left for the bar over the input too.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let st = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "tasks" => {
            let (plan, left) = (plan_of(m), left_of(m));
            // Set only when it changed: the view and the bar draw again on every set.
            if st.plan.with_untracked(|p| *p != plan) {
                st.plan.set(plan);
            }
            if st.left.with_untracked(|l| *l != left) {
                st.left.set(left);
            }
            false
        }
        "plan_undo" | "plan_resume" | "plan_clear" | "plan_review" => {
            match (m["text"].as_str(), m["error"].as_str()) {
                (_, Some(e)) => {
                    toast::error(ui, e);
                }
                (Some(text), None) => {
                    toast::info(ui, text);
                }
                _ => {}
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tasks() -> Value {
        json!({"type": "tasks", "plan": {"title": "Greet", "goal": "Say hello", "review": "proposed", "critique": "",
            "steps": [
                {"number": 1, "title": "Write it", "status": "done", "check": "cargo test", "checkResult": "passed",
                 "checkOutput": "ok", "files": ["src/main.rs"], "symbols": ["main"]},
                {"number": 2, "title": "Print it", "status": "todo", "dependsOn": [1], "notes": "  "}]}})
    }

    #[test]
    fn a_plan_is_read_from_the_sessions_tasks() {
        let p = plan_of(&tasks()).expect("a plan");
        assert_eq!((p.title.as_str(), p.goal.as_deref(), p.proposed()), ("Greet", Some("Say hello"), true));
        assert_eq!(p.critique.as_deref(), Some(""), "an empty critique is kept: the plan held");
        assert_eq!((progress(&p), p.steps[0].files.clone()), ("1/2 done".to_string(), vec!["src/main.rs".to_string()]));
        assert_eq!((p.steps[1].depends_on.clone(), p.steps[1].notes.clone()), (vec![1], None));
        assert!(plan_of(&json!({"type": "tasks", "plan": null})).is_none());
    }

    #[test]
    fn a_step_says_when_it_is_done_and_what_it_waits_for() {
        let p = plan_of(&tasks()).expect("a plan");
        assert_eq!(check_line(&p.steps[0]).as_deref(), Some("Done when: `cargo test` · passed"));
        assert_eq!(check_line(&p.steps[1]), None);
        assert_eq!(after_line(&p.steps[1]).as_deref(), Some("after 1"));
        assert_eq!(after_line(&p.steps[0]), None);
        assert_eq!(
            ["done", "doing", "blocked", "todo", "?"].map(status_word),
            ["Done", "Doing", "Blocked", "To do", "To do"]
        );
    }

    #[test]
    fn the_plan_an_earlier_session_left_is_read() {
        let m = json!({"type": "tasks", "plan": null, "plan_left": {"title": "Old", "done": 1, "total": 3}});
        assert_eq!(left_of(&m), Some(Left { title: "Old".into(), done: 1, total: 3 }));
        assert_eq!(left_of(&tasks()), None);
    }
}
