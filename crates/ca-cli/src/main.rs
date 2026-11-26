//! `code-arena`: Farzan Arena's coding agent.

mod account;
mod app;
mod automation;
mod chat;
mod checklist;
mod commit;
mod desktop;
mod edition;
mod goals;
mod ide;
mod ide_account;
mod ide_agents;
mod ide_arena;
mod ide_attach;
mod ide_automation;
mod ide_banners;
mod ide_chat;
mod ide_complete;
mod ide_dap;
mod ide_ext;
mod ide_extensions;
mod ide_files;
mod ide_folders;
mod ide_format;
mod ide_git;
mod ide_goal;
mod ide_harness;
mod ide_lsp;
mod ide_media;
mod ide_memory;
mod ide_notebook;
mod ide_plan;
mod ide_problems;
mod ide_review;
mod ide_sessions;
mod ide_settings;
mod ide_skills;
mod ide_snippets;
mod ide_symbols;
mod ide_tasks;
mod ide_themes;
mod ide_tools;
mod ide_trust;
mod ide_update;
mod ide_vars;
mod ide_vm;
mod log;
mod loops;
mod plugins;
mod projects;
mod run;
mod schedules;
mod servers;
mod session;
mod session_tools;
mod statusline;
mod ui;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "code-arena",
    version = ca_core::VERSION,
    about = "Farzan Arena's coding agent",
    after_help = "Examples:\n  code-arena login --url https://DOMAIN\n  code-arena -p \"list the TODOs in src/\"\n  git diff | code-arena -p - --output json\n  code-arena doctor"
)]
struct Cli {
    /// Answer once, print the answer and exit (`-` reads the prompt from stdin).
    #[arg(short = 'p', long = "print", value_name = "PROMPT")]
    print: Option<String>,
    /// A message to start the conversation in this terminal with.
    prompt: Option<String>,
    /// Carry on with this folder's last session.
    #[arg(short = 'c', long = "continue", global = true)]
    continue_last: bool,
    /// A new session that carries on the plan an earlier one of this folder left.
    #[arg(long = "plan", global = true)]
    carry_plan: bool,
    /// Goal mode: work until this goal is met (with -p: the run ends when it is met, stops or is out of budget).
    #[arg(long, value_name = "TEXT")]
    goal: Option<String>,
    /// Start the conversation in one of your chats in Arena: its number in /web's list, or its id.
    #[arg(long, value_name = "N|ID")]
    web: Option<String>,
    /// Take up a saved session: its id, or none to choose from a list.
    #[arg(long, num_args = 0..=1, default_missing_value = "", global = true)]
    resume: Option<String>,
    /// With --continue or --resume: carry on in a fork of that session (a new session with all of it), leaving it as
    /// it was.
    #[arg(long = "fork-session", global = true)]
    fork_session: bool,
    /// ask, auto-edit, plan or yolo.
    #[arg(long, global = true)]
    mode: Option<String>,
    #[arg(short = 'm', long, global = true)]
    model: Option<String>,
    /// default, off, low, medium, high or xhigh.
    #[arg(long, global = true)]
    thinking: Option<String>,
    #[arg(long, value_enum, default_value_t = Output::Text)]
    output: Output,
    /// Work in this folder.
    #[arg(short = 'C', long = "cd", value_name = "FOLDER", global = true)]
    cwd: Option<PathBuf>,
    /// A folder the tools may use besides the working directory (again for more).
    #[arg(long = "add-dir", value_name = "FOLDER", global = true)]
    add_dir: Vec<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Output {
    Text,
    Json,
    /// One JSON object a line, as the turn goes.
    StreamJson,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in to an Arena with your API key (or your user name and password), or keep a provider of your own
    /// (--provider): the standalone edition, with no Arena.
    Login {
        /// A provider instead of an Arena: anthropic, openai, deepseek, openrouter, mistral, ollama, or your own name
        /// with --url (any OpenAI-compatible API). The key is asked without showing it, or read from its variable.
        #[arg(long)]
        provider: Option<String>,
        /// openai or anthropic, for a provider of your own.
        #[arg(long)]
        kind: Option<String>,
        /// The Arena's address; with --provider, the provider's API (without /v1).
        #[arg(long)]
        url: Option<String>,
        /// Sign in with this user name and its password instead of a key.
        #[arg(long)]
        user: Option<String>,
        /// The gateway, when it is not at https://gateway.DOMAIN.
        #[arg(long)]
        gateway: Option<String>,
        /// A CA file to trust besides the system's (a company's own CA).
        #[arg(long)]
        ca: Option<PathBuf>,
    },
    /// Forget the API key (Arena revokes it when Code Arena made it), or with --provider a provider's.
    Logout {
        #[arg(long)]
        provider: Option<String>,
    },
    /// The models your key may use, with their windows.
    Models,
    /// What Code Arena sees: its folders, the Arena, and what that Arena has, feature by feature.
    Doctor,
    /// A conversation in this terminal.
    Chat {
        /// A message to start with.
        prompt: Option<String>,
    },
    /// Prompts Code Arena runs on its own, on a schedule: add NAME --every 2h [--at 09:00] [--mode M] PROMPT…, list,
    /// run NAME, remove NAME, due (what a timer of the system's calls), install, uninstall.
    Schedule {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// The IDE: a native window of its own; with --browser, served to a browser instead (on this machine only).
    Ide {
        /// Serve the IDE to a browser instead of opening its native window.
        #[arg(long)]
        browser: bool,
        /// With --browser: the port to listen on, on 127.0.0.1 (any free one when not given).
        #[arg(long)]
        port: Option<u16>,
        /// With --browser: print the address only; open no browser.
        #[arg(long)]
        no_open: bool,
    },
    /// Your agents in Arena (Agent Studio, 8.1 on): list, run NAME [TEXT…] [--field K=V]… (waits for its answer).
    Agents {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Your scheduled tasks in Arena (8.1 on): list, run NAME (now), runs NAME (how its last runs went).
    Tasks {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Plugins (commands, sub-agents, skills, hooks and MCP servers, in the common layout): list, install PATH|GIT_URL,
    /// enable NAME, disable NAME, remove NAME.
    Plugin {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// The version, the licence and the source.
    Version,
}

fn main() {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("code-arena could not start: {e}");
            std::process::exit(1);
        }
    };
    let code = runtime.block_on(async move {
        match go(cli).await {
            Ok(code) => code,
            Err(e) => {
                eprintln!("{}", format_error(&e));
                1
            }
        }
    });
    std::process::exit(code);
}

fn format_error(e: &anyhow::Error) -> String {
    let mut parts: Vec<String> = vec![e.to_string()];
    for cause in e.chain().skip(1) {
        let c = cause.to_string();
        if !parts.iter().any(|p| p.contains(&c)) {
            parts.push(c);
        }
    }
    parts.join(": ")
}

/// What every way of running shares.
struct Common {
    print: Option<String>,
    prompt: Option<String>,
    continue_last: bool,
    resume: Option<String>,
    fork_session: bool,
    mode: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    output: Output,
    cwd: Option<PathBuf>,
    add_dir: Vec<PathBuf>,
    carry_plan: bool,
    web: Option<String>,
    goal: Option<String>,
}

fn cli_opts(c: &Common) -> session::Opts {
    session::Opts {
        cwd: c.cwd.clone(),
        add_dirs: c.add_dir.clone(),
        mode: c.mode.clone(),
        model: c.model.clone(),
        thinking: c.thinking.clone(),
        interactive: c.print.is_none(),
        carry_plan: c.carry_plan,
    }
}

fn resume_of(c: &Common) -> session::Resume {
    let r = match (&c.resume, c.continue_last) {
        (Some(id), _) if !id.is_empty() => session::Resume::Id(id.clone()),
        (_, true) => session::Resume::Latest,
        _ => session::Resume::New,
    };
    // Chosen from the list (--resume alone): the choice is forked once made.
    let choosing = c.resume.as_deref() == Some("");
    match r {
        session::Resume::New if !choosing => r,
        r if c.fork_session => session::Resume::Fork(Box::new(r)),
        r => r,
    }
}

async fn chat(app: &mut app::App, opts: &session::Opts, first: Option<String>, c: &Common) -> anyhow::Result<i32> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        anyhow::bail!("the conversation needs a terminal: with a pipe, use -p (git diff | code-arena -p -)");
    }
    let args = chat::ChatArgs {
        opts: session::Opts { interactive: true, ..opts.clone() },
        resume: resume_of(c),
        first,
        choose: c.resume.as_deref() == Some(""),
        web: c.web.clone(),
    };
    chat::run(app, args).await
}

async fn go(cli: Cli) -> anyhow::Result<i32> {
    let mut app = app::App::load(None)?;
    let cli_common = Common {
        print: cli.print,
        prompt: cli.prompt,
        continue_last: cli.continue_last,
        resume: cli.resume,
        fork_session: cli.fork_session,
        mode: cli.mode,
        model: cli.model,
        thinking: cli.thinking,
        output: cli.output,
        cwd: cli.cwd,
        add_dir: cli.add_dir,
        carry_plan: cli.carry_plan,
        web: cli.web,
        goal: cli.goal,
    };
    match cli.command {
        Some(Command::Login { provider, kind, url, user, gateway, ca }) => {
            account::login(&app, account::LoginArgs { provider, kind, url, user, gateway, ca }).await.map(|()| 0)
        }
        Some(Command::Logout { provider: Some(p) }) => account::logout_provider(&app, &p).map(|()| 0),
        Some(Command::Logout { provider: None }) => account::logout(&app).await.map(|()| 0),
        Some(Command::Models) => account::models(&app).await.map(|()| 0),
        Some(Command::Doctor) => account::doctor(&app).await.map(|()| 0),
        Some(Command::Schedule { args }) => {
            let cwd = std::env::current_dir()?;
            // A timer of the system's goes in the system's own home, whatever CODE_ARENA_HOME says.
            schedules::command(&app.paths.config, dirs::home_dir().as_deref(), &cwd, &args)
        }
        Some(Command::Ide { browser, port, no_open }) => {
            let (opts, resume) = (cli_opts(&cli_common), resume_of(&cli_common));
            #[cfg(feature = "native")]
            if !browser && port.is_none() && !no_open {
                // The native window has the main thread (the system's windows want it); the session's driver runs on
                // the runtime's other threads, and their messages go between them in-process.
                let (to_driver, from_ui) = tokio::sync::mpsc::unbounded_channel();
                let (to_ui, _) = tokio::sync::broadcast::channel(4096);
                let recv = to_ui.subscribe();
                let args = ide::IdeArgs { opts, resume, face: ide::Face::Native { from_ui, to_ui } };
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build();
                    match rt {
                        Ok(rt) => {
                            if let Err(e) = rt.block_on(ide::run(&mut app, args)) {
                                eprintln!("{}", format_error(&e));
                            }
                        }
                        Err(e) => eprintln!("code-arena could not start the session: {e}"),
                    }
                });
                tokio::task::block_in_place(|| ca_native::launch(ca_native::Hub { send: to_driver, recv }));
                return Ok(0);
            }
            #[cfg(not(feature = "native"))]
            if !browser {
                eprintln!("This Code Arena was built without its native window: the IDE is served to a browser.");
            }
            let args =
                ide::IdeArgs { opts, resume, face: ide::Face::Browser { port: port.unwrap_or(0), open: !no_open } };
            ide::run(&mut app, args).await
        }
        Some(Command::Agents { args }) => automation::agents_cli(&app, &args).await,
        Some(Command::Tasks { args }) => automation::tasks_cli(&app, &args).await,
        Some(Command::Plugin { args }) => {
            let file = app.paths.settings_file();
            // Only your own list: a project's is its own.
            let yours = ca_settings::settings::Settings::load(&file, None, Default::default())
                .strings(&ca_settings::schema::PLUGINS);
            plugins::command(&app.paths.data, &file, &yours, &args, &app.agents())
        }
        Some(Command::Version) => {
            let edition = match app.edition() {
                ca_settings::Edition::None => String::new(),
                e => format!(", {} edition", e.name()),
            };
            println!("{} {}{edition} (MIT licence) {}", ca_core::PRODUCT, ca_core::VERSION, ca_core::SOURCE);
            Ok(0)
        }
        Some(Command::Chat { prompt }) => chat(&mut app, &cli_opts(&cli_common), prompt, &cli_common).await,
        None => match cli_common.print.clone() {
            Some(prompt) => {
                let output = match cli_common.output {
                    Output::Text => run::OutputFormat::Text,
                    Output::Json => run::OutputFormat::Json,
                    Output::StreamJson => run::OutputFormat::StreamJson,
                };
                let resume = resume_of(&cli_common);
                let o = cli_opts(&cli_common);
                run::run_once(
                    &mut app,
                    run::RunArgs {
                        prompt,
                        cwd: o.cwd,
                        add_dirs: o.add_dirs,
                        mode: o.mode,
                        model: o.model,
                        thinking: o.thinking,
                        output,
                        resume,
                        carry_plan: cli_common.carry_plan,
                        goal: cli_common.goal.clone(),
                    },
                )
                .await
            }
            None => {
                let first = cli_common.prompt.clone();
                chat(&mut app, &cli_opts(&cli_common), first, &cli_common).await
            }
        },
    }
}
