//! The project's symbols by name (Go to Symbol in Workspace, Ctrl+T and "#" in Quick Open).
//!
//! `symbols{q, limit}` is answered with `symbols{q, items: [{name, kind, container, path, line, end}], building}`:
//! the declarations whose names match what was typed, best first. They come from a scan of the files git would
//! list (its ignores kept), each read by the declaration patterns of its language, kept for the folder and looked
//! at again when it is a few seconds old (only the files changed since are read again). A scan stops after three
//! seconds; what it found is answered with `building` until a later one ends. The scan is all behind [`find`], so a
//! persistent index can take its place under the same message.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use regex::Regex;
use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, send};
use crate::ide_files::Side;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[("symbols", Rule::Side)];

    fn side(side: &Side, m: &Value) -> bool {
        let q = m["q"].as_str().unwrap_or("").to_string();
        let limit = m["limit"].as_u64().map_or(LIMIT, |l| (l as usize).clamp(1, MOST));
        let index = side.kept.of::<Index>();
        let (root, out) = (side.workspace().root().to_path_buf(), side.out().clone());
        tokio::task::spawn_blocking(move || {
            let answer = |q: &str, limit: usize, index: &Index| {
                let (items, building) = find(index, q, limit);
                send(&out, json!({"type": "symbols", "q": q, "items": items, "building": building}));
            };
            // Begun (or kept for the scan's end) before the answer, so that one ending meanwhile still answers it.
            let scan = index.begin(&q, limit);
            answer(&q, limit, &index);
            if scan {
                index.scan(&root);
                if let Some((q, limit)) = index.end() {
                    answer(&q, limit, &index);
                }
            }
        });
        true
    }
}

/// How many are listed when the face does not say, and the most it may ask for.
const LIMIT: usize = 100;
const MOST: usize = 1000;
/// How long a scan may run, and how old one may be before the next question scans again.
const CAP: Duration = Duration::from_secs(3);
const FRESH: Duration = Duration::from_secs(5);
/// Files larger than this are not read (generated or minified ones, mostly).
const LARGEST: u64 = 1 << 20;

/// A declaration: its name, its kind (the Outline's words: `function`, `class`, ...), the declaration it is inside,
/// and its lines (from 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sym {
    pub name: String,
    pub kind: &'static str,
    pub container: String,
    pub line: usize,
    pub end: usize,
}

/// The symbols of the folder's files, as last scanned, and the scan under way.
#[derive(Default)]
pub struct Index {
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    /// By path: the file's change time and size when read, and what was found in it.
    files: HashMap<String, (u64, u64, Arc<Vec<Sym>>)>,
    scanned: Option<Instant>,
    /// The last scan read every file (it was not cut at the cap).
    whole: bool,
    scanning: bool,
    /// The newest question asked while a scan ran: answered again when it ends.
    asked: Option<(String, usize)>,
}

impl Index {
    fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a scan is to be begun now by the one asking (none runs, and the last is old or was cut). A question
    /// asked while one runs is kept for its end.
    fn begin(&self, q: &str, limit: usize) -> bool {
        let mut h = self.held();
        if h.scanning {
            h.asked = Some((q.to_string(), limit));
            return false;
        }
        let stale = !h.whole || h.scanned.is_none_or(|t| t.elapsed() > FRESH);
        if stale {
            h.scanning = true;
            h.asked = Some((q.to_string(), limit));
        }
        stale
    }

    /// The scan ended: the question to answer again.
    fn end(&self) -> Option<(String, usize)> {
        let mut h = self.held();
        h.scanning = false;
        h.asked.take()
    }

    /// The folder's files looked at, those changed since read again, for at most [`CAP`].
    fn scan(&self, root: &Path) {
        let started = Instant::now();
        let had: HashMap<String, (u64, u64, Arc<Vec<Sym>>)> = self.held().files.clone();
        let mut files = HashMap::new();
        let mut whole = true;
        for e in walker(root).flatten() {
            if started.elapsed() > CAP {
                whole = false;
                break;
            }
            if !e.file_type().is_some_and(|t| t.is_file()) || lang_of(e.path()).is_none() {
                continue;
            }
            let Ok(rel) = e.path().strip_prefix(root) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let Ok(meta) = e.metadata() else { continue };
            if meta.len() > LARGEST {
                continue;
            }
            let at = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            let at = at as u64;
            if let Some(was) = had.get(&rel).filter(|(t, n, _)| *t == at && *n == meta.len()) {
                files.insert(rel, was.clone());
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            files.insert(rel.clone(), (at, meta.len(), Arc::new(declarations(&rel, &text))));
        }
        let mut h = self.held();
        if !whole {
            // What was not reached this time is kept as it was.
            for (k, v) in had {
                files.entry(k).or_insert(v);
            }
        }
        h.files = files;
        h.whole = whole;
        h.scanned = Some(Instant::now());
    }
}

/// The folder walked as Quick Open's files are: git's ignores kept, `.git` left out.
fn walker(root: &Path) -> ignore::Walk {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
}

/// The symbols that match what was typed, best first, and whether more may come (a scan runs or was cut).
pub fn find(index: &Index, q: &str, limit: usize) -> (Vec<Value>, bool) {
    let h = index.held();
    let building = h.scanning || !h.whole;
    let all = h.files.iter().flat_map(|(path, (_, _, syms))| syms.iter().map(move |s| (path.as_str(), s)));
    let items = rank(all, q, limit)
        .into_iter()
        .map(|(path, s)| {
            json!({"name": s.name, "kind": s.kind, "container": s.container, "path": path, "line": s.line, "end": s.end})
        })
        .collect();
    (items, building)
}

/// How well a name matches what was typed (lower case): the whole name 100, its start 80, its initials 60
/// (`rta` for `RunToolsAsync`, `gfn` for `get_file_name`), inside it 40; none otherwise.
pub fn score(name: &str, q: &str) -> Option<u32> {
    if q.is_empty() {
        return None;
    }
    let lower = name.to_lowercase();
    if lower == q {
        return Some(100);
    }
    if lower.starts_with(q) {
        return Some(80);
    }
    if q.chars().count() > 1 && initials(name).starts_with(q) {
        return Some(60);
    }
    lower.contains(q).then_some(40)
}

/// A name's initials, lower case: its first letter, each capital after a small letter or a digit, and each letter
/// after `_`, `-`, `.` or `$`.
fn initials(name: &str) -> String {
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for c in name.chars() {
        let starts = match prev {
            None => c.is_alphanumeric(),
            Some(p) => {
                (c.is_uppercase() && (p.is_lowercase() || p.is_ascii_digit()))
                    || (c.is_alphanumeric() && matches!(p, '_' | '-' | '.' | '$' | ' '))
            }
        };
        if starts {
            out.extend(c.to_lowercase());
        }
        prev = Some(c);
    }
    out
}

/// Whether a path is a test's: under a tests folder, or named as a test file.
pub fn is_test(path: &str) -> bool {
    let lower = path.to_lowercase();
    let file = lower.rsplit('/').next().unwrap_or("");
    if lower
        .split('/')
        .rev()
        .skip(1)
        .any(|d| matches!(d, "test" | "tests" | "spec" | "specs" | "__tests__" | "testing"))
    {
        return true;
    }
    let stem = file.split('.').next().unwrap_or(file);
    let named = path.rsplit('/').next().unwrap_or(path);
    let named = named.split('.').next().unwrap_or(named);
    file.contains(".test.")
        || file.contains(".spec.")
        || matches!(stem, "test" | "tests")
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
        || stem.starts_with("test_")
        || named.ends_with("Test")
        || named.ends_with("Tests")
}

/// The best `limit` of the symbols for what was typed: by score, then code before tests, shorter paths, the name,
/// the line.
pub fn rank<'a>(all: impl Iterator<Item = (&'a str, &'a Sym)>, q: &str, limit: usize) -> Vec<(&'a str, &'a Sym)> {
    let q = q.trim().to_lowercase();
    let mut found: Vec<(u32, bool, &str, &Sym)> =
        all.filter_map(|(p, s)| score(&s.name, &q).map(|n| (n, is_test(p), p, s))).collect();
    found.sort_unstable_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.len().cmp(&b.2.len()))
            .then_with(|| a.3.name.cmp(&b.3.name))
            .then_with(|| a.2.cmp(b.2))
            .then(a.3.line.cmp(&b.3.line))
    });
    found.truncate(limit);
    found.into_iter().map(|(_, _, p, s)| (p, s)).collect()
}

/// The languages read, by the patterns their declarations are found by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lang {
    Rust,
    CSharp,
    Script,
    Python,
    Go,
    C,
    Ruby,
    Markdown,
}

fn lang_of(path: &Path) -> Option<Lang> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => Lang::Rust,
        "cs" | "java" | "kt" | "kts" | "scala" | "swift" | "dart" => Lang::CSharp,
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" | "vue" | "svelte" => Lang::Script,
        "py" | "pyi" => Lang::Python,
        "go" => Lang::Go,
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "m" => Lang::C,
        "rb" | "php" | "lua" | "sh" | "bash" => Lang::Ruby,
        "md" | "markdown" => Lang::Markdown,
        _ => return None,
    })
}

/// A declaration's pattern: the indent is its first group and the name its second; `member` ones count only inside
/// a class (where a call or a keyword would look the same at the top).
struct Pat {
    re: Regex,
    kind: &'static str,
    member: bool,
}

/// Kinds that hold others (their members are named with them as the container).
fn holds(kind: &str) -> bool {
    matches!(kind, "class" | "struct" | "interface" | "enum" | "module" | "impl")
}

/// Words that a call, a statement or a type looks like a declaration with.
const NOT_NAMES: &[&str] = &[
    "if", "for", "foreach", "while", "switch", "catch", "return", "new", "else", "using", "lock", "sizeof", "typeof",
    "nameof", "await", "base", "this", "super", "function", "fixed", "when", "with", "delete", "throw", "do", "in",
];

fn pats(lang: Lang) -> &'static [Pat] {
    static ALL: OnceLock<HashMap<u8, Vec<Pat>>> = OnceLock::new();
    let all = ALL.get_or_init(|| {
        let p = |re: &str, kind: &'static str, member: bool| Pat {
            re: Regex::new(re).unwrap_or_else(|e| panic!("pattern {re}: {e}")),
            kind,
            member,
        };
        let rust_vis = r"(?:pub(?:\([^)]*\))?\s+)?";
        let cs_mods = r"(?:(?:public|private|protected|internal|static|sealed|abstract|partial|readonly|ref|unsafe|new|file|virtual|override|async|extern|final|synchronized|native|open|data|inner|private\s+protected|default|strictfp|transient|volatile|const|fileprivate|mutating|required|convenience|@\w+(?:\([^)]*\))?)\s+)";
        let script_pre = r"(?:export\s+)?(?:default\s+)?(?:declare\s+)?";
        let mut m = HashMap::new();
        m.insert(
            Lang::Rust as u8,
            vec![
                p(&format!(r"^(\s*){rust_vis}(?:(?:const|async|unsafe|extern\s+(?:\x22[^\x22]*\x22\s+)?)\s*)*fn\s+([A-Za-z_]\w*)"), "function", false),
                p(&format!(r"^(\s*){rust_vis}(?:struct|union)\s+([A-Za-z_]\w*)"), "struct", false),
                p(&format!(r"^(\s*){rust_vis}enum\s+([A-Za-z_]\w*)"), "enum", false),
                p(&format!(r"^(\s*){rust_vis}(?:unsafe\s+)?trait\s+([A-Za-z_]\w*)"), "interface", false),
                p(&format!(r"^(\s*){rust_vis}type\s+([A-Za-z_]\w*)"), "type", false),
                p(&format!(r"^(\s*){rust_vis}mod\s+([A-Za-z_]\w*)"), "module", false),
                p(&format!(r"^(\s*){rust_vis}const\s+([A-Za-z_]\w*)\s*:"), "constant", false),
                p(&format!(r"^(\s*){rust_vis}static\s+(?:mut\s+)?([A-Za-z_]\w*)\s*:"), "variable", false),
                p(r"^(\s*)macro_rules!\s*([A-Za-z_]\w*)", "function", false),
                p(r"^(\s*)(?:unsafe\s+)?impl\b(?:\s*<[^{]*?>)?\s+(?:[^{]*?\bfor\s+)?(?:\w+::)*([A-Za-z_]\w*)", "impl", false),
            ],
        );
        m.insert(
            Lang::CSharp as u8,
            vec![
                p(r"^(\s*)(?:namespace|package)\s+([\w.]+)", "module", false),
                p(&format!(r"^(\s*){cs_mods}*(?:class|record(?:\s+class)?|object)\s+([A-Za-z_]\w*)"), "class", false),
                p(&format!(r"^(\s*){cs_mods}*(?:struct|record\s+struct)\s+([A-Za-z_]\w*)"), "struct", false),
                p(&format!(r"^(\s*){cs_mods}*(?:interface|protocol|@interface)\s+([A-Za-z_]\w*)"), "interface", false),
                p(&format!(r"^(\s*){cs_mods}*enum(?:\s+class)?\s+([A-Za-z_]\w*)"), "enum", false),
                p(&format!(r"^(\s*){cs_mods}*(?:fun|func)\s+(?:<[^>]*>\s*)?(?:[\w.]+\.)?([A-Za-z_]\w*)"), "function", false),
                p(&format!(r"^(\s*){cs_mods}+(?:[\w.]+(?:<[^()]*?>)?(?:\[\])*\??\s+)?([A-Za-z_]\w*)\s*(?:<[^()]*?>)?\s*\("), "method", false),
                p(&format!(r"^(\s*){cs_mods}+[\w.]+(?:<[^()]*?>)?(?:\[\])*\??\s+([A-Za-z_]\w*)\s*(?:\{{|=>|;|=)"), "field", true),
            ],
        );
        m.insert(
            Lang::Script as u8,
            vec![
                p(&format!(r"^(\s*){script_pre}(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)"), "function", false),
                p(&format!(r"^(\s*){script_pre}(?:abstract\s+)?class\s+([A-Za-z_$][\w$]*)"), "class", false),
                p(&format!(r"^(\s*){script_pre}interface\s+([A-Za-z_$][\w$]*)"), "interface", false),
                p(&format!(r"^(\s*){script_pre}type\s+([A-Za-z_$][\w$]*)\s*(?:<.*>)?\s*="), "type", false),
                p(&format!(r"^(\s*){script_pre}(?:const\s+)?enum\s+([A-Za-z_$][\w$]*)"), "enum", false),
                p(&format!(r"^(\s*){script_pre}(?:namespace|module)\s+([A-Za-z_$][\w$.]*)"), "module", false),
                p(
                    &format!(r"^(\s*){script_pre}(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=]+)?=\s*(?:async\s+)?(?:function\b|\([^)]*\)\s*(?::[^=]+)?=>|[A-Za-z_$][\w$]*\s*=>)"),
                    "function",
                    false,
                ),
                p(&format!(r"^(\s*){script_pre}const\s+([A-Z][A-Z0-9_]+)\s*[:=]"), "constant", false),
                p(
                    r"^(\s+)(?:(?:public|private|protected|static|readonly|async|override|abstract|get|set)\s+)*\*?([A-Za-z_$#][\w$]*)\s*(?:<[^>]*>)?\s*\([^;]*\)\s*(?::\s*[^={;]+)?\s*\{",
                    "method",
                    true,
                ),
            ],
        );
        m.insert(
            Lang::Python as u8,
            vec![
                p(r"^(\s*)(?:async\s+)?def\s+([A-Za-z_]\w*)", "function", false),
                p(r"^(\s*)class\s+([A-Za-z_]\w*)", "class", false),
                p(r"^()([A-Z][A-Z0-9_]+)\s*(?::[^=]*)?=[^=]", "constant", false),
            ],
        );
        m.insert(
            Lang::Go as u8,
            vec![
                p(r"^()func\s+(?:\([^)]*\)\s*)?([A-Za-z_]\w*)", "function", false),
                p(r"^(\s*)type\s+([A-Za-z_]\w*)\s+struct\b", "struct", false),
                p(r"^(\s*)type\s+([A-Za-z_]\w*)\s+interface\b", "interface", false),
                p(r"^(\s*)type\s+([A-Za-z_]\w*)\s", "type", false),
            ],
        );
        m.insert(
            Lang::C as u8,
            vec![
                p(r"^(\s*)(?:typedef\s+)?(?:class|struct|union)\s+([A-Za-z_]\w*)\s*(?:final\s*)?(?:[:{]|$)", "struct", false),
                p(r"^(\s*)(?:typedef\s+)?enum(?:\s+class)?\s+([A-Za-z_]\w*)", "enum", false),
                p(r"^(\s*)namespace\s+([A-Za-z_]\w*)", "module", false),
                p(r"^()(?:[A-Za-z_][\w:<>,]*[\s*&]+)+\**&?(?:[A-Za-z_]\w*::)*([A-Za-z_~]\w*)\s*\([^;]*$", "function", false),
                p(r"^()#define\s+([A-Za-z_]\w*)", "constant", false),
            ],
        );
        m.insert(
            Lang::Ruby as u8,
            vec![
                p(r"^(\s*)(?:(?:public|private|protected|static|abstract|final|local)\s+)*(?:def|function)\s+(?:self\.)?([A-Za-z_][\w.:]*[?!]?)", "function", false),
                p(r"^(\s*)(?:(?:abstract|final)\s+)?(?:class|trait)\s+([A-Za-z_][\w:]*)", "class", false),
                p(r"^(\s*)(?:module|interface)\s+([A-Za-z_][\w:]*)", "module", false),
                p(r"^()([A-Za-z_][\w-]*)\s*\(\)\s*\{", "function", false),
            ],
        );
        m
    });
    all.get(&(lang as u8)).map_or(&[], Vec::as_slice)
}

/// The declarations of a file, in its order, each with the one it is inside and its last line (the line before the
/// next one at its depth or out of it).
pub fn declarations(path: &str, text: &str) -> Vec<Sym> {
    let Some(lang) = lang_of(Path::new(path)) else { return Vec::new() };
    let mut out: Vec<Sym> = Vec::new();
    // The declarations open around the line: (depth, index in `out`, or none for an impl, its name, its kind).
    let mut open: Vec<(usize, Option<usize>, String, &'static str)> = Vec::new();
    let mut fenced = false;
    let mut lines = 0;
    for (n, line) in text.lines().enumerate() {
        lines = n + 1;
        let found = if lang == Lang::Markdown {
            if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
                fenced = !fenced;
            }
            heading(line).filter(|_| !fenced)
        } else {
            // Inside a class: the nearest declaration open around the line's indent holds others.
            let d = width(&line[..line.len() - line.trim_start().len()]);
            let inside = open.iter().rev().find(|o| o.0 < d).is_some_and(|o| holds(o.3));
            pats(lang).iter().filter(|p| !p.member || inside).find_map(|p| {
                let c = p.re.captures(line)?;
                let name = c.get(2)?.as_str();
                let depth = c.get(1).map_or(0, |i| width(i.as_str()));
                (!NOT_NAMES.contains(&name)).then(|| (depth, name.to_string(), p.kind))
            })
        };
        let Some((depth, name, kind)) = found else { continue };
        while open.last().is_some_and(|o| o.0 >= depth) {
            if let Some((_, Some(i), ..)) = open.pop() {
                out[i].end = n.max(out[i].line);
            }
        }
        let parent = open.last();
        let container = parent.map_or(String::new(), |o| o.2.clone());
        let kind = match kind {
            "function" if parent.is_some_and(|o| holds(o.3) && o.3 != "module") => "method",
            k => k,
        };
        let at = (kind != "impl").then(|| {
            out.push(Sym { name: name.clone(), kind, container, line: n + 1, end: n + 1 });
            out.len() - 1
        });
        open.push((depth, at, name, kind));
    }
    for (_, at, ..) in open {
        if let Some(i) = at {
            out[i].end = lines.max(out[i].line);
        }
    }
    out
}

/// A Markdown heading: its level as its depth, its words, as `heading`.
fn heading(line: &str) -> Option<(usize, String, &'static str)> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) || !line[hashes..].starts_with([' ', '\t']) {
        return None;
    }
    let words = line[hashes..].trim().trim_end_matches('#').trim();
    (!words.is_empty()).then(|| (hashes, words.to_string(), "heading"))
}

/// An indent's width, a tab as four.
fn width(indent: &str) -> usize {
    indent.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(syms: &[Sym]) -> Vec<(String, &'static str, String)> {
        syms.iter().map(|s| (s.name.clone(), s.kind, s.container.clone())).collect()
    }

    fn sym(name: &str) -> Sym {
        Sym { name: name.into(), kind: "function", container: String::new(), line: 1, end: 1 }
    }

    #[test]
    fn names_score_whole_then_start_then_initials_then_inside() {
        assert_eq!(score("RunToolsAsync", "runtoolsasync"), Some(100));
        assert_eq!(score("RunToolsAsync", "runt"), Some(80));
        assert_eq!(score("RunToolsAsync", "rta"), Some(60));
        assert_eq!(score("get_file_name", "gfn"), Some(60));
        assert_eq!(score("RunToolsAsync", "tools"), Some(40));
        assert_eq!(score("RunToolsAsync", "rnsy"), None);
        assert_eq!(score("RunToolsAsync", "zz"), None);
        assert_eq!(score("x", ""), None);
        assert_eq!(initials("HTTPServer2Go"), "hg");
    }

    #[test]
    fn ranked_by_score_then_code_before_tests_then_shorter_paths() {
        let (a, b, c, d) = (sym("Parse"), sym("ParseAll"), sym("Parse"), sym("Parse"));
        let all = [("src/deep/er/x.rs", &a), ("src/y.rs", &b), ("tests/z.rs", &c), ("src/a.rs", &d)];
        let got: Vec<&str> = rank(all.iter().copied(), "Parse", 10).into_iter().map(|(p, _)| p).collect();
        assert_eq!(got, ["src/a.rs", "src/deep/er/x.rs", "tests/z.rs", "src/y.rs"]);
        assert_eq!(rank(all.iter().copied(), "parse", 2).len(), 2);
        assert!(is_test("src/foo.test.ts") && is_test("tests/a.rs") && is_test("src/FooTests.cs"));
        assert!(is_test("pkg/a_test.go") && !is_test("src/contest.rs") && !is_test("src/latest.rs"));
    }

    #[test]
    fn rust_declarations_with_their_containers_and_ends() {
        let text = "pub struct Car {\n    wheels: u8,\n}\n\nimpl<T> Drive for Car {\n    pub fn go(&self) {}\n    async fn stop() {}\n}\n\nconst MAX: usize = 3;\nfn main() {\n    if x(y) {}\n}\n";
        let syms = declarations("src/car.rs", text);
        assert_eq!(
            names(&syms),
            [
                ("Car".into(), "struct", "".into()),
                ("go".into(), "method", "Car".into()),
                ("stop".into(), "method", "Car".into()),
                ("MAX".into(), "constant", "".into()),
                ("main".into(), "function", "".into()),
            ]
        );
        assert_eq!((syms[0].line, syms[0].end), (1, 4));
        assert_eq!((syms[4].line, syms[4].end), (11, 13));
    }

    #[test]
    fn csharp_and_script_and_python_declarations() {
        let cs = "namespace Arena.Tools\n{\n    public sealed class Runner\n    {\n        public int Count { get; set; }\n        public Runner(int n) { }\n        public async Task<bool> RunToolsAsync(string s)\n        {\n            if (s == null) return false;\n            foreach (var x in y) { }\n        }\n    }\n}\n";
        let got = names(&declarations("Runner.cs", cs));
        assert_eq!(
            got,
            [
                ("Arena.Tools".into(), "module", "".into()),
                ("Runner".into(), "class", "Arena.Tools".into()),
                ("Count".into(), "field", "Runner".into()),
                ("Runner".into(), "method", "Runner".into()),
                ("RunToolsAsync".into(), "method", "Runner".into()),
            ]
        );
        let ts = "export class Store {\n  async load(id: string): Promise<void> {\n    if (id) {\n    }\n  }\n  save() {\n  }\n}\nexport const useStore = () => 1;\ninterface Props {}\nfor (const a of b) {}\n";
        let got = names(&declarations("web/store.ts", ts));
        assert_eq!(
            got,
            [
                ("Store".into(), "class", "".into()),
                ("load".into(), "method", "Store".into()),
                ("save".into(), "method", "Store".into()),
                ("useStore".into(), "function", "".into()),
                ("Props".into(), "interface", "".into()),
            ]
        );
        let py = "class Shop:\n    def buy(self):\n        pass\n\nasync def main():\n    pass\nLIMIT = 3\n";
        let got = names(&declarations("shop.py", py));
        assert_eq!(
            got,
            [
                ("Shop".into(), "class", "".into()),
                ("buy".into(), "method", "Shop".into()),
                ("main".into(), "function", "".into()),
                ("LIMIT".into(), "constant", "".into()),
            ]
        );
    }

    #[test]
    fn markdown_headings_under_their_parents_not_in_code() {
        let md = "# Guide\n\n## Install\n\n```sh\n# not a heading\n```\n## Use #\n";
        let got = names(&declarations("docs/guide.md", md));
        assert_eq!(
            got,
            [
                ("Guide".into(), "heading", "".into()),
                ("Install".into(), "heading", "Guide".into()),
                ("Use".into(), "heading", "Guide".into()),
            ]
        );
        assert!(declarations("notes.txt", "fn x() {}").is_empty());
    }

    #[tokio::test]
    async fn asked_by_name_the_folder_is_scanned_and_answered_again() {
        let d = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(d.path().join("src")).expect("dir");
        std::fs::write(d.path().join("src/run.rs"), "pub fn run_tools_async() {}\nstruct Other;\n").expect("w");
        std::fs::write(d.path().join(".gitignore"), "gen/\n").expect("w");
        std::fs::create_dir_all(d.path().join("gen")).expect("dir");
        std::fs::write(d.path().join("gen/run.rs"), "fn run_tools_async() {}\n").expect("w");
        let ws = Arc::new(ca_workspace::Workspace::open(d.path(), &[]).expect("ws"));
        let (out, mut rx) = tokio::sync::broadcast::channel(16);
        let side = Side::new(ws, Arc::default(), out, None, &std::env::temp_dir());
        assert!(side.handle(&json!({"type": "symbols", "q": "rta", "limit": 10})));
        let mut last = Value::Null;
        for _ in 0..2 {
            let t = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("answer").expect("recv");
            last = serde_json::from_str(&t).expect("json");
            assert_eq!((last["type"].as_str(), last["q"].as_str()), (Some("symbols"), Some("rta")));
        }
        assert_eq!(last["building"], false);
        assert_eq!(
            last["items"],
            json!([{"name": "run_tools_async", "kind": "function", "container": "", "path": "src/run.rs", "line": 1, "end": 1}])
        );
    }
}
