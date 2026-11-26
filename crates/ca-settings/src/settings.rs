//! The settings in force: the defaults, then the person's file, then the project's, then the environment.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::schema::{self, Scope, Setting};
use crate::{Env, jsonc};

/// Where a value came from, as the Settings editor and `doctor` say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Default,
    User(PathBuf),
    Project(PathBuf),
    Env(&'static str),
    /// Given for this run (a flag).
    Flag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    User,
    Project,
}

/// Something in a settings file that was not taken, said once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub file: PathBuf,
    pub message: String,
}

#[derive(Debug, Default, Clone)]
pub struct Settings {
    user: Option<(PathBuf, Map<String, Value>)>,
    project: Option<(PathBuf, Map<String, Value>)>,
    flags: Map<String, Value>,
    env: Env,
    pub problems: Vec<Problem>,
}

impl Settings {
    /// Reads the person's file and, when given, the project's. A file that is not there is no layer; one that does
    /// not parse is said in [`Settings::problems`] and left out.
    pub fn load(user_file: &Path, project_file: Option<&Path>, env: Env) -> Self {
        let mut s = Settings { env, ..Default::default() };
        s.user = s.read(user_file, Layer::User);
        if let Some(p) = project_file {
            s.project = s.read(p, Layer::Project);
        }
        s
    }

    /// Only the defaults and the environment.
    pub fn defaults(env: Env) -> Self {
        Settings { env, ..Default::default() }
    }

    fn read(&mut self, file: &Path, layer: Layer) -> Option<(PathBuf, Map<String, Value>)> {
        let text = std::fs::read_to_string(file).ok()?;
        let value = match jsonc::parse(&text) {
            Ok(v) => v,
            Err(e) => {
                self.problems.push(Problem { file: file.to_path_buf(), message: format!("not read: {e}") });
                return None;
            }
        };
        let Value::Object(map) = value else {
            self.problems
                .push(Problem { file: file.to_path_buf(), message: "not read: it is not a JSON object".into() });
            return None;
        };
        let mut kept = Map::new();
        for (key, value) in map {
            match schema::find(&key) {
                // Not Code Arena's (VS Code's own, an older name): kept in the file, listed apart by the editor.
                None => {}
                Some(s) if layer == Layer::Project && s.scope == Scope::User => self.problems.push(Problem {
                    file: file.to_path_buf(),
                    message: format!("{key} is the person's own: a project's settings cannot set it"),
                }),
                Some(s) if !s.accepts(&value) => self.problems.push(Problem {
                    file: file.to_path_buf(),
                    message: format!("{key}: {value} is not of its kind, so its default holds"),
                }),
                Some(_) => {
                    kept.insert(key, value);
                }
            }
        }
        Some((file.to_path_buf(), kept))
    }

    /// Sets a value for this run (a command-line flag), over every other layer.
    pub fn set_flag(&mut self, setting: &Setting, value: Value) {
        if setting.accepts(&value) {
            self.flags.insert(setting.key.to_string(), value);
        }
    }

    /// The value in force, and where it came from.
    pub fn resolve(&self, setting: &Setting) -> (Value, Source) {
        if let Some(v) = self.flags.get(setting.key) {
            return (v.clone(), Source::Flag);
        }
        if let Some(var) = setting.env
            && let Some(v) = self.env.get(var).and_then(|t| setting.parse_env(t))
        {
            return (v, Source::Env(var));
        }
        if let Some((file, map)) = &self.project
            && let Some(v) = map.get(setting.key)
        {
            return (merge_project(setting, self.user_value(setting), v.clone()), Source::Project(file.clone()));
        }
        if let Some(v) = self.user_value(setting) {
            let file = self.user.as_ref().map(|(f, _)| f.clone()).unwrap_or_default();
            return (v, Source::User(file));
        }
        (setting.default_value(), Source::Default)
    }

    fn user_value(&self, setting: &Setting) -> Option<Value> {
        self.user.as_ref().and_then(|(_, m)| m.get(setting.key)).cloned()
    }

    pub fn value(&self, setting: &Setting) -> Value {
        self.resolve(setting).0
    }

    pub fn bool(&self, setting: &Setting) -> bool {
        self.value(setting).as_bool().unwrap_or(false)
    }

    pub fn string(&self, setting: &Setting) -> Option<String> {
        self.value(setting).as_str().filter(|s| !s.is_empty()).map(str::to_string)
    }

    pub fn integer(&self, setting: &Setting) -> i64 {
        self.value(setting).as_i64().unwrap_or(0)
    }

    pub fn strings(&self, setting: &Setting) -> Vec<String> {
        self.value(setting)
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }

    /// Whether the person (or the project, a variable, a flag) gave it, rather than its default.
    pub fn given(&self, setting: &Setting) -> bool {
        !matches!(self.resolve(setting).1, Source::Default)
    }

    pub fn env(&self) -> &Env {
        &self.env
    }
}

/// A project's value of a setting both may set. Permissions: the project's `deny` is added to the person's, and its
/// `allow` is never read, so a cloned repository cannot let itself run commands unasked.
fn merge_project(setting: &Setting, user: Option<Value>, project: Value) -> Value {
    if setting.key != schema::PERMISSIONS.key {
        return project;
    }
    let mut merged = user.unwrap_or_else(|| setting.default_value());
    let deny: Vec<Value> = project.get("deny").and_then(Value::as_array).cloned().unwrap_or_default();
    if let Some(obj) = merged.as_object_mut() {
        let list = obj.entry("deny").or_insert_with(|| Value::Array(vec![]));
        if let Some(a) = list.as_array_mut() {
            a.extend(deny);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{MODE, PERMISSIONS, SHELL_TIMEOUT};
    use serde_json::json;

    fn files(user: &str, project: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let u = dir.path().join("user.json");
        let p = dir.path().join("project.json");
        std::fs::write(&u, user).expect("write");
        std::fs::write(&p, project).expect("write");
        (dir, u, p)
    }

    #[test]
    fn layers_in_order() {
        let (_d, u, p) =
            files(r#"{"agent.mode": "auto-edit", "tools.shellTimeout": 30}"#, r#"{"tools.shellTimeout": 60}"#);
        let s = Settings::load(&u, Some(&p), Env::default());
        assert_eq!(s.value(&MODE), json!("auto-edit"));
        assert_eq!(s.resolve(&SHELL_TIMEOUT), (json!(60), Source::Project(p.clone())));
        let s = Settings::load(&u, Some(&p), Env::from_pairs([("CODE_ARENA_MODE", "plan")]));
        assert_eq!(s.resolve(&MODE), (json!("plan"), Source::Env("CODE_ARENA_MODE")));
    }

    #[test]
    fn a_project_cannot_set_the_persons_own() {
        let (_d, u, p) = files("{}", r#"{"agent.mode": "yolo"}"#);
        let s = Settings::load(&u, Some(&p), Env::default());
        assert_eq!(s.value(&MODE), json!("ask"));
        assert_eq!(s.problems.len(), 1);
        assert!(s.problems[0].message.contains("person's own"));
    }

    #[test]
    fn a_project_only_denies() {
        let (_d, u, p) = files(
            r#"{"permissions": {"allow": ["run_shell(npm test*)"], "deny": []}}"#,
            r#"{"permissions": {"allow": ["run_shell(*)"], "deny": ["read_file(.env*)"]}}"#,
        );
        let s = Settings::load(&u, Some(&p), Env::default());
        assert_eq!(s.value(&PERMISSIONS), json!({"allow": ["run_shell(npm test*)"], "deny": ["read_file(.env*)"]}));
    }

    #[test]
    fn a_wrong_kind_keeps_the_default() {
        let (_d, u, _p) = files(
            r#"{"tools.shellTimeout": "soon", // a comment
        }"#,
            "{}",
        );
        let s = Settings::load(&u, None, Env::default());
        assert_eq!(s.value(&SHELL_TIMEOUT), json!(120));
        assert_eq!(s.problems.len(), 1);
    }

    #[test]
    fn a_flag_wins() {
        let mut s = Settings::defaults(Env::from_pairs([("CODE_ARENA_MODE", "plan")]));
        s.set_flag(&MODE, json!("yolo"));
        assert_eq!(s.resolve(&MODE), (json!("yolo"), Source::Flag));
    }
}

/// Keeps one setting in a settings file (the person's `settings.json`), the rest of it as it is: its comments, its
/// order, its other settings. A file not there yet is made.
pub fn keep(file: &Path, key: &str, value: &Value) -> std::io::Result<()> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let val = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
    let quoted = format!("\"{key}\"");
    let out = if text.trim().is_empty() {
        format!("{{\n  {quoted}: {val}\n}}\n")
    } else if let Some(i) = text.lines().position(|l| l.trim_start().starts_with(&format!("{quoted}:"))) {
        // In place: its line, its comma kept.
        let lines: Vec<String> = text
            .lines()
            .enumerate()
            .map(|(n, l)| {
                if n != i {
                    return l.to_string();
                }
                let indent: String = l.chars().take_while(|c| c.is_whitespace()).collect();
                let comma = if l.trim_end().ends_with(',') { "," } else { "" };
                format!("{indent}{quoted}: {val}{comma}")
            })
            .collect();
        lines.join("\n") + "\n"
    } else {
        // Before the closing brace, with a comma after the setting before it.
        let Some(close) = text.rfind('}') else {
            return Err(std::io::Error::other(format!("{}: not a settings object", file.display())));
        };
        let head = text[..close].trim_end();
        let needs_comma = !(head.ends_with('{') || head.ends_with(','));
        // The last line with a value may end in a comment: the comma goes before it.
        let head = if needs_comma {
            match head.rfind('\n').map(|n| (n, &head[n + 1..])) {
                Some((n, last)) if last.contains("//") && !last.trim_start().starts_with("//") => {
                    let at = last.find("//").unwrap_or(last.len());
                    format!("{}\n{}, {}", &head[..n], last[..at].trim_end(), &last[at..])
                }
                _ if head.trim_end().lines().last().is_some_and(|l| l.trim_start().starts_with("//")) => {
                    head.to_string()
                }
                _ => format!("{head},"),
            }
        } else {
            head.to_string()
        };
        format!("{head}\n  {quoted}: {val}\n{}", &text[close..])
    };
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(file, out)
}

#[cfg(test)]
mod keep_tests {
    use super::*;

    #[test]
    fn one_setting_kept_the_rest_as_it_was() {
        let d = tempfile::tempdir().expect("temp");
        let f = d.path().join("settings.json");
        keep(&f, "agent.compactAt", &serde_json::json!(70)).expect("made");
        assert_eq!(crate::jsonc::parse(&std::fs::read_to_string(&f).expect("r")).expect("json")["agent.compactAt"], 70);
        std::fs::write(&f, "{\n  // mine\n  \"model\": \"qwen\", // the usual\n  \"agent.compactAt\": 70\n}\n")
            .expect("w");
        keep(&f, "agent.compactAt", &serde_json::json!(60)).expect("changed");
        keep(&f, "agent.compactTarget", &serde_json::json!(20)).expect("added");
        let t = std::fs::read_to_string(&f).expect("r");
        assert!(t.contains("// mine") && t.contains("// the usual"), "{t}");
        let v = crate::jsonc::parse(&t).expect("still JSONC");
        assert_eq!(
            (v["model"].as_str(), v["agent.compactAt"].as_i64(), v["agent.compactTarget"].as_i64()),
            (Some("qwen"), Some(60), Some(20))
        );
    }
}
