//! The plan's bar over the input: its title, how many of its steps are done (with a bar of the same), and "waiting
//! for your review" while a plan proposed waits for one. A click opens it to the steps, each with its mark.

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, dyn_container, dyn_stack, empty, h_stack, label, v_stack};
use serde_json::Value;

use crate::icons::icon;
use crate::plan_view::{Plan, Step, mark, state as plan};
use crate::state::{Send, Ui};
use crate::widgets::bold;

/// Whether the bar is open to the steps.
#[derive(Clone, Copy)]
struct Open(RwSignal<bool>);

impl crate::ext::State for Open {
    fn new(ui: Ui) -> Self {
        Open(ui.scope.create_rw_signal(false))
    }
}

/// What the bar's line says after the title: " · 1 of 3 done · waiting for your review".
pub fn said(p: &Plan) -> String {
    let wait = if p.proposed() { " · waiting for your review" } else { "" };
    format!(" · {} of {} done{wait}", p.done(), p.steps.len())
}

pub(super) fn view(ui: Ui) -> impl IntoView {
    let (t, st, open) = (ui.colors, plan(ui), ui.state::<Open>().0);
    let words = move |f: fn(&Plan) -> String| move || st.plan.with(|p| p.as_ref().map(f).unwrap_or_default());
    let chevron =
        dyn_container(move || open.get(), |o| icon(if o { "chevron-down" } else { "chevron-right" }, 14.0).into_any());
    let line = h_stack((
        chevron,
        label(|| "Plan").style(move |s| bold(s).font_size(12.0)),
        label(words(|p| p.title.clone()))
            .style(|s| s.font_size(12.0).min_width(0.0).text_overflow(TextOverflow::Ellipsis)),
        label(words(said)).style(move |s| s.font_size(12.0).color(t.get().desc).flex_shrink(0.0_f32)),
    ))
    .style(|s| s.gap(5.0).items_center().width_full().cursor(CursorStyle::Pointer))
    .on_click_stop(move |_| open.update(|o| *o = !*o));
    let fill = empty().style(move |s| {
        let pct = st.plan.with(|p| p.as_ref().map_or(0.0, |p| 100.0 * p.done() as f64 / p.steps.len().max(1) as f64));
        s.height_full().width_pct(pct).background(t.get().progress)
    });
    let track = h_stack((fill,)).style(move |s| {
        let c = t.get();
        s.height(2.0).width_full().margin_top(4.0).background(crate::widgets::mix(c.widget, c.fg, 0.15))
    });
    let shown = create_memo(move |_| open.get());
    let steps = dyn_container(
        move || shown.get(),
        move |o| {
            if !o {
                return empty().into_any();
            }
            dyn_stack(
                move || st.plan.with(|p| p.as_ref().map(|p| p.steps.clone()).unwrap_or_default()),
                |s: &Step| format!("{}|{}|{}", s.number, s.status, s.title),
                move |s| step(ui, s),
            )
            .style(|s| s.flex_col().gap(2.0).margin_top(6.0).width_full())
            .into_any()
        },
    );
    v_stack((line, track, steps)).style(move |s| super::bar(t, s).apply_if(st.plan.with(Option::is_none), |s| s.hide()))
}

/// A step under the bar: its mark and its title.
fn step(ui: Ui, s: Step) -> impl IntoView {
    let t = ui.colors;
    let status = s.status.clone();
    let words = format!("{}. {}", s.number, crate::bidi::isolate(&s.title));
    h_stack((
        icon(mark(&status, &t.get_untracked()).0, 14.0).style(move |st| st.color(mark(&status, &t.get()).1)),
        label(move || words.clone()).style(|s| s.font_size(12.0).min_width(0.0).text_overflow(TextOverflow::Wrap)),
    ))
    .style(|s| s.gap(6.0).items_center().padding_left(19.0))
}

/// What it adds to the window's registries, once at start: its bar over the input.
pub fn register(_ui: Ui, _send: &Send) {
    super::slots::add(super::slots::Slot::Bars, "plan", 10, |ui, _| view(ui).into_any());
}

/// The session's message of its tasks, the plan's part taken in. Never taken away (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "tasks" {
        let now = m["plan"]["steps"].as_array().map(|steps| {
            let done = steps.iter().filter(|st| st["status"] == "done").count();
            (m["plan"]["title"].as_str().unwrap_or("").to_string(), done, steps.len())
        });
        if ui.plan.with_untracked(|p| *p != now) {
            ui.plan.set(now);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_says_how_far_the_plan_is_and_whether_it_waits() {
        let step = |status: &str| Step { status: status.into(), ..Step::default() };
        let mut p = Plan { steps: vec![step("done"), step("doing"), step("todo")], ..Plan::default() };
        assert_eq!(said(&p), " · 1 of 3 done");
        p.review = Some("proposed".into());
        assert_eq!(said(&p), " · 1 of 3 done · waiting for your review");
    }
}
