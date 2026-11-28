//! The tasks of tasks.json and the ones found in the project: listed, run in a terminal of their own, stopped.
//!
//! - `task_list{asked?}` [T] is answered with `task_list{items, errors, trusted, build, inputs, asked}`: every task
//!   (`{label, source, group, is_default, command, detail, depends_on, background, running, terminal}`), what in
//!   `.vscode/tasks.json` could not be read, whether the folder is trusted, and its build configuration. `asked`
//!   comes back as it was sent, so the face knows what it listed them for (a pick to run, to terminate).
//!   (`tasks` is the driver's own reply, about the plan's tasks: this list is named apart from it.)
//! - `task_run{label? | group?, inputs?, context?, allow?}` [T] runs a task (or a group's default: `build`,
//!   `test`), the tasks it depends on first. It is answered with `task_started{label, terminal, replaces}` for each
//!   one started (its terminal's id, and the ended terminal of its last run that it takes the place of), then
//!   `task_ended{label, terminal, code}`; or `task_needs{label, inputs}` (the `${input:id}` to ask for, sent again
//!   with `inputs: {id: answer}`), `task_untrusted{label, commands}` (the folder is not trusted: what would run,
//!   sent again with `allow: true` once the person says so), `task_pick{group, items}` (no default in the group:
//!   which one) or `task_failed{label | group, error}`. `context` is the editor's `{file, line, selected}`.
//! - `task_stop{label}` [T] ends a task that runs (`task_stop{label}`, then its `task_ended`).
//! - `build_configuration{value?}` [T] keeps the folder's build configuration (Debug or Release) and says it to
//!   every face as `build_configuration{folder, value}`.
//!
//! A task runs in its own terminal (`term_started{kind: "task"}`, titled "Task - LABEL"); one that is still active
//! is not run twice. Its dependencies run first, one after another (`dependsOrder: "sequence"`) or together; one
//! that fails stops those that depend on it. A background task (`isBackground`) counts as done once started.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::ide_ext::{Area, Rule, Shared, kind, send};
use crate::ide_vars;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("task_list", Rule::Turn),
        ("task_run", Rule::Turn),
        ("task_stop", Rule::Turn),
        ("build_configuration", Rule::Turn),
    ];

    fn start(sh: &Shared) {
        crate::ide_trust::on_revoke(&sh.kept, "ide_tasks", stop_all);
    }

    fn shared(sh: &Shared, m: &Value) -> bool {
        let (sh, m) = (sh.clone(), m.clone());
        match kind(&m) {
            "task_list" => {
                tokio::task::spawn_blocking(move || send(&sh.out, list_message(&sh, &m)));
            }
            "task_run" => {
                tokio::spawn(async move { run_asked(sh, m).await });
            }
            "task_stop" => {
                tokio::task::spawn_blocking(move || stop(&sh, m["label"].as_str().unwrap_or("")));
            }
            "build_configuration" => {
                tokio::task::spawn_blocking(move || {
                    let (data, root) = (&sh.paths.data, sh.workspace.root());
                    let mut reply = json!({"type": "build_configuration", "folder": root.display().to_string()});
                    if let Some(value) = m["value"].as_str() {
                        if let Err(e) = set_build(data, root, value) {
                            reply["error"] = json!(e);
                        }
                    }
                    reply["value"] = json!(build_of(data, root));
                    send(&sh.out, reply);
                });
            }
            _ => return false,
        }
        true
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Tasks as written

/// How an argument is quoted on a shell's line (tasks.json's `quoting`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Quoting {
    /// Each character the shell reads as its own gets a backslash.
    Escape,
    /// In single quotes: nothing in it is read.
    Strong,
    /// In double quotes: variables are still read.
    Weak,
}

/// One argument of a task's command.
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub value: String,
    /// None: quoted only when it has to be (spaces, the shell's own characters).
    pub quoting: Option<Quoting>,
}

fn arg(value: &str) -> Arg {
    Arg { value: value.to_string(), quoting: None }
}

/// A task: one of tasks.json's, or one found in the project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Task {
    pub label: String,
    /// Where it comes from: "Workspace" (tasks.json), or what found it ("npm", "cargo", "make", ...).
    pub source: String,
    /// Run by the shell (`shell`, and every task found); else the program itself (`process`).
    pub shell: bool,
    pub command: String,
    pub args: Vec<Arg>,
    /// The folder it runs in (`options.cwd`); none: the project's.
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    /// `options.shell`: the shell and its arguments, in place of the default one and `-c`.
    pub shell_exe: Option<String>,
    pub shell_args: Option<Vec<String>>,
    /// "build" or "test" (none: neither).
    pub group: Option<String>,
    pub is_default: bool,
    pub depends_on: Vec<String>,
    /// `dependsOrder: "sequence"`: one after another (else together).
    pub sequence: bool,
    pub background: bool,
    pub detail: String,
    /// `hide`: not offered in Run Task.
    pub hide: bool,
}

/// What the tasks of a folder are: tasks.json's and those found, its inputs, and what could not be read.
#[derive(Clone, Debug, Default)]
pub struct Book {
    pub tasks: Vec<Task>,
    /// tasks.json's `inputs`, as written.
    pub inputs: Vec<Value>,
    pub errors: Vec<String>,
}

impl Book {
    fn find(&self, label: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.label == label)
    }
}

/// The name tasks.json gives this system in its per-system blocks.
fn os_key() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "osx",
        _ => "linux",
    }
}

/// An object with its block for this system laid over it (`options` one level deep, as VS Code does).
fn for_os(v: &Value, os: &str) -> Map<String, Value> {
    let mut out = v.as_object().cloned().unwrap_or_default();
    if let Some(over) = v.get(os).and_then(Value::as_object) {
        for (k, val) in over {
            match (out.get_mut(k), val) {
                (Some(Value::Object(was)), Value::Object(more)) if k == "options" => {
                    for (k2, v2) in more {
                        was.insert(k2.clone(), v2.clone());
                    }
                }
                _ => {
                    out.insert(k.clone(), val.clone());
                }
            }
        }
    }
    for k in ["windows", "osx", "linux"] {
        out.remove(k);
    }
    out
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("value").and_then(text_of),
        Value::Array(a) => Some(a.iter().filter_map(text_of).collect::<Vec<_>>().join(" ")),
        _ => None,
    }
}

fn arg_of(v: &Value) -> Option<Arg> {
    let quoting = |q: &Value| match q.as_str() {
        Some("escape") => Some(Quoting::Escape),
        Some("strong") => Some(Quoting::Strong),
        Some("weak") => Some(Quoting::Weak),
        _ => None,
    };
    match v {
        Value::String(s) => Some(arg(s)),
        Value::Object(o) => Some(Arg { value: o.get("value").and_then(text_of)?, quoting: quoting(&o["quoting"]) }),
        Value::Number(_) | Value::Bool(_) => Some(arg(&v.to_string())),
        _ => None,
    }
}

/// A tasks.json read: its tasks for this system (`os`), its inputs, and what in it could not be taken.
pub fn parse(text: &str, os: &str) -> Book {
    let mut book = Book::default();
    let v = match ca_settings::jsonc::parse(text) {
        Ok(v) => v,
        Err(e) => {
            book.errors.push(format!("tasks.json could not be read: {e}"));
            return book;
        }
    };
    let top = for_os(&v, os);
    if top.get("version").and_then(Value::as_str) != Some("2.0.0") {
        book.errors.push("tasks.json: only version 2.0.0 is read (\"version\": \"2.0.0\").".to_string());
    }
    let top_options = top.get("options").cloned().unwrap_or(Value::Null);
    book.inputs = top.get("inputs").and_then(Value::as_array).cloned().unwrap_or_default();
    for (n, t) in top.get("tasks").and_then(Value::as_array).into_iter().flatten().enumerate() {
        match task_of(&for_os(t, os), &top_options) {
            Ok(task) => book.tasks.push(task),
            Err(e) => book.errors.push(format!("tasks.json, task {}: {e}", n + 1)),
        }
    }
    book
}

/// One task of tasks.json, its system's block laid over it already.
fn task_of(t: &Map<String, Value>, top_options: &Value) -> Result<Task, String> {
    let s = |k: &str| t.get(k).and_then(Value::as_str).map(str::to_string);
    let kind = s("type").unwrap_or_else(|| "shell".to_string());
    let mut task = Task { source: "Workspace".into(), shell: true, ..Task::default() };
    let args: Vec<Arg> = t.get("args").and_then(Value::as_array).into_iter().flatten().filter_map(arg_of).collect();
    match kind.as_str() {
        "shell" | "process" => {
            task.shell = kind == "shell";
            task.command = t.get("command").and_then(text_of).ok_or("it has no \"command\".")?;
            task.args = args;
        }
        "npm" => {
            let script = s("script").ok_or("an npm task needs its \"script\".")?;
            task.command = "npm".into();
            task.args = vec![arg("run"), arg(&script)];
            task.args.extend(args);
            let path = s("path").unwrap_or_default();
            let path = path.trim_end_matches('/');
            task.label = if path.is_empty() { format!("npm: {script}") } else { format!("npm: {script} - {path}") };
            if !path.is_empty() {
                task.cwd = Some(format!("${{workspaceFolder}}/{path}"));
            }
        }
        "typescript" => {
            let config = s("tsconfig").unwrap_or_else(|| "tsconfig.json".into());
            let watch = s("option").as_deref() == Some("watch");
            task.command = "tsc".into();
            task.args = vec![arg("-p"), arg(&config)];
            if watch {
                task.args.push(arg("--watch"));
                task.background = true;
            }
            task.label = format!("tsc: {} - {config}", if watch { "watch" } else { "build" });
        }
        "cargo" => {
            task.command = "cargo".into();
            task.args = vec![arg(&s("command").ok_or("a cargo task needs its \"command\".")?)];
            task.args.extend(args);
            task.label = format!("cargo: {}", task.args.iter().map(|a| a.value.as_str()).collect::<Vec<_>>().join(" "));
        }
        other => return Err(format!("the task type '{other}' is not known.")),
    }
    if let Some(label) = s("label").or_else(|| s("taskName")) {
        task.label = label;
    }
    if task.label.is_empty() {
        task.label = task.command.clone();
    }
    // The file's own options first, the task's over them.
    let options = |k: &str| t.get("options").and_then(|o| o.get(k)).or_else(|| top_options.get(k));
    if let Some(cwd) = options("cwd").and_then(Value::as_str) {
        task.cwd = Some(cwd.to_string());
    }
    let mut env: Vec<(String, String)> = Vec::new();
    for o in [top_options.get("env"), t.get("options").and_then(|o| o.get("env"))].into_iter().flatten() {
        for (k, v) in o.as_object().into_iter().flatten() {
            env.retain(|(had, _)| had != k);
            env.push((k.clone(), text_of(v).unwrap_or_default()));
        }
    }
    task.env = env;
    if let Some(sh) = options("shell") {
        task.shell_exe = sh.get("executable").and_then(Value::as_str).map(str::to_string);
        task.shell_args =
            sh.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(text_of).collect::<Vec<_>>());
    }
    match t.get("group") {
        Some(Value::String(g)) if g != "none" => task.group = Some(g.clone()),
        Some(Value::Object(g)) => {
            task.group = g.get("kind").and_then(Value::as_str).filter(|k| *k != "none").map(str::to_string);
            task.is_default = matches!(g.get("isDefault"), Some(Value::Bool(true)) | Some(Value::String(_)));
        }
        _ => {}
    }
    task.depends_on = match t.get("dependsOn") {
        Some(Value::String(d)) => vec![d.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    task.sequence = s("dependsOrder").as_deref() == Some("sequence");
    if let Some(b) = t.get("isBackground").and_then(Value::as_bool) {
        task.background = b;
    }
    task.detail = s("detail").unwrap_or_default();
    task.hide = t.get("hide").and_then(Value::as_bool).unwrap_or(false);
    Ok(task)
}

/// What is wrong across the tasks: a label given twice, a dependency that is no task, a task that depends on
/// itself (through others too).
pub fn check(tasks: &[Task]) -> Vec<String> {
    let mut errors = Vec::new();
    for (i, t) in tasks.iter().enumerate() {
        if tasks[..i].iter().any(|o| o.label == t.label) {
            errors.push(format!("Two tasks are labelled '{}': the first is run.", t.label));
        }
        for d in &t.depends_on {
            if !tasks.iter().any(|o| &o.label == d) {
                errors.push(format!("The task '{}' depends on '{d}', which is not a task.", t.label));
            }
        }
    }
    for t in tasks {
        if order(tasks, &t.label).is_err() {
            errors.push(format!("The task '{}' depends on itself.", t.label));
        }
    }
    errors
}

/// The tasks one runs, its dependencies first (each once), itself last; an error for one it depends on that is
/// not there or that depends on it again.
pub fn order(tasks: &[Task], label: &str) -> Result<Vec<String>, String> {
    fn visit(tasks: &[Task], label: &str, path: &mut Vec<String>, out: &mut Vec<String>) -> Result<(), String> {
        if out.iter().any(|l| l == label) {
            return Ok(());
        }
        if path.iter().any(|l| l == label) {
            return Err(format!("The task '{label}' depends on itself."));
        }
        let t = tasks.iter().find(|t| t.label == label).ok_or_else(|| format!("There is no task '{label}'."))?;
        path.push(label.to_string());
        for d in &t.depends_on {
            visit(tasks, d, path, out)?;
        }
        path.pop();
        out.push(label.to_string());
        Ok(())
    }
    let mut out = Vec::new();
    visit(tasks, label, &mut Vec::new(), &mut out)?;
    Ok(out)
}

/// Which of a group's tasks Run Build Task (or Run Test Task) runs.
#[derive(Debug, PartialEq)]
pub enum Chosen {
    One(String),
    /// No default among these: the person picks.
    Many(Vec<String>),
    None,
}

pub fn choose(tasks: &[Task], group: &str) -> Chosen {
    let of: Vec<&Task> = tasks.iter().filter(|t| t.group.as_deref() == Some(group)).collect();
    if let Some(t) = of.iter().find(|t| t.is_default) {
        return Chosen::One(t.label.clone());
    }
    match of.as_slice() {
        [] => Chosen::None,
        [one] => Chosen::One(one.label.clone()),
        many => Chosen::Many(many.iter().map(|t| t.label.clone()).collect()),
    }
}

/// An argument as the shell is to read it.
fn quoted(a: &Arg) -> String {
    const SPECIAL: &[char] = &[
        ' ', '\t', '\n', '\'', '"', '\\', '$', '`', '!', '*', '?', ';', '&', '|', '<', '>', '(', ')', '[', ']', '{', '}',
        '#', '~',
    ];
    let v = &a.value;
    let how = match a.quoting {
        Some(q) => q,
        None if v.is_empty() || v.contains(SPECIAL) => Quoting::Strong,
        None => return v.clone(),
    };
    match how {
        Quoting::Strong => format!("'{}'", v.replace('\'', r"'\''")),
        Quoting::Weak => {
            let mut s = String::from("\"");
            for c in v.chars() {
                if matches!(c, '"' | '\\' | '`') {
                    s.push('\\');
                }
                s.push(c);
            }
            s.push('"');
            s
        }
        Quoting::Escape => {
            let mut s = String::new();
            for c in v.chars() {
                if SPECIAL.contains(&c) {
                    s.push('\\');
                }
                s.push(c);
            }
            s
        }
    }
}

/// A task's command as one line (what a shell task's shell is given, and what the lists show).
pub fn line(t: &Task) -> String {
    let mut s = t.command.clone();
    for a in &t.args {
        s.push(' ');
        s.push_str(&if t.shell { quoted(a) } else { a.value.clone() });
    }
    s
}

/// The ids of the inputs a task asks for, anywhere in what it runs.
fn inputs_of(t: &Task) -> Vec<String> {
    let mut texts = vec![t.command.clone()];
    texts.extend(t.args.iter().map(|a| a.value.clone()));
    texts.extend(t.cwd.clone());
    texts.extend(t.env.iter().map(|(_, v)| v.clone()));
    let mut ids: Vec<String> = Vec::new();
    for id in texts.iter().flat_map(|x| ide_vars::inputs_in(x)) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// What a task runs, with its variables in their places: the program, its arguments, its folder.
pub fn command_of(t: &Task, cx: &ide_vars::Ctx, shell: &Path) -> Result<(PathBuf, Vec<String>, PathBuf), String> {
    let x = |s: &str| ide_vars::expand(s, cx);
    let mut expanded = t.clone();
    expanded.command = x(&t.command)?;
    for a in &mut expanded.args {
        a.value = x(&a.value)?;
    }
    let cwd = match &t.cwd {
        Some(c) => {
            let c = PathBuf::from(x(c)?);
            if c.is_absolute() { c } else { cx.folder.join(c) }
        }
        None => cx.folder.to_path_buf(),
    };
    let (program, mut args) = if t.shell {
        let exe = match &t.shell_exe {
            Some(e) => PathBuf::from(x(e)?),
            None => shell.to_path_buf(),
        };
        let mut args = t.shell_args.clone().unwrap_or_else(|| vec!["-c".to_string()]);
        args.push(line(&expanded));
        (exe, args)
    } else {
        (PathBuf::from(&expanded.command), expanded.args.iter().map(|a| a.value.clone()).collect())
    };
    if !t.env.is_empty() {
        // `env NAME=value ... program args`: the terminal's own variables, with the task's laid over them.
        let mut with = Vec::new();
        for (k, v) in &t.env {
            with.push(format!("{k}={}", x(v)?));
        }
        with.push(program.display().to_string());
        with.append(&mut args);
        return Ok((PathBuf::from("env"), with, cwd));
    }
    Ok((program, args, cwd))
}

// ---------------------------------------------------------------------------------------------------------------
// Tasks found in the project

/// The most folders under the project looked in for a package.json.
const MOST_PACKAGES: usize = 30;
/// The most make targets offered.
const MOST_TARGETS: usize = 50;

fn found(source: &str, name: &str, command: &str, args: &[&str], group: Option<&str>) -> Task {
    Task {
        label: format!("{source}: {name}"),
        source: source.into(),
        shell: true,
        command: command.into(),
        args: args.iter().map(|a| arg(a)).collect(),
        group: group.map(str::to_string),
        ..Task::default()
    }
}

fn group_of(name: &str) -> Option<&'static str> {
    match name {
        "build" | "all" | "compile" => Some("build"),
        "test" | "tests" | "check" => Some("test"),
        _ => None,
    }
}

/// The tasks the project's own files offer (npm scripts, cargo, go, make, dotnet, cmake, ctest, pytest), found
/// from the files alone: nothing is run to find them. `build` is the build configuration, Debug or Release.
pub fn detect(root: &Path, build: &str) -> Vec<Task> {
    let has = |name: &str| root.join(name).is_file();
    let read = |name: &str| std::fs::read_to_string(root.join(name)).unwrap_or_default();
    let release = build == "Release";
    let mut out = Vec::new();
    out.extend(npm_tasks(root));
    if has("Cargo.toml") {
        let r: &[&str] = if release { &["--release"] } else { &[] };
        for (name, group) in [("build", Some("build")), ("check", None), ("test", Some("test")), ("run", None)] {
            let mut args = vec![name];
            args.extend(r);
            out.push(found("cargo", name, "cargo", &args, group));
        }
        out.push(found("cargo", "clippy", "cargo", &["clippy"], None));
    }
    if has("go.mod") {
        out.push(found("go", "build", "go", &["build", "./..."], Some("build")));
        out.push(found("go", "test", "go", &["test", "./..."], Some("test")));
    }
    if let Some(make) = ["GNUmakefile", "makefile", "Makefile"].into_iter().find(|m| has(m)) {
        for target in make_targets(&read(make)) {
            out.push(found("make", &target, "make", &[&target], group_of(&target)));
        }
    }
    let dotnet = std::fs::read_dir(root).into_iter().flatten().flatten().any(|e| {
        let n = e.file_name().to_string_lossy().to_string();
        n.ends_with(".sln") || n.ends_with(".csproj") || n.ends_with(".fsproj")
    });
    if dotnet {
        for (name, group) in [("build", Some("build")), ("test", Some("test")), ("run", None)] {
            out.push(found("dotnet", name, "dotnet", &[name, "-c", build], group));
        }
    }
    if has("CMakeLists.txt") {
        let kind = format!("-DCMAKE_BUILD_TYPE={build}");
        out.push(found("cmake", "configure", "cmake", &["-S", ".", "-B", "build", &kind], None));
        out.push(found("cmake", "build", "cmake", &["--build", "build", "--config", build], Some("build")));
        out.push(found("ctest", "test", "ctest", &["--test-dir", "build", "-C", build], Some("test")));
    }
    let pytest = has("pytest.ini")
        || has("conftest.py")
        || read("pyproject.toml").contains("[tool.pytest")
        || read("setup.cfg").contains("[tool:pytest]")
        || read("tox.ini").contains("[pytest]");
    if pytest {
        out.push(found("pytest", "test", "python3", &["-m", "pytest"], Some("test")));
    }
    out
}

/// The npm scripts of the project's package.json and of those one folder down (not node_modules, not hidden).
fn npm_tasks(root: &Path) -> Vec<Task> {
    let mut dirs = vec![(root.to_path_buf(), String::new())];
    let mut under: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.') && n != "node_modules" && n != "target")
        .collect();
    under.sort();
    dirs.extend(under.into_iter().take(MOST_PACKAGES).map(|n| (root.join(&n), n)));
    let mut out = Vec::new();
    for (dir, name) in dirs {
        let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        let runner = if dir.join("pnpm-lock.yaml").is_file() {
            "pnpm"
        } else if dir.join("yarn.lock").is_file() {
            "yarn"
        } else {
            "npm"
        };
        for (script, body) in v["scripts"].as_object().into_iter().flatten() {
            let mut t = found("npm", script, runner, &["run", script], group_of(script));
            if !name.is_empty() {
                t.label = format!("npm: {script} - {name}");
                t.cwd = Some(dir.display().to_string());
            }
            t.detail = body.as_str().unwrap_or("").to_string();
            out.push(t);
        }
    }
    out
}

/// A makefile's targets: the rules named at a line's start, not the special ones (`.PHONY`), not patterns.
pub fn make_targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines() {
        if l.starts_with(['\t', ' ', '.', '#']) {
            continue;
        }
        let Some(colon) = l.find(':') else { continue };
        if l[colon + 1..].starts_with('=') || l[..colon].contains('=') {
            continue;
        }
        for name in l[..colon].split_whitespace() {
            let ok = name.chars().all(|c| c.is_alphanumeric() || "_-./".contains(c));
            if ok && !name.contains('%') && !out.iter().any(|o| o == name) {
                out.push(name.to_string());
            }
        }
        if out.len() >= MOST_TARGETS {
            break;
        }
    }
    out.truncate(MOST_TARGETS);
    out
}

/// The folder's tasks: tasks.json's (read whole, at most 1 MB), then those found that tasks.json does not name.
pub fn load(root: &Path, build: &str) -> Book {
    let file = root.join(".vscode").join("tasks.json");
    let mut book = match std::fs::metadata(&file) {
        Ok(m) if m.len() > 1 << 20 => {
            Book { errors: vec!["tasks.json is larger than 1 MB: it is not read.".into()], ..Book::default() }
        }
        Ok(_) => parse(&std::fs::read_to_string(&file).unwrap_or_default(), os_key()),
        Err(_) => Book::default(),
    };
    for t in detect(root, build) {
        if !book.tasks.iter().any(|w| w.label == t.label) {
            book.tasks.push(t);
        }
    }
    let more = check(&book.tasks);
    book.errors.extend(more);
    book
}

// ---------------------------------------------------------------------------------------------------------------
// The build configuration

/// The file in the data folder the build configurations are kept in: `{"folders": {"/abs/path": "Release"}}`.
const BUILD_FILE: &str = "build-configurations.json";

/// The folder's build configuration: Release when it was chosen, else Debug.
pub fn build_of(data: &Path, root: &Path) -> String {
    let kept = std::fs::read_to_string(data.join(BUILD_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["folders"][root.display().to_string()].as_str().map(str::to_string));
    match kept.as_deref() {
        Some("Release") => "Release".into(),
        _ => "Debug".into(),
    }
}

/// Keeps the folder's build configuration (Debug or Release), written whole.
pub fn set_build(data: &Path, root: &Path, value: &str) -> Result<(), String> {
    if value != "Debug" && value != "Release" {
        return Err(format!("The build configuration is Debug or Release, not '{value}'."));
    }
    let file = data.join(BUILD_FILE);
    let mut all = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or_else(|| json!({"folders": {}}));
    if !all["folders"].is_object() {
        all["folders"] = json!({});
    }
    all["folders"][root.display().to_string()] = json!(value);
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(data)?;
        let tmp = data.join(format!("{BUILD_FILE}.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string_pretty(&all)?)?;
        std::fs::rename(tmp, &file)
    };
    write().map_err(|e| format!("The build configuration was not kept: {e}"))
}

// ---------------------------------------------------------------------------------------------------------------
// Runs

/// A task's last run in this folder.
#[derive(Clone, Copy, Debug)]
struct Run {
    terminal: u64,
    running: bool,
    /// Ended by Terminate Task (or trust taken back), not by itself.
    stopped: bool,
}

/// The folder's runs, by label.
#[derive(Default)]
struct Runs(Mutex<HashMap<String, Run>>);

fn runs(sh: &Shared) -> Arc<Runs> {
    sh.side.kept.of::<Runs>()
}

impl Runs {
    fn get(&self, label: &str) -> Option<Run> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(label).copied()
    }

    fn set(&self, label: &str, run: Run) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(label.to_string(), run);
    }

    fn running(&self) -> Vec<(String, u64)> {
        let all = self.0.lock().unwrap_or_else(|e| e.into_inner());
        all.iter().filter(|(_, r)| r.running).map(|(l, r)| (l.clone(), r.terminal)).collect()
    }
}

/// The list a face asked for.
fn list_message(sh: &Shared, m: &Value) -> Value {
    let (data, root) = (&sh.paths.data, sh.workspace.root());
    let build = build_of(data, root);
    let book = load(root, &build);
    let runs = runs(sh);
    let items: Vec<Value> = book
        .tasks
        .iter()
        .map(|t| {
            let run = runs.get(&t.label);
            json!({
                "label": t.label,
                "source": t.source,
                "group": t.group,
                "is_default": t.is_default,
                "command": line(t),
                "detail": t.detail,
                "depends_on": t.depends_on,
                "background": t.background,
                "hide": t.hide,
                "running": run.is_some_and(|r| r.running),
                "terminal": run.map(|r| r.terminal),
            })
        })
        .collect();
    json!({
        "type": "task_list",
        "asked": m["asked"],
        "items": items,
        "errors": book.errors,
        "trusted": crate::ide_trust::trusted(data, root),
        "build": build,
    })
}

/// What a run reads its variables from, owned, so that it may go into the tasks that run the dependencies.
#[derive(Clone, Default)]
struct Context {
    folder: PathBuf,
    file: Option<String>,
    line: Option<u64>,
    selected: String,
    build: String,
    inputs: Map<String, Value>,
    shell: PathBuf,
    /// For `${config:key}`.
    settings: Option<Arc<ca_settings::Settings>>,
}

impl Context {
    fn expand<R>(&self, f: impl FnOnce(&ide_vars::Ctx) -> R) -> R {
        let config = |key: &str| self.settings.as_ref().map(|s| s.ide(key, None)).filter(|v| !v.is_null());
        let cx = ide_vars::Ctx {
            folder: &self.folder,
            file: self.file.as_deref(),
            line: self.line,
            selected: &self.selected,
            build: &self.build,
            inputs: &self.inputs,
            env: &ide_vars::process_env,
            config: &config,
        };
        f(&cx)
    }
}

/// `task_run`: which task, whether all it needs is there, then the run.
async fn run_asked(sh: Shared, m: Value) {
    let (data, root) = (sh.paths.data.clone(), sh.workspace.root().to_path_buf());
    let loaded = {
        let (data, root) = (data.clone(), root.clone());
        tokio::task::spawn_blocking(move || {
            let build = build_of(&data, &root);
            let book = load(&root, &build);
            (book, build, crate::ide_trust::trusted(&data, &root))
        })
        .await
    };
    let Ok((book, build, trusted)) = loaded else { return };
    let failed = |key: &str, of: &str, error: &str| {
        send(&sh.out, json!({"type": "task_failed", key: of, "error": error}));
    };
    let label = match (m["label"].as_str(), m["group"].as_str()) {
        (Some(l), _) => l.to_string(),
        (None, Some(group)) => match choose(&book.tasks, group) {
            Chosen::One(l) => l,
            Chosen::Many(items) => {
                send(&sh.out, json!({"type": "task_pick", "group": group, "items": items}));
                return;
            }
            Chosen::None => return failed("group", group, &format!("No {group} task to run found.")),
        },
        _ => return failed("label", "", "No task was named."),
    };
    let chain = match order(&book.tasks, &label) {
        Ok(c) => c,
        Err(e) => return failed("label", &label, &e),
    };
    let r = runs(&sh);
    if r.get(&label).is_some_and(|r| r.running) {
        return failed("label", &label, &format!("The task '{label}' is already active."));
    }
    let given = m["inputs"].as_object().cloned().unwrap_or_default();
    let mut needed: Vec<Value> = Vec::new();
    for id in chain.iter().filter_map(|l| book.find(l)).flat_map(inputs_of) {
        if given.contains_key(&id) || needed.iter().any(|n| n["id"] == id.as_str()) {
            continue;
        }
        match book.inputs.iter().find(|i| i["id"] == id.as_str()) {
            Some(i) if matches!(i["type"].as_str(), Some("promptString" | "pickString")) => needed.push(i.clone()),
            Some(i) => {
                let kind = i["type"].as_str().unwrap_or("");
                return failed("label", &label, &format!("The input '{id}' is of the type '{kind}', which is not read."));
            }
            None => return failed("label", &label, &format!("The input '{id}' is not in tasks.json's \"inputs\".")),
        }
    }
    if !needed.is_empty() {
        send(&sh.out, json!({"type": "task_needs", "label": label, "inputs": needed}));
        return;
    }
    if !trusted && m["allow"] != true {
        let commands: Vec<String> = chain.iter().filter_map(|l| book.find(l)).map(line).collect();
        send(&sh.out, json!({"type": "task_untrusted", "label": label, "commands": commands}));
        return;
    }
    let c = &m["context"];
    let cx = Context {
        folder: root,
        file: c["file"].as_str().filter(|f| !f.is_empty()).map(str::to_string),
        line: c["line"].as_u64(),
        selected: c["selected"].as_str().unwrap_or("").to_string(),
        build,
        inputs: given,
        shell: sh.side.default_shell(),
        settings: Some(crate::ide_settings::current(&sh)),
    };
    if let Err(e) = run_one(sh.clone(), Arc::new(book), Arc::new(cx), label.clone()).await {
        failed("label", &label, &e);
    }
}

type Running = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Runs a task once its dependencies have; `Err` says why it did not end well (or was not run).
fn run_one(sh: Shared, book: Arc<Book>, cx: Arc<Context>, label: String) -> Running {
    Box::pin(async move {
        let task = book.find(&label).cloned().ok_or_else(|| format!("There is no task '{label}'."))?;
        // A dependency already active is not started again: it is waited for (a background one is done).
        let r = runs(&sh);
        let mut rx = sh.out.subscribe();
        if let Some(run) = r.get(&label).filter(|r| r.running) {
            if task.background {
                return Ok(());
            }
            let code = ended(&sh, &mut rx, run.terminal).await;
            return verdict(&label, code, r.get(&label).is_some_and(|r| r.stopped));
        }
        if task.sequence {
            for d in &task.depends_on {
                run_one(sh.clone(), book.clone(), cx.clone(), d.clone()).await.map_err(|e| because(&e, &label))?;
            }
        } else {
            let started: Vec<_> = task
                .depends_on
                .iter()
                .map(|d| tokio::spawn(run_one(sh.clone(), book.clone(), cx.clone(), d.clone())))
                .collect();
            let mut first = None;
            for h in started {
                let got = h.await.unwrap_or_else(|e| Err(e.to_string()));
                if let (Err(e), None) = (got, &first) {
                    first = Some(e);
                }
            }
            if let Some(e) = first {
                return Err(because(&e, &label));
            }
        }
        let (program, args, cwd) = cx.expand(|vars| command_of(&task, vars, &cx.shell))?;
        let replaces = r.get(&label).filter(|r| !r.running).map(|r| r.terminal);
        rx = sh.out.subscribe();
        let terminal = sh.side.term_start(&format!("Task - {label}"), "task", &program, &args, &cwd);
        r.set(&label, Run { terminal, running: true, stopped: false });
        send(&sh.out, json!({"type": "task_started", "label": label, "terminal": terminal, "replaces": replaces}));
        let finish = {
            let (sh, label) = (sh.clone(), label.clone());
            async move {
                let code = ended(&sh, &mut rx, terminal).await;
                let r = runs(&sh);
                let stopped = r.get(&label).is_some_and(|r| r.stopped && r.terminal == terminal);
                r.set(&label, Run { terminal, running: false, stopped });
                send(
                    &sh.out,
                    json!({"type": "task_ended", "label": label, "terminal": terminal, "code": code, "stopped": stopped}),
                );
                verdict(&label, code, stopped)
            }
        };
        if task.background {
            tokio::spawn(finish);
            return Ok(());
        }
        finish.await
    })
}

/// A dependency's failure, as the task that waited on it tells it.
fn because(e: &str, label: &str) -> String {
    if e.contains("was not run") { e.to_string() } else { format!("{e} The task '{label}' was not run.") }
}

/// Whether a task ended well.
fn verdict(label: &str, code: Option<i64>, stopped: bool) -> Result<(), String> {
    match code {
        _ if stopped => Err(format!("The task '{label}' was terminated.")),
        Some(0) => Ok(()),
        Some(c) => Err(format!("The task '{label}' ended with exit code {c}.")),
        None => Err(format!("The task '{label}' ended without an exit code.")),
    }
}

/// The exit code of a terminal, once it has ended (none: it gave none, or it was closed).
async fn ended(sh: &Shared, rx: &mut tokio::sync::broadcast::Receiver<String>, id: u64) -> Option<i64> {
    let mine = |s: &str| -> Option<Option<i64>> {
        if !s.contains("\"term_exit\"") {
            return None;
        }
        let v: Value = serde_json::from_str(s).ok()?;
        (v["type"] == "term_exit" && v["id"] == id).then(|| v["code"].as_i64())
    };
    loop {
        match tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Ok(s)) => {
                if let Some(code) = mine(&s) {
                    return code;
                }
            }
            Ok(Err(RecvError::Lagged(_))) => {}
            Ok(Err(RecvError::Closed)) => return None,
            // Its end may have been passed over while too much came at once: the terminal is gone, so what is
            // still coming is read a little longer for its code.
            Err(_) if !sh.side.term_alive(id) => {
                while let Ok(Ok(s)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
                    if let Some(code) = mine(&s) {
                        return code;
                    }
                }
                return None;
            }
            Err(_) => {}
        }
    }
}

/// Terminate Task: the task's terminal closed (its program ends), the tab left with what it showed.
fn stop(sh: &Shared, label: &str) {
    let r = runs(sh);
    let Some(run) = r.get(label).filter(|r| r.running) else {
        send(&sh.out, json!({"type": "task_stop", "label": label, "error": format!("The task '{label}' is not running.")}));
        return;
    };
    r.set(label, Run { stopped: true, ..run });
    sh.side.handle(&json!({"type": "term_close", "id": run.terminal}));
    send(&sh.out, json!({"type": "task_stop", "label": label}));
}

/// Every task that runs, stopped: the folder's trust was taken back.
fn stop_all(sh: &Shared) {
    for (label, _) in runs(sh).running() {
        stop(sh, &label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Book {
        parse(text, "linux")
    }

    #[test]
    fn a_tasks_json_is_read_with_its_comments_groups_and_options() {
        let b = parsed(
            r#"{
            // The project's tasks.
            "version": "2.0.0",
            "options": {"env": {"A": "1"}},
            "tasks": [
                {"label": "build", "type": "shell", "command": "make", "args": ["all", "a b"],
                 "group": {"kind": "build", "isDefault": true}, "options": {"cwd": "sub", "env": {"B": "2"}},
                 "detail": "Builds it"},
                {"label": "lint", "type": "process", "command": "eslint", "args": [{"value": "x y", "quoting": "escape"}],
                 "group": "test", "dependsOn": ["build"], "dependsOrder": "sequence", "isBackground": true,},
            ],
        }"#,
        );
        assert!(b.errors.is_empty(), "{:?}", b.errors);
        let build = &b.tasks[0];
        assert_eq!(build.label, "build");
        assert!(build.shell && build.is_default);
        assert_eq!(build.group.as_deref(), Some("build"));
        assert_eq!(build.cwd.as_deref(), Some("sub"));
        assert_eq!(build.env, vec![("A".to_string(), "1".to_string()), ("B".to_string(), "2".to_string())]);
        assert_eq!(line(build), "make all 'a b'");
        assert_eq!(build.detail, "Builds it");
        let lint = &b.tasks[1];
        assert!(!lint.shell && lint.background && lint.sequence);
        assert_eq!(lint.depends_on, vec!["build"]);
        assert_eq!(lint.group.as_deref(), Some("test"));
        assert_eq!(lint.args[0].quoting, Some(Quoting::Escape));
    }

    #[test]
    fn the_block_for_this_system_is_laid_over_the_task() {
        let text = r#"{"version": "2.0.0", "tasks": [{"label": "open", "command": "xdg-open",
            "windows": {"command": "start"}, "osx": {"command": "open"}, "linux": {"options": {"cwd": "/tmp"}}}]}"#;
        assert_eq!(parse(text, "windows").tasks[0].command, "start");
        assert_eq!(parse(text, "osx").tasks[0].command, "open");
        let linux = &parse(text, "linux").tasks[0];
        assert_eq!((linux.command.as_str(), linux.cwd.as_deref()), ("xdg-open", Some("/tmp")));
    }

    #[test]
    fn npm_and_typescript_tasks_are_named_as_vs_code_names_them() {
        let b = parsed(
            r#"{"version": "2.0.0", "tasks": [{"type": "npm", "script": "build", "path": "web/"},
            {"type": "typescript", "tsconfig": "tsconfig.json", "option": "watch"},
            {"type": "gulp", "task": "x"}]}"#,
        );
        assert_eq!(b.tasks[0].label, "npm: build - web");
        assert_eq!(line(&b.tasks[0]), "npm run build");
        assert_eq!(b.tasks[0].cwd.as_deref(), Some("${workspaceFolder}/web"));
        assert_eq!(b.tasks[1].label, "tsc: watch - tsconfig.json");
        assert!(b.tasks[1].background);
        assert_eq!(b.errors, vec!["tasks.json, task 3: the task type 'gulp' is not known."]);
    }

    #[test]
    fn what_cannot_be_read_is_listed() {
        assert!(parsed("{ nope").errors[0].starts_with("tasks.json could not be read"));
        let b = parsed(r#"{"version": "1.0.0", "tasks": [{"label": "x"}]}"#);
        assert_eq!(b.errors.len(), 2, "{:?}", b.errors);
        assert!(b.errors[1].contains("no \"command\""));
    }

    #[test]
    fn arguments_are_quoted_as_asked() {
        let q = |v: &str, quoting| quoted(&Arg { value: v.into(), quoting });
        assert_eq!(q("plain", None), "plain");
        assert_eq!(q("it's here", None), r"'it'\''s here'");
        assert_eq!(q("", None), "''");
        assert_eq!(q("a $HOME", Some(Quoting::Weak)), "\"a $HOME\"");
        assert_eq!(q("say \"hi\"", Some(Quoting::Weak)), r#""say \"hi\"""#);
        assert_eq!(q("a b;c", Some(Quoting::Escape)), r"a\ b\;c");
        assert_eq!(q("x", Some(Quoting::Strong)), "'x'");
    }

    fn t(label: &str, deps: &[&str]) -> Task {
        Task { label: label.into(), command: "true".into(), depends_on: deps.iter().map(|d| d.to_string()).collect(), ..Task::default() }
    }

    #[test]
    fn dependencies_come_first_each_once_and_a_loop_is_an_error() {
        let tasks = vec![t("a", &["b", "c"]), t("b", &["c"]), t("c", &[])];
        assert_eq!(order(&tasks, "a"), Ok(vec!["c".to_string(), "b".into(), "a".into()]));
        let looped = vec![t("a", &["b"]), t("b", &["a"])];
        assert!(order(&looped, "a").is_err());
        assert!(check(&looped).iter().any(|e| e.contains("depends on itself")));
        let missing = vec![t("a", &["zzz"])];
        assert_eq!(order(&missing, "a"), Err("There is no task 'zzz'.".to_string()));
        assert!(check(&missing)[0].contains("'zzz', which is not a task"));
        assert!(check(&[t("a", &[]), t("a", &[])])[0].contains("Two tasks"));
    }

    #[test]
    fn a_groups_default_is_run_else_the_person_picks() {
        let mut a = t("a", &[]);
        a.group = Some("build".into());
        let mut b = t("b", &[]);
        b.group = Some("build".into());
        assert_eq!(choose(&[a.clone()], "build"), Chosen::One("a".into()));
        assert_eq!(choose(&[a.clone(), b.clone()], "build"), Chosen::Many(vec!["a".into(), "b".into()]));
        assert_eq!(choose(&[a.clone()], "test"), Chosen::None);
        b.is_default = true;
        assert_eq!(choose(&[a, b], "build"), Chosen::One("b".into()));
    }

    #[test]
    fn a_task_runs_in_its_shell_with_its_variables_put_in() {
        let inputs = json!({"name": "web"}).as_object().cloned().unwrap_or_default();
        let env = |_: &str| None;
        let config = |_: &str| None;
        let cx = ide_vars::Ctx {
            folder: Path::new("/w"),
            file: Some("/w/src/a.rs"),
            line: Some(3),
            selected: "",
            build: "Release",
            inputs: &inputs,
            env: &env,
            config: &config,
        };
        let mut task = t("x", &[]);
        task.shell = true;
        task.command = "echo".into();
        task.args = vec![arg("${relativeFile}"), arg("${input:name} app"), arg("${codeArena.buildConfiguration}")];
        task.cwd = Some("out".into());
        let (p, a, c) = command_of(&task, &cx, Path::new("/bin/bash")).expect("runs");
        assert_eq!(p, PathBuf::from("/bin/bash"));
        assert_eq!(a, vec!["-c".to_string(), "echo src/a.rs 'web app' Release".into()]);
        assert_eq!(c, PathBuf::from("/w/out"));
        task.shell = false;
        task.env = vec![("MODE".into(), "${codeArena.buildConfiguration}".into())];
        let (p, a, _) = command_of(&task, &cx, Path::new("/bin/bash")).expect("runs");
        assert_eq!(p, PathBuf::from("env"));
        assert_eq!(a, vec!["MODE=Release", "echo", "src/a.rs", "web app", "Release"]);
        assert_eq!(inputs_of(&task), vec!["name"]);
        task.args = vec![arg("${lineNumber}"), arg("${file}")];
        let none = ide_vars::Ctx { file: None, line: None, ..cx };
        assert!(command_of(&task, &none, Path::new("/bin/sh")).is_err());
    }

    #[test]
    fn the_projects_own_tasks_are_found_from_its_files() {
        let d = tempfile::tempdir().expect("dir");
        let root = d.path();
        let w = |p: &str, text: &str| {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().expect("parent")).expect("dir");
            std::fs::write(f, text).expect("write");
        };
        w("Cargo.toml", "[package]");
        w("go.mod", "module x");
        w("Makefile", ".PHONY: all\nall: build\nbuild:\n\tcc x.c\nCC := gcc\n%.o: %.c\ntest lint: all\n");
        w("package.json", r#"{"scripts": {"build": "vite build", "start": "vite"}}"#);
        w("web/package.json", r#"{"scripts": {"test": "vitest"}}"#);
        w("web/yarn.lock", "");
        w("node_modules/x/package.json", r#"{"scripts": {"no": "x"}}"#);
        w("app.csproj", "<Project/>");
        w("CMakeLists.txt", "project(x)");
        w("pyproject.toml", "[tool.pytest.ini_options]");
        let labels = |build: &str| detect(root, build).into_iter().map(|t| (t.label.clone(), line(&t))).collect::<Vec<_>>();
        let all = labels("Release");
        let has = |l: &str, c: &str| all.iter().any(|(a, b)| a == l && b == c);
        assert!(has("cargo: build", "cargo build --release"), "{all:?}");
        assert!(has("go: test", "go test ./..."));
        assert!(has("make: all", "make all") && has("make: test", "make test") && has("make: lint", "make lint"));
        assert!(!all.iter().any(|(l, _)| l.contains('%') || l.contains("PHONY") || l == "make: CC"));
        assert!(has("npm: build", "npm run build"));
        assert!(has("npm: test - web", "yarn run test"));
        assert!(!all.iter().any(|(l, _)| l.contains("npm: no")));
        assert!(has("dotnet: build", "dotnet build -c Release"));
        assert!(has("cmake: configure", "cmake -S . -B build -DCMAKE_BUILD_TYPE=Release"));
        assert!(has("ctest: test", "ctest --test-dir build -C Release"));
        assert!(has("pytest: test", "python3 -m pytest"));
        assert!(labels("Debug").iter().any(|(l, c)| l == "cargo: build" && c == "cargo build"));
        let web = detect(root, "Debug").into_iter().find(|t| t.label == "npm: test - web").expect("web");
        assert_eq!(web.group.as_deref(), Some("test"));
        assert_eq!(web.cwd, Some(root.join("web").display().to_string()));
    }

    #[test]
    fn tasks_json_wins_over_a_task_found_with_its_label() {
        let d = tempfile::tempdir().expect("dir");
        std::fs::write(d.path().join("go.mod"), "module x").expect("write");
        std::fs::create_dir(d.path().join(".vscode")).expect("dir");
        std::fs::write(
            d.path().join(".vscode/tasks.json"),
            r#"{"version": "2.0.0", "tasks": [{"label": "go: build", "command": "go build -v"}]}"#,
        )
        .expect("write");
        let book = load(d.path(), "Debug");
        let builds: Vec<&Task> = book.tasks.iter().filter(|t| t.label == "go: build").collect();
        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].source, "Workspace");
        assert!(book.errors.is_empty(), "{:?}", book.errors);
    }

    #[test]
    fn the_build_configuration_is_kept_per_folder() {
        let d = tempfile::tempdir().expect("dir");
        let (a, b) = (Path::new("/w/a"), Path::new("/w/b"));
        assert_eq!(build_of(d.path(), a), "Debug");
        set_build(d.path(), a, "Release").expect("kept");
        assert_eq!(build_of(d.path(), a), "Release");
        assert_eq!(build_of(d.path(), b), "Debug");
        assert!(set_build(d.path(), a, "Fast").is_err());
        assert_eq!(build_of(d.path(), a), "Release");
    }

    #[test]
    fn a_task_that_fails_or_was_stopped_says_so() {
        assert_eq!(verdict("a", Some(0), false), Ok(()));
        assert_eq!(verdict("a", Some(2), false), Err("The task 'a' ended with exit code 2.".to_string()));
        assert_eq!(verdict("a", Some(0), true), Err("The task 'a' was terminated.".to_string()));
        let e = because("The task 'a' ended with exit code 2.", "b");
        assert_eq!(e, "The task 'a' ended with exit code 2. The task 'b' was not run.");
        assert_eq!(because(&e, "c"), e);
    }
}
