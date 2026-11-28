//! Steps at once (`run_steps`): steps of the plan that wait for nothing not done run together. A step that changes no
//! file goes to a read-only sub-agent; one that changes files goes to a worker in a git worktree of its own, made from
//! the project as it is now: it edits there, runs no command but its step's check, and its change comes back once that
//! check passed, merged into the project (`git apply --3way`), each file kept for `/rewind`. Each step ends done or
//! blocked, with why in its notes; a worktree whose step did not merge is kept for the person to look at.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ca_llm::ToolDef;
use ca_tools::commits::git;
use ca_tools::plan::{BLOCKED, DOING, DONE, PROPOSED, Plan, Step};
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::{Value, json};

use crate::policy::{Answer, Approver, Mode, Question};
use crate::task::{Crew, TaskTool};

pub const MOST_AT_ONCE: usize = 4;

pub struct RunSteps {
    pub crew: Arc<Mutex<Option<Crew>>>,
}

/// A worker's questions: its step's own check runs; nothing else does (nobody is there to say yes).
struct OnlyItsCheck(Option<String>);

#[async_trait::async_trait]
impl Approver for OnlyItsCheck {
    async fn ask(&self, q: &Question<'_>) -> Option<Answer> {
        let (tool, subject) = (q.tool, q.subject);
        let same = |a: &str, b: &str| {
            a.split_whitespace().collect::<Vec<_>>() == b.trim_matches('`').split_whitespace().collect::<Vec<_>>()
        };
        Some(match &self.0 {
            Some(check) if tool == "run_shell" && same(subject, check) => Answer::Yes,
            _ => Answer::No,
        })
    }
}

fn prompt(plan: &Plan, step: &Step, worktree: Option<&Path>) -> String {
    let mut p = format!("Carry out step {} of this plan, and only it: {}.\n", step.number, step.title);
    if !step.files.is_empty() {
        p.push_str(&format!("Its files: {}.\n", step.files.join(", ")));
    }
    if !step.symbols.is_empty() {
        p.push_str(&format!("What it changes: {}.\n", step.symbols.join(", ")));
    }
    if let Some(n) = &step.notes {
        p.push_str(&format!("Its notes: {n}\n"));
    }
    if let Some(c) = &step.check {
        p.push_str(&match worktree {
            None => format!("Done means: {c}\n"),
            Some(_) => format!("Done means: {c}. When you have changed what it needs, run its check with run_shell exactly as written: `{c}` (no other command runs here).\n"),
        });
    }
    p.push_str(match worktree {
        None => "You cannot change files: find out what the step asks, and report it plainly with the paths and lines it rests on.\n",
        Some(_) => "Change the files with edit_file and write_file. At the end, report in a few lines what you changed and how the check went.\n",
    });
    if let Some(t) = worktree {
        p.push_str(&format!("You work in a worktree of your own at {}: a copy of the project, apart from the person's. Change only what your step needs.\n", t.display()));
    }
    p.push_str(&format!("\nThe whole plan, for what is around your step:\n{}", plan.text()));
    p
}

/// The project as it is now, as a commit (what is changed and not committed kept: `git stash create`), else HEAD.
async fn basis(root: &Path) -> Result<String, String> {
    if let Ok(c) = git(root, &["stash", "create"], None).await
        && !c.trim().is_empty()
    {
        return Ok(c.trim().to_string());
    }
    git(root, &["rev-parse", "HEAD"], None)
        .await
        .map(|h| h.trim().to_string())
        .map_err(|_| "The repository has no commit yet: steps that change files run at once from one.".to_string())
}

/// (done, what came of it, its report)
type Ran = (bool, String, String);

async fn research(crew: &Crew, plan: &Plan, step: &Step, ctx: &Context) -> Ran {
    let mut c = crew.clone();
    c.policy.mode = Mode::Plan;
    c.approver = Arc::new(crate::policy::NoAsking);
    // Each step's sub-agent is told of by an id of its own, not by the call that runs the steps.
    let mut ctx = ctx.clone();
    ctx.call = ctx.call.map(|c| format!("{c}-step-{}", step.number));
    let o = TaskTool::new(Arc::new(Mutex::new(Some(c))))
        .run(json!({"description": format!("step {}", step.number), "prompt": prompt(plan, step, None)}), &ctx)
        .await;
    if o.is_error {
        (false, format!("Its sub-agent could not finish: {}", o.text), String::new())
    } else {
        (true, "Found out by a sub-agent: its report is below.".into(), o.text)
    }
}

async fn worker(crew: &Crew, plan: &Plan, step: &Step, root: &Path, base: &str, session_dir: &Path) -> Ran {
    let tree: PathBuf =
        session_dir.join(format!("step-{}-{:06x}", step.number, std::process::id() ^ (step.number << 8)));
    let _ = std::fs::create_dir_all(session_dir);
    if let Err(e) = git(root, &["worktree", "add", "--detach", &tree.to_string_lossy(), base], None).await {
        return (false, format!("Its worktree could not be made: {e}"), String::new());
    }
    let mut keep = false;
    let ran = async {
        let ws = match ca_workspace::Workspace::open(&tree, &[]) {
            Ok(w) => w,
            Err(e) => return (false, format!("Its worktree could not be opened: {e}"), String::new()),
        };
        let mut c = crew.clone();
        let mut ctx = Context::new(Arc::new(ws), crew.ctx.shell.clone(), crew.ctx.shell_timeout);
        // Commands run only for its check, and as the person, in its own copy.
        ctx.sandbox = None;
        c.ctx = ctx.clone();
        c.policy.mode = Mode::AutoEdit;
        c.approver = Arc::new(OnlyItsCheck(step.check.clone()));
        c.tools.retain(|t| !matches!(t.def().name.as_str(), "run_steps" | "stop_command" | "command_output"));
        let o = TaskTool::new(Arc::new(Mutex::new(Some(c))))
            .run(
                json!({"description": format!("step {}", step.number), "prompt": prompt(plan, step, Some(&tree))}),
                &ctx,
            )
            .await;
        if o.is_error {
            keep = true;
            return (
                false,
                format!("Its worker could not finish ({}); its worktree is kept: {}.", o.text, tree.display()),
                String::new(),
            );
        }
        // Its check, run once more on what it left: done only when it passes.
        if let Some(check) = &step.check
            && ca_tools::plan::is_command(check, &tree)
        {
            let r = ca_tools::RunShell.run(json!({"command": check, "timeout": 600}), &ctx).await;
            if r.is_error {
                keep = true;
                let tail: String = r.text.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
                return (
                    false,
                    format!(
                        "Its check `{check}` failed in its worktree ({}, kept for you to look at): {tail}",
                        tree.display()
                    ),
                    o.text,
                );
            }
        }
        match merge(&tree, base, root, &crew.ctx).await {
            Ok(()) => (
                true,
                match &step.check {
                    Some(c) => format!("Changed in its worktree, its check `{c}` passed, and merged into the project."),
                    None => "Changed in its worktree and merged into the project.".into(),
                },
                o.text,
            ),
            Err(e) => {
                keep = true;
                (false, format!("{e}; its worktree is kept: {}.", tree.display()), o.text)
            }
        }
    }
    .await;
    if !keep {
        let _ = git(root, &["worktree", "remove", "--force", &tree.to_string_lossy()], None).await;
    }
    ran
}

/// The worker's change, from the basis, applied to the project (each file kept for `/rewind` first).
async fn merge(tree: &Path, base: &str, root: &Path, ctx: &Context) -> Result<(), String> {
    git(tree, &["add", "-A"], None).await?;
    let diff = git(tree, &["diff", "--cached", "--binary", base], None)
        .await
        .map_err(|e| format!("Its change could not be read: {e}"))?;
    if diff.trim().is_empty() {
        return Ok(());
    }
    let names = git(tree, &["diff", "--cached", "--name-only", base], None).await.unwrap_or_default();
    for n in names.lines().filter(|l| !l.trim().is_empty()) {
        ctx.checkpoints.before_write(&root.join(n.trim()));
    }
    git(root, &["apply", "--3way", "--binary", "--whitespace=nowarn", "-"], Some(&diff))
        .await
        .map(|_| ())
        .map_err(|e| format!("Its change does not merge into the project as it is now ({e})"))
}

#[async_trait::async_trait]
impl Tool for RunSteps {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "run_steps".into(),
            description: format!(
                "Runs 2 to {MOST_AT_ONCE} steps of your plan at once, those that wait for nothing not done: a step with no files to change goes to a read-only sub-agent; one with files goes to a worker in a git worktree of its own (a copy of the project), which edits there and runs only its step's check, and its change is merged into the project once that check passed. Each step ends done or blocked, with why. Steps that change files run at once only in auto-edit or yolo mode."
            ),
            parameters: json!({"type": "object", "properties": {"steps": {"type": "array", "items": {"type": "integer"}, "description": "The steps' numbers."}}, "required": ["steps"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, a: &Value) -> String {
        let n: Vec<String> =
            a.get("steps").and_then(Value::as_array).into_iter().flatten().map(|v| v.to_string()).collect();
        format!("steps {}", n.join(", "))
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let Some(crew) = self.crew.lock().ok().and_then(|c| c.clone()) else {
            return Output::error("Not set up in this run.");
        };
        let plan = ctx.plan.get();
        if plan.review.as_deref() == Some(PROPOSED) {
            return Output::error("The plan waits for the person's review: it is carried out once approved.");
        }
        let mut asked: Vec<u32> = args
            .get("steps")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_u64())
            .map(|n| n as u32)
            .collect();
        asked.dedup();
        if !(2..=MOST_AT_ONCE).contains(&asked.len()) {
            return Output::error(format!(
                "run_steps runs 2 to {MOST_AT_ONCE} steps at once; carry one step out yourself."
            ));
        }
        let mut steps = Vec::new();
        for n in &asked {
            let Some(s) = plan.steps.iter().find(|s| s.number == *n) else {
                return Output::error(format!("The plan has no step {n}."));
            };
            if s.status == DONE || s.status == DOING {
                return Output::error(format!("Step {n} is {} already.", s.status));
            }
            if let Some(w) = s
                .depends_on
                .iter()
                .find(|d| plan.steps.iter().find(|x| x.number == **d).is_none_or(|x| x.status != DONE))
            {
                return Output::error(format!(
                    "Step {n} waits for step {w}, which is not done: run only steps that wait for nothing not done."
                ));
            }
            steps.push(s.clone());
        }
        let changing: Vec<u32> = steps.iter().filter(|s| !s.files.is_empty()).map(|s| s.number).collect();
        let root = ctx.workspace.root().to_path_buf();
        let mut base = String::new();
        if !changing.is_empty() {
            if !matches!(crew.policy.mode, Mode::AutoEdit | Mode::Yolo) {
                return Output::error(format!(
                    "Steps that change files ({}) run at once only in auto-edit or yolo mode, where edits run without asking: carry them out one at a time.",
                    changing.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
                ));
            }
            if ca_tools::commits::top(&root).await.is_none() {
                return Output::error(
                    "Steps that change files run at once in git worktrees: this folder is not a git repository. Carry them out one at a time.",
                );
            }
            base = match basis(&root).await {
                Ok(b) => b,
                Err(e) => return Output::error(e),
            };
        }
        (crew.say)(format!(
            "Running steps {} at once…",
            asked.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
        ));
        let mut doing = plan.clone();
        for s in doing.steps.iter_mut().filter(|s| asked.contains(&s.number)) {
            s.status = DOING.into();
        }
        ctx.plan.load(doing);
        let session_dir = std::env::temp_dir().join("code-arena-steps").join(format!("{}", std::process::id()));
        let runs = futures_util::future::join_all(steps.iter().map(|s| {
            let (crew, plan, root, base, dir) = (&crew, &plan, &root, &base, &session_dir);
            async move {
                if s.files.is_empty() {
                    research(crew, plan, s, ctx).await
                } else {
                    worker(crew, plan, s, root, base, dir).await
                }
            }
        }))
        .await;
        let mut now = ctx.plan.get();
        let mut report = String::new();
        for (s, (done, said, rep)) in steps.iter().zip(runs) {
            if let Some(x) = now.steps.iter_mut().find(|x| x.number == s.number) {
                x.status = if done { DONE } else { BLOCKED }.into();
                let notes = match &x.notes {
                    Some(n) => format!("{n}\n{said}"),
                    None => said.clone(),
                };
                x.notes = Some(notes.chars().take(4000).collect());
            }
            report.push_str(&format!(
                "Step {} ({}): {}. {said}\n",
                s.number,
                s.title,
                if done { "done" } else { "blocked" }
            ));
            if !rep.trim().is_empty() {
                report.push_str(&rep.chars().take(8000).collect::<String>());
                report.push('\n');
            }
            report.push('\n');
        }
        report.push_str(&now.text());
        ctx.plan.set(now);
        let mut o = Output::ok(report.trim().to_string());
        o.wrote = changing
            .iter()
            .flat_map(|n| {
                steps.iter().filter(move |s| s.number == *n).flat_map(|s| s.files.iter().map(|f| root.join(f)))
            })
            .collect();
        o
    }
}
