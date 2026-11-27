//! `code-arena-ide`: the IDE started from the desktop (a shortcut, a file manager's "Open in Code Arena"),
//! where there is no terminal to type `code-arena ide` in. It runs the `code-arena` beside it with `ide` and
//! what it was given, and ends as that ends.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let Ok(me) = std::env::current_exe() else {
        eprintln!("code-arena-ide could not find where it is.");
        return ExitCode::FAILURE;
    };
    let program = me.with_file_name(if cfg!(windows) { "code-arena.exe" } else { "code-arena" });
    match Command::new(&program).arg("ide").args(std::env::args_os().skip(1)).status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("code-arena-ide could not start {}: {e}", program.display());
            ExitCode::FAILURE
        }
    }
}
