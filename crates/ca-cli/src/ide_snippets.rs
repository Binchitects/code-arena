//! The snippets of the person's and the project's: the files VS Code reads them from, said to the faces as they are
//! on disk, and said again when one changes.
//!
//! - `snippets` → `snippets{files: [{source, path, language, text, version}], problems}`: the person's own (the
//!   settings folder's `snippets/NAME.json`, for the language NAME, and `snippets/NAME.code-snippets`, for any) with
//!   `source: "user"` and `path: "snippets/NAME"` (the name `user_file` and `save_user` take), then the project's
//!   (`.vscode/NAME.code-snippets`) with `source: "workspace"` and its path in the project. `version` is when the file
//!   was last changed. At most [`MOST_FILES`] files of [`MOST_BYTES`] each are read; what is left out is said in
//!   `problems`. Said again by itself when a file is added, changed or taken away.
//! - `snippet_new{name}` → `snippet_new{name, path, made, error}`: a snippets file made with VS Code's commented
//!   example when it is not there yet (never over one that is): `snippets/NAME.json` or `snippets/NAME.code-snippets`
//!   of the person's, or `.vscode/NAME.code-snippets` of the project's.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, Shared, kind, send};
use crate::ide_settings::user_path;

/// How often the snippets files are looked at.
const EVERY: std::time::Duration = std::time::Duration::from_millis(1000);
/// The most files read, and the biggest one read.
pub const MOST_FILES: usize = 100;
pub const MOST_BYTES: u64 = 1024 * 1024;

/// One snippets file found: where it is said to be, where it is on disk, its language, and its stamp.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Found {
    source: &'static str,
    path: String,
    file: PathBuf,
    language: Option<String>,
    stamp: (u64, u64),
}

fn stamp(file: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(file).ok().filter(std::fs::Metadata::is_file)?;
    let at = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
    Some((at.map_or(0, |d| d.as_millis() as u64), meta.len()))
}

/// The files of a folder whose names end in one of `ends`, by name.
fn files_in(dir: &Path, ends: &[&str]) -> Vec<(String, PathBuf)> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut all: Vec<(String, PathBuf)> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let fits = !name.starts_with('.') && ends.iter().any(|end| name.len() > end.len() && name.ends_with(end));
            fits.then(|| (name, e.path()))
        })
        .collect();
    all.sort();
    all
}

/// Every snippets file there is: the person's, then the project's.
fn found(config: &Path, root: &Path) -> Vec<Found> {
    let mut out = Vec::new();
    for (name, file) in files_in(&config.join("snippets"), &[".json", ".code-snippets"]) {
        let Some(stamp) = stamp(&file) else { continue };
        let language = name.strip_suffix(".json").map(str::to_string);
        out.push(Found { source: "user", path: format!("snippets/{name}"), file, language, stamp });
    }
    for (name, file) in files_in(&root.join(".vscode"), &[".code-snippets"]) {
        let Some(stamp) = stamp(&file) else { continue };
        out.push(Found { source: "workspace", path: format!(".vscode/{name}"), file, language: None, stamp });
    }
    out
}

/// The `snippets` message: the files' texts, those not read said in `problems`.
fn message(found: &[Found]) -> Value {
    let mut files = Vec::new();
    let mut problems = Vec::new();
    for (i, f) in found.iter().enumerate() {
        if i >= MOST_FILES {
            problems.push(json!({"path": f.path, "error": format!("only {MOST_FILES} snippets files are read")}));
            continue;
        }
        if f.stamp.1 > MOST_BYTES {
            problems.push(json!({"path": f.path, "error": "the file is bigger than 1 MB"}));
            continue;
        }
        match std::fs::read_to_string(&f.file) {
            Ok(text) => files.push(json!({
                "source": f.source,
                "path": f.path,
                "language": f.language,
                "text": text,
                "version": f.stamp.0,
            })),
            Err(e) => problems.push(json!({"path": f.path, "error": e.to_string()})),
        }
    }
    json!({"type": "snippets", "files": files, "problems": problems})
}

/// A new snippets file's text, as VS Code starts one: its example in a comment (a language's file has no scope).
pub fn template(scoped: bool) -> String {
    let mut t = String::from("{\n");
    if scoped {
        t.push_str(
            "\t// Place your global snippets here. Each snippet is defined under a snippet name and has a scope, prefix, \
             body and\n\t// description. Add comma separated ids of the languages where the snippet is applicable in the \
             scope field. If scope\n\t// is left empty or omitted, the snippet gets applied to all languages. The prefix \
             is what is\n\t// used to trigger the snippet and the body will be expanded and inserted. Possible \
             variables are:\n\t// $1, $2 for tab stops, $0 for the final cursor position, and ${1:label}, ${2:another} \
             for placeholders.\n\t// Placeholders with the same ids are connected.\n\t// Example:\n\t// \"Print to \
             console\": {\n\t// \t\"scope\": \"javascript,typescript\",\n",
        );
    } else {
        t.push_str(
            "\t// Place your snippets here. Each snippet is defined under a snippet name and has a prefix, body and\n\t// \
             description. The prefix is what is used to trigger the snippet and the body will be expanded and \
             inserted. Possible variables are:\n\t// $1, $2 for tab stops, $0 for the final cursor position, and \
             ${1:label}, ${2:another} for placeholders. Placeholders with the\n\t// same ids are connected.\n\t// \
             Example:\n\t// \"Print to console\": {\n",
        );
    }
    t.push_str(
        "\t// \t\"prefix\": \"log\",\n\t// \t\"body\": [\n\t// \t\t\"console.log('$1');\",\n\t// \t\t\"$2\"\n\t// \t],\n\t// \
         \t\"description\": \"Log output to console\"\n\t// }\n}\n",
    );
    t
}

/// Where a new snippets file goes, by the name asked: one of the person's (`snippets/NAME`), or the project's
/// (`.vscode/NAME.code-snippets`, a plain name).
fn place(config: &Path, root: &Path, name: &str) -> Result<PathBuf, String> {
    if name.starts_with("snippets/") {
        return user_path(config, name);
    }
    let base = name.strip_prefix(".vscode/").and_then(|n| n.strip_suffix(".code-snippets"));
    match base {
        Some(b) if !b.is_empty() && !b.starts_with('.') && b.chars().all(|c| c.is_alphanumeric() || "._-".contains(c)) => {
            Ok(root.join(name))
        }
        _ => Err(format!("{name} is not a snippets file")),
    }
}

/// The `snippet_new` message: the file made with its example, unless it is there already.
fn make(config: &Path, root: &Path, name: &str) -> Value {
    let file = match place(config, root, name) {
        Ok(f) => f,
        Err(e) => return json!({"type": "snippet_new", "name": name, "error": e}),
    };
    if file.exists() {
        return json!({"type": "snippet_new", "name": name, "path": name, "made": false});
    }
    let text = template(!name.ends_with(".json"));
    let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, text));
    match written {
        Ok(()) => json!({"type": "snippet_new", "name": name, "path": name, "made": true}),
        Err(e) => json!({"type": "snippet_new", "name": name, "error": format!("{}: {e}", file.display())}),
    }
}

/// What the files were at the last look, and which looking is the current one.
#[derive(Default)]
struct Live(Mutex<(u64, Vec<(String, (u64, u64))>)>);

fn stamps(found: &[Found]) -> Vec<(String, (u64, u64))> {
    found.iter().map(|f| (f.path.clone(), f.stamp)).collect()
}

fn where_(sh: &Shared) -> (PathBuf, PathBuf) {
    (sh.paths.config.clone(), sh.workspace.root().to_path_buf())
}

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[("snippets", Rule::Turn), ("snippet_new", Rule::Turn)];

    /// The snippets files are looked at from now on (each second: their stamps only), and said again when one was
    /// added, changed or taken away; the looking begun for the session or folder before ends.
    fn start(sh: &Shared) {
        let live = sh.kept.of::<Live>();
        let run = {
            let mut l = live.0.lock().unwrap_or_else(|e| e.into_inner());
            l.0 += 1;
            l.1.clear();
            l.0
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let sh = sh.clone();
        runtime.spawn(async move {
            let mut first = true;
            loop {
                if !first {
                    tokio::time::sleep(EVERY).await;
                }
                if live.0.lock().unwrap_or_else(|e| e.into_inner()).0 != run {
                    break;
                }
                let (sh, live, said_first) = (sh.clone(), live.clone(), first);
                first = false;
                let _ = tokio::task::spawn_blocking(move || {
                    let (config, root) = where_(&sh);
                    let now = found(&config, &root);
                    let mut l = live.0.lock().unwrap_or_else(|e| e.into_inner());
                    if l.0 != run {
                        return;
                    }
                    let changed = l.1 != stamps(&now);
                    l.1 = stamps(&now);
                    drop(l);
                    // Another folder's snippets are said at once (the first are asked for); then only a change.
                    if (said_first && run > 1) || (changed && !said_first) {
                        send(&sh.out, message(&now));
                    }
                })
                .await;
            }
        });
    }

    fn shared(sh: &Shared, m: &Value) -> bool {
        let (config, root) = where_(sh);
        let out = sh.out.clone();
        match kind(m) {
            "snippets" => {
                tokio::task::spawn_blocking(move || send(&out, message(&found(&config, &root))));
            }
            "snippet_new" => {
                let name = m["name"].as_str().unwrap_or("").to_string();
                tokio::task::spawn_blocking(move || {
                    send(&out, make(&config, &root, &name));
                    // The new file is in the list at once, not at the next look.
                    send(&out, message(&found(&config, &root)));
                });
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_persons_and_the_projects_files_are_listed_with_their_languages() {
        let (c, r) = (tempfile::tempdir().expect("temp"), tempfile::tempdir().expect("temp"));
        std::fs::create_dir_all(c.path().join("snippets")).expect("dir");
        std::fs::create_dir_all(r.path().join(".vscode")).expect("dir");
        std::fs::write(c.path().join("snippets/rust.json"), "{}").expect("write");
        std::fs::write(c.path().join("snippets/all.code-snippets"), "{\"a\": {}}").expect("write");
        std::fs::write(c.path().join("snippets/notes.txt"), "no").expect("write");
        std::fs::write(r.path().join(".vscode/team.code-snippets"), "{}").expect("write");
        std::fs::write(r.path().join(".vscode/settings.json"), "{}").expect("write");
        let m = message(&found(c.path(), r.path()));
        let files = m["files"].as_array().expect("files");
        let seen: Vec<(&str, &str, Value)> = files
            .iter()
            .map(|f| (f["source"].as_str().unwrap_or(""), f["path"].as_str().unwrap_or(""), f["language"].clone()))
            .collect();
        assert_eq!(
            seen,
            [
                ("user", "snippets/all.code-snippets", Value::Null),
                ("user", "snippets/rust.json", json!("rust")),
                ("workspace", ".vscode/team.code-snippets", Value::Null),
            ]
        );
        assert_eq!(files[0]["text"], "{\"a\": {}}");
        assert!(files[0]["version"].as_u64().is_some());
        assert_eq!(m["problems"], json!([]));
    }

    #[test]
    fn too_many_files_and_too_big_a_file_are_said_not_read() {
        let (c, r) = (tempfile::tempdir().expect("temp"), tempfile::tempdir().expect("temp"));
        let dir = c.path().join("snippets");
        std::fs::create_dir_all(&dir).expect("dir");
        for i in 0..MOST_FILES + 2 {
            std::fs::write(dir.join(format!("f{i:03}.code-snippets")), "{}").expect("write");
        }
        std::fs::write(dir.join("big.json"), "x".repeat(MOST_BYTES as usize + 1)).expect("write");
        let m = message(&found(c.path(), r.path()));
        assert_eq!(m["files"].as_array().map(Vec::len), Some(MOST_FILES - 1));
        let problems = m["problems"].as_array().expect("problems");
        assert_eq!(problems[0], json!({"path": "snippets/big.json", "error": "the file is bigger than 1 MB"}));
        assert_eq!(problems.len(), 4);
    }

    #[test]
    fn a_new_file_is_made_with_its_example_once_and_only_where_snippets_go() {
        let (c, r) = (tempfile::tempdir().expect("temp"), tempfile::tempdir().expect("temp"));
        let made = make(c.path(), r.path(), ".vscode/team.code-snippets");
        assert_eq!(made["made"], true);
        let text = std::fs::read_to_string(r.path().join(".vscode/team.code-snippets")).expect("made");
        assert!(text.contains("\"scope\": \"javascript,typescript\""));
        // The example is a comment: the file is an empty object.
        let parsed: Value = serde_json::from_str(&ca_settings::jsonc::strip(&text)).expect("json");
        assert_eq!(parsed, json!({}));
        std::fs::write(r.path().join(".vscode/team.code-snippets"), "{\"mine\": 1}").expect("write");
        assert_eq!(make(c.path(), r.path(), ".vscode/team.code-snippets")["made"], false);
        assert_eq!(std::fs::read_to_string(r.path().join(".vscode/team.code-snippets")).expect("kept"), "{\"mine\": 1}");
        let lang = make(c.path(), r.path(), "snippets/rust.json");
        assert_eq!((lang["made"].clone(), lang["path"].clone()), (json!(true), json!("snippets/rust.json")));
        let text = std::fs::read_to_string(c.path().join("snippets/rust.json")).expect("made");
        assert!(!text.contains("scope"));
        for bad in [".vscode/../x.code-snippets", ".vscode/a/b.code-snippets", ".vscode/x.json", "snippets/../k.json", "x"] {
            assert!(make(c.path(), r.path(), bad).get("error").is_some(), "{bad}");
        }
    }

    #[test]
    fn the_messages_are_this_areas() {
        use crate::ide_ext::rule;
        assert_eq!((rule("snippets"), rule("snippet_new")), (Some(Rule::Turn), Some(Rule::Turn)));
    }
}
