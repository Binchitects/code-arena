//! The schema: every setting Code Arena reads, with its kind, default, description, where it may be set and the
//! environment variable that overrides it. The Settings editor is built from [`ALL`]; nothing is hidden.

use serde_json::Value;

/// What a setting's value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    String,
    Integer {
        min: i64,
        max: i64,
    },
    /// A number, whole or not (a font's size, a zoom level). Its bounds are whole numbers; `i64::MIN` and
    /// `i64::MAX` are no bound, here and in `Integer`.
    Number {
        min: i64,
        max: i64,
    },
    /// One of these words.
    Enum(&'static [&'static str]),
    /// True, false, or one of these words (VS Code's `git.autoRepositoryDetection`).
    BoolOr(&'static [&'static str]),
    StringList,
    /// A list of numbers (the editor's rulers).
    NumberList,
    /// A JSON object of the setting's own shape (MCP servers, rules…).
    Object,
}

/// Where a setting may be set. A project's file cannot set what is the person's alone: a repository someone cloned
/// must not change how Code Arena treats them (its mode, its commands).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The person's settings.json only.
    User,
    /// The person's, and the project's `.code-arena/settings.json` and `.vscode/settings.json`.
    Project,
    /// As `Project`, and for one language besides: in a `"[python]": {…}` block of any of those files, which wins
    /// over the same setting outside a block.
    Language,
}

#[derive(Debug)]
pub struct Setting {
    pub key: &'static str,
    pub kind: Kind,
    /// The default, as JSON text.
    pub default: &'static str,
    pub description: &'static str,
    pub scope: Scope,
    /// The variable that overrides it for one run.
    pub env: Option<&'static str>,
    /// The Settings editor's section.
    pub section: &'static str,
}

impl Setting {
    pub fn default_value(&self) -> Value {
        serde_json::from_str(self.default).unwrap_or(Value::Null)
    }

    /// Whether a value is of this setting's kind.
    pub fn accepts(&self, value: &Value) -> bool {
        match self.kind {
            Kind::Bool => value.is_boolean(),
            Kind::String => value.is_string() || value.is_null(),
            Kind::Integer { min, max } => value.as_i64().is_some_and(|n| (min..=max).contains(&n)),
            Kind::Number { min, max } => value.as_f64().is_some_and(|n| n >= min as f64 && n <= max as f64),
            Kind::Enum(words) => value.as_str().is_some_and(|w| words.contains(&w)),
            Kind::BoolOr(words) => value.is_boolean() || value.as_str().is_some_and(|w| words.contains(&w)),
            Kind::StringList => value.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
            Kind::NumberList => value.as_array().is_some_and(|a| a.iter().all(Value::is_number)),
            Kind::Object => value.is_object(),
        }
    }

    /// Why a value is not of this setting's kind, as a settings file's problems say it; `None` when it is.
    pub fn refuses(&self, value: &Value) -> Option<String> {
        if self.accepts(value) {
            return None;
        }
        let key = self.key;
        let words = |w: &[&str]| w.iter().map(|e| format!("{e:?}")).collect::<Vec<_>>().join(", ");
        let bound = |b: Option<i64>| b.map_or_else(|| "…".to_string(), |n| n.to_string());
        Some(match self.kind {
            Kind::Bool => format!("\"{key}\" takes true or false."),
            Kind::String => format!("\"{key}\" takes a string."),
            Kind::Integer { .. } | Kind::Number { .. } => {
                let whole = matches!(self.kind, Kind::Integer { .. });
                let (min, max) = self.bounds();
                match value {
                    Value::Number(n) if !whole || n.is_i64() || n.is_u64() => {
                        format!("\"{key}\" is from {} to {}.", bound(min), bound(max))
                    }
                    _ if whole => format!("\"{key}\" takes a whole number."),
                    _ => format!("\"{key}\" takes a number."),
                }
            }
            Kind::Enum(w) => format!("\"{key}\" is one of {}.", words(w)),
            Kind::BoolOr(w) => format!("\"{key}\" is true, false or one of {}.", words(w)),
            Kind::StringList => format!("\"{key}\" is a list of strings."),
            Kind::NumberList => format!("\"{key}\" is a list of numbers."),
            Kind::Object => format!("\"{key}\" takes an object."),
        })
    }

    /// The least and the most a number may be (none: no bound that way).
    pub fn bounds(&self) -> (Option<i64>, Option<i64>) {
        match self.kind {
            Kind::Integer { min, max } | Kind::Number { min, max } => {
                ((min != i64::MIN).then_some(min), (max != i64::MAX).then_some(max))
            }
            _ => (None, None),
        }
    }

    /// Whether a project's settings file may set it (else it is the person's alone).
    pub fn for_project(&self) -> bool {
        self.scope != Scope::User
    }

    /// Whether it may be set for one language, in a `"[python]": {…}` block.
    pub fn for_language(&self) -> bool {
        self.scope == Scope::Language
    }

    /// A variable's text as this setting's value: `true`/`false`/`1`/`0` for a bool, a number, a word, a
    /// list split by the system's path separator.
    pub fn parse_env(&self, text: &str) -> Option<Value> {
        let value = match self.kind {
            Kind::Bool => match text.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Value::Bool(true),
                "0" | "false" | "no" | "off" => Value::Bool(false),
                _ => return None,
            },
            Kind::String | Kind::Enum(_) => Value::String(text.to_string()),
            Kind::BoolOr(_) => match text.to_ascii_lowercase().as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => Value::String(text.to_string()),
            },
            Kind::Integer { .. } => Value::from(text.trim().parse::<i64>().ok()?),
            Kind::Number { .. } => Value::from(text.trim().parse::<f64>().ok()?),
            Kind::StringList => Value::Array(
                std::env::split_paths(text).map(|p| Value::String(p.to_string_lossy().into_owned())).collect(),
            ),
            Kind::NumberList | Kind::Object => serde_json::from_str(text).ok()?,
        };
        self.accepts(&value).then_some(value)
    }
}

pub static MODEL: Setting = Setting {
    key: "agent.model",
    kind: Kind::String,
    default: "null",
    description: "The model the agent answers with (as the gateway lists it, or provider:model). Empty: the first the gateway lists.",
    scope: Scope::User,
    env: Some("ARENA_MODEL"),
    section: "Agent",
};

pub static MODE: Setting = Setting {
    key: "agent.mode",
    kind: Kind::Enum(&["ask", "auto-edit", "plan", "yolo"]),
    default: r#""ask""#,
    description: "What runs without asking: ask (reads only), auto-edit (and file edits), plan (reads only; changes are refused), yolo (everything: for a folder you can throw away).",
    scope: Scope::User,
    env: Some("CODE_ARENA_MODE"),
    section: "Agent",
};

pub static THINKING: Setting = Setting {
    key: "agent.thinking",
    kind: Kind::Enum(&["default", "off", "low", "medium", "high", "xhigh"]),
    default: r#""default""#,
    description: "How much the model thinks: default is the Arena's default level, else the model's own.",
    scope: Scope::User,
    env: None,
    section: "Agent",
};

pub static CONTEXT: Setting = Setting {
    key: "agent.context",
    kind: Kind::Integer { min: 0, max: 10_000_000 },
    default: "0",
    description: "The model's window in tokens, when the gateway does not say it (0: as the gateway says, else 32,768).",
    scope: Scope::User,
    env: None,
    section: "Agent",
};

pub static MAX_STEPS: Setting = Setting {
    key: "agent.maxSteps",
    kind: Kind::Integer { min: 1, max: 100_000 },
    default: "200",
    description: "The most requests to the model one turn makes before it stops and says so.",
    scope: Scope::User,
    env: None,
    section: "Agent",
};

pub static SHELL: Setting = Setting {
    key: "tools.shell",
    kind: Kind::String,
    default: "null",
    description: "The shell the agent's run_shell uses (a path or a name on PATH). Empty: bash, else sh; on Windows Git's bash, else PowerShell 7, else cmd.exe.",
    scope: Scope::User,
    env: None,
    section: "Tools",
};

pub static SHELL_TIMEOUT: Setting = Setting {
    key: "tools.shellTimeout",
    kind: Kind::Integer { min: 1, max: 600 },
    default: "120",
    description: "Seconds a command may run before it is stopped, unless the model asks for another limit (at most 600).",
    scope: Scope::Project,
    env: None,
    section: "Tools",
};

pub static ALLOWED_PATHS: Setting = Setting {
    key: "tools.allowedPaths",
    kind: Kind::StringList,
    default: "[]",
    description: "Folders the agent's tools may use besides the working directory (as --add-dir does).",
    scope: Scope::User,
    env: None,
    section: "Tools",
};

pub static PERMISSIONS: Setting = Setting {
    key: "permissions",
    kind: Kind::Object,
    default: r#"{"allow": [], "deny": []}"#,
    description: "Kept rules: allow lets calls run without asking, deny refuses them in every mode. A rule is a tool's name with an optional pattern: run_shell(npm test*), read_file(.env*). A project's file may only deny.",
    scope: Scope::Project,
    env: None,
    section: "Permissions",
};

pub static AUTO_COMMIT: Setting = Setting {
    key: "git.autoCommit",
    kind: Kind::Bool,
    default: "false",
    description: "Commit what each turn changed, in commits the model groups and writes the messages for. Off by default; /commit does it when asked.",
    scope: Scope::User,
    env: None,
    section: "Git",
};

pub static AUTO_COMMIT_REVIEW: Setting = Setting {
    key: "git.autoCommit.review",
    kind: Kind::Bool,
    default: "true",
    description: "With git.autoCommit on, the commits the model proposes after a turn are shown first, for /commit yes (or no); off: they are made at once.",
    scope: Scope::User,
    env: None,
    section: "Git",
};

pub static PROJECT_FOLDER: Setting = Setting {
    key: "workspace.projectFolder",
    kind: Kind::Bool,
    default: "true",
    description: "Make the project's .code-arena/ folder, with its default files, at the repository's root the first time a repository is opened (a folder that is no repository gets none). Off: it is read when it is there, never made.",
    scope: Scope::User,
    env: None,
    section: "Workspace",
};

pub static UPDATE_CHECK: Setting = Setting {
    key: "update.check",
    kind: Kind::Bool,
    default: "true",
    description: "Ask the Arena once a day whether it serves a newer Code Arena.",
    scope: Scope::User,
    env: None,
    section: "Code Arena on this computer",
};

pub static COLOR: Setting = Setting {
    key: "terminal.color",
    kind: Kind::Bool,
    default: "true",
    description: "Colours in Code Arena's terminal output (NO_COLOR, or output to a file or a pipe, turns them off too).",
    scope: Scope::User,
    env: None,
    section: "Terminal",
};

pub static MCP_SERVERS: Setting = Setting {
    key: "mcpServers",
    kind: Kind::Object,
    default: "{}",
    description: "Your own MCP servers, in the shape other agents use too: {\"name\": {\"command\": \"npx\", \"args\": [...], \"env\": {...}}} for a child process, or {\"url\": \"https://…/mcp\", \"headers\": {\"Authorization\": \"Bearer ${TOKEN}\"}}. ${NAME} is taken from the environment. \"trust\": true lets its tools run without asking; \"disabled\": true keeps it without starting it.",
    scope: Scope::User,
    env: None,
    section: "MCP servers",
};

pub static ARENA_TOOLS: Setting = Setting {
    key: "mcp.arenaTools",
    kind: Kind::Bool,
    default: "true",
    description: "Arena's own tools (web search and pages, Python, pictures, speech, knowledge…), from https://DOMAIN/mcp with your key.",
    scope: Scope::User,
    env: None,
    section: "MCP servers",
};

pub static CODE_INTEL_TOOLS: Setting = Setting {
    key: "mcp.codeIntelTools",
    kind: Kind::Bool,
    default: "true",
    description: "Farzan Graph's code search tools, at the address Arena gives (else https://code-intel.DOMAIN/mcp).",
    scope: Scope::User,
    env: None,
    section: "MCP servers",
};

pub static TOOL_CHARS: Setting = Setting {
    key: "tools.onDemandChars",
    kind: Kind::Integer { min: 0, max: 10_000_000 },
    default: "6000",
    description: "Past this many characters of the servers' tool definitions, only the tools loaded go whole in each request; the others are listed for the model to load with load_tools. 0 sends them whole always.",
    scope: Scope::User,
    env: None,
    section: "Tools",
};

pub static TOOL_MODE: Setting = Setting {
    key: "tools.mode",
    kind: Kind::Enum(&["direct", "code", "both"]),
    default: r#""both""#,
    description: "How the model is given its tools: direct (one call at a time), code (the servers' tools as functions of a run_code program, which calls them as Python), or both. Not set: as Arena says for the model.",
    scope: Scope::User,
    env: None,
    section: "Tools",
};

pub static SYNC_CHATS: Setting = Setting {
    key: "arena.syncChats",
    kind: Kind::Bool,
    default: "true",
    description: "Keep each session in step with a chat in Arena, both ways: what is said here is in the web chat (Chat, with the folder shown), and what is added there comes in at the next turn. Off: sessions stay on this machine (/sync off does it for one session).",
    scope: Scope::User,
    env: None,
    section: "Arena",
};

pub static PROJECT_MEMORY: Setting = Setting {
    key: "memory.project",
    kind: Kind::Bool,
    default: "true",
    description: "The project's long memory in the repository (.remember/MEMORY.md and topic files): read at each session's start and kept current by the agent with remember_project. Off: neither read nor written.",
    scope: Scope::User,
    env: None,
    section: "Memory",
};

pub static BUILTIN_SKILLS: Setting = Setting {
    key: "skills.builtIn",
    kind: Kind::Bool,
    default: "true",
    description: "The skills that come with Code Arena (code-review, debug, release-notes and the others Arena has), read after yours, the project's and Arena's, which win by name. Off: none of them.",
    scope: Scope::User,
    env: None,
    section: "Skills",
};

pub static PLAN_SECOND_READING: Setting = Setting {
    key: "plan.secondReading",
    kind: Kind::Bool,
    default: "true",
    description: "A plan the agent proposes in plan mode is read again with fresh eyes before your review (one more request to the model): what is missing, risky, unchecked or in the wrong order, and the agent changes the plan where the notes hold.",
    scope: Scope::User,
    env: None,
    section: "Agent",
};

pub static HOOKS: Setting = Setting {
    key: "hooks",
    kind: Kind::Object,
    default: "{}",
    description: "Commands run at the agent's moments, in the common shape: {\"PreToolUse\": [{\"matcher\": \"run_shell|Bash\", \"hooks\": [{\"type\": \"command\", \"command\": \"...\", \"timeout\": 60}]}]}. The moments: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Notification, Stop, SubagentStop, PreCompact, PreCommit. Each gets the moment as JSON on stdin; exit code 2 blocks (its stderr says why), or it answers JSON. A project's hooks (.code-arena/settings.json, and the settings.json of the agents compat.agents names) run only once you trust them (/hooks trust).",
    scope: Scope::User,
    env: None,
    section: "Hooks",
};

pub static COLOR_THEME: Setting = Setting {
    key: "workbench.colorTheme",
    kind: Kind::String,
    default: "\"\"",
    description: "The IDE's colour theme, by name or id: Code Arena, Sakura, Midnight, Copper, Forest, Ocean or Lavender (each Dark or Light), Dark Modern, Light Modern, Dark+, Light+, Dark High Contrast, Light High Contrast, Monokai, Monokai Light, Solarized Dark, Solarized Light, One Dark Pro, One Light, Dracula, Alucard, GitHub Dark, GitHub Light, Nord, Nord Light, Dark (Visual Studio), Light (Visual Studio), or one of yours (a VS Code theme's JSON, or a VS Code theme extension's folder, in themes/ in the config folder, ~/.code-arena/themes or the project's .code-arena/themes). Empty: Dark Modern or Light Modern, as the system is dark or light; a name none has, the nearest (VS Code's and older names read as these). Not used while \"window.autoDetectColorScheme\" is on.",
    scope: Scope::User,
    env: None,
    section: "Appearance",
};

pub static COMPAT_AGENTS: Setting = Setting {
    key: "compat.agents",
    kind: Kind::StringList,
    default: "[]",
    description: "Other agents whose files Code Arena reads too, by name (none by default). For an agent named NAME: its folders .NAME/ in the project and the home (commands, skills, sub-agents, output styles, and its settings.json's hooks and status line), its instructions NAME.md, its plugins' .NAME-plugin/ manifests, and the variables ${NAME_PLUGIN_ROOT} and NAME_PROJECT_DIR.",
    scope: Scope::Project,
    env: None,
    section: "Compatibility",
};

pub static OUTPUT_STYLE: Setting = Setting {
    key: "outputStyle",
    kind: Kind::String,
    default: "\"default\"",
    description: "How the agent talks: default, explanatory (explains the codebase's choices as it goes), learning (leaves small pieces for you to write), or one of output-styles/NAME.md (the project's, yours, a plugin's). /output-style lists them.",
    scope: Scope::Project,
    env: None,
    section: "Output",
};

pub static STATUS_LINE: Setting = Setting {
    key: "statusLine",
    kind: Kind::Object,
    default: "{}",
    description: "A line of your own above the prompt, in the common shape: {\"type\": \"command\", \"command\": \"~/.code-arena/statusline.sh\"}. The command gets the session as JSON on stdin (model, workspace, cost, output style, mode, context) and its first line is shown, drawn again after each turn. When this is not set, the statusLine of an agent compat.agents names (~/.NAME/settings.json) is used.",
    scope: Scope::User,
    env: None,
    section: "Output",
};

pub static PLUGINS: Setting = Setting {
    key: "plugins.enabled",
    kind: Kind::StringList,
    default: "[]",
    description: "The plugins in use, by name (code-arena plugin list shows those installed): their commands, sub-agents, skills, hooks and MCP servers. A project's settings may enable more of those installed.",
    scope: Scope::User,
    env: None,
    section: "Plugins",
};

pub static SERVER_WAIT: Setting = Setting {
    key: "mcp.startWait",
    kind: Kind::Integer { min: 0, max: 120 },
    default: "3",
    description: "Seconds a conversation waits at its start for the MCP servers (-p waits up to 25); a server that answers later joins when it does.",
    scope: Scope::User,
    env: None,
    section: "MCP servers",
};

pub static COMPACT_AT: Setting = Setting {
    key: "agent.compactAt",
    kind: Kind::Integer { min: 0, max: 95 },
    default: "80",
    description: "When the next request would pass this share of the model's window (percent, 20 to 95), the older part of the conversation is summarized. 0: only when a request would not fit.",
    scope: Scope::User,
    env: None,
    section: "Agent",
};

pub static COMPACT_TARGET: Setting = Setting {
    key: "agent.compactTarget",
    kind: Kind::Integer { min: 5, max: 85 },
    default: "25",
    description: "What a compaction keeps whole: the recent part of the conversation, up to this share of the window (percent).",
    scope: Scope::User,
    env: None,
    section: "Agent",
};

pub static SANDBOX: Setting = Setting {
    key: "tools.sandbox",
    kind: Kind::Enum(&["auto", "on", "off"]),
    default: r#""auto""#,
    description: "The agent's commands in a sandbox (bubblewrap on Linux, sandbox-exec on macOS): they read everything but what holds secrets, and write only in the working folders, the build caches and a /tmp of their own. auto: when the system has one (else they run as you, and the session says why); on: refuse to start without one; off: run as you.",
    scope: Scope::User,
    env: None,
    section: "Sandbox",
};

pub static SANDBOX_NETWORK: Setting = Setting {
    key: "tools.sandboxNetwork",
    kind: Kind::Bool,
    default: "true",
    description: "Whether commands in the sandbox may reach the network.",
    scope: Scope::Project,
    env: None,
    section: "Sandbox",
};

pub static SANDBOX_WRITABLE: Setting = Setting {
    key: "tools.sandboxWritable",
    kind: Kind::StringList,
    default: "[]",
    description: "More folders commands in the sandbox may write (whole paths).",
    scope: Scope::User,
    env: None,
    section: "Sandbox",
};

/// Every setting, in the Settings editor's order.
pub static ALL: &[&Setting] = &[
    &MODEL,
    &MODE,
    &THINKING,
    &CONTEXT,
    &MAX_STEPS,
    &COMPACT_AT,
    &COMPACT_TARGET,
    &SHELL,
    &SHELL_TIMEOUT,
    &ALLOWED_PATHS,
    &PERMISSIONS,
    &AUTO_COMMIT,
    &AUTO_COMMIT_REVIEW,
    &PROJECT_FOLDER,
    &UPDATE_CHECK,
    &COLOR,
    &MCP_SERVERS,
    &ARENA_TOOLS,
    &CODE_INTEL_TOOLS,
    &TOOL_CHARS,
    &TOOL_MODE,
    &SERVER_WAIT,
    &SYNC_CHATS,
    &PROJECT_MEMORY,
    &BUILTIN_SKILLS,
    &PLAN_SECOND_READING,
    &HOOKS,
    &PLUGINS,
    &OUTPUT_STYLE,
    &COMPAT_AGENTS,
    &COLOR_THEME,
    &STATUS_LINE,
    &SANDBOX,
    &SANDBOX_NETWORK,
    &SANDBOX_WRITABLE,
];

/// A setting by its key: one of [`ALL`], or one of the IDE's own ([`crate::ide_schema::ALL`]).
pub fn find(key: &str) -> Option<&'static Setting> {
    ALL.iter().chain(crate::ide_schema::ALL).copied().find(|s| s.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn keys_are_unique_and_defaults_fit() {
        let mut seen = HashSet::new();
        for s in ALL.iter().chain(crate::ide_schema::ALL) {
            assert!(seen.insert(s.key), "{} twice", s.key);
            assert!(s.accepts(&s.default_value()), "{}'s default is not of its kind", s.key);
            assert!(!s.description.is_empty(), "{} says nothing", s.key);
        }
    }

    #[test]
    fn auto_commit_is_off_by_default() {
        assert_eq!(AUTO_COMMIT.default_value(), Value::Bool(false));
    }

    #[test]
    fn env_text_is_typed() {
        assert_eq!(MODE.parse_env("plan"), Some(Value::from("plan")));
        assert_eq!(MODE.parse_env("reckless"), None);
        assert_eq!(SHELL_TIMEOUT.parse_env("30"), Some(Value::from(30)));
        assert_eq!(SHELL_TIMEOUT.parse_env("9000"), None);
        assert_eq!(AUTO_COMMIT.parse_env("on"), Some(Value::Bool(true)));
    }
}
