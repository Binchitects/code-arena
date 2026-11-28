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

    /// A problem kept: with the IDE's when it is about one of the IDE's settings, a language's block or VS Code's
    /// file.
    fn say(&mut self, file: &Path, layer: Layer, key: &str, message: String) {
        let ide = layer == Layer::Workspace || key.starts_with('[') || ide_schema::ALL.iter().any(|s| s.key == key);
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

    /// The same settings read again from their files (one was saved, or changed on disk): the environment and what
    /// was given for this run stay as they are.
    pub fn again(&self, user_file: &Path, project_file: Option<&Path>) -> Settings {
        let mut s = Settings::load(user_file, project_file, self.env.clone());
        s.flags = self.flags.clone();
        s
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
    if setting.key == schema::TOOL_SETTINGS.key {
        // Read level by level, the project's only for what it may say (ca_agent::tool_settings).
        return user.unwrap_or_else(|| setting.default_value());
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

    /// A project with both of its settings files, and the person's file beside it.
    fn project(user: &str, code_arena: &str, vscode: &str) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let u = dir.path().join("user.json");
        let p = dir.path().join("work/.code-arena/settings.json");
        let w = dir.path().join("work/.vscode/settings.json");
        for (file, text) in [(&u, user), (&p, code_arena), (&w, vscode)] {
            std::fs::create_dir_all(file.parent().expect("in a folder")).expect("folder");
            if !text.is_empty() {
                std::fs::write(file, text).expect("write");
            }
        }
        (dir, u, p, w)
    }

    #[test]
    fn each_layer_over_the_one_before_and_a_languages_own_over_them_all() {
        let tab = schema::find("editor.tabSize").expect("declared");
        let read = |user: &str, code_arena: &str, vscode: &str| {
            let (d, u, p, w) = project(user, code_arena, vscode);
            (Settings::load(&u, Some(&p), Env::default()), d, u, p, w)
        };
        // The default, then the person's file, then the project's own, then VS Code's beside it.
        let (s, ..) = read("", "", "");
        assert_eq!(s.resolve(tab), (json!(4), Source::Default));
        assert!(!s.has(Layer::User) && !s.has(Layer::Project) && !s.has(Layer::Workspace));
        let (s, _d, u, ..) = read(r#"{"editor.tabSize": 8}"#, "", "");
        assert_eq!(s.resolve(tab), (json!(8), Source::User(u)));
        let (s, _d, _u, p, _w) = read(r#"{"editor.tabSize": 8}"#, r#"{"editor.tabSize": 3}"#, "");
        assert_eq!(s.resolve(tab), (json!(3), Source::Project(p)));
        let (s, _d, _u, _p, w) =
            read(r#"{"editor.tabSize": 8}"#, r#"{"editor.tabSize": 3}"#, r#"{"editor.tabSize": 2}"#);
        assert_eq!(s.resolve(tab), (json!(2), Source::Project(w.clone())));
        assert_eq!(s.written(Layer::User, "editor.tabSize"), Some(&json!(8)));
        assert_eq!(s.written(Layer::Project, "editor.tabSize"), Some(&json!(3)));
        assert_eq!(s.written(Layer::Workspace, "editor.tabSize"), Some(&json!(2)));
        // No language's own: a language reads what every language does.
        assert_eq!(s.resolve_for(tab, Some("python")), (json!(2), Source::Project(w)));
        // A language's own, even from the person's file, wins over the project's value for every language.
        let mine = r#"{"editor.tabSize": 8, "[python]": {"editor.tabSize": 6}}"#;
        let (s, _d, u, ..) = read(mine, r#"{"editor.tabSize": 3}"#, r#"{"editor.tabSize": 2}"#);
        assert_eq!(s.resolve_for(tab, Some("python")), (json!(6), Source::User(u)));
        assert_eq!(s.resolve_for(tab, Some("rust")).0, json!(2));
        assert_eq!(s.resolve(tab).0, json!(2));
        assert_eq!((s.ide("editor.tabSize", Some("python")), s.ide("editor.tabSize", None)), (json!(6), json!(2)));
        assert_eq!(s.ide("not.a.setting", None), Value::Null);
        // Among the languages' own, the project's over the person's, and VS Code's file over the project's own.
        let (s, _d, _u, p, _w) = read(mine, r#"{"[python]": {"editor.tabSize": 5}}"#, "");
        assert_eq!(s.resolve_for(tab, Some("python")), (json!(5), Source::Project(p)));
        let (s, _d, _u, _p, w) =
            read(mine, r#"{"[python]": {"editor.tabSize": 5}}"#, r#"{"[python]": {"editor.tabSize": 7}}"#);
        assert_eq!(s.resolve_for(tab, Some("python")), (json!(7), Source::Project(w)));
        assert_eq!(s.languages().into_iter().collect::<Vec<_>>(), ["python"]);
        assert_eq!(s.language_values("python").get("editor.tabSize"), Some(&json!(7)));
        // What was given for this run and the environment still win, and are kept when the files are read again.
        let (mut s, _d, u, p, _w) = read(mine, "", "");
        s.set_flag(&MODE, json!("plan"));
        std::fs::write(&u, r#"{"editor.tabSize": 2, "agent.mode": "yolo"}"#).expect("write");
        let s = s.again(&u, Some(&p));
        assert_eq!(s.resolve(&MODE), (json!("plan"), Source::Flag));
        assert_eq!((s.ide("editor.tabSize", None), s.ide("editor.tabSize", Some("python"))), (json!(2), json!(2)));
    }

    #[test]
    fn a_block_of_several_languages_nested_names_and_what_is_not_taken() {
        let (_d, u, p, w) = project(
            r#"{
              // VS Code's nested way of writing a name, and a block for two languages.
              "editor": {"fontSize": 15, "minimap": {"enabled": false}},
              "[javascript][typescript]": {"editor.tabSize": 2, "files.autoSave": "afterDelay"},
              "[markdown]": {"editor": {"wordWrap": "on"}, "editor.tabSize": "wide"},
              "[go]": 4,
              "editor.rulers": "80",
              "tools.shellTimeout": "soon",
              "somebody.elses": true
            }"#,
            r#"{"terminal.integrated.allowChords": false, "editor.lineNumbers": "relative"}"#,
            r#"{"codeArena.inlineCompletion.enabled": true, "agent.mode": "yolo", "editor.fontSize": 3}"#,
        );
        let s = Settings::load(&u, Some(&p), Env::default());
        let of = |key: &str, language: Option<&str>| s.ide(key, language);
        assert_eq!((of("editor.fontSize", None), of("editor.minimap.enabled", None)), (json!(15), json!(false)));
        assert_eq!(
            (of("editor.tabSize", Some("javascript")), of("editor.tabSize", Some("typescript"))),
            (json!(2), json!(2))
        );
        assert_eq!(of("editor.wordWrap", Some("markdown")), json!("on"));
        assert_eq!(of("editor.lineNumbers", None), json!("relative"));
        // Not taken: a setting that is not a language's to set, a value of the wrong kind (its default holds), a
        // block that is no object, the person's own in a project's file, a value out of its bounds.
        assert_eq!(of("files.autoSave", Some("javascript")), json!("off"));
        assert_eq!(of("editor.tabSize", Some("markdown")), json!(4));
        assert_eq!(of("editor.rulers", None), json!([]));
        assert_eq!(of("terminal.integrated.allowChords", None), json!(true));
        assert_eq!(s.value(&MODE), json!("ask"));
        // Each is listed with its setting and its file. The IDE's own, and all of VS Code's file, are the IDE's to
        // say; the terminal says what is its own.
        let said = |list: &[Problem]| -> Vec<(String, String, String)> {
            let name = |f: &Path| {
                let dir = f.parent().and_then(Path::file_name).unwrap_or_default().to_string_lossy().into_owned();
                format!("{dir}/{}", f.file_name().unwrap_or_default().to_string_lossy())
            };
            list.iter().map(|p| (name(&p.file), p.key.clone(), p.message.clone())).collect()
        };
        let user = format!("{}/user.json", u.parent().and_then(Path::file_name).expect("dir").to_string_lossy());
        let pair = |file: &str, key: &str, message: &str| (file.to_string(), key.to_string(), message.to_string());
        assert_eq!(
            said(&s.problems),
            [pair(&user, "tools.shellTimeout", "\"tools.shellTimeout\" takes a whole number. Its default holds.")]
        );
        assert_eq!(
            said(&s.ide_problems),
            [
                pair(&user, "files.autoSave", "\"files.autoSave\" cannot be set for one language."),
                pair(&user, "editor.tabSize", "\"editor.tabSize\" takes a whole number. Its default holds."),
                pair(&user, "[go]", "\"[go]\" holds an object of settings."),
                pair(&user, "editor.rulers", "\"editor.rulers\" is a list of numbers. Its default holds."),
                pair(
                    ".code-arena/settings.json",
                    "terminal.integrated.allowChords",
                    "terminal.integrated.allowChords is the person's own: a project's settings cannot set it"
                ),
                pair(
                    ".vscode/settings.json",
                    "codeArena.inlineCompletion.enabled",
                    "codeArena.inlineCompletion.enabled is the person's own: a project's settings cannot set it"
                ),
                pair(
                    ".vscode/settings.json",
                    "agent.mode",
                    "agent.mode is the person's own: a project's settings cannot set it"
                ),
                pair(
                    ".vscode/settings.json",
                    "editor.fontSize",
                    "\"editor.fontSize\" is from 6 to 100. Its default holds."
                ),
            ]
        );
        // What is no setting of Code Arena's is left in the file, and listed apart.
        assert_eq!(s.unknown(Layer::User), ["somebody.elses"]);
        // A file of VS Code's that cannot be read is the IDE's to say too, and is no layer.
        std::fs::write(&w, "{ not json").expect("write");
        let s = Settings::load(&u, Some(&p), Env::default());
        assert!(!s.has(Layer::Workspace) && s.problems.len() == 1);
        assert!(s.ide_problems.iter().any(|p| p.file == w && p.key.is_empty() && p.message.starts_with("not read:")));
        // Only a project's `.code-arena/settings.json` has a file of VS Code's beside it.
        assert_eq!(workspace_file(&p), Some(w));
        assert_eq!(workspace_file(&u), None);
        assert_eq!(languages_of("[javascript][typescript]"), Some(vec!["javascript", "typescript"]));
        assert_eq!((languages_of("[]"), languages_of("[a]b"), languages_of("editor.tabSize")), (None, None, None));
    }

    #[test]
    fn a_flag_wins() {
        let mut s = Settings::defaults(Env::from_pairs([("CODE_ARENA_MODE", "plan")]));
        s.set_flag(&MODE, json!("yolo"));
        assert_eq!(s.resolve(&MODE), (json!("yolo"), Source::Flag));
    }
}

/// One setting as a settings file's text has it: its name, where its name begins, where its value is, and the comma
/// after it when it has one.
struct Member {
    key: String,
    start: usize,
    value: std::ops::Range<usize>,
    comma: Option<usize>,
}

/// An object of a settings file's text (the file's own, or a language's block): its braces and what is in it.
struct Object {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

/// Past the spaces and the comments from `i`.
fn past_space(b: &[u8], mut i: usize) -> usize {
    loop {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        match (b.get(i), b.get(i + 1)) {
            (Some(b'/'), Some(b'/')) => {
                while b.get(i).is_some_and(|c| *c != b'\n') {
                    i += 1;
                }
            }
            (Some(b'/'), Some(b'*')) => {
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(b.len());
            }
            _ => return i,
        }
    }
}

/// Past the string that opens at `i`.
fn past_string(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// Past the value that begins at `i`: a string, an object or a list with all that is in it (its strings and its
/// comments read as such), or a word (a number, true, null).
fn past_value(b: &[u8], i: usize) -> Option<usize> {
    match *b.get(i)? {
        b'"' => past_string(b, i),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = past_string(b, j)?;
                        continue;
                    }
                    b'/' if matches!(b.get(j + 1), Some(b'/' | b'*')) => {
                        j = past_space(b, j);
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while b.get(j).is_some_and(|c| !matches!(c, b',' | b'}' | b']' | b'/') && !c.is_ascii_whitespace()) {
                j += 1;
            }
            (j > i).then_some(j)
        }
    }
}

/// The object whose brace is at `open`, read as far as its own settings: what is in their values is passed over.
/// None when the text is not an object there.
fn object_at(text: &str, open: usize) -> Option<Object> {
    let b = text.as_bytes();
    if b.get(open) != Some(&b'{') {
        return None;
    }
    let mut members = Vec::new();
    let mut i = past_space(b, open + 1);
    loop {
        match *b.get(i)? {
            b'}' => return Some(Object { open, close: i, members }),
            b'"' => {
                let end = past_string(b, i)?;
                let key: String = serde_json::from_str(text.get(i..end)?).ok()?;
                let colon = past_space(b, end);
                if b.get(colon) != Some(&b':') {
                    return None;
                }
                let from = past_space(b, colon + 1);
                let to = past_value(b, from)?;
                let after = past_space(b, to);
                let comma = (b.get(after) == Some(&b',')).then_some(after);
                members.push(Member { key, start: i, value: from..to, comma });
                i = past_space(b, comma.map_or(after, |c| c + 1));
            }
            _ => return None,
        }
    }
}

/// Where the line `i` is on begins.
fn line_start(text: &str, i: usize) -> usize {
    text[..i].rfind('\n').map_or(0, |n| n + 1)
}

/// The spaces the line `i` is on begins with.
fn indent_at(text: &str, i: usize) -> &str {
    let line = &text[line_start(text, i)..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// Where the line `i` is on ends (before its line break).
fn line_end(text: &str, i: usize) -> usize {
    let end = text[i..].find('\n').map_or(text.len(), |n| i + n);
    if end > i && text.as_bytes()[end - 1] == b'\r' { end - 1 } else { end }
}

/// Whether nothing but a `//` comment follows on the line, from `i`.
fn line_is_over(text: &str, i: usize) -> bool {
    let rest = text[i..line_end(text, i)].trim();
    rest.is_empty() || rest.starts_with("//")
}

/// An object's text with one setting in it: its value changed where it stands, or the setting added after the
/// last one, on a line of its own when the last one has its own.
fn put(text: &str, object: &Object, key: &str, value: &str) -> String {
    if let Some(m) = object.members.iter().rev().find(|m| m.key == key) {
        return format!("{}{value}{}", &text[..m.value.start], &text[m.value.end..]);
    }
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let member = format!("{}: {value}", Value::String(key.to_string()));
    let outer = indent_at(text, object.open);
    let Some(last) = object.members.last() else {
        let head = &text[..object.open + 1];
        return if text[object.open + 1..object.close].contains('\n') {
            // Before the comments it holds.
            format!("{head}{nl}{outer}  {member}{}", &text[object.open + 1..])
        } else {
            format!("{head}{nl}{outer}  {member}{nl}{outer}{}", text[object.open + 1..].trim_start_matches([' ', '\t']))
        };
    };
    let at = last.comma.map_or(last.value.end, |c| c + 1);
    let comma = if last.comma.is_some() { "" } else { "," };
    if line_is_over(text, at) {
        // On the next line, as the last one is written; a comment after the last one stays with it.
        let indent = if text[line_start(text, last.start)..last.start].trim().is_empty() {
            indent_at(text, last.start).to_string()
        } else {
            format!("{outer}  ")
        };
        let end = line_end(text, at);
        format!("{}{comma}{}{nl}{indent}{member}{}", &text[..at], &text[at..end], &text[end..])
    } else {
        format!("{}{comma} {member}{}", &text[..at], &text[at..])
    }
}

/// An object's text without one setting: its lines gone when they are its own, and the comma of the one before it
/// when it was the last. None when the setting is not there.
fn take_out(text: &str, object: &Object, key: &str) -> Option<String> {
    let at = object.members.iter().rposition(|m| m.key == key)?;
    let m = &object.members[at];
    let before = at.checked_sub(1).map(|n| &object.members[n]);
    let end = m.comma.map_or(m.value.end, |c| c + 1);
    let from = line_start(text, m.start);
    let own_lines = text[from..m.start].trim().is_empty() && line_is_over(text, end);
    let (cut, to) = if own_lines {
        (from, text[end..].find('\n').map_or(text.len(), |n| end + n + 1))
    } else {
        // Among others on its line: it goes with the spaces after it, or, the last, with those before it.
        match (m.comma, before) {
            (None, Some(b)) => (b.comma.map_or(b.value.end, |c| c + 1), end),
            _ => (m.start, end + text[end..].len() - text[end..].trim_start_matches([' ', '\t']).len()),
        }
    };
    let mut out = format!("{}{}", &text[..cut], &text[to..]);
    // It was the last one: the one before it ends the object, without a comma.
    if m.comma.is_none()
        && let Some(c) = before.and_then(|b| b.comma)
    {
        out.replace_range(c..c + 1, "");
    }
    Some(out)
}

/// A settings file's text with one setting kept in it (`value`), or taken out of it (none): for every language,
/// or in a language's own block (`"[python]": {…}`, made when it is not there). The rest of the text stays as it
/// is: its comments, its order, its other settings. An empty text is an empty object.
pub fn edited(text: &str, language: Option<&str>, key: &str, value: Option<&Value>) -> Result<String, String> {
    let text = if text.trim().is_empty() { "{\n}\n" } else { text };
    let not_object = || "it is not a JSON object of settings".to_string();
    let root = object_at(text, past_space(text.as_bytes(), 0)).ok_or_else(not_object)?;
    let written = value.map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".into()));
    let set = |text: &str, object: &Object| match &written {
        Some(v) => put(text, object, key, v),
        None => take_out(text, object, key).unwrap_or_else(|| text.to_string()),
    };
    let Some(language) = language else { return Ok(set(text, &root)) };
    let block = format!("[{language}]");
    let inner = |text: &str, root: &Object| {
        let m = root.members.iter().rev().find(|m| m.key == block)?;
        Some(object_at(text, m.value.start).ok_or_else(|| format!("\"{block}\" holds an object of settings")))
    };
    match inner(text, &root) {
        Some(found) => Ok(set(text, &found?)),
        None if written.is_none() => Ok(text.to_string()),
        None => {
            let with = put(text, &root, &block, "{}");
            let root = object_at(&with, root.open).ok_or_else(not_object)?;
            let made = inner(&with, &root).ok_or_else(not_object)??;
            Ok(set(&with, &made))
        }
    }
}

/// A settings file changed by [`edited`] and written back; a file not there yet is made, with its folder.
fn edit(file: &Path, language: Option<&str>, key: &str, value: Option<&Value>) -> std::io::Result<()> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let out =
        edited(&text, language, key, value).map_err(|e| std::io::Error::other(format!("{}: {e}", file.display())))?;
    if out == text {
        return Ok(());
    }
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(file, out)
}

/// Takes one setting out of a settings file, the rest as it is (its comments, its order). A setting not there, or
/// a file not there, is no change.
pub fn forget(file: &Path, key: &str) -> std::io::Result<()> {
    forget_for(file, None, key)
}

/// As [`forget`], out of a language's own block when one is given.
pub fn forget_for(file: &Path, language: Option<&str>, key: &str) -> std::io::Result<()> {
    if !file.exists() {
        return Ok(());
    }
    edit(file, language, key, None)
}

/// Keeps one setting in a settings file (the person's `settings.json`), the rest of it as it is: its comments, its
/// order, its other settings. A file not there yet is made.
pub fn keep(file: &Path, key: &str, value: &Value) -> std::io::Result<()> {
    keep_for(file, None, key, value)
}

/// As [`keep`], in a language's own block (`"[python]": {…}`) when one is given: the block is made when it is not
/// there, and the same setting outside it is left as it is.
pub fn keep_for(file: &Path, language: Option<&str>, key: &str, value: &Value) -> std::io::Result<()> {
    edit(file, language, key, Some(value))
}

#[cfg(test)]
mod keep_tests {
    use super::*;

    fn read(file: &Path) -> (String, Value) {
        let text = std::fs::read_to_string(file).expect("read");
        let value = crate::jsonc::parse(&text).unwrap_or_else(|e| panic!("still JSON with comments ({e}): {text}"));
        (text, value)
    }

    #[test]
    fn a_setting_kept_for_every_language_leaves_a_languages_own_and_the_other_way() {
        let d = tempfile::tempdir().expect("temp");
        let f = d.path().join("settings.json");
        let tab = "editor.tabSize";
        std::fs::write(
            &f,
            "{\n  // \"editor.tabSize\": 1,\n  \"[python]\": {\n    \"editor.tabSize\": 2 // narrow\n  },\n  \"files.eol\": \"\\\"editor.tabSize\\\": 9\",\n  \"editor.tabSize\": 4\n}\n",
        )
        .expect("w");
        // For every language: the one outside the blocks, not the first line that names it.
        keep(&f, tab, &serde_json::json!(8)).expect("kept");
        let (t, v) = read(&f);
        assert_eq!((&v[tab], &v["[python]"][tab]), (&serde_json::json!(8), &serde_json::json!(2)), "{t}");
        assert!(t.contains("// \"editor.tabSize\": 1,") && t.contains("2 // narrow") && t.contains("9\","), "{t}");
        // For one language: in its block, the comment beside it kept; one more goes on a line of its own there.
        keep_for(&f, Some("python"), tab, &serde_json::json!(3)).expect("kept");
        keep_for(&f, Some("python"), "editor.insertSpaces", &serde_json::json!(false)).expect("added");
        let (t, v) = read(&f);
        assert_eq!((&v[tab], &v["[python]"][tab]), (&serde_json::json!(8), &serde_json::json!(3)), "{t}");
        assert!(t.contains("    \"editor.tabSize\": 3, // narrow\n    \"editor.insertSpaces\": false\n  },"), "{t}");
        // A language with no block yet gets one, after the last setting.
        keep_for(&f, Some("rust"), tab, &serde_json::json!(2)).expect("made");
        let (t, v) = read(&f);
        assert_eq!(v["[rust]"][tab], 2, "{t}");
        assert!(t.ends_with("  \"editor.tabSize\": 8,\n  \"[rust]\": {\n    \"editor.tabSize\": 2\n  }\n}\n"), "{t}");
        // Taken out of a block: the block's other settings and the same setting outside it stay.
        forget_for(&f, Some("python"), "editor.insertSpaces").expect("forgotten");
        forget_for(&f, Some("rust"), tab).expect("forgotten");
        forget_for(&f, Some("go"), tab).expect("nothing to forget");
        let (t, v) = read(&f);
        assert!(t.contains("    \"editor.tabSize\": 3 // narrow\n  },") && t.contains("\"[rust]\": {\n  }"), "{t}");
        assert_eq!(
            (&v[tab], &v["[python]"][tab], &v["[rust]"]),
            (&serde_json::json!(8), &serde_json::json!(3), &serde_json::json!({}))
        );
        forget(&f, tab).expect("forgotten");
        let (t, v) = read(&f);
        assert!(v[tab].is_null() && v["[python]"][tab] == 3 && t.contains("// \"editor.tabSize\": 1,"), "{t}");
    }

    #[test]
    fn settings_written_on_one_line_and_files_that_are_not_settings() {
        let on = |text: &str, language: Option<&str>, key: &str, value: Option<Value>| {
            edited(text, language, key, value.as_ref())
        };
        let one = serde_json::json!(1);
        assert_eq!(on("", None, "a", Some(one.clone())).as_deref(), Ok("{\n  \"a\": 1\n}\n"));
        assert_eq!(on("{}", None, "a", Some(one.clone())).as_deref(), Ok("{\n  \"a\": 1\n}"));
        assert_eq!(
            on("{\"a\": 1}", None, "b", Some(serde_json::json!("x"))).as_deref(),
            Ok("{\"a\": 1, \"b\": \"x\"}")
        );
        assert_eq!(on("{\"a\": 1, \"b\": 2}", None, "b", None).as_deref(), Ok("{\"a\": 1}"));
        assert_eq!(on("{\"a\": 1, \"b\": 2}", None, "a", None).as_deref(), Ok("{\"b\": 2}"));
        assert_eq!(on("{\"a\": 1}", None, "a", None).as_deref(), Ok("{}"));
        assert_eq!(
            on("{\"[go]\": {\"a\": 8}, /* all */ \"a\": 1}", Some("go"), "b", Some(one.clone())).as_deref(),
            Ok("{\"[go]\": {\"a\": 8, \"b\": 1}, /* all */ \"a\": 1}")
        );
        assert_eq!(
            on("{\r\n  \"a\": 1,\r\n}\r\n", None, "b", Some(one.clone())).as_deref(),
            Ok("{\r\n  \"a\": 1,\r\n  \"b\": 1\r\n}\r\n"),
            "a comma that trails already is not written twice, and the file's line ends are kept"
        );
        assert_eq!(
            on("{\n  \"[go]\": {}\n}\n", Some("go"), "a", Some(one.clone())).as_deref(),
            Ok("{\n  \"[go]\": {\n    \"a\": 1\n  }\n}\n")
        );
        // Not an object of settings, or a language's block that is none: said, and nothing is written.
        assert!(on("[1, 2]", None, "a", Some(one.clone())).is_err());
        assert!(on("{\"a\": ", None, "a", Some(one.clone())).is_err());
        assert_eq!(
            on("{\"[go]\": 4}", Some("go"), "a", Some(one)),
            Err("\"[go]\" holds an object of settings".to_string())
        );
        let d = tempfile::tempdir().expect("temp");
        let f = d.path().join("settings.json");
        std::fs::write(&f, "[]").expect("w");
        assert!(keep(&f, "a", &serde_json::json!(1)).is_err());
        assert_eq!(std::fs::read_to_string(&f).expect("r"), "[]");
        forget(&d.path().join("none.json"), "a").expect("no file, nothing to forget");
        assert!(!d.path().join("none.json").exists());
    }

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
