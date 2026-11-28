//! Files attached to a message in the IDE (the paperclip, a paste, a drop on the chat's box), as Arena's web chat
//! attaches them: each sent to Arena as soon as it is added, which reads a document's text (PDF, Word, Excel,
//! PowerPoint, text, code) and keeps a picture, a sound or a video. The model reads a document's text after the
//! message, between `<uploaded>` and `</uploaded>`, and sees a picture when it can (a part of its own; named
//! otherwise). The session keeps that, and the request's details keep the files for its bubble.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver.
//!
//! - `attach{key, name, from? | data?, mime?}` [T]: the file at `from` on this machine, or `data` (base64: a
//!   paste), sent up. While it goes: `attach_progress{key, name, pct}`; then `attached_file{key, name, id, size,
//!   kind, cut, content_type, thumb?}` (`thumb`: a small PNG of a picture, as a data URL), or `attached_file{key,
//!   name, error}`.
//! - `attach_pick{key}` [T]: the system's own dialog for choosing files; `attach_picked{key, paths}` (none when it
//!   was cancelled), or `attach_picked{key, paths: [], error}` when the system has none.
//! - `send{attachments: [id]}`: the files go with the message (see above); the `user` (and `queued`) message carries
//!   `attachments: [{id, name, size, kind, cut, thumb?}]`. Sent while a turn runs, it waits for its own turn: the
//!   running turn reads only words.
//! - `state.attach: {arena, most_bytes}`: whether files may be attached at all (only with Arena), and the most one
//!   may hold. `attach_limits{arena, most_bytes}` says it again once Arena has said its limit.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use ca_arena::attachments::{DEFAULT_MOST_BYTES, MOST_FILES, Uploaded};
use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, Sent, Shared, send};
use crate::ide_files::Side;
use crate::session::Prepared;

/// The most of one document's text a message carries; more is cut.
pub const MOST_CHARS: usize = 60_000;

/// The most bytes of one picture the model is given.
const MOST_PICTURE_BYTES: u64 = 5 * 1024 * 1024;

/// The side of a picture's small copy, in pixels (twice the chip's, for a sharp one).
const THUMB: u32 = 128;

pub const MARKER: &str = "\n\n<uploaded>";
pub const END: &str = "\n</uploaded>";

/// This area in the driver.
pub struct Here;

/// A file sent up, as the model will read it: Arena's answer, its small picture, and its text or picture as read
/// back from Arena (none when it could not be).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kept {
    pub up: Uploaded,
    pub thumb: String,
    pub text: Option<String>,
    pub picture: Option<String>,
}

/// The files sent up while the driver runs, by their id; and the most bytes Arena takes.
#[derive(Default)]
pub struct Files {
    files: Mutex<HashMap<String, Kept>>,
    most: AtomicU64,
}

impl Files {
    fn get(&self, id: &str) -> Option<Kept> {
        self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(id).cloned()
    }

    fn keep(&self, k: Kept) {
        self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(k.up.id.clone(), k);
    }

    fn most(&self) -> u64 {
        match self.most.load(Ordering::Relaxed) {
            0 => DEFAULT_MOST_BYTES,
            n => n,
        }
    }
}

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[("attach", Rule::Turn), ("attach_pick", Rule::Side)];

    fn start(sh: &Shared) {
        let Some(arena) = sh.arena.clone() else { return };
        let (files, out) = (sh.kept.of::<Files>(), sh.out.clone());
        tokio::spawn(async move {
            let Ok(config) = arena.config().await else { return };
            if let Some(most) = config["maxUploadBytes"].as_u64().filter(|n| *n > 0)
                && files.most.swap(most, Ordering::Relaxed) != most
            {
                send(&out, json!({"type": "attach_limits", "arena": true, "most_bytes": most}));
            }
        });
    }

    fn side(side: &Side, m: &Value) -> bool {
        let (key, out) = (m["key"].clone(), side.out().clone());
        tokio::task::spawn_blocking(move || {
            let reply = match pick() {
                Ok(paths) => json!({"type": "attach_picked", "key": key, "paths": paths}),
                Err(e) => json!({"type": "attach_picked", "key": key, "paths": [], "error": e}),
            };
            send(&out, reply);
        });
        true
    }

    fn shared(sh: &Shared, m: &Value) -> bool {
        tokio::spawn(upload(sh.clone(), m.clone()));
        true
    }

    fn shape(sh: &Shared, sent: &mut Sent) {
        let ids: Vec<String> = sent.msg["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_str().or_else(|| a["id"].as_str()))
            .take(MOST_FILES)
            .map(str::to_string)
            .collect();
        if ids.is_empty() {
            return;
        }
        let files = sh.kept.of::<Files>();
        let kept: Vec<Kept> = ids
            .iter()
            .map(|id| {
                files.get(id).unwrap_or_else(|| Kept {
                    up: Uploaded { id: id.clone(), ..Default::default() },
                    ..Default::default()
                })
            })
            .collect();
        let sees = sh.kept.of::<crate::ide_chat::pickers::Infos>().sees(&sh.was.model);
        sent.attached = read(&kept, sees);
        sent.fields.insert("attachments".into(), kept.iter().map(shown).collect());
        // The running turn reads only words: files wait for a turn of their own.
        if sent.meanwhile {
            sent.later = true;
        }
    }

    fn state(sh: &Shared, _p: Option<&Prepared>, v: &mut Value) {
        v["attach"] = json!({"arena": sh.arena.is_some(), "most_bytes": sh.kept.of::<Files>().most()});
    }
}

/// A file as a face shows it under its question.
fn shown(k: &Kept) -> Value {
    let mut v =
        json!({"id": k.up.id, "name": k.up.file_name, "size": k.up.size, "kind": k.up.kind, "cut": k.up.truncated});
    if !k.thumb.is_empty() {
        v["thumb"] = json!(k.thumb);
    }
    v
}

/// A name or a type as an attribute holds it: no quotes, no line breaks, no angle brackets.
fn clean(value: &str) -> String {
    let text: String =
        value.chars().map(|c| if matches!(c, '"' | '<' | '>') || c.is_control() { '_' } else { c }).collect();
    text.trim().chars().take(200).collect()
}

/// What the model reads of the files after the message (each document's text, and each picture by its name), and
/// the pictures it sees when `sees`.
pub fn read(files: &[Kept], sees: bool) -> ca_agent::Attached {
    let mut section = String::from(MARKER);
    let mut images = Vec::new();
    for f in files {
        let kind = if f.up.kind.is_empty() { "file" } else { f.up.kind.as_str() };
        let name = if f.up.file_name.is_empty() { "file".to_string() } else { clean(&f.up.file_name) };
        let ty = if f.up.content_type.is_empty() {
            "application/octet-stream".to_string()
        } else {
            clean(&f.up.content_type)
        };
        section
            .push_str(&format!("\n<upload id=\"{}\" name=\"{name}\" kind=\"{kind}\" type=\"{ty}\">", clean(&f.up.id)));
        match kind {
            "text" => match &f.text {
                Some(text) => {
                    let all = text.chars().count();
                    let mut text = if all > MOST_CHARS {
                        let cut: String = text.chars().take(MOST_CHARS).collect();
                        format!("{cut}\n… (cut: {all} characters in all)")
                    } else {
                        text.clone()
                    };
                    // Its words cannot close the block, nor the list, early.
                    text = text.replace("</upload", "<\\/upload").replace("<upload", "<\\upload");
                    section.push_str(&format!("\n{text}\n</upload>"));
                }
                None => section.push_str("(its text could not be read from Arena)</upload>"),
            },
            "image" if sees => match &f.picture {
                Some(p) => {
                    images.push(p.clone());
                    section.push_str("(the picture is shown with this message)</upload>");
                }
                None => section.push_str("(a picture that could not be read from Arena)</upload>"),
            },
            "image" => section.push_str(
                "(a picture: this model cannot see pictures, ask the person what it shows if it matters)</upload>",
            ),
            other => {
                let what = match other {
                    "audio" => "a sound",
                    "video" => "a video",
                    _ => "a file",
                };
                section.push_str(&format!("({what}: not read here)</upload>"));
            }
        }
    }
    section.push_str(END);
    ca_agent::Attached { section, images }
}

/// A size as the box says it: "20 MB", "512 KB".
pub fn size_text(n: u64) -> String {
    const K: u64 = 1024;
    match n {
        n if n >= K * K && n % (K * K) == 0 => format!("{} MB", n / K / K),
        n if n >= K * K => format!("{:.1} MB", n as f64 / (K * K) as f64),
        n if n >= K => format!("{} KB", n / K),
        n => format!("{n} bytes"),
    }
}

/// A file's type by its name, for Arena (which reads the bytes as well).
pub fn mime_of(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "txt" | "log" | "rs" | "py" | "js" | "ts" | "tsx" | "cs" | "go" | "java" | "c" | "h" | "cpp" | "toml"
        | "yaml" | "yml" | "xml" | "sh" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// A picture's small copy as a PNG data URL; none for what is not a picture this build reads.
pub fn thumb(bytes: &[u8]) -> Option<String> {
    let picture = image::load_from_memory(bytes).ok()?.thumbnail(THUMB, THUMB);
    let mut png = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png.into_inner())))
}

/// One file sent up, saying how far it is, then read back for the model.
async fn upload(sh: Shared, m: Value) {
    let (key, out) = (m["key"].clone(), sh.out.clone());
    let name = m["from"]
        .as_str()
        .and_then(|f| std::path::Path::new(f).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .or_else(|| m["name"].as_str().filter(|n| !n.is_empty()).map(str::to_string))
        .unwrap_or_else(|| "pasted".to_string());
    let failed = |e: String| send(&out, json!({"type": "attached_file", "key": key, "name": name, "error": e}));
    let Some(arena) = sh.arena.clone() else {
        return failed("Attaching files needs Arena: sign in to it first.".into());
    };
    let files = sh.kept.of::<Files>();
    let bytes = match (m["from"].as_str(), m["data"].as_str()) {
        (Some(from), _) => match tokio::fs::read(from).await {
            Ok(b) => b,
            Err(e) => return failed(format!("{name} could not be read: {e}")),
        },
        (None, Some(data)) => match base64::engine::general_purpose::STANDARD.decode(data) {
            Ok(b) => b,
            Err(_) => return failed("What was pasted could not be read.".into()),
        },
        (None, None) => return failed("There is no file to attach.".into()),
    };
    if bytes.len() as u64 > files.most() {
        return failed(format!("Larger than {}.", size_text(files.most())));
    }
    let mime = m["mime"].as_str().filter(|t| !t.is_empty()).unwrap_or_else(|| mime_of(&name)).to_string();
    let said = Arc::new(AtomicU64::new(0));
    let (k, n, o) = (key.clone(), name.clone(), out.clone());
    let going = move |sent: u64, of: u64| {
        let pct = (sent * 100).checked_div(of).unwrap_or(0).min(99);
        if said.swap(pct, Ordering::Relaxed) != pct {
            send(&o, json!({"type": "attach_progress", "key": k, "name": n, "pct": pct}));
        }
    };
    let up = match arena.upload(&name, &mime, &bytes, going).await {
        Ok(up) => up,
        Err(e) => return failed(e.to_string()),
    };
    let mut kept = Kept { up, ..Default::default() };
    match kept.up.kind.as_str() {
        "text" => {
            let most = (MOST_CHARS * 4) as u64;
            kept.text = arena
                .attachment_content(&kept.up.id, most)
                .await
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned());
        }
        "image" => {
            let ty = if kept.up.content_type.is_empty() { mime.clone() } else { kept.up.content_type.clone() };
            kept.picture = arena
                .attachment_content(&kept.up.id, MOST_PICTURE_BYTES)
                .await
                .ok()
                .map(|b| format!("data:{ty};base64,{}", base64::engine::general_purpose::STANDARD.encode(b)));
            kept.thumb = tokio::task::spawn_blocking(move || thumb(&bytes)).await.ok().flatten().unwrap_or_default();
        }
        _ => {}
    }
    let mut reply = shown(&kept);
    reply["type"] = json!("attached_file");
    reply["key"] = key.clone();
    reply["content_type"] = json!(kept.up.content_type);
    if kept.up.file_name.is_empty() {
        reply["name"] = json!(name);
    }
    files.keep(kept);
    send(&out, reply);
}

/// The system's own dialog for choosing files: the paths chosen (none when it was cancelled).
fn pick() -> Result<Vec<String>, String> {
    use std::process::Command;
    let lines = |o: std::process::Output| -> Vec<String> {
        if !o.status.success() {
            return vec![];
        }
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    };
    let tries: Vec<Command> = if cfg!(target_os = "macos") {
        let mut c = Command::new("osascript");
        c.args([
            "-e",
            "set fs to choose file with prompt \"Attach Files\" with multiple selections allowed",
            "-e",
            "set out to \"\"",
            "-e",
            "repeat with f in fs\nset out to out & POSIX path of f & linefeed\nend repeat",
            "-e",
            "out",
        ]);
        vec![c]
    } else if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.args([
            "-NoProfile",
            "-Command",
            "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; \
             $d.Multiselect = $true; $d.Title = 'Attach Files'; if ($d.ShowDialog() -eq 'OK') { $d.FileNames }",
        ]);
        vec![c]
    } else {
        let mut z = Command::new("zenity");
        z.args(["--file-selection", "--multiple", "--separator=\n", "--title=Attach Files"]);
        let mut k = Command::new("kdialog");
        k.args(["--getopenfilename", ".", "--multiple", "--separate-output", "--title", "Attach Files"]);
        vec![z, k]
    };
    for mut c in tries {
        match c.output() {
            Ok(o) => return Ok(lines(o)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("The file dialog could not be opened: {e}")),
        }
    }
    Err("No file dialog was found on this system (zenity or kdialog): drop files on the chat's box instead.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: &str, name: &str, kind: &str) -> Kept {
        Kept {
            up: Uploaded {
                id: id.into(),
                file_name: name.into(),
                size: 10,
                kind: kind.into(),
                content_type: mime_of(name).into(),
                truncated: false,
            },
            ..Default::default()
        }
    }

    #[test]
    fn the_model_reads_a_documents_text_and_sees_a_picture_only_when_it_can() {
        let mut doc = file("d1", "notes.md", "text");
        doc.text = Some("# Notes\n<upload id=\"x\"> and </uploaded>".into());
        let mut pic = file("p1", "shot.png", "image");
        pic.picture = Some("data:image/png;base64,AAAA".into());
        let sound = file("s1", "a \"b\".mp3", "audio");
        let seen = read(&[doc.clone(), pic.clone(), sound.clone()], true);
        assert!(seen.section.starts_with(MARKER) && seen.section.ends_with(END), "{}", seen.section);
        assert!(
            seen.section.contains("<upload id=\"d1\" name=\"notes.md\" kind=\"text\" type=\"text/markdown\">\n# Notes")
        );
        assert!(seen.section.contains("<\\upload id=\"x\"> and <\\/uploaded>"), "its words cannot close the block");
        assert!(seen.section.contains("(the picture is shown with this message)"));
        assert!(
            seen.section.contains("name=\"a _b_.mp3\" kind=\"audio\" type=\"audio/mpeg\">(a sound: not read here)")
        );
        assert_eq!(seen.images, vec!["data:image/png;base64,AAAA".to_string()]);
        let blind = read(&[pic], false);
        assert!(blind.images.is_empty());
        assert!(blind.section.contains("this model cannot see pictures"));
        let unread = read(&[file("d2", "a.pdf", "text")], true);
        assert!(unread.section.contains("(its text could not be read from Arena)</upload>"));
    }

    #[test]
    fn a_long_document_is_cut_and_says_how_long_it_was() {
        let mut doc = file("d1", "big.txt", "text");
        doc.text = Some("é".repeat(MOST_CHARS + 5));
        let r = read(&[doc], false);
        assert!(r.section.contains(&format!("… (cut: {} characters in all)", MOST_CHARS + 5)));
    }

    #[test]
    fn sizes_types_and_small_pictures() {
        assert_eq!(size_text(20 * 1024 * 1024), "20 MB");
        assert_eq!(size_text(1536 * 1024), "1.5 MB");
        assert_eq!(size_text(2048), "2 KB");
        assert_eq!(mime_of("A.PNG"), "image/png");
        assert_eq!(mime_of("noext"), "application/octet-stream");
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(400, 200).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let t = thumb(&png.into_inner()).expect("a thumb");
        let bytes =
            base64::engine::general_purpose::STANDARD.decode(t.trim_start_matches("data:image/png;base64,")).unwrap();
        let small = image::load_from_memory(&bytes).unwrap();
        assert_eq!((small.width(), small.height()), (128, 64));
        assert!(thumb(b"not a picture").is_none());
    }

    #[test]
    fn a_face_is_given_each_file_with_its_thumb_only_when_it_has_one() {
        let mut pic = file("p1", "shot.png", "image");
        assert!(shown(&pic).get("thumb").is_none());
        pic.thumb = "data:image/png;base64,AA".into();
        assert_eq!(
            shown(&pic),
            json!({"id": "p1", "name": "shot.png", "size": 10, "kind": "image", "cut": false, "thumb": "data:image/png;base64,AA"})
        );
    }
}
