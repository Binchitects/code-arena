//! `code-arena`: Farzan Arena's coding agent.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "code-arena", version = ca_core::VERSION, about = "Farzan Arena's coding agent")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// The version, the licence and the source.
    Version,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Version) | None => {
            println!("{} {} (MIT licence) {}", ca_core::PRODUCT, ca_core::VERSION, ca_core::SOURCE);
        }
    }
    Ok(())
}
