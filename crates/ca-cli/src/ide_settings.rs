//! The settings as the window reads them, and the person's own files.
//!
//! - `settings` [T] is answered with `settings{values, by_lang, sources, problems, files}`: every setting's value
//!   in force ([`message`]), each language's own, where each one that is not its default comes from, and what in
//!   the files was not taken. It is said again, unasked, whenever one of the settings files changes: saved from the
//!   window (`set_setting`, `save_user`), or changed on disk by anything else (they are looked at twice a second).
//! - `user_file{name}` [T] is answered with `user_file{name, text, modified, path}` and `save_user{name, text}` [T]
//!   with `saved_user{name, modified, path}` (or `error`), for the files in Code Arena's settings folder that are
//!   the person's to write: `settings.json`, `keybindings.json` and `snippets/NAME` ([`user_path`]). `settings.json`
//!   and `keybindings.json` are said again as `user_file` when they change on disk.
//! - `workspace_settings` [T, file-side] makes the project's `.vscode/settings.json` (an empty object) when it is
//!   not there, never over one that is, and is answered with `workspace_settings{path, made}`.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver. The other areas read one of the IDE's
//! settings with [`current`]: `ide_settings::current(sh).ide("git.autofetch", None)`, the key written as a literal
//! (a test of `ca-settings` reads the calls).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use ca_settings::schema::{self, Kind};
use ca_settings::{Layer, Settings, Source, ide_schema};
use serde_json::{Map, Value, json};

use crate::ide_ext::{Area, Ctx, Rule, Shared, kind, send};
use crate::ide_files::Side;

/// How often the settings files are looked at.
const EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// The biggest file of the person's that is read or written here.
const MOST: u64 = 5 * 1024 * 1024;

/// The project's settings file VS Code keeps, from the project's top.
pub const WORKSPACE_FILE: &str = ".vscode/settings.json";

/// The files the settings are read from: the person's, the project's own, and VS Code's beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Files {
    pub user: PathBuf,
    pub project: PathBuf,
    pub workspace: PathBuf,
}

impl Files {
    pub fn of(paths: &ca_core::Paths, root: &Path) -> Files {
        Files {
            user: paths.settings_file(),
            project: root.join(".code-arena").join("settings.json"),
            workspace: root.join(".vscode").join("settings.json"),
        }
    }

    /// The file a layer is kept in.
    pub fn layer(&self, layer: Layer) -> &Path {
        match layer {
            Layer::User => &self.user,
            Layer::Project => &self.project,
            Layer::Workspace => &self.workspace,
        }
    }

    /// The settings read again from these files (the environment and the command line's stay as they were).
    pub fn read(&self, was: &Settings) -> Settings {
        was.again(&self.user, Some(&self.project))
    }
}

/// Whether a text is a language's id as a settings block names one (`python`, `c++`, `objective-c`).
pub fn language_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || "+-_#.".contains(c))
}

/// The `settings` message: what the window's parts read their settings from.
///
/// - `values`: every setting's value in force, by its key. The agent's own settings that are objects (the MCP
///   servers, the hooks, the permissions…) are left out: they may hold a token, and are edited in `settings.json`.
/// - `by_lang`: for each language that has a block (`"[python]": {…}`), what its blocks set, the project's over the
///   person's. A language's own value wins over `values`.
/// - `sources`: for each setting that is not its default, the file it comes from (`env:NAME` when the environment
///   gives it, `flag` when the command line did).
/// - `problems`: what was not taken, each with its setting (empty: the whole file), its file and why.
/// - `files`: the three files' paths (`user`, `project`, `workspace`), there or not.
pub fn message(s: &Settings, files: &Files) -> Value {
    let (mut values, mut sources) = (Map::new(), Map::new());
    let ide = ide_schema::ALL.iter().map(|s| (*s, true));
    for (setting, ide) in schema::ALL.iter().map(|s| (*s, false)).chain(ide) {
        let (value, source) = s.resolve(setting);
        if ide || !matches!(setting.kind, Kind::Object) {
            values.insert(setting.key.to_string(), value);
        }
        let from = match source {
            Source::Default => continue,
            Source::User(f) | Source::Project(f) => f.display().to_string(),
            Source::Env(name) => format!("env:{name}"),
            Source::Flag => "flag".to_string(),
        };
        sources.insert(setting.key.to_string(), Value::String(from));
    }
    let by_lang: Map<String, Value> =
        s.languages().into_iter().map(|id| (id.clone(), Value::Object(s.language_values(&id)))).collect();
    let problems: Vec<Value> = s
        .problems
        .iter()
        .chain(&s.ide_problems)
        .map(|p| json!({"key": p.key, "file": p.file.display().to_string(), "error": p.message}))
        .collect();
    json!({
        "type": "settings",
        "values": values,
        "by_lang": by_lang,
        "sources": sources,
        "problems": problems,
        "files": {
            "user": files.user.display().to_string(),
            "project": files.project.display().to_string(),
            "workspace": files.workspace.display().to_string(),
        },
    })
}

/// Where one of the person's own files is, by its name: `settings.json`, `keybindings.json`, `snippets/NAME` (one
/// file of the snippets folder, `NAME.json` or `NAME.code-snippets`), or `skills/NAME/SKILL.md` (a skill of theirs).
/// Nothing else in the settings folder is read or written through here: the Arena's key is kept there too.
pub fn user_path(config: &Path, name: &str) -> Result<PathBuf, String> {
    if matches!(name, "settings.json" | "keybindings.json") {
        return Ok(config.join(name));
    }
    let plain = |n: &str| {
        !n.starts_with('.')
            && n.chars().all(|c| c.is_alphanumeric() || " ._+#-".contains(c))
            && [".json", ".code-snippets"].iter().any(|end| n.len() > end.len() && n.ends_with(end))
    };
    // A skill of yours on this computer, as the Skills view opens it: only its SKILL.md.
    if let Some(dir) = name.strip_prefix("skills/").and_then(|n| n.strip_suffix("/SKILL.md"))
        && ca_agent::library::good_skill_name(dir)
    {
        return Ok(config.join("skills").join(dir).join("SKILL.md"));
    }
    match name.strip_prefix("snippets/") {
        Some(n) if plain(n) => Ok(config.join("snippets").join(n)),
        _ => Err(format!("{name} is not a file of yours that is opened here")),
    }
}

/// When a file was last changed, in milliseconds, and how long it is; none: it is not there.
fn stamp(file: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(file).ok()?;
    let at = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
    Some((at.map_or(0, |d| d.as_millis() as u64), meta.len()))
}

/// The `user_file` message: one of the person's files as it is on disk. One not there yet is an empty text with no
/// `modified`.
pub fn read_user(config: &Path, name: &str) -> Value {
    let fail = |e: String| json!({"type": "user_file", "name": name, "error": e});
    let path = match user_path(config, name) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let shown = path.display().to_string();
    let Some((modified, size)) = stamp(&path) else {
        return json!({"type": "user_file", "name": name, "text": "", "modified": null, "path": shown});
    };
    if size > MOST {
        return fail(format!("{shown} is too big to open here"));
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => json!({"type": "user_file", "name": name, "text": text, "modified": modified, "path": shown}),
        Err(e) => fail(format!("{shown}: {e}")),
    }
}

/// One of the person's files written (its folder made when it is not there), and the `saved_user` message.
pub fn write_user(config: &Path, name: &str, text: &str) -> Value {
    let fail = |e: String| json!({"type": "saved_user", "name": name, "error": e});
    let path = match user_path(config, name) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    if text.len() as u64 > MOST {
        return fail(format!("{name} is too big to keep here"));
    }
    let written = match path.parent() {
        Some(dir) => std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, text)),
        None => std::fs::write(&path, text),
    };
    match written {
        Ok(()) => json!({
            "type": "saved_user",
            "name": name,
            "modified": stamp(&path).map(|s| s.0),
            "path": path.display().to_string(),
        }),
        Err(e) => fail(format!("{}: {e}", path.display())),
    }
}

/// The project's `.vscode/settings.json` made as an empty object when it is not there (never over one that is),
/// and the `workspace_settings` message.
pub fn make_workspace_file(side: &Side) -> Value {
    let fail = |e: String| json!({"type": "workspace_settings", "path": WORKSPACE_FILE, "error": e});
    let real = match side.entry(WORKSPACE_FILE) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let made = match real.parent() {
        Some(dir) => std::fs::create_dir_all(dir),
        None => Ok(()),
    }
    .and_then(|()| {
        use std::io::Write as _;
        std::fs::OpenOptions::new().write(true).create_new(true).open(&real)?.write_all(b"{\n}\n")
    });
    match made {
        Ok(()) => json!({"type": "workspace_settings", "path": WORKSPACE_FILE, "made": true}),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            json!({"type": "workspace_settings", "path": WORKSPACE_FILE, "made": false})
        }
        Err(e) => fail(e.to_string()),
    }
}

/// What this area keeps for as long as the driver runs: the settings as they were last read, and what the files
/// looked like then.
#[derive(Default)]
struct Live(Mutex<Seen>);

#[derive(Default)]
struct Seen {
    /// Counted up each time the watching begins again (another session, another folder): the one before ends.
    run: u64,
    /// The three settings files and `keybindings.json`, as they were at the last look.
    stamps: [Option<(u64, u64)>; 4],
    /// The settings read since the session last stood still (none: the session's own are the newest).
    now: Option<Arc<Settings>>,
}

impl Live {
    fn seen(&self) -> MutexGuard<'_, Seen> {
        match self.0.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        }
    }
}

/// The files looked at for a change: the three settings files, then the person's `keybindings.json`.
fn watched(sh: &Shared) -> (Files, PathBuf) {
    (Files::of(&sh.paths, sh.workspace.root()), sh.paths.config.join("keybindings.json"))
}

fn stamps(files: &Files, keys: &Path) -> [Option<(u64, u64)>; 4] {
    [stamp(&files.user), stamp(&files.project), stamp(&files.workspace), stamp(keys)]
}

/// The settings as they are now: read again since the session last stood still when a file changed meanwhile
/// (while a turn runs, `sh.settings` is as it was before the turn).
pub fn current(sh: &Shared) -> Arc<Settings> {
    sh.kept.of::<Live>().seen().now.clone().unwrap_or_else(|| sh.settings.clone())
}

/// The driver kept a setting itself (`set_setting`) and read the settings again: they are said to the faces at
/// once, and the files are known as they are now, so the next look does not say them a second time.
pub fn kept(sh: &Shared, settings: &Settings) {
    let (files, keys) = watched(sh);
    {
        let live = sh.kept.of::<Live>();
        let mut seen = live.seen();
        let now = stamps(&files, &keys);
        // The keys' file, if it changed in the same moment, is still the next look's to say.
        seen.stamps[..3].copy_from_slice(&now[..3]);
        seen.now = Some(Arc::new(settings.clone()));
    }
    send(&sh.out, message(settings, &files));
}

/// One look at the files: when a settings file changed, the settings are read again and said (`settings`, the
/// theme when it is another, and the driver is told to take them for the session); the person's `settings.json`
/// and `keybindings.json` are said as `user_file` when they changed, for the tabs and the keys that show them.
/// It reads files: never called on the driver's own thread.
fn look(sh: &Shared, run: Option<u64>) {
    let (files, keys) = watched(sh);
    let live = sh.kept.of::<Live>();
    let mut seen = live.seen();
    if run.is_some_and(|r| r != seen.run) {
        return;
    }
    let now = stamps(&files, &keys);
    let was = std::mem::replace(&mut seen.stamps, now);
    if was[..3] != now[..3] {
        let before = seen.now.clone().unwrap_or_else(|| sh.settings.clone());
        // Read with the lock held, so that a setting kept meanwhile is never said over by what was read before it.
        let fresh = Arc::new(files.read(&before));
        seen.now = Some(fresh.clone());
        send(&sh.out, message(&fresh, &files));
        let theme = |s: &Settings| s.string(&schema::COLOR_THEME).unwrap_or_default();
        if theme(&before) != theme(&fresh) {
            send(&sh.out, json!({"type": "theme", "theme": theme(&fresh)}));
        }
        sh.post(json!({"type": "_settings_read"}));
    }
    drop(seen);
    if was[0] != now[0] {
        send(&sh.out, read_user(&sh.paths.config, "settings.json"));
    }
    if was[3] != now[3] {
        send(&sh.out, read_user(&sh.paths.config, "keybindings.json"));
    }
}

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("settings", Rule::Turn),
        ("user_file", Rule::Turn),
        ("save_user", Rule::Turn),
        ("workspace_settings", Rule::Side),
        ("_settings_read", Rule::Latest),
    ];

    /// The settings files are looked at from now on, for this session and folder; the looking begun for the one
    /// before ends.
    fn start(sh: &Shared) {
        let (files, keys) = watched(sh);
        let live = sh.kept.of::<Live>();
        let run = {
            let mut seen = live.seen();
            seen.run += 1;
            seen.stamps = stamps(&files, &keys);
            seen.now = None;
            seen.run
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let sh = sh.clone();
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(EVERY).await;
                if live.seen().run != run {
                    break;
                }
                // Four files' stamps: quick. Only a change is read, and that on a thread of its own.
                let (files, keys) = watched(&sh);
                if live.seen().stamps != stamps(&files, &keys) {
                    let sh = sh.clone();
                    let _ = tokio::task::spawn_blocking(move || look(&sh, Some(run))).await;
                }
            }
        });
    }

    fn side(side: &Side, m: &Value) -> bool {
        if kind(m) != "workspace_settings" {
            return false;
        }
        send(side.out(), make_workspace_file(side));
        true
    }

    fn shared(sh: &Shared, m: &Value) -> bool {
        let name = m["name"].as_str().unwrap_or("").to_string();
        match kind(m) {
            // From what is read already: no file is touched.
            "settings" => send(&sh.out, message(&current(sh), &watched(sh).0)),
            "user_file" => {
                let (config, out) = (sh.paths.config.clone(), sh.out.clone());
                tokio::task::spawn_blocking(move || send(&out, read_user(&config, &name)));
            }
            "save_user" => {
                let (sh, text) = (sh.clone(), m["text"].as_str().unwrap_or("").to_string());
                tokio::task::spawn_blocking(move || {
                    send(&sh.out, write_user(&sh.paths.config, &name, &text));
                    // What it changed is said at once, not at the next look.
                    look(&sh, None);
                });
            }
            _ => return false,
        }
        true
    }

    /// The settings read again while the session was busy or stood still become the session's own.
    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        if kind(m) != "_settings_read" {
            return false;
        }
        if let Some(now) = ctx.sh.kept.of::<Live>().seen().now.clone() {
            ctx.app.settings = (*now).clone();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use ca_settings::Env;

    use super::*;

    /// A settings folder and a project, with their three settings files.
    fn folders(user: &str, project: &str, workspace: &str) -> (tempfile::TempDir, ca_core::Paths, Files) {
        let d = tempfile::tempdir().expect("temp");
        let paths = ca_core::Paths::under(&d.path().join("home"));
        let files = Files::of(&paths, &d.path().join("work"));
        for (file, text) in [(&files.user, user), (&files.project, project), (&files.workspace, workspace)] {
            std::fs::create_dir_all(file.parent().expect("in a folder")).expect("folder");
            if !text.is_empty() {
                std::fs::write(file, text).expect("write");
            }
        }
        (d, paths, files)
    }

    #[test]
    fn the_settings_are_said_with_each_languages_own_where_they_come_from_and_what_was_not_taken() {
        let (_d, _paths, files) = folders(
            r#"{
                "editor.fontSize": 15,
                "editor.tabSize": "wide",
                "agent.maxSteps": 40,
                "[python]": {"editor.tabSize": 2},
                "mcpServers": {"mine": {"command": "serve", "env": {"TOKEN": "not-a-real-one"}}},
                "files.associations": {"*.inc": "php"}
            }"#,
            r#"{"editor.fontSize": 16}"#,
            r#"{"editor.wordWrap": "on", "[python]": {"editor.insertSpaces": false}}"#,
        );
        let env = Env::from_pairs([("CODE_ARENA_MODE", "plan")]);
        let s = Settings::load(&files.user, Some(&files.project), env);
        let m = message(&s, &files);
        let (values, sources) = (&m["values"], &m["sources"]);
        assert_eq!(m["type"], "settings");
        // Every setting has its value in force: the default, the person's, the project's two files in their order.
        assert_eq!(values["editor.tabSize"], 4);
        assert_eq!(values["editor.fontSize"], 16);
        assert_eq!(values["editor.wordWrap"], "on");
        assert_eq!(values["editor.minimap.enabled"], true);
        assert_eq!((values["agent.maxSteps"].as_i64(), values["agent.mode"].as_str()), (Some(40), Some("plan")));
        for s in ide_schema::ALL {
            assert!(values.get(s.key).is_some(), "{} has no value", s.key);
        }
        // The agent's own objects stay out (a server's settings may hold a token); the IDE's own are in.
        assert!(values.get("mcpServers").is_none() && values.get("hooks").is_none(), "{values}");
        assert!(!m.to_string().contains("not-a-real-one"));
        assert_eq!(values["files.associations"], json!({"*.inc": "php"}));
        // A language's own, from every file that has a block for it.
        assert_eq!(m["by_lang"], json!({"python": {"editor.tabSize": 2, "editor.insertSpaces": false}}));
        // Where each one that is not its default comes from.
        assert_eq!(sources["editor.fontSize"], files.project.display().to_string());
        assert_eq!(sources["editor.wordWrap"], files.workspace.display().to_string());
        assert_eq!(sources["agent.maxSteps"], files.user.display().to_string());
        assert_eq!(sources["mcpServers"], files.user.display().to_string());
        assert_eq!(sources["agent.mode"], "env:CODE_ARENA_MODE");
        assert!(sources.get("editor.tabSize").is_none() && sources.get("editor.minimap.enabled").is_none());
        // What was not taken, with its setting and its file.
        assert_eq!(
            m["problems"],
            json!([{
                "key": "editor.tabSize",
                "file": files.user.display().to_string(),
                "error": "\"editor.tabSize\" takes a whole number. Its default holds.",
            }])
        );
        assert_eq!(m["files"]["user"], files.user.display().to_string());
        assert_eq!(m["files"]["project"], files.project.display().to_string());
        assert_eq!(m["files"]["workspace"], files.workspace.display().to_string());
        assert_eq!((files.layer(Layer::User), files.layer(Layer::Workspace)), (&*files.user, &*files.workspace));
        // Read again after a file changed: what the environment gave stays.
        std::fs::write(&files.workspace, r#"{"editor.wordWrap": "bounded"}"#).expect("write");
        let again = message(&files.read(&s), &files);
        assert_eq!(again["values"]["editor.wordWrap"], "bounded");
        assert_eq!(again["values"]["agent.mode"], "plan");
        assert_eq!(again["by_lang"], json!({"python": {"editor.tabSize": 2}}));
    }

    #[test]
    fn only_the_persons_own_files_are_opened_and_saved_here() {
        let (_d, paths, _files) = folders("{\n  // mine\n  \"editor.tabSize\": 2\n}\n", "", "");
        let config = &paths.config;
        std::fs::write(paths.credentials_file(), "{\"api_key\": \"not-a-real-one\"}").expect("write");
        // The names that are files of the person's.
        assert_eq!(user_path(config, "settings.json"), Ok(config.join("settings.json")));
        assert_eq!(user_path(config, "keybindings.json"), Ok(config.join("keybindings.json")));
        assert_eq!(user_path(config, "snippets/python.json"), Ok(config.join("snippets/python.json")));
        assert_eq!(
            user_path(config, "snippets/my own.code-snippets"),
            Ok(config.join("snippets/my own.code-snippets"))
        );
        assert_eq!(user_path(config, "skills/tests/SKILL.md"), Ok(config.join("skills/tests/SKILL.md")));
        for no in [
            "",
            "credentials.json",
            "../settings.json",
            "/etc/passwd",
            "snippets/",
            "snippets/../credentials.json",
            "snippets/a/b.json",
            "snippets/.json",
            "snippets/.hidden.json",
            "snippets/notes.txt",
            "snippets\\x.json",
            "ARENA.md",
            "skills/../SKILL.md",
            "skills/Up/SKILL.md",
            "skills/a/notes.md",
            "skills/a/b/SKILL.md",
        ] {
            assert!(user_path(config, no).is_err(), "{no:?} is no file of the person's");
            assert!(read_user(config, no)["error"].is_string() && read_user(config, no).get("text").is_none());
            assert!(write_user(config, no, "x")["error"].is_string());
        }
        assert_eq!(std::fs::read_to_string(paths.credentials_file()).expect("r"), "{\"api_key\": \"not-a-real-one\"}");
        // One read as it is, with its comments; one not there yet is empty and has no time.
        let read = read_user(config, "settings.json");
        assert_eq!(read["text"], "{\n  // mine\n  \"editor.tabSize\": 2\n}\n");
        assert!(read["modified"].as_u64().is_some_and(|at| at > 0));
        assert_eq!(read["path"], config.join("settings.json").display().to_string());
        assert_eq!(
            read_user(config, "keybindings.json"),
            json!({
                "type": "user_file",
                "name": "keybindings.json",
                "text": "",
                "modified": null,
                "path": config.join("keybindings.json").display().to_string(),
            })
        );
        // One written, its folder made; read back as written.
        let saved = write_user(config, "snippets/rust.json", "{\"main\": {}}\n");
        assert_eq!((saved["type"].as_str(), saved["name"].as_str()), (Some("saved_user"), Some("snippets/rust.json")));
        assert!(saved["modified"].as_u64().is_some() && saved.get("error").is_none(), "{saved}");
        assert_eq!(read_user(config, "snippets/rust.json")["text"], "{\"main\": {}}\n");
        assert_eq!(read_user(config, "snippets/rust.json")["modified"], saved["modified"]);
        // Language ids as a block names them.
        assert!(language_id("python") && language_id("c++") && language_id("objective-c") && language_id("c#"));
        assert!(!language_id("") && !language_id("a]b") && !language_id("a b") && !language_id("\"x\""));
    }

    #[test]
    fn the_projects_file_is_made_once_and_never_over_one_that_is_there() {
        use crate::ide_ext::rule;
        let d = tempfile::tempdir().expect("temp");
        let ws = Arc::new(ca_workspace::Workspace::open(d.path(), &[]).expect("ws"));
        let (out, mut rx) = tokio::sync::broadcast::channel(16);
        let side = Side::new(ws, Arc::default(), out, None, &std::env::temp_dir());
        let mut said = || -> Vec<Value> {
            std::iter::from_fn(|| rx.try_recv().ok()).map(|t| serde_json::from_str(&t).expect("json")).collect()
        };
        let file = d.path().join(WORKSPACE_FILE);
        assert!(side.handle(&json!({"type": "workspace_settings"})));
        assert_eq!(said(), [json!({"type": "workspace_settings", "path": ".vscode/settings.json", "made": true})]);
        assert_eq!(std::fs::read_to_string(&file).expect("made"), "{\n}\n");
        std::fs::write(&file, "{\"editor.tabSize\": 2}").expect("write");
        assert!(side.handle(&json!({"type": "workspace_settings"})));
        assert_eq!(said(), [json!({"type": "workspace_settings", "path": ".vscode/settings.json", "made": false})]);
        assert_eq!(std::fs::read_to_string(&file).expect("left"), "{\"editor.tabSize\": 2}");
        // The messages are this area's, each by its rule.
        assert_eq!(
            (rule("settings"), rule("user_file"), rule("save_user")),
            (Some(Rule::Turn), Some(Rule::Turn), Some(Rule::Turn))
        );
        assert_eq!((rule("workspace_settings"), rule("_settings_read")), (Some(Rule::Side), Some(Rule::Latest)));
    }
}
