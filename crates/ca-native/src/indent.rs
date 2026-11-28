//! Indentation and ends of line: found for a file as it opens, shown in the status bar, changed there, as VS Code
//! does.
//!
//! A file's indentation is `editor.tabSize` and `editor.insertSpaces` for its language, or, with
//! `editor.detectIndentation` on, what its first lines are indented with (VS Code's guess: the lines begun with a
//! tab against those begun with spaces, and the most common step between one line's spaces and the next's). Tab,
//! Enter, Indent and Outdent use it (`editor/editing.rs` asks `unit` and `eol`), and a tab is drawn that wide.
//!
//! The status bar says it ("Spaces: 4", "Tab Size: 4"); a click picks Indent Using Spaces or Tabs, Change Tab
//! Display Size, Detect Indentation from Content, or Convert Indentation to Spaces or Tabs. What is picked stays
//! with the file until one of those three settings changes for its language, when the file's indentation is worked
//! out again. Its line ending ("LF", "CRLF") is the one most of its lines end with, or `files.eol` for a file with
//! one line (`auto`: the system's); a click picks another, and the file is saved with every line ending so
//! (`written`). Floem 0.2 fixes a document's ending when it is made and writes what is typed and pasted with it,
//! so the text on screen keeps its own until the file is opened again, and Undo does not take the pick back. The
//! encoding item says what is so: only UTF-8 is read and written.
//!
//! What every part has (`register`, `take`) is said in `ext.rs`.

use std::collections::HashMap;
use std::rc::Rc;

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::text::Styling;
use floem::views::{Decorators, label};
use serde_json::Value;

use crate::editor::editing::{Change, Sel};
use crate::state::{Send, Ui};
use crate::widgets::quick;

/// How a file is indented: with spaces or tabs, and how many columns one is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Indent {
    pub spaces: bool,
    pub size: usize,
}

impl Indent {
    /// What the status bar says of it.
    pub fn label(self) -> String {
        if self.spaces { format!("Spaces: {}", self.size) } else { format!("Tab Size: {}", self.size) }
    }

    /// One indent's text.
    pub fn unit(self) -> String {
        if self.spaces { " ".repeat(self.size) } else { "\t".into() }
    }
}

/// A line ending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Lf,
    CrLf,
}

impl Eol {
    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::CrLf => "CRLF",
        }
    }
}

/// What the settings say of a file's indentation, for its language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Said {
    pub size: usize,
    pub spaces: bool,
    pub detect: bool,
}

/// An open file's indentation and line ending, and what they were worked out from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Doc {
    indent: Indent,
    eol: Eol,
    said: Said,
    /// The document it is of (a tab opened again is another one, and is looked at again).
    doc: usize,
}

/// Each open file's, by its path.
#[derive(Clone, Copy)]
pub struct Docs {
    docs: RwSignal<HashMap<String, Doc>>,
}

impl crate::ext::State for Docs {
    fn new(ui: Ui) -> Docs {
        Docs { docs: ui.scope.create_rw_signal(HashMap::new()) }
    }
}

// ---- Worked out from the text. ----

/// The most lines looked at to guess, as VS Code.
const LOOKED_AT: usize = 10_000;

/// The text's indentation as its lines show it: spaces or tabs (none when no line is indented, or as many each
/// way), and the step between lines indented with spaces (none when there is none).
pub fn guess(text: &str) -> (Option<bool>, Option<usize>) {
    let (mut tabs, mut spaces) = (0usize, 0usize);
    let mut steps = [0usize; 9];
    let mut before: Option<usize> = None;
    for line in text.split('\n').take(LOOKED_AT) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let lead = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
        if lead.len() == line.len() {
            continue; // blank: says nothing
        }
        if lead.starts_with('\t') {
            tabs += 1;
        } else if lead.starts_with(' ') {
            spaces += 1;
        }
        let only_spaces = !lead.contains('\t');
        if only_spaces {
            if let Some(b) = before {
                let step = lead.len().abs_diff(b);
                if (1..steps.len()).contains(&step) {
                    steps[step] += 1;
                }
            }
            before = Some(lead.len());
        } else {
            before = None;
        }
    }
    let use_spaces = (tabs != spaces).then_some(spaces > tabs);
    let mut size = None;
    let mut best = 0;
    for n in [2, 4, 6, 8, 3, 5, 7] {
        if steps[n] > best {
            best = steps[n];
            size = Some(n);
        }
    }
    // Two wins over four when it is nearly as common (a file indented by two with blocks of four).
    if size == Some(4) && steps[2] > 0 && steps[2] * 2 >= steps[4] {
        size = Some(2);
    }
    (use_spaces, size)
}

/// A file's indentation: what the settings say, or with detection on, what its lines show where they show it.
pub fn resolve(text: &str, said: Said) -> Indent {
    let configured = Indent { spaces: said.spaces, size: said.size };
    if !said.detect {
        return configured;
    }
    let (spaces, size) = guess(text);
    let spaces = spaces.unwrap_or(said.spaces);
    // Tabs keep the size set; spaces take the step the lines show.
    let size = if spaces { size.unwrap_or(said.size) } else { said.size };
    Indent { spaces, size }
}

/// The line ending most of the text's lines end with, or `none` for a text of one line.
pub fn eol_of(text: &str, none: Eol) -> Eol {
    let lf = text.bytes().filter(|b| *b == b'\n').count();
    if lf == 0 {
        return none;
    }
    let crlf = text.matches("\r\n").count();
    if crlf * 2 > lf { Eol::CrLf } else { Eol::Lf }
}

/// Where an offset in the text is once the edits (in order, none overlapping) are made.
fn moved(at: usize, edits: &[(usize, usize, String)]) -> usize {
    let mut by: isize = 0;
    for (from, to, with) in edits {
        if *to <= at {
            by += with.len() as isize - (to - from) as isize;
        } else if *from < at {
            return (*from as isize + by) as usize + with.len();
        } else {
            break;
        }
    }
    (at as isize + by) as usize
}

fn change(edits: Vec<(usize, usize, String)>, sels: &[Sel]) -> Option<Change> {
    if edits.is_empty() {
        return None;
    }
    let sels = sels.iter().map(|s| Sel::new(moved(s.anchor, &edits), moved(s.head, &edits))).collect();
    Some(Change { edits, sels })
}

/// The text with every line ending `to` (it is given back as it is when they all are).
pub fn with_eol(text: String, to: Eol) -> String {
    let lf = text.bytes().filter(|b| *b == b'\n').count();
    let crlf = text.matches("\r\n").count();
    match to {
        Eol::Lf if crlf > 0 => text.replace("\r\n", "\n"),
        Eol::CrLf if crlf < lf => text.replace("\r\n", "\n").replace('\n', "\r\n"),
        _ => text,
    }
}

/// How many columns a line's leading spaces and tabs take, a tab to the next stop.
fn columns(lead: &str, size: usize) -> usize {
    lead.chars().fold(0, |col, c| if c == '\t' { col + size - col % size } else { col + 1 })
}

/// Every line's leading spaces and tabs made spaces only, or tabs then fewer spaces than a tab, a tab `size` wide.
pub fn convert(text: &str, sels: &[Sel], spaces: bool, size: usize) -> Option<Change> {
    let size = size.max(1);
    let mut edits = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let lead_len = line.len() - line.trim_start_matches([' ', '\t']).len();
        let lead = &line[..lead_len];
        let width = columns(lead, size);
        let want = if spaces {
            " ".repeat(width)
        } else {
            format!("{}{}", "\t".repeat(width / size), " ".repeat(width % size))
        };
        // A line of nothing but spaces and tabs is left as it is, as VS Code does.
        let blank = line.trim_end_matches(['\n', '\r']).len() == lead_len;
        if !blank && want != lead {
            edits.push((start, start + lead_len, want));
        }
        start += line.len();
    }
    change(edits, sels)
}

/// Tab in a line: an indent where each caret is, spaces to the next stop or a tab, in place of a selection.
/// `before` gives a line's text before an offset in it (only that is read, not the whole text).
pub fn tab(before: impl Fn(usize) -> String, sels: &[Sel], indent: Indent) -> Option<Change> {
    let mut sorted = sels.to_vec();
    sorted.sort_by_key(|s| s.min());
    sorted.dedup_by_key(|s| (s.min(), s.max()));
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for s in &sorted {
        let (lo, hi) = (s.min(), s.max());
        let col = columns(&before(lo).replace(|c: char| c != '\t', " "), indent.size);
        let with = if indent.spaces { " ".repeat(indent.size - col % indent.size) } else { "\t".into() };
        if edits.last().is_some_and(|(_, to, _)| *to > lo) {
            continue;
        }
        edits.push((lo, hi, with));
    }
    if edits.is_empty() {
        return None;
    }
    let mut by = 0isize;
    let sels = edits
        .iter()
        .map(|(from, to, with)| {
            let at = (*from as isize + by) as usize + with.len();
            by += with.len() as isize - (to - from) as isize;
            Sel::caret(at)
        })
        .collect();
    Some(Change { edits, sels })
}

// ---- A file's, in the window. ----

/// What the settings say for a file, by its language.
pub fn said(ui: Ui, path: &str) -> Said {
    let l = crate::language::of(ui, path);
    Said {
        size: crate::settings::now(ui, "editor.tabSize", l).as_u64().unwrap_or(4).clamp(1, 32) as usize,
        spaces: crate::settings::now(ui, "editor.insertSpaces", l).as_bool().unwrap_or(true),
        detect: crate::settings::now(ui, "editor.detectIndentation", l).as_bool().unwrap_or(true),
    }
}

/// The line ending of a new file (`files.eol`), for its language.
pub fn new_eol(ui: Ui, path: &str) -> Eol {
    match crate::settings::now(ui, "files.eol", crate::language::of(ui, path)).as_str() {
        Some("\r\n") => Eol::CrLf,
        Some("\n") => Eol::Lf,
        _ if cfg!(windows) => Eol::CrLf,
        _ => Eol::Lf,
    }
}

/// A file's indentation and line ending as they are kept, or worked out from its text when they are not.
fn doc_of(ui: Ui, path: &str) -> (Indent, Eol) {
    if let Some(d) = ui.state::<Docs>().docs.with_untracked(|d| d.get(path).copied()) {
        return (d.indent, d.eol);
    }
    let text = ui.tab(path).map(|t| t.text()).unwrap_or_default();
    (resolve(&text, said(ui, path)), eol_of(&text, new_eol(ui, path)))
}

/// A file's indentation.
pub fn indent(ui: Ui, path: &str) -> Indent {
    doc_of(ui, path).0
}

/// One indent of a file, and a tab's width: what Tab, Indent and Outdent put in or take out.
pub fn unit(ui: Ui, path: &str) -> (String, usize) {
    let i = indent(ui, path);
    (i.unit(), i.size)
}

/// A file's line ending.
pub fn eol(ui: Ui, path: &str) -> Eol {
    doc_of(ui, path).1
}

/// A file's text as it is saved: every line ending its own. (A document's ending is fixed when it is made in Floem
/// 0.2, which writes what is typed and pasted with it, so the one picked or `files.eol`'s is put in here.)
pub fn written(ui: Ui, path: &str, text: String) -> String {
    with_eol(text, eol(ui, path))
}

/// A tab drawn as wide as the file's indentation says, in every editor of it.
fn draw_tabs(ui: Ui, path: &str, size: usize) {
    let Some(tab) = ui.tab(path) else { return };
    if tab.colours.tab_width_is(size) {
        return;
    }
    tab.colours.set_tab_width(size);
    if let Some((p, e)) = ui.live.get_untracked()
        && p == path
    {
        e.update_styling(tab.colours.clone() as Rc<dyn Styling>);
    }
}

/// A file's indentation kept (set only when it is another).
fn keep(ui: Ui, path: &str, doc: Doc) {
    let docs = ui.state::<Docs>().docs;
    if docs.with_untracked(|d| d.get(path) != Some(&doc)) {
        docs.update(|d| {
            d.insert(path.to_string(), doc);
        });
    }
    draw_tabs(ui, path, doc.indent.size);
}

/// A file's text taken in: a tab opened is looked at; one read again keeps what was picked for it, and takes the
/// line ending the file has on disk (its document keeps its own, see `written`).
fn opened(ui: Ui, path: &str, text: &str) {
    let Some(tab) = ui.tab(path).filter(|t| t.is_file()) else { return };
    let doc = Rc::as_ptr(&tab.doc) as *const () as usize;
    let kept = ui.state::<Docs>().docs.with_untracked(|d| d.get(path).copied()).filter(|d| d.doc == doc);
    let said = said(ui, path);
    let new = match kept {
        Some(k) => Doc { eol: eol_of(text, k.eol), ..k },
        None => Doc { indent: resolve(text, said), eol: eol_of(text, new_eol(ui, path)), said, doc },
    };
    keep(ui, path, new);
}

/// The settings changed: a file whose indentation settings are others is worked out again, what was picked for it
/// gone.
fn settings_changed(ui: Ui) {
    let all: Vec<(String, Doc)> =
        ui.state::<Docs>().docs.with_untracked(|d| d.iter().map(|(p, d)| (p.clone(), *d)).collect());
    for (path, d) in all {
        let now = said(ui, &path);
        if now != d.said {
            let text = ui.tab(&path).map(|t| t.text()).unwrap_or_default();
            keep(ui, &path, Doc { indent: resolve(&text, now), said: now, ..d });
        }
    }
}

/// The editor drawn and its file's path, when it is a file's text.
fn live(ui: Ui) -> Option<(String, floem::views::editor::Editor)> {
    ui.live.get_untracked()
}

/// What the file shown is indented with, picked: kept until its settings change.
fn choose(ui: Ui, path: &str, indent: Indent) {
    let (_, eol) = doc_of(ui, path);
    let old = ui.state::<Docs>().docs.with_untracked(|d| d.get(path).copied());
    let said = old.map_or_else(|| said(ui, path), |d| d.said);
    let doc = old.map_or(0, |d| d.doc);
    keep(ui, path, Doc { indent, eol, said, doc });
}

/// A change made to the file shown, in one step.
fn edit(ui: Ui, work: impl FnOnce(&str, &[Sel]) -> Option<Change>) {
    crate::editor::editing::edit_with(ui, EditType::Other, |_, text, sels| work(&text.to_string(), sels));
}

// ---- What a person picks. ----

pub const USE_SPACES: &str = "editor.action.indentUsingSpaces";
pub const USE_TABS: &str = "editor.action.indentUsingTabs";
pub const TAB_DISPLAY: &str = "editor.action.changeTabDisplaySize";
pub const DETECT: &str = "editor.action.detectIndentation";
pub const TO_SPACES: &str = "editor.action.indentationToSpaces";
pub const TO_TABS: &str = "editor.action.indentationToTabs";
pub const CHANGE_EOL: &str = "workbench.action.editor.changeEOL";
pub const CHANGE_ENCODING: &str = "workbench.action.editor.changeEncoding";
pub const PICK_INDENT: &str = "changeEditorIndentation";

/// The tab size picked for the file shown, then what is done with it.
fn pick_size(ui: Ui, then: impl FnOnce(Ui, String, usize) + 'static) {
    let Some((path, _)) = live(ui) else { return };
    let now = indent(ui, &path).size;
    let configured = said(ui, &path).size;
    let items = (1..=8)
        .map(|n| {
            let it = quick::item(&n.to_string(), &n.to_string());
            if n == configured {
                it.description("Configured Tab Size")
            } else if n == now {
                it.description("Current Tab Size")
            } else {
                it
            }
        })
        .collect();
    quick::pick("Select Tab Size for the Current File")
        .placeholder("Select Tab Size for the Current File")
        .items(items)
        .active(&now.to_string())
        .open(ui, move |it| {
            if let Some(n) = it.and_then(|i| i.id.parse().ok()) {
                then(ui, path, n);
            }
        });
}

fn use_spaces(ui: Ui, spaces: bool) {
    pick_size(ui, move |ui, path, size| choose(ui, &path, Indent { spaces, size }));
}

fn tab_display(ui: Ui) {
    pick_size(ui, |ui, path, size| {
        let spaces = indent(ui, &path).spaces;
        choose(ui, &path, Indent { spaces, size });
    });
}

fn detect(ui: Ui) {
    let Some((path, e)) = live(ui) else { return };
    let text = e.doc().text().to_string();
    choose(ui, &path, resolve(&text, Said { detect: true, ..said(ui, &path) }));
}

fn to(ui: Ui, spaces: bool) {
    let Some((path, _)) = live(ui) else { return };
    let size = indent(ui, &path).size;
    edit(ui, |t, s| convert(t, s, spaces, size));
    choose(ui, &path, Indent { spaces, size });
}

/// The status bar's indentation picker.
fn pick_indent(ui: Ui, send: Send) {
    let view = |id: &str, label: &str| quick::item(id, label).group("change view");
    let file = |id: &str, label: &str| quick::item(id, label).group("convert file");
    quick::pick("Select Action")
        .placeholder("Select Action")
        .items(vec![
            view(USE_SPACES, "Indent Using Spaces"),
            view(USE_TABS, "Indent Using Tabs"),
            view(TAB_DISPLAY, "Change Tab Display Size"),
            view(DETECT, "Detect Indentation from Content"),
            file(TO_SPACES, "Convert Indentation to Spaces"),
            file(TO_TABS, "Convert Indentation to Tabs"),
        ])
        .open(ui, move |it| {
            if let Some(it) = it {
                crate::commands::run(ui, &send, &it.id);
            }
        });
}

fn pick_eol(ui: Ui) {
    let Some((path, _)) = live(ui) else { return };
    let now = doc_of(ui, &path).1;
    quick::pick("Select End of Line Sequence")
        .placeholder("Select End of Line Sequence")
        .items(vec![quick::item("lf", "LF"), quick::item("crlf", "CRLF")])
        .active(if now == Eol::CrLf { "crlf" } else { "lf" })
        .open(ui, move |it| {
            let Some(it) = it else { return };
            set_eol(ui, &path, if it.id == "crlf" { Eol::CrLf } else { Eol::Lf });
        });
}

/// A file's line ending picked: what it is saved with, so the file has changes to save when it is another.
fn set_eol(ui: Ui, path: &str, to: Eol) {
    let (indent, now) = doc_of(ui, path);
    if now == to {
        return;
    }
    let old = ui.state::<Docs>().docs.with_untracked(|d| d.get(path).copied());
    let doc = old.map_or(0, |d| d.doc);
    let said = old.map_or_else(|| said(ui, path), |d| d.said);
    keep(ui, path, Doc { indent, eol: to, said, doc });
    if let Some(tab) = ui.tab(path)
        && !tab.dirty.get_untracked()
    {
        tab.dirty.set(true);
    }
}

fn pick_encoding(ui: Ui) {
    quick::pick("Select File Encoding")
        .placeholder("Select Action")
        .items(vec![quick::item("utf8", "UTF-8").description("utf8").detail("Only UTF-8 is read and written")])
        .active("utf8")
        .open(ui, |_| {});
}

/// The file shown: a text file's tab.
fn shown(ui: Ui) -> bool {
    crate::editor::kinds::text_shown(ui)
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::statusbar::{Side, item};
    let on = |c: crate::commands::Command| add(c.when(shown));
    on(command(USE_SPACES, "", "Indent Using Spaces", |ui, _| use_spaces(ui, true)));
    on(command(USE_TABS, "", "Indent Using Tabs", |ui, _| use_spaces(ui, false)));
    on(command(TAB_DISPLAY, "", "Change Tab Display Size", |ui, _| tab_display(ui)));
    on(command(DETECT, "", "Detect Indentation from Content", |ui, _| detect(ui)));
    on(command(TO_SPACES, "", "Convert Indentation to Spaces", |ui, _| to(ui, true)));
    on(command(TO_TABS, "", "Convert Indentation to Tabs", |ui, _| to(ui, false)));
    on(command(CHANGE_EOL, "", "Change End of Line Sequence", |ui, _| pick_eol(ui)));
    on(command(CHANGE_ENCODING, "", "Change File Encoding", |ui, _| pick_encoding(ui)));
    add(command(PICK_INDENT, "", "Change Editor Indentation", |ui, send| pick_indent(ui, send.clone())).hidden());

    crate::statusbar::add("status.indentation", Side::Right, 210, |ui, send| {
        let (t, docs) = (ui.colors, ui.state::<Docs>().docs);
        label(move || {
            let Some(path) = ui.active.get() else { return String::new() };
            match docs.with(|d| d.get(&path).map(|d| d.indent)) {
                Some(i) => i.label(),
                None => indent(ui, &path).label(),
            }
        })
        .on_click_stop(move |_| pick_indent(ui, send.clone()))
        .style(move |s| item(t, s).apply_if(!shown(ui), |s| s.hide()))
        .into_any()
    });
    crate::statusbar::add("status.encoding", Side::Right, 220, |ui, _| {
        let t = ui.colors;
        label(|| "UTF-8")
            .on_click_stop(move |_| pick_encoding(ui))
            .style(move |s| item(t, s).apply_if(!shown(ui), |s| s.hide()))
            .into_any()
    });
    crate::statusbar::add("status.eol", Side::Right, 230, |ui, _| {
        let (t, docs) = (ui.colors, ui.state::<Docs>().docs);
        label(move || {
            let Some(path) = ui.active.get() else { return "" };
            match docs.with(|d| d.get(&path).map(|d| d.eol)) {
                Some(e) => e.label(),
                None => doc_of(ui, &path).1.label(),
            }
        })
        .on_click_stop(move |_| pick_eol(ui))
        .style(move |s| item(t, s).apply_if(!shown(ui), |s| s.hide()))
        .into_any()
    });

    crate::ext::after("indent", |ui, m, _| match m["type"].as_str() {
        Some("file") if m["text"].is_string() => {
            opened(ui, m["path"].as_str().unwrap_or(""), m["text"].as_str().unwrap_or(""))
        }
        Some("settings") => settings_changed(ui),
        _ => {}
    });
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line's text before an offset, in a text.
    fn line(text: &str) -> impl Fn(usize) -> String + '_ {
        move |at| text[text[..at].rfind('\n').map_or(0, |n| n + 1)..at].to_string()
    }

    fn run(text: &str, c: Option<Change>) -> (String, Vec<Sel>) {
        let c = c.expect("a change");
        (c.applied(text), c.sels)
    }

    const SAID: Said = Said { size: 4, spaces: true, detect: true };

    #[test]
    fn indentation_is_guessed_from_the_lines_and_the_settings_hold_where_they_say_nothing() {
        assert_eq!(guess("a\n\tb\n\t\tc\n"), (Some(false), None));
        assert_eq!(guess("a {\n  b {\n    c\n  }\n}\n"), (Some(true), Some(2)));
        assert_eq!(guess("a:\n    b\n    c:\n        d\n"), (Some(true), Some(4)));
        assert_eq!(guess("one\ntwo\n"), (None, None));
        // Detected: spaces take the step shown, tabs keep the size set.
        assert_eq!(resolve("x\n  y\n", SAID), Indent { spaces: true, size: 2 });
        assert_eq!(resolve("x\n\ty\n", Said { size: 3, ..SAID }), Indent { spaces: false, size: 3 });
        assert_eq!(resolve("x\ny\n", Said { spaces: false, ..SAID }), Indent { spaces: false, size: 4 });
        // Not detected: the settings, whatever the lines show.
        assert_eq!(resolve("x\n\ty\n", Said { detect: false, ..SAID }), Indent { spaces: true, size: 4 });
        assert_eq!(
            (Indent { spaces: true, size: 4 }.label(), Indent { spaces: false, size: 8 }.label()),
            ("Spaces: 4".into(), "Tab Size: 8".into())
        );
    }

    #[test]
    fn a_files_line_ending_is_most_of_its_lines_and_files_eol_for_one_line() {
        assert_eq!(eol_of("a\r\nb\r\nc\n", Eol::Lf), Eol::CrLf);
        assert_eq!(eol_of("a\nb\nc\r\n", Eol::CrLf), Eol::Lf);
        assert_eq!(eol_of("", Eol::CrLf), Eol::CrLf);
        assert_eq!(eol_of("one line", Eol::Lf), Eol::Lf);
    }

    #[test]
    fn a_text_is_written_with_one_line_ending() {
        assert_eq!(with_eol("a\r\nb\nc\r\n".into(), Eol::Lf), "a\nb\nc\n");
        assert_eq!(with_eol("a\nb\r\nc".into(), Eol::CrLf), "a\r\nb\r\nc");
        assert_eq!(with_eol("a\r\nb".into(), Eol::CrLf), "a\r\nb");
        assert_eq!(with_eol("one".into(), Eol::CrLf), "one");
    }

    #[test]
    fn indentation_converts_to_spaces_or_tabs_and_blank_lines_are_left() {
        let text = "a\n\tb\n\t  c\n  \t\n";
        let (t, _) = run(text, convert(text, &[], true, 4));
        assert_eq!(t, "a\n    b\n      c\n  \t\n");
        let text = "a\n    b\n      c\n";
        let (t, s) = run(text, convert(text, &[Sel::caret(10)], false, 4));
        // A caret in the indentation changed goes to its end.
        assert_eq!((t.as_str(), s), ("a\n\tb\n\t  c\n", vec![Sel::caret(8)]));
        assert!(convert("a\n\tb\n", &[], false, 4).is_none());
    }

    #[test]
    fn tab_puts_spaces_to_the_next_stop_or_a_tab_in_place_of_a_selection() {
        let four = Indent { spaces: true, size: 4 };
        assert_eq!(run("ab", tab(line("ab"), &[Sel::caret(2)], four)), ("ab  ".into(), vec![Sel::caret(4)]));
        assert_eq!(run("\tx", tab(line("\tx"), &[Sel::caret(1)], four)), ("\t    x".into(), vec![Sel::caret(5)]));
        let tabs = Indent { spaces: false, size: 4 };
        assert_eq!(run("abc", tab(line("abc"), &[Sel::new(1, 2)], tabs)), ("a\tc".into(), vec![Sel::caret(2)]));
        let two = Indent { spaces: true, size: 2 };
        let (t, s) = run("x\ny", tab(line("x\ny"), &[Sel::caret(0), Sel::caret(2)], two));
        assert_eq!((t.as_str(), s), ("  x\n  y", vec![Sel::caret(2), Sel::caret(6)]));
    }

    #[test]
    fn a_line_ending_picked_is_what_the_file_is_saved_with() {
        use serde_json::json;
        let ui = Ui::new();
        let told = Rc::new(std::cell::RefCell::new(Vec::<Value>::new()));
        let send: Send = {
            let to = told.clone();
            Rc::new(move |v: Value| to.borrow_mut().push(v))
        };
        register(ui, &send);
        let path = "/tmp/eol-test.txt";
        ui.take(&json!({"type": "file", "path": path, "text": "a\r\nb\r\n", "modified": 1}), &send);
        assert_eq!(eol(ui, path), Eol::CrLf);
        set_eol(ui, path, Eol::Lf);
        let tab = ui.tab(path).expect("open");
        assert!(tab.dirty.get_untracked(), "another ending is a change to save");
        ui.save_tab(path, &send);
        assert_eq!(told.borrow().last().expect("saved")["text"], "a\nb\n");
        // A new empty file: files.eol's.
        let empty = "/tmp/eol-empty.txt";
        ui.take(&json!({"type": "file", "path": empty, "text": "", "modified": 1}), &send);
        assert_eq!(eol(ui, empty), if cfg!(windows) { Eol::CrLf } else { Eol::Lf });
    }

    #[test]
    fn a_file_opened_is_looked_at_and_what_is_picked_stays_until_its_settings_change() {
        use serde_json::json;
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        register(ui, &send);
        let path = "/tmp/indent-test.py";
        ui.take(&json!({"type": "file", "path": path, "text": "if a:\r\n  b\r\n", "modified": 1}), &send);
        assert_eq!(indent(ui, path), Indent { spaces: true, size: 2 });
        assert_eq!((unit(ui, path), eol(ui, path)), (("  ".to_string(), 2), Eol::CrLf));
        // Picked: kept when the file is read again, and when other settings change.
        choose(ui, path, Indent { spaces: false, size: 8 });
        ui.take(&json!({"type": "file", "path": path, "text": "if a:\n  b\n", "modified": 2}), &send);
        assert_eq!((indent(ui, path), eol(ui, path)), (Indent { spaces: false, size: 8 }, Eol::Lf));
        let settings = |values: Value| json!({"type": "settings", "values": values, "by_lang": {}, "sources": {}, "problems": [], "files": {}});
        ui.take(&settings(json!({"editor.fontSize": 15})), &send);
        assert_eq!(indent(ui, path).size, 8);
        // Its indentation's settings changed: worked out again from them.
        ui.take(&settings(json!({"editor.detectIndentation": false, "editor.tabSize": 3})), &send);
        assert_eq!(indent(ui, path), Indent { spaces: true, size: 3 });
    }
}
