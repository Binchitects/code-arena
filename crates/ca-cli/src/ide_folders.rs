//! The folder the IDE is opened on: another one opened in the same window (File: Open Folder…), and the folders
//! opened before (Open Recent…).
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn).
//!
//! - `folder_open{path, key?}`: between turns only (refused while one runs). A new session is begun in the folder
//!   (`~/` the home folder's), the one left kept in the store; the project's side is made again when its root is
//!   another. The driver then sends `state` (with the folder's own `view_state`) and `tasks` itself. Always
//!   answered by `folder_opened{path, key, folder?, error?}`: `folder` the folder now worked in, `error` why
//!   nothing changed (not a folder, already open, not prepared).
//! - `recent_folders{key?}` -> `recent_folders{key, home, items: [{path, workspace}]}`: the folders opened, the latest
//!   first, those not there any more left out; answered while a turn runs too. `workspace` is always false (a
//!   workspace of several folders is not opened yet).
//! - `recent_clear{key?}`: Clear Recently Opened; the folder worked in stays. Answered by `recent_folders`.
//!
//! The list is kept in `recent-folders.json` in the data folder; the folder worked in is put first at the start
//! and on each folder opened.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ide_ext::{Area, Ctx, Rule, Shared, kind, send};
use crate::session::Resume;

/// The file the folders are kept in: `{"folders": ["/abs/path", …]}`.
const FILE: &str = "recent-folders.json";

/// The most folders kept.
const MOST: usize = 50;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("folder_open", Rule::Refuse("open another folder")),
        ("recent_folders", Rule::Turn),
        ("recent_clear", Rule::Turn),
    ];

    /// The folder worked in, put first in the list (at the start, and once more for each folder opened).
    fn start(sh: &Shared) {
        let (data, folder) = (sh.paths.data.clone(), sh.folder.clone());
        tokio::task::spawn_blocking(move || {
            let list = pushed(read(&data), &folder);
            let _ = write(&data, &list);
        });
    }

    fn shared(sh: &Shared, m: &Value) -> bool {
        let clear = match kind(m) {
            "recent_folders" => false,
            "recent_clear" => true,
            _ => return false,
        };
        let (data, folder, out, key) = (sh.paths.data.clone(), sh.folder.clone(), sh.out.clone(), m["key"].clone());
        let home = sh.home.as_ref().map(|h| h.display().to_string());
        tokio::task::spawn_blocking(move || {
            if clear {
                let _ = write(&data, std::slice::from_ref(&folder));
            }
            let items: Vec<Value> = read(&data)
                .into_iter()
                .filter(|f| Path::new(f).is_dir())
                .map(|f| json!({"path": f, "workspace": false}))
                .collect();
            send(&out, json!({"type": "recent_folders", "key": key, "items": items, "home": home}));
        });
        true
    }

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        if kind(m) != "folder_open" {
            return false;
        }
        let path = m["path"].as_str().unwrap_or("");
        let mut reply = json!({"type": "folder_opened", "path": path, "key": m["key"]});
        match open(ctx, path).await {
            Ok(folder) => reply["folder"] = json!(folder),
            Err(e) => reply["error"] = json!(e),
        }
        send(&ctx.drv.out, reply);
        true
    }
}

/// Another folder worked in: a new session begun there. The folder now worked in, or why nothing changed.
async fn open(ctx: &mut Ctx<'_>, path: &str) -> Result<String, String> {
    let dir = folder_of(path, ctx.sh.home.as_deref())?;
    if dir == ctx.p.agent.ctx.workspace.cwd() {
        return Err(format!("{} is open already.", dir.display()));
    }
    // The window lets its terminals go: their shells end, and another side takes the old one's place, so a new
    // shell begins in the folder opened even when it is under the same root.
    let side = ctx.side.clone();
    crate::ide_sessions::switch_in(ctx, Resume::New, Some(dir)).await?;
    side.close_terminals();
    Ok(ctx.p.folder.clone())
}

/// The folder a typed path names, made whole and without links; why it cannot be opened.
fn folder_of(path: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("Which folder to open?".into());
    }
    let whole = crate::ide_chat::mentions::whole(path.trim(), home);
    if !whole.is_absolute() {
        return Err(format!("{path} is not a whole path."));
    }
    match std::fs::canonicalize(&whole) {
        Ok(dir) if dir.is_dir() => Ok(dir),
        Ok(_) => Err(format!("{} is not a folder.", whole.display())),
        Err(e) => Err(format!("{} cannot be opened: {e}", whole.display())),
    }
}

/// The folders kept, the latest first; none when the file is not there or not read.
fn read(data: &Path) -> Vec<String> {
    std::fs::read_to_string(data.join(FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["folders"].as_array().cloned())
        .into_iter()
        .flatten()
        .filter_map(|f| f.as_str().map(str::to_string))
        .collect()
}

/// The list with a folder put first: once, `MOST` at most.
fn pushed(mut list: Vec<String>, folder: &str) -> Vec<String> {
    if folder.is_empty() {
        return list;
    }
    list.retain(|f| f != folder);
    list.insert(0, folder.to_string());
    list.truncate(MOST);
    list
}

/// The list kept, written whole so that a reader never finds half a file.
fn write(data: &Path, list: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(data)?;
    let tmp = data.join(format!("{FILE}.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&json!({"folders": list}))?)?;
    std::fs::rename(tmp, data.join(FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_opened_again_goes_first_once_and_the_list_is_cut() {
        let list = pushed(vec!["/a".into(), "/b".into(), "/c".into()], "/b");
        assert_eq!(list, ["/b", "/a", "/c"]);
        let long: Vec<String> = (0..MOST).map(|i| format!("/{i}")).collect();
        let list = pushed(long, "/new");
        assert_eq!((list.len(), list[0].as_str(), list.last().map(String::as_str)), (MOST, "/new", Some("/48")));
        assert_eq!(pushed(vec!["/a".into()], ""), ["/a"]);
    }

    #[test]
    fn the_list_is_kept_and_read_back_and_a_missing_file_reads_as_none() {
        let data = tempfile::tempdir().expect("ok");
        assert!(read(data.path()).is_empty());
        write(data.path(), &["/x".into(), "/y".into()]).expect("ok");
        assert_eq!(read(data.path()), ["/x", "/y"]);
    }

    #[test]
    fn only_a_whole_path_to_a_folder_can_be_opened() {
        let home = tempfile::tempdir().expect("ok");
        std::fs::create_dir(home.path().join("proj")).expect("ok");
        std::fs::write(home.path().join("a.txt"), "x").expect("ok");
        let found = folder_of("~/proj", Some(home.path())).expect("ok");
        assert_eq!(found, std::fs::canonicalize(home.path().join("proj")).expect("ok"));
        assert!(folder_of("~/a.txt", Some(home.path())).expect_err("refused").contains("is not a folder"));
        assert!(folder_of("proj", Some(home.path())).expect_err("refused").contains("not a whole path"));
        assert!(folder_of("~/gone", Some(home.path())).expect_err("refused").contains("cannot be opened"));
        assert_eq!(folder_of("  ", None).expect_err("refused"), "Which folder to open?");
    }
}
