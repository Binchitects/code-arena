//! Whether the folder is trusted: what of its own (formatters, language servers, tasks, debugging, kernels, the
//! project's tools) may run. VS Code's workspace trust, kept on this computer in the data folder's `trust.json`.
//!
//! - `trust` [T] is answered with `trust{folder, trusted, asked}`: the folder open now, whether its programs may
//!   run, and whether the person has answered for it (or for a folder it is in) already, so a face asks only once.
//! - `trust_set{trusted, parent?}` [T] keeps the answer for the folder (for the folder it is in, with `parent`) and
//!   says `trust{folder, trusted, asked}` to every face. Taking trust back calls what the areas gave
//!   [`on_revoke`], so the servers the folder's programs run stop.
//!
//! The other areas ask [`trusted`] before they run anything of the folder's, and refuse with `kind: "not_trusted"`
//! in their reply when it is not: the window shows that as a toast with "Trust".
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use crate::ide_ext::{Area, Kept, Rule, Shared, kind, send};

/// The file in the data folder the answers are kept in: `{"folders": {"/abs/path": true}}`.
const FILE: &str = "trust.json";

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[("trust", Rule::Turn), ("trust_set", Rule::Turn)];

    fn shared(sh: &Shared, m: &Value) -> bool {
        let (data, root) = (sh.paths.data.clone(), sh.workspace.root().to_path_buf());
        match kind(m) {
            "trust" => {
                let out = sh.out.clone();
                tokio::task::spawn_blocking(move || send(&out, message(&data, &root)));
            }
            "trust_set" => {
                let on = m["trusted"].as_bool().unwrap_or(false);
                let parent = m["parent"].as_bool().unwrap_or(false);
                let sh = sh.clone();
                tokio::task::spawn_blocking(move || {
                    let was = trusted(&data, &root);
                    let kept = set(&data, &root, on, parent);
                    if was && !trusted(&data, &root) {
                        revoked(&sh);
                    }
                    let mut reply = message(&data, &root);
                    if let Err(e) = kept {
                        reply["error"] = json!(format!("The folder's trust was not kept: {e}"));
                    }
                    send(&sh.out, reply);
                });
            }
            _ => return false,
        }
        true
    }
}

/// Whether the folder's own programs may run: the nearest answer kept for it or for a folder it is in.
pub fn trusted(data: &Path, root: &Path) -> bool {
    decision(&read(data), root).unwrap_or(false)
}

/// The `trust` message for a folder.
pub fn message(data: &Path, root: &Path) -> Value {
    let kept = decision(&read(data), root);
    json!({"type": "trust", "folder": root.display().to_string(), "trusted": kept.unwrap_or(false), "asked": kept.is_some()})
}

/// What an area stops when the folder's trust is taken back (its servers), by the area's name: given again from
/// `start`, it takes the place of the one before.
#[allow(dead_code)] // For the areas that run the folder's servers.
pub fn on_revoke(kept: &Kept, area: &'static str, stop: impl Fn(&Shared) + Send + Sync + 'static) {
    let stops = kept.of::<Stops>();
    let mut all = stops.0.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|(name, _)| *name != area);
    all.push((area, Arc::new(stop)));
}

/// What the areas gave [`on_revoke`].
#[derive(Default)]
struct Stops(Mutex<Vec<(&'static str, Arc<dyn Fn(&Shared) + Send + Sync>)>>);

fn revoked(sh: &Shared) {
    let all: Vec<_> =
        sh.kept.of::<Stops>().0.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, f)| f.clone()).collect();
    for stop in all {
        stop(sh);
    }
}

/// The answers kept, by folder; nothing when the file is not there or not read.
fn read(data: &Path) -> Map<String, Value> {
    std::fs::read_to_string(data.join(FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("folders").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}

/// The nearest answer for a folder: its own, else that of the nearest folder it is in.
fn decision(folders: &Map<String, Value>, root: &Path) -> Option<bool> {
    root.ancestors().find_map(|p| folders.get(&p.display().to_string()).and_then(Value::as_bool))
}

/// Keeps an answer for a folder (or, with `parent`, for the folder it is in), written whole so that a reader
/// never finds half a file.
fn set(data: &Path, root: &Path, on: bool, parent: bool) -> std::io::Result<()> {
    let mut folders = read(data);
    let at: PathBuf = match root.parent() {
        Some(p) if parent && on => p.to_path_buf(),
        _ => root.to_path_buf(),
    };
    folders.insert(at.display().to_string(), Value::Bool(on));
    // An answer of its own given now is the folder's, whatever was kept for it before under the folder it is in.
    if at != root {
        folders.remove(&root.display().to_string());
    }
    std::fs::create_dir_all(data)?;
    let tmp = data.join(format!("{FILE}.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&json!({"folders": folders}))?)?;
    std::fs::rename(tmp, data.join(FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_not_trusted_until_the_person_says_so_and_the_answer_is_kept() {
        let d = tempfile::tempdir().expect("dir");
        let root = Path::new("/work/repo");
        assert!(!trusted(d.path(), root));
        assert_eq!(message(d.path(), root)["asked"], false);
        set(d.path(), root, false, false).expect("set");
        assert!(!trusted(d.path(), root));
        assert_eq!(message(d.path(), root)["asked"], true);
        set(d.path(), root, true, false).expect("set");
        assert!(trusted(d.path(), root));
        let m = message(d.path(), root);
        assert_eq!((m["folder"].as_str(), m["trusted"].as_bool()), (Some("/work/repo"), Some(true)));
        assert!(!trusted(d.path(), Path::new("/work/other")));
    }

    #[test]
    fn trusting_the_parent_trusts_the_folders_in_it_and_the_nearest_answer_wins() {
        let d = tempfile::tempdir().expect("dir");
        set(d.path(), Path::new("/work/repo"), true, true).expect("set");
        assert!(trusted(d.path(), Path::new("/work/repo")));
        assert!(trusted(d.path(), Path::new("/work/other/deep")));
        set(d.path(), Path::new("/work/other"), false, false).expect("set");
        assert!(!trusted(d.path(), Path::new("/work/other/deep")));
        assert!(trusted(d.path(), Path::new("/work/repo")));
        // Taking trust back with "parent" takes it from the folder only.
        set(d.path(), Path::new("/work/repo"), false, true).expect("set");
        assert!(!trusted(d.path(), Path::new("/work/repo")));
        assert!(trusted(d.path(), Path::new("/work/third")));
    }

    #[test]
    fn a_file_that_does_not_read_trusts_nothing() {
        let d = tempfile::tempdir().expect("dir");
        std::fs::write(d.path().join(FILE), "{not json").expect("w");
        assert!(!trusted(d.path(), Path::new("/work/repo")));
        set(d.path(), Path::new("/work/repo"), true, false).expect("set");
        assert!(trusted(d.path(), Path::new("/work/repo")));
    }
}
