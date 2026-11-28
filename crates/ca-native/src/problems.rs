//! The Problems view and what the window does with the problems, as VS Code's markers: the problems each source
//! said, its squiggles in the editor, their count in the status bar and on the view's icon, and the light bulb that
//! asks the agent to fix them.
//!
//! # The store
//!
//! Each source says all its problems at once, and they take the place of those it said before; none takes the
//! source away. A source is named by what it is: `check` (the project's own check), `task:<label>` (a task's
//! problem matcher), `lsp:<id>` (a language server), `security` (Arena's security review), `review` (the code
//! review). From the session it is the message
//!
//! ```text
//! problems{source, items: [{path, line, col, end_line?, end_col?, severity, code, message}]}
//! ```
//!
//! where `path` is the file's path in the folder with `/` (as the driver says every path; one under the folder
//! given whole is made so), `line` and `col` count from 1 and `col` counts characters, as VS Code's do (0, or none:
//! the whole line from its first word), and `severity` is `error`, `warning`, `info` or `hint`. A part of the
//! window says them with `publish`, and reads them with `of_file` and `counts`.
//!
//! # What is shown
//!
//! - The view: a section for each source, a fold for each file under it (its name, its folder dimmed, its count),
//!   and its problems, the worst first: the severity's icon, the message, the source and code, `[Ln 12, Col 5]`. A
//!   click opens the file there. Under the pointer: Fix with Code Arena; the right-click menu: Copy, Copy Message,
//!   Fix with Code Arena, Quick Fix…. Over the rows: the filter (`text, **/*.ts, !**/node_modules/**`: words looked for
//!   in the message, the code, the source and the path; a pattern of the files; `!` before either leaves them out),
//!   the filter's menu (Show Errors, Show Warnings, Show Infos, Show Active File Only); in the title, Collapse
//!   All. What the filter hides is counted under it.
//! - In every editor of a file with problems, opened before they came or after: a squiggle under each (its hover:
//!   the message, the source and code), its mark in the overview ruler, and the light bulb in the glyph margin of
//!   each line that has one: a click on it (or Ctrl+., Quick Fix…, on the caret's line) offers Fix with Code Arena.
//!   Marks are not moved by edits: they are said again when the source says its problems again.
//! - The status bar: the errors, the warnings (and the infos, when there are any); a click shows the view. The
//!   view's icon in the activity bar counts them all.
//! - The editor's "Fix this" (`codeArena.fixThis`) sends the problems marked in the lines it asks about.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_memo, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::editor::core::cursor::CursorAffinity;
use floem::views::{Decorators, container, h_stack, label, text_input, tooltip, v_stack};
use floem::{AnyView, IntoView};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::{Value, json};

use crate::badges;
use crate::chat_bridge;
use crate::commands::{self, command};
use crate::decor::{self, Mark, Severity, Tint};
use crate::ext::State;
use crate::icons::icon;
use crate::layout::View;
use crate::menus::Menu;
use crate::state::{Send, Ui};
use crate::widgets::tree::{self, Node};
use crate::widgets::{action, popup, toast};

/// Whether the view is in the window.
pub const READY: bool = true;

/// The decorations' owner (`decor.rs`).
const OWNER: &str = "problems";
/// The command of the editor's light bulb, as VS Code's.
pub const QUICK_FIX: &str = "editor.action.quickFix";
/// What the light bulb and the problem's menu offer.
const FIX_TITLE: &str = "Fix with Code Arena";
/// The filter's hint, as VS Code's.
const FILTER_HINT: &str = "Filter (text, **/*.ts, !exclude)";
const NONE_AT_ALL: &str = "No problems have been detected in the workspace.";
const NONE_IN_FILE: &str = "No problems have been detected in the current file.";
const NONE_SHOWN: &str = "No results found with provided filter criteria.";

/// A problem a source said: where it is (lines and columns from 1, columns in characters; 0: none), how bad, its
/// code and its message.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub path: String,
    pub line: usize,
    pub col: usize,
    pub end_line: Option<usize>,
    pub end_col: Option<usize>,
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

/// How many problems of each kind there are (hints with the infos, as VS Code's view has them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
}

impl Counts {
    pub fn total(self) -> usize {
        self.errors + self.warnings + self.infos
    }
}

/// The worst first.
fn rank(s: Severity) -> u8 {
    match s {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
        Severity::Hint => 3,
    }
}

fn severity_of(text: &str) -> Severity {
    match text.to_ascii_lowercase().as_str() {
        "error" | "err" | "fatal" => Severity::Error,
        "warning" | "warn" => Severity::Warning,
        "hint" => Severity::Hint,
        _ => Severity::Info,
    }
}

fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

/// VS Code's `MarkerSeverity` numbers, for Copy.
fn severity_number(s: Severity) -> u8 {
    match s {
        Severity::Error => 8,
        Severity::Warning => 4,
        Severity::Info => 2,
        Severity::Hint => 1,
    }
}

/// A source's name as the view's section says it.
pub fn source_title(source: &str) -> String {
    if let Some(label) = source.strip_prefix("task:") {
        return format!("Task: {label}");
    }
    if let Some(id) = source.strip_prefix("lsp:") {
        return id.to_string();
    }
    match source {
        "check" => "Project Check".into(),
        "security" => "Security Review".into(),
        "review" => "Code Review".into(),
        other => other.to_string(),
    }
}

/// A source's short name, as VS Code writes it after a message (`ts(2322)`).
fn source_short(source: &str) -> &str {
    source.split_once(':').map_or(source, |(_, rest)| rest)
}

/// "rustc(E0308)", "check", or "E0308" when the source says nothing more.
fn origin(source: &str, code: &str) -> String {
    let short = source_short(source);
    match (short.is_empty(), code.is_empty()) {
        (_, true) => short.to_string(),
        (true, false) => code.to_string(),
        (false, false) => format!("{short}({code})"),
    }
}

/// A path as the window knows a file: in the folder, with `/`.
fn path_in(folder: &str, path: &str) -> String {
    let path = path.replace('\\', "/");
    let folder = folder.trim_end_matches('/');
    let path = match path.strip_prefix(folder) {
        Some(rest) if !folder.is_empty() && rest.starts_with('/') => rest.trim_start_matches('/').to_string(),
        _ => path,
    };
    path.trim_start_matches("./").to_string()
}

/// A source's items, read and put in the view's order: by file, the worst first, then by place.
pub fn read(m: &Value, folder: &str) -> Vec<Problem> {
    let n = |v: &Value| v.as_u64().map(|n| n as usize);
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    let mut out: Vec<Problem> = m["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["path"].as_str().is_some_and(|s| !s.is_empty()))
        .map(|p| Problem {
            path: path_in(folder, p["path"].as_str().unwrap_or("")),
            line: n(&p["line"]).unwrap_or(1).max(1),
            col: n(&p["col"]).unwrap_or(0),
            end_line: n(&p["end_line"]).filter(|l| *l > 0),
            end_col: n(&p["end_col"]),
            severity: severity_of(p["severity"].as_str().unwrap_or("error")),
            code: match &p["code"] {
                Value::Number(c) => c.to_string(),
                c => text(c),
            },
            message: text(&p["message"]),
        })
        .collect();
    sort(&mut out);
    out
}

fn sort(list: &mut [Problem]) {
    list.sort_by(|a, b| {
        (a.path.as_str(), rank(a.severity), a.line, a.col).cmp(&(b.path.as_str(), rank(b.severity), b.line, b.col))
    });
}

/// The view's state and the store: every source's problems (in the view's order), the filter and its toggles, the
/// rows folded and the tree.
#[derive(Clone, Copy)]
struct Store {
    by_source: RwSignal<BTreeMap<String, Rc<Vec<Problem>>>>,
    filter: RwSignal<String>,
    errors: RwSignal<bool>,
    warnings: RwSignal<bool>,
    infos: RwSignal<bool>,
    active_only: RwSignal<bool>,
    folded: RwSignal<HashSet<String>>,
    tree: tree::State,
}

impl State for Store {
    fn new(ui: Ui) -> Store {
        Store {
            by_source: ui.scope.create_rw_signal(BTreeMap::new()),
            filter: ui.scope.create_rw_signal(String::new()),
            errors: ui.scope.create_rw_signal(true),
            warnings: ui.scope.create_rw_signal(true),
            infos: ui.scope.create_rw_signal(true),
            active_only: ui.scope.create_rw_signal(false),
            folded: ui.scope.create_rw_signal(HashSet::new()),
            tree: tree::State::new(ui),
        }
    }
}

fn store(ui: Ui) -> Store {
    ui.state::<Store>()
}

/// A source's problems, in place of those it said before (none: the source is gone).
pub fn publish(ui: Ui, source: &str, mut items: Vec<Problem>) {
    for p in &mut items {
        p.line = p.line.max(1);
    }
    sort(&mut items);
    let s = store(ui);
    let same = s.by_source.with_untracked(|m| match m.get(source) {
        Some(was) => **was == items,
        None => items.is_empty(),
    });
    if same {
        return;
    }
    s.by_source.update(|m| {
        if items.is_empty() {
            m.remove(source);
        } else {
            m.insert(source.to_string(), Rc::new(items));
        }
    });
    let total = counts_now(ui).total();
    badges::set(ui, View::Problems, total);
}

/// Every source's problems in a file, by source.
pub fn of_file(ui: Ui, path: &str) -> Vec<(String, Problem)> {
    store(ui).by_source.with(|m| {
        m.iter()
            .flat_map(|(src, list)| list.iter().filter(|p| p.path == path).map(move |p| (src.clone(), p.clone())))
            .collect()
    })
}

/// Each problem's file and severity, of every source (followed where it is read).
pub fn severities(ui: Ui) -> Vec<(String, Severity)> {
    store(ui).by_source.with(|m| m.values().flat_map(|l| l.iter()).map(|p| (p.path.clone(), p.severity)).collect())
}

/// How many problems there are of each kind (followed where it is read).
pub fn counts(ui: Ui) -> Counts {
    store(ui).by_source.with(|m| count(m.values().flat_map(|l| l.iter())))
}

fn counts_now(ui: Ui) -> Counts {
    store(ui).by_source.with_untracked(|m| count(m.values().flat_map(|l| l.iter())))
}

fn count<'a>(all: impl Iterator<Item = &'a Problem>) -> Counts {
    let mut c = Counts::default();
    for p in all {
        match p.severity {
            Severity::Error => c.errors += 1,
            Severity::Warning => c.warnings += 1,
            Severity::Info | Severity::Hint => c.infos += 1,
        }
    }
    c
}

// ---- The filter ----

/// The filter's text read: words looked for and patterns of files, each kept or left out.
#[derive(Default)]
struct Filter {
    words: Vec<String>,
    not_words: Vec<String>,
    globs: Option<GlobSet>,
    not_globs: Option<GlobSet>,
}

fn glob_set(patterns: &[String]) -> Option<GlobSet> {
    if patterns.is_empty() {
        return None;
    }
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        // A pattern without a folder is of a file's name anywhere, as VS Code's.
        let p = if p.contains('/') { p.clone() } else { format!("**/{p}") };
        if let Ok(g) = Glob::new(&p) {
            b.add(g);
        }
    }
    b.build().ok()
}

impl Filter {
    fn of(text: &str) -> Filter {
        let (mut words, mut not_words, mut globs, mut not_globs) = (vec![], vec![], vec![], vec![]);
        for term in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let (not, term) = match term.strip_prefix('!') {
                Some(rest) => (true, rest.trim()),
                None => (false, term),
            };
            if term.is_empty() {
                continue;
            }
            let glob = term.contains(['*', '?', '[', '{']);
            match (glob, not) {
                (true, false) => globs.push(term.to_string()),
                (true, true) => not_globs.push(term.to_string()),
                (false, false) => words.push(term.to_lowercase()),
                (false, true) => not_words.push(term.to_lowercase()),
            }
        }
        Filter { words, not_words, globs: glob_set(&globs), not_globs: glob_set(&not_globs) }
    }

    fn keeps(&self, source: &str, p: &Problem) -> bool {
        let hay = || format!("{} {} {} {}", p.message, p.code, source, p.path).to_lowercase();
        let hay = if self.words.is_empty() && self.not_words.is_empty() { String::new() } else { hay() };
        if self.not_words.iter().any(|w| hay.contains(w))
            || self.not_globs.as_ref().is_some_and(|g| g.is_match(&p.path))
        {
            return false;
        }
        if self.words.is_empty() && self.globs.is_none() {
            return true;
        }
        self.words.iter().any(|w| hay.contains(w)) || self.globs.as_ref().is_some_and(|g| g.is_match(&p.path))
    }
}

/// What the filter's toggles let through.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shown {
    errors: bool,
    warnings: bool,
    infos: bool,
}

impl Shown {
    fn has(self, s: Severity) -> bool {
        match s {
            Severity::Error => self.errors,
            Severity::Warning => self.warnings,
            Severity::Info | Severity::Hint => self.infos,
        }
    }
}

// ---- The rows ----

const SEP: char = '\u{1f}';

fn source_id(src: &str) -> String {
    format!("s{SEP}{src}")
}

fn file_id(src: &str, path: &str) -> String {
    format!("f{SEP}{src}{SEP}{path}")
}

fn problem_id(src: &str, at: usize) -> String {
    format!("p{SEP}{src}{SEP}{at}")
}

/// What a row is, by its id.
#[derive(Debug, PartialEq)]
enum Row<'a> {
    Source(&'a str),
    File(&'a str, &'a str),
    Problem(&'a str, usize),
}

fn row_of(id: &str) -> Option<Row<'_>> {
    let mut parts = id.splitn(3, SEP);
    match (parts.next()?, parts.next()?, parts.next()) {
        ("s", src, None) => Some(Row::Source(src)),
        ("f", src, Some(path)) => Some(Row::File(src, path)),
        ("p", src, Some(at)) => Some(Row::Problem(src, at.parse().ok()?)),
        _ => None,
    }
}

fn stamp_of(h: impl Hash) -> u64 {
    let mut s = std::collections::hash_map::DefaultHasher::new();
    h.hash(&mut s);
    s.finish()
}

/// The rows shown, and how many problems the filter hid: each source with its files under it and their problems
/// under them, a folded row's children left out.
fn rows(
    sources: &BTreeMap<String, Rc<Vec<Problem>>>,
    keep: impl Fn(&str, &Problem) -> bool,
    folded: &HashSet<String>,
) -> (im::Vector<Node>, usize) {
    let (mut out, mut hidden) = (im::Vector::new(), 0);
    for (src, list) in sources {
        let kept: Vec<usize> = (0..list.len()).filter(|&i| keep(src, &list[i])).collect();
        hidden += list.len() - kept.len();
        if kept.is_empty() {
            continue;
        }
        let sid = source_id(src);
        let open = !folded.contains(&sid);
        out.push_back(Node { stamp: kept.len() as u64, ..tree::folder(&sid, 0, open) });
        if !open {
            continue;
        }
        let mut at = 0;
        while at < kept.len() {
            let path = &list[kept[at]].path;
            let end = kept[at..].iter().position(|&i| list[i].path != *path).map_or(kept.len(), |n| at + n);
            let fid = file_id(src, path);
            let open = !folded.contains(&fid);
            out.push_back(Node { stamp: (end - at) as u64, ..tree::folder(&fid, 1, open) });
            if open {
                for &i in &kept[at..end] {
                    let p = &list[i];
                    let stamp = stamp_of((&p.message, &p.code, p.line, p.col, rank(p.severity)));
                    out.push_back(Node { stamp, ..tree::leaf(&problem_id(src, i), 2) });
                }
            }
            at = end;
        }
    }
    (out, hidden)
}

fn problem_at(ui: Ui, src: &str, at: usize) -> Option<Problem> {
    store(ui).by_source.with_untracked(|m| m.get(src).and_then(|l| l.get(at).cloned()))
}

/// The file the active tab shows (none: a tab that is not a file's text).
fn active_file(ui: Ui) -> Option<String> {
    let path = ui.active.get()?;
    ui.tabs.with(|t| t.iter().find(|t| t.path == path && t.is_file()).map(|t| t.file.clone()))
}

// ---- Asking the agent ----

/// A problem as it is said to the agent: its place as a mention, how bad, its origin, its message.
fn said(source: &str, p: &Problem) -> String {
    let at = if p.col > 0 { format!(":{}:{}", p.line, p.col) } else { format!(":{}", p.line) };
    let place = crate::agent::mentions::mention(&p.path, &at, true);
    let from = origin(source, &p.code);
    let from = if from.is_empty() { String::new() } else { format!(" {from}") };
    format!("- {place}{}{from}: {}", severity_word(p.severity), p.message)
}

/// What asks the agent to fix these problems.
fn fix_words(problems: &[(String, Problem)]) -> String {
    let head = if problems.len() == 1 { "Fix this problem:" } else { "Fix these problems:" };
    let list: Vec<String> = problems.iter().map(|(s, p)| said(s, p)).collect();
    format!("{head}\n{}", list.join("\n"))
}

/// The agent asked at once to fix these problems.
fn fix(ui: Ui, send: &Send, problems: &[(String, Problem)]) {
    if problems.is_empty() {
        return;
    }
    chat_bridge::ask_now(ui, send, &fix_words(problems));
    ui.reveal(View::Agent);
}

/// The problems of a file on a line (from 0).
fn on_line(ui: Ui, path: &str, line: usize) -> Vec<(String, Problem)> {
    let mut all = of_file(ui, path);
    all.retain(|(_, p)| p.line == line + 1 || (p.line <= line + 1 && p.end_line.is_some_and(|e| e > line)));
    all
}

/// The light bulb's menu for a line (from 0) of a file: Fix with Code Arena, and the whole file's when it has
/// more; by the line in its editor. Nothing when the editor is not on screen: the agent is asked only by a click.
fn quick_fix_at(ui: Ui, send: &Send, path: &str, line: usize) {
    let here = on_line(ui, path, line);
    if here.is_empty() {
        toast::info(ui, "No code actions available");
        return;
    }
    let all = of_file(ui, path);
    let Some(at) = at_line(ui, path, line) else { return };
    let (s1, s2) = (send.clone(), send.clone());
    let mut items: Vec<popup::Item> = vec![popup::entry(FIX_TITLE, move || fix(ui, &s1, &here)).into()];
    if all.len() > 1 {
        let n = all.len();
        items.push(
            popup::entry(&format!("Fix All {n} Problems in File with Code Arena"), move || fix(ui, &s2, &all)).into(),
        );
    }
    popup::open(ui, at, items);
}

/// Where a menu for a line (from 0) of the file in the live editor opens: under the line's start.
fn at_line(ui: Ui, path: &str, line: usize) -> Option<popup::At> {
    let editor = ui.live.with_untracked(|l| l.as_ref().filter(|(p, _)| p == path).map(|(_, e)| e.clone()))?;
    let view = editor.editor_view_id.get_untracked()?;
    let offset = editor.offset_of_line(line.min(editor.last_line()));
    let (o, v) = (editor.window_origin.get_untracked(), editor.viewport.get_untracked());
    let (top, bottom) = editor.points_of_offset(offset, CursorAffinity::Forward);
    if bottom.y == 0.0 && top.y == 0.0 && line > 0 {
        return None;
    }
    Some(popup::At::point(view, o.x + top.x - v.x0, o.y + bottom.y - v.y0).back(view))
}

/// The live editor's file and its caret's line (from 0).
fn caret_line(ui: Ui) -> Option<(String, usize)> {
    let (path, e) = ui.live.get_untracked()?;
    let tab = ui.tab(&path)?;
    if !tab.is_file() {
        return None;
    }
    let offset = e.cursor.with_untracked(|c| c.offset());
    Some((tab.file.clone(), e.line_of_offset(offset)))
}

/// The editor's "Fix this", with the problems marked in its lines.
fn fix_this(ui: Ui, send: &Send) {
    let Some(chip) = crate::agent::pieces::current(ui) else { return };
    let piece = &chip.piece;
    let marked: Vec<(String, Problem)> =
        of_file(ui, &piece.path).into_iter().filter(|(_, p)| p.line >= piece.start && p.line <= piece.end).collect();
    let words = if marked.is_empty() {
        "Fix this".to_string()
    } else {
        let list: Vec<String> = marked.iter().map(|(s, p)| said(s, p)).collect();
        format!("Fix this. The problems marked in these lines:\n{}", list.join("\n"))
    };
    crate::agent::pieces::ask(ui, send, &words);
}

// ---- The editor's marks ----

/// A column (from 1, in characters; 0: none) as a byte offset in a line.
fn byte_at(line: &str, col: usize) -> usize {
    if col == 0 {
        return 0;
    }
    line.char_indices().nth(col - 1).map_or(line.len(), |(b, _)| b)
}

/// Where a problem's squiggle goes, in lines from 0 and byte columns: its range; with no end, the word at its
/// place (the rest of the line when it is at none); with no column, the line from its first word.
fn range_of(p: &Problem, text_of: &dyn Fn(usize) -> String) -> ((usize, usize), (usize, usize)) {
    let line = p.line.saturating_sub(1);
    let text = text_of(line);
    let start = if p.col == 0 { text.len() - text.trim_start().len() } else { byte_at(&text, p.col) };
    if let (Some(el), Some(ec)) = (p.end_line, p.end_col) {
        let el = el.saturating_sub(1);
        let end = if el == line { byte_at(&text, ec) } else { byte_at(&text_of(el), ec) };
        if (el, end) > (line, start) {
            return ((line, start), (el, end));
        }
    }
    if p.col == 0 {
        return ((line, start), (line, text.trim_end().len().max(start)));
    }
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let rest = &text[start..];
    let n = rest.find(|c: char| !word(c)).unwrap_or(rest.len());
    let end = if n > 0 { start + n } else { start + rest.chars().next().map_or(0, char::len_utf8) };
    ((line, start), (line, end))
}

/// A file's marks: a squiggle for each problem, and the light bulb on each line that has one.
fn marks_of(ui_path: &str, problems: &[(String, Problem)], text_of: &dyn Fn(usize) -> String) -> Vec<Mark> {
    let mut out = Vec::new();
    let mut lines = HashSet::new();
    for (src, p) in problems {
        let (from, to) = range_of(p, text_of);
        let from_said = origin(src, &p.code);
        let hover = if from_said.is_empty() { p.message.clone() } else { format!("{} {from_said}", p.message) };
        out.push(Mark::squiggle(from, to, p.severity, &hover));
        lines.insert(from.0);
    }
    let mut lines: Vec<usize> = lines.into_iter().collect();
    lines.sort_unstable();
    for line in lines {
        let path = ui_path.to_string();
        let click: decor::Click = Rc::new(move |ui, send, line| quick_fix_at(ui, send, &path, line));
        out.push(Mark::Glyph {
            line,
            icon: "lightbulb-sparkle",
            tint: Tint::Theme(|c| c.light_bulb),
            title: format!("{FIX_TITLE} (Ctrl+.)"),
            click: Some(click),
        });
    }
    out
}

thread_local! {
    /// The files marked, by their tab's path: what their problems were when marked.
    static MARKED: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

/// The open files' marks said again where their problems changed; the marks of a file closed taken away (it is
/// marked again from its text when it opens).
fn remark(ui: Ui, open: &[(String, String)]) {
    let mut keep = HashSet::new();
    for (path, file) in open {
        keep.insert(path.clone());
        let problems = store(ui).by_source.with_untracked(|m| {
            m.iter()
                .flat_map(|(s, l)| l.iter().filter(|p| p.path == *file).map(move |p| (s.clone(), p.clone())))
                .collect::<Vec<_>>()
        });
        let sum = stamp_of(
            problems
                .iter()
                .map(|(s, p)| (s, &p.message, &p.code, p.line, p.col, p.end_line, p.end_col, rank(p.severity)))
                .collect::<Vec<_>>(),
        );
        if MARKED.with(|m| m.borrow().get(path) == Some(&sum)) {
            continue;
        }
        MARKED.with(|m| m.borrow_mut().insert(path.clone(), sum));
        if problems.is_empty() {
            decor::clear(ui, OWNER, path);
            continue;
        }
        let Some(tab) = ui.tab(path) else { continue };
        let text = tab.text();
        let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
        let text_of = |line: usize| lines.get(line).map_or(String::new(), |l| l.to_string());
        decor::set(ui, OWNER, path, marks_of(path, &problems, &text_of));
    }
    let gone: Vec<String> = MARKED.with(|m| m.borrow().keys().filter(|p| !keep.contains(*p)).cloned().collect());
    for path in gone {
        MARKED.with(|m| m.borrow_mut().remove(&path));
        decor::clear(ui, OWNER, &path);
    }
}

// ---- The view ----

/// The count after a section's or a file's name, as VS Code's.
fn count_badge(ui: Ui, n: usize) -> impl IntoView {
    let t = ui.colors;
    label(move || n.to_string()).style(move |s| {
        let c = t.get();
        s.font_size(11.0)
            .padding_horiz(5.0)
            .height(16.0)
            .min_width(16.0)
            .items_center()
            .justify_center()
            .border_radius(8.0)
            .background(c.scm_count_bg)
            .color(c.scm_count_fg)
    })
}

fn severity_icon(ui: Ui, s: Severity) -> impl IntoView {
    let t = ui.colors;
    let name = match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info | Severity::Hint => "info",
    };
    icon(name, 16.0).style(move |st| {
        let c = t.get();
        let colour: Color = match s {
            Severity::Error => c.problem_error,
            Severity::Warning => c.warning,
            Severity::Info | Severity::Hint => c.problem_info,
        };
        st.color(colour).flex_shrink(0.0_f32)
    })
}

/// A row's content: a source with its count, a file (its name, its folder dimmed, its count) or a problem.
fn row_view(ui: Ui, node: &Node) -> AnyView {
    let t = ui.colors;
    let desc = move |text: String| {
        label(move || text.clone())
            .style(move |s| s.font_size(12.0).color(t.get().desc).text_ellipsis().min_width(0.0).flex_shrink(1.0_f32))
    };
    match row_of(&node.id) {
        Some(Row::Source(src)) => {
            let title = source_title(src);
            h_stack((
                label(move || title.clone()).style(|s| s.font_size(13.0).font_bold().text_ellipsis().min_width(0.0)),
                count_badge(ui, node.stamp as usize),
            ))
            .style(|s| s.items_center().gap(6.0).min_width(0.0))
            .into_any()
        }
        Some(Row::File(_, path)) => {
            let (dir, name) = match path.rsplit_once('/') {
                Some((d, n)) => (d.to_string(), n.to_string()),
                None => (String::new(), path.to_string()),
            };
            h_stack((
                icon("file", 16.0).style(|s| s.flex_shrink(0.0_f32)),
                label(move || name.clone()).style(|s| s.font_size(13.0).flex_shrink(0.0_f32)),
                desc(dir),
                count_badge(ui, node.stamp as usize),
            ))
            .style(|s| s.items_center().gap(6.0).min_width(0.0))
            .into_any()
        }
        Some(Row::Problem(src, at)) => {
            let Some(p) = problem_at(ui, src, at) else { return label(|| "").into_any() };
            let place = if p.col > 0 { format!("[Ln {}, Col {}]", p.line, p.col) } else { format!("[Ln {}]", p.line) };
            let from = origin(src, &p.code);
            let message = p.message.lines().next().unwrap_or("").to_string();
            let whole = p.message.clone();
            let line = h_stack((
                severity_icon(ui, p.severity),
                label(move || message.clone())
                    .style(|s| s.font_size(13.0).text_ellipsis().min_width(80.0).flex_shrink(1.0_f32)),
                desc(from).style(|s| s.flex_shrink(4.0_f32)),
                desc(place).style(|s| s.flex_shrink(2.0_f32)),
            ))
            .style(|s| s.items_center().gap(6.0).min_width(0.0).width_full());
            tooltip(line, move || {
                let whole = whole.clone();
                label(move || whole.clone()).style(|s| s.max_width(600.0))
            })
            .style(|s| s.min_width(0.0).width_full())
            .into_any()
        }
        None => label(|| "").into_any(),
    }
}

/// The problem's place opened in its file.
fn open_problem(ui: Ui, send: &Send, p: &Problem) {
    crate::quick_modes::open_at(ui, send, &p.path, p.line, p.col.max(1));
}

/// What Copy puts on the clipboard, as VS Code's: the marker as JSON.
fn copied(ui: Ui, source: &str, p: &Problem) -> String {
    let folder = ui.folder.get_untracked();
    let resource =
        if folder.is_empty() { p.path.clone() } else { format!("{}/{}", folder.trim_end_matches('/'), p.path) };
    let end_line = p.end_line.unwrap_or(p.line);
    let end_col = p.end_col.unwrap_or(p.col);
    let marker = json!({
        "resource": resource,
        "owner": source,
        "code": p.code,
        "severity": severity_number(p.severity),
        "message": p.message,
        "source": source_short(source),
        "startLineNumber": p.line,
        "startColumn": p.col.max(1),
        "endLineNumber": end_line,
        "endColumn": end_col.max(1),
    });
    serde_json::to_string_pretty(&[marker]).unwrap_or_default()
}

fn copy(ui: Ui, text: String) {
    if floem::Clipboard::set_contents(text).is_err() {
        toast::error(ui, "The problem could not be copied.");
    }
}

/// A problem's right-click menu, then what the window's parts registered for the place.
fn open_menu(ui: Ui, send: &Send, src: &str, p: Problem, at: popup::At) {
    let (c1, c2) = (copied(ui, src, &p), p.message.clone());
    let pair = vec![(src.to_string(), p.clone())];
    let (fixer, opener) = (send.clone(), send.clone());
    let quick = p.clone();
    let mut items: Vec<popup::Item> = vec![
        popup::entry("Copy", move || copy(ui, c1.clone())).into(),
        popup::entry("Copy Message", move || copy(ui, c2.clone())).into(),
        popup::line(),
        popup::entry(FIX_TITLE, move || fix(ui, &fixer, &pair)).into(),
        popup::entry("Quick Fix...", move || {
            open_problem(ui, &opener, &quick);
            menu_when_drawn(ui, opener.clone(), quick.path.clone(), quick.line.saturating_sub(1), 20);
        })
        .keys("Ctrl+.")
        .into(),
    ];
    let more = popup::of_rows(ui, send, crate::menus::rows(ui, Menu::ProblemsRow));
    if !more.is_empty() {
        items.push(popup::line());
        items.extend(more);
    }
    popup::open(ui, at, items);
}

/// The light bulb's menu once the file's editor is drawn (asked every 50 ms, a second at most).
fn menu_when_drawn(ui: Ui, send: Send, path: String, line: usize, left: u32) {
    floem::action::exec_after(std::time::Duration::from_millis(50), move |_| {
        if at_line(ui, &path, line).is_some() {
            quick_fix_at(ui, &send, &path, line);
        } else if left > 0 {
            menu_when_drawn(ui, send, path, line, left - 1);
        }
    });
}

/// The view's own content.
pub fn view(ui: Ui, send: Send) -> floem::AnyView {
    let t = ui.colors;
    let s = store(ui);
    let shown = create_memo(move |_| {
        let filter = Filter::of(&s.filter.get());
        let on = Shown { errors: s.errors.get(), warnings: s.warnings.get(), infos: s.infos.get() };
        let only = if s.active_only.get() { Some(active_file(ui).unwrap_or_default()) } else { None };
        let keep = |src: &str, p: &Problem| {
            on.has(p.severity) && only.as_ref().is_none_or(|f| *f == p.path) && filter.keeps(src, p)
        };
        s.folded.with(|f| s.by_source.with(|m| rows(m, keep, f)))
    });
    let any = create_memo(move |_| s.by_source.with(|m| !m.is_empty()));
    let hidden = create_memo(move |_| shown.with(|r| r.1));
    let empty = create_memo(move |_| shown.with(|r| r.0.is_empty()));
    let (acting, opening, menu_send) = (send.clone(), send.clone(), send);
    let list = tree::tree(ui, s.tree, move || shown.with(|r| r.0.clone()))
        .content(move |node| row_view(ui, node))
        .actions(move |node| {
            let Some(Row::Problem(src, at)) = row_of(&node.id) else { return vec![] };
            let Some(p) = problem_at(ui, src, at) else { return vec![] };
            let (pair, send) = (vec![(src.to_string(), p)], acting.clone());
            vec![tree::act("sparkle", FIX_TITLE, move || fix(ui, &send, &pair))]
        })
        .on_toggle(move |id| {
            s.folded.update(|f| {
                if !f.remove(id) {
                    f.insert(id.to_string());
                }
            });
        })
        .on_open(move |id| {
            if let Some(Row::Problem(src, at)) = row_of(id)
                && let Some(p) = problem_at(ui, src, at)
            {
                open_problem(ui, &opening, &p);
            }
        })
        .on_menu(move |id, at| {
            if let Some(Row::Problem(src, i)) = id.and_then(row_of)
                && let Some(p) = problem_at(ui, src, i)
            {
                open_menu(ui, &menu_send, src, p, at);
            }
        })
        .view();
    let clear = label(|| "Clear Filters").on_click_stop(move |_| clear_filters(ui)).style(move |st| {
        let c = t.get();
        st.color(c.link).cursor(CursorStyle::Pointer)
    });
    let said = label(move || {
        if !any.get() {
            NONE_AT_ALL.to_string()
        } else if hidden.get() == 0 && s.active_only.get() {
            NONE_IN_FILE.to_string()
        } else {
            NONE_SHOWN.to_string()
        }
    })
    // A narrow side bar wraps it, as VS Code's message does, rather than clipping it.
    .style(move |st| st.color(t.get().desc).text_overflow(TextOverflow::Wrap).min_width(0.0).max_width_full());
    let note =
        h_stack((said, clear.style(move |st| st.apply_if(hidden.get() == 0, |st| st.hide())))).style(move |st| {
            st.gap(6.0)
                .padding_horiz(20.0)
                .padding_vert(6.0)
                .line_height(1.4)
                .flex_wrap(floem::style::FlexWrap::Wrap)
                .apply_if(!empty.get(), |st| st.hide())
        });
    let hid = h_stack((
        label(move || hidden_text(hidden.get())).style(move |st| st.color(t.get().desc)),
        label(|| "Clear Filters").on_click_stop(move |_| clear_filters(ui)).style(move |st| {
            let c = t.get();
            st.color(c.link).cursor(CursorStyle::Pointer)
        }),
    ))
    .style(move |st| {
        st.gap(6.0)
            .padding_horiz(20.0)
            .padding_vert(3.0)
            .font_size(12.0)
            .apply_if(hidden.get() == 0 || empty.get(), |st| st.hide())
    });
    v_stack((
        filter_row(ui),
        note,
        hid,
        list.style(|st| st.flex_grow(1.0_f32).flex_basis(0.0).width_full().min_height(0.0)),
    ))
    .style(|st| st.size_full().min_height(0.0))
    .into_any()
}

/// "3 problems hidden by the filter".
fn hidden_text(n: usize) -> String {
    if n == 1 { "1 problem hidden by the filter.".into() } else { format!("{n} problems hidden by the filter.") }
}

/// The filter's text emptied and every kind shown again, for the whole folder.
fn clear_filters(ui: Ui) {
    let s = store(ui);
    if !s.filter.with_untracked(String::is_empty) {
        s.filter.set(String::new());
    }
    for on in [s.errors, s.warnings, s.infos] {
        if !on.get_untracked() {
            on.set(true);
        }
    }
    if s.active_only.get_untracked() {
        s.active_only.set(false);
    }
}

/// Every source and file folded.
fn collapse_all(ui: Ui) {
    let s = store(ui);
    let all: HashSet<String> = s.by_source.with_untracked(|m| {
        m.iter()
            .flat_map(|(src, l)| {
                std::iter::once(source_id(src)).chain(l.iter().map(|p| file_id(src, &p.path)).collect::<Vec<_>>())
            })
            .collect()
    });
    if s.folded.with_untracked(|f| *f != all) {
        s.folded.set(all);
    }
}

/// The filter over the rows, the whole width: its text, and its menu of what is shown.
fn filter_row(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let s = store(ui);
    let focused = create_rw_signal(false);
    let input = text_input(s.filter)
        .placeholder(FILTER_HINT)
        .on_event_cont(floem::event::EventListener::FocusGained, move |_| focused.set(true));
    let input =
        input.on_event_cont(floem::event::EventListener::FocusLost, move |_| focused.set(false)).style(move |st| {
            let c = t.get();
            let st = st
                .flex_grow(1.0_f32)
                .min_width(0.0)
                .padding_horiz(4.0)
                .border(0.0)
                .font_size(12.0)
                .background(Color::TRANSPARENT)
                .color(c.input_fg)
                .cursor_color(floem::peniko::Brush::Solid(c.cursor));
            crate::widgets::steady_input(st, Color::TRANSPARENT, None)
        });
    let narrowed = create_memo(move |_| !(s.errors.get() && s.warnings.get() && s.infos.get()) || s.active_only.get());
    let menu = container(icon("filter", 16.0)).style(move |st| {
        let c = t.get();
        st.padding(2.0)
            .border_radius(3.0)
            .cursor(CursorStyle::Pointer)
            .apply_if(narrowed.get(), |st| st.color(c.focus))
            .hover(|st| st.background(c.toolbar_hover))
    });
    let id = floem::View::id(&menu);
    let menu = menu.on_click_stop(move |_| {
        let toggle = |on: RwSignal<bool>| move || on.update(|v| *v = !*v);
        popup::open(
            ui,
            popup::At::under(id),
            vec![
                popup::entry("Show Errors", toggle(s.errors)).checked(s.errors.get_untracked()).into(),
                popup::entry("Show Warnings", toggle(s.warnings)).checked(s.warnings.get_untracked()).into(),
                popup::entry("Show Infos", toggle(s.infos)).checked(s.infos.get_untracked()).into(),
                popup::line(),
                popup::entry("Show Active File Only", toggle(s.active_only))
                    .checked(s.active_only.get_untracked())
                    .into(),
            ],
        );
    });
    let menu = tooltip(menu, || label(|| "More Filters..."));
    let filter = h_stack((input, menu)).style(move |st| {
        let c = t.get();
        st.width_full()
            .height(24.0)
            .padding_right(2.0)
            .items_center()
            .border(1.0)
            .border_radius(2.0)
            .border_color(if focused.get() { c.focus } else { c.input_border })
            .background(c.input_bg)
    });
    container(filter).style(|st| st.width_full().padding_left(20.0).padding_right(12.0).padding_vert(4.0))
}

/// The actions in its title: Collapse All.
pub fn actions(ui: Ui, _send: Send) -> Option<floem::AnyView> {
    let t = ui.colors;
    let collapse = tooltip(action(t, "collapse-all", move || collapse_all(ui)), || label(|| "Collapse All"));
    Some(collapse.into_any())
}

/// The status bar's words for the counts' tooltip.
fn counts_tip(c: Counts) -> String {
    if c.total() == 0 {
        "No Problems".into()
    } else {
        format!("Errors: {}, Warnings: {}, Infos: {}", c.errors, c.warnings, c.infos)
    }
}

/// What it adds to the window's registries, once at start: the status bar's counts, Quick Fix (Ctrl+.), the
/// editor's "Fix this" with the problems in its lines, and the marks of the open files followed.
pub fn register(ui: Ui, _send: &Send) {
    crate::statusbar::add("status.problems", crate::statusbar::Side::Left, 120, |ui, send| {
        let t = ui.colors;
        let c = create_memo(move |_| counts(ui));
        let n = move |pick: fn(Counts) -> usize| label(move || pick(c.get()).to_string());
        let infos = move || c.get().infos == 0;
        tooltip(
            h_stack((
                icon("error", 16.0),
                n(|c| c.errors),
                icon("warning", 16.0).style(|s| s.margin_left(4.0)),
                n(|c| c.warnings),
                icon("info", 16.0).style(move |s| s.margin_left(4.0).apply_if(infos(), |s| s.hide())),
                n(|c| c.infos).style(move |s| s.apply_if(infos(), |s| s.hide())),
            ))
            .style(|s| s.gap(3.0).items_center())
            .on_click_stop(move |_| {
                commands::run(ui, &send, View::Problems.command());
            }),
            move || label(move || counts_tip(c.get())),
        )
        .style(move |s| crate::statusbar::item(t, s).cursor(CursorStyle::Pointer))
        .into_any()
    });
    let in_file = |ui: Ui| caret_line(ui).is_some();
    commands::add(
        command(QUICK_FIX, "", "Quick Fix...", |ui, send| {
            if let Some((path, line)) = caret_line(ui) {
                let tab = ui.live.get_untracked().map(|(p, _)| p).unwrap_or(path);
                let file = ui.tab(&tab).map(|t| t.file).unwrap_or_default();
                quick_fix_at_file(ui, send, &tab, &file, line);
            }
        })
        .keys("Ctrl+.")
        .when(in_file),
    );
    let offered = |ui: Ui| crate::agent::pieces::current(ui).is_some();
    commands::add(command(crate::agent::pieces::FIX, "Code Arena", "Fix this", fix_this).when(offered));
    // The open files' marks, said again when the problems change or a file opens or closes.
    let open = ui.scope.create_memo(move |_| {
        ui.tabs.with(|t| t.iter().filter(|t| t.is_file()).map(|t| (t.path.clone(), t.file.clone())).collect::<Vec<_>>())
    });
    let by_source = store(ui).by_source;
    ui.scope.create_effect(move |_| {
        by_source.track();
        open.with(|o| remark(ui, o));
    });
}

/// The light bulb's menu for a tab's file at a line: the tab's path is the editor's, the file the problems'.
fn quick_fix_at_file(ui: Ui, send: &Send, tab: &str, file: &str, line: usize) {
    if tab == file {
        quick_fix_at(ui, send, file, line);
        return;
    }
    let here = on_line(ui, file, line);
    if here.is_empty() {
        toast::info(ui, "No code actions available");
    } else {
        fix(ui, send, &here);
    }
}

/// The session's messages that are this part's, taken in: a source's problems (left for the parts after it, the
/// Explorer's decorations among them).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"].as_str() == Some("problems")
        && let Some(source) = m["source"].as_str().filter(|s| !s.is_empty())
    {
        let items = read(m, &ui.folder.get_untracked());
        publish(ui, source, items);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str, line: usize, col: usize, sev: Severity, msg: &str) -> Problem {
        Problem {
            path: path.into(),
            line,
            col,
            end_line: None,
            end_col: None,
            severity: sev,
            code: String::new(),
            message: msg.into(),
        }
    }

    #[test]
    fn a_sources_items_are_read_in_the_folder_the_worst_first() {
        let m = json!({"type": "problems", "source": "check", "items": [
            {"path": "/w/src/b.rs", "line": 3, "col": 2, "severity": "warning", "code": "unused", "message": "x"},
            {"path": "src/b.rs", "line": 9, "col": 1, "severity": "error", "code": 308, "message": "y"},
            {"path": "./src/a.rs", "line": 0, "severity": "hint", "message": "z"},
            {"path": "", "line": 1, "message": "no file"},
        ]});
        let got = read(&m, "/w");
        let short: Vec<(&str, usize, Severity, &str)> =
            got.iter().map(|p| (p.path.as_str(), p.line, p.severity, p.code.as_str())).collect();
        assert_eq!(
            short,
            vec![
                ("src/a.rs", 1, Severity::Hint, ""),
                ("src/b.rs", 9, Severity::Error, "308"),
                ("src/b.rs", 3, Severity::Warning, "unused"),
            ]
        );
    }

    #[test]
    fn sources_are_named_and_codes_said_as_vs_code_does() {
        assert_eq!(source_title("task:build"), "Task: build");
        assert_eq!(source_title("lsp:rust-analyzer"), "rust-analyzer");
        assert_eq!(source_title("check"), "Project Check");
        assert_eq!(origin("lsp:ts", "2322"), "ts(2322)");
        assert_eq!(origin("check", ""), "check");
        assert_eq!(counts_tip(Counts::default()), "No Problems");
        assert_eq!(counts_tip(Counts { errors: 1, warnings: 2, infos: 0 }), "Errors: 1, Warnings: 2, Infos: 0");
        assert_eq!(hidden_text(3), "3 problems hidden by the filter.");
    }

    #[test]
    fn the_filter_keeps_words_and_patterns_and_leaves_out_what_has_a_bang() {
        let a = p("src/app.ts", 1, 1, Severity::Error, "Type mismatch");
        let b = p("node_modules/x/y.ts", 1, 1, Severity::Error, "Other");
        let c = p("src/lib.rs", 1, 1, Severity::Warning, "unused variable");
        let keeps = |f: &str| {
            let f = Filter::of(f);
            [&a, &b, &c].iter().map(|p| f.keeps("check", p)).collect::<Vec<_>>()
        };
        assert_eq!(keeps(""), [true, true, true]);
        assert_eq!(keeps("mismatch"), [true, false, false]);
        assert_eq!(keeps("**/*.ts"), [true, true, false]);
        assert_eq!(keeps("*.ts, !**/node_modules/**"), [true, false, false]);
        assert_eq!(keeps("!unused"), [true, true, false]);
        assert_eq!(keeps("CHECK"), [true, true, true], "the source is looked in, whatever the case");
    }

    #[test]
    fn the_rows_are_sources_then_files_then_problems_and_a_fold_hides_its_children() {
        let mut m = BTreeMap::new();
        m.insert(
            "check".to_string(),
            Rc::new(vec![
                p("a.rs", 1, 1, Severity::Error, "e"),
                p("a.rs", 2, 1, Severity::Warning, "w"),
                p("b.rs", 1, 1, Severity::Info, "i"),
            ]),
        );
        let all = |_: &str, _: &Problem| true;
        let (r, hidden) = rows(&m, all, &HashSet::new());
        let ids: Vec<(Option<Row>, usize, u64)> =
            r.iter().map(|n| (row_of(&n.id), n.depth, if n.fold.is_some() { n.stamp } else { 0 })).collect();
        assert_eq!(hidden, 0);
        assert_eq!(
            ids,
            vec![
                (Some(Row::Source("check")), 0, 3),
                (Some(Row::File("check", "a.rs")), 1, 2),
                (Some(Row::Problem("check", 0)), 2, 0),
                (Some(Row::Problem("check", 1)), 2, 0),
                (Some(Row::File("check", "b.rs")), 1, 1),
                (Some(Row::Problem("check", 2)), 2, 0),
            ]
        );
        let folded: HashSet<String> = [file_id("check", "a.rs")].into();
        let no_infos = |_: &str, p: &Problem| p.severity != Severity::Info;
        let (r, hidden) = rows(&m, no_infos, &folded);
        assert_eq!((r.len(), hidden), (2, 1), "the source and its folded file; the info hidden");
    }

    #[test]
    fn a_squiggle_covers_the_range_or_the_word_at_its_place() {
        let lines = ["    let été = foo(bar);", "end"];
        let text_of = |l: usize| lines.get(l).map(|s| s.to_string()).unwrap_or_default();
        // The word at column 9 (characters): "été", three characters of five bytes.
        let at = p("a.rs", 1, 9, Severity::Error, "m");
        assert_eq!(range_of(&at, &text_of), ((0, 8), (0, 13)));
        // A range given, columns in characters.
        let ranged = Problem { end_line: Some(2), end_col: Some(2), ..at.clone() };
        assert_eq!(range_of(&ranged, &text_of), ((0, 8), (1, 1)));
        // No column: the line from its first word.
        let line = p("a.rs", 1, 0, Severity::Error, "m");
        assert_eq!(range_of(&line, &text_of), ((0, 4), (0, lines[0].len())));
        // At a character that is no word: that character.
        let paren = p("a.rs", 1, 18, Severity::Error, "m");
        assert_eq!(range_of(&paren, &text_of), ((0, 19), (0, 20)));
    }

    #[test]
    fn a_file_has_a_squiggle_for_each_problem_and_a_light_bulb_on_each_line() {
        let text_of = |_: usize| "fn main() {}".to_string();
        let mut e = p("a.rs", 1, 4, Severity::Error, "bad");
        e.code = "E1".into();
        let list = vec![
            ("lsp:rustc".to_string(), e),
            ("check".to_string(), p("a.rs", 1, 1, Severity::Warning, "w")),
            ("check".to_string(), p("a.rs", 3, 1, Severity::Warning, "w")),
        ];
        let marks = marks_of("a.rs", &list, &text_of);
        let hovers: Vec<String> = marks
            .iter()
            .filter_map(|m| match m {
                Mark::Range { hover, .. } => hover.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(hovers, ["bad rustc(E1)", "w check", "w check"]);
        let bulbs: Vec<usize> = marks
            .iter()
            .filter_map(|m| match m {
                Mark::Glyph { line, icon, .. } if *icon == "lightbulb-sparkle" => Some(*line),
                _ => None,
            })
            .collect();
        assert_eq!(bulbs, [0, 2]);
    }

    #[test]
    fn the_agent_is_asked_with_each_problems_place_and_message() {
        let mut e = p("src/a.rs", 12, 5, Severity::Error, "mismatched types");
        e.code = "E0308".into();
        let one = fix_words(&[("lsp:rustc".into(), e.clone())]);
        assert_eq!(one, "Fix this problem:\n- @src/a.rs:12:5 error rustc(E0308): mismatched types");
        let two = fix_words(&[("check".into(), e), ("check".into(), p("b.rs", 1, 0, Severity::Warning, "w"))]);
        assert!(two.starts_with("Fix these problems:\n") && two.ends_with("- @b.rs:1 warning check: w"));
    }

    #[test]
    fn a_source_says_its_problems_again_and_none_takes_it_away() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let m = json!({"type": "problems", "source": "check", "items": [
            {"path": "a.rs", "line": 1, "col": 1, "severity": "error", "message": "e"},
            {"path": "a.rs", "line": 2, "col": 1, "severity": "warning", "message": "w"}]});
        assert!(!take(ui, &m, &send));
        let t = json!({"type": "problems", "source": "task:build", "items": [
            {"path": "b.rs", "line": 1, "col": 1, "severity": "info", "message": "i"}]});
        take(ui, &t, &send);
        assert_eq!(counts(ui), Counts { errors: 1, warnings: 1, infos: 1 });
        assert_eq!(badges::count(ui, View::Problems).get_untracked(), 3);
        assert_eq!(of_file(ui, "a.rs").len(), 2);
        take(ui, &json!({"type": "problems", "source": "check", "items": []}), &send);
        assert_eq!(counts(ui), Counts { errors: 0, warnings: 0, infos: 1 });
        assert!(store(ui).by_source.with_untracked(|m| !m.contains_key("check")));
    }
}
