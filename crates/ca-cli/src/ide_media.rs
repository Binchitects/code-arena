//! A picture, a sound, a video or a diagram for the window: a file of the project's, read as bytes, and the
//! system's own player opened on one.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn).
//!
//! - `media{path, data?}` → `media{path, mime, size, modified, data}`: the file's bytes in base64 (`data: false`
//!   asks for the rest only: a sound or a video is shown by its name and size), or `media{path, error}`. Only a
//!   plain file of the project's, and only up to 50 MB when its bytes are asked for.
//! - `media_open{path}` → `media_open{path}` once the system's player was started on it (xdg-open, open,
//!   start), or `media_open{path, error}`.

use std::path::Path;

use base64::Engine;
use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, send};
use crate::ide_files::{Side, modified};

/// The most bytes a file's `media` reply carries.
const MOST_BYTES: u64 = 50 * 1024 * 1024;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[("media", Rule::Side), ("media_open", Rule::Side)];

    fn side(side: &Side, m: &Value) -> bool {
        let path = m["path"].as_str().unwrap_or("");
        let reply = match m["type"].as_str().unwrap_or("") {
            "media" => media(side, path, m["data"] != false),
            _ => match side.real(path).and_then(|real| system_open(&real)) {
                Ok(()) => json!({"type": "media_open", "path": path}),
                Err(e) => json!({"type": "media_open", "path": path, "error": e}),
            },
        };
        send(side.out(), reply);
        true
    }
}

/// A file's media type, by its name's ending (what the window shows it as).
pub fn mime(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "weba" => "audio/webm",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

fn media(side: &Side, path: &str, with_data: bool) -> Value {
    let fail = |e: &str| json!({"type": "media", "path": path, "error": e});
    let real = match side.real(path) {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    let Ok(meta) = std::fs::metadata(&real) else { return fail("not there") };
    if !meta.is_file() {
        return fail("not a plain file (a folder, a pipe or a device is not opened)");
    }
    let shown = side.shown(&real);
    let mut reply = json!({
        "type": "media",
        "path": shown,
        "mime": mime(&shown),
        "size": meta.len(),
        "modified": modified(&meta),
    });
    if with_data {
        if meta.len() > MOST_BYTES {
            return fail("bigger than 50 MB: open it with another program");
        }
        let Ok(bytes) = std::fs::read(&real) else { return fail("it could not be read") };
        reply["data"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
    }
    reply
}

/// The system's own program for a file started on it, as the page's "open with the system" is.
fn system_open(real: &Path) -> Result<(), String> {
    let (program, mut args): (&str, Vec<std::ffi::OsString>) = if cfg!(target_os = "macos") {
        ("open", vec![])
    } else if cfg!(windows) {
        ("cmd", vec!["/C".into(), "start".into(), "".into()])
    } else {
        ("xdg-open", vec![])
    };
    args.push(real.as_os_str().to_owned());
    std::process::Command::new(program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{program} could not be started: {e}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn a_file_is_read_as_bytes_with_its_type_and_size() {
        let d = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(d.path().join("img")).expect("dir");
        std::fs::write(d.path().join("img/a.PNG"), [137, 80, 78, 71, 0, 1]).expect("w");
        std::fs::write(d.path().join("song.mp3"), [1, 2, 3]).expect("w");
        let ws = Arc::new(ca_workspace::Workspace::open(d.path(), &[]).expect("ws"));
        let (out, mut rx) = tokio::sync::broadcast::channel(16);
        let side = Side::new(ws, Arc::default(), out, None, &std::env::temp_dir());
        let mut ask = |m: Value| -> Value {
            assert!(side.handle(&m));
            serde_json::from_str(&rx.try_recv().expect("a reply")).expect("json")
        };
        let got = ask(json!({"type": "media", "path": "img/a.PNG"}));
        assert_eq!((got["path"].as_str(), got["mime"].as_str(), got["size"].as_u64()), (Some("img/a.PNG"), Some("image/png"), Some(6)));
        assert_eq!(got["data"], "iVBORwAB");
        assert!(got["modified"].as_u64().is_some_and(|m| m > 0));
        // A sound by its size only, when its bytes are not asked for.
        let got = ask(json!({"type": "media", "path": "song.mp3", "data": false}));
        assert_eq!((got["mime"].as_str(), got["size"].as_u64(), got.get("data")), (Some("audio/mpeg"), Some(3), None));
        // What is not a plain file of the project's is refused, with the path it was asked by.
        let got = ask(json!({"type": "media", "path": "img"}));
        assert!(got["error"].as_str().is_some_and(|e| e.contains("plain file")), "{got}");
        let got = ask(json!({"type": "media", "path": "/etc/passwd"}));
        assert_eq!((got["path"].as_str(), got["error"].is_string()), (Some("/etc/passwd"), true));
        let got = ask(json!({"type": "media_open", "path": "../out.mp3"}));
        assert_eq!((got["path"].as_str(), got["error"].is_string()), (Some("../out.mp3"), true));
    }

    #[test]
    fn a_files_type_is_known_by_its_ending() {
        assert_eq!(mime("a/b.JPEG"), "image/jpeg");
        assert_eq!(mime("clip.mov"), "video/quicktime");
        assert_eq!(mime("voice.weba"), "audio/webm");
        assert_eq!(mime("Makefile"), "application/octet-stream");
    }
}
