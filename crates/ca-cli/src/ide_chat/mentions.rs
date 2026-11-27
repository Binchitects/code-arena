//! The box's @ mentions: any folder on this machine listed for a whole path typed after @, and what a message's
//! mentions sent with it.
//!
//! - `folders{path}` → `folders{path, dir, parent, entries: [{name, dir}], more, error}`: the folder `path` names
//!   (`~/` the home folder's), its folders first and then its files, each by name as the system sorts them less
//!   case, `MOST` at most (`more`: there were others). `path` is said again as it came, `dir` is the folder as a
//!   whole path, `parent` the one above it (null at the top); a folder that cannot be read says why in `error`.
//!   A `key` the request carries is said again.
//! - `attached{q, labels}` after `user`: what the engine's mention expansion (`ca_agent::mentions`) sent with
//!   question `q`, each as a chip names it (`src/a.rs:12-30`, `src/`). Not sent when nothing went.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ide_ext::{Ctx, Shared, send};

/// The most entries a folder lists.
const MOST: usize = 2000;

/// A path as typed made whole: `~` the home folder.
fn whole(path: &str, home: Option<&Path>) -> PathBuf {
    let path = match (path.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(path),
    };
    // Without the `/` it ends in, as the folder's own name.
    path.components().collect()
}

/// The folder a path names, listed as the reply says it.
fn list(path: &str, home: Option<&Path>) -> Value {
    let dir = whole(path, home);
    let mut reply = json!({
        "type": "folders",
        "path": path,
        "dir": dir.to_string_lossy().replace('\\', "/"),
        "parent": dir.parent().map(|p| p.to_string_lossy().replace('\\', "/")),
        "entries": [],
        "more": false,
        "error": null,
    });
    if !dir.is_absolute() {
        reply["error"] = json!("Not a whole path.");
        return reply;
    }
    let read = match std::fs::read_dir(&dir) {
        Ok(read) => read,
        Err(e) => {
            reply["error"] = json!(e.to_string());
            return reply;
        }
    };
    let mut entries: Vec<(bool, String)> = read
        .flatten()
        .map(|e| {
            // A link is listed as what it leads to.
            let is_dir = std::fs::metadata(e.path()).map(|m| m.is_dir()).unwrap_or(false);
            (is_dir, e.file_name().to_string_lossy().into_owned())
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    reply["more"] = json!(entries.len() > MOST);
    reply["entries"] =
        entries.into_iter().take(MOST).map(|(dir, name)| json!({"name": name, "dir": dir})).collect();
    reply
}

/// `folders`: the folder listed off the driver's loop.
pub fn asked(sh: &Shared, m: &Value) -> bool {
    let (path, key, home, out) = (m["path"].as_str().unwrap_or("").to_string(), m["key"].clone(), sh.home.clone(), sh.out.clone());
    let work = move || {
        let mut reply = list(&path, home.as_deref());
        if key.is_string() || key.is_number() {
            reply["key"] = key;
        }
        send(&out, reply);
    };
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => drop(rt.spawn_blocking(work)),
        Err(_) => work(),
    }
    true
}

/// A turn begins with this message: what its mentions send with it is named for the question's chips, off the
/// driver's loop. The question's number is the one its `user` said.
pub async fn before_turn(ctx: &mut Ctx<'_>, message: &str) {
    if ca_agent::mentions::find(message).is_empty() {
        return;
    }
    let q = crate::session::questions(ctx.p).map(|qs| qs.len() + 1).unwrap_or(0);
    let (ws, out, message) = (ctx.sh.workspace.clone(), ctx.sh.out.clone(), message.to_string());
    let work = move || {
        let labels = ca_agent::mentions::attached(&message, &ws);
        if !labels.is_empty() {
            send(&out, json!({"type": "attached", "q": q, "labels": labels}));
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => drop(rt.spawn_blocking(work)),
        Err(_) => work(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_lists_its_folders_first_then_its_files() {
        let d = tempfile::tempdir().expect("temp");
        std::fs::create_dir(d.path().join("src")).expect("mkdir");
        std::fs::create_dir(d.path().join("Docs")).expect("mkdir");
        std::fs::write(d.path().join("b.txt"), "").expect("write");
        std::fs::write(d.path().join("A.md"), "").expect("write");
        let path = d.path().to_string_lossy().replace('\\', "/");
        let m = list(&path, None);
        assert_eq!(m["type"], "folders");
        assert_eq!(m["path"], path.as_str());
        assert!(m["error"].is_null());
        let names: Vec<(&str, bool)> = m["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|e| (e["name"].as_str().unwrap_or(""), e["dir"] == true))
            .collect();
        assert_eq!(names, [("Docs", true), ("src", true), ("A.md", false), ("b.txt", false)]);
        assert_eq!(m["parent"], d.path().parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default());
    }

    #[test]
    fn the_home_folder_is_reached_by_a_tilde_and_what_cannot_be_read_says_why() {
        let home = tempfile::tempdir().expect("temp");
        std::fs::create_dir(home.path().join("notes")).expect("mkdir");
        std::fs::write(home.path().join("notes/n.md"), "").expect("write");
        let m = list("~/notes/", Some(home.path()));
        assert_eq!(m["path"], "~/notes/");
        assert_eq!(m["entries"][0]["name"], "n.md");
        assert!(m["dir"].as_str().unwrap_or("").ends_with("notes"));
        let gone = list("~/gone/", Some(home.path()));
        assert!(gone["error"].is_string() && gone["entries"].as_array().is_some_and(Vec::is_empty));
        assert_eq!(list("relative/", None)["error"], "Not a whole path.");
    }
}
