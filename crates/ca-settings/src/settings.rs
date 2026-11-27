//! The settings in force: the defaults, then the person's file, then the project's two (`.code-arena/settings.json`,
//! then VS Code's `.vscode/settings.json` beside it), then the environment. A setting that may be set for one
//! language is read for it from the `"[python]": {…}` blocks of those files first: a language's own value, from
//! any file, wins over the same setting outside a block.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::schema::{self, Setting};
use crate::{Env, ide_schema, jsonc};

/// Where a value came from, as the Settings editor and `doctor` say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Default,
    User(PathBuf),
    /// One of the project's two files.
    Project(PathBuf),
    Env(&'static str),
    /// Given for this run (a flag).
    Flag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    User,
    /// The project's `.code-arena/settings.json`.
    Project,
    /// The project's `.vscode/settings.json`, as VS Code keeps a folder's settings.
    Workspace,
}

/// Something in a settings file that was not taken, said once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub file: PathBuf,
    pub message: String,
    /// The setting it is about (empty: the file itself, which was not read).
    pub key: String,
}

/// One settings file, read: its values by setting, and each language's own (`"[python]": {…}`; `"[a][b]"` gives
/// both the same).
#[derive(Debug, Default, Clone)]
struct File {
    path: PathBuf,
    values: Map<String, Value>,
    languages: BTreeMap<String, Map<String, Value>>,
    /// What it holds that is no setting of Code Arena's (VS Code's own, an older name): left in the file.
    unknown: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Settings {
    user: Option<File>,
    project: Option<File>,
    workspace: Option<File>,
    flags: Map<String, Value>,
    env: Env,
    /// What was not taken of the settings the terminal reads too (the agent's, the tools', the servers'), and a
    /// file of Code Arena's own that could not be read: said at each start.
    pub problems: Vec<Problem>,
    /// What was not taken of the IDE's settings, and of `.vscode/settings.json`: the IDE says these, the terminal
    /// does not (a value VS Code takes and Code Arena does not is no fault of a command's).
    pub ide_problems: Vec<Problem>,
}

/// The project's `.vscode/settings.json`, from its `.code-arena/settings.json` beside it.
pub fn workspace_file(project_file: &Path) -> Option<PathBuf> {
    let dir = project_file.parent()?;
    (dir.file_name()? == ".code-arena").then(|| dir.parent().map(|root| root.join(".vscode").join("settings.json")))?
}

/// The languages a settings key names, when it is a block of theirs: `[python]`, `[javascript][typescript]`.
pub fn languages_of(key: &str) -> Option<Vec<&str>> {
    let mut ids = Vec::new();
    let mut rest = key;
    while let Some(r) = rest.strip_prefix('[') {
        let end = r.find(']').filter(|end| *end > 0)?;
        ids.push(&r[..end]);
        rest = &r[end + 1..];
    }
    (rest.is_empty() && !ids.is_empty()).then_some(ids)
}

/// Nested objects as VS Code reads them: `{"editor": {"fontSize": 14}}` is `editor.fontSize`. An object is gone
/// into only while its name is no setting itself and begins one.
fn flatten(map: Map<String, Value>, prefix: &str, out: &mut Vec<(String, Value)>) {
    for (k, v) in map {
        let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        let begins = || {
            let stem = format!("{key}.");
            schema::ALL.iter().chain(ide_schema::ALL).any(|s| s.key.starts_with(&stem))
        };
        match v {
            Value::Object(inner) if !k.starts_with('[') && schema::find(&key).is_none() && begins() => {
                flatten(inner, &key, out)
            }
            v => out.push((key, v)),
        }
    }
}

impl Settings {
    /// Reads the person's file and, when given, the project's (and `.vscode/settings.json` beside its
    /// `.code-arena/`). A file that is not there is no layer; one that does not parse is said in the problems and
    /// left out.
    pub fn load(user_file: &Path, project_file: Option<&Path>, env: Env) -> Self {
        let mut s = Settings { env, ..Default::default() };
        s.user = s.read(user_file, Layer::User);
        if let Some(p) = project_file {
            s.project = s.read(p, Layer::Project);
            if let Some(w) = workspace_file(p) {
                s.workspace = s.read(&w, Layer::Workspace);
            }
        }
        s
    }

    /// Only the defaults and the environment.
    pub fn defaults(env: Env) -> Self {
        Settings { env, ..Default::default() }
    }

    /// A problem kept: with the IDE's when it is about one of the IDE's settings or VS Code's file.
    fn say(&mut self, file: &Path, layer: Layer, key: &str, message: String) {
        let ide = layer == Layer::Workspace || ide_schema::ALL.iter().any(|s| s.key == key);
        let list = if ide { &mut self.ide_problems } else { &mut self.problems };
        list.push(Problem { file: file.to_path_buf(), message, key: key.to_string() });
    }

    fn read(&mut self, file: &Path, layer: Layer) -> Option<File> {
        let text = std::fs::read_to_string(file).ok()?;
        let value = match jsonc::parse(&text) {
            Ok(v) => v,
            Err(e) => {
                self.say(file, layer, "", format!("not read: {e}"));
                return None;
            }
        };
        let Value::Object(map) = value else {
            self.say(file, layer, "", "not read: it is not a JSON object".into());
            return None;
        };
        let mut read = File { path: file.to_path_buf(), ..Default::default() };
        let mut flat = Vec::new();
        flatten(map, "", &mut flat);
        for (key, value) in flat {
            let Some(ids) = languages_of(&key) else {
                if let Some(value) = self.taken(file, layer, &key, value, false, &mut read.unknown) {
                    read.values.insert(key, value);
                }
                continue;
            };
            let Value::Object(block) = value else {
                self.say(file, layer, &key, format!("\"{key}\" holds an object of settings."));
                continue;
            };
            let mut inner = Vec::new();
            flatten(block, "", &mut inner);
            let mut own = Map::new();
            for (k, v) in inner {
                if let Some(v) = self.taken(file, layer, &k, v, true, &mut read.unknown) {
                    own.insert(k, v);
                }
            }
            for id in ids {
                read.languages.entry(id.to_string()).or_default().extend(own.clone());
            }
        }
        Some(read)
    }

    /// One setting of a file as it is taken: `None` when it is not (no setting of Code Arena's, of the wrong
    /// kind, the person's alone in a project's file, in a language's block and not a language's to set).
    fn taken(
        &mut self,
        file: &Path,
        layer: Layer,
        key: &str,
        value: Value,
        language: bool,
        unknown: &mut Vec<String>,
    ) -> Option<Value> {
        let Some(s) = schema::find(key) else {
            // Not Code Arena's (VS Code's own, an older name): kept in the file, listed apart by the editor.
            if !unknown.iter().any(|k| k == key) {
                unknown.push(key.to_string());
            }
            return None;
        };
        if layer != Layer::User && !s.for_project() {
            self.say(file, layer, key, format!("{key} is the person's own: a project's settings cannot set it"));
            return None;
        }
        if let Some(why) = s.refuses(&value) {
            self.say(file, layer, key, format!("{why} Its default holds."));
            return None;
        }
        if language && !s.for_language() {
            self.say(file, layer, key, format!("\"{key}\" cannot be set for one language."));
            return None;
        }
        Some(value)
    }

    /// Sets a value for this run (a command-line flag), over every other layer.
    pub fn set_flag(&mut self, setting: &Setting, value: Value) {
        if setting.accepts(&value) {
            self.flags.insert(setting.key.to_string(), value);
        }
    }

    /// The files read, the least first: the person's, the project's `.code-arena/settings.json`, its
    /// `.vscode/settings.json`.
    fn files(&self) -> impl DoubleEndedIterator<Item = (&File, Layer)> {
        [(&self.user, Layer::User), (&self.project, Layer::Project), (&self.workspace, Layer::Workspace)]
            .into_iter()
            .filter_map(|(f, layer)| f.as_ref().map(|f| (f, layer)))
    }

    /// The value in force, and where it came from.
    pub fn resolve(&self, setting: &Setting) -> (Value, Source) {
        self.resolve_for(setting, None)
    }

    /// The value in force for a language (VS Code's id: `python`, `typescriptreact`), and where it came from: the
    /// language's own from a `"[python]"` block when the setting may be set for one (the project's block over the
    /// person's), else the value for every language.
    pub fn resolve_for(&self, setting: &Setting, language: Option<&str>) -> (Value, Source) {
        if let Some(v) = self.flags.get(setting.key) {
            return (v.clone(), Source::Flag);
        }
        if let Some(var) = setting.env
            && let Some(v) = self.env.get(var).and_then(|t| setting.parse_env(t))
        {
            return (v, Source::Env(var));
        }
        if let Some(id) = language.filter(|_| setting.for_language()) {
            for (file, layer) in self.files().rev() {
                if let Some(v) = file.languages.get(id).and_then(|own| own.get(setting.key)) {
                    return (v.clone(), source(file, layer));
                }
            }
        }
        let mut found: Option<(Value, Source)> = None;
        for (file, layer) in self.files() {
            let Some(v) = file.values.get(setting.key) else { continue };
            found = Some(match layer {
                Layer::User => (v.clone(), source(file, layer)),
                _ => (merge_project(setting, found.map(|f| f.0), v.clone()), source(file, layer)),
            });
        }
        found.unwrap_or_else(|| (setting.default_value(), Source::Default))
    }

    pub fn value(&self, setting: &Setting) -> Value {
        self.resolve(setting).0
    }

    /// A setting's value by its key, for a language when one is given: how the IDE's parts read theirs
    /// (`settings.ide("git.autofetch", None)`). Null for a key that is no setting.
    pub fn ide(&self, key: &str, language: Option<&str>) -> Value {
        schema::find(key).map_or(Value::Null, |s| self.resolve_for(s, language).0)
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

    /// The languages that have settings of their own in any of the files.
    pub fn languages(&self) -> BTreeSet<String> {
        self.files().flat_map(|(f, _)| f.languages.keys().cloned()).collect()
    }

    /// A language's own settings, by key: what its blocks give, the project's over the person's.
    pub fn language_values(&self, language: &str) -> Map<String, Value> {
        let mut own = Map::new();
        for (file, _) in self.files() {
            if let Some(block) = file.languages.get(language) {
                own.extend(block.clone());
            }
        }
        own
    }

    /// Whether a file was read for a layer (it is there, and is a JSON object).
    pub fn has(&self, layer: Layer) -> bool {
        self.file(layer).is_some()
    }

    fn file(&self, layer: Layer) -> Option<&File> {
        match layer {
            Layer::User => self.user.as_ref(),
            Layer::Project => self.project.as_ref(),
            Layer::Workspace => self.workspace.as_ref(),
        }
    }

    /// What a layer's file sets itself, outside any language's block (`None`: it does not).
    pub fn written(&self, layer: Layer, key: &str) -> Option<&Value> {
        self.file(layer)?.values.get(key)
    }

    /// What a layer's file holds that is no setting of Code Arena's, by key (nested ones by their whole name).
    pub fn unknown(&self, layer: Layer) -> &[String] {
        self.file(layer).map_or(&[], |f| f.unknown.as_slice())
    }

    pub fn env(&self) -> &Env {
        &self.env
    }
}

fn source(file: &File, layer: Layer) -> Source {
    match layer {
        Layer::User => Source::User(file.path.clone()),
        Layer::Project | Layer::Workspace => Source::Project(file.path.clone()),
    }
}

/// A project's value of a setting both may set. Permissions: the project's `deny` is added to the person's, and its
/// `allow` is never read, so a cloned repository cannot let itself run commands unasked.
fn merge_project(setting: &Setting, user: Option<Value>, project: Value) -> Value {
    if setting.key == schema::PLUGINS.key {
        // A project enables more of the plugins installed; it does not take yours away.
        let mut names: Vec<Value> = user.and_then(|u| u.as_array().cloned()).unwrap_or_default();
        for p in project.as_array().into_iter().flatten() {
            if !names.contains(p) {
                names.push(p.clone());
            }
        }
        return Value::Array(names);
    }
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

    #[test]
    fn a_value_over_several_lines_is_kept_and_forgotten_whole() {
        let d = tempfile::tempdir().expect("temp");
        let f = d.path().join("settings.json");
        std::fs::write(
            &f,
            "{\n  // mine\n  \"agent.mode\": \"ask\",\n  \"hooks\": {\n    \"Stop\": [{\"hooks\": [{\"command\": \"echo } done\"}]}]\n  },\n  \"agent.maxSteps\": 40 // enough\n}\n",
        )
        .expect("w");
        keep(&f, "hooks", &serde_json::json!({})).expect("keep");
        let t = std::fs::read_to_string(&f).expect("r");
        assert!(t.contains("\"hooks\": {},\n  \"agent.maxSteps\": 40"), "{t}");
        assert!(serde_json::from_str::<Value>(&strip(&t)).is_ok(), "{t}");
        forget(&f, "agent.maxSteps").expect("forget");
        let t = std::fs::read_to_string(&f).expect("r");
        assert!(!t.contains("maxSteps") && t.contains("\"hooks\": {}\n}"), "the comma before goes too: {t}");
        assert!(t.contains("// mine"), "comments stay: {t}");
        forget(&f, "agent.mode").expect("forget");
        forget(&f, "not.there").expect("nothing");
        assert!(serde_json::from_str::<Value>(&strip(&std::fs::read_to_string(&f).expect("r"))).is_ok());
    }

    /// The file without its comments, as JSON.
    fn strip(t: &str) -> String {
        t.lines()
            .map(|l| if l.trim_start().starts_with("//") { "" } else { l.split(" //").next().unwrap_or(l) })
            .collect::<Vec<_>>()
            .join("\n")
    }
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

/// The line a value that starts on line `i` ends on: its brackets and braces counted outside strings and comments
/// (a value written over several lines, an object by hand).
fn value_end(lines: &[&str], i: usize) -> usize {
    let mut depth: i64 = 0;
    let mut in_string = false;
    for (n, line) in lines.iter().enumerate().skip(i) {
        let mut chars = line.chars().peekable();
        // On the key's line, what comes after the key's colon.
        let mut seen_colon = n > i;
        let mut escaped = false;
        while let Some(c) = chars.next() {
            if in_string {
                match (escaped, c) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, '"') => in_string = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => in_string = true,
                ':' => seen_colon = true,
                '/' if chars.peek() == Some(&'/') => break,
                '{' | '[' if seen_colon => depth += 1,
                '}' | ']' if seen_colon => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 && !in_string {
            return n;
        }
    }
    lines.len().saturating_sub(1)
}

/// Takes one setting out of a settings file, the rest as it is (its comments, its order). A setting not there is no
/// change.
pub fn forget(file: &Path, key: &str) -> std::io::Result<()> {
    let Ok(text) = std::fs::read_to_string(file) else { return Ok(()) };
    let quoted = format!("\"{key}\"");
    let lines: Vec<&str> = text.lines().collect();
    let Some(i) = lines.iter().position(|l| l.trim_start().starts_with(&format!("{quoted}:"))) else { return Ok(()) };
    let end = value_end(&lines, i);
    let had_comma = lines[end].split("//").next().unwrap_or("").trim_end().ends_with(',');
    let mut out: Vec<String> = lines[..i].iter().map(|l| l.to_string()).collect();
    // It was the last setting: the one before it loses its comma.
    if !had_comma
        && let Some(prev) = out.iter_mut().rev().find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("//"))
    {
        let (code, comment) = match prev.find("//") {
            Some(at) if !prev.trim_start().starts_with("//") => (prev[..at].to_string(), prev[at..].to_string()),
            _ => (prev.clone(), String::new()),
        };
        if let Some(stripped) = code.trim_end().strip_suffix(',') {
            *prev = if comment.is_empty() { stripped.to_string() } else { format!("{stripped} {comment}") };
        }
    }
    out.extend(lines[end + 1..].iter().map(|l| l.to_string()));
    std::fs::write(file, out.join("\n") + "\n")
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
        // In place: its lines (a value written over several), its comma kept.
        let all: Vec<&str> = text.lines().collect();
        let end = value_end(&all, i);
        let indent: String = all[i].chars().take_while(|c| c.is_whitespace()).collect();
        let comma = if all[end].split("//").next().unwrap_or("").trim_end().ends_with(',') { "," } else { "" };
        let mut lines: Vec<String> = all[..i].iter().map(|l| l.to_string()).collect();
        lines.push(format!("{indent}{quoted}: {val}{comma}"));
        lines.extend(all[end + 1..].iter().map(|l| l.to_string()));
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
