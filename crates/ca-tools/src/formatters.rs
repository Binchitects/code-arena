//! The formatters a file may be formatted with (Prettier, Ruff, Black, gofmt, rustfmt, clang-format, dotnet
//! format, the person's own), which of them is here, and a text run through one.
//!
//! - [`discover`] lists a file's formatters in the order they are offered, each with where it was found, whether it
//!   can run and why not, and how to install it. [`versions`] asks each found one its version; [`Cache`] keeps a
//!   listing for ten seconds.
//! - [`plan`] says how a formatter is run on a text (its program, arguments, folder and time limit), and
//!   [`Runner::run`] runs it: in the sandbox (read-only, a `/tmp` of its own, no network), with none of the
//!   driver's environment but a short list (so no key), `NO_COLOR`, at most four at once, the text on standard
//!   input and the formatted text from standard output. dotnet format, which formats files only, is given a copy
//!   of the file in a folder of its own.
//! - A failure says the formatter's own words, and the line and column (1-based, as the formatter said them) of
//!   the fault when it named one.
//!
//! A range to format is given as 0-based lines and columns counted in characters; each formatter is given it in
//! its own units (Prettier's UTF-16 offsets, Ruff's 1-based line:column, Black's and clang-format's lines).

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::sandbox::Sandbox;

/// How long a listing is kept before the programs are looked for again.
pub const KEPT_FOR: Duration = Duration::from_secs(10);
/// How long a formatter may take.
pub const TIME_LIMIT: Duration = Duration::from_secs(10);
/// How long dotnet format may take (it starts the .NET SDK).
pub const DOTNET_TIME_LIMIT: Duration = Duration::from_secs(30);
/// How long `--version` may take.
const VERSION_TIME_LIMIT: Duration = Duration::from_secs(5);
/// How many formatters run at once.
pub const AT_ONCE: usize = 4;

/// Why nothing of the folder's runs while it is not trusted.
pub const UNTRUSTED: &str = "The folder is not trusted: its formatters do not run in Restricted Mode.";

/// A formatter Code Arena knows.
struct Known {
    id: &'static str,
    name: &'static str,
    /// The program's name.
    program: &'static str,
    languages: &'static [&'static str],
    range: bool,
    hint: &'static str,
}

const PRETTIER_LANGUAGES: &[&str] = &[
    "javascript",
    "javascriptreact",
    "typescript",
    "typescriptreact",
    "json",
    "jsonc",
    "json5",
    "css",
    "scss",
    "less",
    "html",
    "vue",
    "markdown",
    "mdx",
    "yaml",
    "graphql",
    "handlebars",
];

/// The formatters Code Arena knows, in the order a file's are offered.
const KNOWN: &[Known] = &[
    Known {
        id: "prettier",
        name: "Prettier",
        program: "prettier",
        languages: PRETTIER_LANGUAGES,
        range: true,
        hint: "npm install --save-dev prettier",
    },
    Known { id: "ruff", name: "Ruff", program: "ruff", languages: &["python"], range: true, hint: "pip install ruff" },
    Known {
        id: "black",
        name: "Black",
        program: "black",
        languages: &["python"],
        range: true,
        hint: "pip install black",
    },
    Known {
        id: "gofmt",
        name: "gofmt",
        program: "gofmt",
        languages: &["go"],
        range: false,
        hint: "Install Go: https://go.dev/dl/",
    },
    Known {
        id: "rustfmt",
        name: "rustfmt",
        program: "rustfmt",
        languages: &["rust"],
        range: false,
        hint: "rustup component add rustfmt",
    },
    Known {
        id: "clang-format",
        name: "clang-format",
        program: "clang-format",
        languages: &["c", "cpp", "objective-c", "objective-cpp", "cuda-cpp", "proto", "java"],
        range: true,
        hint: "Install clang-format: apt install clang-format, brew install clang-format",
    },
    Known {
        id: "dotnet-format",
        name: "dotnet format",
        program: "dotnet",
        languages: &["csharp", "vb"],
        range: false,
        hint: "Install the .NET SDK: https://dotnet.microsoft.com/download",
    },
];

/// One of a file's formatters, as the window lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Formatter {
    /// `prettier`, `ruff`, …, or the key of one of the person's own.
    pub id: String,
    pub name: String,
    /// Where its program was found: `project` (the folder's `node_modules`, a virtual environment), `global` (the
    /// PATH) or `settings` (one of the person's own `formatters`); `None` when it was not found.
    pub source: Option<String>,
    /// Whether it formats a part of a file.
    pub range: bool,
    pub version: Option<String>,
    pub available: bool,
    /// Why it cannot run, when it cannot.
    pub reason: Option<String>,
    /// How to install it, when it is not found.
    pub hint: Option<String>,
    /// Its program.
    pub path: Option<String>,
    #[serde(skip)]
    program: Option<PathBuf>,
    #[serde(skip)]
    custom: Option<Custom>,
}

/// One of the person's own formatters (`formatters` in settings.json): `{"id": {"command": "yapf", "args":
/// ["--style", "pep8"], "languages": ["python"]}}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Custom {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// Added to `args` to format a part (`${startLine}`, `${endLine}`, …); none: it formats whole files only.
    pub range_args: Vec<String>,
    pub languages: Vec<String>,
    /// File endings it formats too (`.py`), whatever the language.
    pub extensions: Vec<String>,
    pub timeout: Duration,
}

impl Custom {
    /// One of the person's formatters as settings.json gives it; `None` when it has no command.
    pub fn of(id: &str, v: &Value) -> Option<Custom> {
        let strings = |k: &str| -> Vec<String> {
            v[k].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()
        };
        let command = v["command"].as_str().filter(|c| !c.trim().is_empty())?.to_string();
        Some(Custom {
            name: v["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
            command,
            args: strings("args"),
            range_args: strings("rangeArgs"),
            languages: strings("languages"),
            extensions: strings("extensions").into_iter().map(|e| e.to_ascii_lowercase()).collect(),
            timeout: v["timeout"]
                .as_f64()
                .filter(|s| *s > 0.0)
                .map(|s| Duration::from_secs_f64(s.min(60.0)))
                .unwrap_or(TIME_LIMIT),
        })
    }

    fn formats(&self, language: &str, file: &Path) -> bool {
        let ext = file.extension().map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()));
        self.languages.iter().any(|l| l == language) || ext.is_some_and(|e| self.extensions.contains(&e))
    }
}

/// Where a file's formatters are looked for.
pub struct Look<'a> {
    pub root: &'a Path,
    pub file: &'a Path,
    /// VS Code's language id; empty: the file's ending says.
    pub language: &'a str,
    /// `formatters` of settings.json.
    pub custom: &'a Value,
    /// The PATH the programs are looked for on.
    pub path_var: Option<OsString>,
    pub trusted: bool,
}

/// The formatters a file may be formatted with, in the order they are offered: Code Arena's own for its language,
/// then the person's own. Nothing is run (see [`versions`]).
pub fn discover(look: &Look) -> Vec<Formatter> {
    let language = if look.language.is_empty() { language_of(look.file) } else { look.language };
    let dir = look.file.parent().unwrap_or(look.root);
    let mut out = Vec::new();
    for k in KNOWN.iter().filter(|k| k.languages.contains(&language)) {
        let found = match k.id {
            "prettier" => node_bin(dir, k.program).map(|p| (p, "project")),
            "ruff" | "black" => venv_bin(dir, look.root, k.program).map(|p| (p, "project")),
            _ => None,
        }
        .or_else(|| on_path(k.program, look.path_var.as_ref()).map(|p| (p, "global")));
        let mut f = Formatter {
            id: k.id.into(),
            name: k.name.into(),
            source: found.as_ref().map(|(_, s)| s.to_string()),
            range: k.range,
            version: None,
            available: found.is_some(),
            reason: None,
            hint: None,
            path: found.as_ref().map(|(p, _)| p.display().to_string()),
            program: found.map(|(p, _)| p),
            custom: None,
        };
        if f.program.is_none() {
            f.reason = Some(match k.id {
                "prettier" => "Prettier is not in the folder's node_modules or on the PATH.".into(),
                "ruff" | "black" => format!("{} is not in the folder's virtual environment or on the PATH.", k.name),
                _ => format!("{} is not on the PATH.", k.program),
            });
            f.hint = Some(k.hint.into());
        }
        out.push(f);
    }
    if let Some(all) = look.custom.as_object() {
        for (id, v) in all {
            let Some(c) = Custom::of(id, v).filter(|c| c.formats(language, look.file)) else {
                continue;
            };
            let command = c.command.replace("${workspaceFolder}", &look.root.display().to_string());
            let program = if command.contains('/') || command.contains('\\') {
                let p = look.root.join(&command);
                p.is_file().then_some(p)
            } else {
                on_path(&command, look.path_var.as_ref())
            };
            let f = Formatter {
                id: id.clone(),
                name: c.name.clone(),
                source: Some("settings".into()),
                range: !c.range_args.is_empty(),
                version: None,
                available: program.is_some(),
                reason: program.is_none().then(|| format!("{command} is not found.")),
                hint: None,
                path: program.as_ref().map(|p| p.display().to_string()),
                program,
                custom: Some(c),
            };
            // One of the person's own named as one of Code Arena's takes its place.
            match out.iter().position(|o| o.id == *id) {
                Some(i) => out[i] = f,
                None => out.push(f),
            }
        }
    }
    if !look.trusted {
        for f in &mut out {
            if f.available {
                f.available = false;
                f.reason = Some(UNTRUSTED.into());
            }
        }
    }
    out
}

/// Asks each formatter that can run its version, a few at once; one whose `--version` fails cannot run.
pub fn versions(items: &mut [Formatter], runner: &Runner, root: &Path) {
    let asked: Vec<(usize, PathBuf, Vec<String>)> = items
        .iter()
        .enumerate()
        .filter(|(_, f)| f.available && f.custom.is_none() && f.id != "gofmt")
        .filter_map(|(i, f)| f.program.clone().map(|p| (i, p, vec!["--version".to_string()])))
        .collect();
    let answers: Vec<(usize, Result<String, Failure>)> = std::thread::scope(|s| {
        let handles: Vec<_> = asked
            .iter()
            .map(|(i, p, args)| {
                let plan = Plan {
                    name: items[*i].name.clone(),
                    program: p.clone(),
                    args: args.clone(),
                    cwd: root.to_path_buf(),
                    timeout: VERSION_TIME_LIMIT,
                    mirror: None,
                    readable: vec![root.to_path_buf(), p.clone()],
                };
                let i = *i;
                s.spawn(move || (i, runner.run(&plan, "")))
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    for (i, got) in answers {
        match got {
            Ok(text) => items[i].version = version_in(&text),
            Err(e) => {
                items[i].available = false;
                items[i].reason = Some(format!("{} --version failed: {}", items[i].name, e.message));
            }
        }
    }
}

/// The version a program's `--version` says (`black, 24.1.0 (compiled: yes)` is 24.1.0).
pub fn version_in(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\d+\.\d+(?:\.\d+)?").expect("version pattern"));
    re.find(text).map(|m| m.as_str().to_string())
}

/// VS Code's extension ids as Code Arena's formatters (`esbenp.prettier-vscode` is `prettier`); any other id as it
/// is.
pub fn canonical(id: &str) -> String {
    let id = id.trim();
    match id.to_ascii_lowercase().as_str() {
        "esbenp.prettier-vscode" | "prettier.prettier-vscode" => "prettier".into(),
        "charliermarsh.ruff" => "ruff".into(),
        "ms-python.black-formatter" => "black".into(),
        "golang.go" => "gofmt".into(),
        "rust-lang.rust-analyzer" => "rustfmt".into(),
        "xaver.clang-format" | "ms-vscode.cpptools" | "llvm-vs-code-extensions.vscode-clangd" => "clang-format".into(),
        "ms-dotnettools.csharp" | "ms-dotnettools.csdevkit" | "dotnet format" => "dotnet-format".into(),
        _ => id.to_string(),
    }
}

/// The formatter a file is formatted with: the one `editor.defaultFormatter` names, when the file has it (even
/// when it cannot run, so the window says so), else the first that can run.
pub fn default_of(items: &[Formatter], configured: &str) -> Option<String> {
    let wanted = canonical(configured);
    items
        .iter()
        .find(|f| !wanted.is_empty() && f.id == wanted)
        .or_else(|| items.iter().find(|f| f.available))
        .map(|f| f.id.clone())
}

/// VS Code's language id for a file by its ending, for a face that gave none.
pub fn language_of(file: &Path) -> &'static str {
    let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "json" => "json",
        "jsonc" => "jsonc",
        "json5" => "json5",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "html" | "htm" => "html",
        "vue" => "vue",
        "md" | "markdown" => "markdown",
        "mdx" => "mdx",
        "yaml" | "yml" => "yaml",
        "graphql" | "gql" => "graphql",
        "hbs" | "handlebars" => "handlebars",
        "py" | "pyi" => "python",
        "go" => "go",
        "rs" => "rust",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        "cu" | "cuh" => "cuda-cpp",
        "proto" => "proto",
        "java" => "java",
        "cs" => "csharp",
        "vb" => "vb",
        _ => "plaintext",
    }
}

/// A program on the PATH.
pub fn on_path(name: &str, path_var: Option<&OsString>) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.cmd"), format!("{name}.bat"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    std::env::split_paths(path_var?)
        .filter(|d| d.is_absolute())
        .flat_map(|d| names.iter().map(move |n| d.join(n)).collect::<Vec<_>>())
        .find(|p| p.is_file())
}

/// The nearest `node_modules/.bin/NAME` from a folder up.
fn node_bin(dir: &Path, name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) { format!("{name}.cmd") } else { name.to_string() };
    dir.ancestors().map(|d| d.join("node_modules").join(".bin").join(&file)).find(|p| p.is_file())
}

/// `NAME` in the nearest virtual environment (`.venv`, `venv`) from a folder up to the root.
fn venv_bin(dir: &Path, root: &Path, name: &str) -> Option<PathBuf> {
    let (bin, file) = if cfg!(windows) { ("Scripts", format!("{name}.exe")) } else { ("bin", name.to_string()) };
    for d in dir.ancestors() {
        for venv in [".venv", "venv"] {
            let p = d.join(venv).join(bin).join(&file);
            if p.is_file() {
                return Some(p);
            }
        }
        if d == root || !d.starts_with(root) {
            break;
        }
    }
    None
}

/// The edition rustfmt formats a file as: its package's (Cargo.toml's `edition`, the workspace's for
/// `edition.workspace = true`), 2015 for a package that says none (Cargo's own), 2021 for a file in no package.
pub fn edition(file: &Path) -> String {
    let mut from_workspace = false;
    for dir in file.parent().into_iter().flat_map(Path::ancestors) {
        let Ok(text) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        if from_workspace {
            if let Some(e) = toml_value(&text, "workspace.package", "edition") {
                return e;
            }
            continue;
        }
        if text.lines().any(|l| l.trim() == "[package]") {
            match toml_value(&text, "package", "edition") {
                Some(e) if e == "workspace" => from_workspace = true,
                Some(e) => return e,
                None => return "2015".into(),
            }
        } else if let Some(e) = toml_value(&text, "workspace.package", "edition") {
            return e;
        }
    }
    "2021".into()
}

/// A key's string in a section of a Cargo.toml, read line by line; `workspace` for `key.workspace = true` and
/// `key = { workspace = true }`.
fn toml_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut here = false;
    for line in text.lines() {
        let l = line.split('#').next().unwrap_or("").trim();
        if l.starts_with('[') {
            here = l.trim_start_matches('[').trim_end_matches(']').trim() == section;
            continue;
        }
        if !here {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k == format!("{key}.workspace") || (k == key && v.starts_with('{') && v.contains("workspace")) {
            return Some("workspace".into());
        }
        if k == key {
            return Some(v.trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    None
}

/// Whether Prettier finds a configuration of the folder's for a file (then the `prettier.*` settings are not
/// given: as in VS Code, the folder's own wins).
pub fn prettier_config(file: &Path, root: &Path) -> bool {
    const FILES: &[&str] = &[
        ".prettierrc",
        ".prettierrc.json",
        ".prettierrc.yaml",
        ".prettierrc.yml",
        ".prettierrc.json5",
        ".prettierrc.js",
        ".prettierrc.cjs",
        ".prettierrc.mjs",
        ".prettierrc.ts",
        ".prettierrc.toml",
        "prettier.config.js",
        "prettier.config.cjs",
        "prettier.config.mjs",
        "prettier.config.ts",
    ];
    for d in file.parent().into_iter().flat_map(Path::ancestors) {
        if FILES.iter().any(|f| d.join(f).is_file()) {
            return true;
        }
        let package = std::fs::read_to_string(d.join("package.json")).ok();
        if package.and_then(|t| serde_json::from_str::<Value>(&t).ok()).is_some_and(|v| v.get("prettier").is_some()) {
            return true;
        }
        if d == root || !d.starts_with(root) {
            break;
        }
    }
    false
}

/// The `prettier.*` settings, as VS Code's Prettier has them.
#[derive(Debug, Clone, PartialEq)]
pub struct PrettierSettings {
    pub print_width: i64,
    pub tab_width: i64,
    pub use_tabs: bool,
    pub semi: bool,
    pub single_quote: bool,
    pub trailing_comma: String,
    pub bracket_spacing: bool,
    pub arrow_parens: String,
    pub end_of_line: String,
}

impl PrettierSettings {
    /// Prettier's flags for what differs from its own defaults.
    pub fn flags(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut give = |k: &str, v: String| out.extend([k.to_string(), v]);
        if self.print_width != 80 {
            give("--print-width", self.print_width.to_string());
        }
        if self.tab_width != 2 {
            give("--tab-width", self.tab_width.to_string());
        }
        if self.trailing_comma != "all" && !self.trailing_comma.is_empty() {
            give("--trailing-comma", self.trailing_comma.clone());
        }
        if self.arrow_parens != "always" && !self.arrow_parens.is_empty() {
            give("--arrow-parens", self.arrow_parens.clone());
        }
        if self.end_of_line != "lf" && !self.end_of_line.is_empty() {
            give("--end-of-line", self.end_of_line.clone());
        }
        if self.use_tabs {
            out.push("--use-tabs".into());
        }
        if !self.semi {
            out.push("--no-semi".into());
        }
        if self.single_quote {
            out.push("--single-quote".into());
        }
        if !self.bracket_spacing {
            out.push("--no-bracket-spacing".into());
        }
        out
    }
}

/// A part of a text: 0-based lines, columns in characters, the end not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

impl Span {
    /// A range as the window sends it: `{"start": {"line", "character"}, "end": {…}}`.
    pub fn of(v: &Value) -> Option<Span> {
        let at = |p: &Value| Some((p["line"].as_u64()? as usize, p["character"].as_u64()? as usize));
        let (start, end) = (at(&v["start"])?, at(&v["end"])?);
        Some(if end < start { Span { start: end, end: start } } else { Span { start, end } })
    }

    /// The 1-based lines it covers, the last one not counted when the span ends at its start.
    fn lines(&self) -> (usize, usize) {
        let last = if self.end.1 == 0 && self.end.0 > self.start.0 { self.end.0 - 1 } else { self.end.0 };
        (self.start.0 + 1, last + 1)
    }
}

/// Where a line and column is in a text, in UTF-16 code units (as JavaScript counts) or in characters.
pub fn offset(text: &str, at: (usize, usize), utf16: bool) -> usize {
    let mut n = 0;
    for (i, line) in text.split('\n').enumerate() {
        let width = |s: &str| if utf16 { s.encode_utf16().count() } else { s.chars().count() };
        if i == at.0 {
            let part: String = line.chars().take(at.1).collect();
            return n + width(&part);
        }
        n += width(line) + 1;
    }
    n.saturating_sub(1)
}

/// What is formatted, and how.
#[derive(Debug, Clone, Default)]
pub struct Job {
    pub text: String,
    pub range: Option<Span>,
    pub tab_size: u32,
    pub insert_spaces: bool,
    /// Prettier's flags from the `prettier.*` settings (none when the folder has a configuration of its own).
    pub prettier: Vec<String>,
}

/// A formatter as it is run.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub timeout: Duration,
    /// The file the formatter formats in place, in a folder of its own (dotnet format): written with the text
    /// first, read back after, the folder removed.
    pub mirror: Option<(PathBuf, PathBuf)>,
    /// What it reads that may be under `/tmp` (the folder, the program).
    pub readable: Vec<PathBuf>,
}

/// Why a text was not formatted.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Failure {
    pub message: String,
    /// The fault's place, 1-based, when the formatter named one.
    pub line: Option<u32>,
    pub col: Option<u32>,
    /// `failed`, `timeout`, `no_range`, `not_found`.
    pub kind: &'static str,
}

impl Failure {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Failure {
        Failure { message: message.into(), line: None, col: None, kind }
    }
}

/// How a formatter formats a text of a file.
pub fn plan(f: &Formatter, job: &Job, root: &Path, file: &Path) -> Result<Plan, Failure> {
    let Some(program) = f.program.clone().filter(|_| f.available) else {
        return Err(Failure::new("not_found", f.reason.clone().unwrap_or_else(|| format!("{} cannot run.", f.name))));
    };
    if job.range.is_some() && !f.range {
        return Err(Failure::new("no_range", format!("{} formats whole files only, not a selection.", f.name)));
    }
    let dir = file.parent().filter(|d| d.is_dir()).unwrap_or(root).to_path_buf();
    let path = file.display().to_string();
    let lines = job.range.map(|r| r.lines());
    let mut plan = Plan {
        name: f.name.clone(),
        program: program.clone(),
        args: Vec::new(),
        cwd: dir.clone(),
        timeout: TIME_LIMIT,
        mirror: None,
        readable: vec![root.to_path_buf(), dir, program],
    };
    let a = &mut plan.args;
    match f.custom.as_ref() {
        Some(c) => {
            let fill = |s: &str| placeholders(s, root, file, job);
            a.extend(c.args.iter().map(|s| fill(s)));
            if job.range.is_some() {
                a.extend(c.range_args.iter().map(|s| fill(s)));
            }
            plan.timeout = c.timeout;
        }
        None => match f.id.as_str() {
            "prettier" => {
                a.extend(["--stdin-filepath".into(), path, "--no-color".into()]);
                a.extend(job.prettier.iter().cloned());
                if let Some(r) = job.range {
                    a.extend([
                        "--range-start".into(),
                        offset(&job.text, r.start, true).to_string(),
                        "--range-end".into(),
                        offset(&job.text, r.end, true).to_string(),
                    ]);
                }
            }
            "ruff" => {
                a.extend(["format".into(), "--quiet".into(), "--stdin-filename".into(), path]);
                if let Some(r) = job.range {
                    a.push(format!("--range={}:{}-{}:{}", r.start.0 + 1, r.start.1 + 1, r.end.0 + 1, r.end.1 + 1));
                }
                a.push("-".into());
            }
            "black" => {
                a.extend(["--quiet".into(), "--stdin-filename".into(), path]);
                if let Some((from, to)) = lines {
                    a.push(format!("--line-ranges={from}-{to}"));
                }
                a.push("-".into());
            }
            "gofmt" => {}
            "rustfmt" => a.extend(["--edition".into(), edition(file), "--color".into(), "never".into()]),
            "clang-format" => {
                a.push(format!("--assume-filename={path}"));
                if let Some((from, to)) = lines {
                    a.push(format!("--lines={from}:{to}"));
                }
            }
            "dotnet-format" => {
                let folder = mirror_folder();
                let rel = file.strip_prefix(root).ok().filter(|r| r.components().count() > 0).map(Path::to_path_buf);
                let rel = rel.unwrap_or_else(|| PathBuf::from(file.file_name().unwrap_or_else(|| "file.cs".as_ref())));
                a.extend([
                    "format".into(),
                    "whitespace".into(),
                    folder.display().to_string(),
                    "--folder".into(),
                    "--include".into(),
                    rel.display().to_string(),
                ]);
                plan.timeout = DOTNET_TIME_LIMIT;
                plan.cwd = folder.clone();
                plan.mirror = Some((folder, rel));
            }
            other => return Err(Failure::new("not_found", format!("No formatter is named {other}."))),
        },
    }
    Ok(plan)
}

/// `${file}`, `${fileBasename}`, `${fileBasenameNoExtension}`, `${fileExtname}`, `${fileDirname}`,
/// `${relativeFile}`, `${workspaceFolder}`, `${tabSize}`, `${insertSpaces}`, and for a range `${startLine}`,
/// `${endLine}` (1-based) and `${startOffset}`, `${endOffset}` (in characters), in one of the person's own
/// formatter's arguments.
pub fn placeholders(arg: &str, root: &Path, file: &Path, job: &Job) -> String {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = file.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = file.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let dir = file.parent().map(|d| d.display().to_string()).unwrap_or_default();
    let rel = file.strip_prefix(root).map(|r| r.display().to_string()).unwrap_or_else(|_| file.display().to_string());
    let (lines, offsets) = match job.range {
        Some(r) => (r.lines(), (offset(&job.text, r.start, false), offset(&job.text, r.end, false))),
        None => ((1, job.text.lines().count().max(1)), (0, job.text.chars().count())),
    };
    let mut out = arg.to_string();
    for (k, v) in [
        ("${fileBasenameNoExtension}", stem),
        ("${fileBasename}", name),
        ("${fileExtname}", ext),
        ("${fileDirname}", dir),
        ("${relativeFile}", rel),
        ("${workspaceFolder}", root.display().to_string()),
        ("${file}", file.display().to_string()),
        ("${tabSize}", job.tab_size.to_string()),
        ("${insertSpaces}", job.insert_spaces.to_string()),
        ("${startLine}", lines.0.to_string()),
        ("${endLine}", lines.1.to_string()),
        ("${startOffset}", offsets.0.to_string()),
        ("${endOffset}", offsets.1.to_string()),
    ] {
        out = out.replace(k, &v);
    }
    out
}

/// A new folder of its own under the system's temporary folder.
fn mirror_folder() -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("code-arena-format-{}-{n}-{t}", std::process::id()))
}

/// The environment's names a formatter is given; nothing else of the driver's (its keys) reaches it.
const PASSED: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SYSTEMROOT",
    "WINDIR",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PATHEXT",
    "COMSPEC",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "NODE_PATH",
    "NVM_BIN",
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
    "GOPATH",
    "GOROOT",
    "GOCACHE",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "DOTNET_ROOT",
    "DOTNET_CLI_HOME",
];

/// What of an environment a formatter is given.
pub fn passed_env(vars: impl Iterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)> {
    vars.filter(|(k, _)| PASSED.iter().any(|p| k.to_str() == Some(*p))).collect()
}

/// At most [`AT_ONCE`] formatters running.
#[derive(Default)]
pub struct Gate {
    running: Mutex<usize>,
    freed: Condvar,
}

/// A place in the [`Gate`], given back when dropped.
pub struct Pass<'a>(&'a Gate);

impl Gate {
    pub fn enter(&self) -> Pass<'_> {
        let mut n = self.running.lock().unwrap_or_else(|e| e.into_inner());
        while *n >= AT_ONCE {
            n = self.freed.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n += 1;
        Pass(self)
    }
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        *self.0.running.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        self.0.freed.notify_one();
    }
}

/// The listings asked lately, by what they were asked for.
#[derive(Default)]
pub struct Cache(Mutex<HashMap<String, (Instant, Vec<Formatter>)>>);

impl Cache {
    /// The listing kept for a key, made again by `make` when it is older than [`KEPT_FOR`].
    pub fn get_or(&self, key: &str, make: impl FnOnce() -> Vec<Formatter>) -> Vec<Formatter> {
        {
            let all = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, items)) = all.get(key).filter(|(at, _)| at.elapsed() < KEPT_FOR) {
                return items.clone();
            }
        }
        let items = make();
        let mut all = self.0.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|_, (at, _)| at.elapsed() < KEPT_FOR);
        all.insert(key.to_string(), (Instant::now(), items.clone()));
        items
    }
}

/// What runs formatters: the sandbox they run in (none: as the driver), the environment they are given, the gate.
pub struct Runner<'a> {
    pub sandbox: Option<Sandbox>,
    pub env: Vec<(OsString, OsString)>,
    pub gate: &'a Gate,
}

impl Runner<'_> {
    /// A text through a formatter: what it wrote, or why not.
    pub fn run(&self, plan: &Plan, text: &str) -> Result<String, Failure> {
        let _pass = self.gate.enter();
        if let Some((folder, rel)) = &plan.mirror {
            let file = folder.join(rel);
            let made =
                file.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())).and_then(|_| std::fs::write(&file, text));
            if let Err(e) = made {
                let _ = std::fs::remove_dir_all(folder);
                return Err(Failure::new(
                    "failed",
                    format!("{}: the file could not be copied to format: {e}", plan.name),
                ));
            }
            let got = self.spawn(plan, "", Some(folder));
            let back = got.and_then(|_| {
                std::fs::read_to_string(&file).map_err(|e| Failure::new("failed", format!("{}: {e}", plan.name)))
            });
            let _ = std::fs::remove_dir_all(folder);
            return back;
        }
        self.spawn(plan, text, None)
    }

    fn spawn(&self, plan: &Plan, text: &str, writable: Option<&PathBuf>) -> Result<String, Failure> {
        let program = plan.program.display().to_string();
        let (program, args) = match &self.sandbox {
            Some(s) => {
                let mut s = s.clone();
                s.network = false;
                s.writable.extend(writable.cloned());
                s.wrap_reading(&program, &plan.args, &plan.cwd, &plan.readable)
            }
            None => (program, plan.args.clone()),
        };
        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .current_dir(&plan.cwd)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .env("NO_COLOR", "1")
            .env("FORCE_COLOR", "0")
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
            .env("DOTNET_NOLOGO", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child =
            cmd.spawn().map_err(|e| Failure::new("failed", format!("{} could not be started: {e}", plan.name)))?;
        let stdin = child.stdin.take();
        let input = text.to_string();
        // Written and read at once: a large file would fill a pipe and stop both ends.
        let writer = std::thread::spawn(move || {
            if let Some(mut s) = stdin {
                let _ = s.write_all(input.as_bytes());
            }
        });
        let read = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut out = Vec::new();
                if let Some(mut p) = pipe {
                    let _ = p.read_to_end(&mut out);
                }
                out
            })
        };
        let out = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let err = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let deadline = Instant::now() + plan.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() >= deadline => break None,
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break None,
            }
        };
        let Some(status) = status else {
            #[cfg(unix)]
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(child.id() as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
            let _ = child.kill();
            let _ = child.wait();
            let _ = writer.join();
            return Err(Failure::new(
                "timeout",
                format!("{} took longer than {} s and was stopped.", plan.name, plan.timeout.as_secs()),
            ));
        };
        let _ = writer.join();
        let out = out.join().unwrap_or_default();
        let err = err.join().unwrap_or_default();
        if status.success() {
            return String::from_utf8(out)
                .map_err(|_| Failure::new("failed", format!("{} wrote what is not UTF-8 text.", plan.name)));
        }
        let said = String::from_utf8_lossy(if err.iter().any(|b| !b.is_ascii_whitespace()) { &err } else { &out });
        Err(failure(&plan.name, &said, status.code()))
    }
}

/// What a formatter that failed said: its first line of words, and the place it named.
pub fn failure(name: &str, said: &str, code: Option<i32>) -> Failure {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    let ansi = ANSI.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").expect("ansi pattern"));
    let said = ansi.replace_all(said, "");
    let first = said
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.trim_start_matches("[error]").trim().to_string())
        .unwrap_or_default();
    let mut message = if first.is_empty() {
        match code {
            Some(c) => format!("{name} stopped with exit code {c}."),
            None => format!("{name} was stopped."),
        }
    } else {
        format!("{name}: {first}")
    };
    if message.chars().count() > 300 {
        message = message.chars().take(299).collect::<String>() + "…";
    }
    let (line, col) = place(&said).map_or((None, None), |(l, c)| (Some(l), Some(c)));
    Failure { message, line, col, kind: "failed" }
}

/// The line and column (1-based) a formatter's words name: Prettier's `(3:5)`, Black's `Cannot parse: 3:5`,
/// and `NAME:3:5` of gofmt, Ruff and rustfmt.
pub fn place(said: &str) -> Option<(u32, u32)> {
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        [r"\((\d+):(\d+)\)", r"Cannot parse[^:\n]*: (\d+):(\d+)", r":(\d+):(\d+)"]
            .iter()
            .map(|p| Regex::new(p).expect("place pattern"))
            .collect()
    });
    res.iter().find_map(|re| {
        let c = re.captures(said)?;
        Some((c[1].parse().ok()?, c[2].parse().ok()?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn look<'a>(root: &'a Path, file: &'a Path, language: &'a str, custom: &'a Value, bin: &Path) -> Look<'a> {
        Look { root, file, language, custom, path_var: Some(bin.as_os_str().to_owned()), trusted: true }
    }

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, "").expect("write");
    }

    #[cfg(unix)]
    #[test]
    fn a_files_formatters_are_found_in_the_project_first_then_on_the_path() {
        let d = tempfile::tempdir().expect("temp");
        let root = d.path().join("work");
        let bin = d.path().join("bin");
        touch(&root.join("node_modules/.bin/prettier"));
        touch(&root.join(".venv/bin/black"));
        touch(&bin.join("ruff"));
        touch(&bin.join("prettier"));
        let none = json!({});
        let file = root.join("web/app.ts");
        let items = discover(&look(&root, &file, "typescript", &none, &bin));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "prettier");
        assert_eq!(items[0].source.as_deref(), Some("project"));
        assert!(items[0].path.as_deref().is_some_and(|p| p.ends_with("work/node_modules/.bin/prettier")));

        let py = root.join("src/main.py");
        let items = discover(&look(&root, &py, "", &none, &bin));
        let ids: Vec<_> = items.iter().map(|f| (f.id.as_str(), f.source.as_deref())).collect();
        assert_eq!(ids, [("ruff", Some("global")), ("black", Some("project"))]);

        let go = root.join("main.go");
        let items = discover(&look(&root, &go, "go", &none, &bin));
        assert_eq!(items[0].id, "gofmt");
        assert!(!items[0].available);
        assert_eq!(items[0].reason.as_deref(), Some("gofmt is not on the PATH."));
        assert_eq!(items[0].hint.as_deref(), Some("Install Go: https://go.dev/dl/"));
        assert!(discover(&look(&root, &root.join("notes.txt"), "", &none, &bin)).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_persons_own_formatters_join_by_language_or_ending_and_an_untrusted_folder_runs_none() {
        let d = tempfile::tempdir().expect("temp");
        let root = d.path().join("work");
        let bin = d.path().join("bin");
        touch(&bin.join("yapf"));
        touch(&bin.join("ruff"));
        let custom = json!({
            "yapf": {"command": "yapf", "args": ["--style", "${tabSize}"], "languages": ["python"]},
            "upper": {"name": "Upper", "command": "tools/upper.sh", "extensions": [".txt"], "rangeArgs": ["${startLine}"]},
            "broken": {"args": []},
        });
        let py = root.join("a.py");
        let items = discover(&look(&root, &py, "python", &custom, &bin));
        let ids: Vec<_> = items.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["ruff", "black", "yapf"]);
        assert_eq!(items[2].source.as_deref(), Some("settings"));
        assert!(items[2].available && !items[2].range);

        let txt = root.join("notes.txt");
        let items = discover(&look(&root, &txt, "plaintext", &custom, &bin));
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].name.as_str(), items[0].available, items[0].range), ("Upper", false, true));
        touch(&root.join("tools/upper.sh"));
        assert!(discover(&look(&root, &txt, "plaintext", &custom, &bin))[0].available);

        let mut l = look(&root, &py, "python", &custom, &bin);
        l.trusted = false;
        let items = discover(&l);
        assert!(items.iter().all(|f| !f.available));
        assert_eq!(items[0].reason.as_deref(), Some(UNTRUSTED));
        // What is not found says so, trusted or not.
        assert_eq!(
            items[1].reason.as_deref(),
            Some("Black is not in the folder's virtual environment or on the PATH.")
        );
    }

    fn found(id: &str, available: bool) -> Formatter {
        Formatter {
            id: id.into(),
            name: id.into(),
            source: None,
            range: false,
            version: None,
            available,
            reason: None,
            hint: None,
            path: None,
            program: Some(PathBuf::from("/bin/true")),
            custom: None,
        }
    }

    #[test]
    fn the_default_is_the_one_named_even_by_vs_codes_id_else_the_first_that_runs() {
        let items = [found("ruff", false), found("black", true)];
        assert_eq!(default_of(&items, ""), Some("black".into()));
        assert_eq!(default_of(&items, "charliermarsh.ruff"), Some("ruff".into()));
        assert_eq!(default_of(&items, "esbenp.prettier-vscode"), Some("black".into()));
        assert_eq!(default_of(&[found("ruff", false)], ""), None);
        assert_eq!(canonical("ms-python.black-formatter"), "black");
        assert_eq!(canonical("mine"), "mine");
    }

    #[test]
    fn rustfmt_takes_the_packages_edition_or_the_workspaces() {
        let d = tempfile::tempdir().expect("temp");
        let root = d.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", \"b\", \"c\"]\n\n[workspace.package]\nedition = \"2024\" # newest\n",
        )
        .expect("write");
        std::fs::create_dir_all(root.join("a/src")).expect("mkdir");
        std::fs::write(root.join("a/Cargo.toml"), "[package]\nname = \"a\"\nedition = \"2018\"\n").expect("write");
        std::fs::create_dir_all(root.join("b/src")).expect("mkdir");
        std::fs::write(root.join("b/Cargo.toml"), "[package]\nname = \"b\"\nedition.workspace = true\n")
            .expect("write");
        std::fs::create_dir_all(root.join("c/src")).expect("mkdir");
        std::fs::write(root.join("c/Cargo.toml"), "[package]\nname = \"c\"\n\n[dependencies]\nedition = \"x\"\n")
            .expect("write");
        assert_eq!(edition(&root.join("a/src/lib.rs")), "2018");
        assert_eq!(edition(&root.join("b/src/lib.rs")), "2024");
        assert_eq!(edition(&root.join("c/src/lib.rs")), "2015");
        assert_eq!(edition(&root.join("tools/x.rs")), "2024");
        assert_eq!(edition(Path::new("/no/such/place/x.rs")), "2021");
    }

    #[test]
    fn each_formatter_is_given_the_range_in_its_own_units() {
        let root = Path::new("/w");
        let file = Path::new("/w/src/a.ts");
        // "é" is one character, "😀" two UTF-16 units.
        let text = "aé😀b\nline two\nthree\n".to_string();
        let span = Span { start: (0, 3), end: (2, 0) };
        let job = Job { text, range: Some(span), tab_size: 4, insert_spaces: true, prettier: vec!["--no-semi".into()] };
        let mut f = found("prettier", true);
        f.range = true;
        let p = plan(&f, &job, root, file).expect("plan");
        assert_eq!(
            p.args,
            ["--stdin-filepath", "/w/src/a.ts", "--no-color", "--no-semi", "--range-start", "4", "--range-end", "15"]
        );
        f.id = "ruff".into();
        let p = plan(&f, &job, root, file).expect("plan");
        assert!(p.args.contains(&"--range=1:4-3:1".to_string()), "{:?}", p.args);
        f.id = "black".into();
        let p = plan(&f, &job, root, file).expect("plan");
        assert!(p.args.contains(&"--line-ranges=1-2".to_string()), "{:?}", p.args);
        f.id = "clang-format".into();
        let p = plan(&f, &job, root, file).expect("plan");
        assert_eq!(p.args, ["--assume-filename=/w/src/a.ts", "--lines=1:2"]);
        let g = found("gofmt", true);
        assert_eq!(plan(&g, &job, root, file).map_err(|e| e.kind), Err("no_range"));
        assert_eq!(plan(&found("gofmt", false), &job, root, file).map_err(|e| e.kind), Err("not_found"));
        let d = plan(&found("dotnet-format", true), &Job::default(), root, Path::new("/w/src/A.cs")).expect("plan");
        assert_eq!(d.timeout, DOTNET_TIME_LIMIT);
        let (folder, rel) = d.mirror.expect("mirror");
        assert_eq!(rel, PathBuf::from("src/A.cs"));
        assert_eq!(d.args[..3], ["format".to_string(), "whitespace".into(), folder.display().to_string()]);
    }

    #[test]
    fn placeholders_name_the_file_the_folder_and_the_range() {
        let job = Job {
            text: "ab\ncd\n".into(),
            range: Some(Span { start: (1, 1), end: (1, 2) }),
            tab_size: 2,
            insert_spaces: false,
            prettier: vec![],
        };
        let root = Path::new("/w");
        let file = Path::new("/w/src/x.py");
        let f = |s: &str| placeholders(s, root, file, &job);
        assert_eq!(f("${file}|${relativeFile}|${fileBasename}"), "/w/src/x.py|src/x.py|x.py");
        assert_eq!(f("${fileBasenameNoExtension}${fileExtname} ${fileDirname}"), "x.py /w/src");
        assert_eq!(f("${workspaceFolder} ${tabSize} ${insertSpaces}"), "/w 2 false");
        assert_eq!(f("${startLine}-${endLine} ${startOffset}-${endOffset}"), "2-2 4-5");
    }

    #[test]
    fn prettiers_settings_become_flags_only_where_they_differ_from_its_defaults() {
        let mut s = PrettierSettings {
            print_width: 80,
            tab_width: 2,
            use_tabs: false,
            semi: true,
            single_quote: false,
            trailing_comma: "all".into(),
            bracket_spacing: true,
            arrow_parens: "always".into(),
            end_of_line: "lf".into(),
        };
        assert!(s.flags().is_empty());
        s.print_width = 100;
        s.semi = false;
        s.single_quote = true;
        s.trailing_comma = "es5".into();
        assert_eq!(s.flags(), ["--print-width", "100", "--trailing-comma", "es5", "--no-semi", "--single-quote"]);
    }

    #[test]
    fn a_folders_prettier_configuration_is_found_from_the_file_up() {
        let d = tempfile::tempdir().expect("temp");
        let root = d.path().join("w");
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        assert!(!prettier_config(&root.join("src/a.ts"), &root));
        std::fs::write(root.join("package.json"), r#"{"name": "w", "prettier": {"semi": false}}"#).expect("write");
        assert!(prettier_config(&root.join("src/a.ts"), &root));
        std::fs::write(root.join("package.json"), r#"{"name": "w"}"#).expect("write");
        assert!(!prettier_config(&root.join("src/a.ts"), &root));
        std::fs::write(root.join("src/.prettierrc"), "{}").expect("write");
        assert!(prettier_config(&root.join("src/a.ts"), &root));
    }

    #[test]
    fn a_failure_says_the_formatters_words_and_the_place() {
        let f = failure("Prettier", "[error] stdin: SyntaxError: Unexpected token (3:5)\n[error]   1 | x", Some(2));
        assert_eq!(f.message, "Prettier: stdin: SyntaxError: Unexpected token (3:5)");
        assert_eq!((f.line, f.col), (Some(3), Some(5)));
        let f = failure(
            "Black",
            "error: cannot format -: Cannot parse for target version Python 3.12: 7:2: x(\n",
            Some(123),
        );
        assert_eq!((f.line, f.col), (Some(7), Some(2)));
        let f = failure("gofmt", "<standard input>:4:10: expected '}', found 'EOF'\n", Some(2));
        assert_eq!((f.line, f.col), (Some(4), Some(10)));
        let f = failure("rustfmt", "\x1b[31merror\x1b[0m: expected one of `;`\n --> <stdin>:2:9\n", Some(1));
        assert_eq!(f.message, "rustfmt: error: expected one of `;`");
        assert_eq!((f.line, f.col), (Some(2), Some(9)));
        let f = failure("Ruff", "  \n", Some(2));
        assert_eq!(f.message, "Ruff stopped with exit code 2.");
        assert_eq!(f.line, None);
        assert_eq!(version_in("black, 24.1.0 (compiled: yes)\nPython (CPython) 3.12.1"), Some("24.1.0".into()));
        assert_eq!(version_in("Ubuntu clang-format version 14.0.0-1ubuntu1"), Some("14.0.0".into()));
    }

    #[test]
    fn offsets_count_utf16_units_or_characters() {
        let t = "a😀\nbc";
        assert_eq!(offset(t, (0, 2), true), 3);
        assert_eq!(offset(t, (0, 2), false), 2);
        assert_eq!(offset(t, (1, 1), true), 5);
        assert_eq!(offset(t, (1, 1), false), 4);
        assert_eq!(offset(t, (9, 0), false), 5);
        let s = Span::of(&json!({"start": {"line": 3, "character": 0}, "end": {"line": 1, "character": 2}}));
        assert_eq!(s, Some(Span { start: (1, 2), end: (3, 0) }));
    }

    #[test]
    fn only_four_run_at_once() {
        let gate = Gate::default();
        let most = std::sync::atomic::AtomicUsize::new(0);
        let now = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..10 {
                s.spawn(|| {
                    let _p = gate.enter();
                    let n = now.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    most.fetch_max(n, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    now.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                });
            }
        });
        assert!(most.into_inner() <= AT_ONCE);
    }

    #[test]
    fn a_listing_is_kept_for_a_while() {
        let cache = Cache::default();
        let made = std::cell::Cell::new(0);
        let make = || {
            made.set(made.get() + 1);
            vec![found("ruff", true)]
        };
        assert_eq!(cache.get_or("k", make).len(), 1);
        assert_eq!(cache.get_or("k", make).len(), 1);
        assert_eq!(made.get(), 1);
        cache.get_or("other", make);
        assert_eq!(made.get(), 2);
    }

    #[cfg(unix)]
    fn sh_plan(script: &str, timeout: Duration) -> Plan {
        Plan {
            name: "Fake".into(),
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script.into()],
            cwd: std::env::temp_dir(),
            timeout,
            mirror: None,
            readable: vec![],
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_text_goes_through_standard_input_and_comes_back_with_no_key_in_reach() {
        let gate = Gate::default();
        let env = passed_env(
            [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("CODE_ARENA_KEY"), OsString::from("fake-key")),
            ]
            .into_iter(),
        );
        let r = Runner { sandbox: None, env, gate: &gate };
        // Larger than a pipe holds, so both ends must move at once.
        let big = "abc\n".repeat(100_000);
        let out = r.run(&sh_plan("tr a-z A-Z", TIME_LIMIT), &big).expect("formatted");
        assert_eq!(out, "ABC\n".repeat(100_000));
        let out = r.run(&sh_plan("echo \"${CODE_ARENA_KEY:-none} $NO_COLOR\"", TIME_LIMIT), "").expect("ran");
        assert_eq!(out, "none 1\n");
        let e = r.run(&sh_plan("echo 'x.py:3:4: bad' >&2; exit 2", TIME_LIMIT), "").expect_err("fails");
        assert_eq!((e.message.as_str(), e.line, e.col, e.kind), ("Fake: x.py:3:4: bad", Some(3), Some(4), "failed"));
        let started = Instant::now();
        let e = r.run(&sh_plan("sleep 5", Duration::from_millis(200)), "").expect_err("stopped");
        assert_eq!(e.kind, "timeout");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn a_formatter_of_files_formats_a_copy_in_a_folder_of_its_own() {
        let gate = Gate::default();
        let r = Runner { sandbox: None, env: vec![], gate: &gate };
        let folder = mirror_folder();
        let mut p = sh_plan("tr a-z A-Z < src/A.cs > t && mv t src/A.cs", TIME_LIMIT);
        p.cwd = folder.clone();
        p.mirror = Some((folder.clone(), PathBuf::from("src/A.cs")));
        assert_eq!(r.run(&p, "class a {}").expect("formatted"), "CLASS A {}");
        assert!(!folder.exists());
    }
}
