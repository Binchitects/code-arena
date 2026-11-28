//! The chat's own commands in the IDE, and what follows each turn there as it does in the terminal.
//!
//! A line sent between turns that is one of the chat's commands is answered here, in the thread. Which way each
//! is answered is [`route`]'s:
//!
//! | Command | In the IDE |
//! |---|---|
//! | `/help` | its list, as a notice |
//! | `/tasks`, `/context`, `/projects`, `/loop`, `/goal TEXT` | open their views: `ui_action{open, text?}` (`/loop EVERY ...`, `/goal` alone and `/goal` with a word of its own or a `--` are `ide_goal`'s) |
//! | `/resume` | opens the Sessions view; `/resume ID` switches to that session |
//! | `/fork [N]` | `session_fork{before_q?}`, as the Sessions view sends it |
//! | `/compact` | the driver's `compact` |
//! | `/rewind N [code\|chat]` | the driver's `rewind{q, what}`; `/rewind` alone lists the questions |
//! | `/clear`, `/new` | `ide_sessions`'s |
//! | `/review [WHAT]` | a turn: the model reviews the changes not committed yet |
//! | `/exit`, `/quit` | a notice: the window is closed to leave |
//! | the rest of `/help`'s list | the terminal's own command, its words collected (`Ui::capture`) and said as one notice; `status` and `tasks` follow, for what it changed (the model, the mode) |
//!
//! A command that needs Farzan Arena says so in the standalone edition (`edition::locked`). `!command` runs the
//! command at once and says its output; the model reads it with the next message (a note). Sent while a turn
//! runs, it waits for the turn's end, as a `/command` does.
//!
//! After each turn: `git.autoCommit` commits what the turn changed (the changes there were before it are left
//! alone), the `statusLine` command's first line goes to the faces (`status_line{text}`, and `status_line` in
//! `state`), a long turn has the next one reminded to keep what it worked out, and what the session's background
//! work said meanwhile (`Notices`) is said as `notice{text, toast: true, problem}` every few seconds.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::ide_ext::{Ctx, Sent, Shared, TurnEnd, say, send};
use crate::session::Resume;
use crate::ui::Ui;

/// One of the chat's own commands, as the `/` menu offers it.
pub struct Builtin {
    pub name: &'static str,
    pub description: &'static str,
    /// Offered in the `/` menu (a hidden one works when typed).
    pub listed: bool,
}

const fn b(name: &'static str, description: &'static str) -> Builtin {
    Builtin { name, description, listed: true }
}

/// Every command of the chat's own, in `/help`'s order.
pub const BUILTINS: &[Builtin] = &[
    b("/help", "Show the chat's commands"),
    b("/model", "Show the models, or switch to one"),
    b("/mode", "Show the mode, or switch: ask, auto-edit, plan, yolo"),
    b("/thinking", "Set the thinking: default, off, low, medium, high, xhigh"),
    b("/cost", "Show the tokens spent in this session"),
    b("/context", "Show what fills the model's window"),
    b("/compact", "Summarize the older part of the conversation now"),
    b("/clear", "Start a new session (the last stays saved)"),
    b("/new", "Start a new session (the last stays saved)"),
    b("/resume", "Show the folder's sessions, or switch to one"),
    b("/fork", "Carry on in a copy of this session (of before question N)"),
    b("/output-style", "Set how the agent talks: default, explanatory, learning, or yours"),
    b("/jobs", "Show the commands run in the background; stop N stops one"),
    b("/tools", "Show the tools by group; turn one on or off for this session"),
    b("/permissions", "Show the kept rules; allow or deny one for this session"),
    b("/compact-at", "Set when the session compacts, and what it keeps"),
    b("/sandbox", "Show what commands in the sandbox may do"),
    b("/rewind", "Show your questions; /rewind N goes back to before question N"),
    b("/search", "Find earlier sessions in this folder that said these words"),
    b("/commands", "Show your own and the project's commands, sub-agents and skills"),
    b("/skills", "Show the skills; import or export one"),
    b("/mcp", "Show the MCP servers: connected or not, and why"),
    b("/plan", "Show the working plan; approve, change or reject a proposed one"),
    b("/checklist", "Show your checklists; turn one on, tick an item"),
    b("/memory", "Show what is remembered; forget N forgets one"),
    b("/remember", "Keep a memory for this project (--person: for every project)"),
    b("/projects", "Show your projects in Arena"),
    b("/project", "Put this session in a project, make one, or show it"),
    b("/assistant", "Work as one of your assistants in Arena"),
    b("/arena-agents", "Show your agents in Arena; run one"),
    b("/arena-tasks", "Show your scheduled tasks in Arena; run one now"),
    b("/goal", "Set a goal: the agent works until it is met"),
    b("/loop", "Run a prompt every 30m (2h, 1d) while the session is open"),
    b("/commit", "Commit the working tree's changes, grouped and written by the model"),
    b("/review", "Review the changes not committed yet"),
    b("/hooks", "Show the hooks that run; trust runs the project's own"),
    b("/sync", "Show where this session stands with its chat in Arena"),
    b("/web", "Show your chats in Arena; /web N continues one here"),
    b("/exit", "Leave: close the window"),
    Builtin { name: "/quit", description: "Leave: close the window", listed: false },
    Builtin { name: "/tasks", description: "Show the tasks: the plan, jobs, loops and goal", listed: false },
];

/// What the `/` menu offers of them in this edition: `[{name, description}]`, names without the `/`.
pub fn listed(edition: ca_settings::Edition) -> Value {
    Value::Array(
        BUILTINS
            .iter()
            .filter(|c| c.listed && crate::edition::locked(edition, c.name).is_none())
            .map(|c| json!({"name": &c.name[1..], "description": c.description}))
            .collect(),
    )
}

/// `/help` in the IDE: the commands, then how a command or a skill of one's own and `!command` are typed.
fn help(edition: ca_settings::Edition) -> String {
    let mut lines: Vec<String> =
        BUILTINS.iter().filter(|c| c.listed).map(|c| format!("{} — {}", c.name, c.description)).collect();
    lines.push("/NAME [words] — a command or a skill of yours or the project's, with your words".into());
    lines.push("!command — run a command yourself; its output goes with your next message".into());
    crate::edition::help(edition, &lines.join("\n"))
}

/// How a line sent is answered.
#[derive(Debug, PartialEq)]
pub enum Route {
    /// Not the chat's own here: another area's, a command or skill of the library's, or words.
    Leave,
    Help,
    /// The terminal's command, its words said as a notice.
    Text,
    /// A view opened in the faces, with the text it starts with.
    Open(&'static str, String),
    /// A message of the driver's own, answered once this is.
    Post(Value),
    Resume(String),
    /// A turn with the review asked for.
    Review(String),
    Exit,
}

/// The way a line beginning with `/` is answered (see the module's table).
pub fn route(line: &str) -> Route {
    let line = line.trim();
    let (name, rest) = line.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((line, ""));
    match name {
        "/help" => Route::Help,
        "/clear" | "/new" => Route::Leave,
        "/tasks" => Route::Open("tasks", String::new()),
        "/context" => Route::Open("context", String::new()),
        "/projects" => Route::Open("projects", String::new()),
        "/loop" if rest.is_empty() => Route::Open("loops", String::new()),
        "/loop" => Route::Leave,
        "/goal" => {
            let first = rest.split_whitespace().next().unwrap_or("");
            if rest.is_empty() || matches!(first, "pause" | "resume" | "cancel" | "drop") || rest.contains("--") {
                Route::Leave
            } else {
                Route::Open("goal", rest.to_string())
            }
        }
        "/resume" if rest.is_empty() => Route::Open("sessions", String::new()),
        "/resume" => Route::Resume(rest.to_string()),
        "/fork" if rest.is_empty() => Route::Post(json!({"type": "session_fork"})),
        "/fork" => match rest.trim_start_matches('#').parse::<u64>() {
            Ok(q) => Route::Post(json!({"type": "session_fork", "before_q": q})),
            Err(_) => Route::Text,
        },
        "/compact" => Route::Post(json!({"type": "compact"})),
        "/rewind" if rest.is_empty() => Route::Text,
        "/rewind" => {
            let mut words = rest.split_whitespace();
            let q = words.next().map(|w| w.trim_start_matches('#')).and_then(|w| w.parse::<u64>().ok());
            let what = match words.next() {
                None => Some("both"),
                Some("code") => Some("code"),
                Some("chat") => Some("chat"),
                Some(_) => None,
            };
            match (q, what, words.next()) {
                (Some(q), Some(what), None) => Route::Post(json!({"type": "rewind", "q": q, "what": what})),
                _ => Route::Text,
            }
        }
        "/review" => Route::Review(rest.to_string()),
        "/exit" | "/quit" => Route::Exit,
        _ if BUILTINS.iter().any(|c| c.name == name) => Route::Text,
        _ => Route::Leave,
    }
}

/// What the model is asked by `/review`.
fn review_prompt(what: &str) -> String {
    let mut ask = "Review the changes in this working tree that are not committed yet (git status, git diff and git \
                   diff --staged). Look for bugs, risky changes and what is missing (tests, docs); say what you find \
                   by file and line, the worst first. Change nothing."
        .to_string();
    if !what.is_empty() {
        ask.push_str(&format!("\n\nLook especially at: {what}"));
    }
    ask
}

/// The tokens this session spent in the IDE, for `/cost` and the status line.
#[derive(Default)]
struct Spent(Mutex<(String, ca_llm::Usage)>);

impl Spent {
    fn of(&self, session: &str) -> ca_llm::Usage {
        let g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 == session { g.1 } else { ca_llm::Usage::default() }
    }

    fn add(&self, session: &str, used: ca_llm::Usage) {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 != session {
            *g = (session.to_string(), ca_llm::Usage::default());
        }
        g.1.add(used);
    }
}

/// The working tree's changes before the turn began (`git.autoCommit` leaves them alone).
#[derive(Default)]
struct Dirty(Mutex<Option<Vec<String>>>);

/// The status line's last line, for a face that comes later.
#[derive(Default)]
struct Line(Mutex<Option<String>>);

/// Which watch of the notices is the live one: one that sees another number ends.
#[derive(Default)]
struct Watch(std::sync::atomic::AtomicU64);

/// A line sent between turns: answered here when it is one of the chat's own commands or a `!command`.
pub async fn command(ctx: &mut Ctx<'_>, sent: &mut Sent) {
    let line = sent.model.trim().to_string();
    if let Some(cmd) = line.strip_prefix('!') {
        sent.done = true;
        shell(ctx, cmd.trim()).await;
        return;
    }
    if !line.starts_with('/') {
        return;
    }
    let route = route(&line);
    if route == Route::Leave {
        return;
    }
    sent.done = true;
    let out = ctx.sh.out.clone();
    let name = line.split_whitespace().next().unwrap_or(&line);
    if let Some(why) = crate::edition::locked(ctx.sh.edition, name) {
        say(&out, &why);
        return;
    }
    match route {
        Route::Leave => {}
        Route::Help => say(&out, &help(ctx.sh.edition)),
        Route::Open(view, text) => send(&out, ui_action(view, &text)),
        Route::Post(m) => ctx.sh.post(m),
        Route::Resume(id) => {
            if let Err(e) = crate::ide_sessions::switch_session(ctx, Resume::Id(id)).await {
                say(&out, &e);
            }
        }
        Route::Review(what) => {
            sent.model = review_prompt(&what);
            sent.done = false;
        }
        Route::Exit => {
            say(&out, "To leave, close the window (File > Exit). This session is kept: /resume brings it back.")
        }
        Route::Text => terminal(ctx, &line).await,
    }
}

/// `ui_action{open, text?}`: a view the faces open.
fn ui_action(view: &str, text: &str) -> Value {
    let mut v = json!({"type": "ui_action", "open": view});
    if !text.is_empty() {
        v["text"] = json!(text);
    }
    v
}

/// The terminal's own command, what it says collected and said in the thread as one notice.
async fn terminal(ctx: &mut Ctx<'_>, line: &str) {
    let cap = Ui::capture();
    let was = std::mem::replace(&mut ctx.p.ui, cap.ui);
    let mut usage = ctx.sh.kept.of::<Spent>().of(&ctx.p.session);
    let done = crate::chat::command(line, ctx.p, ctx.app, cap.ui, &mut usage).await;
    ctx.p.ui = was;
    let mut said = cap.lines();
    if let Err(e) = done {
        said.push(sentence(&e.to_string()));
    }
    let text = said.join("\n");
    if !text.trim().is_empty() {
        say(&ctx.sh.out, &text);
    }
    send(&ctx.sh.out, crate::ide::status(ctx.p, ctx.sh));
    send(&ctx.sh.out, crate::ide::tasks(ctx.sh));
}

/// An error's words as a sentence: a capital first and a full stop last.
fn sentence(e: &str) -> String {
    let mut c = e.trim().chars();
    let mut s: String = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
    if !s.is_empty() && !s.ends_with(['.', '!', '?', ')']) {
        s.push('.');
    }
    s
}

/// What a `!command`'s output shows in the thread at most (the model reads all of it).
const SHOWN: usize = 4000;

/// `!command`: run now, its output said, and read by the model with the next message.
async fn shell(ctx: &mut Ctx<'_>, cmd: &str) {
    if cmd.is_empty() {
        say(&ctx.sh.out, "Type a command after the !, as in !git status.");
        return;
    }
    let o = ca_tools::RunShell::run_line(cmd, &ctx.p.agent.ctx).await;
    say(&ctx.sh.out, &format!("$ {cmd}\n{}", shown(&o.text)));
    ctx.p.agent.note(format!("I ran `{cmd}` myself:\n{}", o.text));
}

/// A command's output as the thread shows it: its last part when it is long.
fn shown(text: &str) -> String {
    let text = text.trim_end();
    if text.len() <= SHOWN {
        return text.to_string();
    }
    let mut from = text.len() - SHOWN;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    format!("…{}", &text[from..])
}

/// A `!command` sent while a turn runs waits for the turn's end, as a `/command` does.
pub fn shape(sent: &mut Sent) {
    if sent.meanwhile && sent.model.trim_start().starts_with('!') {
        sent.later = true;
    }
}

/// Before a turn: the working tree's changes as they are, when `git.autoCommit` will commit the turn's own.
pub async fn before_turn(ctx: &mut Ctx<'_>) {
    let dirty = if ctx.app.settings.bool(&ca_settings::schema::AUTO_COMMIT) {
        Some(crate::commit::dirty(ctx.p).await)
    } else {
        None
    };
    *ctx.sh.kept.of::<Dirty>().0.lock().unwrap_or_else(|e| e.into_inner()) = dirty;
}

/// After a turn: its tokens counted, the reminder after a long one, its changes committed, the status line.
pub async fn after_turn(ctx: &mut Ctx<'_>, end: &TurnEnd) {
    let spent = ctx.sh.kept.of::<Spent>();
    if let Some(o) = &end.outcome {
        spent.add(&ctx.p.session, o.usage);
        ctx.p.nudge = o.steps >= 8;
    }
    let dirty = ctx.sh.kept.of::<Dirty>().0.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let (Some(dirty), Some(o)) = (dirty, &end.outcome)
        && !o.wrote.is_empty()
    {
        let review = ctx.app.settings.bool(&ca_settings::schema::AUTO_COMMIT_REVIEW);
        let cap = Ui::capture();
        let said = crate::commit::after_turn(ctx.p, &o.wrote, &dirty, review, cap.ui).await;
        let mut lines = cap.lines();
        lines.extend(said);
        if !lines.is_empty() {
            say(&ctx.sh.out, &lines.join("\n"));
        }
    }
    status_line(ctx, spent.of(&ctx.p.session));
}

/// The `statusLine` command run off the driver's loop, its first line sent as `status_line{text}`.
fn status_line(ctx: &mut Ctx<'_>, usage: ca_llm::Usage) {
    let Some(cmd) = crate::statusline::command(ctx.app) else { return };
    let input = crate::statusline::input(ctx.p, ctx.app, &usage);
    let cwd = ctx.p.agent.ctx.workspace.cwd().to_path_buf();
    let (out, kept) = (ctx.sh.out.clone(), ctx.sh.kept.of::<Line>());
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    rt.spawn(async move {
        let text = crate::statusline::run(&cmd, &input, &cwd).await.map(|l| plain(&l)).unwrap_or_default();
        *kept.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.clone());
        send(&out, json!({"type": "status_line", "text": text}));
    });
}

/// A line without its terminal escapes (colours and the like).
pub fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // CSI: ESC [ parameters, then one final byte from @ to ~; any other escape is ESC, its intermediate bytes
        // (space to /) and one final character.
        if chars.peek() == Some(&'[') {
            chars.next();
            for f in chars.by_ref() {
                if ('@'..='~').contains(&f) {
                    break;
                }
            }
        } else {
            while chars.next_if(|f| (' '..='/').contains(f)).is_some() {}
            chars.next();
        }
    }
    out.trim().to_string()
}

/// The status line's last text in `state`, for a face that comes later.
pub fn state(sh: &Shared, v: &mut Value) {
    if let Some(t) = sh.kept.of::<Line>().0.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        v["status_line"] = json!(t);
    }
}

/// How often what the background work said is looked for.
const EVERY: Duration = Duration::from_secs(2);

/// Says what the session's background work said meanwhile, as toasts; one watch for the session at a time.
pub fn start(sh: &Shared) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    let watch = sh.kept.of::<Watch>();
    let mine = watch.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let (notices, out) = (sh.notices.clone(), sh.out.clone());
    rt.spawn(async move {
        while watch.0.load(std::sync::atomic::Ordering::SeqCst) == mine {
            for (problem, text) in taken(&notices) {
                send(&out, json!({"type": "notice", "text": text, "toast": true, "problem": problem}));
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

fn taken(notices: &Arc<Mutex<Vec<(bool, String)>>>) -> Vec<(bool, String)> {
    notices.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_of_the_terminals_help_is_answered_here_or_by_its_own_area() {
        for c in BUILTINS {
            let r = route(c.name);
            // ide_sessions's, and ide_goal's where the goal stands.
            let owned = matches!(c.name, "/clear" | "/new" | "/goal");
            assert_eq!(r == Route::Leave, owned, "{} routes to {r:?}", c.name);
        }
        assert_eq!(route("/fix-tests now"), Route::Leave, "a command of the library's");
        assert_eq!(route("/loop 5m check"), Route::Leave, "ide_goal's");
        assert_eq!(route("/goal"), Route::Leave, "where the goal stands, from ide_goal");
        assert_eq!(route("/goal pause"), Route::Leave);
        assert_eq!(route("/goal make it pass --check cargo test"), Route::Leave);
    }

    #[test]
    fn the_views_open_and_the_drivers_own_messages_are_sent() {
        assert_eq!(route("/goal make the tests pass"), Route::Open("goal", "make the tests pass".into()));
        assert_eq!(route("/loop"), Route::Open("loops", String::new()));
        assert_eq!(route("/context"), Route::Open("context", String::new()));
        assert_eq!(route("/resume"), Route::Open("sessions", String::new()));
        assert_eq!(route("/resume abc"), Route::Resume("abc".into()));
        assert_eq!(route("/fork #3"), Route::Post(json!({"type": "session_fork", "before_q": 3})));
        assert_eq!(route("/fork"), Route::Post(json!({"type": "session_fork"})));
        assert_eq!(route("/fork x"), Route::Text, "the terminal's words say what is wrong");
        assert_eq!(route("/rewind 2 code"), Route::Post(json!({"type": "rewind", "q": 2, "what": "code"})));
        assert_eq!(route("/rewind 2"), Route::Post(json!({"type": "rewind", "q": 2, "what": "both"})));
        assert_eq!(route("/rewind"), Route::Text);
        assert_eq!(route("/rewind 2 files"), Route::Text);
        assert_eq!(route("/compact"), Route::Post(json!({"type": "compact"})));
        assert_eq!(route("/model qwen"), Route::Text);
        assert_eq!(route("/review the parser"), Route::Review("the parser".into()));
        assert!(review_prompt("the parser").ends_with("Look especially at: the parser"));
        assert_eq!(ui_action("goal", "x"), json!({"type": "ui_action", "open": "goal", "text": "x"}));
        assert_eq!(ui_action("tasks", ""), json!({"type": "ui_action", "open": "tasks"}));
    }

    #[test]
    fn the_menu_lists_the_commands_this_edition_has_and_help_names_them() {
        let all = listed(ca_settings::Edition::Managed);
        let names: Vec<&str> = all.as_array().expect("list").iter().filter_map(|c| c["name"].as_str()).collect();
        assert!(names.contains(&"compact") && names.contains(&"web") && !names.contains(&"tasks"), "{names:?}");
        let standalone = listed(ca_settings::Edition::Standalone);
        assert!(!standalone.to_string().contains("\"web\""), "locked without Arena");
        let h = help(ca_settings::Edition::Standalone);
        assert!(h.contains("/compact — ") && !h.contains("/web — ") && h.contains("!command — "), "{h}");
    }

    #[test]
    fn a_shell_command_sent_meanwhile_waits_and_its_output_is_cut_from_the_front() {
        let mut s = Sent::new(&Value::Null, "!ls".into(), true);
        shape(&mut s);
        assert!(s.later);
        let mut s = Sent::new(&Value::Null, "look at !this".into(), true);
        shape(&mut s);
        assert!(!s.later);
        let long = "é".repeat(SHOWN);
        let cut = shown(&long);
        assert!(cut.starts_with('…') && cut.len() <= SHOWN + 4);
        assert_eq!(shown("a\n\n"), "a");
    }

    #[test]
    fn a_status_line_loses_its_escapes_and_an_error_reads_as_a_sentence() {
        assert_eq!(plain("\u{1b}[1;32mmain\u{1b}[0m · qwen "), "main · qwen");
        assert_eq!(plain("a\u{1b}(Bb"), "ab");
        assert_eq!(
            sentence("there is no question 4: /rewind lists them"),
            "There is no question 4: /rewind lists them."
        );
        assert_eq!(sentence("Done."), "Done.");
    }

    #[test]
    fn what_is_spent_is_the_sessions_own() {
        let s = Spent::default();
        s.add("a", ca_llm::Usage { prompt: 10, cached: 2, completion: 3 });
        s.add("a", ca_llm::Usage { prompt: 5, ..Default::default() });
        assert_eq!(s.of("a").prompt, 15);
        assert_eq!(s.of("b").prompt, 0);
        s.add("b", ca_llm::Usage { prompt: 1, ..Default::default() });
        assert_eq!((s.of("a").prompt, s.of("b").prompt), (0, 1), "another session counts from nothing");
    }
}
