//! `code-arena chat`: a conversation in this terminal. The prompt is edited in place; the answer streams; what is
//! typed while a turn runs is read by the model at the turn's next step; Ctrl+C stops the turn and keeps what was said.

mod editor;
mod history;

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ca_agent::{Answer, Approver, Mode, TurnError};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, queue, terminal};
use tokio::sync::{mpsc, oneshot};
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::run::{Progress, usage_line};
use crate::session::{self, Opts, Prepared, Resume};
use crate::ui::{self, Ui};
use editor::{Action, Editor};
use history::InputHistory;

const PROMPT: &str = "› ";

/// Raw mode while the chat runs, given back however it ends.
struct RawMode;

impl RawMode {
    fn on() -> std::io::Result<Self> {
        terminal::enable_raw_mode()?;
        let _ = execute!(std::io::stdout(), event::EnableBracketedPaste);
        ui::set_raw(true);
        Ok(RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = execute!(std::io::stdout(), event::DisableBracketedPaste);
        let _ = terminal::disable_raw_mode();
        ui::set_raw(false);
    }
}

/// The keys, from a thread of their own: the terminal is read by nothing else.
fn spawn_input() -> mpsc::UnboundedReceiver<Event> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(e) = event::read() {
            if tx.send(e).is_err() {
                break;
            }
        }
    });
    rx
}

/// Where the prompt was drawn, so it can be drawn again in place.
#[derive(Default)]
struct Drawn {
    /// Rows from the prompt's first row to the cursor's.
    cursor_row: u16,
}

/// Draws the prompt and its text (lines after the first indented as the prompt), the cursor where it is.
fn draw(ed: &Editor, drawn: &mut Drawn) {
    // A terminal that says no size (0) is taken as 80 columns.
    let width = terminal::size().ok().map(|(w, _)| w as usize).filter(|w| *w >= 10).unwrap_or(80);
    let mut o = std::io::stdout().lock();
    if drawn.cursor_row > 0 {
        let _ = queue!(o, cursor::MoveUp(drawn.cursor_row));
    }
    let _ = queue!(o, cursor::MoveToColumn(0), terminal::Clear(terminal::ClearType::FromCursorDown));
    let (mut row, mut col) = (0usize, PROMPT.chars().count());
    let (mut cur_row, mut cur_col) = (0usize, col);
    let mut text = String::from(PROMPT);
    for (i, c) in ed.text().chars().enumerate() {
        if i == ed.cursor() {
            (cur_row, cur_col) = (row, col);
        }
        if c == '\n' {
            text.push_str("\r\n  ");
            row += 1;
            col = 2;
            continue;
        }
        let w = c.width().unwrap_or(1);
        if col + w > width {
            row += 1;
            col = 0;
            text.push_str("\r\n");
        }
        text.push(c);
        col += w;
    }
    if ed.cursor() >= ed.text().chars().count() {
        (cur_row, cur_col) = (row, col);
    }
    let _ = o.write_all(text.as_bytes());
    if row > cur_row {
        let _ = queue!(o, cursor::MoveUp((row - cur_row) as u16));
    }
    let _ = queue!(o, cursor::MoveToColumn(cur_col as u16));
    let _ = o.flush();
    drawn.cursor_row = cur_row as u16;
}

/// Leaves the prompt's drawing as it is and goes below it.
fn finish_prompt(ed_text: &str, drawn: &mut Drawn) {
    let rows = ed_text.matches('\n').count() as u16;
    let mut o = std::io::stdout().lock();
    if rows > drawn.cursor_row {
        let _ = queue!(o, cursor::MoveDown(rows - drawn.cursor_row));
    }
    let _ = o.write_all(b"\r\n");
    let _ = o.flush();
    drawn.cursor_row = 0;
}

/// Takes the prompt's drawing away, to print above it (the web's news); it is drawn again after.
fn clear_prompt(drawn: &mut Drawn) {
    let mut o = std::io::stdout().lock();
    if drawn.cursor_row > 0 {
        let _ = queue!(o, cursor::MoveUp(drawn.cursor_row));
    }
    let _ = queue!(o, cursor::MoveToColumn(0), terminal::Clear(terminal::ClearType::FromCursorDown));
    let _ = o.flush();
    drawn.cursor_row = 0;
}

/// A question the turn waits on, answered by the next key.
type Pending = Arc<Mutex<Option<oneshot::Sender<Answer>>>>;

struct KeyApprover {
    ui: Ui,
    pending: Pending,
}

#[async_trait::async_trait]
impl Approver for KeyApprover {
    async fn ask(&self, tool: &str, subject: &str) -> Option<Answer> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = self.pending.lock() {
            *p = Some(tx);
        }
        let shown: String = subject.chars().take(300).collect();
        ui::out(&format!(
            "\x07{} {}\n  {}  ",
            self.ui.yellow(&format!("Allow {tool}?")),
            self.ui.dim(&shown),
            self.ui.bold("[y]es  [n]o  [a]lways for this session")
        ));
        rx.await.ok()
    }
}

/// The chat's own commands; anything else starting with `/` goes to the model as typed.
fn help() -> &'static str {
    "/help                 these commands\n\
/model [name]         show the models, or switch\n\
/mode [mode]          show the mode, or switch: ask, auto-edit, plan, yolo\n\
/thinking [level]     default, off, low, medium, high, xhigh\n\
/cost                 tokens spent in this session\n\
/context              what fills the model's window, piece by piece\n\
/compact              summarize the older part of the conversation now\n\
/clear                a new session (the last stays saved)\n\
/resume [id]          the folder's sessions, or switch to one\n\
/jobs [stop N]        commands run with no time limit or in the background; stop N stops one\n\
/tools [on|off NAME]  the tools, by group; turn a tool or a server's tools on or off for this session\n\
/permissions [allow|deny RULE]  the kept rules, and more for this session\n\
/compact-at [% [%]]   when the session compacts, and what it keeps; off for only when a request would not fit\n\
/sandbox              what commands in the sandbox may do\n\
/rewind [N] [code|chat]  your questions; /rewind N goes back to before question N\n\
/search WORDS         earlier sessions in this folder that said these words\n\
/commands             your own and the project's commands, sub-agents and skills\n\
/skills               the skills; /skills import PATH [--to project|mine|arena] [--replace] (a .zip, a folder, a SKILL.md);\n\
                      /skills export NAME [PATH.zip]\n\
/NAME [words]         a command or a skill, with your words\n\
/mcp                  Arena's, Farzan Graph's and your MCP servers: connected or not, and why\n\
/plan                 the working plan; /plan approve [ask] | change N what | another why | reject why reviews a proposed one;\n\
                      /plan undo N puts the files back as before step N; /plan resume carries on the last session's plan\n\
/checklist [N|NAME|off]  your checklists; turn one on (the model ticks each item), tick N, new NAME: a; b, edit N: a; b, delete N\n\
/memory [forget N]    what is remembered (Arena's, else this machine's); forget N forgets one\n\
/remember [--person] TEXT  keep a memory for this project (or, with --person, for every project)\n\
/projects             your projects in Arena; /project N puts this session in one (off: none)\n\
/assistant [N|off]    your assistants in Arena; work as one\n\
/sync [on|off]        where this session stands with its chat in Arena; off keeps it here only\n\
/web [N]              your chats in Arena; /web N continues one here\n\
/exit                 leave (also Ctrl+D)\n\
!command              run a command yourself; its output goes with your next message\n\
\n\
Keys: Enter sends; Alt+Enter (or a line ending in \\) starts a new line; ↑ ↓ bring back what you sent;\n\
Ctrl+C stops a turn (at the prompt it clears the line; twice on an empty one leaves); Ctrl+Z stops code-arena."
}

pub struct ChatArgs {
    pub opts: Opts,
    pub resume: Resume,
    pub first: Option<String>,
    /// `--resume` with no id: choose from a list.
    pub choose: bool,
}

pub async fn run(app: &mut App, args: ChatArgs) -> anyhow::Result<i32> {
    let ui = app.ui;
    let pending: Pending = Arc::new(Mutex::new(None));
    let approver = Arc::new(KeyApprover { ui, pending: pending.clone() });
    let mut resume = args.resume.clone();
    if args.choose {
        resume = choose_session(app, &args.opts)?;
    }
    // Keys are read from the start: what is typed while the session gets ready waits for the prompt.
    let raw = RawMode::on()?;
    let mut keys = spawn_input();
    let mut p = session::prepare(app, &args.opts, approver.clone(), resume).await?;
    let inbox = Arc::new(Mutex::new(Vec::<String>::new()));
    p.agent.inbox = Some(inbox.clone());
    let hist = InputHistory::new(&app.paths.data, p.agent.ctx.workspace.cwd());
    let mut ed = Editor::new(hist.load());

    ui.say(&format!(
        "{} {} {} · {} · {} · {}",
        ui.bold(ca_core::PRODUCT),
        ca_core::VERSION,
        ui.dim(app.edition().name()),
        p.agent.model,
        p.agent.policy.mode.name(),
        ui.dim(&p.folder)
    ));
    if let Some(said) = &p.resumed {
        ui.say(&ui.dim(&format!("Resumed {}, {said}.", p.session)));
        show_last_turns(&p, ui);
    }
    ui.say(&ui.dim("/help for the commands. Ctrl+C stops a turn; Ctrl+D leaves."));

    let mut ended = match &p.agent.ctx.jobs {
        Some(j) => j.listen(),
        None => mpsc::unbounded_channel().1,
    };
    let _raw = raw;
    // The web's news while the prompt waits: taken in at once.
    let (news_tx, mut news) = mpsc::unbounded_channel::<()>();
    let mut listening: Option<Arc<ca_sync::ChatSync>> = None;
    let mut drawn = Drawn::default();
    let mut interrupted_once = false;
    let mut session_usage = ca_llm::Usage::default();
    let mut shell_output: Option<String> = None;
    let mut first = args.first.clone();

    loop {
        let mut carry_on = false;
        let line = if let Some(f) = first.take() {
            ui.say(&format!("{PROMPT}{f}"));
            f
        } else if let Some(said) = p.said_next.take() {
            // A command's words to the agent (a plan's review), as the person's next message.
            carry_on = true;
            ui.say(&ui.dim(&format!("{PROMPT}{said}")));
            said
        } else if let Ok(j) = ended.try_recv() {
            carry_on = true;
            job_note(&j, ui)
        } else {
            // A new session (/clear, /resume, /web) is listened for too.
            if let Some(s) = &p.sync
                && !listening.as_ref().is_some_and(|l| Arc::ptr_eq(l, s))
            {
                s.listen(news_tx.clone());
                listening = Some(s.clone());
            }
            session::say_notices(&p.notices, ui);
            draw(&ed, &mut drawn);
            match read_line(&mut ed, &mut keys, &mut ended, &mut news, &mut drawn, &mut interrupted_once, ui).await {
                Input::Line(l) => l,
                Input::News => {
                    if let Some(s) = p.sync.clone() {
                        let taken = s.take_in().await;
                        for line in &taken.said {
                            ui.say(&ui.dim(line));
                        }
                        if taken.any() {
                            session::reload_history(&mut p);
                        }
                    }
                    continue;
                }
                Input::JobEnded(j) => {
                    carry_on = true;
                    job_note(&j, ui)
                }
                Input::Leave => break,
            }
        };
        if !carry_on {
            hist.add(&line);
            ed.remember(&line);
        }
        let trimmed = line.trim();
        if trimmed == "/exit" || trimmed == "/quit" {
            break;
        }
        if let Some(cmd) = trimmed.strip_prefix('!') {
            let o = ca_tools::RunShell::run_line(cmd, &p.agent.ctx).await;
            ui::out(&format!("{}\n", o.text));
            shell_output = Some(format!("I ran `{cmd}` myself:\n{}", o.text));
            continue;
        }
        if !carry_on && trimmed.starts_with('/') && command(trimmed, &mut p, app, ui, &mut session_usage).await? {
            continue;
        }
        // Read again at each message: a command or skill added or changed works now.
        let library = ca_agent::library::load(&p.sources);
        let line = if !carry_on && trimmed.starts_with('/') {
            match library.slash(trimmed) {
                Some(expanded) => expanded,
                None => {
                    ui.say(&ui.yellow(&format!(
                        "No command or skill {}: /help, /commands and /skills list them.",
                        trimmed.split_whitespace().next().unwrap_or(trimmed)
                    )));
                    continue;
                }
            }
        } else {
            line
        };
        let message = match shell_output.take() {
            Some(s) => format!("{s}\n\n{line}"),
            None => line.clone(),
        };
        session::begin_turn(&mut p, ui).await;
        session::before_message(&mut p, &message).await;
        let started = Instant::now();
        let mut progress = Progress::new(ui, true);
        progress.show_thinking = true;
        progress.plan = vec![p.agent.ctx.plan.clone()];
        let mut typed = Editor::default();
        let jobs = p.agent.ctx.jobs.clone();
        let result = {
            let turn = p.agent.turn(&message, &mut progress);
            tokio::pin!(turn);
            loop {
                tokio::select! {
                    r = &mut turn => break Some(r),
                    Some(ev) = keys.recv() => {
                        let Event::Key(k) = ev else { continue };
                        if k.kind != KeyEventKind::Press { continue; }
                        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
                            // The turn stops, and the commands it started with no time limit; those in the background run on.
                            if let Some(j) = &jobs
                                && let Some(t) = j.current_turn()
                            {
                                for id in j.stop_turn(t) {
                                    ui.say(&ui.dim(&format!("Stopped job {id}.")));
                                }
                            }
                            break None;
                        }
                        let waiting = pending.lock().ok().and_then(|mut g| g.take());
                        if let Some(tx) = waiting {
                            let answer = answer_of(k);
                            ui::out(&format!("{}\n", match answer { Answer::Yes => "yes", Answer::Always => "always, for this session", Answer::No => "no" }));
                            let _ = tx.send(answer);
                            continue;
                        }
                        // Typed while it works: kept, and read at the turn's next step.
                        if let Action::Submit(text) = typed.key(k)
                            && let Ok(mut q) = inbox.lock()
                        {
                            q.push(text);
                        }
                    }
                }
            }
        };
        if !progress.at_line_start {
            ui::out("\n");
        }
        match result {
            None => {
                if let Ok(mut g) = pending.lock() {
                    g.take();
                }
                ui.say(&ui.yellow("Stopped. What was said is kept."));
            }
            Some(Ok(out)) => {
                session_usage.add(out.usage);
                p.nudge = out.steps >= 8;
                let secs = started.elapsed().as_secs();
                ui.say(&ui.dim(&format!("{} · {}", usage_line(&out), context_line(&p.agent))));
                if secs > 10 {
                    ui::out("\x07");
                }
            }
            Some(Err(TurnError::Llm(e))) => ui.say(&ui.red(&e.to_string())),
            Some(Err(e)) => ui.say(&ui.red(&e.to_string())),
        }
        if let Some(s) = &p.sync {
            s.end_turn_soon();
        }
        // A line typed as the turn ended waits in the prompt.
        let left: Vec<String> = inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        if !left.is_empty() {
            first = Some(left.join("\n"));
        } else if !typed.text().is_empty() {
            ed.set(&typed.text());
        }
    }
    if let Some(j) = &p.agent.ctx.jobs {
        j.stop_all();
    }
    // What the last turn said reaches the chat in Arena before Code Arena goes (at most a few seconds).
    if let Some(s) = &p.sync {
        s.end_turn().await;
        s.flush(Duration::from_secs(5)).await;
    }
    Ok(0)
}

/// Reads keys until a line is sent; `None` to leave.
/// What the prompt gave: a line, a job that ended meanwhile, or the end.
enum Input {
    Line(String),
    JobEnded(ca_tools::jobs::Ended),
    /// The web chat has messages this session lacks.
    News,
    Leave,
}

async fn read_line(
    ed: &mut Editor,
    keys: &mut mpsc::UnboundedReceiver<Event>,
    ended: &mut mpsc::UnboundedReceiver<ca_tools::jobs::Ended>,
    news: &mut mpsc::UnboundedReceiver<()>,
    drawn: &mut Drawn,
    once: &mut bool,
    ui: Ui,
) -> Input {
    loop {
        let ev = tokio::select! {
            e = keys.recv() => match e { Some(e) => e, None => return Input::Leave },
            Some(j) = ended.recv() => {
                finish_prompt(&ed.text(), drawn);
                return Input::JobEnded(j);
            }
            Some(()) = news.recv() => {
                // Several at once are one take-in.
                while news.try_recv().is_ok() {}
                clear_prompt(drawn);
                return Input::News;
            }
        };
        let action = match ev {
            Event::Paste(s) => {
                ed.paste(&s);
                Action::Edited
            }
            Event::Key(k) if k.kind == KeyEventKind::Press => ed.key(k),
            Event::Resize(_, _) => Action::Edited,
            _ => Action::Ignored,
        };
        if action != Action::Interrupt {
            *once = false;
        }
        match action {
            Action::Submit(line) => {
                finish_prompt(&line, drawn);
                return Input::Line(line);
            }
            Action::Exit => {
                finish_prompt("", drawn);
                return Input::Leave;
            }
            Action::Interrupt if *once => {
                finish_prompt("", drawn);
                return Input::Leave;
            }
            Action::Interrupt => {
                *once = true;
                finish_prompt("", drawn);
                ui.say(&ui.dim("(Ctrl+C again, or Ctrl+D, to leave)"));
            }
            Action::ClearScreen => {
                let _ = execute!(std::io::stdout(), terminal::Clear(terminal::ClearType::All), cursor::MoveTo(0, 0));
                drawn.cursor_row = 0;
            }
            Action::Suspend => suspend(),
            Action::Cleared | Action::Edited | Action::Ignored => {}
        }
        draw(ed, drawn);
    }
}

/// Ctrl+Z: the terminal given back, code-arena stopped as a shell stops a job; `fg` brings it back.
fn suspend() {
    #[cfg(unix)]
    {
        let _ = terminal::disable_raw_mode();
        ui::set_raw(false);
        let _ = nix::sys::signal::raise(nix::sys::signal::Signal::SIGTSTP);
        let _ = terminal::enable_raw_mode();
        ui::set_raw(true);
    }
}

/// `/tools`: the tools by group; `/tools off NAME` (a tool, or a group: a server's name) leaves them out of the
/// requests for this session, and a call to one is refused; `/tools on NAME` brings them back.
fn tools_command(rest: &str, p: &mut Prepared, ui: Ui) {
    let (cmd, name) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
    let agent = &mut p.agent;
    match cmd {
        "off" if !name.is_empty() => {
            let (off, on): (Vec<_>, Vec<_>) =
                agent.tools.drain(..).partition(|t| t.def().name == name || t.group() == name);
            agent.tools = on;
            let (off2, on2): (Vec<_>, Vec<_>) =
                agent.deferred.drain(..).partition(|t| t.def().name == name || t.group() == name);
            agent.deferred = on2;
            let n = off.len() + off2.len();
            p.disabled.extend(off.into_iter().chain(off2));
            ui.say(&if n == 0 {
                format!("No tool or group named {name}.")
            } else {
                format!("{n} tools off for this session.")
            });
        }
        "on" if !name.is_empty() => {
            let (on, still): (Vec<_>, Vec<_>) =
                p.disabled.drain(..).partition(|t| t.def().name == name || t.group() == name);
            p.disabled = still;
            ui.say(&format!("{} tools on again.", on.len()));
            p.agent.tools.extend(on);
        }
        _ => {
            let mut groups: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
            for t in &agent.tools {
                groups.entry(t.group()).or_default().push(t.def().name);
            }
            for t in &agent.deferred {
                groups.entry(t.group()).or_default().push(format!("{} (loaded when needed)", t.def().name));
            }
            for t in &p.disabled {
                groups.entry(t.group()).or_default().push(format!("{} (off)", t.def().name));
            }
            for (g, names) in groups {
                ui.say(&format!("{} {}", ui.bold(&g), ui.dim(&format!("({})", names.len()))));
                ui.say(&format!("  {}", names.join(", ")));
            }
        }
    }
}

/// The person's questions in this session: (number, its place in the session, its first line).
fn questions(p: &Prepared) -> anyhow::Result<Vec<(usize, i64, String)>> {
    let store = p.store.lock().map_err(|_| anyhow::anyhow!("the session's store"))?;
    let turns = store.summary(&p.session)?.turns;
    let page = store.page(&p.session, turns.max(1), None)?;
    let mut out = Vec::new();
    let mut last_turn = 0;
    for m in page.messages {
        if m.turn != last_turn {
            last_turn = m.turn;
            // A turn a job's end started is no question of the person's.
            if m.message.content.starts_with("(Code Arena: job ") {
                continue;
            }
            out.push((
                out.len() + 1,
                m.seq,
                m.message.content.lines().next().unwrap_or("").chars().take(100).collect(),
            ));
        }
    }
    Ok(out)
}

/// `/rewind`: the questions, with which have a checkpoint; `/rewind N [code|chat]` goes back to before question N:
/// the files put back (a file a turn made is removed) and the conversation cut there, or only one of them.
fn rewind(rest: &str, p: &mut Prepared, ui: Ui) -> anyhow::Result<()> {
    let qs = questions(p)?;
    let mut words = rest.split_whitespace();
    let Some(n) = words.next().and_then(|w| w.parse::<usize>().ok()) else {
        for (n, seq, text) in &qs {
            let mark =
                if p.agent.ctx.checkpoints.has(*seq) { "" } else { "  (conversation only: no checkpoint in this run)" };
            ui.say(&format!("{n:>3}. {text}{}", ui.dim(mark)));
        }
        ui.say(&ui.dim("/rewind N goes back to before question N: the code and the conversation; /rewind N code or /rewind N chat for one."));
        return Ok(());
    };
    let Some((_, seq, text)) = qs.iter().find(|(k, _, _)| *k == n).cloned() else {
        anyhow::bail!("there is no question {n}: /rewind lists them");
    };
    let what = words.next().unwrap_or("both");
    if what == "code" || what == "both" {
        let r = p.agent.ctx.checkpoints.rewind(seq);
        let ws = p.agent.ctx.workspace.clone();
        for f in &r.put_back {
            ui.say(&format!("  put back {}", ws.display(f)));
        }
        for f in &r.removed {
            ui.say(&format!("  removed {} (a turn made it)", ws.display(f)));
        }
        for (f, e) in &r.failed {
            ui.say(&ui.red(&format!("  {}: {e}", ws.display(f))));
        }
        if r.put_back.is_empty() && r.removed.is_empty() {
            ui.say(&ui.dim("  No file to put back (what commands changed is not put back: git has it)."));
        }
    }
    if what == "chat" || what == "both" {
        let store = p.store.lock().map_err(|_| anyhow::anyhow!("the session's store"))?;
        store.truncate(&p.session, seq)?;
        let (history, _) = session::model_history(&store, &p.session)?;
        drop(store);
        p.agent.notes = history
            .iter()
            .enumerate()
            .map(|(i, m)| i == 0 && m.content.starts_with("(Code Arena: the earlier part"))
            .collect();
        p.agent.history = history;
        ui.say(&format!("Back to before question {n}: {text}"));
    }
    Ok(())
}

/// `context 18.2k / 131k (14%), compacts at 80%`
fn context_line(a: &ca_agent::Agent) -> String {
    let used = a.context_used();
    let w = &a.window;
    let when = match w.compact_at {
        Some(p) => format!("compacts at {p}%"),
        None => "compacts only when a request would not fit".into(),
    };
    format!("context {} / {} ({}%), {when}", ui::short(used), ui::short(w.size), used * 100 / w.size.max(1))
}

/// A job with no time limit ended: said here, and the model carries on from it in a turn of its own.
fn job_note(j: &ca_tools::jobs::Ended, ui: Ui) -> String {
    let how = match (j.stopped, j.exit) {
        (true, _) => "was stopped".to_string(),
        (false, Some(c)) => format!("ended with exit code {c}"),
        (false, None) => "ended by a signal".to_string(),
    };
    ui.say(&ui.dim(&format!("job {} {how}: the model carries on", j.id)));
    format!("(Code Arena: job {} `{}` {how}. The end of its output:)\n{}", j.id, j.command, j.tail.trim_end())
}

/// The last two turns of a resumed session, as they were said.
fn show_last_turns(p: &Prepared, ui: Ui) {
    let Ok(store) = p.store.lock() else { return };
    let Ok(page) = store.page(&p.session, 2, None) else { return };
    for m in page.messages {
        let text: String = m.message.content.lines().next().unwrap_or("").chars().take(160).collect();
        match m.message.role {
            ca_llm::Role::User if m.message.tool_call_id.is_none() && !text.is_empty() => {
                ui.say(&format!("{PROMPT}{text}"))
            }
            ca_llm::Role::Assistant if !m.message.tool_calls.is_empty() => {
                for c in &m.message.tool_calls {
                    ui.say(&ui.dim(&format!("● {}", c.name)));
                }
            }
            ca_llm::Role::Assistant if !text.is_empty() => ui.say(&ui.dim(&text)),
            _ => {}
        }
    }
}

fn choose_session(app: &App, opts: &Opts) -> anyhow::Result<Resume> {
    let store = session::open_store(app)?;
    let folder = opts.cwd.clone().map(Ok).unwrap_or_else(std::env::current_dir)?;
    let folder = std::fs::canonicalize(&folder).unwrap_or(folder).display().to_string();
    let list = store.list(&folder, 20)?;
    if list.is_empty() {
        app.ui.say("No saved session in this folder: a new one begins.");
        return Ok(Resume::New);
    }
    for (i, s) in list.iter().enumerate() {
        let when = chrono::DateTime::from_timestamp_millis(s.updated)
            .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        app.ui.say(&format!(
            "{:>3}. {}  {}  {}",
            i + 1,
            app.ui.dim(&when),
            s.title,
            app.ui.dim(&format!("({} messages)", s.messages))
        ));
    }
    let pick = ui::ask_line("Which one (a number, Enter for the newest)? ")?;
    let n: usize = if pick.is_empty() { 1 } else { pick.parse().unwrap_or(0) };
    match list.get(n.wrapping_sub(1)) {
        Some(s) => Ok(Resume::Id(s.id.clone())),
        None => anyhow::bail!("{pick} is not one of the numbers listed"),
    }
}

/// Runs one of the chat's commands; false when it is none of them (it goes to the model as typed).
async fn command(
    line: &str,
    p: &mut Prepared,
    app: &mut App,
    ui: Ui,
    usage: &mut ca_llm::Usage,
) -> anyhow::Result<bool> {
    let (name, rest) = line.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((line, ""));
    if let Some(why) = crate::edition::locked(app.edition(), name) {
        ui.say(&ui.yellow(&why));
        return Ok(true);
    }
    match name {
        "/help" => ui.say(&crate::edition::help(app.edition(), help())),
        "/model" if rest.is_empty() => match p.agent.gateway.models().await {
            Ok(ms) => {
                for m in ms {
                    let mark = if m.id == p.agent.model { "●" } else { " " };
                    ui.say(&format!("{mark} {}", m.id));
                }
            }
            Err(e) => ui.say(&ui.red(&e.to_string())),
        },
        "/model" => {
            p.agent.model = rest.to_string();
            let _ = p.store.lock().map(|s| s.set_model(&p.session, rest, p.agent.policy.mode.name()));
            if let Some(s) = &p.sync {
                s.set_model(rest);
            }
            ui.say(&format!("The model is {rest}."));
        }
        "/mode" if rest.is_empty() => {
            ui.say(&format!("The mode is {} (ask, auto-edit, plan, yolo).", p.agent.policy.mode.name()))
        }
        "/mode" => match Mode::parse(rest) {
            Some(m) => {
                p.agent.policy.mode = m;
                ui.say(&format!("The mode is {}.", m.name()));
                if m == Mode::Yolo {
                    ui.say(&ui.yellow("yolo: every command and edit runs without asking."));
                }
            }
            None => ui.say(&ui.red("The modes are ask, auto-edit, plan and yolo.")),
        },
        "/thinking" => {
            p.agent.thinking = ca_llm::Thinking::parse(rest);
            ui.say(&format!("Thinking: {}.", if rest.is_empty() { "default" } else { rest }));
        }
        "/cost" => ui.say(&format!(
            "This run: {} in ({} read from the cache) · {} out.",
            ui::thousands(usage.prompt),
            ui::thousands(usage.cached),
            ui::thousands(usage.completion)
        )),
        "/clear" => {
            let opts = Opts {
                cwd: Some(p.agent.ctx.workspace.cwd().to_path_buf()),
                add_dirs: vec![],
                mode: Some(p.agent.policy.mode.name().into()),
                model: Some(p.agent.model.clone()),
                thinking: None,
                interactive: true,
                carry_plan: false,
            };
            let approver = p.agent.approver.clone();
            let inbox = p.agent.inbox.clone();
            *p = session::prepare(app, &opts, approver, Resume::New).await?;
            p.agent.inbox = inbox;
            ui.say(&ui.dim(&format!("A new session: {}.", p.session)));
        }
        "/resume" if rest.is_empty() => {
            let list = p.store.lock().map_err(|_| anyhow::anyhow!("the store"))?.list(&p.folder, 20)?;
            for s in list {
                ui.say(&format!("{}  {}  {}", s.id, s.title, ui.dim(&format!("({} messages)", s.messages))));
            }
            ui.say(&ui.dim("/resume ID switches to one."));
        }
        "/resume" => {
            let opts = Opts {
                cwd: Some(p.agent.ctx.workspace.cwd().to_path_buf()),
                add_dirs: vec![],
                mode: Some(p.agent.policy.mode.name().into()),
                model: None,
                thinking: None,
                interactive: true,
                carry_plan: false,
            };
            let approver = p.agent.approver.clone();
            let inbox = p.agent.inbox.clone();
            *p = session::prepare(app, &opts, approver, Resume::Id(rest.to_string())).await?;
            p.agent.inbox = inbox;
            ui.say(&ui.dim(&format!("Resumed {}, {}.", p.session, p.resumed.clone().unwrap_or_default())));
            show_last_turns(p, ui);
        }
        "/mcp" => {
            for line in crate::servers::describe(&p.servers) {
                ui.say(&line);
            }
            if p.servers.is_empty() {
                ui.say("No MCP server: sign in for Arena's tools, or add yours under mcpServers in settings.json.");
            }
        }
        "/compact" => {
            let mut progress = Progress::new(ui, false);
            match p.agent.compact(&mut progress, true).await {
                Ok(Some(_)) => ui.say(&format!("Compacted: {}.", context_line(&p.agent))),
                Ok(None) => ui.say("Nothing older to summarize."),
                Err(e) => ui.say(&ui.red(&format!("The model gave no summary ({e}): the conversation is as it was."))),
            }
        }
        "/context" => {
            let b = p.agent.context_breakdown();
            let w = &p.agent.window;
            let t = |c: usize| ui::short(w.tokens(c));
            ui.say(&context_line(&p.agent));
            for (label, chars) in [
                ("system prompt and instructions", b.system),
                ("tools' definitions", b.tools),
                ("your messages", b.you),
                ("answers and calls", b.answers),
                ("tool results", b.tool_results),
                ("Code Arena's notes", b.notes),
            ] {
                ui.say(&format!("  {:<32}{:>8}", label, t(chars)));
            }
            ui.say(&format!("  {:<32}{:>8}", "kept for the answer", ui::short(w.reserve)));
            ui.say(&ui.dim("  the biggest:"));
            for (i, role, chars, start) in b.biggest {
                ui.say(&ui.dim(&format!("    #{:<4} {role:<12}{:>8}  {start}", i + 1, t(chars))));
            }
        }
        "/skills" if rest.starts_with("import") || rest.starts_with("export") => {
            ui.say(&skills_transfer(rest, p, app).await)
        }
        "/commands" | "/skills" => {
            let lib = ca_agent::library::load(&p.sources);
            if name == "/commands" {
                for c in &lib.commands {
                    ui.say(&format!("/{:<20} {} {}", c.name, c.description, ui.dim(&format!("({})", c.source))));
                }
                for a in &lib.agents {
                    ui.say(&format!(
                        "{:<21} {} {}",
                        format!("agent {}", a.name),
                        a.description,
                        ui.dim(&format!("({})", a.source))
                    ));
                }
            }
            for k in &lib.skills {
                ui.say(&format!("/{:<20} {} {}", k.name, k.description, ui.dim(&format!("(skill, {})", k.source))));
            }
            if lib.commands.is_empty() && lib.skills.is_empty() && lib.agents.is_empty() {
                ui.say("None yet: .code-arena/commands/NAME.md is /NAME, .code-arena/skills/NAME/SKILL.md a skill, .code-arena/agents/NAME.md a sub-agent.");
            }
        }
        "/rewind" => rewind(rest, p, ui)?,
        "/web" => web(rest, p, app, ui).await?,
        "/checklist" => ui.say(&crate::checklist::command(rest, p).await),
        "/projects" | "/project" => ui.say(&crate::projects::command(name, rest, p).await),
        "/assistant" => ui.say(&crate::projects::assistant(rest, p).await),
        "/memory" | "/remember" => ui.say(&session::memory_command(p, name, rest, ui).await),
        "/plan" if rest.is_empty() => ui.say(&session::render_plan(&p.agent.ctx.plan.get(), ui)),
        "/plan" => {
            let (word, more) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
            let said = match word.to_ascii_lowercase().as_str() {
                "resume" => session::resume_plan(p).map(|s| (s, false)),
                "clear" => {
                    p.agent.ctx.plan.set(ca_tools::plan::Plan::default());
                    Ok(("The plan is cleared.".to_string(), false))
                }
                "undo" => match more.trim_start_matches('#').parse::<u32>() {
                    Ok(n) => session::undo_step(p, n).map(|s| (s, false)),
                    Err(_) => Err("Say which step to undo: /plan undo 2.".to_string()),
                },
                w => session::review_plan(p, w, more).map(|s| (s, true)),
            };
            match said {
                Ok((s, false)) => {
                    ui.say(&ui.dim(&s));
                    ui.say(&session::render_plan(&p.agent.ctx.plan.get(), ui));
                }
                // A review is said to the agent as the person's next message.
                Ok((s, true)) => {
                    ui.say(&ui.dim(&format!("The mode is {}.", p.agent.policy.mode.name())));
                    p.said_next = Some(s);
                }
                Err(e) => ui.say(&ui.red(&e)),
            }
        }
        "/sync" => match (&p.sync, rest) {
            (None, _) => ui.say(&ui.dim(
                "Kept here only: sign in (code-arena login) with arena.syncChats on to keep sessions in Arena's chats.",
            )),
            (Some(s), "off") => {
                s.set_on(false);
                ui.say("This session is kept here only from now on (/sync on sends it again).");
            }
            (Some(s), "on") => {
                s.set_on(true);
                ui.say("This session is kept with its chat in Arena again: what it lacks is on its way.");
            }
            (Some(s), _) => ui.say(&s.describe()),
        },
        "/tools" => tools_command(rest, p, ui),
        "/permissions" => {
            let (cmd, rule) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
            match cmd {
                "allow" if !rule.is_empty() => {
                    p.agent.policy.rules.allow(rule);
                    ui.say(&format!(
                        "Allowed for this session: {rule} (add it to permissions.allow in settings.json to keep it)."
                    ));
                }
                "deny" if !rule.is_empty() => {
                    p.agent.policy.rules.deny(rule);
                    ui.say(&format!(
                        "Refused for this session: {rule} (add it to permissions.deny in settings.json to keep it)."
                    ));
                }
                _ => {
                    let (allow, deny) = p.agent.policy.rules.written();
                    ui.say(&format!(
                        "Allowed without asking: {}",
                        if allow.is_empty() { "none".into() } else { allow.join(", ") }
                    ));
                    ui.say(&format!(
                        "Refused in every mode: {}",
                        if deny.is_empty() { "none".into() } else { deny.join(", ") }
                    ));
                    ui.say(&ui.dim("/permissions allow RULE or deny RULE: a tool's name with a pattern, run_shell(npm test*), read_file(.env*)."));
                }
            }
        }
        "/compact-at" => {
            let mut words = rest.split_whitespace();
            match words.next() {
                None => ui.say(&context_line(&p.agent)),
                Some("off") => {
                    p.agent.window.compact_at = None;
                    ui.say("For this session: compacts only when a request would not fit.");
                }
                Some(n) => {
                    match n.trim_end_matches('%').parse::<u64>() {
                        Ok(v) if (20..=95).contains(&v) => {
                            p.agent.window.compact_at = Some(v);
                            if let Some(t) = words.next().and_then(|t| t.trim_end_matches('%').parse::<u64>().ok()) {
                                p.agent.window.target = t.clamp(5, v.saturating_sub(10).max(5));
                            }
                            ui.say(&format!("For this session: {} (agent.compactAt and agent.compactTarget in settings.json keep it).", context_line(&p.agent)));
                        }
                        _ => ui.say(&ui.red("A share from 20 to 95, or off.")),
                    }
                }
            }
        }
        "/sandbox" => match &p.agent.ctx.sandbox {
            Some(s) => {
                for line in s.describe() {
                    ui.say(&line);
                }
            }
            None => ui.say(&format!(
                "No sandbox: commands run as you, and the mode is what guards them ({}).",
                ca_tools::sandbox::find().err().unwrap_or_else(|| "tools.sandbox is off".into())
            )),
        },
        "/jobs" => match &p.agent.ctx.jobs {
            None => ui.say("No jobs in this run."),
            Some(j) => {
                if let Some(id) = rest.strip_prefix("stop").map(str::trim).and_then(|n| n.parse::<u32>().ok()) {
                    ui.say(if j.stop(id) { "Stopped." } else { "That job is not running." });
                } else {
                    let list = j.list();
                    if list.is_empty() {
                        ui.say("No command runs with no time limit or in the background.");
                    }
                    for i in list {
                        let state = if i.running {
                            "running".to_string()
                        } else {
                            format!("ended ({})", i.exit.map(|c| c.to_string()).unwrap_or_else(|| "stopped".into()))
                        };
                        ui.say(&format!(
                            "{:>3}  {:<12} {}{}",
                            i.id,
                            state,
                            i.command,
                            if i.background { ui.dim("  (background)") } else { String::new() }
                        ));
                    }
                }
            }
        },
        "/search" => {
            let hits = p.store.lock().map_err(|_| anyhow::anyhow!("the store"))?.search(&p.folder, rest, 20)?;
            if hits.is_empty() {
                ui.say("Nothing found.");
            }
            for (s, seq, text) in hits {
                ui.say(&format!("{} #{}  {text}", ui.dim(&s), seq + 1));
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// What a key typed at a question answers: y (or Enter), a, and anything else is a no.
/// `/skills import PATH [--to project|mine|arena] [--replace]` and `/skills export NAME [PATH]`.
async fn skills_transfer(rest: &str, p: &mut Prepared, app: &App) -> String {
    use ca_agent::skill_transfer as st;
    let words: Vec<&str> = rest.split_whitespace().collect();
    let cwd = p.agent.ctx.workspace.cwd().to_path_buf();
    let at = |w: &str| {
        let path = std::path::PathBuf::from(w);
        if path.is_absolute() { path } else { cwd.join(path) }
    };
    if words.first() == Some(&"export") {
        let Some(name) = words.get(1) else { return "/skills export NAME [PATH.zip]".into() };
        let lib = ca_agent::library::load(&p.sources);
        let Some(k) = lib.skills.iter().find(|k| k.name == *name) else {
            return format!("No skill named {name}: /skills lists them.");
        };
        let Some(dir) = k.path.parent() else { return "That skill has no folder.".into() };
        let to = words.get(2).map(|w| at(w)).unwrap_or_else(|| st::default_export(&cwd, name));
        return match st::export_zip(dir, name, &to) {
            Ok(left) if left.is_empty() => format!("/{name} is in {}.", to.display()),
            Ok(left) => format!("/{name} is in {} (left out: {}).", to.display(), left.join("; ")),
            Err(e) => e,
        };
    }
    let Some(from) = words.get(1) else {
        return "/skills import PATH (a .zip, a folder, or a SKILL.md) [--to project|mine|arena] [--replace]".into();
    };
    let to = words.iter().position(|w| *w == "--to").and_then(|i| words.get(i + 1)).copied().unwrap_or("project");
    let replace = words.contains(&"--replace");
    let found = match st::read(&at(from)) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let left =
        if found.left_out.is_empty() { String::new() } else { format!(" Left out: {}.", found.left_out.join("; ")) };
    match to {
        "arena" => {
            if app.edition() != ca_settings::Edition::Managed {
                return "Skills in Arena need Farzan Arena (the managed edition): import to project or mine instead."
                    .into();
            }
            let Ok(arena) = app.arena() else { return "Not signed in to Arena.".into() };
            let folders: Vec<ca_arena::skills::Folder> = found
                .skills
                .iter()
                .map(|s| ca_arena::skills::Folder { folder: s.name.clone(), files: s.files.clone() })
                .collect();
            let choices: Vec<serde_json::Value> = if replace {
                found.skills.iter().map(|s| serde_json::json!({"folder": s.name, "action": "replace"})).collect()
            } else {
                vec![]
            };
            match arena.import_skills(&folders, &choices, false).await {
                Ok(v) => {
                    let names = |k: &str, f: &str| -> Vec<String> {
                        v[k].as_array()
                            .into_iter()
                            .flatten()
                            .map(|x| x[f].as_str().unwrap_or("?").to_string())
                            .collect()
                    };
                    let skipped: Vec<String> = v["skipped"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|x| {
                            format!("{} ({})", x["name"].as_str().unwrap_or("?"), x["reason"].as_str().unwrap_or(""))
                        })
                        .collect();
                    format!(
                        "In Arena, as yours (every project and the web chat): {}.{}{left}",
                        if names("imported", "name").is_empty() {
                            "none".into()
                        } else {
                            names("imported", "name").join(", ")
                        },
                        if skipped.is_empty() {
                            String::new()
                        } else {
                            format!(" Not brought in: {}.", skipped.join("; "))
                        }
                    )
                }
                Err(e) => e.to_string(),
            }
        }
        where_ @ ("project" | "mine") => {
            let dir = if where_ == "project" {
                p.agent.ctx.workspace.root().join(".code-arena").join("skills")
            } else {
                app.paths.config.join("skills")
            };
            match st::write_into(&dir, &found.skills, replace) {
                Ok((done, kept)) => format!(
                    "Into {}: {}.{}{left} They are used from the next message.",
                    dir.display(),
                    if done.is_empty() { "none".into() } else { done.join(", ") },
                    if kept.is_empty() {
                        String::new()
                    } else {
                        format!(" There already (kept; --replace replaces them): {}.", kept.join(", "))
                    }
                ),
                Err(e) => e.to_string(),
            }
        }
        other => format!("--to is project, mine or arena, not {other}."),
    }
}

/// `/web`: the person's chats in Arena; `/web N` (or its id) continues one here, in a session of its own.
async fn web(rest: &str, p: &mut Prepared, app: &mut App, ui: Ui) -> anyhow::Result<()> {
    if p.sync.is_none() {
        ui.say(&ui.dim("Not kept with Arena: sign in (code-arena login) with arena.syncChats on."));
        return Ok(());
    }
    let arena = app.arena()?;
    let list = match arena.chats(20).await {
        Ok(l) => l.chats,
        Err(e) => {
            ui.say(&ui.red(&e.to_string()));
            return Ok(());
        }
    };
    if rest.is_empty() {
        if list.is_empty() {
            ui.say(&ui.dim("No chats in Arena yet."));
        }
        for (i, c) in list.iter().enumerate() {
            let place = c.origin_place.as_deref().map(|p| format!(", {p}")).unwrap_or_default();
            ui.say(&format!("{:>3}  {}  {}", i + 1, c.title, ui.dim(&format!("({} messages{place})", c.messages))));
        }
        ui.say(&ui.dim("/web N continues one here."));
        return Ok(());
    }
    let chosen = match rest.parse::<usize>() {
        Ok(n) if (1..=list.len()).contains(&n) => list[n - 1].clone(),
        _ => match list.iter().find(|c| c.id == rest) {
            Some(c) => c.clone(),
            None => {
                ui.say(&ui.red(&format!("No chat {rest}: /web lists them.")));
                return Ok(());
            }
        },
    };
    // A session of this folder's that is kept with it already: taken up; else a new one, filled from the web.
    let (session, fresh) = {
        let store = p.store.lock().map_err(|_| anyhow::anyhow!("the store"))?;
        match store.by_chat(&chosen.id)? {
            Some(s) if store.summary(&s).map(|x| x.folder == p.folder).unwrap_or(false) => (s, false),
            _ => {
                let id = store.create(&p.folder, &p.agent.model, p.agent.policy.mode.name())?;
                store.link_chat(&id, &chosen.id)?;
                (id, true)
            }
        }
    };
    let opts = Opts {
        cwd: Some(p.agent.ctx.workspace.cwd().to_path_buf()),
        add_dirs: vec![],
        mode: Some(p.agent.policy.mode.name().into()),
        model: chosen.model.clone().or_else(|| Some(p.agent.model.clone())),
        thinking: None,
        interactive: true,
        carry_plan: false,
    };
    let approver = p.agent.approver.clone();
    let inbox = p.agent.inbox.clone();
    *p = session::prepare(app, &opts, approver, Resume::Id(session)).await?;
    p.agent.inbox = inbox;
    if let Some(s) = p.sync.clone() {
        let taken = s.take_in().await;
        if taken.any() {
            session::reload_history(p);
        }
    }
    let _ = arena.continued(&chosen.id).await;
    let count = p.store.lock().ok().and_then(|s| s.summary(&p.session).ok()).map(|s| s.messages).unwrap_or(0);
    ui.say(&ui.dim(&format!(
        "{} the web chat \"{}\" here: {count} messages.",
        if fresh { "Continuing" } else { "Back in" },
        chosen.title
    )));
    show_last_turns(p, ui);
    Ok(())
}

fn answer_of(k: KeyEvent) -> Answer {
    match k.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Answer::Yes,
        KeyCode::Char('a') | KeyCode::Char('A') => Answer::Always,
        _ => Answer::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_takes_y_a_or_no() {
        let k = |c| KeyEvent::new(c, KeyModifiers::NONE);
        assert_eq!(answer_of(k(KeyCode::Char('y'))), Answer::Yes);
        assert_eq!(answer_of(k(KeyCode::Enter)), Answer::Yes);
        assert_eq!(answer_of(k(KeyCode::Char('a'))), Answer::Always);
        assert_eq!(answer_of(k(KeyCode::Char('x'))), Answer::No);
        assert_eq!(answer_of(k(KeyCode::Esc)), Answer::No);
    }
}
