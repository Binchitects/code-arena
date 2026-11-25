//! `code-arena chat`: a conversation in this terminal. The prompt is edited in place; the answer streams; what is
//! typed while a turn runs is read by the model at the turn's next step; Ctrl+C stops the turn and keeps what was said.

mod editor;
mod history;

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
/search WORDS         earlier sessions in this folder that said these words\n\
/mcp                  Arena's, Farzan Graph's and your MCP servers: connected or not, and why\n\
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
    let mut p = session::prepare(app, &args.opts, approver.clone(), resume).await?;
    let inbox = Arc::new(Mutex::new(Vec::<String>::new()));
    p.agent.inbox = Some(inbox.clone());
    let hist = InputHistory::new(&app.paths.data, p.agent.ctx.workspace.cwd());
    let mut ed = Editor::new(hist.load());

    ui.say(&format!(
        "{} {} · {} · {} · {}",
        ui.bold(ca_core::PRODUCT),
        ca_core::VERSION,
        p.agent.model,
        p.agent.policy.mode.name(),
        ui.dim(&p.folder)
    ));
    if let Some(said) = &p.resumed {
        ui.say(&ui.dim(&format!("Resumed {}, {said}.", p.session)));
        show_last_turns(&p, ui);
    }
    ui.say(&ui.dim("/help for the commands. Ctrl+C stops a turn; Ctrl+D leaves."));

    let _raw = RawMode::on()?;
    let mut keys = spawn_input();
    let mut drawn = Drawn::default();
    let mut interrupted_once = false;
    let mut session_usage = ca_llm::Usage::default();
    let mut shell_output: Option<String> = None;
    let mut first = args.first.clone();

    loop {
        let line = if let Some(f) = first.take() {
            ui.say(&format!("{PROMPT}{f}"));
            f
        } else {
            draw(&ed, &mut drawn);
            let Some(line) = read_line(&mut ed, &mut keys, &mut drawn, &mut interrupted_once, ui) else { break };
            line
        };
        hist.add(&line);
        ed.remember(&line);
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
        if trimmed.starts_with('/') && command(trimmed, &mut p, app, ui, &mut session_usage).await? {
            continue;
        }
        let message = match shell_output.take() {
            Some(s) => format!("{s}\n\n{line}"),
            None => line.clone(),
        };
        let started = Instant::now();
        let mut progress = Progress::new(ui, true);
        let mut typed = Editor::default();
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
                let secs = started.elapsed().as_secs();
                ui.say(&ui.dim(&format!("{} · {}", usage_line(&out), context_line(&p.agent))));
                if secs > 10 {
                    ui::out("\x07");
                }
            }
            Some(Err(TurnError::Llm(e))) => ui.say(&ui.red(&e.to_string())),
            Some(Err(e)) => ui.say(&ui.red(&e.to_string())),
        }
        // A line typed as the turn ended waits in the prompt.
        let left: Vec<String> = inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        if !left.is_empty() {
            first = Some(left.join("\n"));
        } else if !typed.text().is_empty() {
            ed.set(&typed.text());
        }
    }
    Ok(0)
}

/// Reads keys until a line is sent; `None` to leave.
fn read_line(
    ed: &mut Editor,
    keys: &mut mpsc::UnboundedReceiver<Event>,
    drawn: &mut Drawn,
    once: &mut bool,
    ui: Ui,
) -> Option<String> {
    loop {
        let ev = keys.blocking_recv_compat()?;
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
                return Some(line);
            }
            Action::Exit => {
                finish_prompt("", drawn);
                return None;
            }
            Action::Interrupt if *once => {
                finish_prompt("", drawn);
                return None;
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

/// A key from the input thread, waiting for it (the prompt has nothing else to do meanwhile).
trait BlockingRecv {
    fn blocking_recv_compat(&mut self) -> Option<Event>;
}

impl BlockingRecv for mpsc::UnboundedReceiver<Event> {
    fn blocking_recv_compat(&mut self) -> Option<Event> {
        tokio::task::block_in_place(|| self.blocking_recv())
    }
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
    match name {
        "/help" => ui.say(help()),
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
