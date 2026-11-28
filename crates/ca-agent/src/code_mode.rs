//! Code mode: the model calls its tools by writing code that runs them, instead of one call at a time.
//!
//! `run_code` runs Python in the chat's own interpreter, kept from one call to the next as a notebook keeps its
//! kernel (variables, imports and loaded data stay), the project's virtual environment when it has one, in the
//! working directory and the session's sandbox. In it, `import arena` calls the session's tools as functions
//! (`arena.local.read_file`, `arena.<server>.<tool>`); each call is checked as a direct one is (the hooks, the
//! mode, the kept rules, the question), at most [`MOST_CALLS`] in one run, and only what the program prints comes
//! back to the model, with the value of its last line, its error and the plots it saved.
//!
//! How the model is given its tools is the [`ToolMode`]: one by one (direct), the servers' tools only as code
//! (code), or both. It is the person's `tools.mode` setting, else Arena's word for the model, else both.
//!
//! The interpreter talks on its own stdin and stdout: what a program prints comes as it is, and a line that begins
//! with the interpreter's mark is a message, a call of a tool (answered by a JSON line on its stdin) or the end of
//! the run.

use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::time::Duration;

use ca_llm::ToolDef;
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::time::Instant;

use crate::turn::{Agent, Call, Events};

/// The tool's name.
pub const RUN_CODE: &str = "run_code";

/// The most calls of the session's tools one program makes.
pub const MOST_CALLS: usize = 100;

/// The most characters of a run's output the model reads.
pub const MOST_CHARS: usize = 30_000;

/// Code Arena's own tools' group, as [`Tool::group`] names it.
const OWN: &str = "Code Arena's own";

/// The tools a program may not call: those that run code themselves, wait on or stop commands, hand work on to a
/// sub-agent, or load tools into the requests.
const NOT_FROM_CODE: &[&str] =
    &[RUN_CODE, "command_output", "stop_command", "task", "run_steps", "goal_check", "load_tools"];

/// How the model is given its tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolMode {
    /// One call at a time: no `run_code`.
    Direct,
    /// The servers' tools only as functions of a `run_code` program; Code Arena's own stay direct.
    Code,
    /// Every tool directly, and `run_code` too.
    #[default]
    Both,
}

impl ToolMode {
    /// Every mode, in the menu's order.
    pub const ALL: [ToolMode; 3] = [ToolMode::Direct, ToolMode::Code, ToolMode::Both];

    pub fn parse(word: &str) -> Option<ToolMode> {
        match word {
            "direct" => Some(ToolMode::Direct),
            "code" => Some(ToolMode::Code),
            "both" => Some(ToolMode::Both),
            _ => None,
        }
    }

    /// Its word, as `tools.mode` and the driver's messages say it.
    pub fn name(self) -> &'static str {
        match self {
            ToolMode::Direct => "direct",
            ToolMode::Code => "code",
            ToolMode::Both => "both",
        }
    }

    /// The mode in force: the person's choice, else Arena's word for the model, else both.
    pub fn of(chosen: Option<&str>, arena: Option<&str>) -> ToolMode {
        chosen.and_then(ToolMode::parse).or_else(|| arena.and_then(ToolMode::parse)).unwrap_or_default()
    }
}

/// A tool a program calls in place of the model in code mode: a server's (or the Arena project's), not Code
/// Arena's own.
pub fn programmed(t: &dyn Tool) -> bool {
    t.group() != OWN
}

/// Whether a program may call it.
pub fn callable(t: &dyn Tool) -> bool {
    !NOT_FROM_CODE.contains(&t.def().name.as_str())
}

/// The tools sent whole in this mode, from those the session has: all but `run_code` when direct; as code, all but
/// the servers' tools (only when `run_code` is there to call them), its description naming them as functions.
pub fn offered(mode: ToolMode, tools: &[Arc<dyn Tool>], deferred: &[Arc<dyn Tool>]) -> Vec<ToolDef> {
    let has_code = tools.iter().any(|t| t.def().name == RUN_CODE);
    match mode {
        ToolMode::Direct => tools.iter().map(|t| t.def()).filter(|d| d.name != RUN_CODE).collect(),
        ToolMode::Code if has_code => {
            let in_code: Vec<&Arc<dyn Tool>> =
                tools.iter().chain(deferred).filter(|t| programmed(t.as_ref()) && callable(t.as_ref())).collect();
            tools
                .iter()
                .filter(|t| !programmed(t.as_ref()))
                .map(|t| {
                    let mut d = t.def();
                    if d.name == RUN_CODE && !in_code.is_empty() {
                        d.description.push_str("\n\n");
                        d.description.push_str(&in_code_note(&in_code));
                    }
                    d
                })
                .collect()
        }
        _ => tools.iter().map(|t| t.def()).collect(),
    }
}

/// Whether `load_tools` is offered: only while the listed tools are the model's to call.
pub fn loads(mode: ToolMode, tools: &[Arc<dyn Tool>], deferred: &[Arc<dyn Tool>]) -> bool {
    !deferred.is_empty() && !(mode == ToolMode::Code && tools.iter().any(|t| t.def().name == RUN_CODE))
}

/// The servers' tools as a program calls them: each with its parameters (the required ones first) and the first
/// line of what it does.
fn in_code_note(tools: &[&Arc<dyn Tool>]) -> String {
    let lines: Vec<String> = tools
        .iter()
        .map(|t| {
            let d = t.def();
            let about = d.description.lines().next().unwrap_or("");
            let about = if about.chars().count() > 140 {
                format!("{}…", about.chars().take(140).collect::<String>())
            } else {
                about.to_string()
            };
            let args: Vec<String> = parameters(&d.parameters)
                .into_iter()
                .map(|(p, required)| if required { p } else { format!("{p}=…") })
                .collect();
            format!("- {}({}): {about}", python_name(&t.group(), &d.name), args.join(", "))
        })
        .collect();
    format!(
        "The servers' tools are functions to call from a run_code program, not one by one: write the program, call them with keyword arguments, and print what you need (help(...) in it tells one's parameters in full). The files, search, git and shell tools stay as they are. They are:\n{}",
        lines.join("\n")
    )
}

/// A schema's parameters, the required ones first: each with whether it is required.
fn parameters(schema: &Value) -> Vec<(String, bool)> {
    let required: Vec<&str> =
        schema["required"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut all: Vec<(String, bool)> = schema["properties"]
        .as_object()
        .map(|p| p.keys().map(|k| (k.clone(), required.contains(&k.as_str()))).collect())
        .unwrap_or_default();
    all.sort_by_key(|(_, r)| !r);
    all
}

/// Python's keywords: a name that is one gets a `_` after it.
const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
    "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal",
    "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
];

/// A name as a Python identifier: what is not a letter, digit or `_` becomes `_`.
fn identifier(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    let clean = clean.trim_matches('_');
    let clean = if clean.is_empty() {
        "tool".to_string()
    } else if clean.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{clean}")
    } else {
        clean.to_string()
    };
    if KEYWORDS.contains(&clean.as_str()) { format!("{clean}_") } else { clean }
}

/// A tool's group as the arena module names it: `local` for Code Arena's own; a name the module has for its own
/// (`tools`, `call`, `ToolError`) gets a `_` after it.
fn group_name(group: &str) -> String {
    if group == OWN {
        return "local".into();
    }
    let g = identifier(group);
    if matches!(g.as_str(), "tools" | "call" | "ToolError" | "local") { format!("{g}_") } else { g }
}

/// A function as a program calls it: `arena.<group>.<function>`.
pub fn python_name(group: &str, function: &str) -> String {
    format!("arena.{}.{}", group_name(group), identifier(function))
}

/// What the interpreter is told of the tools a program may call, for the arena module.
fn listing(tools: &[Arc<dyn Tool>]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| {
                let d = t.def();
                json!({
                    "name": d.name,
                    "group": group_name(&t.group()),
                    "function": identifier(&d.name),
                    "call": python_name(&t.group(), &d.name),
                    "description": d.description,
                    "parameters": d.parameters,
                })
            })
            .collect(),
    )
}

/// The tool itself, as the model is given it. It runs only within a turn, which answers the calls its programs
/// make ([`Agent::run_code`]).
pub struct RunCode;

#[async_trait::async_trait]
impl Tool for RunCode {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: RUN_CODE.into(),
            description: format!(
                "Run Python in this chat's own interpreter, as a notebook does: variables, imports and data stay from one call to the next (fresh: true starts anew). It is the project's virtual environment when it has one (.venv, venv), in the working directory and the session's sandbox. What it prints comes back, and the value of a last line that is an expression, an error with its line, and the plots it makes (matplotlib: kept in .code-arena/plots). In it, `import arena` calls this session's tools as functions: arena.tools.search(\"words\") lists them (arena.local.read_file, arena.<server>.<tool>), help(...) tells one's parameters, and each call (keyword arguments) returns the tool's result, a dict or a list when it is JSON; a refused call raises arena.ToolError. Each call is checked as a direct one is (the mode, the rules); at most {MOST_CALLS} in one run. Use it to compute, to explore data, and for many tool calls whose results you only filter or join; print only what you need."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "code": {"type": "string", "description": "The code to run; print() what you need, or end with the expression to show."},
                    "fresh": {"type": "boolean", "description": "Start a new interpreter first: what the earlier calls made is gone."},
                    "timeout_seconds": {"type": "integer", "description": "Stop it after this long (default 120, at most 600): the interpreter is stopped, and its variables with it."}
                },
                "required": ["code"]
            }),
        }
    }

    fn kind(&self) -> Kind {
        Kind::Shell
    }

    fn subject(&self, args: &Value) -> String {
        let code = args["code"].as_str().unwrap_or("");
        let fresh = if args["fresh"] == true { "(fresh) " } else { "" };
        format!("{fresh}{code}")
    }

    async fn run(&self, _args: Value, _ctx: &Context) -> Output {
        Output::error("run_code runs only within a turn, which answers the tools its program calls.")
    }
}

/// The program in the interpreter: it reads each run's code (and the tools it may call) from its own copy of
/// stdin, runs it, and says the run's end on a line that begins with its mark.
const RUNNER: &str = r#"
import ast, json, linecache, os, sys, traceback, types, warnings

requests = os.fdopen(os.dup(0), "r", encoding="utf-8")
null = os.open(os.devnull, os.O_RDONLY)
os.dup2(null, 0)
sys.stdin = open(os.devnull, encoding="utf-8")
os.dup2(1, 2)
sys.stderr = sys.stdout
MARK = os.environ.pop("ARENA_MARK")
PLOTS = os.environ.get("ARENA_PLOTS")
warnings.filterwarnings("ignore", message=".*non-interactive.*")
scope = {"__name__": "__main__", "__builtins__": __builtins__}
sys.path.insert(0, os.getcwd())
cell = 0


def say(message):
    try:
        sys.stdout.flush()
    except Exception:
        pass
    os.write(1, ("\n" + MARK + json.dumps(message) + "\n").encode("utf-8"))


class ToolError(Exception):
    """A call of a tool that was refused, or that failed: its words say why."""


def call(name, **args):
    """Calls one of the session's tools by its name, with its arguments."""
    say({"call": name, "args": args})
    line = requests.readline()
    if not line:
        raise SystemExit(1)
    answer = json.loads(line)
    if not answer.get("ok"):
        raise ToolError(answer.get("error") or "refused")
    result = answer.get("result", "")
    try:
        return json.loads(result)
    except ValueError:
        return result


def function(tool):
    schema = tool.get("parameters") or {}
    props = schema.get("properties") or {}
    required = [p for p in schema.get("required") or [] if p in props]
    order = required + [p for p in props if p not in required]

    def run(*given, **args):
        if len(given) > len(order):
            raise TypeError(f"{tool['call']} takes at most {len(order)} arguments by place")
        for key, value in zip(order, given):
            if key in args:
                raise TypeError(f"{tool['call']} was given {key} twice")
            args[key] = value
        return call(tool["name"], **args)

    lines = []
    for p in order:
        about = props.get(p) or {}
        said = about.get("description") or about.get("type") or ""
        lines.append(f"  {p}{'' if p in required else ' (optional)'}: {said}")
    run.__name__ = tool["function"]
    run.__qualname__ = tool["call"]
    run.__doc__ = (tool.get("description") or "") + ("\n\nParameters:\n" + "\n".join(lines) if lines else "")
    return run


def build(tools):
    arena = types.ModuleType("arena", "This session's tools as functions: arena.tools.search(\"words\") lists them.")
    arena.ToolError = ToolError
    arena.call = call
    for tool in tools:
        group = getattr(arena, tool["group"], None)
        if group is None:
            group = types.SimpleNamespace()
            setattr(arena, tool["group"], group)
        setattr(group, tool["function"], function(tool))

    def search(words=""):
        """The tools whose name or description has every one of these words (none: every tool)."""
        wanted = words.lower().split()
        found = []
        for tool in tools:
            text = (tool["call"] + " " + (tool.get("description") or "")).lower()
            if all(w in text for w in wanted):
                found.append({"call": tool["call"], "about": (tool.get("description") or "").split("\n")[0][:140]})
        return found

    arena.tools = types.SimpleNamespace(search=search)
    sys.modules["arena"] = arena


def plots():
    plt = sys.modules.get("matplotlib.pyplot")
    if plt is None or not PLOTS:
        return []
    saved = []
    for num in plt.get_fignums():
        os.makedirs(PLOTS, exist_ok=True)
        path = os.path.join(PLOTS, f"plot-{os.getpid()}-{cell}-{num}.png")
        plt.figure(num).savefig(path, dpi=100, bbox_inches="tight")
        saved.append(path)
    plt.close("all")
    return saved


for request in requests:
    asked = json.loads(request)
    if "code" not in asked:
        continue
    build(asked.get("tools") or [])
    code = asked["code"]
    cell += 1
    name = f"<call {cell}>"
    linecache.cache[name] = (len(code), None, code.splitlines(True), name)
    ok, code_exit = True, None
    try:
        tree = ast.parse(code, name, "exec")
        last = tree.body.pop() if tree.body and isinstance(tree.body[-1], ast.Expr) else None
        exec(compile(tree, name, "exec"), scope)
        if last is not None:
            value = eval(compile(ast.Expression(last.value), name, "eval"), scope)
            if value is not None:
                scope["_"] = value
                print(repr(value))
    except SystemExit as e:
        code_exit = e.code if isinstance(e.code, int) else (0 if e.code is None else 1)
        ok = code_exit == 0
    except SyntaxError as e:
        ok = False
        traceback.print_exception(type(e), e, None)
    except BaseException as e:
        ok = False
        tb = e.__traceback__
        while tb is not None and not tb.tb_frame.f_code.co_filename.startswith("<call "):
            tb = tb.tb_next
        traceback.print_exception(type(e), e, tb)
    try:
        saved = plots()
    except Exception as e:
        saved = []
        print(f"(the plot could not be saved: {e})")
    say({"done": True, "ok": ok, "exit": code_exit, "plots": saved})
"#;

/// The Python a run starts: the project's virtual environment when it has one, else the system's.
pub fn python(root: &std::path::Path) -> String {
    let inside = if cfg!(windows) { ["Scripts", "python.exe"] } else { ["bin", "python"] };
    for dir in [root.join(".venv"), root.join("venv"), root.join("env")] {
        let p = dir.join(inside[0]).join(inside[1]);
        if p.is_file() && dir.join("pyvenv.cfg").is_file() {
            return p.to_string_lossy().into_owned();
        }
    }
    if cfg!(windows) { "python".into() } else { "python3".into() }
}

/// What a run gave.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ran {
    pub output: String,
    pub ok: bool,
    /// The code `sys.exit` gave: the interpreter goes on.
    pub exit: Option<i64>,
    /// The plots it saved (files).
    pub plots: Vec<String>,
    /// The interpreter ended with it: the next run starts a new one.
    pub ended: bool,
    pub timed_out: bool,
}

/// What the interpreter said next.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// A program calls a tool: answer it with [`Kernel::answer`].
    Call {
        name: String,
        args: Value,
    },
    Done(Ran),
}

/// Output kept up to a bound, and how much more there was.
#[derive(Debug, Default)]
pub struct Bounded {
    text: String,
    /// The characters kept, counted as they come.
    kept: usize,
    most: usize,
    more: usize,
}

impl Bounded {
    pub fn new(most: usize) -> Bounded {
        Bounded { text: String::new(), kept: 0, most, more: 0 }
    }

    pub fn push(&mut self, s: &str) {
        let room = self.most.saturating_sub(self.kept);
        let n = s.chars().count();
        if n <= room {
            self.text.push_str(s);
            self.kept += n;
        } else {
            self.text.extend(s.chars().take(room));
            self.kept += room;
            self.more += n - room;
        }
    }

    pub fn text(&self) -> String {
        let t = self.text.trim_end().to_string();
        if self.more > 0 { format!("{t}\n… ({} more characters, cut)", self.more) } else { t }
    }
}

/// The chat's interpreter.
pub struct Kernel {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    mark: String,
    python: String,
}

impl Kernel {
    /// Starts the interpreter in the working directory, in the session's sandbox.
    pub fn start(ctx: &Context) -> Result<Kernel, String> {
        let root = ctx.workspace.root().to_path_buf();
        let py = python(&root);
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),
        );
        let mark = format!("\u{1}arena-{:016x} ", hasher.finish());
        let (program, args) = ctx.command(&py, &["-u".into(), "-c".into(), RUNNER.into()]);
        let mut cmd = tokio::process::Command::new(&program);
        cmd.args(&args)
            .current_dir(ctx.workspace.cwd())
            .env("ARENA_MARK", mark.trim_end())
            .env("ARENA_PLOTS", root.join(".code-arena").join("plots"))
            .env("MPLBACKEND", "Agg")
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Could not start {py} ({e}): install Python 3, or make the project's .venv."))?;
        let stdin = child.stdin.take().ok_or("the interpreter has no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("the interpreter has no stdout")?);
        Ok(Kernel { child, stdin, stdout, mark: mark.trim_end().to_string(), python: py })
    }

    /// The Python it runs.
    pub fn python(&self) -> &str {
        &self.python
    }

    /// Whether it still runs; its exit code when it has ended.
    pub fn ended(&mut self) -> Option<Option<i32>> {
        match self.child.try_wait() {
            Ok(Some(s)) => Some(s.code()),
            Ok(None) => None,
            Err(_) => Some(None),
        }
    }

    /// Gives it a program to run, with the tools it may call.
    pub async fn begin(&mut self, code: &str, tools: &Value) -> std::io::Result<()> {
        self.line(&json!({"code": code, "tools": tools})).await
    }

    /// Answers the call a program made.
    pub async fn answer(&mut self, answer: &Value) -> std::io::Result<()> {
        self.line(answer).await
    }

    async fn line(&mut self, v: &Value) -> std::io::Result<()> {
        let mut text = v.to_string();
        text.push('\n');
        self.stdin.write_all(text.as_bytes()).await?;
        self.stdin.flush().await
    }

    /// Reads what the program prints into `out` until it calls a tool or its run ends (or `until` comes: the
    /// interpreter is stopped then).
    pub async fn next(&mut self, until: Instant, out: &mut Bounded) -> Step {
        // The line before a message is the runner's own (it begins each on a new line): an empty one is not the
        // program's.
        let mut blank = false;
        loop {
            // Bytes, not text: a command the program runs may print what is not UTF-8.
            let mut bytes = Vec::new();
            let read = tokio::time::timeout_at(until, self.stdout.read_until(b'\n', &mut bytes)).await;
            let line = String::from_utf8_lossy(&bytes).into_owned();
            match read {
                Err(_) => {
                    let _ = self.child.start_kill();
                    return Step::Done(Ran { output: out.text(), ended: true, timed_out: true, ..Ran::default() });
                }
                Ok(Err(_)) | Ok(Ok(0)) => {
                    if blank {
                        out.push("\n");
                    }
                    let _ = self.child.start_kill();
                    return Step::Done(Ran { output: out.text(), ended: true, ..Ran::default() });
                }
                Ok(Ok(_)) => {}
            }
            if let Some(said) = line.strip_prefix(&self.mark) {
                let v: Value = serde_json::from_str(said.trim()).unwrap_or(Value::Null);
                if let Some(name) = v["call"].as_str() {
                    return Step::Call { name: name.to_string(), args: v["args"].clone() };
                }
                if v["done"] == true {
                    return Step::Done(Ran {
                        output: out.text(),
                        ok: v["ok"] == true,
                        exit: v["exit"].as_i64(),
                        plots: v["plots"]
                            .as_array()
                            .map(|p| p.iter().filter_map(Value::as_str).map(str::to_string).collect())
                            .unwrap_or_default(),
                        ended: false,
                        timed_out: false,
                    });
                }
                continue;
            }
            if blank {
                out.push("\n");
            }
            blank = line == "\n";
            if !blank {
                out.push(&line);
            }
        }
    }
}

/// A run's status line, as the model reads it after the output.
fn status(ran: &Ran, secs: u64, exit_code: Option<i32>) -> String {
    if ran.timed_out {
        format!("timed out after {secs} s: the interpreter was stopped, and its variables with it")
    } else if ran.ended {
        let code = exit_code.map(|c| format!(" with exit code {c}")).unwrap_or_default();
        format!("the interpreter ended{code}: the next call starts a new one")
    } else if let Some(e) = ran.exit {
        format!("exit code {e} (sys.exit: the interpreter goes on, its variables kept)")
    } else if ran.ok {
        "ok".into()
    } else {
        "error".into()
    }
}

/// The calls a run made, as its status line lists them: each name once, with how many times.
fn counted(calls: &[String]) -> String {
    let mut seen: Vec<(&str, usize)> = Vec::new();
    for c in calls {
        match seen.iter_mut().find(|(n, _)| n == c) {
            Some((_, k)) => *k += 1,
            None => seen.push((c, 1)),
        }
    }
    seen.iter().map(|(n, k)| if *k == 1 { n.to_string() } else { format!("{n} ×{k}") }).collect::<Vec<_>>().join(", ")
}

/// What the model reads of a run.
fn result_text(
    anew: &str,
    ran: &Ran,
    shown_plots: &[String],
    calls: &[String],
    secs: u64,
    code: Option<i32>,
) -> String {
    let mut text = anew.to_string();
    if ran.output.is_empty() {
        text.push_str("(no output)\n");
    } else {
        text.push_str(&ran.output);
        text.push('\n');
    }
    if !shown_plots.is_empty() {
        let what = if shown_plots.len() == 1 { "plot" } else { "plots" };
        text.push_str(&format!("[{what} kept as {}]\n", shown_plots.join(", ")));
    }
    text.push('[');
    text.push_str(&status(ran, secs, code));
    if !calls.is_empty() {
        text.push_str("; calls: ");
        text.push_str(&counted(calls));
    }
    text.push(']');
    text
}

impl Agent {
    /// The tools a program may call now: those sent and those listed, but those that may not be called from code.
    fn code_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.iter().chain(self.deferred.iter()).filter(|t| callable(t.as_ref())).cloned().collect()
    }

    /// Runs a `run_code` call (allowed already): its program in the chat's interpreter, each tool it calls checked
    /// as a direct call is.
    pub(crate) async fn run_code(&mut self, call_id: &str, args: &Value, events: &mut dyn Events) -> Output {
        let Some(code) = args["code"].as_str().filter(|c| !c.trim().is_empty()) else {
            return Output::error("code is required: the Python to run.");
        };
        let fresh = args["fresh"] == true;
        // Taken out while it runs: a turn stopped meanwhile drops it, and the interpreter with it.
        let mut kernel = self.kernel.take();
        let mut anew = String::new();
        if fresh {
            kernel = None;
        } else if let Some(k) = kernel.as_mut()
            && let Some(code) = k.ended()
        {
            let was = code.map(|c| format!(" with exit code {c}")).unwrap_or_default();
            anew = format!(
                "(The earlier interpreter had ended{was}: this is a new one, and what the earlier calls made is gone.)\n"
            );
            kernel = None;
        }
        let mut kernel = match kernel {
            Some(k) => k,
            None => match Kernel::start(&self.ctx) {
                Ok(k) => k,
                Err(e) => return Output::error(e),
            },
        };
        let tools = self.code_tools();
        if let Err(e) = kernel.begin(code, &listing(&tools)).await {
            return Output::error(format!("{} did not take the program ({e}).", kernel.python()));
        }
        let secs = args["timeout_seconds"].as_u64().unwrap_or(120).clamp(1, 600);
        let mut until = Instant::now() + Duration::from_secs(secs);
        let mut out = Bounded::new(MOST_CHARS);
        let mut calls: Vec<String> = Vec::new();
        let mut wrote = Vec::new();
        let ran = loop {
            match kernel.next(until, &mut out).await {
                Step::Done(r) => break r,
                Step::Call { name, args } => {
                    let began = Instant::now();
                    let answer =
                        self.answer_from_code(call_id, &tools, calls.len(), &name, args, events, &mut wrote).await;
                    calls.push(name);
                    // The time the person took to answer is not the program's.
                    until += began.elapsed();
                    if kernel.answer(&answer).await.is_err() {
                        continue;
                    }
                }
            }
        };
        let exit_code = if ran.ended { kernel.ended().flatten() } else { None };
        let root = self.ctx.workspace.root().to_path_buf();
        let shown: Vec<String> = ran
            .plots
            .iter()
            .filter(|p| std::path::Path::new(p).is_file())
            .map(|p| {
                std::path::Path::new(p)
                    .strip_prefix(&root)
                    .map(|r| r.display().to_string())
                    .unwrap_or_else(|_| p.clone())
            })
            .collect();
        if !ran.ended {
            self.kernel = Some(kernel);
        }
        let text = result_text(&anew, &ran, &shown, &calls, secs, exit_code);
        Output { text, is_error: !ran.ok, wrote }
    }

    /// A tool a program called: checked as a direct call is (the hooks, the mode, the rules, the question), and run
    /// with the context of the `run_code` call. The answer the program reads.
    #[allow(clippy::too_many_arguments)]
    async fn answer_from_code(
        &mut self,
        call_id: &str,
        tools: &[Arc<dyn Tool>],
        made: usize,
        name: &str,
        args: Value,
        events: &mut dyn Events,
        wrote: &mut Vec<std::path::PathBuf>,
    ) -> Value {
        let refused = |why: String| json!({"ok": false, "error": why});
        if made >= MOST_CALLS {
            return refused(format!("A run makes at most {MOST_CALLS} calls."));
        }
        let Some(tool) = tools.iter().find(|t| t.def().name == name).cloned() else {
            return refused(format!("There is no tool {name} to call from code (arena.tools.search finds them)."));
        };
        let mut args = if args.is_object() { args } else { json!({}) };
        let pre = Self::pre_tool(&self.hooks.clone(), name, &args, events).await;
        if let Some(why) = pre.block {
            return refused(format!("A hook blocked this call: {why}"));
        }
        if let Some(u) = pre.updated_input {
            args = u;
        }
        let subject = tool.subject(&args);
        let said = Call { id: call_id, name, subject: &subject, arguments: &args };
        if let Err(why) = self.allow(tool.kind(), &said, pre.permission.as_deref(), events).await {
            return refused(why);
        }
        let line: String = subject.lines().next().unwrap_or("").chars().take(70).collect();
        events.tool_progress(call_id, (made + 1) as f64, None, &format!("↳ {name} {line}"));
        let mut ctx = self.ctx.clone();
        ctx.call = Some(call_id.to_string());
        let o = tool.run(args.clone(), &ctx).await;
        let o = Self::post_tool(&self.hooks.clone(), name, &args, o, events).await;
        for w in &o.wrote {
            if !wrote.contains(w) {
                wrote.push(w.clone());
            }
        }
        if o.is_error { refused(o.text) } else { json!({"ok": true, "result": o.text}) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        name: &'static str,
        group: &'static str,
    }

    #[async_trait::async_trait]
    impl Tool for Fake {
        fn def(&self) -> ToolDef {
            ToolDef {
                name: self.name.into(),
                description: format!("Does {}.\nMore words.", self.name),
                parameters: json!({"type": "object", "properties": {"limit": {"type": "integer"}, "query": {"type": "string"}}, "required": ["query"]}),
            }
        }
        fn kind(&self) -> Kind {
            Kind::Read
        }
        fn subject(&self, args: &Value) -> String {
            args.to_string()
        }
        fn group(&self) -> String {
            self.group.into()
        }
        async fn run(&self, args: Value, _ctx: &Context) -> Output {
            Output::ok(json!({"echo": args}).to_string())
        }
    }

    fn tools(with_code: bool) -> Vec<Arc<dyn Tool>> {
        let mut t: Vec<Arc<dyn Tool>> = vec![
            Arc::new(Fake { name: "read_file", group: OWN }),
            Arc::new(Fake { name: "web_search", group: "arena" }),
            Arc::new(Fake { name: "find-symbol", group: "code-intel" }),
        ];
        if with_code {
            t.push(Arc::new(RunCode));
        }
        t
    }

    fn names(defs: &[ToolDef]) -> Vec<&str> {
        defs.iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn the_mode_is_the_persons_else_arenas_else_both() {
        assert_eq!(ToolMode::of(Some("code"), Some("direct")), ToolMode::Code);
        assert_eq!(ToolMode::of(None, Some("direct")), ToolMode::Direct);
        assert_eq!(ToolMode::of(None, None), ToolMode::Both);
        assert_eq!(ToolMode::of(Some("sideways"), None), ToolMode::Both);
        for m in ToolMode::ALL {
            assert_eq!(ToolMode::parse(m.name()), Some(m));
        }
    }

    #[test]
    fn direct_has_no_run_code_and_both_has_everything() {
        let t = tools(true);
        assert_eq!(names(&offered(ToolMode::Direct, &t, &[])), ["read_file", "web_search", "find-symbol"]);
        assert_eq!(names(&offered(ToolMode::Both, &t, &[])), ["read_file", "web_search", "find-symbol", "run_code"]);
    }

    #[test]
    fn as_code_the_servers_tools_are_functions_named_in_run_codes_description() {
        let t = tools(true);
        let defs = offered(ToolMode::Code, &t, &[]);
        assert_eq!(names(&defs), ["read_file", "run_code"]);
        let about = &defs[1].description;
        assert!(about.contains("- arena.arena.web_search(query, limit=…): Does web_search."), "{about}");
        assert!(about.contains("- arena.code_intel.find_symbol(query, limit=…)"), "{about}");
        assert!(!about.contains("read_file("), "Code Arena's own stay direct: {about}");
        // Without run_code to call them, code mode keeps them direct; listed tools are loaded only while the model
        // calls them itself.
        assert_eq!(names(&offered(ToolMode::Code, &tools(false), &[])).len(), 3);
        let listed: Vec<Arc<dyn Tool>> = vec![Arc::new(Fake { name: "deep", group: "x" })];
        assert!(!loads(ToolMode::Code, &t, &listed));
        assert!(loads(ToolMode::Both, &t, &listed));
        assert!(!loads(ToolMode::Both, &t, &[]));
    }

    #[test]
    fn names_are_python_identifiers() {
        assert_eq!(python_name(OWN, "read_file"), "arena.local.read_file");
        assert_eq!(python_name("code-intel", "find-symbol"), "arena.code_intel.find_symbol");
        assert_eq!(python_name("tools", "class"), "arena.tools_.class_");
        assert_eq!(python_name("9lives", "--"), "arena._9lives.tool");
    }

    #[test]
    fn a_program_calls_neither_run_code_nor_what_waits_on_commands() {
        assert!(!callable(&RunCode));
        assert!(!callable(&Fake { name: "command_output", group: OWN }));
        assert!(callable(&Fake { name: "read_file", group: OWN }));
    }

    #[test]
    fn output_is_kept_up_to_its_bound() {
        let mut b = Bounded::new(5);
        b.push("abc");
        b.push("defgh");
        assert_eq!(b.text(), "abcde\n… (3 more characters, cut)");
    }

    #[test]
    fn the_status_line_says_how_it_went_and_the_calls() {
        let ran = Ran { output: "4".into(), ok: true, ..Ran::default() };
        let calls = ["read_file".to_string(), "grep".into(), "read_file".into()];
        assert_eq!(result_text("", &ran, &[], &calls, 120, None), "4\n[ok; calls: read_file ×2, grep]");
        let ran = Ran { timed_out: true, ended: true, ..Ran::default() };
        assert_eq!(
            result_text("", &ran, &[], &[], 5, None),
            "(no output)\n[timed out after 5 s: the interpreter was stopped, and its variables with it]"
        );
        let ran = Ran { exit: Some(2), ..Ran::default() };
        assert!(result_text("", &ran, &["p.png".into()], &[], 5, None).contains("[plot kept as p.png]\n[exit code 2"));
    }

    fn have_python() -> bool {
        std::process::Command::new("python3").arg("-c").arg("pass").status().is_ok_and(|s| s.success())
    }

    /// The interpreter keeps its variables, says a last expression's value, and calls the tools through the mark.
    #[tokio::test]
    async fn the_kernel_runs_keeps_and_calls() {
        if !have_python() {
            return;
        }
        let dir = tempfile::tempdir().expect("the kernel");
        let ws = ca_workspace::Workspace::open(dir.path(), &[]).expect("the kernel");
        let ctx = Context::new(Arc::new(ws), None, Duration::from_secs(30));
        let mut k = Kernel::start(&ctx).expect("the kernel");
        let t = listing(&tools(false));
        let until = Instant::now() + Duration::from_secs(20);
        k.begin("x = 40\nprint('hi')\nx + 2", &t).await.expect("the kernel");
        let mut out = Bounded::new(1000);
        let Step::Done(r) = k.next(until, &mut out).await else { panic!("a call") };
        assert_eq!((r.output.as_str(), r.ok), ("hi\n42", true));
        k.begin("import arena\nr = arena.arena.web_search('cats', limit=2)\nprint(r['echo']['query'])", &t)
            .await
            .expect("the kernel");
        let mut out = Bounded::new(1000);
        let step = k.next(until, &mut out).await;
        assert_eq!(step, Step::Call { name: "web_search".into(), args: json!({"query": "cats", "limit": 2}) });
        k.answer(&json!({"ok": true, "result": "{\"echo\": {\"query\": \"cats\"}}"})).await.expect("the kernel");
        let Step::Done(r) = k.next(until, &mut out).await else { panic!("another call") };
        assert_eq!(r.output, "cats");
        k.begin("import arena\ntry:\n    arena.local.read_file(query='a')\nexcept arena.ToolError as e:\n    print('no:', e)\nx", &t)
            .await
            .expect("the kernel");
        let mut out = Bounded::new(1000);
        assert!(matches!(k.next(until, &mut out).await, Step::Call { .. }));
        k.answer(&json!({"ok": false, "error": "refused"})).await.expect("the kernel");
        let Step::Done(r) = k.next(until, &mut out).await else { panic!("a call") };
        assert_eq!(r.output, "no: refused\n40");
        k.begin("1/0", &t).await.expect("the kernel");
        let mut out = Bounded::new(1000);
        let Step::Done(r) = k.next(until, &mut out).await else { panic!("a call") };
        assert!(!r.ok && r.output.contains("ZeroDivisionError") && !r.output.contains("RUNNER"), "{}", r.output);
    }
}
