//! The files an answer cites, and where its links lead. A citation is a path of the project written as an answer
//! writes one: `src/app.ts:12`, `src/app.ts:12-30`, `src/app.ts:12:5`, `src/app.ts#L12`, `Program.cs(44,13)` or the
//! path alone, in inline code, in a link (`[app](src/app.ts#L3)`) or in the running text. Only a path that is one of
//! the project's files (as the session lists them) is a link: anything else the model wrote stays text. A link to
//! the web opens in the system's browser, a file in the editor at its line.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;
use std::sync::LazyLock;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use regex::Regex;
use serde_json::{Value, json};

use crate::state::{Send, Ui};

/// A place in a file an answer cites: the file's path in the project, its first line and last (from 1), its column.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Ref {
    pub path: String,
    pub line: Option<usize>,
    pub end: Option<usize>,
    pub column: Option<usize>,
}

/// Where a link leads.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// An address the system's browser (or mail program) opens.
    Web(String),
    /// A file of the project, at its place.
    File(Ref),
}

/// The forms a citation takes: `path#L12-L30`, `path(12,5)`, `path:12-30` or `path:12:5`, the path alone.
static FORMS: LazyLock<[Regex; 4]> = LazyLock::new(|| {
    let path = r#"(?P<path>[^\s:#()'"`]+?)"#;
    [
        format!(r"^{path}#L(?P<line>\d+)(?:-L?(?P<end>\d+))?$"),
        format!(r"^{path}\((?P<line>\d+)(?:,(?P<column>\d+))?\)$"),
        format!(r"^{path}:(?P<line>\d+)(?:-(?P<end>\d+)|:(?P<column>\d+))?:?$"),
        format!(r"^{path}$"),
    ]
    .map(|f| Regex::new(&f).expect("a citation's form"))
});

/// A citation in running text: a path with a folder in it or an extension, with its lines or not. What stands on
/// either side of it is looked at by [`find_refs`] (no letter, slash or dot is part of the word it is in).
static IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:[\w.-]+[/\\])+[\w.-]+(?::\d+(?:-\d+|:\d+)?|#L\d+(?:-L?\d+)?|\(\d+(?:,\d+)?\))?|[\w-][\w.-]*\.[A-Za-z][A-Za-z0-9]{0,7}(?::\d+(?:-\d+|:\d+)?|#L\d+(?:-L?\d+)?|\(\d+(?:,\d+)?\))?",
    )
    .expect("a citation in text")
});

/// A citation of a file, read from what is written (`src/app.ts:12`, a path); none when it is not one (an address).
pub fn parse_ref(text: &str) -> Option<Ref> {
    let t = text.trim();
    if t.is_empty() || t.len() > 400 || t.contains("://") {
        return None;
    }
    FORMS.iter().find_map(|form| {
        let g = form.captures(t)?;
        let number = |name: &str| g.name(name).and_then(|m| m.as_str().parse::<usize>().ok()).filter(|n| *n > 0);
        let line = number("line");
        Some(Ref {
            path: g.name("path")?.as_str().to_string(),
            line,
            end: number("end").filter(|e| line.is_some_and(|l| *e >= l)),
            column: number("column").filter(|_| line.is_some()),
        })
    })
}

/// The path as the project lists it (`./` and `\` taken off), when it is one of its files.
pub fn known(files: &HashSet<String>, path: &str) -> Option<String> {
    let p = path.trim().replace('\\', "/");
    let p = p.trim_start_matches("./");
    files.contains(p).then(|| p.to_string())
}

/// The citations of the project's files in a text: where each is, and the place it cites.
pub fn find_refs(text: &str, files: &HashSet<String>) -> Vec<(Range<usize>, Ref)> {
    if files.is_empty() {
        return Vec::new();
    }
    let part_of_word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '/' | '\\');
    IN_TEXT
        .find_iter(text)
        .filter_map(|m| {
            let before = text[..m.start()].chars().next_back();
            if before.is_some_and(|c| part_of_word(c) || matches!(c, '.' | '@' | '-')) {
                return None;
            }
            // The sentence's own full stop is not the file's.
            let found = m.as_str().trim_end_matches('.');
            let end = m.start() + found.len();
            let mut after = text[end..].chars().skip_while(|c| *c == '.');
            if text[end..].starts_with(|c| part_of_word(c)) || after.next().is_some_and(part_of_word) {
                return None;
            }
            let mut r = parse_ref(found)?;
            r.path = known(files, &r.path)?;
            Some((m.start()..end, r))
        })
        .collect()
}

/// Where a link's address leads: the web for an address with a scheme the browser opens, a file of the project for
/// a path that is one (`%20` read as a space), nowhere for anything else (`#a-heading`, `javascript:`, a path that
/// is not the project's).
pub fn target(href: &str, files: &HashSet<String>) -> Option<Target> {
    let h = href.trim();
    let lower = h.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
        return Some(Target::Web(h.to_string()));
    }
    if h.starts_with('#') || h.starts_with('/') || lower.split_once(':').is_some_and(|(s, _)| scheme(s)) {
        return None;
    }
    let h = unescape(h);
    // A link's path may hold a space (`docs/a%20b.md`), a citation's never does.
    if let Some(path) = known(files, &h) {
        return Some(Target::File(Ref { path, ..Ref::default() }));
    }
    let mut r = parse_ref(&h)?;
    r.path = known(files, &r.path)?;
    Some(Target::File(r))
}

/// The scheme of an address (`file`, `vscode`), not a path's drive or a line (`C`, `src/a.rs`).
fn scheme(s: &str) -> bool {
    s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// An address's `%XX` read as the bytes they stand for.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        match hex {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// What this part keeps for a window: the project's files as a set (looked up for each citation), and whether they
/// have been asked for.
#[derive(Clone, Copy)]
pub struct Citations {
    pub files: RwSignal<Rc<HashSet<String>>>,
}

impl crate::ext::State for Citations {
    fn new(ui: Ui) -> Self {
        Citations { files: ui.scope.create_rw_signal(Rc::new(HashSet::new())) }
    }
}

/// The project's files, to know a citation by (read where it is drawn, so a citation becomes a link once they come).
pub fn files(ui: Ui) -> Rc<HashSet<String>> {
    ui.state::<Citations>().files.get()
}

thread_local! {
    /// The session's ear, for what an answer's text does when it is clicked (it is drawn without one).
    static SEND: RefCell<Option<Send>> = const { RefCell::new(None) };
}

/// What a link leads to, opened: a file in the editor at its line, an address in the system's browser.
pub fn open(ui: Ui, to: &Target) {
    match to {
        Target::File(r) => {
            let Some(send) = SEND.with(|s| s.borrow().clone()) else { return };
            match r.line {
                Some(line) => ui.open_at(&r.path, line, &send),
                None => ui.open(&r.path, &send),
            }
        }
        Target::Web(address) => browse(address),
    }
}

/// A code block saved as a file of the project (the session says it was, as for any save).
pub fn save(path: &str, text: &str) {
    if let Some(send) = SEND.with(|s| s.borrow().clone()) {
        let path = path.trim().replace('\\', "/");
        send(json!({"type": "save", "path": path.trim_start_matches("./"), "text": text}));
    }
}

/// An address given to the system to open (off the window's thread: the program it starts is waited for there).
fn browse(address: &str) {
    if cfg!(test) {
        return;
    }
    let address = address.to_string();
    std::thread::spawn(move || {
        let mut command = if cfg!(target_os = "windows") {
            let mut c = std::process::Command::new("explorer");
            c.arg(&address);
            c
        } else {
            let mut c = std::process::Command::new(if cfg!(target_os = "macos") { "open" } else { "xdg-open" });
            c.arg(&address);
            c
        };
        let _ = command.stdin(std::process::Stdio::null()).status();
    });
}

/// The session's ear kept, for an answer's links.
pub fn register(_ui: Ui, send: &Send) {
    SEND.with(|s| *s.borrow_mut() = Some(send.clone()));
}

/// The project's files, read from the session's list of them, and asked for once the session is there and again
/// at each turn's end (the agent may have made the files it cites). Never taken away from the rest of the window.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "files" => {
            let paths = || m["paths"].as_array().into_iter().flatten().filter_map(Value::as_str);
            let files = ui.state::<Citations>().files;
            let same = files.with_untracked(|f| f.len() == paths().count() && paths().all(|p| f.contains(p)));
            if !same {
                files.set(Rc::new(paths().map(str::to_string).collect()));
            }
        }
        "turn_end" => send(json!({"type": "files"})),
        "state" if ui.state::<Citations>().files.with_untracked(|f| f.is_empty()) => send(json!({"type": "files"})),
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> HashSet<String> {
        ["src/app.ts", "Program.cs", "README.md", "docs/a b.md"].map(String::from).into()
    }

    fn r(path: &str, line: Option<usize>, end: Option<usize>, column: Option<usize>) -> Ref {
        Ref { path: path.into(), line, end, column }
    }

    #[test]
    fn a_citation_is_read_in_each_of_its_forms() {
        assert_eq!(parse_ref("src/app.ts:12"), Some(r("src/app.ts", Some(12), None, None)));
        assert_eq!(parse_ref("src/app.ts:12-30"), Some(r("src/app.ts", Some(12), Some(30), None)));
        assert_eq!(parse_ref("src/app.ts:12:5"), Some(r("src/app.ts", Some(12), None, Some(5))));
        assert_eq!(parse_ref("src/app.ts#L12"), Some(r("src/app.ts", Some(12), None, None)));
        assert_eq!(parse_ref("src/app.ts#L12-L30"), Some(r("src/app.ts", Some(12), Some(30), None)));
        assert_eq!(parse_ref("Program.cs(44,13)"), Some(r("Program.cs", Some(44), None, Some(13))));
        assert_eq!(parse_ref("Program.cs(44)"), Some(r("Program.cs", Some(44), None, None)));
        assert_eq!(parse_ref(" README.md "), Some(r("README.md", None, None, None)));
        // A range that runs backwards is its first line only; an address is not a file.
        assert_eq!(parse_ref("a.rs:30-12"), Some(r("a.rs", Some(30), None, None)));
        assert_eq!(parse_ref("https://x.dev/a.ts"), None);
        assert_eq!(parse_ref("let x = 1;"), None);
    }

    #[test]
    fn only_the_projects_own_files_are_cited() {
        let f = files();
        assert_eq!(known(&f, "./src/app.ts").as_deref(), Some("src/app.ts"));
        assert_eq!(known(&f, "src\\app.ts").as_deref(), Some("src/app.ts"));
        assert_eq!(known(&f, "src/other.ts"), None);
    }

    #[test]
    fn citations_are_found_in_running_text_and_not_inside_other_words() {
        let f = files();
        let text = "See src/app.ts:12-30, then Program.cs(44,13) and README.md. Not x/src/app.ts nor src/app.tsx, \
                    nor a@README.md or src/nope.ts.";
        let found: Vec<(&str, Ref)> = find_refs(text, &f).into_iter().map(|(at, r)| (&text[at], r)).collect();
        assert_eq!(
            found,
            [
                ("src/app.ts:12-30", r("src/app.ts", Some(12), Some(30), None)),
                ("Program.cs(44,13)", r("Program.cs", Some(44), None, Some(13))),
                ("README.md", r("README.md", None, None, None)),
            ]
        );
        assert!(find_refs(text, &HashSet::new()).is_empty());
    }

    #[test]
    fn a_link_leads_to_the_web_to_a_file_of_the_project_or_nowhere() {
        let f = files();
        assert_eq!(target("https://x.dev/a", &f), Some(Target::Web("https://x.dev/a".into())));
        assert_eq!(target("mailto:a@b.c", &f), Some(Target::Web("mailto:a@b.c".into())));
        assert_eq!(target("./src/app.ts#L3", &f), Some(Target::File(r("src/app.ts", Some(3), None, None))));
        assert_eq!(target("docs/a%20b.md", &f), Some(Target::File(r("docs/a b.md", None, None, None))));
        assert_eq!(target("#usage", &f), None);
        assert_eq!(target("javascript:alert(1)", &f), None);
        assert_eq!(target("file:///etc/passwd", &f), None);
        assert_eq!(target("/etc/passwd", &f), None);
        assert_eq!(target("src/nope.ts", &f), None);
    }

    #[test]
    fn the_files_listed_are_kept_and_asked_for_at_a_turns_end() {
        let ui = Ui::new();
        let said = Rc::new(RefCell::new(Vec::new()));
        let to = said.clone();
        let send: Send = Rc::new(move |v: Value| to.borrow_mut().push(v));
        take(ui, &json!({"type": "state"}), &send);
        assert_eq!(*said.borrow(), [json!({"type": "files"})]);
        take(ui, &json!({"type": "files", "paths": ["a.rs", "b/c.rs"]}), &send);
        let first = super::files(ui);
        assert!(first.contains("b/c.rs") && first.len() == 2);
        // Listed again as they were: the set held stays the same one (nothing drawn from it is made again).
        take(ui, &json!({"type": "files", "paths": ["b/c.rs", "a.rs"]}), &send);
        assert!(Rc::ptr_eq(&first, &super::files(ui)));
        take(ui, &json!({"type": "state"}), &send);
        take(ui, &json!({"type": "turn_end"}), &send);
        assert_eq!(said.borrow().len(), 2, "asked again at the turn's end, not at each state");
    }
}
