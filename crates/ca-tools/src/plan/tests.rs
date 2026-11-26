use super::*;

fn cmd(c: &str) -> bool {
    c.starts_with("cargo") || c.starts_with("npm")
}

fn apply(p: &Plan, args: Value) -> Result<(Plan, Vec<String>), PlanError> {
    p.apply(&args, &cmd)
}

#[test]
fn a_plan_written_whole_then_changed_step_by_step() {
    let (p, _) = apply(
        &Plan::default(),
        json!({"title": "Fix the parser", "goal": "Numbers with exponents parse", "steps": [
            {"title": "Write a failing test", "check": "cargo test -p parser exponent"},
            {"title": "Fix the lexer", "files": ["src/lex.rs"], "depends_on": [1]},
            {"title": "Docs", "check": "the README says so"}
        ]}),
    )
    .expect("made");
    assert_eq!(p.steps.iter().map(|s| s.number).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!(p.steps[1].depends_on, vec![1]);
    let (p, _) = apply(&p, json!({"steps": [{"number": 1, "status": "doing"}]})).expect("begun");
    assert_eq!(p.current().map(|s| s.number), Some(1));
    // Done only once its command passed.
    let e = apply(&p, json!({"steps": [{"number": 1, "status": "done"}]})).expect_err("not yet");
    assert!(e.0.contains("only once its check passed"), "{e}");
    let ran =
        p.ran("cd crates && cargo test -p parser exponent 2>&1 | tail -5", false, "1 failed", "t").expect("its check");
    let e = apply(&ran, json!({"steps": [{"number": 1, "status": "done"}]})).expect_err("failed");
    assert!(e.0.contains("failed last time"), "{e}");
    let ran = ran.ran("cargo test -p parser exponent", true, "ok", "t").expect("its check");
    let (p, _) = apply(&ran, json!({"steps": [{"number": 1, "status": "done"}]})).expect("done");
    assert_eq!(p.done(), 1);
    // A check in words is the model's to judge.
    let (p, _) = apply(&p, json!({"steps": [{"number": 3, "status": "done"}]})).expect("words");
    assert_eq!(p.done(), 2);
    // Removing a step takes it out of what waits for it.
    let (p, _) = apply(&p, json!({"steps": [{"number": 1, "remove": true}]})).expect("removed");
    assert!(p.steps.iter().all(|s| s.depends_on.is_empty()));
    assert!(p.text().contains("1 of 2 done."), "{}", p.text());
}

#[test]
fn a_call_is_read_kindly_and_said_back() {
    let (p, mended) = apply(
        &Plan::default(),
        json!({"steps": "[{\"id\": \"#1\", \"description\": \"look\", \"status\": \"in_progress\"}, {\"name\": \"fix\", \"dependsOn\": [1]}]"}),
    )
    .expect("made");
    assert!(mended.iter().any(|m| m.contains("string of JSON")));
    assert_eq!(p.steps[0].title, "look");
    assert_eq!(p.steps[0].status, DOING);
    assert_eq!(p.steps[1].depends_on, vec![1]);
    assert_eq!(p.title, "Plan");
}

#[test]
fn what_arenas_rules_refuse() {
    let base = apply(&Plan::default(), json!({"steps": [{"title": "a"}, {"title": "b"}]})).expect("made").0;
    let e = apply(&base, json!({"steps": [{"number": 1, "depends_on": [2]}, {"number": 2, "depends_on": [1]}]}))
        .expect_err("circle");
    assert!(e.0.contains("circle"), "{e}");
    let e = apply(&base, json!({"steps": [{"number": 1, "depends_on": [9]}]})).expect_err("no such");
    assert!(e.0.contains("does not have"), "{e}");
    let e = apply(&base, json!({"steps": [{"number": 7, "status": "done"}]})).expect_err("no step");
    assert!(e.0.contains("no step 7"), "{e}");
    let e = apply(&base, json!({"steps": [{"number": 1, "status": "maybe"}]})).expect_err("status");
    assert!(e.0.contains("one of todo, doing"), "{e}");
    assert!(apply(&base, json!({"steps": [{"check": "x"}]})).is_err(), "a new step needs a title");
}

#[test]
fn one_step_doing_at_a_time() {
    let base =
        apply(&Plan::default(), json!({"steps": [{"title": "a", "status": "doing"}, {"title": "b"}]})).expect("made").0;
    let (p, mended) = apply(&base, json!({"steps": [{"number": 2, "status": "doing"}]})).expect("moved on");
    assert_eq!(p.current().map(|s| s.number), Some(2));
    assert_eq!(p.steps[0].status, TODO);
    assert!(mended[0].contains("step 1 was doing"));
}

#[test]
fn checks_are_told_from_the_commands_that_run_them() {
    assert!(runs("cargo test", "cargo test"));
    assert!(runs("cd src && cargo  test", "cargo test"));
    assert!(runs("cargo test 2>&1 | tail -20", "cargo test"));
    assert!(!runs("cargo test -p other", "cargo test"));
    assert!(!runs("cargo build", ""));
}

#[test]
fn arenas_shape_and_back_from_the_store() {
    let (p, _) =
        apply(&Plan::default(), json!({"title": "T", "steps": [{"title": "a", "check": "npm test"}]})).expect("made");
    let v = p.to_arena("/repo");
    assert_eq!(v["steps"][0]["dependsOn"], json!([]));
    assert_eq!(v["place"], "/repo");
    let back: Plan = serde_json::from_str(&serde_json::to_string(&p).expect("json")).expect("back");
    assert_eq!(back, p);
}

#[tokio::test]
async fn the_tool_proposes_in_plan_mode_and_reads_with_nothing() {
    let (_d, mut ctx) = crate::testing::context();
    ctx.planning = true;
    let o = PlanTool.run(json!({"title": "T", "steps": [{"title": "a"}]}), &ctx).await;
    assert!(!o.is_error, "{}", o.text);
    assert!(o.text.contains("proposed"));
    assert_eq!(ctx.plan.get().review.as_deref(), Some(PROPOSED));
    let o = PlanTool.run(json!({}), &ctx).await;
    assert!(o.text.starts_with("Plan: T"));
    let changed = Arc::new(Mutex::new(0));
    let c2 = changed.clone();
    ctx.plan.on_change(Arc::new(move |_| *c2.lock().expect("n") += 1));
    let begun = Arc::new(Mutex::new(Vec::new()));
    let b2 = begun.clone();
    ctx.plan.on_begin(Arc::new(move |n| b2.lock().expect("b").push(n)));
    PlanTool.run(json!({"steps": [{"number": 1, "status": "doing"}]}), &ctx).await;
    assert_eq!(*changed.lock().expect("n"), 1);
    assert_eq!(*begun.lock().expect("b"), vec![1]);
}
