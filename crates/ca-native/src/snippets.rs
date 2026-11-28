//! Snippets, as VS Code's: the person's (`snippets/NAME.json` for a language, `snippets/NAME.code-snippets` for any)
//! and the project's (`.vscode/NAME.code-snippets`), read from the session's `snippets` message, again when one of
//! them is saved or changes on disk.
//!
//! - Suggested as their prefixes are typed (`complete.rs`, labelled "User Snippet" or "Workspace Snippet", by the
//!   file's language, `scope`, `include` and `exclude`; `editor.snippetSuggestions: none` hides them); taking one puts
//!   its body in place of the word typed.
//! - A body is VS Code's snippet syntax (`body.rs`): Tab and Shift+Tab go through its tab stops, a stop's mirrors are
//!   selected with it (typing changes them all), a mirror's transform is filled in when Tab leaves its stop, a stop's
//!   choices are suggested when Tab comes to it, and the last Tab goes to `$0` and ends it. Escape ends it, as does
//!   the caret leaving the snippet.
//! - Snippets: Insert Snippet and Surround With Snippet (only those that take `$TM_SELECTED_TEXT` or `$SELECTION`)
//!   pick one for the file's language; Snippets: Configure Snippets makes or opens a snippets file (`snippet_new`).

// A stop's places are a list of ranges, often of one.
#![allow(clippy::single_range_in_vec_init)]

mod body;
mod list;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{SignalGet, SignalTrack, SignalUpdate, SignalWith};
use floem::views::editor::Editor;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::cursor::CursorMode;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::{SelRegion, Selection};
use floem::views::editor::core::xi_rope::delta::Transformer;
use serde_json::{Value, json};

use self::body::{Fill, Stop};
use self::list::{Snippet, Source};
use crate::complete::{self, Item, Kind};
use crate::state::{Send, Ui};
use crate::widgets::quick;

/// How the status bar's message about a snippets file not read begins.
const NOT_READ: &str = "Could not read the snippets of";

/// A snippets file the session said: where it is from, its path (`snippets/NAME`, `.vscode/NAME.code-snippets`),
/// and its language (a person's `NAME.json`'s).
#[derive(Clone, Debug)]
struct File {
    source: Source,
    path: String,
    language: Option<String>,
}

/// A snippet being filled in: in which editor, its tab stops as they are now, the one at, and the whole of it.
struct Session {
    path: String,
    doc: usize,
    editor: Editor,
    stops: Vec<Stop>,
    at: usize,
    whole: Range<usize>,
}

thread_local! {
    static ALL: RefCell<Rc<Vec<Snippet>>> = RefCell::new(Rc::new(Vec::new()));
    static FILES: RefCell<Rc<Vec<File>>> = RefCell::new(Rc::new(Vec::new()));
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    /// The documents whose edits move a session's stops (a document's hook is added once).
    static HOOKED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    static ASKED: Cell<bool> = const { Cell::new(false) };
    /// The snippets file asked to be made, opened when the session says it is there.
    static MAKING: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The stops' places moved by an edit: `map(offset, after)` is where an offset is after it (`after`: an insertion
/// at the offset goes before it). The stop at grows with what is typed at its ends; the others do not.
fn shift(stops: &mut [Stop], at: usize, whole: &mut Range<usize>, map: &mut dyn FnMut(usize, bool) -> usize) {
    fn one(r: &mut Range<usize>, grows: bool, map: &mut dyn FnMut(usize, bool) -> usize) {
        let (s, e) = (map(r.start, !grows), map(r.end, grows));
        *r = s..e.max(s);
    }
    for (i, stop) in stops.iter_mut().enumerate() {
        for r in &mut stop.ranges {
            one(r, i == at, map);
        }
        for (r, _) in &mut stop.derived {
            one(r, false, map);
        }
    }
    one(whole, true, map);
}

/// The selection of a stop: each of its places, the caret at its end.
fn selection_of(stop: &Stop) -> Selection {
    let mut sel = Selection::new();
    for r in &stop.ranges {
        sel.add_region(SelRegion::new(r.start, r.end, None));
    }
    sel
}

/// The edits filling in a stop's transformed mirrors from what was typed in it.
fn derived_edits(stop: &Stop, text: &str) -> Vec<(Range<usize>, String)> {
    let typed = stop.ranges.first().and_then(|r| text.get(r.clone())).unwrap_or("");
    stop.derived.iter().map(|(r, t)| (r.clone(), body::apply(t, typed))).collect()
}

fn in_session(ui: Ui) -> bool {
    let live = ui.live.with_untracked(|l| l.as_ref().map(|(p, _)| p.clone()));
    SESSION.with(|s| s.borrow().as_ref().is_some_and(|s| Some(&s.path) == live.as_ref()))
}

fn has_next(ui: Ui) -> bool {
    in_session(ui) && SESSION.with(|s| s.borrow().as_ref().is_some_and(|s| s.at + 1 < s.stops.len()))
}

fn has_prev(ui: Ui) -> bool {
    in_session(ui) && SESSION.with(|s| s.borrow().as_ref().is_some_and(|s| s.at > 0))
}

fn leave() {
    SESSION.with(|s| s.borrow_mut().take());
}

/// A document's edits followed, once for each document: they move the stops of the session in it.
fn hook(ui: Ui, path: &str) -> Option<usize> {
    let doc = ui.tab(path)?.doc;
    let id = Rc::as_ptr(&doc) as usize;
    if HOOKED.with(|h| h.borrow_mut().insert(id)) {
        doc.add_on_update(move |u| {
            SESSION.with(|s| {
                // An edit made while the session is read elsewhere is the session's own: it was taken already.
                let Ok(mut s) = s.try_borrow_mut() else { return };
                let Some(s) = s.as_mut().filter(|s| s.doc == id) else { return };
                for delta in u.deltas() {
                    let mut t = Transformer::new(delta);
                    shift(&mut s.stops, s.at, &mut s.whole, &mut |ix, after| t.transform(ix, after));
                }
            })
        });
    }
    Some(id)
}

/// The values of the variables for a snippet put in at `at` of a file, `selected` the text it takes the place of.
fn variables(ui: Ui, path: &str, editor: &Editor, at: usize, selected: &str) -> HashMap<&'static str, String> {
    let (line, col) = editor.offset_to_line_col(at);
    let text = editor.rope_text().line_content(line).trim_end_matches(['\n', '\r']).to_string();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let col = col.min(text.len());
    let from = text[..col].rfind(|c: char| !is_word(c)).map_or(0, |i| i + 1);
    let to = text[col..].find(|c: char| !is_word(c)).map_or(text.len(), |i| col + i);
    let folder = ui.folder.get_untracked();
    let full = if path.starts_with('/') || folder.is_empty() { path.to_string() } else { format!("{folder}/{path}") };
    let name = full.rsplit('/').next().unwrap_or(&full).to_string();
    let base = name.rsplit_once('.').map_or(name.clone(), |(b, _)| b.to_string());
    let dir = full.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string());
    let relative = full.strip_prefix(&format!("{folder}/")).unwrap_or(&full).to_string();
    let now = chrono::Local::now();
    let seed = std::hash::BuildHasher::hash_one(&std::collections::hash_map::RandomState::new(), at);
    let mut rand = seed;
    let mut next = move || {
        rand ^= rand << 13;
        rand ^= rand >> 7;
        rand ^= rand << 17;
        rand
    };
    let uuid = {
        let (a, b) = (next(), next());
        format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            a >> 32,
            (a >> 16) & 0xffff,
            a & 0xfff,
            (b >> 48) & 0x3fff | 0x8000,
            b & 0xffff_ffff_ffff
        )
    };
    let tokens = crate::editor::editing::tokens_of(path);
    let mut v: HashMap<&'static str, String> = HashMap::new();
    v.insert("TM_SELECTED_TEXT", selected.to_string());
    v.insert("SELECTION", selected.to_string());
    v.insert("TM_CURRENT_LINE", text.clone());
    v.insert("TM_CURRENT_WORD", text[from..to.max(from)].to_string());
    v.insert("TM_LINE_INDEX", line.to_string());
    v.insert("TM_LINE_NUMBER", (line + 1).to_string());
    v.insert("TM_FILENAME", name);
    v.insert("TM_FILENAME_BASE", base);
    v.insert("TM_DIRECTORY", dir);
    v.insert("TM_FILEPATH", full.clone());
    v.insert("RELATIVE_FILEPATH", relative);
    v.insert("CLIPBOARD", floem::Clipboard::get_contents().unwrap_or_default());
    v.insert("WORKSPACE_NAME", folder.rsplit('/').next().unwrap_or("").to_string());
    v.insert("WORKSPACE_FOLDER", folder.clone());
    v.insert("CURSOR_INDEX", "0".into());
    v.insert("CURSOR_NUMBER", "1".into());
    for (key, f) in [
        ("CURRENT_YEAR", "%Y"),
        ("CURRENT_YEAR_SHORT", "%y"),
        ("CURRENT_MONTH", "%m"),
        ("CURRENT_MONTH_NAME", "%B"),
        ("CURRENT_MONTH_NAME_SHORT", "%b"),
        ("CURRENT_DATE", "%d"),
        ("CURRENT_DAY_NAME", "%A"),
        ("CURRENT_DAY_NAME_SHORT", "%a"),
        ("CURRENT_HOUR", "%H"),
        ("CURRENT_MINUTE", "%M"),
        ("CURRENT_SECOND", "%S"),
    ] {
        v.insert(key, now.format(f).to_string());
    }
    v.insert("CURRENT_SECONDS_UNIX", now.timestamp().to_string());
    v.insert("CURRENT_TIMEZONE_OFFSET", now.format("%:z").to_string());
    v.insert("RANDOM", format!("{:06}", next() % 1_000_000));
    v.insert("RANDOM_HEX", format!("{:06x}", next() & 0xff_ffff));
    v.insert("UUID", uuid);
    v.insert("LINE_COMMENT", tokens.line.unwrap_or("").to_string());
    v.insert("BLOCK_COMMENT_START", tokens.block.map_or("", |b| b.0).to_string());
    v.insert("BLOCK_COMMENT_END", tokens.block.map_or("", |b| b.1).to_string());
    v
}

/// A snippet's body put in place of `range` of a file's editor, its first tab stop selected.
fn insert(ui: Ui, path: &str, editor: &Editor, range: Range<usize>, body_text: &str) {
    if editor.cursor.try_get_untracked().is_none() {
        return;
    }
    let text = editor.doc().text();
    let selected = text.slice_to_cow(range.clone()).to_string();
    let vars = variables(ui, path, editor, range.start, &selected);
    let line = editor.offset_to_line_col(range.start).0;
    let line_text = editor.rope_text().line_content(line).to_string();
    let indent: String = line_text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let (tab, _) = crate::indent::unit(ui, path);
    let var = |n: &str| vars.get(n).cloned();
    let e = body::expand(body_text, &Fill { var: &var, indent: &indent, tab: &tab });
    leave();
    let doc = hook(ui, path);
    editor.doc().edit_single(Selection::region(range.start, range.end), &e.text, EditType::Completion);
    let mut stops = e.stops;
    let base = range.start;
    let mut moved = |ix: usize, _: bool| ix + base;
    let mut whole = 0..e.text.len();
    shift(&mut stops, 0, &mut whole, &mut moved);
    let first = selection_of(&stops[0]);
    if stops.len() > 1
        && let Some(doc) = doc
    {
        SESSION.with(|s| {
            *s.borrow_mut() = Some(Session { path: path.to_string(), doc, editor: editor.clone(), stops, at: 0, whole })
        });
    }
    editor.cursor.update(|c| c.set_insert(first));
    choices_shown(ui);
}

/// Tab (`by` 1) or Shift+Tab (-1): the stop's transformed mirrors filled in, the next stop selected; the last ends
/// the snippet.
fn jump(ui: Ui, by: isize) -> bool {
    let Some((editor, edits)) = SESSION.with(|s| {
        let s = s.borrow();
        let s = s.as_ref()?;
        let text = s.editor.doc().text().to_string();
        Some((s.editor.clone(), derived_edits(&s.stops[s.at], &text)))
    }) else {
        return false;
    };
    if !edits.is_empty() {
        let sels: Vec<(Selection, &str)> =
            edits.iter().map(|(r, t)| (Selection::region(r.start, r.end), t.as_str())).collect();
        editor.doc().edit(&mut sels.into_iter(), EditType::Completion);
    }
    let Some((sel, last)) = SESSION.with(|s| {
        let mut s = s.borrow_mut();
        let s = s.as_mut()?;
        let to = (s.at as isize + by).clamp(0, s.stops.len() as isize - 1) as usize;
        s.at = to;
        Some((selection_of(&s.stops[to]), to + 1 == s.stops.len()))
    }) else {
        return false;
    };
    if last {
        leave();
    }
    editor.cursor.update(|c| c.set_insert(sel));
    choices_shown(ui);
    true
}

/// The stop at has choices: they are suggested (the suggest widget opened on it).
fn choices_shown(ui: Ui) {
    if current_choices().is_some() {
        crate::editor::tell(ui, |send| {
            crate::commands::run(ui, send, "editor.action.triggerSuggest");
        });
    }
}

/// The choices of the stop at, and where its first place is.
fn current_choices() -> Option<(Vec<String>, Range<usize>)> {
    SESSION.with(|s| {
        let s = s.borrow();
        let s = s.as_ref()?;
        let stop = &s.stops[s.at];
        (!stop.choices.is_empty()).then(|| (stop.choices.clone(), stop.ranges.first().cloned().unwrap_or(0..0)))
    })
}

/// Tab, Shift+Tab and Escape while a snippet is filled in (called by the editor's keys, after the suggest widget's).
pub fn key(ui: Ui, key: &Key, mods: Modifiers) -> bool {
    if !in_session(ui) {
        return false;
    }
    let shift = mods == Modifiers::SHIFT;
    match key {
        Key::Named(NamedKey::Tab) if mods.is_empty() && has_next(ui) => jump(ui, 1),
        Key::Named(NamedKey::Tab) if shift && has_prev(ui) => jump(ui, -1),
        Key::Named(NamedKey::Escape) if mods.is_empty() => {
            leave();
            true
        }
        _ => false,
    }
}

/// The suggestions for the word typed: each prefix of the snippets for the file, or the choices of the stop at.
fn suggestions(ui: Ui, ask: &complete::Ask) -> Vec<Item> {
    if let Some((choices, range)) = current_choices()
        && (range.start..=range.end).contains(&ask.offset)
    {
        return choices
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                Item::new(c.clone(), Kind::EnumMember).filter(ask.word).sort(format!("{i:04}")).on_accept(
                    move |editor, _| {
                        let Some(r) = current_choices().map(|(_, r)| r) else { return };
                        editor.doc().edit_single(Selection::region(r.start, r.end), &c, EditType::Completion);
                        let end = r.start + c.len();
                        editor.cursor.update(|cur| cur.set_insert(Selection::caret(end)));
                    },
                )
            })
            .collect();
    }
    let how = crate::settings::now(ui, "editor.snippetSuggestions", ask.lang);
    if how.as_str() == Some("none") {
        return Vec::new();
    }
    let lang = crate::language::now(ui, ask.path);
    let all = ALL.with(|a| a.borrow().clone());
    let mut out = Vec::new();
    for s in all.iter().filter(|s| list::applies(s, lang, ask.path)) {
        let docs = format!("```{lang}\n{}\n```", body::plain(&s.body));
        for prefix in &s.prefixes {
            let (ui, path, body_text) = (ui, ask.path.to_string(), s.body.clone());
            let detail =
                format!("{} ({})", if s.description.is_empty() { &s.name } else { &s.description }, s.source.label());
            out.push(
                Item::new(prefix.clone(), Kind::Snippet)
                    .detail(detail)
                    .docs(docs.clone())
                    .on_accept(move |editor, range| insert(ui, &path, editor, range, &body_text)),
            );
        }
    }
    out
}

/// The snippets for the file shown, and its editor's selection: none with a message when no file is shown.
fn for_shown(ui: Ui, surround: bool) -> Option<(String, Editor, Range<usize>, Vec<Snippet>)> {
    let Some((path, editor)) = ui.live.get_untracked() else {
        ui.message.set("Open a file to insert a snippet into.".into());
        return None;
    };
    let lang = crate::language::now(ui, &path);
    let all = ALL.with(|a| a.borrow().clone());
    let list: Vec<Snippet> = all
        .iter()
        .filter(|s| list::applies(s, lang, &path) && (!surround || body::takes_selection(&s.body)))
        .cloned()
        .collect();
    if list.is_empty() {
        ui.message.set(if surround {
            format!("No snippet for {lang} takes the selection ($TM_SELECTED_TEXT).")
        } else {
            format!("There is no snippet for {lang}: Configure Snippets makes one.")
        });
        return None;
    }
    let range = editor.cursor.with_untracked(|c| match &c.mode {
        CursorMode::Insert(sel) => sel.last_inserted().map_or(c.offset()..c.offset(), |r| r.min()..r.max()),
        _ => c.offset()..c.offset(),
    });
    Some((path, editor, range, list))
}

/// Snippets: Insert Snippet, or Surround With Snippet: one picked for the file's language, put in place of the
/// selection.
fn pick(ui: Ui, surround: bool) {
    let Some((path, editor, range, list)) = for_shown(ui, surround) else { return };
    let items = list
        .iter()
        .enumerate()
        .map(|(i, s)| {
            quick::item(&i.to_string(), &s.name)
                .description(&s.prefixes.join(", "))
                .detail(&s.description)
                .group(s.source.label())
        })
        .collect();
    let title = if surround { "Surround With Snippet" } else { "Insert Snippet" };
    quick::pick(title).placeholder("Select a snippet").items(items).back_to_editor(ui).open(ui, move |item| {
        let Some(s) = item.and_then(|i| i.id.parse::<usize>().ok()).and_then(|i| list.get(i)) else { return };
        insert(ui, &path, &editor, range, &s.body);
    });
}

/// A snippets file name as VS Code takes one.
fn bad_name(v: &str) -> Option<String> {
    let v = v.trim();
    let base = v.strip_suffix(".code-snippets").unwrap_or(v);
    let ok = !base.is_empty()
        && base.len() <= 64
        && !base.starts_with('.')
        && base.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    (!ok).then(|| "Letters, digits, dots, dashes and underscores (64 at most).".into())
}

/// A snippets file opened in its tab: a person's through `user_file`, the project's as any file.
fn open_file(ui: Ui, send: &Send, path: &str) {
    if path.starts_with("snippets/") {
        crate::settings::open_user(ui, send, path);
    } else {
        ui.open(path, send);
    }
}

/// The snippets file made when it is not there, then opened.
fn make(send: &Send, name: String) {
    send(json!({"type": "snippet_new", "name": name}));
    MAKING.with(|m| *m.borrow_mut() = Some(name));
}

/// Snippets: Configure Snippets: a new snippets file for every project or for this folder, the files there are, and
/// a language's own file.
fn configure(ui: Ui, send: &Send) {
    let files = FILES.with(|f| f.borrow().clone());
    let folder = ui.folder.get_untracked();
    let folder = folder.rsplit('/').next().unwrap_or("").to_string();
    let mut items = vec![
        quick::item("global", "New Global Snippets file...").description("for every project"),
        quick::item("project", &format!("New Snippets file for '{folder}'..."))
            .description(".vscode/NAME.code-snippets"),
    ];
    for f in files.iter() {
        let name = f.path.rsplit('/').next().unwrap_or(&f.path);
        let description = match f.source {
            Source::User => "User".to_string(),
            Source::Workspace => f.path.clone(),
        };
        items.push(quick::item(&format!("f:{}", f.path), name).description(&description).group("existing snippets"));
    }
    let own: HashSet<&str> = files.iter().filter_map(|f| f.language.as_deref()).collect();
    let mut langs: Vec<_> = crate::language::LANGUAGES.iter().filter(|l| !own.contains(l.id)).collect();
    langs.sort_by_key(|l| l.id);
    items.extend(
        langs.into_iter().map(|l| {
            quick::item(&format!("l:{}", l.id), l.id).description(l.name).group("new snippets for a language")
        }),
    );
    let send = send.clone();
    quick::pick("Configure Snippets").placeholder("Select a snippets file, or make one").items(items).open(
        ui,
        move |item| {
            let Some(item) = item else { return };
            if let Some(path) = item.id.strip_prefix("f:") {
                return open_file(ui, &send, path);
            }
            if let Some(lang) = item.id.strip_prefix("l:") {
                return make(&send, format!("snippets/{lang}.json"));
            }
            let global = item.id == "global";
            let title = if global { "New Global Snippets file" } else { "New Snippets file for this folder" };
            quick::input(title).placeholder("Type snippet file name").validate(bad_name).open(ui, move |typed| {
                let Some(typed) = typed else { return };
                let typed = typed.trim();
                let base = typed.strip_suffix(".code-snippets").unwrap_or(typed);
                let dir = if global { "snippets" } else { ".vscode" };
                make(&send, format!("{dir}/{base}.code-snippets"));
            });
        },
    );
}

/// The session's `snippets` message read off the window's thread, then made the snippets suggested.
fn read_all(ui: Ui, m: &Value) {
    let files: Vec<File> = m["files"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| File {
                    source: Source::of(f["source"].as_str().unwrap_or("")),
                    path: f["path"].as_str().unwrap_or("").to_string(),
                    language: f["language"].as_str().map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();
    FILES.with(|f| *f.borrow_mut() = Rc::new(files));
    let given = m["files"].clone();
    let problems: Vec<String> = m["problems"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| format!("{NOT_READ} {}: {}", p["path"].as_str().unwrap_or(""), p["error"].as_str().unwrap_or("")))
        .collect();
    let done = floem::ext_event::create_ext_action(ui.scope, move |(all, bad): (Vec<Snippet>, Vec<String>)| {
        ALL.with(|a| *a.borrow_mut() = Rc::new(all));
        // A file not read is said; once all are read again, what was said of one is taken away.
        let shown = ui.message.get_untracked();
        match bad.first() {
            Some(first) if *first != shown => ui.message.set(first.clone()),
            None if shown.starts_with(NOT_READ) => ui.message.set(String::new()),
            _ => {}
        }
    });
    std::thread::spawn(move || {
        let mut all = Vec::new();
        let mut bad = problems;
        for f in given.as_array().into_iter().flatten() {
            let path = f["path"].as_str().unwrap_or("");
            let source = Source::of(f["source"].as_str().unwrap_or(""));
            match list::read(f["text"].as_str().unwrap_or(""), f["language"].as_str(), source) {
                Ok(s) => all.extend(s),
                Err(e) => bad.push(format!("{NOT_READ} {path}: {e}")),
            }
        }
        done((all, bad));
    });
}

/// What it adds to the window's registries, once at start.
pub fn register(ui: Ui, _send: &Send) {
    complete::provide("snippets", &[], suggestions);
    use crate::commands::{add, command};
    let editor_shown = |ui: Ui| ui.live.with_untracked(Option::is_some);
    add(command("editor.action.insertSnippet", "Snippets", "Insert Snippet", |ui, _| pick(ui, false))
        .when(editor_shown));
    add(command("editor.action.surroundWithSnippet", "Snippets", "Surround With Snippet...", |ui, _| pick(ui, true))
        .when(editor_shown));
    add(command("workbench.action.openSnippets", "Snippets", "Configure Snippets", configure));
    add(command("jumpToNextSnippetPlaceholder", "", "Go to Next Snippet Placeholder", |ui, _| {
        jump(ui, 1);
    })
    .when(has_next)
    .hidden());
    add(command("jumpToPrevSnippetPlaceholder", "", "Go to Previous Snippet Placeholder", |ui, _| {
        jump(ui, -1);
    })
    .when(has_prev)
    .hidden());
    add(command("leaveSnippet", "", "Leave Snippet", |_, _| leave()).when(in_session).hidden());
    crate::keymap::context_key("inSnippetMode", |ui| Value::Bool(in_session(ui)));
    crate::keymap::context_key("hasNextTabstop", |ui| Value::Bool(has_next(ui)));
    crate::keymap::context_key("hasPrevTabstop", |ui| Value::Bool(has_prev(ui)));
    // The snippet ends when the caret leaves it, or its editor is no longer the one on screen.
    ui.scope.create_effect(move |_| {
        ui.caret.track();
        let live = ui.live.with(|l| l.as_ref().map(|(p, _)| p.clone()));
        let out = SESSION.with(|s| {
            let s = s.borrow();
            let Some(s) = s.as_ref() else { return false };
            if live.as_ref() != Some(&s.path) {
                return true;
            }
            let caret = s.editor.cursor.try_with_untracked(|c| c.map(|c| c.offset()));
            caret.is_none_or(|c| !(s.whole.start..=s.whole.end).contains(&c))
        });
        if out {
            leave();
        }
    });
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "state" if !ASKED.with(|a| a.replace(true)) => send(json!({"type": "snippets"})),
        "snippets" => read_all(ui, m),
        "snippet_new" => {
            let name = m["name"].as_str().unwrap_or("");
            if MAKING.with(|w| w.borrow().as_deref() == Some(name)) {
                MAKING.with(|w| w.borrow_mut().take());
                match m["error"].as_str() {
                    Some(e) => ui.message.set(format!("Could not make {name}: {e}")),
                    None => open_file(ui, send, name),
                }
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(index: u32, ranges: Vec<Range<usize>>) -> Stop {
        Stop { index, ranges, ..Stop::default() }
    }

    /// An insertion of `len` at `at`, as a delta maps offsets.
    fn insertion(at: usize, len: usize) -> impl FnMut(usize, bool) -> usize {
        move |ix, after| if ix > at || (ix == at && after) { ix + len } else { ix }
    }

    #[test]
    fn typing_in_the_stop_at_grows_it_and_moves_the_others() {
        // "for (i; i < ; i++)": stop 1 at "i" three times, stop 2 empty after "< ", the end after it all.
        let mut stops = vec![stop(1, vec![5..6, 8..9, 15..16]), stop(2, vec![12..12]), stop(0, vec![19..19])];
        let mut whole = 0..19;
        // "dx" typed after the first "i": only that place grows (the editor types in each of the mirrors itself).
        shift(&mut stops, 0, &mut whole, &mut insertion(6, 2));
        assert_eq!(stops[0].ranges, [5..8, 10..11, 17..18]);
        assert_eq!(stops[1].ranges, [14..14]);
        assert_eq!(stops[2].ranges, [21..21]);
        assert_eq!(whole, 0..21);
    }

    #[test]
    fn typing_at_the_edge_of_another_stop_does_not_grow_it() {
        let mut stops = vec![stop(1, vec![0..0]), stop(2, vec![0..3]), stop(0, vec![3..3])];
        let mut whole = 0..3;
        shift(&mut stops, 0, &mut whole, &mut insertion(0, 1));
        assert_eq!(stops[0].ranges, [0..1]);
        assert_eq!(stops[1].ranges, [1..4]);
        let mut whole = 0..4;
        shift(&mut stops, 1, &mut whole, &mut insertion(4, 1));
        assert_eq!((stops[1].ranges[0].clone(), stops[2].ranges[0].clone()), (1..5, 5..5));
    }

    #[test]
    fn a_stops_transformed_mirrors_are_filled_in_from_what_was_typed() {
        let var = |_: &str| None;
        let e = body::expand("${1:a} ${1/(.*)/${1:/upcase}/}", &Fill { var: &var, indent: "", tab: "\t" });
        let mut first = e.stops[0].clone();
        let text = "word A";
        first.ranges = vec![0..4];
        first.derived[0].0 = 5..6;
        assert_eq!(derived_edits(&first, text), [(5..6, "WORD".to_string())]);
        let sel = selection_of(&stop(1, vec![1..2, 4..6]));
        assert_eq!(sel.regions().iter().map(|r| (r.start, r.end)).collect::<Vec<_>>(), [(1, 2), (4, 6)]);
    }

    #[test]
    fn snippets_file_names_are_checked_as_vs_codes() {
        assert_eq!(bad_name("team"), None);
        assert_eq!(bad_name("my-own.code-snippets"), None);
        assert!(bad_name("").is_some());
        assert!(bad_name("../x").is_some());
        assert!(bad_name(".hidden").is_some());
        assert!(bad_name("a b").is_some());
    }
}
