//! The plan's review (approve, another way, reject, change a step), a step undone, an earlier session's plan carried
//! on, the plan cleared; and, in `tasks`, the plan an earlier session of this folder left (`plan_left`).
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn). The work itself is the terminal's (`session::review_plan`,
//! `undo_step`, `resume_plan`): a review becomes the person's next message (`said_next`), so it runs as the next turn.
//!
//! Messages:
//! - `plan_review{action: approve | approve-ask | change | another | reject, step?, text?}` [Q]: answered by the
//!   next turn, or by `plan_review{action, error}` when there is no plan to review or a step is wrong.
//! - `plan_undo{step}`, `plan_resume`, `plan_clear` [R]: answered by `plan_undo{step, text | error}`,
//!   `plan_resume{text | error}`, `plan_clear{text}`.
//!
//! After each, `tasks` (and `status`, for the mode an approval sets) are said again.

use serde_json::{Value, json};

use crate::ide_ext::{Area, Ctx, Rule, Shared, send};

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("plan_review", Rule::Queue),
        ("plan_undo", Rule::Refuse("undo a step of the plan")),
        ("plan_resume", Rule::Refuse("carry on an earlier plan")),
        ("plan_clear", Rule::Refuse("clear the plan")),
    ];

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        let out = ctx.sh.out.clone();
        match crate::ide_ext::kind(m) {
            "plan_review" => {
                let action = m["action"].as_str().unwrap_or("");
                let said = review_words(m).and_then(|(w, rest)| crate::session::review_plan(ctx.p, w, &rest));
                match said {
                    Ok(text) => ctx.p.said_next = Some(text),
                    Err(e) => send(&out, json!({"type": "plan_review", "action": action, "error": e})),
                }
            }
            "plan_undo" => {
                let step = m["step"].as_u64().unwrap_or(0);
                let reply = match u32::try_from(step).map_err(|_| "No such step.".to_string()) {
                    Ok(n) => crate::session::undo_step(ctx.p, n),
                    Err(e) => Err(e),
                };
                send(&out, said("plan_undo", Some(step), reply));
            }
            "plan_resume" => send(&out, said("plan_resume", None, crate::session::resume_plan(ctx.p))),
            "plan_clear" => {
                ctx.p.agent.ctx.plan.set(ca_tools::plan::Plan::default());
                send(&out, said("plan_clear", None, Ok("The plan is cleared.".into())));
            }
            _ => return false,
        }
        send(&out, crate::ide::tasks(ctx.sh));
        send(&out, crate::ide::status(ctx.p, ctx.sh));
        true
    }

    /// The plan an earlier session of this folder left unfinished, while this session has none of its own.
    fn tasks(sh: &Shared, v: &mut Value) {
        if !sh.plan.get().is_empty() {
            return;
        }
        let left = sh.store.lock().ok().and_then(|s| s.plan_left(&sh.folder, &sh.session).ok().flatten());
        if let Some(l) = left.and_then(|(_, _, json)| left_of(&json)) {
            v["plan_left"] = l;
        }
    }
}

/// The review's words for `session::review_plan`: its action and what follows it.
fn review_words(m: &Value) -> Result<(&'static str, String), String> {
    let text = m["text"].as_str().unwrap_or("").trim().to_string();
    match m["action"].as_str().unwrap_or("") {
        "approve" => Ok(("approve", String::new())),
        "approve-ask" => Ok(("approve", "ask".into())),
        "change" => match m["step"].as_u64() {
            Some(n) => Ok(("change", format!("{n} {text}"))),
            None => Err("Say which step to change.".into()),
        },
        "another" => Ok(("another", text)),
        "reject" => Ok(("reject", text)),
        _ => Err("A plan's review is approve, approve-ask, change, another or reject.".into()),
    }
}

/// A reply: its words, or why it could not be done.
fn said(kind: &str, step: Option<u64>, r: Result<String, String>) -> Value {
    let mut v = match r {
        Ok(text) => json!({"type": kind, "text": text}),
        Err(e) => json!({"type": kind, "error": e}),
    };
    if let Some(n) = step {
        v["step"] = json!(n);
    }
    v
}

/// A plan left, as `tasks` says it: `{title, done, total}`; none when it cannot be read, is empty or is done (as
/// `session::resume_plan` would refuse it).
fn left_of(json: &str) -> Option<Value> {
    let plan: ca_tools::plan::Plan = serde_json::from_str(json).ok()?;
    let (done, total) = (plan.done(), plan.steps.len());
    (total > 0 && done < total).then(|| json!({"title": plan.title, "done": done, "total": total}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_review_is_said_in_the_terminals_words() {
        let r = |v: Value| review_words(&v);
        assert_eq!(r(json!({"action": "approve"})), Ok(("approve", String::new())));
        assert_eq!(r(json!({"action": "approve-ask"})), Ok(("approve", "ask".into())));
        assert_eq!(
            r(json!({"action": "change", "step": 2, "text": " use a map "})),
            Ok(("change", "2 use a map".into()))
        );
        assert!(r(json!({"action": "change", "text": "x"})).is_err(), "a change names its step");
        assert_eq!(r(json!({"action": "another", "text": "too slow"})), Ok(("another", "too slow".into())));
        assert_eq!(r(json!({"action": "reject"})), Ok(("reject", String::new())));
        assert!(r(json!({"action": "maybe"})).is_err());
    }

    #[test]
    fn a_plan_left_is_offered_only_while_some_of_it_is_to_do() {
        let plan = |a: &str, b: &str| {
            format!(
                r#"{{"title":"T","steps":[{{"number":1,"title":"a","status":"{a}"}},{{"number":2,"title":"b","status":"{b}"}}]}}"#
            )
        };
        assert_eq!(left_of(&plan("done", "todo")), Some(json!({"title": "T", "done": 1, "total": 2})));
        assert_eq!(left_of(&plan("done", "done")), None);
        assert_eq!(left_of(r#"{"title":"T","steps":[]}"#), None);
        assert_eq!(left_of("not json"), None);
    }

    #[test]
    fn a_reply_carries_its_step_and_its_words_or_why_not() {
        assert_eq!(said("plan_undo", Some(2), Ok("x".into())), json!({"type": "plan_undo", "step": 2, "text": "x"}));
        assert_eq!(said("plan_resume", None, Err("no".into())), json!({"type": "plan_resume", "error": "no"}));
    }
}
