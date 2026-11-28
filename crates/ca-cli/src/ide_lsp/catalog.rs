//! The language servers Code Arena knows, the person's own from the `languageServers` setting, and where each is
//! found on this computer: in the project's virtual environment for Python's, in PATH's full folders, then where
//! their usual installs put them (GOBIN, ~/go/bin, ~/.cargo/bin, ~/.dotnet/tools, clangd-NN). Nothing is installed:
//! a server that is missing says how to get it.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use ca_tools::formatters::{self, Gate, Plan, Runner};
use serde_json::{Map, Value, json};

/// How long a server has to say its version.
const VERSION_LIMIT: Duration = Duration::from_secs(5);

/// How the web language servers are installed.
const WEB: &str = "npm install -g vscode-langservers-extracted";

/// One of the language servers Code Arena knows.
pub struct Kind {
    pub id: &'static str,
    pub name: &'static str,
    /// The languages it serves (the editor's language ids).
    pub languages: &'static [&'static str],
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// What its environment gets: nothing fetched (no Go modules, no crates).
    pub env: &'static [(&'static str, &'static str)],
    /// How a person installs it.
    pub install: &'static str,
    /// The arguments that make it say its version (none: it is not asked).
    pub version: Option<&'static [&'static str]>,
}

/// The built-in servers, in the order they are offered for a language.
pub const CATALOG: &[Kind] = &[
    Kind {
        id: "typescript",
        name: "typescript-language-server",
        languages: &["typescript", "javascript", "typescriptreact", "javascriptreact"],
        program: "typescript-language-server",
        args: &["--stdio"],
        env: &[],
        install: "npm install -g typescript-language-server typescript",
        version: Some(&["--version"]),
    },
    Kind {
        id: "pyright",
        name: "Pyright",
        languages: &["python"],
        program: "pyright-langserver",
        args: &["--stdio"],
        env: &[],
        install: "npm install -g pyright (or pip install pyright)",
        version: None,
    },
    Kind {
        id: "basedpyright",
        name: "basedpyright",
        languages: &["python"],
        program: "basedpyright-langserver",
        args: &["--stdio"],
        env: &[],
        install: "pip install basedpyright",
        version: None,
    },
    Kind {
        id: "pylsp",
        name: "Python LSP Server",
        languages: &["python"],
        program: "pylsp",
        args: &[],
        env: &[],
        install: "pip install python-lsp-server (in the project's virtual environment, or for the python3 on PATH)",
        version: None,
    },
    Kind {
        id: "gopls",
        name: "gopls",
        languages: &["go"],
        program: "gopls",
        args: &[],
        env: &[("GOPROXY", "off")],
        install: "go install golang.org/x/tools/gopls@latest (it lands in ~/go/bin)",
        version: Some(&["version"]),
    },
    Kind {
        id: "rust-analyzer",
        name: "rust-analyzer",
        languages: &["rust"],
        program: "rust-analyzer",
        args: &[],
        env: &[("CARGO_NET_OFFLINE", "true")],
        install: "rustup component add rust-analyzer",
        version: Some(&["--version"]),
    },
    Kind {
        id: "clangd",
        name: "clangd",
        languages: &["c", "cpp"],
        program: "clangd",
        args: &[],
        env: &[],
        install: if cfg!(windows) {
            "winget install LLVM.LLVM"
        } else if cfg!(target_os = "macos") {
            "brew install llvm"
        } else {
            "apt install clangd (dnf install clang-tools-extra)"
        },
        version: Some(&["--version"]),
    },
    Kind {
        id: "csharp-ls",
        name: "csharp-ls",
        languages: &["csharp"],
        program: "csharp-ls",
        args: &[],
        env: &[],
        install: "dotnet tool install --global csharp-ls",
        version: None,
    },
    Kind {
        id: "json",
        name: "vscode-json-language-server",
        languages: &["json", "jsonc"],
        program: "vscode-json-language-server",
        args: &["--stdio"],
        env: &[],
        install: WEB,
        version: None,
    },
    Kind {
        id: "css",
        name: "vscode-css-language-server",
        languages: &["css", "scss", "less"],
        program: "vscode-css-language-server",
        args: &["--stdio"],
        env: &[],
        install: WEB,
        version: None,
    },
    Kind {
        id: "html",
        name: "vscode-html-language-server",
        languages: &["html"],
        program: "vscode-html-language-server",
        args: &["--stdio"],
        env: &[],
        install: WEB,
        version: None,
    },
    Kind {
        id: "yaml",
        name: "yaml-language-server",
        languages: &["yaml"],
        program: "yaml-language-server",
        args: &["--stdio"],
        env: &[],
        install: "npm install -g yaml-language-server",
        version: None,
    },
    Kind {
        id: "bash",
        name: "bash-language-server",
        languages: &["shellscript"],
        program: "bash-language-server",
        args: &["start"],
        env: &[],
        install: "npm install -g bash-language-server",
        version: None,
    },
];

/// A built-in server by its id.
pub fn kind(id: &str) -> Option<&'static Kind> {
    CATALOG.iter().find(|k| k.id == id)
}

/// A server as the `languageServers` setting names it: the command that starts it (the program, then its
/// arguments), the languages it serves, what its initialize gets as `initializationOptions`, the settings it is
/// answered with, and what its environment adds (null takes a variable away); `disabled` turns a built-in one off.
/// An entry named after a built-in changes only what it gives. An entry that cannot be used keeps what is wrong.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Own {
    pub command: Vec<String>,
    pub languages: Option<Vec<String>>,
    pub options: Option<Map<String, Value>>,
    pub settings: Option<Map<String, Value>>,
    pub env: Vec<(String, Option<String>)>,
    pub disabled: bool,
    pub problem: Option<String>,
}

impl Own {
    pub fn of(v: &Value) -> Own {
        let broken = |problem: &str| Own { problem: Some(problem.to_string()), ..Own::default() };
        let shape = "give \"command\" (the program and its arguments), \"languages\" (the editor's language ids), and \
                     if it needs them \"initializationOptions\", \"settings\" and \"env\" (objects)";
        let Some(o) = v.as_object() else { return broken(shape) };
        let strings = |v: &Value| -> Option<Vec<String>> {
            v.as_array()?.iter().map(|x| x.as_str().filter(|s| !s.is_empty()).map(String::from)).collect()
        };
        let command = match o.get("command") {
            None | Some(Value::Null) => Some(Vec::new()),
            Some(c) => strings(c),
        };
        let languages = match o.get("languages") {
            None | Some(Value::Null) => None,
            Some(l) => match strings(l) {
                Some(l) => Some(l),
                None => return broken(shape),
            },
        };
        let Some(command) = command else { return broken(shape) };
        let object = |k: &str| o.get(k).filter(|v| !v.is_null());
        if ["initializationOptions", "settings", "env"].iter().any(|k| object(k).is_some_and(|v| !v.is_object())) {
            return broken("\"initializationOptions\", \"settings\" and \"env\" are objects");
        }
        let mut env = Vec::new();
        for (name, value) in object("env").and_then(Value::as_object).into_iter().flatten() {
            match value {
                Value::Null => env.push((name.clone(), None)),
                Value::String(s) => env.push((name.clone(), Some(s.clone()))),
                _ => return broken(&format!("\"env\".\"{name}\" is a string (or null, to take it away)")),
            }
        }
        Own {
            command,
            languages,
            options: object("initializationOptions").and_then(Value::as_object).cloned(),
            settings: object("settings").and_then(Value::as_object).cloned(),
            env,
            disabled: o.get("disabled").and_then(Value::as_bool) == Some(true),
            problem: None,
        }
    }
}

/// A server as the setting and the catalog make it.
pub struct Entry {
    pub id: String,
    pub name: String,
    pub languages: Vec<String>,
    pub kind: Option<&'static Kind>,
    pub own: Option<Own>,
}

/// The setting's own servers first, in its order (they come before the built-in ones for their languages), then
/// the catalog's with the setting's entry of each over it.
pub fn entries(setting: &Value) -> Vec<Entry> {
    let given: Vec<(String, Own)> =
        setting.as_object().into_iter().flatten().map(|(id, v)| (id.clone(), Own::of(v))).collect();
    let mut all: Vec<Entry> = given
        .iter()
        .filter(|(id, _)| kind(id).is_none())
        .map(|(id, own)| Entry {
            id: id.clone(),
            name: id.clone(),
            languages: own.languages.clone().unwrap_or_default(),
            kind: None,
            own: Some(own.clone()),
        })
        .collect();
    for k in CATALOG {
        let own = given.iter().find(|(id, _)| id == k.id).map(|(_, o)| o.clone());
        all.push(Entry {
            id: k.id.to_string(),
            name: k.name.to_string(),
            languages: own
                .as_ref()
                .and_then(|o| o.languages.clone())
                .unwrap_or_else(|| k.languages.iter().map(|l| l.to_string()).collect()),
            kind: Some(k),
            own,
        });
    }
    all
}

/// How a person gets a server, for the window to show when it is missing.
pub fn install(id: &str) -> String {
    match kind(id) {
        Some(k) => {
            let args: String = k.args.iter().map(|a| format!(", \"{a}\"")).collect();
            format!(
                "{}, or name it in the languageServers setting: {{\"{id}\": {{\"command\": [\"/path/to/{}\"{args}]}}}}.",
                k.install, k.program
            )
        }
        None => format!(
            "Give its command in the languageServers setting: {{\"{id}\": {{\"command\": [...], \"languages\": [...]}}}}."
        ),
    }
}

/// Where programs are looked for: the folder, PATH and the variables the usual installs go by.
pub struct Look<'a> {
    pub root: &'a Path,
    pub home: Option<&'a Path>,
    pub env: &'a [(OsString, OsString)],
}

impl Look<'_> {
    fn var(&self, name: &str) -> Option<&OsString> {
        self.env.iter().find(|(k, v)| k.to_str() == Some(name) && !v.is_empty()).map(|(_, v)| v)
    }

    /// PATH's folders, the full ones only: "." or "bin" in PATH would find the project's own program of that name.
    fn path_folders(&self) -> Vec<PathBuf> {
        self.var("PATH").map(|p| std::env::split_paths(p).filter(|d| d.is_absolute()).collect()).unwrap_or_default()
    }

    /// A program as a full path: as given when it is one, else found on PATH; a relative path is none.
    pub fn locate(&self, program: &str) -> Option<PathBuf> {
        if program.contains('/') || program.contains('\\') || Path::new(program).is_absolute() {
            let p = Path::new(program);
            return (p.is_absolute() && p.is_file()).then(|| p.to_path_buf());
        }
        formatters::on_path(program, self.var("PATH"))
    }

    /// Where a built-in server may be, in the order looked: the project's virtual environment for Python's, PATH,
    /// then the usual installs.
    pub fn candidates(&self, k: &Kind) -> Vec<PathBuf> {
        let exe = |name: &str| if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
        let home = self.home.map(Path::to_path_buf);
        let python = matches!(k.id, "pyright" | "basedpyright" | "pylsp");
        let known: Vec<PathBuf> = match k.id {
            _ if python => {
                let venvs = [
                    self.var("VIRTUAL_ENV").map(PathBuf::from),
                    Some(self.root.join(".venv")),
                    Some(self.root.join("venv")),
                ];
                let bin = if cfg!(windows) { "Scripts" } else { "bin" };
                venvs.into_iter().flatten().map(|v| v.join(bin).join(exe(k.program))).collect()
            }
            "gopls" => [
                self.var("GOBIN").map(PathBuf::from),
                self.var("GOPATH").map(|g| PathBuf::from(g).join("bin")),
                home.as_ref().map(|h| h.join("go").join("bin")),
            ]
            .into_iter()
            .flatten()
            .map(|d| d.join(exe(k.program)))
            .collect(),
            "rust-analyzer" => home.iter().map(|h| h.join(".cargo").join("bin").join(exe(k.program))).collect(),
            "csharp-ls" => home.iter().map(|h| h.join(".dotnet").join("tools").join(exe(k.program))).collect(),
            "clangd" => {
                let mut v = self.versioned(k.program);
                v.extend(
                    [
                        "/opt/homebrew/opt/llvm/bin/clangd",
                        "/usr/local/opt/llvm/bin/clangd",
                        r"C:\Program Files\LLVM\bin\clangd.exe",
                    ]
                    .map(PathBuf::from),
                );
                v
            }
            _ => Vec::new(),
        };
        let on_path = formatters::on_path(k.program, self.var("PATH"));
        // The project's own environment first (it is the project's Python); the rest after PATH.
        let all: Vec<PathBuf> = if python {
            known.into_iter().chain(on_path).collect()
        } else {
            on_path.into_iter().chain(known).collect()
        };
        all.into_iter().filter(|p| p.is_file()).collect()
    }

    /// clangd-19, clangd-18 … on PATH, as LLVM's Debian packages name them, the newest first.
    fn versioned(&self, name: &str) -> Vec<PathBuf> {
        let mut found: Vec<(u32, PathBuf)> = Vec::new();
        for d in self.path_folders() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let file = e.file_name().to_string_lossy().into_owned();
                if let Some(n) = file.strip_prefix(name).and_then(|r| r.strip_prefix('-')).and_then(|n| n.parse().ok())
                {
                    found.push((n, e.path()));
                }
            }
        }
        found.sort_by_key(|a| std::cmp::Reverse(a.0));
        found.into_iter().map(|(_, p)| p).collect()
    }
}

/// A server found here, ready to start.
#[derive(Debug, Clone)]
pub struct Found {
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// What its environment adds (None takes a variable away).
    pub env: Vec<(String, Option<String>)>,
    /// What its initialize gets as `initializationOptions`.
    pub options: Map<String, Value>,
    /// The settings it is answered with (`workspace/configuration`).
    pub settings: Map<String, Value>,
    /// Whether the setting named its command.
    pub given: bool,
}

/// A server as found here, or why there is none.
pub fn find(e: &Entry, look: &Look) -> Result<Found, String> {
    let own = e.own.as_ref();
    if let Some(problem) = own.and_then(|o| o.problem.as_deref()) {
        return Err(format!("\"languageServers\".\"{}\" in the settings: {problem}.", e.id));
    }
    if own.is_some_and(|o| o.disabled) {
        return Err(format!("Turned off in the settings (\"languageServers\".\"{}\".\"disabled\").", e.id));
    }
    if e.languages.is_empty() {
        return Err(format!(
            "\"languageServers\".\"{}\" in the settings names no \"languages\" (the editor's language ids: \"lua\", \"java\"…).",
            e.id
        ));
    }
    let given = own.is_some_and(|o| !o.command.is_empty());
    let (program, args) = if let Some(o) = own.filter(|_| given) {
        match look.locate(&o.command[0]) {
            Some(p) => (p, o.command[1..].to_vec()),
            None => {
                return Err(format!(
                    "{} (\"languageServers\" in the settings) is not there: give its full path, or a name on PATH.",
                    o.command[0]
                ));
            }
        }
    } else if let Some(k) = e.kind {
        match look.candidates(k).into_iter().next() {
            Some(p) => (p, k.args.iter().map(|a| a.to_string()).collect()),
            None => {
                let venv = if matches!(k.id, "pyright" | "basedpyright" | "pylsp") {
                    " and in the project's .venv"
                } else {
                    ""
                };
                return Err(format!("{} is not installed (looked for on PATH{venv}).", k.program));
            }
        }
    } else {
        return Err(format!("\"languageServers\".\"{}\" in the settings has no \"command\".", e.id));
    };
    let mut env: Vec<(String, Option<String>)> =
        e.kind.map(|k| k.env.iter().map(|(n, v)| (n.to_string(), Some(v.to_string()))).collect()).unwrap_or_default();
    for (name, value) in own.map(|o| o.env.clone()).unwrap_or_default() {
        env.retain(|(n, _)| *n != name);
        env.push((name, value));
    }
    let mut options = defaults_options(e.kind.map(|k| k.id), look.root);
    merge(&mut options, own.and_then(|o| o.options.as_ref()));
    let mut settings = default_settings(e.kind.map(|k| k.id));
    merge(&mut settings, own.and_then(|o| o.settings.as_ref()));
    Ok(Found { name: e.name.clone(), program, args, env, options, settings, given })
}

/// What a built-in server's initialize gets: typescript-language-server never downloads type packages (VS Code's
/// automatic type acquisition) and uses the project's own TypeScript when the folder has one; the JSON server
/// reads only files itself.
fn defaults_options(id: Option<&str>, root: &Path) -> Map<String, Value> {
    let v = match id {
        Some("typescript") => {
            let mut o = json!({"disableAutomaticTypingAcquisition": true});
            let tsserver = root.join("node_modules").join("typescript").join("lib").join("tsserver.js");
            if tsserver.is_file() {
                o["tsserver"] = json!({"path": tsserver.display().to_string()});
            }
            o
        }
        Some("json") => json!({"provideFormatter": true, "handledSchemaProtocols": ["file"]}),
        _ => json!({}),
    };
    v.as_object().cloned().unwrap_or_default()
}

/// The settings a built-in server is answered with before the person's: the YAML server never fetches the schema
/// store's catalog.
fn default_settings(id: Option<&str>) -> Map<String, Value> {
    let v = match id {
        Some("yaml") => json!({"yaml": {"schemaStore": {"enable": false}}}),
        _ => json!({}),
    };
    v.as_object().cloned().unwrap_or_default()
}

/// One object's keys over another's, objects within merged the same way.
pub fn merge(into: &mut Map<String, Value>, over: Option<&Map<String, Value>>) {
    for (k, v) in over.into_iter().flatten() {
        match (into.get_mut(k), v) {
            (Some(Value::Object(there)), Value::Object(inner)) => merge(there, Some(inner)),
            _ => {
                into.insert(k.clone(), v.clone());
            }
        }
    }
}

/// What the programs said their versions are, by program and its time, asked once for as long as it is unchanged.
#[derive(Default)]
pub struct Versions(Mutex<HashMap<String, Result<Option<String>, String>>>);

impl Versions {
    /// A built-in server's version, or why its program cannot run (a rustup proxy whose component is missing).
    /// None of the setting's own and none in the folder is asked: nothing of the folder's runs to list them. It is
    /// asked in Code Arena's data folder, not the project's.
    pub fn of(
        &self,
        f: &Found,
        kind: Option<&Kind>,
        root: &Path,
        data: &Path,
        env: &[(OsString, OsString)],
    ) -> Result<Option<String>, String> {
        let Some(args) = kind.and_then(|k| k.version).filter(|_| !f.given && !f.program.starts_with(root)) else {
            return Ok(None);
        };
        let time = std::fs::metadata(&f.program).and_then(|m| m.modified()).ok();
        let key = format!("{}\n{time:?}", f.program.display());
        if let Some(known) = self.0.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return known.clone();
        }
        let _ = std::fs::create_dir_all(data);
        let gate = Gate::default();
        let runner = Runner { sandbox: None, env: env.to_vec(), gate: &gate };
        let plan = Plan {
            name: f.name.clone(),
            program: f.program.clone(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: data.to_path_buf(),
            timeout: VERSION_LIMIT,
            mirror: None,
            copies: Vec::new(),
            readable: vec![f.program.clone()],
        };
        let got = match runner.run(&plan, "") {
            Ok(text) => Ok(formatters::version_in(&text)),
            Err(e) if e.kind == "timeout" => Ok(None),
            Err(e) => Err(format!("{} cannot run: {}", f.name, e.message)),
        };
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(key, got.clone());
        got
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
        std::fs::write(path, "#!/bin/sh\n").expect("w");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("mode");
        }
    }

    fn env(pairs: &[(&str, &Path)]) -> Vec<(OsString, OsString)> {
        pairs.iter().map(|(k, v)| (OsString::from(k), v.as_os_str().to_owned())).collect()
    }

    #[cfg(unix)]
    #[test]
    fn a_server_is_found_in_the_venv_first_then_on_path_then_where_its_install_puts_it() {
        let d = tempfile::tempdir().expect("temp");
        let (root, bin, home) = (d.path().join("w"), d.path().join("bin"), d.path().join("home"));
        std::fs::create_dir_all(&root).expect("root");
        program(&bin.join("pylsp"));
        program(&root.join(".venv/bin/pylsp"));
        program(&home.join("go/bin/gopls"));
        program(&bin.join("clangd-14"));
        program(&bin.join("clangd-18"));
        let vars = env(&[("PATH", &bin)]);
        let look = Look { root: &root, home: Some(&home), env: &vars };
        let all = entries(&Value::Null);
        let by = |id: &str| all.iter().find(|e| e.id == id).expect("entry");
        assert_eq!(find(by("pylsp"), &look).expect("pylsp").program, root.join(".venv/bin/pylsp"));
        assert_eq!(find(by("gopls"), &look).expect("gopls").program, home.join("go/bin/gopls"));
        assert_eq!(find(by("gopls"), &look).expect("gopls").env, [("GOPROXY".to_string(), Some("off".to_string()))]);
        assert_eq!(find(by("clangd"), &look).expect("clangd").program, bin.join("clangd-18"), "the newest clangd-NN");
        let missing = find(by("rust-analyzer"), &look).expect_err("not installed");
        assert_eq!(missing, "rust-analyzer is not installed (looked for on PATH).");
        assert!(install("rust-analyzer").starts_with("rustup component add rust-analyzer, or name it"));
        // A relative PATH folder would find the project's own program: it is not looked in.
        let rel = vec![(OsString::from("PATH"), OsString::from("bin"))];
        let look = Look { root: d.path(), home: None, env: &rel };
        assert!(find(by("rust-analyzer"), &look).is_err());
    }

    #[test]
    fn the_setting_adds_servers_before_the_catalog_changes_a_built_in_and_says_what_is_wrong() {
        let d = tempfile::tempdir().expect("temp");
        let setting = json!({
            "lua": {"command": ["/no/such/lua-ls"], "languages": ["lua"]},
            "nolang": {"command": ["x"]},
            "broken": {"command": "lua-ls"},
            "yaml": {"settings": {"yaml": {"format": {"enable": true}}}, "env": {"NODE_OPTIONS": null}},
            "bash": {"disabled": true},
        });
        let all = entries(&setting);
        assert_eq!(all.iter().take(3).map(|e| e.id.as_str()).collect::<Vec<_>>(), ["lua", "nolang", "broken"]);
        assert_eq!(all.len(), 3 + CATALOG.len());
        let vars = Vec::new();
        let look = Look { root: d.path(), home: None, env: &vars };
        let by = |id: &str| all.iter().find(|e| e.id == id).expect("entry");
        assert!(find(by("lua"), &look).expect_err("missing").contains("give its full path"));
        assert!(find(by("nolang"), &look).expect_err("no languages").contains("names no \"languages\""));
        assert!(find(by("broken"), &look).expect_err("broken").contains("give \"command\""));
        assert!(find(by("bash"), &look).expect_err("off").starts_with("Turned off in the settings"));
        // A built-in's own entry changes only what it gives: its settings merged over the defaults.
        let yaml = by("yaml");
        assert_eq!(yaml.languages, ["yaml"]);
        let own = yaml.own.as_ref().expect("own");
        assert_eq!(own.env, [("NODE_OPTIONS".to_string(), None)]);
        let mut s = default_settings(Some("yaml"));
        merge(&mut s, own.settings.as_ref());
        assert_eq!(Value::Object(s), json!({"yaml": {"schemaStore": {"enable": false}, "format": {"enable": true}}}));
    }

    #[cfg(unix)]
    #[test]
    fn a_command_of_the_setting_is_found_with_its_arguments_and_options() {
        let d = tempfile::tempdir().expect("temp");
        program(&d.path().join("bin/lua-ls"));
        let setting = json!({"lua": {"command": ["lua-ls", "--stdio"], "languages": ["lua"],
            "initializationOptions": {"a": 1}}});
        let all = entries(&setting);
        let vars = env(&[("PATH", &d.path().join("bin"))]);
        let look = Look { root: d.path(), home: None, env: &vars };
        let f = find(&all[0], &look).expect("found");
        assert_eq!((f.program, f.args, f.given), (d.path().join("bin/lua-ls"), vec!["--stdio".to_string()], true));
        assert_eq!(Value::Object(f.options), json!({"a": 1}));
        let ts = all.iter().find(|e| e.id == "typescript").expect("ts");
        assert_eq!(
            Value::Object(defaults_options(ts.kind.map(|k| k.id), d.path())),
            json!({"disableAutomaticTypingAcquisition": true})
        );
    }
}
