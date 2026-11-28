//! The notebook editor: a Jupyter notebook (`.ipynb`) opened as its cells, one under another, as VS Code's. A code
//! cell is an editor of its own (its language's colours) with its outputs under it, its run button and its count
//! (`[3]`, `[*]` while it runs) at its left; a Markdown cell is drawn as it reads, and edited on a double-click or
//! Enter. The toolbar adds cells (+ Code, + Markdown), runs them all, clears their outputs, interrupts and restarts
//! the kernel and says which it is.
//!
//! Two modes, as Jupyter's: in a cell's editor (edit mode) the keys type, Escape leaves it; with the cell itself
//! selected (command mode) Enter edits it, ↑ ↓ (or K J) go from cell to cell, A and B add a cell above and below,
//! D D deletes it, M and Y make it Markdown or code, C X V copy, cut and paste cells, Z and Shift+Z undo and redo
//! a change to the cells. In both, Ctrl+Enter runs the cell, Shift+Enter runs it and goes to the next (a new one
//! after the last), Alt+Enter runs it and adds one below.
//!
//! The notebook is its file: its tab has the dot once anything is changed, Ctrl+S writes it as Jupyter does
//! (`model.rs`). The file's text comes as any file's does (the session's `file`, asked with `open`): an `.ipynb`
//! that has no text tab open is taken here and opened as a notebook. Open as Text (or Reopen Editor With…) shows
//! its JSON in the text editor; Open as Notebook (at the breadcrumbs' end) goes back. A notebook that is not
//! nbformat 4 opens as text. Create: New Jupyter Notebook makes one with an empty Python cell (written only when
//! no file is there).
//!
//! Running is the kernel's (another part, which sets `Kernel` with `set_kernel`): until there is one, a cell asked
//! to run says so. What a kernel tells of a cell is put with `set_run` and `set_outputs`; how outputs are drawn may
//! be replaced with `set_outputs_view` (the plain text of each, until then).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::CursorStyle;
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use floem::views::{
    Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, text_editor_keys, tooltip, v_stack,
};
use floem::{AnyView, IntoView, View, ViewId};
use serde_json::{Map, Value, json};

use crate::commands::{self, command};
use crate::editor::{breadcrumbs, kinds};
use crate::highlight::Colours;
use crate::icons::icon;
use crate::prose::{self, Doc};
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, Theme, quick, toast};
use model::{Change, History, Kind, Notebook};

pub mod model;
pub mod outputs;

/// The kind of tab a notebook is (`notebook:<path>`).
const KIND: &str = "notebook";

/// Below this size a notebook is read and written on the window's thread (it takes less than a frame); above it,
/// off it.
const SMALL: usize = 256 * 1024;

/// The time within which a second D deletes the cell, as Jupyter's D D.
const TWICE: Duration = Duration::from_millis(600);

/// Whether a path is a notebook's.
pub fn is_notebook(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".ipynb")
}

fn tab_of(path: &str) -> String {
    format!("{KIND}:{path}")
}

/// Where a cell's run is.
#[allow(dead_code)] // Queued, Running and Done are the notebook kernel part's to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    Idle,
    /// Waiting for the cells before it.
    Queued,
    Running,
    /// Ended: well (true) or with an error.
    Done(bool),
}

/// A cell as it is on screen: its text in a document of its own (kept while the notebook is open, so a cell drawn
/// again has its text, its caret and its undo), its outputs and its count as signals.
pub struct Live {
    pub key: u64,
    pub kind: Kind,
    pub doc: Rc<TextDocument>,
    colours: Rc<Colours>,
    pub outputs: RwSignal<Arc<Vec<Value>>>,
    pub count: RwSignal<Option<u64>>,
    pub run: RwSignal<Run>,
    /// A Markdown cell in its editor (false: drawn as it reads).
    editing: RwSignal<bool>,
    /// A Markdown cell's text as it is drawn (what it was when its editor was left).
    shown: RwSignal<String>,
    rest: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
enum Load {
    Reading,
    Ready,
}

/// What the notebook has besides its cells.
#[derive(Clone)]
struct Head {
    minor: u64,
    indent: usize,
    rest: Map<String, Value>,
}

/// An open notebook.
pub struct Book {
    pub path: String,
    pub cells: RwSignal<Vec<Rc<Live>>>,
    /// The cell selected (by its key).
    pub selected: RwSignal<Option<u64>>,
    /// Whether the keys are in the selected cell's editor (edit mode), else the cells' (command mode).
    pub editing: RwSignal<bool>,
    load: RwSignal<Load>,
    head: RefCell<Head>,
    /// When the file was read or last written, as the session says it: a save over a newer file is asked about.
    modified: Cell<Option<u64>>,
    size: Cell<usize>,
    history: RefCell<History>,
    /// The view of the cells (that has the keys in command mode), and each cell's, to be scrolled to.
    list: Cell<Option<ViewId>>,
    views: RefCell<HashMap<u64, ViewId>>,
    editors: RefCell<HashMap<u64, Editor>>,
    /// The cell whose editor is to have the keys once it is drawn.
    focus_wanted: Cell<Option<u64>>,
    /// When D was pressed last (D D deletes).
    d_at: Cell<Option<Instant>>,
    saving: Cell<bool>,
    lang: RefCell<String>,
}

/// What runs a notebook's cells: the kernel's part sets it. `run` is given the notebook's path and its code cells
/// to run in order, each by its key with its source; `label` says what the toolbar shows of the notebook's kernel.
#[derive(Clone)]
pub struct Kernel {
    pub run: RunCells,
    pub interrupt: OfNotebook,
    pub restart: OfNotebook,
    pub label: Label,
}

/// Code cells run: the notebook's path, and each cell's key and source.
pub type RunCells = Rc<dyn Fn(Ui, &Send, &str, Vec<(u64, String)>)>;
/// Something done to a notebook's kernel, by the notebook's path.
pub type OfNotebook = Rc<dyn Fn(Ui, &Send, &str)>;
/// What the toolbar says of a notebook's kernel, by the notebook's path.
pub type Label = Rc<dyn Fn(Ui, &str) -> String>;

/// How a code cell's outputs are drawn: given the notebook's path and the cell's outputs.
pub type OutputsView = Rc<dyn Fn(Ui, &Send, &str, RwSignal<Arc<Vec<Value>>>) -> AnyView>;

thread_local! {
    static KERNEL: RefCell<Option<Kernel>> = const { RefCell::new(None) };
    static OUTPUTS: RefCell<Option<OutputsView>> = const { RefCell::new(None) };
}

/// The kernel that runs the cells (from its part's `register`).
#[allow(dead_code)] // Set by the notebook kernel's part.
pub fn set_kernel(k: Kernel) {
    KERNEL.with(|s| *s.borrow_mut() = Some(k));
}

fn kernel() -> Option<Kernel> {
    KERNEL.with(|s| s.borrow().clone())
}

/// How outputs are drawn (from the output renderers' part's `register`).
#[allow(dead_code)] // Set by the notebook output renderers' part.
pub fn set_outputs_view(f: OutputsView) {
    OUTPUTS.with(|s| *s.borrow_mut() = Some(f));
}

/// The window's notebooks, by their paths, and what it keeps between them.
#[derive(Clone, Copy)]
struct Books {
    open: RwSignal<HashMap<String, Rc<Book>>>,
    /// The paths asked to open as text (Open as Text): their next file is the text editor's.
    as_text: RwSignal<HashSet<String>>,
    /// The notebooks being made (Create: New Jupyter Notebook): their `saved` is taken here.
    making: RwSignal<HashSet<String>>,
    /// The cells copied (C, X), to be pasted (V).
    copied: RwSignal<Vec<model::Cell>>,
}

impl crate::ext::State for Books {
    fn new(ui: Ui) -> Books {
        Books {
            open: ui.scope.create_rw_signal(HashMap::new()),
            as_text: ui.scope.create_rw_signal(HashSet::new()),
            making: ui.scope.create_rw_signal(HashSet::new()),
            copied: ui.scope.create_rw_signal(Vec::new()),
        }
    }
}

/// An open notebook, by its file's path.
pub fn book(ui: Ui, path: &str) -> Option<Rc<Book>> {
    ui.state::<Books>().open.with_untracked(|o| o.get(path).cloned())
}

fn new_book(ui: Ui, path: &str) -> Rc<Book> {
    let b = Rc::new(Book {
        path: path.to_string(),
        cells: ui.scope.create_rw_signal(Vec::new()),
        selected: ui.scope.create_rw_signal(None),
        editing: ui.scope.create_rw_signal(false),
        load: ui.scope.create_rw_signal(Load::Reading),
        head: RefCell::new(Head { minor: 5, indent: 1, rest: Map::new() }),
        modified: Cell::new(None),
        size: Cell::new(0),
        history: RefCell::new(History::default()),
        list: Cell::new(None),
        views: RefCell::new(HashMap::new()),
        editors: RefCell::new(HashMap::new()),
        focus_wanted: Cell::new(None),
        d_at: Cell::new(None),
        saving: Cell::new(false),
        lang: RefCell::new("python".into()),
    });
    ui.state::<Books>().open.update(|o| {
        o.insert(path.to_string(), b.clone());
    });
    b
}

/// A cell's editor's file name, by which its language's grammar and the editor's settings are found.
fn cell_file(lang: &str) -> String {
    let ext = match lang {
        "python" => "py",
        "r" => "r",
        "julia" => "jl",
        "javascript" => "js",
        "typescript" => "ts",
        "c#" | "csharp" => "cs",
        "rust" => "rs",
        "scala" => "scala",
        "sql" => "sql",
        _ => "py",
    };
    format!("cell.{ext}")
}

fn live_of(ui: Ui, c: model::Cell, lang: &str) -> Rc<Live> {
    let doc = Rc::new(TextDocument::new(ui.scope, c.source.as_str()));
    let file = match c.kind {
        Kind::Code => cell_file(lang),
        Kind::Markdown => "cell.md".into(),
        Kind::Raw => "cell.txt".into(),
    };
    let colours = Colours::new(doc.clone(), &file);
    Rc::new(Live {
        key: c.key,
        kind: c.kind,
        doc,
        colours,
        outputs: ui.scope.create_rw_signal(c.outputs),
        count: ui.scope.create_rw_signal(c.count),
        run: ui.scope.create_rw_signal(Run::Idle),
        // An empty Markdown cell is edited, as there is nothing to draw.
        editing: ui.scope.create_rw_signal(c.kind == Kind::Markdown && c.source.is_empty()),
        shown: ui.scope.create_rw_signal(c.source),
        rest: c.rest,
    })
}

impl Book {
    /// The cells as they are now, their texts read from their editors.
    pub fn cells_now(&self) -> Vec<model::Cell> {
        self.cells.with_untracked(|cells| {
            cells
                .iter()
                .map(|l| model::Cell {
                    key: l.key,
                    kind: l.kind,
                    source: l.doc.text().to_string(),
                    outputs: l.outputs.get_untracked(),
                    count: l.count.get_untracked(),
                    rest: l.rest.clone(),
                })
                .collect()
        })
    }

    fn notebook(&self, cells: Vec<model::Cell>) -> Notebook {
        let h = self.head.borrow();
        Notebook { cells, minor: h.minor, indent: h.indent, rest: h.rest.clone() }
    }

    /// The cells put on screen: a cell already there (by its key, of the same kind) keeps its editor, given the text
    /// it is to have; the others are made.
    fn set_cells(&self, ui: Ui, cells: Vec<model::Cell>) {
        let lang = self.lang.borrow().clone();
        let old: HashMap<(u64, Kind), Rc<Live>> =
            self.cells.with_untracked(|c| c.iter().map(|l| ((l.key, l.kind), l.clone())).collect());
        let made: Vec<Rc<Live>> = cells
            .into_iter()
            .map(|c| match old.get(&(c.key, c.kind)) {
                Some(l) => {
                    fill(&l.doc, &c.source);
                    if l.outputs.with_untracked(|o| !Arc::ptr_eq(o, &c.outputs) && **o != *c.outputs) {
                        l.outputs.set(c.outputs);
                    }
                    if l.count.get_untracked() != c.count {
                        l.count.set(c.count);
                    }
                    l.clone()
                }
                None => live_of(ui, c, &lang),
            })
            .collect();
        let keys: HashSet<u64> = made.iter().map(|l| l.key).collect();
        self.views.borrow_mut().retain(|k, _| keys.contains(k));
        self.editors.borrow_mut().retain(|k, _| keys.contains(k));
        self.cells.set(made);
        if self.selected.get_untracked().is_none_or(|k| !keys.contains(&k)) {
            let first = self.cells.with_untracked(|c| c.first().map(|l| l.key));
            self.selected.set(first);
        }
    }

    pub fn live(&self, key: u64) -> Option<Rc<Live>> {
        self.cells.with_untracked(|c| c.iter().find(|l| l.key == key).cloned())
    }

    fn at(&self, key: u64) -> Option<usize> {
        self.cells.with_untracked(|c| c.iter().position(|l| l.key == key))
    }

    fn key_at(&self, at: usize) -> Option<u64> {
        self.cells.with_untracked(|c| c.get(at).map(|l| l.key))
    }

    fn len(&self) -> usize {
        self.cells.with_untracked(Vec::len)
    }
}

/// A document given a text, as one edit (nothing when it has it).
fn fill(doc: &TextDocument, text: &str) {
    let now = doc.text();
    if now.to_string() != text {
        doc.edit_single(Selection::region(0, now.len()), text, EditType::Other);
    }
}

fn dirty(ui: Ui, book: &Book) {
    if let Some(tab) = ui.tab(&tab_of(&book.path))
        && !tab.dirty.get_untracked()
    {
        tab.dirty.set(true);
    }
}

/// A notebook's text read into its cells. One that is not a notebook (not nbformat 4) is opened as text instead.
fn read(ui: Ui, send: &Send, book: Rc<Book>, text: String, modified: Option<u64>) {
    book.modified.set(modified);
    book.size.set(text.len());
    if text.len() < SMALL {
        let parsed = model::parse(&text);
        took(ui, send, &book, parsed);
        return;
    }
    let send = send.clone();
    let done = floem::ext_event::create_ext_action(ui.scope, move |parsed: Result<Notebook, String>| {
        took(ui, &send, &book, parsed);
    });
    std::thread::spawn(move || done(model::parse(&text)));
}

fn took(ui: Ui, send: &Send, book: &Rc<Book>, parsed: Result<Notebook, String>) {
    match parsed {
        Ok(nb) => {
            *book.lang.borrow_mut() = nb.language();
            *book.head.borrow_mut() = Head { minor: nb.minor, indent: nb.indent, rest: nb.rest };
            *book.history.borrow_mut() = History::default();
            // Read again (the file was opened once more): its cells are new.
            book.cells.set(Vec::new());
            book.set_cells(ui, nb.cells);
            if book.load.get_untracked() != Load::Ready {
                book.load.set(Load::Ready);
            }
        }
        Err(e) => {
            toast::warning(ui, &format!("{} opens as text: {e}", book.path));
            let path = book.path.clone();
            forget(ui, &path);
            ui.close(&tab_of(&path));
            open_as_text_now(ui, send, &path);
        }
    }
}

fn forget(ui: Ui, path: &str) {
    ui.state::<Books>().open.update(|o| {
        o.remove(path);
    });
}

/// A notebook opened: its tab made at once (its cells come when its file has been read) and its file asked for.
pub fn open(ui: Ui, send: &Send, path: &str) {
    let id = tab_of(path);
    if ui.tab(&id).is_none() || book(ui, path).is_none() {
        new_book(ui, path);
        send(json!({"type": "open", "path": path}));
    }
    kinds::open(ui, &id);
}

/// The text editor's tab of a notebook's file shown in its place.
fn open_as_text_now(ui: Ui, send: &Send, path: &str) {
    ui.state::<Books>().as_text.update(|a| {
        a.insert(path.to_string());
    });
    ui.open(path, send);
}

/// Open as Text: the notebook shown as its JSON in the text editor, in the notebook's place (asked to be saved
/// first when it has changes).
pub fn open_as_text(ui: Ui, send: &Send, path: &str) {
    let id = tab_of(path);
    if ui.tab(&id).is_some_and(|t| t.dirty.get_untracked()) {
        toast::info(ui, &format!("Save {} first: its changes are the notebook editor's.", name_of(path)));
        return;
    }
    if ui.tab(&id).is_some() {
        ui.close(&id);
    }
    forget(ui, path);
    open_as_text_now(ui, send, path);
}

/// Open as Notebook: a notebook's text tab shown as its cells, in its place (asked to be saved first).
pub fn open_as_notebook(ui: Ui, send: &Send, path: &str) {
    if let Some(tab) = ui.tab(path) {
        if tab.dirty.get_untracked() {
            toast::info(ui, &format!("Save {} first: its changes are the text editor's.", name_of(path)));
            return;
        }
        ui.close(path);
    }
    open(ui, send, path);
}

fn name_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Save: the notebook written as Jupyter writes it. A big one is written off the window's thread.
fn save(ui: Ui, send: &Send, path: &str) {
    let Some(book) = book(ui, path) else { return };
    if book.load.get_untracked() != Load::Ready {
        return;
    }
    let nb = book.notebook(book.cells_now());
    book.saving.set(true);
    let modified = book.modified.get();
    let (path, send) = (path.to_string(), send.clone());
    let out = move |text: String| send(json!({"type": "save", "path": path, "text": text, "modified": modified}));
    if book.size.get() < SMALL {
        out(model::write(&nb));
        return;
    }
    let done = floem::ext_event::create_ext_action(ui.scope, out);
    std::thread::spawn(move || done(model::write(&nb)));
}

/// Create: New Jupyter Notebook: a path in the folder asked for, the file made there with one empty Python cell
/// (not over a file there already: that one opens as it is), and opened.
fn create(ui: Ui, send: &Send) {
    let send = send.clone();
    quick::input("New Jupyter Notebook")
        .placeholder("Its path in the folder (notebooks/analysis.ipynb)")
        .value("Untitled.ipynb")
        .validate(|v| (v.trim().is_empty() || v.contains("..")).then(|| "Give a path in the folder.".into()))
        .open(ui, move |typed| {
            let Some(typed) = typed.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) else { return };
            let path = if is_notebook(&typed) { typed } else { format!("{typed}.ipynb") };
            ui.state::<Books>().making.update(|m| {
                m.insert(path.clone());
            });
            // A time no file has: a file there already is not written over (its time is another), only said.
            let text = model::write(&Notebook::empty());
            send(json!({"type": "save", "path": path, "text": text, "modified": 0}));
        });
}

/// What the kernel's label in the toolbar says.
fn kernel_label(ui: Ui, path: &str) -> String {
    match kernel() {
        Some(k) => (k.label)(ui, path),
        None => "Select Kernel".into(),
    }
}

/// Cells run by the kernel (Markdown cells are only drawn as they read).
fn run(ui: Ui, send: &Send, book: &Rc<Book>, keys: &[u64]) {
    let mut code = Vec::new();
    for key in keys {
        let Some(l) = book.live(*key) else { continue };
        match l.kind {
            Kind::Markdown => leave_markdown(&l),
            Kind::Code => code.push((l.key, l.doc.text().to_string())),
            Kind::Raw => {}
        }
    }
    if code.is_empty() {
        return;
    }
    match kernel() {
        Some(k) => (k.run)(ui, send, &book.path, code),
        None => {
            toast::info(ui, "No kernel to run the cells: there is none in this window yet.");
        }
    }
}

fn leave_markdown(l: &Live) {
    if l.kind == Kind::Markdown {
        let text = l.doc.text().to_string();
        if l.shown.with_untracked(|s| *s != text) {
            l.shown.set(text);
        }
        if l.editing.get_untracked() {
            l.editing.set(false);
        }
    }
}

/// A change to the cells made, one step of Undo, and the notebook changed.
fn change(ui: Ui, book: &Book, c: Change) -> bool {
    let before = book.cells_now();
    let mut nb = book.notebook(before.clone());
    if !model::apply(&mut nb, c) {
        return false;
    }
    book.history.borrow_mut().before(before);
    book.set_cells(ui, nb.cells);
    dirty(ui, book);
    true
}

fn undo(ui: Ui, book: &Book, back: bool) {
    let now = book.cells_now();
    // What was typed since is the editors' own undo's: a cell still there keeps its text.
    let texts: HashMap<u64, String> = now.iter().map(|c| (c.key, c.source.clone())).collect();
    let to = {
        let mut h = book.history.borrow_mut();
        if back { h.undo(now) } else { h.redo(now) }
    };
    if let Some(mut cells) = to {
        for c in &mut cells {
            if let Some(text) = texts.get(&c.key) {
                c.source.clone_from(text);
            }
        }
        book.set_cells(ui, cells);
        dirty(ui, book);
    }
}

/// A cell selected, in command mode: the cells' view has the keys, and the cell is scrolled to.
fn select(book: &Book, key: u64) {
    if book.selected.get_untracked() != Some(key) {
        book.selected.set(Some(key));
    }
    command_mode(book);
    if let Some(v) = book.views.borrow().get(&key) {
        v.scroll_to(None);
    }
}

fn command_mode(book: &Book) {
    if book.editing.get_untracked() {
        book.editing.set(false);
    }
    if let Some(list) = book.list.get() {
        list.request_focus();
    }
}

/// A cell's editor given the keys (edit mode): a Markdown cell's is drawn first.
fn edit(book: &Book, key: u64) {
    let Some(l) = book.live(key) else { return };
    if book.selected.get_untracked() != Some(key) {
        book.selected.set(Some(key));
    }
    if !book.editing.get_untracked() {
        book.editing.set(true);
    }
    if l.kind == Kind::Markdown && !l.editing.get_untracked() {
        book.focus_wanted.set(Some(key));
        l.editing.set(true);
        return;
    }
    let view = book.editors.borrow().get(&key).and_then(|e| e.editor_view_id.get_untracked());
    match view {
        Some(v) => {
            v.request_focus();
            v.scroll_to(None);
        }
        None => book.focus_wanted.set(Some(key)),
    }
}

/// How a cell is run from its keys.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Then {
    /// Ctrl+Enter: it stays selected.
    Stay,
    /// Shift+Enter: the next is selected (one is added after the last).
    Next,
    /// Alt+Enter: a code cell is added below and edited.
    Insert,
}

fn run_then(ui: Ui, send: &Send, book: &Rc<Book>, then: Then) {
    let Some(key) = book.selected.get_untracked() else { return };
    let was_editing = book.editing.get_untracked();
    run(ui, send, book, &[key]);
    let at = book.at(key).unwrap_or(0);
    match then {
        Then::Stay => {
            if book.live(key).is_some_and(|l| l.kind == Kind::Markdown) {
                select(book, key);
            }
        }
        Then::Next => match book.key_at(at + 1) {
            Some(next) => {
                let code = book.live(next).is_some_and(|l| l.kind == Kind::Code);
                if was_editing && code { edit(book, next) } else { select(book, next) }
            }
            None => {
                let new = model::next_key();
                change(ui, book, Change::Insert(at + 1, Kind::Code, new));
                edit(book, new);
            }
        },
        Then::Insert => {
            let new = model::next_key();
            change(ui, book, Change::Insert(at + 1, Kind::Code, new));
            edit(book, new);
        }
    }
}

/// A cell added at a place, selected (and edited when `edit_it`).
fn insert(ui: Ui, book: &Book, at: usize, kind: Kind, edit_it: bool) {
    let key = model::next_key();
    if change(ui, book, Change::Insert(at, kind, key)) {
        if edit_it { edit(book, key) } else { select(book, key) }
    }
}

fn delete(ui: Ui, book: &Book, key: u64) {
    let at = book.at(key).unwrap_or(0);
    if change(ui, book, Change::Delete(key)) {
        // The one after it selected, else the one before (none: the notebook has no cell left).
        let next = book.key_at(at).or_else(|| at.checked_sub(1).and_then(|a| book.key_at(a)));
        match next {
            Some(k) => select(book, k),
            None => book.selected.set(None),
        }
    }
}

/// The keys of command mode: true when one was taken.
fn command_key(ui: Ui, send: &Send, book: &Rc<Book>, key: &Key, m: Modifiers) -> bool {
    let ctrl = m.contains(Modifiers::CONTROL) || m.contains(Modifiers::META);
    let (shift, alt) = (m.contains(Modifiers::SHIFT), m.contains(Modifiers::ALT));
    let plain = !ctrl && !alt && !shift;
    let (ctrl_only, shift_only) = (ctrl && !alt && !shift, shift && !ctrl && !alt);
    // The second key of a chord (Ctrl+K M) is the workbench's.
    if crate::commands::waiting().is_some() {
        return false;
    }
    let letter = |l: &str| matches!(key, Key::Character(c) if c.eq_ignore_ascii_case(l));
    let selected = book.selected.get_untracked();
    let at = selected.and_then(|k| book.at(k));
    if !matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("d")) {
        book.d_at.set(None);
    }
    match key {
        Key::Named(NamedKey::Enter) if shift && !ctrl && !alt => run_then(ui, send, book, Then::Next),
        Key::Named(NamedKey::Enter) if ctrl && !alt && !shift => run_then(ui, send, book, Then::Stay),
        Key::Named(NamedKey::Enter) if alt && !ctrl && !shift => run_then(ui, send, book, Then::Insert),
        Key::Named(NamedKey::Enter) if plain => match selected {
            Some(k) => edit(book, k),
            None => return false,
        },
        Key::Named(NamedKey::ArrowUp) if plain => go(book, at, -1),
        Key::Named(NamedKey::ArrowDown) if plain => go(book, at, 1),
        Key::Named(NamedKey::Home) if ctrl => go(book, Some(0), -1),
        Key::Named(NamedKey::End) if ctrl => go(book, Some(book.len().saturating_sub(1)), 1),
        _ if plain && letter("k") => go(book, at, -1),
        _ if plain && letter("j") => go(book, at, 1),
        _ if plain && letter("a") => insert(ui, book, at.unwrap_or(0), Kind::Code, false),
        _ if plain && letter("b") => insert(ui, book, at.map_or(book.len(), |a| a + 1), Kind::Code, false),
        _ if plain && letter("d") => {
            let twice = book.d_at.get().is_some_and(|t| t.elapsed() < TWICE);
            book.d_at.set(if twice { None } else { Some(Instant::now()) });
            if twice && let Some(k) = selected {
                delete(ui, book, k);
            }
        }
        Key::Named(NamedKey::Delete) if plain => {
            if let Some(k) = selected {
                delete(ui, book, k);
            }
        }
        _ if plain && letter("m") => to_kind(ui, book, selected, Kind::Markdown),
        _ if plain && letter("y") => to_kind(ui, book, selected, Kind::Code),
        _ if plain && letter("r") => to_kind(ui, book, selected, Kind::Raw),
        _ if (plain || ctrl_only) && letter("z") => undo(ui, book, true),
        _ if (shift_only || ctrl && shift && !alt) && letter("z") || ctrl_only && letter("y") => undo(ui, book, false),
        _ if (plain || ctrl_only) && letter("c") => copy(ui, book, selected, false),
        _ if (plain || ctrl_only) && letter("x") => copy(ui, book, selected, true),
        _ if (plain || ctrl_only) && letter("v") => paste(ui, book, selected),
        _ if shift_only && letter("v") => paste(ui, book, selected.and_then(|k| prev_of(book, k))),
        _ => return false,
    }
    true
}

/// The cell before one (none: it is the first).
fn prev_of(book: &Book, key: u64) -> Option<u64> {
    book.at(key).and_then(|a| a.checked_sub(1)).and_then(|a| book.key_at(a))
}

fn go(book: &Book, at: Option<usize>, by: i64) {
    let n = book.len();
    if n == 0 {
        return;
    }
    let to = match at {
        Some(a) => (a as i64 + by).clamp(0, n as i64 - 1) as usize,
        None => 0,
    };
    if let Some(k) = book.key_at(to) {
        select(book, k);
    }
}

fn to_kind(ui: Ui, book: &Book, key: Option<u64>, kind: Kind) {
    if let Some(k) = key
        && change(ui, book, Change::Kind(k, kind))
    {
        select(book, k);
    }
}

fn copy(ui: Ui, book: &Book, key: Option<u64>, cut: bool) {
    let Some(k) = key else { return };
    let Some(cell) = book.cells_now().into_iter().find(|c| c.key == k) else { return };
    ui.state::<Books>().copied.set(vec![cell]);
    if cut {
        delete(ui, book, k);
    }
}

/// The cells copied put after a cell (none: at the top).
fn paste(ui: Ui, book: &Book, after: Option<u64>) {
    let cells = ui.state::<Books>().copied.get_untracked();
    if cells.is_empty() {
        return;
    }
    let at = after.and_then(|k| book.at(k)).map_or(0, |a| a + 1);
    if change(ui, book, Change::Paste(after, cells))
        && let Some(k) = book.key_at(at)
    {
        select(book, k);
    }
}

// --- What is drawn -------------------------------------------------------------------------------------------

/// A button of the notebook's toolbar: its codicon and its words, as VS Code's.
/// Its words are left out while `short` holds (a narrow editor), as VS Code's toolbar does.
fn tool(
    t: Theme,
    short: RwSignal<bool>,
    name: &'static str,
    text: &'static str,
    on: impl Fn() + 'static,
) -> impl IntoView {
    let words = label(move || text).style(move |s| s.apply_if(short.get(), |s| s.hide()));
    h_stack((icon(name, 16.0), words)).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        s.height(22.0)
            .gap(4.0)
            .padding_horiz(4.0)
            .items_center()
            .border_radius(5.0)
            .font_size(12.0)
            .color(c.fg)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.toolbar_hover))
    })
}

fn separator(t: Theme) -> impl IntoView {
    empty().style(move |s| s.width(1.0).height(16.0).margin_horiz(4.0).background(t.get().nb_toolbar_separator))
}

fn toolbar(ui: Ui, send: &Send, book: &Rc<Book>) -> AnyView {
    let t = ui.colors;
    let short = floem::reactive::create_rw_signal(false);
    let (b1, b2, b3, b4, b5, b6) = (book.clone(), book.clone(), book.clone(), book.clone(), book.clone(), book.clone());
    let (s1, s2, s3, s4) = (send.clone(), send.clone(), send.clone(), send.clone());
    let path = book.path.clone();
    let kernel_at = book.path.clone();
    h_stack((
        tool(t, short, "add", "Code", move || {
            insert(
                ui,
                &b1,
                b1.selected.get_untracked().and_then(|k| b1.at(k)).map_or(b1.len(), |a| a + 1),
                Kind::Code,
                true,
            )
        }),
        tool(t, short, "add", "Markdown", move || {
            insert(
                ui,
                &b2,
                b2.selected.get_untracked().and_then(|k| b2.at(k)).map_or(b2.len(), |a| a + 1),
                Kind::Markdown,
                true,
            )
        }),
        separator(t),
        tool(t, short, "run-all", "Run All", move || {
            let keys: Vec<u64> = b3.cells.with_untracked(|c| c.iter().map(|l| l.key).collect());
            run(ui, &s1, &b3, &keys);
        }),
        tool(t, short, "clear-all", "Clear All Outputs", move || {
            change(ui, &b4, Change::Clear(None));
        }),
        tool(t, short, "debug-stop", "Interrupt", move || {
            if let Some(k) = kernel() {
                (k.interrupt)(ui, &s2, &b5.path);
            }
        }),
        tool(t, short, "debug-restart", "Restart", move || {
            if let Some(k) = kernel() {
                (k.restart)(ui, &s3, &b6.path);
            }
        }),
        empty().style(|s| s.flex_grow(1.0_f32)),
        tooltip(tool(t, short, "go-to-file", "Open as Text", move || open_as_text(ui, &s4, &path)), || {
            label(|| "Reopen the notebook as its JSON text")
        }),
        label(move || kernel_label(ui, &kernel_at))
            .on_click_stop(move |_| {
                if kernel().is_none() {
                    toast::info(ui, "No kernel to select: there is none in this window yet.");
                }
            })
            .style(move |s| {
                let c = t.get();
                s.height(22.0)
                    .padding_horiz(6.0)
                    .items_center()
                    .border_radius(5.0)
                    .font_size(12.0)
                    .color(c.fg)
                    .cursor(CursorStyle::Pointer)
                    .hover(|s| s.background(c.toolbar_hover))
            }),
    ))
    .on_resize(move |r| {
        // Its words left out where they do not fit beside the kernel's name.
        let now = r.width() < 760.0;
        if short.get_untracked() != now {
            short.set(now);
        }
    })
    .style(move |s| {
        let c = t.get();
        s.width_full()
            .height(32.0)
            .padding_horiz(12.0)
            .gap(2.0)
            .items_center()
            .border_bottom(1.0)
            .border_color(c.nb_cell_border)
            .background(c.editor_bg)
    })
    .into_any()
}

/// What a notebook's tab draws: its toolbar and its cells.
fn view(ui: Ui, send: Send, path: &str) -> AnyView {
    let t = ui.colors;
    let Some(book) = book(ui, path) else {
        return label(|| "").style(|s| s.size_full()).into_any();
    };
    let load = book.load;
    let ready = create_memo(move |_| load.get() == Load::Ready);
    let b = book.clone();
    let s = send.clone();
    let body = dyn_container(
        move || ready.get(),
        move |ready| {
            if ready {
                cells_view(ui, &s, &b).into_any()
            } else {
                label(|| "Reading the notebook…").style(move |s| s.padding(20.0).color(t.get().desc)).into_any()
            }
        },
    )
    .style(|s| s.width_full().flex_grow(1.0_f32).flex_basis(0.0_f32).min_height(0.0));
    let bar = toolbar(ui, &send, &book);
    v_stack((bar, body)).style(move |s| s.size_full().background(t.get().editor_bg)).into_any()
}

fn cells_view(ui: Ui, send: &Send, book: &Rc<Book>) -> AnyView {
    let t = ui.colors;
    let short = floem::reactive::create_rw_signal(false);
    let (b, s) = (book.clone(), send.clone());
    let cells = book.cells;
    let list =
        dyn_stack(move || cells.get(), |l: &Rc<Live>| (l.key, l.kind), move |l: Rc<Live>| cell_view(ui, &s, &b, l))
            .style(|s| s.flex_col().width_full());
    let (b1, b2) = (book.clone(), book.clone());
    let end = h_stack((
        tool(t, short, "add", "Code", move || insert(ui, &b1, b1.len(), Kind::Code, true)),
        tool(t, short, "add", "Markdown", move || insert(ui, &b2, b2.len(), Kind::Markdown, true)),
    ))
    .style(|s| s.gap(8.0).padding_vert(12.0).padding_left(56.0));
    let (kb, ks) = (book.clone(), send.clone());
    let all = v_stack((list, end))
        .keyboard_navigable()
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            if command_key(ui, &ks, &kb, &k.key.logical_key, k.modifiers) {
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .style(|s| s.flex_col().width_full().padding_vert(8.0).padding_right(20.0).min_height_pct(100.0));
    book.list.set(Some(all.id()));
    // The cells' view has the keys when the notebook is shown in command mode.
    let first = book.clone();
    let id = all.id();
    floem::action::exec_after(Duration::ZERO, move |_| {
        if !first.editing.get_untracked() {
            id.request_focus();
        } else if let Some(k) = first.selected.get_untracked() {
            edit(&first, k);
        }
    });
    scroll(all).style(|s| s.size_full()).into_any()
}

/// A cell: its run button and count at its left, its editor or its text drawn, its outputs, and its tools while it
/// is selected.
fn cell_view(ui: Ui, send: &Send, book: &Rc<Book>, l: Rc<Live>) -> AnyView {
    let t = ui.colors;
    let key = l.key;
    let selected = book.selected;
    let editing = book.editing;
    let is_selected = create_memo(move |_| selected.get() == Some(key));
    let left = gutter(ui, send, book, &l);
    let body: AnyView = match l.kind {
        Kind::Code | Kind::Raw => {
            let ed = cell_editor(ui, send, book, &l);
            let outs = outputs_view(ui, send, &book.path, &l);
            v_stack((ed, outs)).style(|s| s.flex_col().width_full().min_width(0.0)).into_any()
        }
        Kind::Markdown => markdown_cell(ui, send, book, &l),
    };
    let body = container(body).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0_f32).min_width(0.0));
    let tools = cell_tools(ui, book, &l);
    let b = book.clone();
    // The tools float over the cell's top right corner, as VS Code's do, so selecting a cell leaves its text where
    // it was.
    let tools = tools.style(|s| s.absolute().inset_top(-6.0).inset_right(16.0).z_index(2));
    let row = h_stack((left, body, tools)).style(
        move |s| {
            let c = t.get();
            let on = is_selected.get();
            s.width_full()
                .padding_vert(4.0)
                .border_left(2.0)
                .border_color(if on && !editing.get() {
                    c.nb_focused_cell
                } else if on {
                    c.nb_cell_border
                } else {
                    floem::peniko::Color::TRANSPARENT
                })
                .background(if on { c.nb_selected_bg.multiply_alpha(0.35) } else { floem::peniko::Color::TRANSPARENT })
                .margin_vert(2.0)
        },
    );
    book.views.borrow_mut().insert(key, row.id());
    // A press on the cell selects it, in command mode: once what was pressed (its text, which takes the keys to
    // copy) has done with it. One in its editor is the editor's (edit mode).
    catch(row, move |_| {
        let b = b.clone();
        floem::action::exec_after(Duration::ZERO, move |_| {
            if !(b.editing.get_untracked() && b.selected.get_untracked() == Some(key)) {
                select(&b, key);
            }
        });
    })
    .style(|s| s.width_full())
    .into_any()
}

/// A view that is told of each press in it (the number of clicks) before what is under the pointer takes it.
struct Catch {
    id: ViewId,
    on: Box<dyn Fn(u8)>,
}

fn catch(child: impl IntoView + 'static, on: impl Fn(u8) + 'static) -> Catch {
    let id = ViewId::new();
    id.set_children(vec![child.into_any()]);
    Catch { id, on: Box::new(on) }
}

impl View for Catch {
    fn id(&self) -> ViewId {
        self.id
    }

    fn event_before_children(&mut self, _: &mut floem::context::EventCx, event: &Event) -> EventPropagation {
        if let Event::PointerDown(p) = event
            && p.button.is_primary()
        {
            (self.on)(p.count);
        }
        EventPropagation::Continue
    }
}

/// A cell's left column: a code cell's run button (a spinner while it runs) and its count, as `[3]`, `[*]`.
fn gutter(ui: Ui, send: &Send, book: &Rc<Book>, l: &Rc<Live>) -> AnyView {
    let t = ui.colors;
    if l.kind != Kind::Code {
        return empty().style(|s| s.width(54.0)).into_any();
    }
    let (run_state, count) = (l.run, l.count);
    let (b, s, key) = (book.clone(), send.clone(), l.key);
    let busy = create_memo(move |_| matches!(run_state.get(), Run::Running | Run::Queued));
    let button =
        dyn_container(move || busy.get(), move |busy| icon(if busy { "loading" } else { "play" }, 16.0).into_any())
            .on_click_stop(move |_| {
                if let Some(k) = kernel()
                    && busy.get_untracked()
                {
                    (k.interrupt)(ui, &s, &b.path);
                } else {
                    b.selected.set(Some(key));
                    run(ui, &s, &b, &[key]);
                }
            })
            .style(move |s| {
                let c = t.get();
                s.width(22.0)
                    .height(22.0)
                    .items_center()
                    .justify_center()
                    .border_radius(5.0)
                    .color(c.icon)
                    .cursor(CursorStyle::Pointer)
                    .hover(|s| s.background(c.toolbar_hover))
            });
    let button = tooltip(button, move || {
        label(move || if busy.get_untracked() { "Stop Cell Execution" } else { "Execute Cell (Ctrl+Enter)" })
    });
    let shown = label(move || count_text(run_state.get(), count.get())).style(move |s| {
        let c = t.get();
        let colour = match run_state.get() {
            Run::Done(false) => c.nb_error,
            _ => c.desc,
        };
        s.font_size(11.0).font_family(MONO_FONT.clone()).color(colour).selectable(false)
    });
    v_stack((button, shown)).style(|s| s.width(54.0).items_center().gap(2.0).padding_top(4.0)).into_any()
}

/// A code cell's count as Jupyter shows it: `[*]` while it runs, `[ ]` before it has run.
fn count_text(run: Run, count: Option<u64>) -> String {
    match (run, count) {
        (Run::Running | Run::Queued, _) => "[*]".into(),
        (_, Some(n)) => format!("[{n}]"),
        (_, None) => "[ ]".into(),
    }
}

/// The tools of the cell selected: Move Up, Move Down, Change to Markdown or Code, Clear Outputs, Delete.
fn cell_tools(ui: Ui, book: &Rc<Book>, l: &Rc<Live>) -> AnyView {
    let t = ui.colors;
    let key = l.key;
    let kind = l.kind;
    let selected = book.selected;
    let on = create_memo(move |_| selected.get() == Some(key));
    let b = book.clone();
    dyn_container(
        move || on.get(),
        move |on| {
            if !on {
                return empty().style(|s| s.width(0.0)).into_any();
            }
            let (b1, b2, b3, b4, b5) = (b.clone(), b.clone(), b.clone(), b.clone(), b.clone());
            let other = if kind == Kind::Code { Kind::Markdown } else { Kind::Code };
            let mut items: Vec<AnyView> = vec![
                tip(t, "arrow-up", "Move Cell Up", move || {
                    change(ui, &b1, Change::Move(key, true));
                }),
                tip(t, "arrow-down", "Move Cell Down", move || {
                    change(ui, &b2, Change::Move(key, false));
                }),
                tip(
                    t,
                    if other == Kind::Markdown { "markdown" } else { "code" },
                    if other == Kind::Markdown { "Change Cell to Markdown" } else { "Change Cell to Code" },
                    move || to_kind(ui, &b3, Some(key), other),
                ),
            ];
            if kind == Kind::Code {
                items.push(tip(t, "clear-all", "Clear Cell Outputs", move || {
                    change(ui, &b4, Change::Clear(Some(key)));
                }));
            }
            items.push(tip(t, "trash", "Delete Cell", move || delete(ui, &b5, key)));
            h_stack_from(items)
                .style(move |s| {
                    let c = t.get();
                    s.gap(2.0)
                        .padding(2.0)
                        .border(1.0)
                        .border_radius(4.0)
                        .border_color(c.nb_cell_border)
                        .background(c.editor_bg)
                })
                .into_any()
        },
    )
    .into_any()
}

fn h_stack_from(items: Vec<AnyView>) -> impl View {
    floem::views::h_stack_from_iter(items)
}

fn tip(t: Theme, name: &'static str, text: &'static str, on: impl Fn() + 'static) -> AnyView {
    tooltip(crate::widgets::action(t, name, on), move || label(move || text)).into_any()
}

/// A cell's editor: as tall as its text, its language's colours, the editor's font; its keys the notebook's first
/// (Escape, the runs), then the editor's and the workbench's.
fn cell_editor(ui: Ui, send: &Send, book: &Rc<Book>, l: &Rc<Live>) -> AnyView {
    let t = ui.colors;
    let key = l.key;
    let map = KeypressMap::default();
    let (b, s) = (book.clone(), send.clone());
    let file = match l.kind {
        Kind::Code => cell_file(&book.lang.borrow()),
        _ => "cell.md".into(),
    };
    let o = crate::editor::options::memo(ui, move || Some(file.clone()));
    let (selected, editing) = (book.selected, book.editing);
    l.colours.set_theme(crate::highlight::theme_of(&ui.theme_json()));
    let field = text_editor_keys("", move |editor, press, mods| {
        if let KeyInput::Keyboard(k, _) = &press.key {
            let m = mods;
            let none = !m.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::META);
            match k {
                Key::Named(NamedKey::Escape) if none => {
                    if let Some(l) = b.live(key) {
                        leave_markdown(&l);
                    }
                    select(&b, key);
                    return CommandExecuted::Yes;
                }
                Key::Named(NamedKey::Enter) if m.contains(Modifiers::SHIFT) && !m.contains(Modifiers::CONTROL) => {
                    run_then(ui, &s, &b, Then::Next);
                    return CommandExecuted::Yes;
                }
                Key::Named(NamedKey::Enter) if m.contains(Modifiers::CONTROL) && !m.contains(Modifiers::SHIFT) => {
                    run_then(ui, &s, &b, Then::Stay);
                    return CommandExecuted::Yes;
                }
                Key::Named(NamedKey::Enter) if m.contains(Modifiers::ALT) => {
                    run_then(ui, &s, &b, Then::Insert);
                    return CommandExecuted::Yes;
                }
                _ => {}
            }
        }
        if crate::editor::keys::editor_key(ui, press, mods) {
            return CommandExecuted::Yes;
        }
        match crate::editor::keys::default_key(&map, editor, press, mods) {
            CommandExecuted::No => {
                if let KeyInput::Keyboard(k, _) = &press.key
                    && crate::keys_hook::editor(ui, k, mods)
                {
                    crate::editor::tell(ui, |send| crate::workbench::key(ui, send, k, mods));
                }
                CommandExecuted::No
            }
            done => done,
        }
    })
    .use_doc(l.doc.clone())
    .styling_rc(l.colours.clone())
    .editor_style(move |s| {
        let c = t.get();
        s.hide_gutter(true)
            .scroll_beyond_last_line(false)
            .cursor_color(if selected.get() == Some(key) && editing.get() {
                c.cursor
            } else {
                floem::peniko::Color::TRANSPARENT
            })
            .selection_color(c.selection)
            .current_line_color(floem::peniko::Color::TRANSPARENT)
            .indent_guide_color(c.indent_guide)
            .phantom_color(c.desc)
            .placeholder_color(c.placeholder)
    });
    let editor = field.editor().clone();
    {
        let first = o.get_untracked();
        l.colours.set_look(&first.font, &[], t.get_untracked().editor_ruler);
        editor.update_styling(l.colours.clone());
    }
    book.editors.borrow_mut().insert(key, editor.clone());
    // As tall as its lines (one at least).
    let rows = floem::reactive::create_rw_signal(1usize);
    {
        let (doc, rev) = (l.doc.clone(), l.doc.cache_rev());
        create_effect(move |_| {
            rev.track();
            let n = doc.rope_text().num_lines().max(1);
            if rows.get_untracked() != n {
                rows.set(n);
            }
        });
    }
    // Typed in: the notebook has changes.
    {
        let (rev, b) = (l.doc.cache_rev(), book.clone());
        create_effect(move |first: Option<()>| {
            rev.track();
            if first.is_some() {
                dirty(ui, &b);
            }
        });
    }
    // Given the keys (clicked, or Enter): edit mode in this cell.
    {
        let (gained, b) = (editor.editor_view_focused, book.clone());
        create_effect(move |first: Option<()>| {
            gained.track();
            if first.is_some() {
                if b.selected.get_untracked() != Some(key) {
                    b.selected.set(Some(key));
                }
                if !b.editing.get_untracked() {
                    b.editing.set(true);
                }
            }
        });
    }
    let line = f64::from(editor.line_height(0)).max(16.0);
    let field = field.style(move |s| {
        let c = t.get();
        s.width_full()
            .height(rows.get() as f64 * line)
            .set(floem::views::editor::Foreground, c.editor_fg)
            .color(c.editor_fg)
    });
    let wanted = book.focus_wanted.get() == Some(key);
    if wanted {
        book.focus_wanted.set(None);
        let ed = editor.clone();
        floem::action::exec_after(Duration::ZERO, move |_| {
            if let Some(v) = ed.editor_view_id.get_untracked() {
                v.request_focus();
                v.scroll_to(None);
            }
        });
    }
    let b = book.clone();
    let field = field.on_cleanup(move || {
        let eid = editor.id();
        let mut eds = b.editors.borrow_mut();
        if eds.get(&key).is_some_and(|e| e.id() == eid) {
            eds.remove(&key);
        }
    });
    container(field)
        .style(move |s| {
            let c = t.get();
            let on = selected.get() == Some(key) && editing.get();
            s.width_full()
                .padding_vert(8.0)
                .padding_horiz(10.0)
                .border(1.0)
                .border_radius(4.0)
                .border_color(if on { c.nb_focused_editor } else { c.nb_cell_border })
                .background(c.nb_editor_bg)
        })
        .into_any()
}

/// A Markdown cell: drawn as it reads, or its editor while it is edited (double-click, or Enter).
fn markdown_cell(ui: Ui, send: &Send, book: &Rc<Book>, l: &Rc<Live>) -> AnyView {
    let t = ui.colors;
    let editing = l.editing;
    let on = create_memo(move |_| editing.get());
    let (b, s, live) = (book.clone(), send.clone(), l.clone());
    dyn_container(
        move || on.get(),
        move |on| {
            if on {
                return cell_editor(ui, &s, &b, &live);
            }
            let shown = live.shown;
            let blocks = create_memo(move |_| {
                shown.with(|t| crate::markdown::parse(t).into_iter().map(Rc::new).collect::<Vec<_>>())
            });
            let folder = folder_of(&b.path).to_string();
            let to = s.clone();
            let doc = Doc {
                target: Box::new(move |href| crate::md_preview::target(&folder, href)),
                follow: Box::new(move |link| follow(ui, &to, link)),
                picture: Box::new(move |_src, alt| {
                    let alt = alt.to_string();
                    label(move || alt.clone()).style(move |s| s.color(t.get().desc)).into_any()
                }),
                placed: Box::new(|_, _| {}),
            };
            let empty_text = create_memo(move |_| shown.with(|s| s.trim().is_empty()));
            let (bb, key) = (b.clone(), live.key);
            let drawn = dyn_container(
                move || empty_text.get(),
                move |nothing| {
                    if nothing {
                        label(|| "Empty markdown cell, double-click or press Enter to edit.")
                            .style(move |s| s.color(t.get().desc).font_style(floem::text::Style::Italic))
                            .into_any()
                    } else {
                        empty().into_any()
                    }
                },
            );
            let text = v_stack((drawn, prose::document(ui, blocks, doc).style(|s| s.width_full())))
                .style(|s| s.width_full().padding_horiz(10.0).padding_vert(4.0).min_height(24.0));
            // A double-click edits it (seen before its text, which takes the presses to select words).
            catch(text, move |count| {
                if count >= 2 {
                    edit(&bb, key);
                }
            })
            .style(|s| s.width_full())
            .into_any()
        },
    )
    .style(|s| s.width_full())
    .into_any()
}

fn folder_of(path: &str) -> &str {
    path.rfind('/').map_or("", |at| &path[..at])
}

fn follow(ui: Ui, send: &Send, to: &crate::citations::Target) {
    use crate::citations::Target;
    match to {
        Target::File(r) => match r.line {
            Some(line) => ui.open_at(&r.path, line, send),
            None => ui.open(&r.path, send),
        },
        Target::Web(_) => crate::citations::open(ui, to),
        Target::Heading(_) => {}
    }
}

/// A code cell's outputs: as the output renderers draw them, else the text of each.
fn outputs_view(ui: Ui, send: &Send, path: &str, l: &Rc<Live>) -> AnyView {
    if let Some(f) = OUTPUTS.with(|o| o.borrow().clone()) {
        return f(ui, send, path, l.outputs);
    }
    let t = ui.colors;
    let outputs = l.outputs;
    let text = create_memo(move |_| outputs.with(|o| plain(o)));
    dyn_container(
        move || text.get(),
        move |shown| {
            if shown.is_empty() {
                return empty().into_any();
            }
            label(move || shown.clone())
                .style(move |s| {
                    let c = t.get();
                    s.width_full()
                        .padding_horiz(10.0)
                        .padding_vert(6.0)
                        .font_family(MONO_FONT.clone())
                        .font_size(12.0)
                        .color(c.editor_fg)
                })
                .into_any()
        },
    )
    .style(|s| s.width_full())
    .into_any()
}

/// As many lines of a cell's outputs as are drawn as text.
const MOST_LINES: usize = 500;

/// A cell's outputs as plain text: what it printed, its value's text, an error's name, words and traceback (their
/// terminal colours taken out). Pictures and other forms say what they are.
fn plain(outputs: &[Value]) -> String {
    let mut out = String::new();
    for o in outputs {
        let part = match o["output_type"].as_str().unwrap_or("") {
            "stream" => model::text_of(&o["text"]),
            "execute_result" | "display_data" => {
                let data = &o["data"];
                if !data["text/plain"].is_null() {
                    model::text_of(&data["text/plain"])
                } else {
                    data.as_object()
                        .and_then(|d| d.keys().next().cloned())
                        .map(|mime| format!("<{mime}>"))
                        .unwrap_or_default()
                }
            }
            "error" => {
                let trace: Vec<String> =
                    o["traceback"].as_array().into_iter().flatten().filter_map(Value::as_str).map(strip_ansi).collect();
                if trace.is_empty() {
                    format!("{}: {}", o["ename"].as_str().unwrap_or(""), o["evalue"].as_str().unwrap_or(""))
                } else {
                    trace.join("\n")
                }
            }
            _ => String::new(),
        };
        if !part.is_empty() {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&part);
        }
    }
    let lines: Vec<&str> = out.lines().collect();
    if lines.len() > MOST_LINES {
        let more = lines.len() - MOST_LINES;
        return format!("{}\n… {more} more lines", lines[..MOST_LINES].join("\n"));
    }
    out.trim_end_matches('\n').to_string()
}

/// A text's terminal escape codes (colours, moves, titles) taken out.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(']') => {
                for c in chars.by_ref() {
                    if c == '\u{7}' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

// --- What a kernel tells -----------------------------------------------------------------------------------

/// Where a cell's run is (`[*]` while it runs), as the kernel says.
#[allow(dead_code)] // Called by the notebook kernel's part.
pub fn set_run(ui: Ui, path: &str, key: u64, run: Run) {
    if let Some(l) = book(ui, path).and_then(|b| b.live(key))
        && l.run.get_untracked() != run
    {
        l.run.set(run);
    }
}

/// A cell's outputs and count, as the kernel says them: the notebook has changes.
#[allow(dead_code)] // Called by the notebook kernel's part.
pub fn set_outputs(ui: Ui, path: &str, key: u64, outputs: Vec<Value>, count: Option<u64>) {
    let Some(b) = book(ui, path) else { return };
    let Some(l) = b.live(key) else { return };
    if l.outputs.with_untracked(|o| **o != outputs) {
        l.outputs.set(Arc::new(outputs));
    }
    if l.count.get_untracked() != count {
        l.count.set(count);
    }
    dirty(ui, &b);
}

// --- The part --------------------------------------------------------------------------------------------------

/// The notebook shown, by its file's path (none: the tab shown is no notebook).
fn notebook_shown(ui: Ui) -> Option<String> {
    let active = ui.active.get_untracked()?;
    kinds::split(&active).filter(|(k, _)| *k == KIND).map(|(_, rest)| rest.to_string())
}

/// A notebook's file shown as text, by its path.
fn text_shown(ui: Ui) -> Option<String> {
    let active = ui.active.get_untracked()?;
    (is_notebook(&active) && ui.tab(&active).is_some_and(|t| t.is_file())).then_some(active)
}

/// Reopen Editor With…: the notebook shown as its cells or as its text.
fn reopen_with(ui: Ui, send: &Send) {
    let (path, now) = match (notebook_shown(ui), text_shown(ui)) {
        (Some(p), _) => (p, "notebook"),
        (None, Some(p)) => (p, "text"),
        _ => return,
    };
    let send = send.clone();
    quick::pick("Reopen Editor With")
        .placeholder(&format!("Select editor for '{}'", name_of(&path)))
        .items(vec![
            quick::item("notebook", "Jupyter Notebook").description("Default"),
            quick::item("text", "Text Editor").description("Built-in"),
        ])
        .active(now)
        .open(ui, move |item| match item.map(|i| i.id) {
            Some(id) if id == now => {}
            Some(id) if id == "text" => open_as_text(ui, &send, &path),
            Some(_) => open_as_notebook(ui, &send, &path),
            None => {}
        });
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    kinds::add(
        kinds::kind(KIND, |_ui, rest| name_of(rest).to_string(), |ui, send, tab| view(ui, send, &tab.file))
            .icon("notebook")
            .save(|ui, send, tab| save(ui, send, &tab.file))
            .kept(|ui, send, path| {
                if let Some((_, rest)) = kinds::split(path) {
                    open(ui, send, rest);
                }
            }),
    );
    commands::add(command("ipynb.newUntitledIpynb", "Create", "New Jupyter Notebook", create));
    commands::add(
        command("workbench.action.reopenWithEditor", "View", "Reopen Editor With...", reopen_with)
            .when(|ui| notebook_shown(ui).is_some() || text_shown(ui).is_some()),
    );
    commands::add(
        command("notebook.execute", "Notebook", "Run All", |ui, send| {
            if let Some(b) = notebook_shown(ui).and_then(|p| book(ui, &p)) {
                let keys: Vec<u64> = b.cells.with_untracked(|c| c.iter().map(|l| l.key).collect());
                run(ui, send, &b, &keys);
            }
        })
        .when(|ui| notebook_shown(ui).is_some()),
    );
    commands::add(
        command("notebook.clearAllCellsOutputs", "Notebook", "Clear All Outputs", |ui, _| {
            if let Some(b) = notebook_shown(ui).and_then(|p| book(ui, &p)) {
                change(ui, &b, Change::Clear(None));
            }
        })
        .when(|ui| notebook_shown(ui).is_some()),
    );
    commands::add(
        command("notebook.cancelExecution", "Notebook", "Interrupt", |ui, send| {
            if let (Some(p), Some(k)) = (notebook_shown(ui), kernel()) {
                (k.interrupt)(ui, send, &p);
            }
        })
        .when(|ui| notebook_shown(ui).is_some()),
    );
    commands::add(
        command("notebook.restartKernel", "Notebook", "Restart Kernel", |ui, send| {
            if let (Some(p), Some(k)) = (notebook_shown(ui), kernel()) {
                (k.restart)(ui, send, &p);
            }
        })
        .when(|ui| notebook_shown(ui).is_some()),
    );
    commands::add(
        command("notebook.openAsText", "Notebook", "Open as Text", |ui, send| {
            if let Some(p) = notebook_shown(ui) {
                open_as_text(ui, send, &p);
            }
        })
        .when(|ui| notebook_shown(ui).is_some()),
    );
    breadcrumbs::end("notebook", "Open as Notebook", |_ui, file| is_notebook(file), open_as_notebook);
}

/// The session's messages of the notebooks: an `.ipynb`'s file that has no text tab, opened (or read again) as a
/// notebook, and the `saved` of a notebook written; both taken away from the parts after this one, as theirs is
/// the text editor's.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let path = m["path"].as_str().unwrap_or("");
    match m["type"].as_str().unwrap_or("") {
        // A notebook whose tab waits for its file that could not be read: its tab goes, and the editor says why.
        "file" if is_notebook(path) && !m["error"].is_null() => {
            if book(ui, path).is_some_and(|b| b.load.get_untracked() == Load::Reading) {
                forget(ui, path);
                if ui.tab(&tab_of(path)).is_some() {
                    ui.close(&tab_of(path));
                }
            }
            false
        }
        "file" if is_notebook(path) => {
            let books = ui.state::<Books>();
            if books.as_text.with_untracked(|a| a.contains(path)) {
                books.as_text.update(|a| {
                    a.remove(path);
                });
                return false;
            }
            if ui.tab(path).is_some() {
                return false;
            }
            let text = m["text"].as_str().unwrap_or("").to_string();
            let modified = m["modified"].as_u64();
            let id = tab_of(path);
            let b = match book(ui, path) {
                // Asked again while it has changes of its own: they are kept.
                Some(_) if ui.tab(&id).is_some_and(|t| t.dirty.get_untracked()) => {
                    kinds::open(ui, &id);
                    return true;
                }
                Some(b) if ui.tab(&id).is_some() => b,
                _ => new_book(ui, path),
            };
            kinds::open(ui, &id);
            read(ui, send, b, text, modified);
            // The order of a kept layout's tabs is put right as each file comes (`kinds`).
            kinds::take(ui, m, send);
            true
        }
        "saved" => {
            let books = ui.state::<Books>();
            if books.making.with_untracked(|s| s.contains(path)) {
                books.making.update(|s| {
                    s.remove(path);
                });
                match m["error"].as_str() {
                    None => {}
                    Some("changed") => {
                        toast::info(ui, &format!("{path} is there already: it opens as it is."));
                    }
                    Some(e) => {
                        toast::warning(ui, &format!("{path} could not be made: {e}"));
                        return true;
                    }
                }
                ui.open(path, send);
                return true;
            }
            let Some(b) = book(ui, path).filter(|b| b.saving.get()) else { return false };
            b.saving.set(false);
            match m["error"].as_str() {
                None => {
                    b.modified.set(m["modified"].as_u64());
                    kinds::saved(ui, &tab_of(path));
                }
                Some("changed") => {
                    // Saved again, it is written over what changed.
                    b.modified.set(m["modified"].as_u64());
                    toast::warning(
                        ui,
                        &format!("{} changed on disk since it was read: save again to write over it.", name_of(path)),
                    );
                }
                Some(e) => ui.message.set(format!("Could not save {path}: {e}")),
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn sender() -> (Send, Rc<RefCell<Vec<Value>>>) {
        let said = Rc::new(RefCell::new(Vec::new()));
        let to = said.clone();
        (Rc::new(move |m| to.borrow_mut().push(m)), said)
    }

    fn ui() -> (Ui, Send, Rc<RefCell<Vec<Value>>>) {
        let ui = Ui::new();
        let (send, said) = sender();
        register(ui, &send);
        (ui, send, said)
    }

    const NB: &str = "{\"cells\": [{\"cell_type\": \"code\", \"execution_count\": 1, \"metadata\": {}, \
        \"outputs\": [{\"name\": \"stdout\", \"output_type\": \"stream\", \"text\": [\"hi\\n\"]}], \
        \"source\": [\"print('hi')\"]}, {\"cell_type\": \"markdown\", \"metadata\": {}, \"source\": [\"# A\"]}], \
        \"metadata\": {}, \"nbformat\": 4, \"nbformat_minor\": 4}";

    fn opened(ui: Ui, send: &Send) -> Rc<Book> {
        let file = json!({"type": "file", "path": "a.ipynb", "text": NB, "modified": 7});
        assert!(take(ui, &file, send), "a notebook's file is taken");
        book(ui, "a.ipynb").expect("open")
    }

    #[test]
    fn an_ipynb_opens_as_its_cells_in_a_tab_of_its_own() {
        let (ui, send, _) = ui();
        let b = opened(ui, &send);
        assert_eq!(ui.active.get_untracked().as_deref(), Some("notebook:a.ipynb"));
        assert!(ui.tab("a.ipynb").is_none(), "no text tab");
        assert_eq!(b.len(), 2);
        assert_eq!(b.load.get_untracked(), Load::Ready);
        assert_eq!(b.selected.get_untracked(), b.key_at(0));
        let first = b.live(b.key_at(0).expect("there")).expect("there");
        assert_eq!(first.count.get_untracked(), Some(1));
        assert_eq!(plain(&first.outputs.get_untracked()), "hi");
        // Another file is not taken.
        assert!(!take(ui, &json!({"type": "file", "path": "a.py", "text": "", "modified": 1}), &send));
    }

    #[test]
    fn saved_as_it_was_read_and_its_saved_taken() {
        let (ui, send, said) = ui();
        let b = opened(ui, &send);
        kinds::save(ui, &send, &ui.tab("notebook:a.ipynb").expect("there"));
        let sent = said.borrow().last().cloned().expect("a save");
        assert_eq!(sent["type"], "save");
        assert_eq!(sent["modified"], 7);
        let text = sent["text"].as_str().expect("there");
        assert_eq!(model::parse(text).expect("there").cells.len(), 2);
        assert!(text.ends_with("}\n") && text.contains("\n \"cells\": ["));
        dirty(ui, &b);
        assert!(take(ui, &json!({"type": "saved", "path": "a.ipynb", "modified": 9}), &send));
        assert!(!ui.tab("notebook:a.ipynb").expect("there").dirty.get_untracked());
        assert_eq!(b.modified.get(), Some(9));
        // Another's saved is left to the others.
        assert!(!take(ui, &json!({"type": "saved", "path": "a.ipynb", "modified": 10}), &send));
    }

    #[test]
    fn command_mode_keys_move_add_delete_and_change_cells() {
        let (ui, send, _) = ui();
        let b = opened(ui, &send);
        let ch = |c: &str| Key::Character(c.into());
        let none = Modifiers::empty();
        let (one, two) = (b.key_at(0).expect("there"), b.key_at(1).expect("there"));
        assert!(command_key(ui, &send, &b, &ch("j"), none));
        assert_eq!(b.selected.get_untracked(), Some(two));
        assert!(command_key(ui, &send, &b, &Key::Named(NamedKey::ArrowUp), none));
        assert_eq!(b.selected.get_untracked(), Some(one));
        assert!(command_key(ui, &send, &b, &ch("b"), none));
        assert_eq!(b.len(), 3);
        assert_eq!(b.at(b.selected.get_untracked().expect("there")), Some(1), "added below, and selected");
        assert!(ui.tab("notebook:a.ipynb").expect("there").dirty.get_untracked());
        let added = b.selected.get_untracked().expect("there");
        assert!(command_key(ui, &send, &b, &ch("m"), none));
        assert_eq!(b.live(added).expect("there").kind, Kind::Markdown);
        assert!(command_key(ui, &send, &b, &ch("d"), none));
        assert_eq!(b.len(), 3, "one D deletes nothing");
        assert!(command_key(ui, &send, &b, &ch("d"), none));
        assert_eq!(b.len(), 2);
        assert_eq!(b.selected.get_untracked(), Some(two), "the next selected");
        // Undo takes back the deletion, then the change to Markdown.
        assert!(command_key(ui, &send, &b, &ch("z"), none));
        assert_eq!(b.len(), 3);
        assert_eq!(b.live(added).expect("there").kind, Kind::Markdown);
        assert!(command_key(ui, &send, &b, &ch("z"), none));
        assert_eq!(b.live(added).expect("there").kind, Kind::Code);
        assert!(command_key(ui, &send, &b, &ch("z"), Modifiers::SHIFT), "redo");
        assert_eq!(b.live(added).expect("there").kind, Kind::Markdown);
        assert!(!command_key(ui, &send, &b, &ch("q"), none), "not a key of the notebook's");
    }

    #[test]
    fn a_cell_keeps_its_text_across_changes_and_undo_puts_it_back() {
        let (ui, send, _) = ui();
        let b = opened(ui, &send);
        let one = b.key_at(0).expect("there");
        let doc = b.live(one).expect("there").doc.clone();
        fill(&doc, "x = 1");
        change(ui, &b, Change::Move(one, false));
        assert!(Rc::ptr_eq(&b.live(one).expect("there").doc, &doc), "the same editor's text");
        assert_eq!(b.at(one), Some(1));
        undo(ui, &b, true);
        assert_eq!(b.at(one), Some(0));
        assert_eq!(doc.text().to_string(), "x = 1");
    }

    #[test]
    fn copy_and_paste_and_shift_enter_after_the_last() {
        let (ui, send, said) = ui();
        let b = opened(ui, &send);
        let ch = |c: &str| Key::Character(c.into());
        let none = Modifiers::empty();
        assert!(command_key(ui, &send, &b, &ch("c"), none));
        assert!(command_key(ui, &send, &b, &ch("v"), none));
        assert_eq!(b.len(), 3);
        let pasted = b.live(b.key_at(1).expect("there")).expect("there");
        assert_eq!(pasted.doc.text().to_string(), "print('hi')");
        // Shift+Enter on the last runs it (no kernel: it says so) and adds a code cell after it.
        let last = b.key_at(2).expect("there");
        b.selected.set(Some(last));
        assert!(command_key(ui, &send, &b, &Key::Named(NamedKey::Enter), Modifiers::SHIFT));
        assert_eq!(b.len(), 4);
        assert_eq!(b.live(b.key_at(3).expect("there")).expect("there").kind, Kind::Code);
        assert!(said.borrow().iter().all(|m| m["type"] != "save"));
    }

    #[test]
    fn open_as_text_and_back() {
        let (ui, send, said) = ui();
        opened(ui, &send);
        open_as_text(ui, &send, "a.ipynb");
        assert!(ui.tab("notebook:a.ipynb").is_none());
        assert_eq!(said.borrow().last(), Some(&json!({"type": "open", "path": "a.ipynb"})));
        let file = json!({"type": "file", "path": "a.ipynb", "text": NB, "modified": 7});
        // Asked as text: the text editor's (the window gives it to every part, this one first).
        ui.take(&file, &send);
        assert!(ui.tab("a.ipynb").is_some());
        assert!(!take(ui, &file, &send), "its text tab is open");
        open_as_notebook(ui, &send, "a.ipynb");
        assert!(ui.tab("a.ipynb").is_none());
        assert!(take(ui, &file, &send));
        assert!(ui.tab("notebook:a.ipynb").is_some());
    }

    #[test]
    fn a_file_that_is_no_notebook_opens_as_text() {
        let (ui, send, said) = ui();
        let file = json!({"type": "file", "path": "old.ipynb", "text": "{\"nbformat\": 3, \"worksheets\": []}", "modified": 1});
        assert!(take(ui, &file, &send));
        assert!(ui.tab("notebook:old.ipynb").is_none());
        assert_eq!(said.borrow().last(), Some(&json!({"type": "open", "path": "old.ipynb"})));
        assert!(!take(ui, &file, &send), "the answer is the text editor's");
    }

    #[test]
    fn a_new_notebook_is_made_only_where_there_is_none() {
        let (ui, send, said) = ui();
        ui.state::<Books>().making.update(|m| {
            m.insert("n.ipynb".into());
        });
        assert!(take(ui, &json!({"type": "saved", "path": "n.ipynb", "error": "changed", "modified": 3}), &send));
        assert_eq!(said.borrow().last(), Some(&json!({"type": "open", "path": "n.ipynb"})));
    }

    #[test]
    fn outputs_as_text_and_counts_as_jupyter_shows_them() {
        let outs = vec![
            json!({"output_type": "stream", "name": "stdout", "text": ["a\n", "b\n"]}),
            json!({"output_type": "execute_result", "data": {"text/plain": ["3"]}, "execution_count": 2}),
            json!({"output_type": "display_data", "data": {"image/png": "AAAA"}}),
            json!({"output_type": "error", "ename": "E", "evalue": "v", "traceback": ["\u{1b}[31mE\u{1b}[0m: v"]}),
        ];
        assert_eq!(plain(&outs), "a\nb\n3\n<image/png>\nE: v");
        assert_eq!(count_text(Run::Running, Some(2)), "[*]");
        assert_eq!(count_text(Run::Idle, Some(2)), "[2]");
        assert_eq!(count_text(Run::Idle, None), "[ ]");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}x"), "x");
    }

    #[test]
    fn the_editor_is_built() {
        crate::icons::load();
        let (ui, send, _) = ui();
        let b = opened(ui, &send);
        let _view = view(ui, send.clone(), "a.ipynb").into_view();
        let _cell = cell_view(ui, &send, &b, b.live(b.key_at(0).expect("there")).expect("there")).into_view();
        let _md = cell_view(ui, &send, &b, b.live(b.key_at(1).expect("there")).expect("there")).into_view();
    }

    #[test]
    fn undo_takes_back_the_change_not_what_was_typed_since() {
        let (ui, send, _) = ui();
        let b = opened(ui, &send);
        let one = b.key_at(0).expect("there");
        insert(ui, &b, 2, Kind::Code, false);
        fill(&b.live(one).expect("there").doc, "typed after");
        undo(ui, &b, true);
        assert_eq!(b.len(), 2);
        assert_eq!(b.live(one).expect("there").doc.text().to_string(), "typed after");
    }

    #[test]
    fn the_workbenchs_keys_are_not_the_cells() {
        let (ui, send, _) = ui();
        let b = opened(ui, &send);
        let ch = |c: &str| Key::Character(c.into());
        // Ctrl+Shift+X (Extensions), Ctrl+Shift+C, Alt+M are the workbench's.
        assert!(!command_key(ui, &send, &b, &ch("x"), Modifiers::CONTROL | Modifiers::SHIFT));
        assert!(!command_key(ui, &send, &b, &ch("c"), Modifiers::CONTROL | Modifiers::SHIFT));
        assert!(!command_key(ui, &send, &b, &ch("m"), Modifiers::ALT));
        assert_eq!(b.len(), 2);
        assert!(command_key(ui, &send, &b, &ch("z"), Modifiers::CONTROL | Modifiers::SHIFT), "redo");
    }

    #[test]
    fn a_notebook_that_cannot_be_read_leaves_no_tab_waiting() {
        let (ui, send, said) = ui();
        open(ui, &send, "gone.ipynb");
        assert_eq!(said.borrow().last(), Some(&json!({"type": "open", "path": "gone.ipynb"})));
        assert!(ui.tab("notebook:gone.ipynb").is_some());
        let failed = json!({"type": "file", "path": "gone.ipynb", "error": "not there", "kind": "other"});
        assert!(!take(ui, &failed, &send), "the editor says why");
        assert!(ui.tab("notebook:gone.ipynb").is_none());
        assert!(book(ui, "gone.ipynb").is_none());
    }
}
