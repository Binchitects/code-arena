//! `code-arena`: Farzan Arena's coding agent.

mod account;
mod app;
mod run;
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
    /// A prompt to start with (the terminal conversation comes in 0.2; for now it runs as -p).
    prompt: Option<String>,
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

async fn go(cli: Cli) -> anyhow::Result<i32> {
    let mut app = app::App::load(None)?;
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
        None => {
            let prompt = match (cli.print, cli.prompt) {
                (Some(p), _) => p,
                (None, Some(p)) => {
                    app.ui.say(&app.ui.dim("(The terminal conversation comes in 0.2: this runs once, as -p.)"));
                    p
                }
                (None, None) => {
                    println!(
                        "{} {}: the IDE comes in 0.4 and the terminal conversation in 0.2.",
                        ca_core::PRODUCT,
                        ca_core::VERSION
                    );
                    println!(
                        "For now: code-arena -p \"what to do\", code-arena login, code-arena doctor (code-arena --help for more)."
                    );
                    return Ok(0);
                }
            };
            let output = match cli.output {
                Output::Text => run::OutputFormat::Text,
                Output::Json => run::OutputFormat::Json,
            };
            run::run_once(
                &mut app,
                run::RunArgs {
                    prompt,
                    cwd: cli.cwd,
                    add_dirs: cli.add_dir,
                    mode: cli.mode,
                    model: cli.model,
                    thinking: cli.thinking,
                    output,
                },
            )
            .await
        }
    }
}
