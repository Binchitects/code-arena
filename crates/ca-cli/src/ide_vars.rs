//! The variables a task or a launch configuration may use (`${workspaceFolder}`, `${file}`, `${env:NAME}`,
//! `${config:key}`, `${input:id}`), put in their places. Shared by the tasks (`ide_tasks`) and run and debug
//! (`ide_dap`); it answers no message of its own.
//!
//! As VS Code's: a variable it does not know is left as it is written, one it knows but cannot give (`${file}`
//! with no file open) is an error that names it, and `${input:id}` is the answer the person gave, asked for
//! before the text is put together ([`inputs_in`] says which).

use std::path::Path;

use serde_json::{Map, Value};

/// What the variables are read from.
pub struct Ctx<'a> {
    /// The folder open: `${workspaceFolder}`, and what `${relativeFile}` is relative to.
    pub folder: &'a Path,
    /// The file of the editor shown, its whole path.
    pub file: Option<&'a str>,
    /// The caret's line in it, from 1.
    pub line: Option<u64>,
    /// The text selected in it.
    pub selected: &'a str,
    /// `${codeArena.buildConfiguration}`: Debug or Release.
    pub build: &'a str,
    /// The answers given for `${input:id}`, by id.
    pub inputs: &'a Map<String, Value>,
    /// A variable of the environment (`${env:NAME}`); none: not set.
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// A setting's value (`${config:key}`); none: no such setting.
    pub config: &'a dyn Fn(&str) -> Option<Value>,
}

/// The variables of the environment never put in a task's line: the keys Code Arena reads (the Arena's, the
/// providers'), whatever a tasks.json asks.
pub const NEVER: &[&str] = &[
    "ARENA_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "DEEPSEEK_API_KEY",
    "OPENROUTER_API_KEY",
    "MISTRAL_API_KEY",
    "OLLAMA_API_KEY",
];

/// A variable of the process's environment that a tasks.json or launch.json names (`${env:NAME}`): the person's
/// own, read when the task runs; none for the keys in [`NEVER`].
pub fn process_env(name: &str) -> Option<String> {
    if NEVER.contains(&name) || name.is_empty() {
        return None;
    }
    std::env::var(name).ok()
}

/// The ids of the inputs a text asks for (`${input:id}`), each once, in the order they come.
pub fn inputs_in(text: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for name in names(text) {
        if let Some(id) = name.strip_prefix("input:")
            && !ids.iter().any(|i| i == id)
        {
            ids.push(id.to_string());
        }
    }
    ids
}

/// Every `${...}` of a text, the words inside.
fn names(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        out.push(&after[..end]);
        rest = &after[end + 1..];
    }
    out
}

/// The text with its variables in their places; an error names the one that could not be given.
pub fn expand(text: &str, cx: &Ctx) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let name = &after[..end];
        match value(name, cx)? {
            Some(v) => out.push_str(&v),
            None => {
                out.push_str("${");
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// One variable's value; none: not one it knows (left as written).
fn value(name: &str, cx: &Ctx) -> Result<Option<String>, String> {
    let shown = |p: &Path| p.display().to_string();
    let folder = cx.folder;
    if let Some(var) = name.strip_prefix("env:") {
        return Ok(Some((cx.env)(var).unwrap_or_default()));
    }
    if let Some(key) = name.strip_prefix("config:") {
        return Ok(Some(match (cx.config)(key) {
            Some(Value::String(s)) => s,
            Some(Value::Null) | None => String::new(),
            Some(v) => v.to_string(),
        }));
    }
    if let Some(id) = name.strip_prefix("input:") {
        return match cx.inputs.get(id) {
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(v) if !v.is_null() => Ok(Some(v.to_string())),
            _ => Err(format!("Input variable '{id}' was not given.")),
        };
    }
    if name.starts_with("workspaceFolder:") {
        return Ok(Some(shown(folder)));
    }
    let file = || {
        cx.file.map(Path::new).ok_or_else(|| format!("Variable {name} can not be resolved. Please open an editor."))
    };
    let base = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let relative = |p: &Path| p.strip_prefix(folder).map(Path::to_path_buf).unwrap_or_else(|_| p.to_path_buf());
    let v = match name {
        "workspaceFolder" | "workspaceRoot" | "cwd" | "fileWorkspaceFolder" => shown(folder),
        "workspaceFolderBasename" => base(folder),
        "pathSeparator" | "/" => std::path::MAIN_SEPARATOR.to_string(),
        "codeArena.buildConfiguration" => cx.build.to_string(),
        "file" => shown(file()?),
        "relativeFile" => shown(&relative(file()?)),
        "relativeFileDirname" => relative(file()?).parent().map(shown).unwrap_or_default(),
        "fileBasename" => base(file()?),
        "fileBasenameNoExtension" => {
            file()?.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        }
        "fileExtname" => {
            file()?.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default()
        }
        "fileDirname" => file()?.parent().map(shown).unwrap_or_default(),
        "fileDirnameBasename" => file()?.parent().map(base).unwrap_or_default(),
        "lineNumber" => match cx.line {
            Some(n) => n.to_string(),
            None => return Err(format!("Variable {name} can not be resolved. Please open an editor.")),
        },
        "selectedText" => cx.selected.to_string(),
        _ => return Ok(None),
    };
    Ok(Some(v))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn with(file: Option<&str>, inputs: Value, f: impl FnOnce(&Ctx)) {
        let inputs = inputs.as_object().cloned().unwrap_or_default();
        let env = |n: &str| (n == "HOME_FAKE").then(|| "/home/fake".to_string());
        let config = |k: &str| match k {
            "editor.tabSize" => Some(json!(4)),
            "terminal.integrated.fontFamily" => Some(json!("Mono")),
            _ => None,
        };
        let cx = Ctx {
            folder: Path::new("/work/app"),
            file,
            line: file.map(|_| 12),
            selected: "picked",
            build: "Release",
            inputs: &inputs,
            env: &env,
            config: &config,
        };
        f(&cx);
    }

    #[test]
    fn the_folder_and_the_file_are_put_in_their_places() {
        with(Some("/work/app/src/main.rs"), json!({}), |cx| {
            let x = |t: &str| expand(t, cx).unwrap_or_else(|e| e);
            assert_eq!(x("${workspaceFolder}/out"), "/work/app/out");
            assert_eq!(x("${workspaceFolderBasename}"), "app");
            assert_eq!(x("${file}"), "/work/app/src/main.rs");
            assert_eq!(x("${relativeFile}"), "src/main.rs");
            assert_eq!(x("${relativeFileDirname}"), "src");
            assert_eq!(x("${fileBasename} ${fileBasenameNoExtension} ${fileExtname}"), "main.rs main .rs");
            assert_eq!(x("${fileDirname}"), "/work/app/src");
            assert_eq!(x("${fileDirnameBasename}"), "src");
            assert_eq!(x("${lineNumber}:${selectedText}"), "12:picked");
            assert_eq!(x("-c ${codeArena.buildConfiguration}"), "-c Release");
        });
    }

    #[test]
    fn env_and_config_are_read_through_what_is_given() {
        with(None, json!({}), |cx| {
            assert_eq!(expand("${env:HOME_FAKE}/x", cx).as_deref(), Ok("/home/fake/x"));
            assert_eq!(expand("[${env:NOT_SET}]", cx).as_deref(), Ok("[]"));
            assert_eq!(expand("${config:editor.tabSize}", cx).as_deref(), Ok("4"));
            assert_eq!(expand("${config:terminal.integrated.fontFamily}", cx).as_deref(), Ok("Mono"));
            assert_eq!(expand("${config:no.such}", cx).as_deref(), Ok(""));
        });
    }

    #[test]
    fn the_keys_are_never_read_from_the_environment() {
        for key in NEVER {
            assert_eq!(process_env(key), None, "{key}");
        }
    }

    #[test]
    fn unknown_variables_stay_and_a_file_needs_an_editor() {
        with(None, json!({}), |cx| {
            assert_eq!(expand("${nothing} ${defaultBuildTask} $x {y}", cx).as_deref(), Ok("${nothing} ${defaultBuildTask} $x {y}"));
            assert_eq!(expand("open ${", cx).as_deref(), Ok("open ${"));
            assert_eq!(
                expand("${file}", cx),
                Err("Variable file can not be resolved. Please open an editor.".to_string())
            );
            assert!(expand("${lineNumber}", cx).is_err());
        });
    }

    #[test]
    fn inputs_are_asked_for_once_and_put_in() {
        assert_eq!(inputs_in("${input:a} ${file} ${input:b} ${input:a}"), vec!["a", "b"]);
        with(None, json!({"a": "one", "b": 2}), |cx| {
            assert_eq!(expand("${input:a}-${input:b}", cx).as_deref(), Ok("one-2"));
            assert_eq!(expand("${input:c}", cx), Err("Input variable 'c' was not given.".to_string()));
        });
    }
}
