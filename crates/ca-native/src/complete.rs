//! The suggest widget, as VS Code's: the suggestions under the word at the caret, filtered as it is typed, one
//! taken. Ctrl+Space opens it (and, open, shows or hides the focused suggestion's details beside it), as do a word's
//! letters typed (`editor.quickSuggestions`) and a provider's trigger characters (`editor.suggestOnTriggerCharacters`).
//! Each row has its kind's icon, the letters typed lit, and on the focused row its detail; the details pane shows the
//! detail and the documentation (Markdown). Up and Down (round the list), PageUp and PageDown move; Enter
//! (`editor.acceptSuggestionOnEnter`) and Tab take the focused suggestion, a click takes one, Escape closes it.
//!
//! What is suggested comes from providers: `provide(id, triggers, items)` adds one (the same id again takes the
//! first's place). Each is asked with an [`Ask`] when the widget opens and answers at once with its [`Item`]s, or
//! later with `deliver(ui, id, seq, items)` (a language server's answer), kept only while that same opening lasts.
//! The words of the open documents are the first provider (`editor.wordBasedSuggestions`). With the widget closed,
//! Tab completes the word typed with the best suggestion when `editor.tabCompletion` says so.
//!
//! Ghost text: `ghost(ui, path, line, col, text)` shows an inline suggestion at the caret in
//! `editorGhostText.foreground` (its first line; `editor.inlineSuggest.enabled`), Tab takes it whole, Escape or the
//! caret moving away drops it, `ghost_clear` takes it away.

// The providers' API is for the parts to come (snippets, language servers, the inline completion).
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::Rect;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use floem::text::{Attrs, AttrsList, FamilyOwned, LineHeightValue, TextLayout, Weight};
use floem::views::editor::Editor;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::cursor::CursorAffinity;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, clip, dyn_container, empty, h_stack, label, rich_text, scroll,
    v_stack, virtual_stack,
};
use floem::{AnyView, IntoView};
use serde_json::Value;

use crate::colors::Colors;
use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::MONO_FONT;

/// VS Code's widget: 430 px wide, 12 rows at most, the details pane beside it.
const WIDTH: f64 = 430.0;
const MOST_ROWS: usize = 12;
const DETAILS_WIDTH: f64 = 330.0;
/// The words of a document read for suggestions: none from a larger one, and so many at most from each.
const WORDS_MOST_BYTES: usize = 2 << 20;
const WORDS_MOST: usize = 5000;

/// What a suggestion is, as VS Code's `CompletionItemKind` (and its icon and colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Struct,
    Interface,
    Module,
    Property,
    Event,
    Operator,
    Unit,
    Value,
    Constant,
    Enum,
    EnumMember,
    Keyword,
    Text,
    Color,
    File,
    Reference,
    Folder,
    TypeParameter,
    Snippet,
}

impl Kind {
    /// The kind a language server's `CompletionItemKind` number is (1 to 25); Text for another.
    pub fn from_lsp(n: u64) -> Kind {
        use Kind::*;
        match n {
            2 => Method,
            3 => Function,
            4 => Constructor,
            5 => Field,
            6 => Variable,
            7 => Class,
            8 => Interface,
            9 => Module,
            10 => Property,
            11 => Unit,
            12 => Value,
            13 => Enum,
            14 => Keyword,
            15 => Snippet,
            16 => Color,
            17 => File,
            18 => Reference,
            19 => Folder,
            20 => EnumMember,
            21 => Constant,
            22 => Struct,
            23 => Event,
            24 => Operator,
            25 => TypeParameter,
            _ => Text,
        }
    }

    /// Its codicon, as VS Code's suggest widget draws it.
    pub fn icon(self) -> &'static str {
        use Kind::*;
        match self {
            Method | Function | Constructor => "symbol-method",
            Field => "symbol-field",
            Variable => "symbol-variable",
            Class => "symbol-class",
            Struct => "symbol-structure",
            Interface => "symbol-interface",
            Module => "symbol-namespace",
            Property => "symbol-property",
            Event => "symbol-event",
            Operator => "symbol-operator",
            Unit => "symbol-ruler",
            Value | Enum => "symbol-enum",
            Constant => "symbol-constant",
            EnumMember => "symbol-enum-member",
            Keyword => "symbol-keyword",
            Text => "symbol-string",
            Color => "symbol-color",
            File => "symbol-file",
            Reference => "references",
            Folder => "folder",
            TypeParameter => "symbol-parameter",
            Snippet => "symbol-snippet",
        }
    }

    /// Its icon's colour (`symbolIcon.*Foreground`).
    fn tint(self, c: &Colors) -> Color {
        use Kind::*;
        match self {
            Method | Function => c.symbol_method,
            Constructor => c.symbol_constructor,
            Field => c.symbol_field,
            Variable | TypeParameter => c.symbol_variable,
            Class => c.symbol_class,
            Struct => c.symbol_struct,
            Interface => c.symbol_interface,
            Module => c.symbol_namespace,
            Property => c.symbol_property,
            Event => c.symbol_event,
            Value | Enum => c.symbol_enum,
            EnumMember => c.symbol_enum_member,
            Constant => c.symbol_constant,
            Keyword => c.symbol_keyword,
            Snippet => c.symbol_snippet,
            Text => c.symbol_text,
            Operator | Unit | Color | File | Reference | Folder => c.fg,
        }
    }
}

/// What taking a suggestion does instead of putting its text in: given the editor and the range of the word typed
/// (a snippet put in with its tab stops).
pub type Accept = Rc<dyn Fn(&Editor, Range<usize>)>;

/// A suggestion.
#[derive(Clone)]
pub struct Item {
    pub label: String,
    pub kind: Kind,
    /// Shown on the focused row and at the top of the details (a signature, a type).
    pub detail: String,
    /// The details pane's documentation, in Markdown.
    pub docs: String,
    /// What is put in place of the word typed; empty: the label.
    pub insert: String,
    /// What the word typed is matched against; empty: the label.
    pub filter: String,
    /// What it is sorted by among those matched as well; empty: the label.
    pub sort: String,
    pub accept: Option<Accept>,
}

impl Item {
    pub fn new(label: impl Into<String>, kind: Kind) -> Item {
        Item {
            label: label.into(),
            kind,
            detail: String::new(),
            docs: String::new(),
            insert: String::new(),
            filter: String::new(),
            sort: String::new(),
            accept: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Item {
        self.detail = detail.into();
        self
    }

    pub fn docs(mut self, docs: impl Into<String>) -> Item {
        self.docs = docs.into();
        self
    }

    pub fn insert(mut self, insert: impl Into<String>) -> Item {
        self.insert = insert.into();
        self
    }

    pub fn filter(mut self, filter: impl Into<String>) -> Item {
        self.filter = filter.into();
        self
    }

    pub fn sort(mut self, sort: impl Into<String>) -> Item {
        self.sort = sort.into();
        self
    }

    pub fn on_accept(mut self, accept: impl Fn(&Editor, Range<usize>) + 'static) -> Item {
        self.accept = Some(Rc::new(accept));
        self
    }

    fn inserted(&self) -> &str {
        if self.insert.is_empty() { &self.label } else { &self.insert }
    }

    fn filtered(&self) -> &str {
        if self.filter.is_empty() { &self.label } else { &self.filter }
    }

    fn sorted(&self) -> &str {
        if self.sort.is_empty() { &self.label } else { &self.sort }
    }
}

/// What a provider is asked: where the caret is and the word typed before it.
pub struct Ask<'a> {
    pub path: &'a str,
    /// VS Code's language id of the file, when known.
    pub lang: Option<&'a str>,
    /// Line and column (a byte in the line) of the caret, from 0.
    pub line: usize,
    pub col: usize,
    pub offset: usize,
    /// The word's letters before the caret.
    pub word: &'a str,
    /// The trigger character typed that opened the widget.
    pub trigger: Option<char>,
    /// Opened by Ctrl+Space, not by typing.
    pub explicit: bool,
    /// This opening's number, to `deliver` a later answer with.
    pub seq: u64,
}

type Items = Rc<dyn Fn(Ui, &Ask) -> Vec<Item>>;

struct Provider {
    id: &'static str,
    triggers: Vec<char>,
    items: Items,
}

/// A suggestion matched against the word typed: its lit letters (in its label) and its score.
#[derive(Clone)]
struct Row {
    item: Rc<Item>,
    lit: Vec<Range<usize>>,
    score: i32,
}

/// The widget open: on which editor, from which offset (the word's start), and each provider's answer.
struct Session {
    path: String,
    editor: Editor,
    start: usize,
    line: usize,
    seq: u64,
    explicit: bool,
    prefix: String,
    lists: Vec<(&'static str, Vec<Rc<Item>>)>,
}

/// An inline suggestion shown.
struct Ghost {
    path: String,
    offset: usize,
    text: String,
}

thread_local! {
    static PROVIDERS: RefCell<Vec<Provider>> = const { RefCell::new(Vec::new()) };
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    static GHOST: RefCell<Option<Ghost>> = const { RefCell::new(None) };
    /// The character just typed and the offset before it: the caret moving past it opens the widget.
    static TYPED: Cell<Option<(char, usize)>> = const { Cell::new(None) };
    static SEQ: Cell<u64> = const { Cell::new(0) };
    /// Each document's words, by path, as of its text's revision.
    static WORDS: RefCell<HashMap<String, (u64, Rc<Vec<String>>)>> = RefCell::new(HashMap::new());
}

/// Adds a provider of suggestions; the same id again takes the first's place. `triggers` are the characters that
/// open the widget when typed (asking only the providers that have that character).
pub fn provide(id: &'static str, triggers: &[char], items: impl Fn(Ui, &Ask) -> Vec<Item> + 'static) {
    PROVIDERS.with(|p| {
        let mut p = p.borrow_mut();
        let provider = Provider { id, triggers: triggers.to_vec(), items: Rc::new(items) };
        match p.iter_mut().find(|q| q.id == id) {
            Some(q) => *q = provider,
            None => p.push(provider),
        }
    });
}

/// A provider's answer given later, for the opening numbered `seq`: dropped when the widget was closed or opened
/// again since.
pub fn deliver(ui: Ui, id: &'static str, seq: u64, items: Vec<Item>) {
    let kept = SESSION.with(|s| {
        let mut s = s.borrow_mut();
        let Some(session) = s.as_mut().filter(|s| s.seq == seq) else { return false };
        let items = items.into_iter().map(Rc::new).collect();
        match session.lists.iter_mut().find(|(p, _)| *p == id) {
            Some((_, list)) => *list = items,
            None => session.lists.push((id, items)),
        }
        true
    });
    if kept {
        show(ui, true);
    }
}

/// Whether the widget is open with suggestions to take.
pub fn is_open(ui: Ui) -> bool {
    let s = ui.state::<Suggest>();
    s.place.with_untracked(Option::is_some) && s.rows.with_untracked(|r| !r.is_empty())
}

// ---- What is pure: words, matching, sorting, keys. ----

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The byte column where the word ending at `col` begins.
fn word_start(line: &str, col: usize) -> usize {
    let col = col.min(line.len());
    line[..col].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(col, |(i, _)| i)
}

/// The words of a text (two letters or more, not begun by a digit), each once, in the order first met; `skip` is a
/// range of the text whose word is left out (the one being typed).
fn words_of(text: &str, skip: Option<Range<usize>>, most: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut from = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        match (is_word(c), from) {
            (true, None) => from = Some(i),
            (false, Some(f)) => {
                from = None;
                let w = &text[f..i];
                let skipped = skip.as_ref().is_some_and(|s| s.start <= i && f <= s.end);
                if w.len() >= 2 && !w.starts_with(|c: char| c.is_ascii_digit()) && !skipped && seen.insert(w) {
                    out.push(w.to_string());
                    if out.len() >= most {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether a character starts a part of a word: its first, one after a separator, an upper-case one after a
/// lower-case one.
fn boundary(chars: &[(usize, char)], i: usize) -> bool {
    i == 0 || !is_word(chars[i - 1].1) || chars[i - 1].1 == '_' || (chars[i].1.is_uppercase() && chars[i - 1].1.is_lowercase())
}

/// How well the letters typed match a word, as VS Code's fuzzy score does it: each letter in order, ignoring case,
/// the first at the word's start or at the start of a part of it. The score and the bytes of the word lit, or none.
fn fuzzy(pattern: &str, word: &str) -> Option<(i32, Vec<Range<usize>>)> {
    if pattern.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let pat: Vec<char> = pattern.chars().collect();
    let same = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    let first = (0..chars.len()).find(|&i| boundary(&chars, i) && same(chars[i].1, pat[0]))?;
    let mut score = if first == 0 { 8 } else { 4 } + i32::from(chars[first].1 == pat[0]);
    let mut hits = vec![first];
    let mut at = first + 1;
    for &p in &pat[1..] {
        let next = if at < chars.len() && same(chars[at].1, p) {
            score += 5;
            at
        } else if let Some(i) = (at..chars.len()).find(|&i| boundary(&chars, i) && same(chars[i].1, p)) {
            score += 3;
            i
        } else {
            let i = (at..chars.len()).find(|&i| same(chars[i].1, p))?;
            score += 1;
            i
        };
        score += i32::from(chars[next].1 == p);
        hits.push(next);
        at = next + 1;
    }
    // A word that is the letters typed and little more first.
    if hits.iter().enumerate().all(|(n, &h)| n == h) {
        score += 10;
    }
    score -= (chars.len() as i32 - pat.len() as i32).min(20) / 4;
    let mut lit: Vec<Range<usize>> = Vec::new();
    for h in hits {
        let (b, c) = chars[h];
        match lit.last_mut() {
            Some(r) if r.end == b => r.end = b + c.len_utf8(),
            _ => lit.push(b..b + c.len_utf8()),
        }
    }
    Some((score, lit))
}

/// The suggestions that match the word typed, best first (then by their sort text, then their label); a label
/// suggested twice is shown once, as its first provider gave it.
fn filtered<'a>(lists: impl IntoIterator<Item = &'a Rc<Item>>, prefix: &str) -> Vec<Row> {
    let mut seen = HashSet::new();
    let mut rows: Vec<Row> = lists
        .into_iter()
        .filter(|item| seen.insert((item.label.as_str(), item.kind == Kind::Text)))
        .filter_map(|item| {
            let (score, lit) = fuzzy(prefix, item.filtered())?;
            // The letters lit are in the label only when the label is what was matched.
            let lit = if item.filter.is_empty() || item.filter == item.label { lit } else { Vec::new() };
            Some(Row { item: item.clone(), lit, score })
        })
        .collect();
    rows.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| a.item.sorted().cmp(b.item.sorted())).then_with(|| a.item.label.cmp(&b.item.label))
    });
    rows
}

/// The row focused after a move by `by` rows among `n`: one at a time goes round, a page stops at the ends.
fn step(at: usize, n: usize, by: isize) -> usize {
    if n == 0 {
        return 0;
    }
    if by.abs() == 1 {
        return (at as isize + by).rem_euclid(n as isize) as usize;
    }
    (at as isize + by).clamp(0, n as isize - 1) as usize
}

/// Whether Enter takes the suggestion (`editor.acceptSuggestionOnEnter`): "on" always, "smart" when it changes the
/// text, "off" never (Enter then breaks the line).
fn enter_takes(setting: &str, typed: &str, inserted: &str) -> bool {
    match setting {
        "off" => false,
        "smart" => typed != inserted,
        _ => true,
    }
}

/// VS Code's language id of a file, by its name, for the languages it is plain to tell.
fn lang_of(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    Some(match ext.as_str() {
        "rs" => "rust",
        "py" => "python",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "md" => "markdown",
        "json" => "json",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "java" => "java",
        "rb" => "ruby",
        "sh" | "bash" => "shellscript",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "html" | "htm" => "html",
        "css" => "css",
        _ => return None,
    })
}

// ---- The widget's state. ----

/// Where the widget is drawn, in the window: its top left, and whether it is above the line.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Spot {
    x: f64,
    y: f64,
    row: f64,
}

#[derive(Clone, Copy)]
struct Suggest {
    rows: RwSignal<im::Vector<Row>>,
    /// Counted up each time the rows are another list (the rows' views are made again).
    rev: RwSignal<u64>,
    at: RwSignal<usize>,
    place: RwSignal<Option<Spot>>,
    /// The details pane beside the list (Ctrl+Space with the widget open), kept as VS Code keeps it.
    details: RwSignal<bool>,
    /// Opened by Ctrl+Space with nothing to suggest: "No suggestions." is shown.
    none: RwSignal<bool>,
}

impl crate::ext::State for Suggest {
    fn new(ui: Ui) -> Self {
        Suggest {
            rows: ui.scope.create_rw_signal(im::Vector::new()),
            rev: ui.scope.create_rw_signal(0),
            at: ui.scope.create_rw_signal(0),
            place: ui.scope.create_rw_signal(None),
            details: ui.scope.create_rw_signal(false),
            none: ui.scope.create_rw_signal(false),
        }
    }
}

fn set_if<T: PartialEq + Clone + 'static>(s: RwSignal<T>, v: T) {
    if s.with_untracked(|was| *was != v) {
        s.set(v);
    }
}

/// The editor on screen, while it is still there.
fn live(ui: Ui) -> Option<(String, Editor)> {
    ui.live.get_untracked().filter(|(_, e)| e.cursor.try_get_untracked().is_some())
}

fn caret_of(editor: &Editor) -> usize {
    editor.cursor.with_untracked(|c| c.offset())
}

/// The words of every open document the setting says, the one typed in first (without the word at the caret).
fn word_items(ui: Ui, path: &str, lang: Option<&str>, editor: &Editor, offset: usize) -> Vec<Item> {
    let how = crate::settings::now(ui, "editor.wordBasedSuggestions", lang);
    let how = how.as_str().unwrap_or("matchingDocuments");
    if how == "off" {
        return Vec::new();
    }
    let mut words: Vec<String> = Vec::new();
    let text = editor.doc().text();
    if text.len() <= WORDS_MOST_BYTES {
        let text = text.to_string();
        words = words_of(&text, Some(offset..offset), WORDS_MOST);
    }
    if how != "currentDocument" {
        let tabs = ui.tabs.with_untracked(|t| {
            t.iter().filter(|t| t.is_file() && t.path != path).map(|t| (t.path.clone(), t.doc.clone())).collect::<Vec<_>>()
        });
        let mut seen: HashSet<String> = words.iter().cloned().collect();
        for (p, doc) in tabs {
            if how == "matchingDocuments" && lang_of(&p) != lang_of(path) {
                continue;
            }
            let rev = doc.cache_rev().get_untracked();
            let cached = WORDS.with(|w| w.borrow().get(&p).filter(|(r, _)| *r == rev).map(|(_, l)| l.clone()));
            let list = cached.unwrap_or_else(|| {
                let text = doc.text();
                let list =
                    Rc::new(if text.len() <= WORDS_MOST_BYTES { words_of(&text.to_string(), None, WORDS_MOST) } else { Vec::new() });
                WORDS.with(|w| w.borrow_mut().insert(p.clone(), (rev, list.clone())));
                list
            });
            words.extend(list.iter().filter(|w| seen.insert((*w).clone())).cloned());
        }
    }
    words.into_iter().map(|w| Item::new(w, Kind::Text)).collect()
}

/// Opens the widget at the caret of the editor on screen: by Ctrl+Space (`explicit`), by a trigger character typed,
/// or by a word's letter typed.
fn open(ui: Ui, explicit: bool, trigger: Option<char>) {
    let Some((path, editor)) = live(ui) else { return };
    let offset = caret_of(&editor);
    let (line, col) = editor.offset_to_line_col(offset);
    let text = editor.rope_text().line_content(line).to_string();
    let from = word_start(&text, col);
    let prefix = text[from..col.min(text.len())].to_string();
    if !explicit && trigger.is_none() && prefix.is_empty() {
        return;
    }
    let seq = SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    let lang = lang_of(&path);
    let ask = Ask { path: &path, lang, line, col, offset, word: &prefix, trigger, explicit, seq };
    let providers =
        PROVIDERS.with(|p| p.borrow().iter().map(|p| (p.id, p.triggers.clone(), p.items.clone())).collect::<Vec<_>>());
    let mut lists = Vec::new();
    if trigger.is_none() {
        let words = word_items(ui, &path, lang, &editor, offset);
        lists.push(("words", words.into_iter().map(Rc::new).collect::<Vec<_>>()));
    }
    for (id, triggers, items) in providers {
        if trigger.is_some_and(|t| !triggers.contains(&t)) {
            continue;
        }
        lists.push((id, items(ui, &ask).into_iter().map(Rc::new).collect()));
    }
    let start = offset - (col - from.min(col));
    SESSION.with(|s| *s.borrow_mut() = Some(Session { path, editor, start, line, seq, explicit, prefix, lists }));
    show(ui, true);
}

/// The widget closed.
pub fn close(ui: Ui) {
    SESSION.with(|s| s.borrow_mut().take());
    let s = ui.state::<Suggest>();
    set_if(s.place, None);
    set_if(s.none, false);
    if s.rows.with_untracked(|r| !r.is_empty()) {
        s.rows.set(im::Vector::new());
    }
}

/// The rows filtered again by the word typed (when `again`, or when it changed), and the widget placed under it.
fn show(ui: Ui, again: bool) {
    let s = ui.state::<Suggest>();
    let shown = SESSION.with(|sess| {
        let sess = sess.borrow();
        let session = sess.as_ref()?;
        let rows = again.then(|| filtered(session.lists.iter().flat_map(|(_, l)| l.iter()), &session.prefix));
        Some((rows, session.explicit, spot(&session.editor, session.start, session.line)))
    });
    let Some((rows, explicit, place)) = shown else { return close(ui) };
    if let Some(rows) = rows {
        s.rev.update(|n| *n += 1);
        set_if(s.at, 0);
        set_if(s.none, explicit && rows.is_empty());
        s.rows.set(rows.into_iter().collect());
    }
    set_if(s.place, place);
}

/// Where the widget goes for a word starting at `start`: its label under the word's, below the line, or above it
/// where there is no room below; none when the word is scrolled out of sight.
fn spot(editor: &Editor, start: usize, line: usize) -> Option<Spot> {
    let (o, v) = (editor.window_origin.get_untracked(), editor.viewport.get_untracked());
    let row = f64::from(editor.line_height(line));
    let (top, bottom) = editor.points_of_offset(start, CursorAffinity::Forward);
    if bottom.y == 0.0 {
        return None;
    }
    let (top, bottom) = (o.y + top.y - v.y0, o.y + bottom.y - v.y0);
    if bottom <= o.y || top >= o.y + v.height() {
        return None;
    }
    let height = row * MOST_ROWS as f64 + 2.0;
    let y = if bottom + height > o.y + v.height() && top - height >= o.y { top - height } else { bottom };
    // The label (after the 16 px icon and its padding) under the word.
    Some(Spot { x: (o.x + top_x(editor, start) - v.x0 - 26.0).max(0.0), y, row })
}

fn top_x(editor: &Editor, offset: usize) -> f64 {
    editor.line_point_of_offset(offset, CursorAffinity::Forward).x
}

/// The caret moved, the text changed or the editor scrolled: the widget follows, closes, or opens for what was typed.
fn refresh(ui: Ui) {
    let Some((path, editor)) = live(ui) else {
        drop_ghost(ui);
        return close(ui);
    };
    let offset = caret_of(&editor);
    let typed = TYPED.with(Cell::take);
    if GHOST.with(|g| g.borrow().as_ref().is_some_and(|g| g.path != path || g.offset != offset)) {
        drop_ghost(ui);
    }
    let open_now = SESSION.with(|s| s.borrow().as_ref().map(|s| (s.path == path && s.editor.id() == editor.id(), s.start, s.line)));
    match open_now {
        Some((true, start, line)) => {
            let (l, col) = editor.offset_to_line_col(offset);
            let text = editor.rope_text().line_content(l).to_string();
            let line_start = offset - col.min(offset);
            let prefix = (offset >= start && l == line)
                .then(|| text.get(start - line_start..col.min(text.len())).map(str::to_string))
                .flatten()
                .filter(|p| p.chars().all(is_word));
            let Some(prefix) = prefix else { return close(ui) };
            let again = SESSION.with(|s| {
                let mut s = s.borrow_mut();
                let session = s.as_mut().expect("open");
                let again = session.prefix != prefix;
                session.prefix = prefix;
                again
            });
            show(ui, again);
        }
        Some((false, ..)) => close(ui),
        None => {
            let Some((c, before)) = typed.filter(|(c, before)| before + c.len_utf8() == offset) else { return };
            let lang = lang_of(&path);
            let trigger = PROVIDERS.with(|p| p.borrow().iter().any(|p| p.triggers.contains(&c)));
            if trigger && crate::settings::now(ui, "editor.suggestOnTriggerCharacters", lang).as_bool() != Some(false) {
                open(ui, false, Some(c));
            } else if is_word(c) && crate::settings::now(ui, "editor.quickSuggestions", lang).as_bool() != Some(false) {
                let _ = before;
                open(ui, false, None);
            }
        }
    }
}

/// The focused suggestion (or the `i`th row) taken: its text in place of the word typed.
fn accept(ui: Ui, i: Option<usize>) {
    let s = ui.state::<Suggest>();
    let i = i.unwrap_or_else(|| s.at.get_untracked());
    let Some(row) = s.rows.with_untracked(|r| r.get(i).cloned()) else { return };
    let Some((editor, start)) = SESSION.with(|s| s.borrow().as_ref().map(|s| (s.editor.clone(), s.start))) else {
        return;
    };
    close(ui);
    if editor.cursor.try_get_untracked().is_none() {
        return;
    }
    put(&editor, start, &row.item);
    if let Some(id) = editor.editor_view_id.get_untracked() {
        id.request_focus();
    }
}

fn put(editor: &Editor, start: usize, item: &Item) {
    let end = caret_of(editor).max(start);
    if let Some(accept) = &item.accept {
        return accept(editor, start..end);
    }
    let text = item.inserted();
    editor.doc().edit_single(Selection::region(start, end), text, EditType::Completion);
    editor.cursor.update(|c| c.set_insert(Selection::caret(start + text.len())));
}

/// Tab with the widget closed (`editor.tabCompletion`): the word typed completed with the best suggestion ("on"),
/// or the best snippet ("onlySnippets").
fn tab_complete(ui: Ui) -> bool {
    let Some((path, editor)) = live(ui) else { return false };
    let lang = lang_of(&path);
    let how = crate::settings::now(ui, "editor.tabCompletion", lang);
    let how = how.as_str().unwrap_or("off");
    if how == "off" {
        return false;
    }
    let offset = caret_of(&editor);
    let (line, col) = editor.offset_to_line_col(offset);
    let text = editor.rope_text().line_content(line).to_string();
    let from = word_start(&text, col);
    if from == col.min(text.len()) {
        return false;
    }
    let prefix = text[from..col].to_string();
    let seq = SEQ.with(|s| s.get());
    let ask = Ask { path: &path, lang, line, col, offset, word: &prefix, trigger: None, explicit: false, seq };
    let mut items: Vec<Rc<Item>> = PROVIDERS
        .with(|p| p.borrow().iter().map(|p| p.items.clone()).collect::<Vec<_>>())
        .into_iter()
        .flat_map(|items| items(ui, &ask))
        .map(Rc::new)
        .collect();
    if how == "on" {
        items.extend(word_items(ui, &path, lang, &editor, offset).into_iter().map(Rc::new));
    } else {
        items.retain(|i| i.kind == Kind::Snippet);
    }
    let rows = filtered(items.iter(), &prefix);
    let Some(best) = rows.into_iter().find(|r| r.item.inserted() != prefix) else { return false };
    put(&editor, offset - (col - from), &best.item);
    true
}

// ---- Ghost text. ----

/// Shows an inline suggestion at a place of a file (line and column from 0, the column a byte in the line), in place
/// of the one shown; only its first line is drawn, Tab takes it whole.
pub fn ghost(ui: Ui, path: &str, line: usize, col: usize, text: &str) {
    if crate::settings::now(ui, "editor.inlineSuggest.enabled", lang_of(path)).as_bool() == Some(false) || text.is_empty()
    {
        return ghost_clear(ui, path);
    }
    let Some(tab) = ui.tab(path) else { return };
    let rope = tab.doc.text();
    let offset = rope.offset_of_line(line.min(rope.line_of_offset(rope.len()))) + col;
    let first = text.lines().next().unwrap_or("").to_string();
    let mark = crate::decor::Mark::Text { line, col, text: first, at: crate::decor::Phantom::Ghost };
    crate::decor::set(ui, "ghost", path, vec![mark]);
    GHOST.with(|g| *g.borrow_mut() = Some(Ghost { path: path.to_string(), offset, text: text.to_string() }));
}

/// The inline suggestion of a file taken away.
pub fn ghost_clear(ui: Ui, path: &str) {
    if GHOST.with(|g| g.borrow().as_ref().is_some_and(|g| g.path == path)) {
        GHOST.with(|g| g.borrow_mut().take());
        crate::decor::clear(ui, "ghost", path);
    }
}

/// The inline suggestion shown: its file, its offset and its whole text.
pub fn ghost_shown() -> Option<(String, usize, String)> {
    GHOST.with(|g| g.borrow().as_ref().map(|g| (g.path.clone(), g.offset, g.text.clone())))
}

fn drop_ghost(ui: Ui) {
    if let Some(path) = GHOST.with(|g| g.borrow().as_ref().map(|g| g.path.clone())) {
        ghost_clear(ui, &path);
    }
}

/// Tab on an inline suggestion at the caret: its text put in.
fn take_ghost(ui: Ui) -> bool {
    let Some((path, editor)) = live(ui) else { return false };
    let offset = caret_of(&editor);
    let Some(text) = GHOST.with(|g| g.borrow().as_ref().filter(|g| g.path == path && g.offset == offset).map(|g| g.text.clone()))
    else {
        return false;
    };
    ghost_clear(ui, &path);
    editor.doc().edit_single(Selection::caret(offset), &text, EditType::Completion);
    editor.cursor.update(|c| c.set_insert(Selection::caret(offset + text.len())));
    true
}

// ---- Keys. ----

/// A key pressed in the editor, before its keymap has it: true when it was the widget's (or the ghost text's).
pub fn key(ui: Ui, key: &Key, mods: Modifiers) -> bool {
    let ctrl = mods.contains(Modifiers::CONTROL);
    let plain = !mods.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::META);
    let bare = plain && !mods.contains(Modifiers::SHIFT);
    let s = ui.state::<Suggest>();
    let open = is_open(ui);
    let space = matches!(key, Key::Named(NamedKey::Space)) || matches!(key, Key::Character(c) if c == " ");
    if space && ctrl && !mods.intersects(Modifiers::ALT | Modifiers::SHIFT | Modifiers::META) {
        if open {
            s.details.update(|d| *d = !*d);
        } else {
            open_now(ui);
        }
        return true;
    }
    let n = s.rows.with_untracked(im::Vector::len);
    match key {
        Key::Named(NamedKey::Escape) if plain => {
            if SESSION.with(|s| s.borrow().is_some()) {
                close(ui);
                return true;
            }
            if ghost_shown().is_some() {
                drop_ghost(ui);
                return true;
            }
            false
        }
        Key::Named(NamedKey::ArrowDown) if open && bare => move_by(ui, 1),
        Key::Named(NamedKey::ArrowUp) if open && bare => move_by(ui, -1),
        Key::Named(NamedKey::PageDown) if open && bare => move_by(ui, MOST_ROWS as isize - 1),
        Key::Named(NamedKey::PageUp) if open && bare => move_by(ui, -(MOST_ROWS as isize - 1)),
        Key::Named(NamedKey::Enter) if open && bare && n > 0 => {
            let row = s.rows.with_untracked(|r| r.get(s.at.get_untracked()).cloned());
            let (path, typed) = SESSION.with(|s| s.borrow().as_ref().map(|s| (s.path.clone(), s.prefix.clone()))).unwrap_or_default();
            let how = crate::settings::now(ui, "editor.acceptSuggestionOnEnter", lang_of(&path));
            if row.is_some_and(|r| enter_takes(how.as_str().unwrap_or("on"), &typed, r.item.inserted())) {
                accept(ui, None);
                return true;
            }
            close(ui);
            false
        }
        Key::Named(NamedKey::Tab) if bare => {
            if open {
                accept(ui, None);
                return true;
            }
            take_ghost(ui) || tab_complete(ui)
        }
        Key::Character(c) if plain => {
            if let (Some(ch), Some((_, editor))) = (c.chars().next(), live(ui)) {
                TYPED.with(|t| t.set(Some((ch, caret_of(&editor)))));
            }
            false
        }
        _ => false,
    }
}

fn move_by(ui: Ui, by: isize) -> bool {
    let s = ui.state::<Suggest>();
    let n = s.rows.with_untracked(im::Vector::len);
    set_if(s.at, step(s.at.get_untracked(), n, by));
    true
}

/// Ctrl+Space (`editor.action.triggerSuggest`): the widget opened at the caret.
fn open_now(ui: Ui) {
    close(ui);
    open(ui, true, None);
}

// ---- The widget drawn. ----

fn font_size(ui: Ui) -> f32 {
    crate::settings::now(ui, "editor.fontSize", None).as_f64().unwrap_or(14.0) as f32
}

/// A row's label laid out, the letters typed lit (bold, `editorSuggestWidget.highlightForeground`).
fn label_layout(row: &Row, c: &Colors, focused: bool, size: f32, height: f64) -> TextLayout {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list(&MONO_FONT).collect();
    let (fg, lit) =
        if focused { (c.suggest_selected_fg, c.suggest_focus_highlight) } else { (c.suggest_fg, c.suggest_highlight) };
    let base = Attrs::new().family(&family).font_size(size).line_height(LineHeightValue::Px(height as f32)).color(fg);
    let mut attrs = AttrsList::new(base);
    for r in row.lit.iter().filter(|r| r.end <= row.item.label.len()) {
        attrs.add_span(r.clone(), base.color(lit).weight(Weight::BOLD));
    }
    let mut layout = TextLayout::new();
    layout.set_text(&row.item.label, attrs);
    layout
}

fn row_view(ui: Ui, i: usize, row: Row, height: f64) -> impl IntoView {
    let s = ui.state::<Suggest>();
    let t = ui.colors;
    let size = font_size(ui);
    let focused = floem::reactive::create_memo(move |_| s.at.get() == i);
    let kind = row.item.kind;
    let (detail, has_docs) = (row.item.detail.clone(), !row.item.docs.is_empty() || !row.item.detail.is_empty());
    let has_detail = !detail.is_empty();
    let text = rich_text(move || label_layout(&row, &t.get(), focused.get(), size, height)).style(|s| s.flex_shrink(0.0_f32));
    h_stack((
        icon(kind.icon(), 16.0).style(move |s| {
            let c = t.get();
            s.margin_horiz(4.0).flex_shrink(0.0_f32).color(if focused.get() { c.suggest_selected_icon } else { kind.tint(&c) })
        }),
        clip(text).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0)),
        label(move || detail.clone()).style(move |s| {
            let c = t.get();
            s.font_size(size * 0.85)
                .padding_left(8.0)
                .max_width_pct(60.0)
                .text_ellipsis()
                .color(c.suggest_selected_fg.multiply_alpha(0.7))
                .apply_if(!has_detail || !focused.get() || s_details(ui), |s| s.hide())
        }),
        icon("chevron-right", 16.0)
            .on_click_stop(move |_| s.details.update(|d| *d = !*d))
            .style(move |s| {
                s.margin_horiz(4.0)
                    .flex_shrink(0.0_f32)
                    .color(t.get().suggest_selected_fg)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .apply_if(!has_docs || !focused.get() || s_details(ui), |s| s.hide())
            }),
    ))
    .on_click_stop(move |_| accept(ui, Some(i)))
    .style(move |s| {
        let c = t.get();
        s.height(height)
            .width_full()
            .items_center()
            .padding_right(4.0)
            .cursor(floem::style::CursorStyle::Pointer)
            .apply_if(focused.get(), |s| s.background(c.suggest_selected_bg))
            .hover(|s| s.apply_if(!focused.get(), |s| s.background(c.list_hover)))
    })
}

fn s_details(ui: Ui) -> bool {
    ui.state::<Suggest>().details.get()
}

/// The details pane: the focused suggestion's detail (in the editor's font) and its documentation.
fn details_view(ui: Ui) -> impl IntoView {
    let s = ui.state::<Suggest>();
    let t = ui.colors;
    let shown = floem::reactive::create_memo(move |_| {
        if !s.details.get() {
            return None;
        }
        s.rev.track();
        let at = s.at.get();
        s.rows.with(|r| r.get(at).map(|r| (r.item.label.clone(), r.item.detail.clone(), r.item.docs.clone())))
            .filter(|(_, d, m)| !d.is_empty() || !m.is_empty())
    });
    dyn_container(
        move || shown.get(),
        move |item| {
            let Some((_, detail, docs)) = item else { return empty().into_any() };
            let has_detail = !detail.is_empty();
            let docs_view: AnyView = if docs.is_empty() {
                empty().into_any()
            } else {
                crate::prose::answer(ui, floem::reactive::create_rw_signal(docs)).into_any()
            };
            v_stack((
                label(move || detail.clone()).style(move |s| {
                    s.font_family(MONO_FONT.to_string()).font_size(13.0).color(t.get().suggest_fg).apply_if(!has_detail, |s| s.hide())
                }),
                docs_view,
            ))
            .style(|s| s.row_gap(6.0).width_full())
            .into_any()
        },
    )
    .style(move |s| {
        let c = t.get();
        s.width(DETAILS_WIDTH)
            .max_height(MOST_ROWS as f64 * 19.0 + 2.0)
            .padding(8.0)
            .font_size(13.0)
            .color(c.suggest_fg)
            .background(c.suggest_bg)
            .border(1.0)
            .border_color(c.suggest_border)
            .apply_if(shown.get().is_none(), |s| s.hide())
    })
}

/// The widget, over the window at the word typed; hidden when closed.
fn widget(ui: Ui) -> AnyView {
    let s = ui.state::<Suggest>();
    let t = ui.colors;
    let height = move || s.place.with(|p| p.map_or(19.0, |p| p.row));
    let rows = virtual_stack(
        VirtualDirection::Vertical,
        VirtualItemSize::Fixed(Box::new(move || s.place.with_untracked(|p| p.map_or(19.0, |p| p.row)))),
        move || {
            let rev = s.rev.get();
            s.rows.get().into_iter().enumerate().map(move |(i, r)| (i, rev, r)).collect::<im::Vector<_>>()
        },
        |(i, rev, _): &(usize, u64, Row)| (*i, *rev),
        move |(i, _, r): (usize, u64, Row)| {
            let h = s.place.with_untracked(|p| p.map_or(19.0, |p| p.row));
            row_view(ui, i, r, h)
        },
    )
    .style(|s| s.flex_col().width_full());
    let list = scroll(rows)
        .ensure_visible(move || {
            let h = height();
            let y = s.at.get() as f64 * h;
            Rect::new(0.0, y, 1.0, y + h)
        })
        .style(move |st| {
            let n = s.rows.with(im::Vector::len).min(MOST_ROWS) as f64;
            st.width_full().height(n * height()).apply_if(n == 0.0, |s| s.hide())
        });
    let none = label(|| "No suggestions.".to_string()).style(move |st| {
        st.padding_horiz(8.0).height(height()).items_center().color(t.get().suggest_fg).apply_if(!s.none.get(), |s| s.hide())
    });
    let box_ = v_stack((list, none)).style(move |st| {
        let c = t.get();
        st.width(WIDTH)
            .font_size(font_size(ui))
            .background(c.suggest_bg)
            .border(1.0)
            .border_color(c.suggest_border)
            .box_shadow_blur(8.0)
            .box_shadow_color(c.shadow)
    });
    h_stack((box_, details_view(ui)))
        .style(move |st| match s.place.get() {
            Some(p) if s.rows.with(|r| !r.is_empty()) || s.none.get() => st.absolute().inset_left(p.x).inset_top(p.y).items_start(),
            _ => st.hide(),
        })
        .into_any()
}

/// What it adds to the window's registries, once at start.
pub fn register(ui: Ui, _send: &Send) {
    crate::layers::add(crate::layers::Place::Over, "editor.suggest", 15, |ui, _| widget(ui));
    crate::commands::add(
        crate::commands::command("editor.action.triggerSuggest", "", "Trigger Suggest", |ui, _| open_now(ui))
            .keys("Ctrl+Space")
            .when(|ui| ui.live.with_untracked(Option::is_some)),
    );
    // The caret, the editor on screen and its scrolling followed.
    ui.scope.create_effect(move |_| {
        ui.caret.track();
        if let Some((_, editor)) = ui.live.get() {
            editor.viewport.track();
        }
        refresh(ui);
    });
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str) -> Rc<Item> {
        Rc::new(Item::new(label, Kind::Text))
    }

    #[test]
    fn the_word_before_the_caret_and_the_words_of_a_text_are_found() {
        assert_eq!(word_start("let foo_bar", 11), 4);
        assert_eq!(word_start("a.b", 2), 2);
        assert_eq!(word_start("  ", 2), 2);
        assert_eq!(word_start("été", "été".len()), 0);
        let text = "fn alpha(beta: u8) { alpha + 12 + x; gamma }";
        assert_eq!(words_of(text, None, 100), ["fn", "alpha", "beta", "u8", "gamma"]);
        // The word at the caret (in "gam|ma") is left out, and so many at most.
        let at = text.find("gamma").unwrap() + 3;
        assert!(!words_of(text, Some(at..at), 100).contains(&"gamma".to_string()));
        assert_eq!(words_of(text, None, 2), ["fn", "alpha"]);
    }

    #[test]
    fn letters_match_in_order_from_a_words_start_or_part_and_are_lit() {
        assert_eq!(fuzzy("fb", "fooBar").map(|f| f.1), Some(vec![0..1, 3..4]));
        assert_eq!(fuzzy("gt", "get_thing").map(|f| f.1), Some(vec![0..1, 4..5]));
        assert_eq!(fuzzy("FOO", "foobar").map(|f| f.1), Some(vec![0..3]));
        assert!(fuzzy("bar", "foobar").is_none(), "the first letter not at a part's start");
        assert!(fuzzy("bar", "fooBar").is_some());
        assert!(fuzzy("xz", "xyz").is_some() && fuzzy("zx", "xyz").is_none());
        assert_eq!(fuzzy("", "any"), Some((0, Vec::new())));
        // The word typed exactly scores above one matched in parts.
        assert!(fuzzy("form", "format").unwrap().0 > fuzzy("form", "fooOrMap").unwrap().0);
    }

    #[test]
    fn rows_are_sorted_best_first_and_a_label_is_shown_once() {
        let items = [item("fooOrMap"), item("format"), item("format"), item("other"), item("Format")];
        let rows = filtered(items.iter(), "form");
        let labels: Vec<&str> = rows.iter().map(|r| r.item.label.as_str()).collect();
        assert_eq!(labels, ["format", "Format", "fooOrMap"]);
        // A suggestion matched by its filter text has no letters lit in its label.
        let other = Rc::new(Item::new("x()", Kind::Method).filter("xyz"));
        let rows = filtered([&other], "xy");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].lit.is_empty());
        // Sorted by the sort text among those matched as well.
        let a = Rc::new(Item::new("ab", Kind::Field).sort("2"));
        let b = Rc::new(Item::new("ac", Kind::Field).sort("1"));
        let rows = filtered([&a, &b], "a");
        assert_eq!(rows[0].item.label, "ac");
    }

    #[test]
    fn moving_goes_round_one_at_a_time_and_a_page_stops_at_the_ends() {
        assert_eq!(step(0, 5, -1), 4);
        assert_eq!(step(4, 5, 1), 0);
        assert_eq!(step(2, 5, 11), 4);
        assert_eq!(step(2, 5, -11), 0);
        assert_eq!(step(0, 0, 1), 0);
    }

    #[test]
    fn enter_takes_a_suggestion_as_the_setting_says() {
        assert!(enter_takes("on", "abc", "abc"));
        assert!(!enter_takes("off", "ab", "abc"));
        assert!(enter_takes("smart", "ab", "abc"));
        assert!(!enter_takes("smart", "abc", "abc"));
    }

    #[test]
    fn each_kind_has_a_known_icon_and_a_language_server_kind_is_read() {
        crate::icons::load();
        for n in 0..=26 {
            let k = Kind::from_lsp(n);
            assert!(crate::icons::known(k.icon()), "{k:?}");
        }
        assert_eq!(Kind::from_lsp(2), Kind::Method);
        assert_eq!(Kind::from_lsp(15), Kind::Snippet);
        assert_eq!(lang_of("src/a.tsx"), Some("typescriptreact"));
        assert_eq!(lang_of("Makefile"), None);
    }

    #[test]
    fn a_provider_is_replaced_by_its_id_and_a_late_answer_is_kept_only_for_its_opening() {
        let ui = Ui::new();
        provide("test", &['.'], |_, _| vec![Item::new("one", Kind::Field)]);
        provide("test", &['.'], |_, _| vec![Item::new("two", Kind::Field)]);
        let names = PROVIDERS.with(|p| p.borrow().iter().filter(|p| p.id == "test").count());
        assert_eq!(names, 1);
        // Nothing open: an answer is dropped.
        deliver(ui, "test", 7, vec![Item::new("late", Kind::Field)]);
        assert!(!is_open(ui));
        close(ui);
    }
}
