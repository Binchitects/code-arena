//! `code-arena`: Farzan Arena's coding agent.

mod account;
mod app;
mod chat;
mod run;
mod servers;
mod session;
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
    #[arg(short = 'c', long = "continue")]
    continue_last: bool,
    /// Take up a saved session: its id, or none to choose from a list.
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    resume: Option<String>,
    /// ask, auto-edit, plan or yolo.
    #[arg(long)]
    mode: Option<String>,
    #[arg(short = 'm', long)]
    model: Option<String>,
    /// default, off, low, medium, high or xhigh.
    #[arg(long)]
    thinking: Option<String>,
    #[arg(long, value_enum, default_value_t = Output::Text)]
    output: Output,
    /// Work in this folder.
    #[arg(short = 'C', long = "cd", value_name = "FOLDER")]
    cwd: Option<PathBuf>,
    /// A folder the tools may use besides the working directory (again for more).
    #[arg(long = "add-dir", value_name = "FOLDER")]
    add_dir: Vec<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Output {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in to an Arena with your API key (or your user name and password).
    Login {
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
    /// Forget the API key (Arena revokes it when Code Arena made it).
    Logout,
    /// The models your key may use, with their windows.
    Models,
    /// What Code Arena sees: its folders, the Arena, and what that Arena has, feature by feature.
    Doctor,
    /// A conversation in this terminal.
    Chat {
        /// A message to start with.
        prompt: Option<String>,
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
    mode: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    output: Output,
    cwd: Option<PathBuf>,
    add_dir: Vec<PathBuf>,
}

fn cli_opts(c: &Common) -> session::Opts {
    session::Opts {
        cwd: c.cwd.clone(),
        add_dirs: c.add_dir.clone(),
        mode: c.mode.clone(),
        model: c.model.clone(),
        thinking: c.thinking.clone(),
        interactive: c.print.is_none(),
    }
}

fn resume_of(c: &Common) -> session::Resume {
    match (&c.resume, c.continue_last) {
        (Some(id), _) if !id.is_empty() => session::Resume::Id(id.clone()),
        (_, true) => session::Resume::Latest,
        _ => session::Resume::New,
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
        mode: cli.mode,
        model: cli.model,
        thinking: cli.thinking,
        output: cli.output,
        cwd: cli.cwd,
        add_dir: cli.add_dir,
    };
    match cli.command {
        Some(Command::Login { url, user, gateway, ca }) => {
            account::login(&app, account::LoginArgs { url, user, gateway, ca }).await.map(|()| 0)
        }
        Some(Command::Logout) => account::logout(&app).await.map(|()| 0),
        Some(Command::Models) => account::models(&app).await.map(|()| 0),
        Some(Command::Doctor) => account::doctor(&app).await.map(|()| 0),
        Some(Command::Version) => {
            println!("{} {} (MIT licence) {}", ca_core::PRODUCT, ca_core::VERSION, ca_core::SOURCE);
            Ok(0)
        }
        Some(Command::Chat { prompt }) => chat(&mut app, &cli_opts(&cli_common), prompt, &cli_common).await,
        None => match cli_common.print.clone() {
            Some(prompt) => {
                let output = match cli_common.output {
                    Output::Text => run::OutputFormat::Text,
                    Output::Json => run::OutputFormat::Json,
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
