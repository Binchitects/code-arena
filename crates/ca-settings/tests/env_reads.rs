//! Nothing in Code Arena reads the environment but `ca-settings`: every variable is in the schema (or the
//! connection's list), so the Settings editor and `doctor` can show it. This test fails when code reads one
//! anywhere else.

use std::path::{Path, PathBuf};

/// Reads that are not settings, each with why.
const ALLOWED: &[(&str, &str)] = &[
    // The one place the process's environment is read, into a snapshot.
    ("ca-settings/src/env.rs", "std::env::var("),
    // `${NAME}` in the person's own MCP servers' settings: a variable they name there.
    ("ca-settings/src/env.rs", "match std::env::var(name)"),
    // Finding a program on PATH is the system's business, not a setting.
    ("ca-tools/src/shell.rs", "std::env::var_os(\"PATH\")"),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read dir").flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "target" && n != "templates") {
                sources(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn only_the_settings_read_the_environment() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("crates");
    let mut files = Vec::new();
    sources(crates, &mut files);
    assert!(files.len() > 20, "found the sources");
    let mut offenders = Vec::new();
    for f in files {
        let rel = f.strip_prefix(crates).expect("in").to_string_lossy().replace('\\', "/");
        if rel.ends_with("tests/env_reads.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).expect("read");
        for (n, line) in text.lines().enumerate() {
            let reads = ["env::var(", "env::var_os(", "env::vars("].iter().any(|r| line.contains(r));
            if reads && !ALLOWED.iter().any(|(file, what)| rel == *file && line.contains(what)) {
                offenders.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "read the environment through ca_settings::Env, and declare the variable:\n{}",
        offenders.join("\n")
    );
}
