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
//! - `diagram{id, kind, source, dark}` → `diagram{id, svg}`: a Mermaid diagram (`kind` "mermaid") drawn as SVG in
//!   the window's theme, by mermaid-cli (`mmdc`, the project's own or on `PATH`), else by a headless Chrome or
//!   Chromium with a local copy of mermaid.js (the project's `node_modules`, or the global one). It is drawn on a
//!   thread of its own and answered when done; `diagram{id, error}` says what failed (Mermaid's own words for a
//!   diagram that does not parse), `diagram{id, error, missing: true}` that nothing here draws diagrams.
//! - `diagram_open{id, kind, source, dark}` → `diagram_open{id}` once the system's browser was started on a page
//!   that draws it (with the local mermaid.js, else Mermaid from its CDN), or `diagram_open{id, error}`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, send};
use crate::ide_files::{Side, modified};

/// The most bytes a file's `media` reply carries.
const MOST_BYTES: u64 = 50 * 1024 * 1024;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] =
        &[("media", Rule::Side), ("media_open", Rule::Side), ("diagram", Rule::Side), ("diagram_open", Rule::Side)];

    fn side(side: &Side, m: &Value) -> bool {
        let path = m["path"].as_str().unwrap_or("");
        let reply = match m["type"].as_str().unwrap_or("") {
            "media" => media(side, path, m["data"] != false),
            "diagram" => {
                let (out, root) = (side.out().clone(), side.workspace().root().to_path_buf());
                let asked = Asked::of(m);
                let path = std::env::var_os("PATH");
                std::thread::spawn(move || send(&out, asked.drawn(&root, path.as_deref(), DRAW_TIMEOUT)));
                return true;
            }
            "diagram_open" => {
                let asked = Asked::of(m);
                let path = std::env::var_os("PATH");
                match asked.page(side.workspace().root(), path.as_deref()).and_then(|page| system_open(&page)) {
                    Ok(()) => json!({"type": "diagram_open", "id": asked.id}),
                    Err(e) => json!({"type": "diagram_open", "id": asked.id, "error": e}),
                }
            }
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

/// The longest diagram source drawn.
const MOST_SOURCE: usize = 64 * 1024;
/// How long a diagram is given to be drawn before its drawer is stopped.
const DRAW_TIMEOUT: Duration = Duration::from_secs(30);
/// Mermaid from its CDN, for a page the browser opens when there is no local copy.
const MERMAID_CDN: &str = "https://cdn.jsdelivr.net/npm/mermaid@10.9.1/dist/mermaid.min.js";

/// A diagram asked for: the key its reply carries, its kind, its source, and whether the window is dark.
struct Asked {
    id: Value,
    kind: String,
    source: String,
    dark: bool,
}

/// What draws a diagram here.
#[derive(Debug, PartialEq)]
enum Drawer {
    /// mermaid-cli.
    Mmdc(PathBuf),
    /// A headless Chrome or Chromium, with a local mermaid.js.
    Chrome { chrome: PathBuf, mermaid: PathBuf },
}

/// A program by its name in one of `PATH`'s folders (with Windows' endings there).
fn on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let endings: &[&str] = if cfg!(windows) { &[".cmd", ".exe", ""] } else { &[""] };
    std::env::split_paths(path?)
        .flat_map(|d| endings.iter().map(move |e| d.join(format!("{name}{e}"))))
        .find(|c| c.is_file())
}

/// The local mermaid.js: the project's own, else the one npm keeps globally (beside a program on `PATH`), or the
/// one mermaid-cli carries.
fn mermaid_js(root: &Path, path: Option<&OsStr>) -> Option<PathBuf> {
    let dist = |modules: &Path| {
        [
            modules.join("mermaid/dist/mermaid.min.js"),
            modules.join("@mermaid-js/mermaid-cli/node_modules/mermaid/dist/mermaid.min.js"),
        ]
    };
    let mut places = dist(&root.join("node_modules")).to_vec();
    for dir in path.map(|p| std::env::split_paths(p).collect::<Vec<_>>()).unwrap_or_default() {
        if let Some(prefix) = dir.parent() {
            places.extend(dist(&prefix.join("lib/node_modules")));
            places.extend(dist(&prefix.join("node_modules")));
        }
    }
    places.into_iter().find(|p| p.is_file())
}

/// What draws diagrams here: mermaid-cli (the project's, else on `PATH`), else a Chrome with a local mermaid.js.
fn drawer(root: &Path, path: Option<&OsStr>) -> Option<Drawer> {
    let own = root.join("node_modules/.bin/mmdc");
    if own.is_file() {
        return Some(Drawer::Mmdc(own));
    }
    if let Some(mmdc) = on_path("mmdc", path) {
        return Some(Drawer::Mmdc(mmdc));
    }
    let chrome = ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"]
        .into_iter()
        .find_map(|name| on_path(name, path))?;
    Some(Drawer::Chrome { chrome, mermaid: mermaid_js(root, path)? })
}

/// A folder of its own for one drawing, gone when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Scratch, String> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("code-arena-diagram-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("a folder for it could not be made: {e}"))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A program run to its end in the folder, its output and errors into files there; stopped when it takes too long.
/// Its output, or what it said of its failure.
fn run(mut cmd: std::process::Command, dir: &Path, timeout: Duration) -> Result<String, String> {
    let file = |name: &str| std::fs::File::create(dir.join(name)).map_err(|e| e.to_string());
    cmd.stdin(std::process::Stdio::null()).stdout(file("stdout")?).stderr(file("stderr")?).current_dir(dir);
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd.spawn().map_err(|e| format!("{program} could not be started: {e}"))?;
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("it took longer than {} seconds", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
    if status.success() { Ok(read("stdout")) } else { Err(said(&read("stderr"), &status.to_string())) }
}

/// What a failing program said, shortly: its last lines that are not a stack's frames.
fn said(stderr: &str, otherwise: &str) -> String {
    let lines: Vec<&str> =
        stderr.lines().map(str::trim_end).filter(|l| !l.is_empty() && !l.trim_start().starts_with("at ")).collect();
    let kept = lines[lines.len().saturating_sub(8)..].join("\n");
    if kept.is_empty() { otherwise.to_string() } else { kept.chars().take(1500).collect() }
}

impl Asked {
    fn of(m: &Value) -> Asked {
        Asked {
            id: m["id"].clone(),
            kind: m["kind"].as_str().unwrap_or("mermaid").to_string(),
            source: m["source"].as_str().unwrap_or("").to_string(),
            dark: m["dark"].as_bool().unwrap_or(true),
        }
    }

    fn theme(&self) -> &'static str {
        if self.dark { "dark" } else { "default" }
    }

    /// Mermaid's settings: the theme, and labels as SVG text (the window does not draw an HTML label).
    fn config(&self) -> Value {
        json!({
            "startOnLoad": false,
            "theme": self.theme(),
            "securityLevel": "strict",
            "htmlLabels": false,
            "flowchart": {"htmlLabels": false},
        })
    }

    /// The reply: the diagram drawn, or why not.
    fn drawn(&self, root: &Path, path: Option<&OsStr>, timeout: Duration) -> Value {
        let reply = |field: &str, v: Value| json!({"type": "diagram", "id": self.id, field: v});
        if self.kind != "mermaid" {
            return reply("error", json!(format!("A {} diagram is not drawn here.", self.kind)));
        }
        if self.source.trim().is_empty() || self.source.len() > MOST_SOURCE {
            return reply("error", json!("The diagram is empty, or longer than 64 KB."));
        }
        let Some(drawer) = drawer(root, path) else {
            let mut r = reply(
                "error",
                json!("Nothing here draws diagrams: install mermaid-cli (npm install -g @mermaid-js/mermaid-cli)."),
            );
            r["missing"] = json!(true);
            return r;
        };
        match self.draw(&drawer, timeout) {
            Ok(svg) => reply("svg", json!(svg)),
            Err(e) => reply("error", json!(e)),
        }
    }

    fn draw(&self, drawer: &Drawer, timeout: Duration) -> Result<String, String> {
        let dir = Scratch::new()?;
        let write = |name: &str, text: &str| std::fs::write(dir.0.join(name), text).map_err(|e| e.to_string());
        match drawer {
            Drawer::Mmdc(mmdc) => {
                write("in.mmd", &self.source)?;
                write("config.json", &self.config().to_string())?;
                let mut cmd = std::process::Command::new(mmdc);
                cmd.args(["-i", "in.mmd", "-o", "out.svg", "-t", self.theme(), "-b", "transparent"]);
                cmd.args(["-c", "config.json", "-q"]);
                run(cmd, &dir.0, timeout)?;
                std::fs::read_to_string(dir.0.join("out.svg")).map_err(|_| "mmdc drew nothing".to_string())
            }
            Drawer::Chrome { chrome, mermaid } => {
                write("page.html", &self.page_html(&file_url(mermaid), true))?;
                let mut cmd = std::process::Command::new(chrome);
                cmd.args(["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check"]);
                cmd.args(["--allow-file-access-from-files", "--virtual-time-budget=15000", "--dump-dom"]);
                cmd.arg(format!("--user-data-dir={}", dir.0.join("profile").display()));
                cmd.arg(file_url(&dir.0.join("page.html")));
                dumped(&run(cmd, &dir.0, timeout)?)
            }
        }
    }

    /// A page that draws the diagram with the mermaid.js at this address. For a headless Chrome to print, into its
    /// body's text as `SVG:` and the SVG's base64 (`ERR:` and the error's); else on the page, for a person.
    fn page_html(&self, mermaid: &str, dump: bool) -> String {
        // In a script, so nothing in the source closes it.
        let source = serde_json::to_string(&self.source).unwrap_or_default().replace("</", "<\\/");
        let config = self.config().to_string();
        let b64 = "(s => btoa(unescape(encodeURIComponent(s))))";
        let error = "String(e && e.message || e)";
        let (ok, fail) = if dump {
            (
                format!("document.body.textContent = 'SVG:' + {b64}(svg)"),
                format!("document.body.textContent = 'ERR:' + {b64}({error})"),
            )
        } else {
            ("document.body.innerHTML = svg".to_string(), format!("document.body.textContent = {error}"))
        };
        let (background, color) = if self.dark { ("#1e1e1e", "#cccccc") } else { ("#ffffff", "#3b3b3b") };
        format!(
            "<!doctype html><meta charset=\"utf-8\"><title>Diagram</title>\
             <style>body{{margin:24px;background:{background};color:{color};font-family:sans-serif}}</style>\
             <body><script src=\"{mermaid}\"></script><script>(async () => {{ try {{ mermaid.initialize({config}); \
             const {{ svg }} = await mermaid.render('diagram', {source}); {ok} }} catch (e) {{ {fail} }} }})()\
             </script>"
        )
    }

    /// A page of its own the browser opens, drawing the diagram.
    fn page(&self, root: &Path, path: Option<&OsStr>) -> Result<PathBuf, String> {
        if self.source.len() > MOST_SOURCE {
            return Err("The diagram is longer than 64 KB.".into());
        }
        let mermaid = mermaid_js(root, path).map(|p| file_url(&p)).unwrap_or_else(|| MERMAID_CDN.to_string());
        let dir = std::env::temp_dir().join("code-arena-diagrams");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&(&self.source, self.dark), &mut h);
        let page = dir.join(format!("{:016x}.html", std::hash::Hasher::finish(&h)));
        std::fs::write(&page, self.page_html(&mermaid, false)).map_err(|e| e.to_string())?;
        Ok(page)
    }
}

/// A file's address for a browser.
fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.replace('%', "%25").replace(' ', "%20").replace('#', "%23").replace('?', "%3F");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

/// The diagram a headless Chrome printed of the page: its SVG, or Mermaid's error.
fn dumped(dom: &str) -> Result<String, String> {
    let text = |at: usize| {
        let b64: String =
            dom[at + 10..].chars().take_while(|c| c.is_ascii_alphanumeric() || "+/=".contains(*c)).collect();
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or_else(|| "Chrome's page could not be read".to_string())
    };
    if let Some(at) = dom.find("<body>SVG:") {
        return text(at);
    }
    match dom.find("<body>ERR:") {
        Some(at) => Err(text(at)?),
        None => Err("Chrome drew nothing (mermaid.js did not run)".into()),
    }
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
        assert_eq!(
            (got["path"].as_str(), got["mime"].as_str(), got["size"].as_u64()),
            (Some("img/a.PNG"), Some("image/png"), Some(6))
        );
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

    fn asked(source: &str) -> Asked {
        Asked { id: json!("d1"), kind: "mermaid".into(), source: source.into(), dark: true }
    }

    /// A program of this shell script in the folder, made runnable.
    #[cfg(unix)]
    fn program(dir: &Path, name: &str, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{script}\n")).expect("w");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("mode");
        p
    }

    #[test]
    fn mermaid_cli_is_found_before_chrome_and_chrome_only_with_a_local_mermaid() {
        let d = tempfile::tempdir().expect("temp");
        let (root, bin) = (d.path().join("project"), d.path().join("prefix/bin"));
        std::fs::create_dir_all(&root).expect("dir");
        std::fs::create_dir_all(&bin).expect("dir");
        let path = bin.clone().into_os_string();
        assert_eq!(drawer(&root, Some(&path)), None);
        std::fs::write(bin.join("chromium"), "").expect("w");
        assert_eq!(drawer(&root, Some(&path)), None, "Chrome alone has no mermaid.js");
        let global = d.path().join("prefix/lib/node_modules/mermaid/dist");
        std::fs::create_dir_all(&global).expect("dir");
        std::fs::write(global.join("mermaid.min.js"), "").expect("w");
        assert_eq!(
            drawer(&root, Some(&path)),
            Some(Drawer::Chrome { chrome: bin.join("chromium"), mermaid: global.join("mermaid.min.js") })
        );
        std::fs::write(bin.join("mmdc"), "").expect("w");
        assert_eq!(drawer(&root, Some(&path)), Some(Drawer::Mmdc(bin.join("mmdc"))));
        std::fs::create_dir_all(root.join("node_modules/.bin")).expect("dir");
        std::fs::write(root.join("node_modules/.bin/mmdc"), "").expect("w");
        assert_eq!(drawer(&root, Some(&path)), Some(Drawer::Mmdc(root.join("node_modules/.bin/mmdc"))));
    }

    #[test]
    fn nothing_to_draw_with_says_so_and_what_is_not_drawn_says_why() {
        let d = tempfile::tempdir().expect("temp");
        let got = asked("graph TD; A-->B").drawn(d.path(), None, DRAW_TIMEOUT);
        assert_eq!((got["id"].as_str(), got["missing"].as_bool()), (Some("d1"), Some(true)), "{got}");
        assert!(got["error"].as_str().is_some_and(|e| e.contains("mermaid-cli")));
        let got = asked("  ").drawn(d.path(), None, DRAW_TIMEOUT);
        assert_eq!((got["error"].is_string(), got.get("missing")), (true, None));
        let got = Asked { kind: "plantuml".into(), ..asked("a") }.drawn(d.path(), None, DRAW_TIMEOUT);
        assert!(got["error"].as_str().is_some_and(|e| e.contains("plantuml")), "{got}");
    }

    #[cfg(unix)]
    #[test]
    fn mermaid_cli_draws_into_its_file_and_its_error_and_its_slowness_are_told() {
        let d = tempfile::tempdir().expect("temp");
        let bin = d.path().join("bin");
        std::fs::create_dir_all(&bin).expect("dir");
        let path = bin.clone().into_os_string();
        // Writes the theme it was given and its input into the file after -o.
        let draws = r#"while [ $# -gt 0 ]; do case $1 in -o) o=$2; shift;; -t) t=$2; shift;; -i) i=$2; shift;; esac; shift; done
printf '<svg data-theme="%s">%s</svg>' "$t" "$(cat "$i")" > "$o""#;
        program(&bin, "mmdc", draws);
        let got = asked("graph TD; A-->B").drawn(d.path(), Some(&path), DRAW_TIMEOUT);
        assert_eq!(got["svg"], r#"<svg data-theme="dark">graph TD; A-->B</svg>"#, "{got}");
        program(&bin, "mmdc", "echo 'Error: Parse error on line 2:' >&2\necho '    at parse (x.js:1)' >&2\nexit 1");
        let got = asked("graph TD; A-->").drawn(d.path(), Some(&path), DRAW_TIMEOUT);
        assert_eq!(got["error"], "Error: Parse error on line 2:", "{got}");
        program(&bin, "mmdc", "sleep 5");
        let got = asked("graph TD; A-->B").drawn(d.path(), Some(&path), Duration::from_millis(300));
        assert!(got["error"].as_str().is_some_and(|e| e.contains("longer than")), "{got}");
    }

    #[test]
    fn chromes_page_is_read_for_its_svg_or_mermaids_error() {
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let page = |body: &str| format!("<html><head></head><body>{body}</body></html>");
        assert_eq!(dumped(&page(&format!("SVG:{}", b64("<svg/>")))), Ok("<svg/>".into()));
        assert_eq!(dumped(&page(&format!("ERR:{}", b64("Parse error")))), Err("Parse error".into()));
        // The page's own script, when mermaid.js did not run, is not read as a drawing.
        assert!(dumped(&asked("a").page_html("file:///none.js", true)).is_err());
        // Nothing in the source closes the script it is in.
        let html = asked("A[\"</script>\"]").page_html("m.js", false);
        assert_eq!(html.matches("</script>").count(), 2, "{html}");
    }
}
