//! The editor's extensions: VS Code's extensions taken for what their `package.json` declares, never for their code
//! (Code Arena has no extension host, and no extension's code runs). One is installed from a .vsix the person
//! names, or imported from VS Code's own extensions on this computer (Code's, VSCodium's and Insiders'); the
//! project's `.vscode/extensions.json` recommends some, each said with where it stands here.
//!
//! What is used of an extension: its colour themes and file icon themes, snippets, languages (and their
//! language configurations), grammars (kept, not used yet), keybindings for commands that are not its own, and
//! configuration defaults. Everything else is listed as not used here, with why. An extension with nothing used is
//! refused.
//!
//! Extensions are kept in the data folder, in `extensions/<publisher.name>-<version>/` with only the files their
//! contributions name, and listed in `extensions/installed.json` with whether each is on. A new version replaces
//! the one before (on or off as it was). A .vsix is taken up to 50 MB, a file in it up to 5 MB, and 50 MB and
//! 10,000 files are kept per extension at most; a .vsix with a name that leads out of it (`..`, `/`, `C:`) or a link
//! in it is refused whole, and a path in its `package.json` that leads out of the extension is left out.
//!
//! # Messages (all answered at once, a turn running or not; the work is done off the driver's loop)
//!
//! | Asked | Answered |
//! |---|---|
//! | `extensions` | `extensions{items}` |
//! | `ext_install{path}` | `ext_install{path, item}` or `{path, error, unused}`, then `extensions` and `ext_contributions` |
//! | `ext_import{id}` | `ext_import{id, item}` or `{id, error, unused}`, then the same two |
//! | `ext_uninstall{id}` | `ext_uninstall{id}` or `{id, error}`, then the same two |
//! | `ext_enable{id, on}` | `ext_enable{id, on, item}` or `{id, on, error}`, then the same two |
//! | `ext_local` | `ext_local{items}` |
//! | `ext_recommended` | `ext_recommended{file, recommendations, unwanted, error?}` |
//! | `ext_contributions` | `ext_contributions{themes, iconThemes, snippets, languages, grammars, keybindings, configurationDefaults}` |
//! | `ext_file{id, path}` | `ext_file{id, path, mime, data}` (base64) or `{id, path, error}` |

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use serde_json::{Map, Value, json};

use crate::ide_ext::{Area, Rule, Shared, send};

/// The largest .vsix taken.
const MOST_VSIX: u64 = 50 * 1024 * 1024;
/// The largest file kept from an extension.
const MOST_FILE: u64 = 5 * 1024 * 1024;
/// The most an extension's kept files hold together, and how many they are.
const MOST_TOTAL: u64 = 50 * 1024 * 1024;
const MOST_FILES: usize = 10_000;
/// The list's file in the extensions' folder.
const LIST: &str = "installed.json";

/// The contributions used here (grammars only kept, for now).
const USED: &[&str] = &["themes", "iconThemes", "snippets", "languages", "grammars", "keybindings", "configurationDefaults"];

/// The contributions merged for the window to apply, each entry with its extension and version.
const MERGED: &[&str] = &["themes", "iconThemes", "snippets", "languages", "grammars", "keybindings"];

/// This area in the driver.
pub struct Here;

/// Only one change of the extensions' folder at a time (an install, an uninstall, a switch).
#[derive(Default)]
struct Gate(Mutex<()>);

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("extensions", Rule::Turn),
        ("ext_install", Rule::Turn),
        ("ext_import", Rule::Turn),
        ("ext_uninstall", Rule::Turn),
        ("ext_enable", Rule::Turn),
        ("ext_local", Rule::Turn),
        ("ext_recommended", Rule::Turn),
        ("ext_contributions", Rule::Turn),
        ("ext_file", Rule::Turn),
    ];

    fn shared(sh: &Shared, m: &Value) -> bool {
        let kind = m["type"].as_str().unwrap_or("").to_string();
        if !Self::MESSAGES.iter().any(|(name, _)| *name == kind) {
            return false;
        }
        let at = Folders {
            root: sh.paths.data.join("extensions"),
            home: sh.home.clone(),
            project: sh.workspace.root().to_path_buf(),
        };
        let (gate, out, m) = (sh.kept.of::<Gate>(), sh.out.clone(), m.clone());
        tokio::task::spawn_blocking(move || {
            let held = match gate.0.lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            let (reply, changed) = answer(&at, &kind, &m);
            send(&out, reply);
            if changed {
                send(&out, json!({"type": "extensions", "items": list(&at.root)}));
                send(&out, contributions(&at.root, "ext_contributions"));
            }
            drop(held);
        });
        true
    }
}

/// Where the extensions are kept, VS Code's are looked for, and the project is.
struct Folders {
    root: PathBuf,
    home: Option<PathBuf>,
    project: PathBuf,
}

/// A message's reply, and whether the extensions kept changed (so the list and the contributions are said again).
fn answer(at: &Folders, kind: &str, m: &Value) -> (Value, bool) {
    let id = m["id"].as_str().unwrap_or("").trim().to_string();
    let refused = |mut v: Value, r: Refused| {
        v["error"] = json!(r.text);
        if !r.unused.is_empty() {
            v["unused"] = json!(r.unused);
        }
        (v, false)
    };
    match kind {
        "extensions" => (json!({"type": kind, "items": list(&at.root)}), false),
        "ext_install" => {
            let path = m["path"].as_str().unwrap_or("").to_string();
            let shown = json!({"type": kind, "path": path});
            match install(&at.root, &picked(&path, at.home.as_deref(), &at.project)) {
                Ok(item) => (json!({"type": kind, "path": path, "item": item}), true),
                Err(r) => refused(shown, r),
            }
        }
        "ext_import" => match import(&at.root, at.home.as_deref(), &id) {
            Ok(item) => (json!({"type": kind, "id": id, "item": item}), true),
            Err(r) => refused(json!({"type": kind, "id": id}), r),
        },
        "ext_uninstall" => match uninstall(&at.root, &id) {
            Ok(()) => (json!({"type": kind, "id": id}), true),
            Err(e) => refused(json!({"type": kind, "id": id}), e.into()),
        },
        "ext_enable" => {
            let on = m["on"].as_bool();
            match on.ok_or_else(|| "Say whether it is on: {\"id\", \"on\": true or false}.".to_string()).and_then(|on| enable(&at.root, &id, on)) {
                Ok(item) => (json!({"type": kind, "id": id, "on": on, "item": item}), true),
                Err(e) => refused(json!({"type": kind, "id": id, "on": on}), e.into()),
            }
        }
        "ext_local" => (json!({"type": kind, "items": local(&at.root, at.home.as_deref())}), false),
        "ext_recommended" => {
            let mut v = recommended(&at.root, at.home.as_deref(), &at.project);
            v["type"] = json!(kind);
            (v, false)
        }
        "ext_contributions" => (contributions(&at.root, kind), false),
        "ext_file" => {
            let path = m["path"].as_str().unwrap_or("").to_string();
            match file(&at.root, &id, &path) {
                Ok((mime, bytes)) => {
                    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                    (json!({"type": kind, "id": id, "path": path, "mime": mime, "data": data}), false)
                }
                Err(e) => refused(json!({"type": kind, "id": id, "path": path}), e.into()),
            }
        }
        _ => (json!({"type": kind, "error": "Not an extensions message."}), false),
    }
}

/// A .vsix's path as the person gave it: `~/` is the home folder, and a relative one is the project's.
fn picked(path: &str, home: Option<&Path>, project: &Path) -> PathBuf {
    let path = path.trim();
    match (path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")), home) {
        (Some(rest), Some(h)) => h.join(rest),
        _ if Path::new(path).is_absolute() => PathBuf::from(path),
        _ => project.join(path),
    }
}

/// Why an install or an import did not happen, and what of the extension is not used here.
#[derive(Debug)]
struct Refused {
    text: String,
    unused: Vec<String>,
}

impl From<String> for Refused {
    fn from(text: String) -> Self {
        Refused { text, unused: Vec::new() }
    }
}

// ------------------------------------------------------------------ the manifest

/// An extension's `package.json` as it is taken here: who it is, what it contributes that is used (in the window's
/// words), what is not, and the files that takes.
#[derive(Debug, Clone)]
struct Manifest {
    id: String,
    name: String,
    publisher: String,
    version: String,
    display_name: String,
    description: Option<String>,
    icon: Option<String>,
    contributes: Map<String, Value>,
    unused: Vec<String>,
    files: BTreeSet<String>,
    usable: bool,
    has: String,
}

/// Where an extension's files are read from: a .vsix, or a folder of VS Code's.
trait Source {
    /// A file by its path in the extension (`themes/dark.json`); `None` when it is not there. An error for one
    /// that is a link or too large.
    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, String>;
}

/// A .vsix: its files under `extension/`, refused whole when a name leads out of it or one is a link.
struct Vsix {
    zip: zip::ZipArchive<std::io::Cursor<Vec<u8>>>,
    /// Each file's place in the archive, by its path under `extension/` in lower case (VS Code's are not cased).
    entries: HashMap<String, usize>,
}

impl Vsix {
    fn new(bytes: Vec<u8>) -> Result<Vsix, String> {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|_| "That is not a .vsix: a VS Code extension is a zip file.".to_string())?;
        let mut entries = HashMap::new();
        for i in 0..zip.len() {
            let f = zip.by_index_raw(i).map_err(|e| format!("The .vsix cannot be read: {e}."))?;
            let raw = f.name().to_string();
            let name = raw.replace('\\', "/");
            let parts: Vec<&str> = name.split('/').collect();
            if name.starts_with('/') || name.contains('\0') || parts[0].contains(':') || parts.contains(&"..") {
                return Err(format!("The .vsix holds a file whose name leads out of it ({}): it is refused.", one_line(&raw)));
            }
            if f.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) {
                return Err(format!("The .vsix holds a link ({}): links are not taken.", one_line(&raw)));
            }
            if name.len() > 10 && name[..10].eq_ignore_ascii_case("extension/") && !name.ends_with('/') {
                entries.entry(name[10..].to_lowercase()).or_insert(i);
            }
        }
        Ok(Vsix { zip, entries })
    }
}

impl Source for Vsix {
    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(&i) = self.entries.get(&path.to_lowercase()) else {
            return Ok(None);
        };
        let f = self.zip.by_index(i).map_err(|e| format!("{path} cannot be read: {e}."))?;
        if f.size() > MOST_FILE {
            return Err(too_large(path));
        }
        capped(f, path).map(Some)
    }
}

/// A folder of VS Code's: files inside it only, none through a link.
struct Folder(PathBuf);

impl Source for Folder {
    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(inside) = inside("", path) else {
            return Ok(None);
        };
        let mut at = self.0.clone();
        let parts: Vec<&str> = inside.split('/').collect();
        for (n, part) in parts.iter().enumerate() {
            at.push(part);
            let Ok(meta) = std::fs::symlink_metadata(&at) else {
                return Ok(None);
            };
            if meta.file_type().is_symlink() {
                return Err(format!("{path} is a link (or in one): links are not taken."));
            }
            if n + 1 == parts.len() {
                if !meta.is_file() {
                    return Err(format!("{path} is not a regular file."));
                }
                if meta.len() > MOST_FILE {
                    return Err(too_large(path));
                }
            }
        }
        let f = std::fs::File::open(&at).map_err(|e| format!("{path}: {e}"))?;
        capped(f, path).map(Some)
    }
}

/// A reader's bytes, refused past [`MOST_FILE`] (what a header said is not trusted).
fn capped(r: impl Read, path: &str) -> Result<Vec<u8>, String> {
    let mut b = Vec::new();
    r.take(MOST_FILE + 1).read_to_end(&mut b).map_err(|e| format!("{path} cannot be read: {e}."))?;
    if b.len() as u64 > MOST_FILE {
        return Err(too_large(path));
    }
    Ok(b)
}

fn too_large(path: &str) -> String {
    format!("{path} is larger than {} MB.", MOST_FILE / 1024 / 1024)
}

fn one_line(s: &str) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if flat.chars().count() > 120 { format!("{}…", flat.chars().take(119).collect::<String>()) } else { flat }
}

fn parse(bytes: &[u8]) -> Option<Value> {
    ca_settings::jsonc::parse(&String::from_utf8_lossy(bytes)).ok()
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn objects(v: Option<&Value>) -> impl Iterator<Item = &Value> {
    v.and_then(Value::as_array).into_iter().flatten().filter(|x| x.is_object())
}

fn strings(v: Option<&Value>) -> Vec<Value> {
    v.and_then(Value::as_array).into_iter().flatten().filter(|x| x.is_string()).take(500).cloned().collect()
}

/// A path a manifest gives (`./themes/a.json`), from a folder of the extension's, as its path in it; `None` when it
/// leads out.
fn inside(from: &str, path: &str) -> Option<String> {
    if path.is_empty() || path.len() > 1024 || path.contains(['\0', ':', '?', '#']) {
        return None;
    }
    let p = path.replace('\\', "/");
    if p.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn dir_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

/// The kind of a file kept, by its name: what the window reads (JSON) or shows (images, fonts), and grammars.
fn type_of(path: &str) -> Option<&'static str> {
    let name = path.to_lowercase();
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    Some(match ext {
        "json" | "jsonc" | "code-snippets" => "application/json",
        "tmlanguage" | "plist" => "text/plain",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => return None,
    })
}

/// What a contribution not used here needs, by its name.
fn why(key: &str) -> &'static str {
    match key {
        "configuration" => "its own settings, which its code reads",
        "jsonValidation" => "JSON schemas for files are not read yet",
        "productIconThemes" => "product icon themes are not supported",
        "colors" => "colours of its own views",
        "semanticTokenTypes" | "semanticTokenModifiers" | "semanticTokenScopes" => {
            "semantic tokens come from language servers, not from an extension's declarations"
        }
        "icons" => "product icons are not supported",
        _ => "needs VS Code's extension host (its code)",
    }
}

fn word_ok(s: &str, more: &[char], most: usize) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.len() <= most
        && cs.all(|c| c.is_ascii_alphanumeric() || more.contains(&c))
}

fn publisher_ok(s: &str) -> bool {
    word_ok(s, &['-'], 128)
}

fn package_ok(s: &str) -> bool {
    word_ok(s, &['.', '_', '-'], 128)
}

fn version_ok(s: &str) -> bool {
    word_ok(s, &['.', '+', '-'], 64)
}

/// An extension's id as VS Code writes it: `publisher.name`.
fn id_ok(s: &str) -> bool {
    s.split_once('.').is_some_and(|(p, n)| publisher_ok(p) && package_ok(n))
}

/// Every `"%key%"` text replaced by `package.nls.json`'s message for it.
fn localized(v: &Value, nls: &Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), localized(x, nls))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(|x| localized(x, nls)).collect()),
        Value::String(s) if s.len() > 2 && s.starts_with('%') && s.ends_with('%') && !s[1..s.len() - 1].contains('%') => {
            match &nls[&s[1..s.len() - 1]] {
                Value::String(t) => json!(t),
                said @ Value::Object(_) => said.get("message").cloned().filter(Value::is_string).unwrap_or_else(|| v.clone()),
                _ => v.clone(),
            }
        }
        _ => v.clone(),
    }
}

/// The files a manifest's contributions name, and what of it is not used.
#[derive(Default)]
struct Taking {
    files: BTreeSet<String>,
    unused: Vec<String>,
}

impl Taking {
    /// A path a contribution names (relative to the extension, or to the file that names it), kept when it stays
    /// inside the extension and (when `known`) is a kind of file read here.
    fn keep(&mut self, at: &str, path: Option<&str>, what: &str, known: bool) -> Option<String> {
        let path = path.filter(|p| !p.trim().is_empty())?;
        let Some(inside) = inside(at, path) else {
            self.unused.push(format!("{what}: {} leads outside the extension", one_line(path)));
            return None;
        };
        if known && type_of(&inside).is_none() {
            self.unused.push(format!("{what}: {} is not a kind of file Code Arena reads", one_line(path)));
            return None;
        }
        self.files.insert(inside.clone());
        Some(inside)
    }
}

/// `package.json` read, its `%placeholders%` filled from `package.nls.json`; with `deep`, the theme and icon theme
/// files too, for the files they name in turn (includes, icons, fonts).
fn read_manifest(src: &mut dyn Source, deep: bool) -> Result<Manifest, String> {
    let bytes = src
        .read("package.json")?
        .ok_or("There is no extension/package.json: this is not a VS Code extension.")?;
    let raw = ca_settings::jsonc::parse(&String::from_utf8_lossy(&bytes))
        .map_err(|e| format!("Its package.json is not JSON: {e}."))?;
    if !raw.is_object() {
        return Err("Its package.json is not JSON: it holds no object.".into());
    }
    let nls = src.read("package.nls.json")?.and_then(|b| parse(&b)).unwrap_or(Value::Null);
    let json = localized(&raw, &nls);
    let (publisher, name, version) = (
        text(&json, "publisher").unwrap_or("").to_string(),
        text(&json, "name").unwrap_or("").to_string(),
        text(&json, "version").unwrap_or("").to_string(),
    );
    if !publisher_ok(&publisher) || !package_ok(&name) {
        return Err("Its package.json does not name its publisher and name as VS Code's do (letters, digits and -).".into());
    }
    if !version_ok(&version) {
        return Err("Its package.json does not give a version as VS Code's do (1.2.3).".into());
    }
    let id = format!("{publisher}.{name}").to_lowercase();
    let empty = Map::new();
    let given = json.get("contributes").and_then(Value::as_object).unwrap_or(&empty);
    let mut t = Taking::default();
    let mut contributes = Map::new();

    let mut themes = Vec::new();
    for th in objects(given.get("themes")) {
        let path = text(th, "path").unwrap_or("");
        let stem = path.rsplit('/').next().unwrap_or("theme").split('.').next().unwrap_or("theme");
        let label = text(th, "label").or(text(th, "id")).unwrap_or(stem).to_string();
        if path.to_lowercase().ends_with(".tmtheme") {
            t.unused.push(format!("theme {label}: .tmTheme (XML) colour themes are not read, only JSON ones"));
            continue;
        }
        let Some(kept) = t.keep("", Some(path), &format!("theme {label}"), true) else {
            continue;
        };
        let ui = text(th, "uiTheme").filter(|u| ["vs", "vs-dark", "hc-black", "hc-light"].contains(u)).unwrap_or("vs-dark");
        themes.push(json!({"label": label, "uiTheme": ui, "path": kept, "id": th.get("id").cloned().unwrap_or(Value::Null)}));
        if deep {
            theme_files(src, &kept, &label, &mut t, &mut HashSet::from([kept.clone()]))?;
        }
    }
    contributes.insert("themes".into(), Value::Array(themes));

    let mut icon_themes = Vec::new();
    for th in objects(given.get("iconThemes")) {
        let theme_id = text(th, "id").unwrap_or("");
        let label = text(th, "label").unwrap_or(theme_id).to_string();
        if theme_id.is_empty() {
            continue;
        }
        let Some(kept) = t.keep("", text(th, "path"), &format!("icon theme {label}"), true) else {
            continue;
        };
        icon_themes.push(json!({"id": theme_id, "label": label, "path": kept}));
        if deep && let Some(theme) = src.read(&kept)?.and_then(|b| parse(&b)) {
            let (dir, what) = (dir_of(&kept).to_string(), format!("icon theme {label}"));
            for def in theme.get("iconDefinitions").and_then(Value::as_object).into_iter().flat_map(|o| o.values()) {
                t.keep(&dir, text(def, "iconPath"), &what, true);
            }
            for font in objects(theme.get("fonts")) {
                for s in objects(font.get("src")) {
                    t.keep(&dir, text(s, "path"), &what, true);
                }
            }
        }
    }
    contributes.insert("iconThemes".into(), Value::Array(icon_themes));

    let mut snippets = Vec::new();
    for s in objects(given.get("snippets")) {
        let language = text(s, "language");
        let what = format!("snippets for {}", language.unwrap_or("every language"));
        if let Some(kept) = t.keep("", text(s, "path"), &what, true) {
            snippets.push(json!({"language": language, "path": kept}));
        }
    }
    contributes.insert("snippets".into(), Value::Array(snippets));

    let mut languages = Vec::new();
    for l in objects(given.get("languages")) {
        let Some(lang) = text(l, "id").filter(|s| !s.is_empty()) else {
            continue;
        };
        let mut language = json!({"id": lang});
        for key in ["extensions", "filenames", "filenamePatterns", "aliases", "mimetypes"] {
            language[key] = Value::Array(strings(l.get(key)));
        }
        language["firstLine"] = json!(text(l, "firstLine"));
        language["configuration"] = json!(t.keep("", text(l, "configuration"), &format!("language {lang}'s configuration"), true));
        languages.push(language);
    }
    contributes.insert("languages".into(), Value::Array(languages));

    let mut grammars = Vec::new();
    for g in objects(given.get("grammars")) {
        let what = format!("grammar {}", text(g, "scopeName").unwrap_or(""));
        if let Some(kept) = t.keep("", text(g, "path"), &what, true) {
            grammars.push(json!({"language": text(g, "language"), "scopeName": text(g, "scopeName"), "path": kept}));
        }
    }
    if !grammars.is_empty() {
        t.unused.push("grammars: kept, not used yet (the editor's own highlighting is used)".into());
    }
    contributes.insert("grammars".into(), Value::Array(grammars));

    // A keybinding for one of its own commands needs its code; one for VS Code's (a keymap's) may be Code Arena's.
    let own: HashSet<&str> = objects(given.get("commands")).filter_map(|c| text(c, "command")).collect();
    let rules: Vec<&Value> = match given.get("keybindings") {
        Some(one @ Value::Object(_)) => vec![one],
        other => objects(other).collect(),
    };
    let (mut keys, mut own_keys) = (Vec::new(), 0);
    for k in rules {
        let Some(command) = text(k, "command").filter(|c| !c.is_empty() && !c.starts_with('-')) else {
            continue;
        };
        if own.contains(command) {
            own_keys += 1;
            continue;
        }
        let mut rule = json!({"command": command});
        for key in ["key", "mac", "linux", "win", "when"] {
            if let Some(v) = text(k, key).filter(|v| !v.is_empty()) {
                rule[key] = json!(v);
            }
        }
        if let Some(args) = k.get("args") {
            rule["args"] = args.clone();
        }
        if rule.get("key").is_some() {
            keys.push(rule);
        }
    }
    if own_keys > 0 {
        t.unused.push(format!("keybindings: {own_keys} for its own commands, which need its code"));
    }
    contributes.insert("keybindings".into(), Value::Array(keys));
    contributes.insert(
        "configurationDefaults".into(),
        given.get("configurationDefaults").cloned().filter(Value::is_object).unwrap_or_else(|| json!({})),
    );

    for key in given.keys() {
        if !USED.contains(&key.as_str()) {
            t.unused.push(format!("{key}: {}", why(key)));
        }
    }
    let code: Vec<String> = ["main", "browser"].iter().filter_map(|k| text(&json, k).map(|m| format!("{k}: {m}"))).collect();
    if !code.is_empty() {
        t.unused.insert(0, format!("its code ({}): Code Arena runs no extension's code", code.join(", ")));
    }
    let icon = text(&json, "icon").and_then(|i| inside("", i)).filter(|i| {
        let l = i.to_lowercase();
        [".png", ".svg", ".jpg", ".jpeg"].iter().any(|e| l.ends_with(e))
    });
    if let Some(i) = &icon {
        t.files.insert(i.clone());
    }
    let usable = brings(&contributes);
    let has = if !code.is_empty() {
        "its code".to_string()
    } else if !given.is_empty() {
        given.keys().cloned().collect::<Vec<_>>().join(", ")
    } else {
        "nothing it contributes".to_string()
    };
    Ok(Manifest {
        id,
        display_name: text(&json, "displayName").unwrap_or(&name).to_string(),
        name,
        publisher,
        version,
        description: text(&json, "description").map(String::from),
        icon,
        contributes,
        unused: t.unused,
        files: t.files,
        usable,
        has,
    })
}

/// A colour theme's file and what it takes in: its `include` (and that one's), and `tokenColors` given as a file.
fn theme_files(src: &mut dyn Source, path: &str, label: &str, t: &mut Taking, seen: &mut HashSet<String>) -> Result<(), String> {
    if seen.len() > 10 {
        return Ok(());
    }
    let Some(theme) = src.read(path)?.and_then(|b| parse(&b)) else {
        return Ok(());
    };
    let (dir, what) = (dir_of(path).to_string(), format!("theme {label}"));
    if let Some(included) = t.keep(&dir, text(&theme, "include"), &what, true)
        && seen.insert(included.clone())
    {
        theme_files(src, &included, label, t, seen)?;
    }
    if let Some(tokens) = text(&theme, "tokenColors") {
        if tokens.to_lowercase().ends_with(".tmtheme") {
            t.unused.push(format!("theme {label}: its tokenColors are a .tmTheme (XML), which is not read"));
        } else {
            t.keep(&dir, Some(tokens), &what, true);
        }
    }
    Ok(())
}

/// Whether contributions as the window reads them hold anything used here.
fn brings(c: &Map<String, Value>) -> bool {
    ["themes", "iconThemes", "snippets", "languages", "grammars", "keybindings"]
        .iter()
        .any(|k| c.get(*k).and_then(Value::as_array).is_some_and(|a| !a.is_empty()))
        || c.get("configurationDefaults").and_then(Value::as_object).is_some_and(|o| !o.is_empty())
}

/// Contributions whose file is not in the extension left out (a language keeps its names, without its
/// configuration).
fn prune(c: &mut Map<String, Value>, missing: &HashSet<&String>) {
    let gone = |v: &Value| text(v, "path").is_some_and(|p| missing.contains(&p.to_string()));
    for kind in ["themes", "iconThemes", "snippets", "grammars"] {
        if let Some(Value::Array(list)) = c.get_mut(kind) {
            list.retain(|x| !gone(x));
        }
    }
    if let Some(Value::Array(list)) = c.get_mut("languages") {
        for l in list {
            if text(l, "configuration").is_some_and(|p| missing.contains(&p.to_string())) {
                l["configuration"] = Value::Null;
            }
        }
    }
}

/// Why an extension is refused: it has nothing used here.
fn nothing_usable(m: &Manifest) -> Refused {
    Refused {
        text: format!(
            "{} has nothing Code Arena can use ({}). Code Arena takes only what extensions declare: colour and file \
             icon themes, snippets, languages, grammars, keybindings and configuration defaults; it runs no \
             extension's code.",
            m.display_name, m.has
        ),
        unused: m.unused.clone(),
    }
}

// --------------------------------------------------------------------- keeping

/// A .vsix the person named: read, its used files kept, listed (in place of another version of it).
fn install(root: &Path, path: &Path) -> Result<Value, Refused> {
    let meta = std::fs::metadata(path).map_err(|_| format!("There is no file {}.", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file.", path.display()).into());
    }
    if meta.len() > MOST_VSIX {
        return Err(format!("A .vsix is taken up to {} MB.", MOST_VSIX / 1024 / 1024).into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MOST_VSIX + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MOST_VSIX {
        return Err(format!("A .vsix is taken up to {} MB.", MOST_VSIX / 1024 / 1024).into());
    }
    let mut src = Vsix::new(bytes)?;
    let m = read_manifest(&mut src, true)?;
    store(root, &mut src, m, "vsix")
}

/// One of VS Code's own extensions on this computer (its newest copy), copied as a .vsix's would be.
fn import(root: &Path, home: Option<&Path>, id: &str) -> Result<Value, Refused> {
    let wanted = id.trim().to_lowercase();
    let found = local_copies(home)
        .into_iter()
        .filter(|c| c.id == wanted)
        .max_by(|a, b| version_cmp(&a.version, &b.version))
        .ok_or_else(|| format!("VS Code has no extension {id} on this computer."))?;
    let mut src = Folder(found.dir);
    let m = read_manifest(&mut src, true)?;
    store(root, &mut src, m, "vscode")
}

/// A new name in the extensions' folder for what is being written, never one another write has.
fn temp_name(what: &str) -> String {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    format!(".{what}-{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed))
}

/// The extension's files copied into its folder, in place of any other version of it, and listed.
fn store(root: &Path, src: &mut dyn Source, mut m: Manifest, from: &str) -> Result<Value, Refused> {
    if !m.usable {
        return Err(nothing_usable(&m));
    }
    let wanted: Vec<String> = m.files.iter().cloned().collect();
    if wanted.len() > MOST_FILES {
        return Err(format!("{} names {} files: {MOST_FILES} are kept at most.", m.display_name, wanted.len()).into());
    }
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let temp = root.join(temp_name("new"));
    let done = copy_files(src, &mut m, &wanted, &temp).and_then(|kept| keep(root, &temp, &m, &wanted, &kept, from));
    let _ = std::fs::remove_dir_all(&temp);
    done
}

/// The files named copied into `temp`: those kept (a file named but not in the extension is said, and left out).
fn copy_files(src: &mut dyn Source, m: &mut Manifest, wanted: &[String], temp: &Path) -> Result<Vec<String>, Refused> {
    std::fs::create_dir_all(temp).map_err(|e| format!("{}: {e}", temp.display()))?;
    let (mut kept, mut total) = (Vec::new(), 0u64);
    for file in wanted {
        let Some(bytes) = src.read(file)? else {
            m.unused.push(format!("{file}: named, but not in the extension"));
            continue;
        };
        total += bytes.len() as u64;
        if total > MOST_TOTAL {
            return Err(format!("{}'s files hold more than {} MB.", m.display_name, MOST_TOTAL / 1024 / 1024).into());
        }
        let to = temp.join(file);
        // A name that is both a file and a folder ("a" and "a/b"), or one this computer's files cannot have.
        to.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&to, &bytes))
            .map_err(|e| format!("{file} cannot be kept: {e}"))?;
        kept.push(file.clone());
    }
    Ok(kept)
}

/// The copied folder put in its place and the list written: the extension's entry, as the window reads it.
fn keep(root: &Path, temp: &Path, m: &Manifest, wanted: &[String], kept: &[String], from: &str) -> Result<Value, Refused> {
    let mut contributes = m.contributes.clone();
    if kept.len() < wanted.len() {
        let missing: HashSet<&String> = wanted.iter().filter(|w| !kept.contains(w)).collect();
        prune(&mut contributes, &missing);
        if !brings(&contributes) {
            return Err(nothing_usable(m));
        }
    }
    let dir = format!("{}-{}", m.id, m.version);
    let info = json!({
        "name": m.name,
        "publisher": m.publisher,
        "displayName": m.display_name,
        "description": m.description,
        "icon": m.icon.as_ref().filter(|i| kept.contains(i)),
        "contributes": contributes,
        "unused": m.unused,
    });
    let mut all = load(root);
    let before = all.iter().position(|e| text(e, "id") == Some(m.id.as_str()));
    let installed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let entry = json!({
        "id": m.id,
        "version": m.version,
        "dir": dir,
        "enabled": before.and_then(|i| all[i]["enabled"].as_bool()).unwrap_or(true),
        "source": from,
        "installed": installed,
        "files": kept,
        "info": info,
    });
    match before {
        Some(i) => {
            remove(root, text(&all[i], "dir").unwrap_or(""));
            all[i] = entry.clone();
        }
        None => all.push(entry.clone()),
    }
    remove(root, &dir);
    std::fs::rename(temp, root.join(&dir)).map_err(|e| format!("{dir} cannot be kept: {e}"))?;
    save(root, &all)?;
    Ok(item(&entry))
}

/// An extension's folder taken away (one the list names only: never anything else of the data folder).
fn remove(root: &Path, dir: &str) {
    if dir.is_empty() || dir.contains(['/', '\\']) || dir.starts_with('.') {
        return;
    }
    // Another program may hold a file in it: the list no longer names it, and a later install replaces it.
    let _ = std::fs::remove_dir_all(root.join(dir));
}

/// Takes an extension away, its files with it.
fn uninstall(root: &Path, id: &str) -> Result<(), String> {
    let mut all = load(root);
    let i = find(&all, id)?;
    let e = all.remove(i);
    save(root, &all)?;
    remove(root, text(&e, "dir").unwrap_or(""));
    Ok(())
}

/// Turns an extension on or off: its contributions apply, or not.
fn enable(root: &Path, id: &str, on: bool) -> Result<Value, String> {
    let mut all = load(root);
    let i = find(&all, id)?;
    all[i]["enabled"] = json!(on);
    save(root, &all)?;
    Ok(item(&all[i]))
}

/// One of an extension's files its contributions name, and its type; any other is refused.
fn file(root: &Path, id: &str, path: &str) -> Result<(&'static str, Vec<u8>), String> {
    let all = load(root);
    let e = &all[find(&all, id)?];
    let named = e["files"].as_array().into_iter().flatten().any(|f| f.as_str() == Some(path));
    let no = || format!("{id} has no such file it uses.");
    if !named || inside("", path).as_deref() != Some(path) {
        return Err(no());
    }
    let bytes = Folder(root.join(text(e, "dir").unwrap_or(""))).read(path)?.ok_or_else(|| format!("{path} of {id} is gone."))?;
    Ok((type_of(path).unwrap_or("application/octet-stream"), bytes))
}

fn find(all: &[Value], id: &str) -> Result<usize, String> {
    let id = id.trim();
    all.iter()
        .position(|e| text(e, "id").is_some_and(|x| x.eq_ignore_ascii_case(id)))
        .ok_or_else(|| format!("There is no extension {id} here."))
}

/// The list as kept: each entry well formed, one per extension.
fn load(root: &Path) -> Vec<Value> {
    let file = root.join(LIST);
    let list = std::fs::metadata(&file)
        .ok()
        .filter(|m| m.len() <= MOST_FILE)
        .and_then(|_| std::fs::read(&file).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let mut seen = HashSet::new();
    list.and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            text(e, "id").is_some()
                && text(e, "version").is_some()
                && text(e, "dir").is_some_and(|d| !d.is_empty() && !d.contains(['/', '\\']) && !d.starts_with('.'))
        })
        .filter(|e| seen.insert(text(e, "id").unwrap_or("").to_lowercase()))
        .collect()
}

/// The list written whole or not at all: another run of code-arena may read it meanwhile.
fn save(root: &Path, all: &[Value]) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let (file, temp) = (root.join(LIST), root.join(temp_name("list")));
    let text = serde_json::to_string_pretty(all).map_err(|e| e.to_string())?;
    std::fs::write(&temp, text)
        .and_then(|()| std::fs::rename(&temp, &file))
        .map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            format!("{}: {e}", file.display())
        })
}

/// Contributions as the window lists them: keybindings counted, not given.
fn counted(c: &Value) -> Value {
    let mut c = c.clone();
    if c.is_object() {
        c["keybindings"] = json!(c["keybindings"].as_array().map_or(0, Vec::len));
    }
    c
}

/// An entry of the list as the window reads it.
fn item(e: &Value) -> Value {
    let info = &e["info"];
    json!({
        "id": e["id"],
        "name": info["name"],
        "displayName": info["displayName"],
        "version": e["version"],
        "publisher": info["publisher"],
        "description": info["description"],
        "icon": info["icon"],
        "enabled": e["enabled"].as_bool() != Some(false),
        "source": e["source"],
        "contributes": counted(&info["contributes"]),
        "unused": info["unused"].as_array().cloned().unwrap_or_default(),
    })
}

/// The extensions kept, in the order they came: each with what it brings and what is not used here.
fn list(root: &Path) -> Vec<Value> {
    load(root).iter().map(item).collect()
}

/// What the extensions that are on bring, merged for the window to apply: each entry says its extension and
/// version (whose files it names). Keybindings whole here.
fn contributions(root: &Path, kind: &str) -> Value {
    let mut all = json!({"type": kind, "configurationDefaults": []});
    for k in MERGED {
        all[*k] = json!([]);
    }
    for e in load(root).iter().filter(|e| e["enabled"].as_bool() != Some(false)) {
        let (id, version) = (&e["id"], &e["version"]);
        let c = &e["info"]["contributes"];
        for k in MERGED {
            for x in objects(c.get(*k)) {
                let mut x = x.clone();
                x["extension"] = id.clone();
                x["version"] = version.clone();
                if let Some(Value::Array(a)) = all.get_mut(*k) {
                    a.push(x);
                }
            }
        }
        if let Some(d) = c["configurationDefaults"].as_object().filter(|d| !d.is_empty())
            && let Some(Value::Array(a)) = all.get_mut("configurationDefaults")
        {
            a.push(json!({"extension": id, "version": version, "values": d}));
        }
    }
    all
}

// ------------------------------------------------------------------ VS Code's own

/// VS Code's own folders of extensions on this computer: Code's, VSCodium's and Insiders'.
fn local_roots(home: Option<&Path>) -> Vec<(PathBuf, &'static str)> {
    let Some(h) = home else {
        return Vec::new();
    };
    vec![
        (h.join(".vscode").join("extensions"), "VS Code"),
        (h.join(".vscode-oss").join("extensions"), "VSCodium"),
        (h.join(".vscode-insiders").join("extensions"), "VS Code - Insiders"),
    ]
}

/// One extension folder of VS Code's, its `package.json` read (not its other files).
struct LocalCopy {
    id: String,
    version: String,
    dir: PathBuf,
    from: &'static str,
    manifest: Option<Manifest>,
    problem: Option<String>,
}

/// A folder of VS Code's extensions by its name: `publisher.name-version`, then the platform it is for, if any.
fn folder_name(name: &str) -> Option<(String, String)> {
    let b = name.as_bytes();
    let at = (1..b.len().saturating_sub(1)).find(|&i| b[i] == b'-' && b[i + 1].is_ascii_digit())?;
    let rest = &name[at + 1..];
    Some((name[..at].to_lowercase(), rest.split('-').next().unwrap_or(rest).to_string()))
}

/// Each extension folder of VS Code's on this computer.
fn local_copies(home: Option<&Path>) -> Vec<LocalCopy> {
    let mut copies = Vec::new();
    for (root, from) in local_roots(home) {
        let Ok(read) = std::fs::read_dir(&root) else {
            continue;
        };
        // VS Code lists the folders it removed but could not delete yet here.
        let obsolete = std::fs::read(root.join(".obsolete")).ok().and_then(|b| parse(&b)).unwrap_or(Value::Null);
        let mut names: Vec<String> = read.flatten().map(|d| d.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for name in names.into_iter().take(2000) {
            let dir = root.join(&name);
            if name.starts_with('.') || obsolete.get(&name).is_some() || !dir.join("package.json").is_file() {
                continue;
            }
            match read_manifest(&mut Folder(dir.clone()), false) {
                Ok(m) => copies.push(LocalCopy {
                    id: m.id.clone(),
                    version: m.version.clone(),
                    dir,
                    from,
                    manifest: Some(m),
                    problem: None,
                }),
                Err(e) => {
                    let (id, version) = folder_name(&name).unwrap_or_else(|| (name.to_lowercase(), String::new()));
                    copies.push(LocalCopy { id, version, dir, from, manifest: None, problem: Some(e) });
                }
            }
        }
    }
    copies
}

/// The newest copy of each, by id.
fn newest(copies: Vec<LocalCopy>) -> std::collections::BTreeMap<String, LocalCopy> {
    let mut by = std::collections::BTreeMap::<String, LocalCopy>::new();
    for c in copies {
        match by.get(&c.id) {
            Some(had) if version_cmp(&had.version, &c.version).is_ge() => {}
            _ => {
                by.insert(c.id.clone(), c);
            }
        }
    }
    by
}

/// A manifest as `ext_local` lists it: the list's shape, without what only kept ones have.
fn summary(m: &Manifest) -> Value {
    json!({
        "id": m.id,
        "name": m.name,
        "displayName": m.display_name,
        "version": m.version,
        "publisher": m.publisher,
        "description": m.description,
        "contributes": counted(&Value::Object(m.contributes.clone())),
        "unused": m.unused,
    })
}

fn kept_versions(root: &Path) -> HashMap<String, String> {
    load(root)
        .iter()
        .map(|e| (text(e, "id").unwrap_or("").to_lowercase(), text(e, "version").unwrap_or("").to_string()))
        .collect()
}

/// VS Code's own extensions on this computer, the newest copy of each: whether it brings something used here
/// (what, and what not), and the version kept here already, if any.
fn local(root: &Path, home: Option<&Path>) -> Vec<Value> {
    let kept = kept_versions(root);
    newest(local_copies(home))
        .into_values()
        .map(|c| {
            let mut v = match &c.manifest {
                Some(m) => summary(m),
                None => json!({"id": c.id, "name": c.id, "displayName": c.id, "version": c.version}),
            };
            v["from"] = json!(c.from);
            v["path"] = json!(c.dir.display().to_string());
            v["usable"] = json!(c.manifest.as_ref().is_some_and(|m| m.usable));
            v["problem"] = json!(c.problem.clone().or_else(|| c.manifest.as_ref().filter(|m| !m.usable).map(|m| nothing_usable(m).text)));
            v["installed"] = json!(kept.get(&c.id));
            v
        })
        .collect()
}

// --------------------------------------------------------------------- recommended

/// VS Code extensions whose work Code Arena does itself, and how.
fn built_in(id: &str) -> Option<&'static str> {
    Some(match id {
        "esbenp.prettier-vscode" | "prettier.prettier-vscode" => {
            "Formatting: Prettier from the project's node_modules (Format Document, format on save)."
        }
        "charliermarsh.ruff" => "Formatting: ruff, from the project's .venv or PATH.",
        "ms-python.black-formatter" => "Formatting: black, from the project's .venv or PATH.",
        "ms-python.python" => "Run and Debug: debugpy (python configurations); formatting: ruff or black.",
        "ms-python.debugpy" => "Run and Debug: debugpy, when it is installed.",
        "ms-vscode.cpptools" | "ms-vscode.cpptools-extension-pack" => {
            "Run and Debug: gdb or lldb-dap (cppdbg configurations); formatting: clang-format."
        }
        "vadimcn.vscode-lldb" => "Run and Debug: lldb-dap (lldb configurations).",
        "llvm-vs-code-extensions.lldb-dap" => "Run and Debug: lldb-dap (lldb-dap configurations).",
        "ms-dotnettools.csharp" | "ms-dotnettools.csdevkit" => {
            "Run and Debug: netcoredbg (coreclr configurations); formatting: dotnet format."
        }
        "golang.go" => "Run and Debug: Delve (go configurations); formatting: gofmt.",
        "rust-lang.rust-analyzer" => "Language features: rust-analyzer, when it is installed; formatting: rustfmt.",
        "xaver.clang-format" => "Formatting: clang-format.",
        "ms-vscode.js-debug" | "ms-vscode.js-debug-nightly" => {
            "Run and Debug: js-debug (node configurations), when it is installed."
        }
        "ms-toolsai.jupyter" => "Notebooks: .ipynb files open in Code Arena's own notebook editor.",
        _ => return None,
    })
}

/// VS Code extensions that are their code, and what to do instead here.
fn code_only(id: &str) -> Option<&'static str> {
    Some(match id {
        "dbaeumer.vscode-eslint" => {
            "ESLint's extension runs code: not supported here. Run ESLint as the project's check in Problems, or as a task."
        }
        "stylelint.vscode-stylelint" => {
            "Stylelint's extension runs code: not supported here. Run it as the project's check, or as a task."
        }
        "eamodio.gitlens" => {
            "GitLens runs code: not supported here. Source Control shows the changes, a file's history and compares with any revision."
        }
        "ms-python.vscode-pylance" => "Pylance is licensed for Microsoft's products only.",
        "github.copilot" => "Copilot runs code: Code Arena's own completions and agent do this work.",
        "github.copilot-chat" => "Copilot runs code: Code Arena's own chat and agent do this work.",
        "ms-azuretools.vscode-docker" => "Docker's extension runs code: not supported here. Use the terminal.",
        "ms-vscode-remote.remote-ssh" => "Remote development is not supported here: run code-arena on that machine.",
        "ms-vscode-remote.remote-containers" => {
            "Remote development is not supported here: run code-arena in the container."
        }
        "ms-vscode-remote.remote-wsl" => "Remote development is not supported here: run code-arena in WSL.",
        _ => return None,
    })
}

/// Extension ids a list gives, as VS Code writes them, each once.
fn ids(v: Option<&Value>) -> Vec<String> {
    let mut seen = HashSet::new();
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(str::trim))
        .filter(|s| id_ok(s) && seen.insert(s.to_lowercase()))
        .map(String::from)
        .collect()
}

/// The project's `.vscode/extensions.json` (JSON with comments): each recommended extension and where it stands
/// here: `installed`, `built-in` (Code Arena does its work), `unsupported` (it is its code), `local` (VS Code has it
/// on this computer: it can be imported) or `unknown` (install it from a .vsix).
fn recommended(root: &Path, home: Option<&Path>, project: &Path) -> Value {
    let file = project.join(".vscode").join("extensions.json");
    let Ok(bytes) = std::fs::metadata(&file).and_then(|_| std::fs::read(&file)) else {
        return json!({"file": false, "recommendations": [], "unwanted": []});
    };
    let json = match ca_settings::jsonc::parse(&String::from_utf8_lossy(&bytes)) {
        Ok(v) if v.is_object() => v,
        Ok(_) => return json!({"file": true, "error": ".vscode/extensions.json holds no object.", "recommendations": [], "unwanted": []}),
        Err(e) => return json!({"file": true, "error": format!(".vscode/extensions.json: {e}"), "recommendations": [], "unwanted": []}),
    };
    let kept = kept_versions(root);
    let mut local: Option<std::collections::BTreeMap<String, LocalCopy>> = None;
    let mut out = Vec::new();
    for raw in ids(json.get("recommendations")).into_iter().take(200) {
        let id = raw.to_lowercase();
        let mut item = json!({"id": raw});
        let (status, note) = if let Some(version) = kept.get(&id) {
            ("installed", format!("Installed here ({version})."))
        } else if let Some(how) = built_in(&id) {
            ("built-in", how.to_string())
        } else if let Some(instead) = code_only(&id) {
            ("unsupported", instead.to_string())
        } else {
            let copies = local.get_or_insert_with(|| newest(local_copies(home)));
            match copies.get(&id).and_then(|c| c.manifest.as_ref().map(|m| (c.from, m))) {
                Some((from, m)) => {
                    item["name"] = json!(m.display_name);
                    if m.usable {
                        ("local", format!("{from} has it on this computer ({}): import it.", m.version))
                    } else {
                        ("unsupported", nothing_usable(m).text)
                    }
                }
                None => ("unknown", "Not on this computer: install it from its .vsix (Open VSX or the Marketplace's page has one).".to_string()),
            }
        };
        item["status"] = json!(status);
        item["note"] = json!(note);
        out.push(item);
    }
    json!({"file": true, "recommendations": out, "unwanted": ids(json.get("unwantedRecommendations"))})
}

/// Versions as VS Code compares them: their numbers first (1.10 after 1.9), then a release after its pre-release,
/// then as text.
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let numbers = |v: &str| -> Vec<u64> {
        v.split(['-', '+']).next().unwrap_or("").split('.').map(|p| p.parse().unwrap_or(0)).collect()
    };
    let (x, y) = (numbers(a), numbers(b));
    (0..x.len().max(y.len()))
        .map(|i| x.get(i).unwrap_or(&0).cmp(y.get(i).unwrap_or(&0)))
        .find(|o| o.is_ne())
        // A pre-release (`2.0.0-next`) comes before its release, as semver has it.
        .unwrap_or_else(|| b.contains('-').cmp(&a.contains('-')).then_with(|| a.cmp(b)))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// A .vsix of these files at `at/name` (each under `extension/` unless its name begins with `!`).
    fn vsix_named(at: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let path = at.join(name);
        let mut z = zip::ZipWriter::new(std::fs::File::create(&path).expect("vsix"));
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("[Content_Types].xml", o).expect("start");
        for (name, body) in files {
            let name = match name.strip_prefix('!') {
                Some(raw) => raw.to_string(),
                None => format!("extension/{name}"),
            };
            z.start_file(name, o).expect("start");
            z.write_all(body.as_bytes()).expect("write");
        }
        z.finish().expect("finish");
        path
    }

    fn vsix(at: &Path, files: &[(&str, &str)]) -> PathBuf {
        vsix_named(at, "x.vsix", files)
    }

    const THEMED: &str = r#"{
        "publisher": "Acme", "name": "night", "version": "1.2.0", "displayName": "%display%",
        "main": "./out/extension.js",
        "contributes": {
            "themes": [{"label": "Night", "uiTheme": "vs-dark", "path": "./themes/night.json"},
                       {"label": "Old", "path": "./themes/old.tmTheme"},
                       {"label": "Away", "path": "../../etc/passwd.json"}],
            "keybindings": [{"command": "acme.go", "key": "ctrl+g"},
                            {"command": "editor.action.formatDocument", "key": "alt+f"}],
            "commands": [{"command": "acme.go", "title": "Go"}],
            "views": {}
        }
    }"#;

    fn themed(at: &Path) -> PathBuf {
        vsix(at, &[
            ("package.json", THEMED),
            ("package.nls.json", r#"{"display": "Acme Night"}"#),
            ("themes/night.json", r#"{"include": "./base.json", "colors": {}}"#),
            ("themes/base.json", r#"{"colors": {}}"#),
            ("themes/unused.json", "{}"),
            ("out/extension.js", "run()"),
        ])
    }

    #[test]
    fn a_path_a_manifest_gives_stays_inside_the_extension() {
        assert_eq!(inside("", "./themes/a.json").as_deref(), Some("themes/a.json"));
        assert_eq!(inside("themes", "../icons/x.svg").as_deref(), Some("icons/x.svg"));
        assert_eq!(inside("themes", "..\\a.json").as_deref(), Some("a.json"));
        for out in ["../a.json", "/etc/passwd", "C:/x.json", "themes/../../a", "", "."] {
            assert_eq!(inside("", out), None, "{out}");
        }
    }

    #[test]
    fn placeholders_are_filled_and_versions_ordered_as_vs_code_does() {
        let nls = json!({"a": "Alpha", "b": {"message": "Beta"}});
        let filled = localized(&json!({"x": ["%a%", "%b%", "%c%", "50%"]}), &nls);
        assert_eq!(filled, json!({"x": ["Alpha", "Beta", "%c%", "50%"]}));
        assert!(version_cmp("1.10.0", "1.9.3").is_gt() && version_cmp("2.0.0-pre", "2.0.0").is_lt());
        let guessed = folder_name("ms-python.python-2024.1.0-linux-x64");
        assert_eq!(guessed, Some(("ms-python.python".into(), "2024.1.0".into())));
        assert!(id_ok("esbenp.prettier-vscode") && !id_ok("noperiod") && !id_ok("a/b.c"));
    }

    #[test]
    fn a_vsix_is_installed_with_only_the_files_its_contributions_name() {
        let d = tempfile::tempdir().expect("temp");
        let root = d.path().join("ext");
        let item = install(&root, &themed(d.path())).expect("installed");
        assert_eq!((item["id"].as_str(), item["displayName"].as_str()), (Some("acme.night"), Some("Acme Night")));
        assert_eq!(item["contributes"]["themes"][0]["path"], "themes/night.json");
        assert_eq!(item["contributes"]["keybindings"], 1, "its own command's key needs its code");
        let unused = item["unused"].to_string();
        for said in ["its code", ".tmTheme", "leads outside", "views", "for its own commands"] {
            assert!(unused.contains(said), "{said} in {unused}");
        }
        let kept = root.join("acme.night-1.2.0");
        assert!(kept.join("themes/night.json").is_file() && kept.join("themes/base.json").is_file());
        assert!(!kept.join("themes/unused.json").exists() && !kept.join("out/extension.js").exists());
        assert!(!kept.join("package.json").exists(), "only what the contributions name");
        // What is on is merged, each with its extension; a file it names is served, any other is not.
        let c = contributions(&root, "ext_contributions");
        assert_eq!(c["themes"][0]["extension"], "acme.night");
        assert_eq!(c["keybindings"][0]["key"], "alt+f");
        let (mime, bytes) = file(&root, "acme.night", "themes/base.json").expect("file");
        assert_eq!((mime, bytes.as_slice()), ("application/json", br#"{"colors": {}}"#.as_slice()));
        assert!(file(&root, "acme.night", "themes/unused.json").is_err());
        assert!(file(&root, "acme.night", "../installed.json").is_err());
        // Off: kept, but its contributions do not apply; a new version keeps it off and replaces the old one.
        assert_eq!(enable(&root, "ACME.night", false).expect("off")["enabled"], false);
        assert_eq!(contributions(&root, "ext_contributions")["themes"], json!([]));
        let pkg = THEMED.replace("1.2.0", "1.3.0");
        let newer = vsix_named(d.path(), "y.vsix", &[("package.json", &pkg), ("themes/night.json", "{}")]);
        assert_eq!(install(&root, &newer).expect("newer")["enabled"], false);
        assert!(!kept.exists() && root.join("acme.night-1.3.0/themes/night.json").is_file());
        assert_eq!(list(&root).len(), 1);
        uninstall(&root, "acme.night").expect("gone");
        assert!(list(&root).is_empty() && !root.join("acme.night-1.3.0").exists());
        assert!(uninstall(&root, "acme.night").is_err());
    }

    #[test]
    fn a_vsix_that_leads_out_of_itself_or_holds_a_link_is_refused_whole() {
        let d = tempfile::tempdir().expect("temp");
        let root = d.path().join("ext");
        let escape = vsix(d.path(), &[("package.json", THEMED), ("!extension/../../evil.json", "{}")]);
        assert!(install(&root, &escape).expect_err("refused").text.contains("leads out of it"));
        let path = d.path().join("link.vsix");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&path).expect("vsix"));
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("extension/package.json", o).expect("start");
        z.write_all(THEMED.as_bytes()).expect("write");
        z.add_symlink("extension/themes/night.json", "/etc/passwd", o).expect("link");
        z.finish().expect("finish");
        assert!(install(&root, &path).expect_err("refused").text.contains("link"));
        assert!(!root.join(LIST).exists(), "nothing was kept");
        // Not a zip, not an extension, nothing used: each said.
        std::fs::write(d.path().join("plain.vsix"), "hello").expect("w");
        assert!(install(&root, &d.path().join("plain.vsix")).expect_err("zip").text.contains("zip"));
        let bare = vsix(d.path(), &[("readme.md", "")]);
        assert!(install(&root, &bare).expect_err("bare").text.contains("package.json"));
        let pkg = r#"{"publisher": "a", "name": "b", "version": "1.0.0", "main": "x.js", "contributes": {"commands": []}}"#;
        let r = install(&root, &vsix(d.path(), &[("package.json", pkg)])).expect_err("code");
        assert!(r.text.contains("nothing Code Arena can use (its code)") && r.unused[0].contains("its code"));
        // A theme named but not in the extension: left out, and with nothing else it brings, refused.
        let pkg = r#"{"publisher": "a", "name": "b", "version": "1.0.0",
            "contributes": {"themes": [{"label": "T", "path": "./t.json"}]}}"#;
        let missing = vsix(d.path(), &[("package.json", pkg)]);
        assert!(install(&root, &missing).expect_err("missing").text.contains("nothing Code Arena can use"));
        assert!(std::fs::read_dir(&root).expect("root").next().is_none(), "no folder is left behind");
    }

    /// VS Code's extensions folder in `home` with this extension at this version: snippets for Rust.
    fn vscode_has(home: &Path, id: (&str, &str), version: &str) -> PathBuf {
        let dir = home.join(".vscode/extensions").join(format!("{}.{}-{version}", id.0, id.1));
        std::fs::create_dir_all(dir.join("snippets")).expect("dir");
        let pkg = json!({"publisher": id.0, "name": id.1, "version": version, "contributes": {
            "snippets": [{"language": "rust", "path": "./snippets/rust.json"}]}});
        std::fs::write(dir.join("package.json"), pkg.to_string()).expect("w");
        std::fs::write(dir.join("snippets/rust.json"), "{}").expect("w");
        dir
    }

    #[test]
    fn vs_codes_own_extensions_are_found_newest_first_and_imported() {
        let d = tempfile::tempdir().expect("temp");
        let (root, home) = (d.path().join("ext"), d.path().join("home"));
        vscode_has(&home, ("acme", "snips"), "1.9.0");
        let newest = vscode_has(&home, ("acme", "snips"), "1.10.0");
        vscode_has(&home, ("acme", "gone"), "1.0.0");
        let vs = home.join(".vscode/extensions");
        std::fs::write(vs.join(".obsolete"), r#"{"acme.gone-1.0.0": true}"#).expect("w");
        std::fs::create_dir_all(vs.join("broken.thing-2.0.0")).expect("dir");
        std::fs::write(vs.join("broken.thing-2.0.0/package.json"), "{").expect("w");
        let found = local(&root, Some(&home));
        let ids: Vec<&str> = found.iter().filter_map(|v| v["id"].as_str()).collect();
        assert_eq!(ids, ["acme.snips", "broken.thing"]);
        assert_eq!((found[0]["version"].as_str(), found[0]["usable"].as_bool()), (Some("1.10.0"), Some(true)));
        assert_eq!((found[1]["version"].as_str(), found[1]["problem"].is_string()), (Some("2.0.0"), true));
        let item = import(&root, Some(&home), "Acme.Snips").expect("imported");
        assert_eq!((item["version"].as_str(), item["source"].as_str()), (Some("1.10.0"), Some("vscode")));
        assert!(root.join("acme.snips-1.10.0/snippets/rust.json").is_file());
        assert!(newest.join("package.json").is_file(), "VS Code's copy is left as it is");
        assert_eq!(local(&root, Some(&home))[0]["installed"], "1.10.0");
        assert!(import(&root, Some(&home), "acme.none").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_file_reached_through_a_link_in_vs_codes_folder_is_not_taken() {
        let d = tempfile::tempdir().expect("temp");
        let (root, home) = (d.path().join("ext"), d.path().join("home"));
        let dir = vscode_has(&home, ("acme", "snips"), "1.0.0");
        std::fs::remove_file(dir.join("snippets/rust.json")).expect("rm");
        std::fs::write(d.path().join("secret"), "{}").expect("w");
        std::os::unix::fs::symlink(d.path().join("secret"), dir.join("snippets/rust.json")).expect("link");
        assert!(import(&root, Some(&home), "acme.snips").expect_err("link").text.contains("link"));
    }

    #[test]
    fn recommendations_say_where_each_stands_here() {
        let d = tempfile::tempdir().expect("temp");
        let (root, home, project) = (d.path().join("ext"), d.path().join("home"), d.path().join("p"));
        assert_eq!(recommended(&root, Some(&home), &project)["file"], false);
        vscode_has(&home, ("acme", "snips"), "1.0.0");
        install(&root, &themed(d.path())).expect("installed");
        std::fs::create_dir_all(project.join(".vscode")).expect("dir");
        let said = r#"{
            // Comments are allowed, as in VS Code.
            "recommendations": ["acme.night", "esbenp.prettier-vscode", "dbaeumer.vscode-eslint", "acme.snips",
                                "someone.else", "ACME.NIGHT", "not an id"],
            "unwantedRecommendations": ["github.copilot"]
        }"#;
        std::fs::write(project.join(".vscode/extensions.json"), said).expect("w");
        let r = recommended(&root, Some(&home), &project);
        let stands: Vec<(&str, &str)> = r["recommendations"]
            .as_array()
            .expect("list")
            .iter()
            .map(|x| (x["id"].as_str().unwrap_or(""), x["status"].as_str().unwrap_or("")))
            .collect();
        assert_eq!(stands, [
            ("acme.night", "installed"),
            ("esbenp.prettier-vscode", "built-in"),
            ("dbaeumer.vscode-eslint", "unsupported"),
            ("acme.snips", "local"),
            ("someone.else", "unknown"),
        ]);
        assert_eq!(r["unwanted"], json!(["github.copilot"]));
        std::fs::write(project.join(".vscode/extensions.json"), "[1]").expect("w");
        assert!(recommended(&root, Some(&home), &project)["error"].is_string());
    }

    #[test]
    fn each_message_is_answered_with_the_key_it_carries() {
        let d = tempfile::tempdir().expect("temp");
        let at = Folders {
            root: d.path().join("ext"),
            home: Some(d.path().join("home")),
            project: d.path().to_path_buf(),
        };
        themed(d.path());
        let (r, changed) = answer(&at, "ext_install", &json!({"type": "ext_install", "path": "x.vsix"}));
        assert!(changed && r["path"] == "x.vsix" && r["item"]["id"] == "acme.night", "{r}");
        let (r, changed) = answer(&at, "ext_install", &json!({"path": "nope.vsix"}));
        assert!(!changed && r["path"] == "nope.vsix" && r["error"].is_string());
        let (r, _) = answer(&at, "ext_enable", &json!({"id": "acme.night"}));
        assert!(r["error"].as_str().is_some_and(|e| e.contains("on")) && r["id"] == "acme.night");
        let (r, changed) = answer(&at, "ext_enable", &json!({"id": "acme.night", "on": false}));
        assert!(changed && r["on"] == false && r["item"]["enabled"] == false);
        let (r, _) = answer(&at, "ext_file", &json!({"id": "acme.night", "path": "themes/base.json"}));
        assert_eq!((r["mime"].as_str(), r["data"].as_str()), (Some("application/json"), Some("eyJjb2xvcnMiOiB7fX0=")));
        let (r, _) = answer(&at, "extensions", &json!({}));
        assert_eq!(r["items"][0]["id"], "acme.night");
        let (r, changed) = answer(&at, "ext_uninstall", &json!({"id": "acme.night"}));
        assert!(changed && r == json!({"type": "ext_uninstall", "id": "acme.night"}));
        assert_eq!(picked("~/a.vsix", Some(Path::new("/h")), Path::new("/p")), PathBuf::from("/h/a.vsix"));
        assert_eq!(picked("/abs/a.vsix", None, Path::new("/p")), PathBuf::from("/abs/a.vsix"));
        assert_eq!(picked("a.vsix", None, Path::new("/p")), PathBuf::from("/p/a.vsix"));
    }
}
