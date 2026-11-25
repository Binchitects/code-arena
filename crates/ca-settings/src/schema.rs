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
    /// One of these words.
    Enum(&'static [&'static str]),
    StringList,
    /// A JSON object of the setting's own shape (MCP servers, rules…).
    Object,
}

/// Where a setting may be set. A project's file cannot set what is the person's alone: a repository someone cloned
/// must not change how Code Arena treats them (its mode, its commands).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The person's settings.json only.
    User,
    /// The person's, and the project's `.code-arena/settings.json`.
    Project,
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
            Kind::Enum(words) => value.as_str().is_some_and(|w| words.contains(&w)),
            Kind::StringList => value.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
            Kind::Object => value.is_object(),
        }
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
            Kind::Integer { .. } => Value::from(text.trim().parse::<i64>().ok()?),
            Kind::StringList => Value::Array(
                std::env::split_paths(text).map(|p| Value::String(p.to_string_lossy().into_owned())).collect(),
            ),
            Kind::Object => serde_json::from_str(text).ok()?,
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

/// Every setting, in the Settings editor's order.
pub static ALL: &[&Setting] = &[
    &MODEL,
    &MODE,
    &THINKING,
    &CONTEXT,
    &MAX_STEPS,
    &SHELL,
    &SHELL_TIMEOUT,
    &ALLOWED_PATHS,
    &PERMISSIONS,
    &AUTO_COMMIT,
    &PROJECT_FOLDER,
    &UPDATE_CHECK,
    &COLOR,
];

pub fn find(key: &str) -> Option<&'static Setting> {
    ALL.iter().copied().find(|s| s.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn keys_are_unique_and_defaults_fit() {
        let mut seen = HashSet::new();
        for s in ALL {
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
