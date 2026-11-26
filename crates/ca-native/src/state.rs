//! Everything the window shows, as signals, and the session's messages taken into them as the page takes them.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith, create_rw_signal};
use floem::views::editor::Editor;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use serde_json::{Value, json};

use crate::colors::Colors;
use crate::dock::Dock;
use crate::model::{Item, Kind};
use crate::outline::Symbol;

pub type Send = Rc<dyn Fn(Value)>;

/// Where a path is once `from` has become `to`: `from` itself, or anything in it; none when it is neither.
pub fn moved_path(path: &str, from: &str, to: &str) -> Option<String> {
    let rest = path.strip_prefix(from)?;
    (rest.is_empty() || rest.starts_with('/')).then(|| format!("{to}{rest}"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
}

/// A line the Search view found.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub path: String,
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub hits: Vec<Hit>,
    pub more: bool,
    pub error: String,
}

/// A file the agent changed in this session: what it was, what it is (none: it was not there, or is gone).
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

impl Change {
    /// VS Code's letter for it: Added, Deleted, Modified.
    pub fn letter(&self) -> &'static str {
        match (&self.before, &self.after) {
            (None, _) => "A",
            (_, None) => "D",
            _ => "M",
        }
    }
}

/// What became of an open file on disk while it has changes not saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDisk {
    Changed,
    Gone,
}

/// An open editor: its file, its text (kept while another tab is shown), whether it has changes not saved; or a
/// file's changes, shown as a diff (its `path`, the tab's own name among the tabs, is then `changes:` and the file).
#[derive(Clone)]
pub struct Tab {
    pub path: String,
    pub file: String,
    pub doc: Rc<TextDocument>,
    pub dirty: RwSignal<bool>,
    pub diff: Option<Rc<Vec<crate::diff::Line>>>,
    /// Its syntax colours (and the editor's font), kept with it so a tab shown again is not parsed again.
    pub colours: Rc<crate::highlight::Colours>,
}

impl Tab {
    pub fn name(&self) -> String {
        let name = self.file.rsplit('/').next().unwrap_or(&self.file).to_string();
        if self.diff.is_some() { format!("{name} (changes)") } else { name }
    }

    pub fn text(&self) -> String {
        self.doc.text().to_string()
    }
}

/// A picker of the Agent view's input: which of its menus is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Picker {
    Mode,
    Model,
    Thinking,
}

#[derive(Clone, Copy)]
pub struct Ui {
    pub colors: RwSignal<Colors>,
    pub themes: RwSignal<Vec<ca_themes::Theme>>,
    pub choice: RwSignal<String>,

    pub items: RwSignal<Vec<Item>>,
    pub next: RwSignal<u64>,
    pub running: RwSignal<bool>,
    pub model: RwSignal<String>,
    pub mode: RwSignal<String>,
    /// The models the session can be given, by their ids, and how much it thinks (`default`, `off`, or a level).
    pub models: RwSignal<Vec<String>>,
    pub thinking: RwSignal<String>,
    /// The input's picker that is open, and the item in it the keys are at.
    pub picker: RwSignal<Option<Picker>>,
    pub picker_at: RwSignal<usize>,
    /// The theme's token colours, made once for a theme (an answer's code is coloured with them).
    pub tokens: RwSignal<Rc<syntect::highlighting::Theme>>,
    pub folder: RwSignal<String>,
    pub branch: RwSignal<String>,
    pub plan: RwSignal<Option<(String, usize, usize)>>,
    pub goal: RwSignal<Option<String>>,
    pub jobs: RwSignal<Vec<(u64, String)>>,
    pub draft: RwSignal<String>,

    /// Where the views are (the dock tree), the groups hidden, and each part's size by its number.
    pub dock: RwSignal<Dock>,
    pub hidden: RwSignal<HashSet<u32>>,
    pub sizes: RwSignal<HashMap<u32, RwSignal<f64>>>,
    /// A view being dragged to dock it.
    pub drag: RwSignal<Option<crate::docking::Drag>>,
    /// The window's size (the parts' sizes are bounded by it).
    pub window: RwSignal<(f64, f64)>,
    /// The title bar's menu open, by its place.
    pub menu: RwSignal<Option<usize>>,
    /// A view's own menu (Move to…), open where it was right-clicked (in the window).
    pub context: RwSignal<Option<(crate::layout::View, f64, f64)>>,
    pub changes: RwSignal<Vec<Change>>,
    /// The shell the session's terminals run (its name, for their tabs).
    pub shell: RwSignal<String>,
    /// The terminal the panel shows, and its screen.
    pub term: RwSignal<crate::terminal::State>,
    pub term_screen: RwSignal<crate::terminal::Screen>,
    /// The window's terminals, as the tabs list shows them.
    pub terms: RwSignal<Vec<crate::terminal::Shell>>,
    /// The number the next terminal takes (none is taken twice).
    pub term_next: RwSignal<u64>,
    /// The size the panel gives a terminal, in columns and lines (0: it has not been laid out yet).
    pub term_size: RwSignal<(usize, usize)>,
    /// Counts each time the terminal is to be given the keys (one made, one chosen in the list).
    pub term_focus: RwSignal<u64>,
    pub search_query: RwSignal<String>,
    pub search_case: RwSignal<bool>,
    pub search_regex: RwSignal<bool>,
    pub search: RwSignal<Option<Found>>,
    /// A line to show in a file once its editor is drawn: the file, the line (from 1), and a number of its own so
    /// the same line asked again is gone to again.
    pub goto: RwSignal<Option<(String, usize, u64)>>,
    /// The view being dragged, and the container it is over.
    /// Where the pointer is in the window (for a menu opened by a right-click).
    pub pointer: RwSignal<(f64, f64)>,
    /// The layout kept for the folder has been read: from then on a change is kept.
    pub loaded: RwSignal<bool>,

    pub tree: RwSignal<HashMap<String, Vec<Entry>>>,
    pub expanded: RwSignal<HashSet<String>>,
    pub selected: RwSignal<String>,
    /// The Explorer's inline input: what it is for (a new file or folder, a row renamed), and what is typed in it.
    pub explorer_edit: RwSignal<Option<crate::explorer::Edit>>,
    pub explorer_text: RwSignal<String>,
    /// The file this window asked the session to make: opened once it is there.
    pub explorer_asked: RwSignal<Option<String>>,
    /// The Explorer's menu or its question, open over the window; and the tree, which has the keys again after.
    pub explorer_popup: RwSignal<Option<floem::ViewId>>,
    pub explorer_tree: RwSignal<Option<floem::ViewId>>,
    pub tabs: RwSignal<Vec<Tab>>,
    pub active: RwSignal<Option<String>>,
    pub caret: RwSignal<(usize, usize)>,
    /// A line for the status bar: what was just done, or went wrong.
    pub message: RwSignal<String>,
    /// The quick input open, as it was opened: on files or on the themes (commands are the files' with ">" typed).
    pub quick: RwSignal<Option<crate::palette::Mode>>,
    pub quick_text: RwSignal<String>,
    /// The row chosen, among the rows shown.
    pub quick_at: RwSignal<usize>,
    pub quick_rows: RwSignal<Rc<Vec<crate::palette::Row>>>,
    /// The project's files, as the session listed them last.
    pub quick_files: RwSignal<Rc<crate::palette::Files>>,
    /// The theme in force when the themes were opened, gone back to when none is chosen.
    pub quick_theme: RwSignal<Option<String>>,
    /// The scope open editors' documents live in (not an effect's, which goes when the effect runs again).
    pub scope: Scope,

    /// The way to the session's driver, for the parts that are not handed it (the editor group's own keys).
    pub out: RwSignal<Option<Send>>,
    /// The editor drawn, and its tab: where the find widget and a file read again move the caret.
    pub live: RwSignal<Option<(String, Editor)>>,
    /// Counted up at each edit of an open file (the Outline and the find widget read the text again after it).
    pub edited: RwSignal<u64>,
    /// The Outline: the file it is of, and its symbols.
    pub symbols: RwSignal<(String, Rc<Vec<Symbol>>)>,
    /// The find widget: shown, what it looks for and how, where each match is in the text (from, to), and whether
    /// what it looks for cannot be read (a regular expression that is not one).
    pub find: RwSignal<bool>,
    pub find_query: RwSignal<String>,
    pub find_case: RwSignal<bool>,
    pub find_regex: RwSignal<bool>,
    pub find_hits: RwSignal<Rc<Vec<(usize, usize)>>>,
    pub find_error: RwSignal<bool>,
    /// Counted up when the find widget is asked for: its box takes the keys, with the editor's selection in it.
    pub find_asked: RwSignal<u64>,
    /// When each open file was last changed on disk, as the session said last.
    pub modified: RwSignal<HashMap<String, u64>>,
    /// The open files with changes not saved that changed on disk, or are gone from it.
    pub on_disk: RwSignal<HashMap<String, OnDisk>>,
    /// The files asked for again because they changed on disk (their text is taken, their tab is not shown).
    pub reloading: RwSignal<HashSet<String>>,
    /// The tab with changes not saved that is being closed: asked about first.
    pub closing: RwSignal<Option<String>>,
    /// The tab to close once it is saved (Save, in that question).
    pub close_saved: RwSignal<Option<String>>,
}

impl Ui {
    pub fn new() -> Ui {
        let themes = ca_themes::built_in();
        let start = ca_themes::chosen(&themes, "", true);
        Ui {
            colors: create_rw_signal(Colors::of(&start)),
            themes: create_rw_signal(themes),
            choice: create_rw_signal(String::new()),
            items: create_rw_signal(Vec::new()),
            next: create_rw_signal(1),
            running: create_rw_signal(false),
            model: create_rw_signal(String::new()),
            mode: create_rw_signal(String::new()),
            models: create_rw_signal(Vec::new()),
            thinking: create_rw_signal(String::new()),
            picker: create_rw_signal(None),
            picker_at: create_rw_signal(0),
            tokens: create_rw_signal(Rc::new(crate::highlight::theme_of(&start.json))),
            folder: create_rw_signal(String::new()),
            branch: create_rw_signal(String::new()),
            plan: create_rw_signal(None),
            goal: create_rw_signal(None),
            jobs: create_rw_signal(Vec::new()),
            draft: create_rw_signal(String::new()),
            dock: create_rw_signal(Dock::default()),
            hidden: create_rw_signal(HashSet::new()),
            sizes: create_rw_signal(HashMap::new()),
            drag: create_rw_signal(None),
            window: create_rw_signal((1280.0, 800.0)),
            menu: create_rw_signal(None),
            context: create_rw_signal(None),
            loaded: create_rw_signal(false),
            pointer: create_rw_signal((0.0, 0.0)),
            changes: create_rw_signal(Vec::new()),
            shell: create_rw_signal(String::new()),
            term: create_rw_signal(crate::terminal::State::None),
            term_screen: create_rw_signal(crate::terminal::Screen::default()),
            terms: create_rw_signal(Vec::new()),
            term_next: create_rw_signal(crate::terminal::FIRST_ID),
            term_size: create_rw_signal((0, 0)),
            term_focus: create_rw_signal(0),
            search_query: create_rw_signal(String::new()),
            search_case: create_rw_signal(false),
            search_regex: create_rw_signal(false),
            search: create_rw_signal(None),
            goto: create_rw_signal(None),
            tree: create_rw_signal(HashMap::new()),
            expanded: create_rw_signal(HashSet::new()),
            selected: create_rw_signal(String::new()),
            explorer_edit: create_rw_signal(None),
            explorer_text: create_rw_signal(String::new()),
            explorer_asked: create_rw_signal(None),
            explorer_popup: create_rw_signal(None),
            explorer_tree: create_rw_signal(None),
            tabs: create_rw_signal(Vec::new()),
            active: create_rw_signal(None),
            caret: create_rw_signal((1, 1)),
            message: create_rw_signal(String::new()),
            quick: create_rw_signal(None),
            quick_text: create_rw_signal(String::new()),
            quick_at: create_rw_signal(0),
            quick_rows: create_rw_signal(Rc::new(Vec::new())),
            quick_files: create_rw_signal(Rc::new(crate::palette::Files::default())),
            quick_theme: create_rw_signal(None),
            scope: Scope::new(),
            out: create_rw_signal(None),
            live: create_rw_signal(None),
            edited: create_rw_signal(0),
            symbols: create_rw_signal((String::new(), Rc::new(Vec::new()))),
            find: create_rw_signal(false),
            find_query: create_rw_signal(String::new()),
            find_case: create_rw_signal(false),
            find_regex: create_rw_signal(false),
            find_hits: create_rw_signal(Rc::new(Vec::new())),
            find_error: create_rw_signal(false),
            find_asked: create_rw_signal(0),
            modified: create_rw_signal(HashMap::new()),
            on_disk: create_rw_signal(HashMap::new()),
            reloading: create_rw_signal(HashSet::new()),
            closing: create_rw_signal(None),
            close_saved: create_rw_signal(None),
        }
    }

    pub(crate) fn id(&self) -> u64 {
        let n = self.next.get_untracked();
        self.next.set(n + 1);
        n
    }

    pub(crate) fn push(&self, kind: Kind) {
        let id = self.id();
        self.items.update(|l| l.push(Item { id, kind }));
    }

    /// The answer being written: the last item when it is one, else a new one.
    pub(crate) fn current(&self) -> Item {
        if let Some(last) = self.items.with_untracked(|l| l.last().cloned())
            && matches!(last.kind, Kind::Assistant { .. })
        {
            return last;
        }
        let item = Item { id: self.id(), kind: Kind::assistant(self.scope) };
        self.items.update(|l| l.push(item.clone()));
        item
    }

    /// The theme in force, as its JSON (for the editor's syntax colours).
    pub fn theme_json(&self) -> Value {
        ca_themes::chosen(&self.themes.get_untracked(), &self.choice.get_untracked(), true).json
    }

    fn apply_theme(&self) {
        let t = ca_themes::chosen(&self.themes.get_untracked(), &self.choice.get_untracked(), true);
        self.colors.set(Colors::of(&t));
        self.tokens.set(Rc::new(crate::highlight::theme_of(&t.json)));
        crate::terminal::recolour(*self);
    }

    /// A theme shown without being kept (the theme pick's preview, and its way back).
    pub fn show_theme(&self, choice: &str) {
        self.choice.set(choice.to_string());
        self.apply_theme();
    }

    pub fn tab(&self, path: &str) -> Option<Tab> {
        self.tabs.with_untracked(|t| t.iter().find(|t| t.path == path).cloned())
    }

    /// A file opened in an editor: its tab shown, or asked of the session when it is not open yet.
    pub fn open(&self, path: &str, send: &Send) {
        self.selected.set(path.to_string());
        if self.tab(path).is_some() {
            self.active.set(Some(path.to_string()));
        } else {
            send(json!({"type": "open", "path": path}));
        }
    }

    /// A file's changes in a tab of their own (made again from what the session said last).
    pub fn open_changes(&self, c: &Change) {
        let id = format!("changes:{}", c.path);
        let lines = crate::diff::lines(c.before.as_deref().unwrap_or(""), c.after.as_deref().unwrap_or(""));
        let tab = Tab {
            path: id.clone(),
            file: c.path.clone(),
            colours: crate::highlight::Colours::new(Rc::new(TextDocument::new(self.scope, "")), ""),
            doc: Rc::new(TextDocument::new(self.scope, "")),
            dirty: self.scope.create_rw_signal(false),
            diff: Some(Rc::new(lines)),
        };
        self.tabs.update(|t| match t.iter_mut().find(|t| t.path == id) {
            Some(old) => *old = tab,
            None => t.push(tab),
        });
        self.active.set(Some(id));
    }

    /// A file opened at a line (a search's hit).
    pub fn open_at(&self, path: &str, line: usize, send: &Send) {
        let n = self.goto.with_untracked(|g| g.as_ref().map(|g| g.2 + 1).unwrap_or(0));
        self.goto.set(Some((path.to_string(), line, n)));
        self.open(path, send);
    }

    /// A tab closed; one with changes not saved is shown and asked about first (Save, Don't Save, Cancel).
    pub fn close(&self, path: &str) {
        if self.tab(path).is_some_and(|t| t.diff.is_none() && t.dirty.get_untracked()) {
            self.active.set(Some(path.to_string()));
            self.closing.set(Some(path.to_string()));
        } else {
            self.close_now(path);
        }
    }

    /// A tab closed whatever is in it.
    pub fn close_now(&self, path: &str) {
        if self.closing.with_untracked(|c| c.as_deref() == Some(path)) {
            self.closing.set(None);
        }
        let tabs = self.tabs.get_untracked();
        let Some(at) = tabs.iter().position(|t| t.path == path) else { return };
        self.tabs.update(|t| {
            t.remove(at);
        });
        self.modified.update(|m| {
            m.remove(path);
        });
        self.forget_on_disk(path);
        if self.active.get_untracked().as_deref() == Some(path) {
            let rest = self.tabs.get_untracked();
            self.active.set(rest.get(at.min(rest.len().saturating_sub(1))).map(|t| t.path.clone()));
        }
    }

    pub fn save(&self, send: &Send) {
        if let Some(path) = self.active.get_untracked() {
            self.save_tab(&path, send);
        }
    }

    pub fn save_tab(&self, path: &str, send: &Send) {
        if let Some(tab) = self.tab(path)
            && tab.diff.is_none()
        {
            send(json!({"type": "save", "path": path, "text": tab.text()}));
        }
    }

    /// A file read again from disk: its text is taken when it comes, its tab left where it is.
    pub fn reload(&self, path: &str, send: &Send) {
        self.reloading.update(|r| {
            r.insert(path.to_string());
        });
        send(json!({"type": "open", "path": path}));
    }

    /// What is on disk taken in place of a tab's changes not saved.
    pub fn take_disk(&self, path: &str, send: &Send) {
        if let Some(tab) = self.tab(path) {
            tab.dirty.set(false);
            self.reload(path, send);
        }
    }

    /// A tab's own changes kept though the file changed on disk: no more said of it.
    pub fn forget_on_disk(&self, path: &str) {
        if self.on_disk.with_untracked(|d| d.contains_key(path)) {
            self.on_disk.update(|d| {
                d.remove(path);
            });
        }
    }

    /// A tab's text made what is on disk, by the part that differs only (what is before and after it stands, and is
    /// not coloured again); the caret stays on its line.
    pub(crate) fn replace(&self, tab: &Tab, text: &str) {
        let old = tab.text();
        let Some((from, to, with)) = changed(&old, text) else { return };
        let live = self.live.with_untracked(|l| l.as_ref().filter(|(p, _)| *p == tab.path).map(|(_, e)| e.clone()));
        floem::reactive::batch(|| {
            let caret = live.as_ref().map(|e| e.offset_to_line_col(e.cursor.with_untracked(|c| c.offset())));
            tab.doc.edit_single(Selection::region(from, to), with, EditType::Other);
            tab.dirty.set(false);
            if let (Some(e), Some((line, col))) = (live, caret) {
                let line = line.min(e.last_line());
                let at = e.offset_of_line_col(line, col.min(e.line_end_col(line, true)));
                e.cursor.update(|c| c.set_insert(Selection::caret(at)));
            }
        });
    }

    /// The look kept for this folder: where its views are, its parts' sizes.
    pub fn view_state(&self) -> Value {
        let editors: Vec<String> =
            self.tabs.with_untracked(|t| t.iter().filter(|t| t.diff.is_none()).map(|t| t.path.clone()).collect());
        let active = self.active.get_untracked().filter(|a| editors.contains(a));
        json!({
            "editors": editors,
            "active": active,
            "dock": self.dock.get_untracked().to_json(),
            "hidden": self.hidden.with_untracked(|h| { let mut h: Vec<u32> = h.iter().copied().collect(); h.sort(); h }),
            "sizes": self.sizes.with_untracked(|m| {
                m.iter().map(|(id, s)| (id.to_string(), json!(s.get_untracked()))).collect::<serde_json::Map<_, _>>()
            }),
        })
    }

    fn restore(&self, v: &Value, send: &Send) {
        if !v.is_object() {
            return;
        }
        // The files that were open, opened again in their order; the one shown last asked once more (the answers
        // come in the order asked, each shown as it comes), so it is the one shown.
        for p in v["editors"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            send(json!({"type": "open", "path": p}));
        }
        if let Some(a) = v["active"].as_str() {
            send(json!({"type": "open", "path": a}));
        }
        // A layout kept in the old form (three containers) is read as the default.
        let dock = Dock::from_json(&v["dock"]);
        let mut groups = Vec::new();
        dock.root.groups(&mut groups);
        self.hidden.set(
            v["hidden"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_u64().map(|x| x as u32))
                .filter(|g| groups.contains(g))
                .collect(),
        );
        self.dock.set(dock);
        self.open_floating();
        for (id, size) in v["sizes"].as_object().into_iter().flatten() {
            if let (Ok(id), Some(x)) = (id.parse::<u32>(), size.as_f64().filter(|x| x.is_finite())) {
                self.size(id).set(x.clamp(77.0, 4000.0));
            }
        }
    }

    /// A folder in the Explorer opened or closed; its entries asked for the first time it opens.
    pub fn toggle_folder(&self, path: &str, send: &Send) {
        let open = self.expanded.with_untracked(|e| e.contains(path));
        self.expanded.update(|e| {
            if open {
                e.remove(path);
            } else {
                e.insert(path.to_string());
            }
        });
        if !open && self.tree.with_untracked(|t| !t.contains_key(path)) {
            send(json!({"type": "ls", "path": path}));
        }
    }

    /// A file or a folder was moved: its open editors, the selection and the open folders follow it (and all that
    /// was in it, when it is a folder).
    pub fn moved(&self, from: &str, to: &str) {
        let now = |p: &str| moved_path(p, from, to);
        self.tabs.update(|tabs| {
            for t in tabs.iter_mut().filter(|t| t.diff.is_none()) {
                if let Some(n) = now(&t.path) {
                    t.path = n.clone();
                    t.file = n;
                }
            }
        });
        if let Some(n) = self.active.get_untracked().as_deref().and_then(now) {
            self.active.set(Some(n));
        }
        if let Some(n) = now(&self.selected.get_untracked()) {
            self.selected.set(n);
        }
        self.expanded.update(|e| *e = e.drain().map(|p| now(&p).unwrap_or(p)).collect());
        self.tree.update(|t| *t = t.drain().map(|(p, v)| (now(&p).unwrap_or(p), v)).collect());
    }

    /// A file or a folder was deleted: its editors close (and those of all that was in it).
    pub fn gone(&self, path: &str) {
        let inside = |p: &str| moved_path(p, path, "").is_some();
        let open: Vec<String> = self.tabs.with_untracked(|tabs| {
            tabs.iter().filter(|t| t.diff.is_none() && inside(&t.path)).map(|t| t.path.clone()).collect()
        });
        for p in open {
            self.close(&p);
        }
        if inside(&self.selected.get_untracked()) {
            self.selected.set(String::new());
        }
        self.expanded.update(|e| e.retain(|p| !inside(p)));
        self.tree.update(|t| t.retain(|p, _| !inside(p)));
    }

    /// One message from the session, as the page takes it: the window's parts are given it first (`ext::take`), then
    /// what is the whole window's is taken in here (the session's state, the theme).
    pub fn take(&self, m: &Value, send: &Send) {
        if crate::agent::stream::take(*self, m, send)
            || crate::save::take(*self, m, send)
            || crate::editor::take(*self, m, send)
            || crate::explorer::take(*self, m, send)
            || crate::search::take(*self, m, send)
            || crate::changes::take(*self, m, send)
            || crate::terminal::take(*self, m, send)
            || crate::palette::take(*self, m, send)
        {
            return;
        }
        let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
        match m["type"].as_str().unwrap_or("") {
            "state" => {
                self.model.set(s("model"));
                self.mode.set(s("mode"));
                self.thinking.set(s("thinking"));
                self.models.set(
                    m["models"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                );
                self.folder.set(s("folder"));
                self.branch.set(s("branch"));
                self.running.set(m["running"] == true);
                self.shell.set(s("shell"));
                self.choice.set(s("theme"));
                self.apply_theme();
                self.items.set(crate::agent::stream::thread_of(*self, m));
                if !self.loaded.get_untracked() {
                    self.restore(&m["view_state"], send);
                    self.loaded.set(true);
                }
                send(json!({"type": "themes"}));
                send(json!({"type": "ls", "path": ""}));
            }
            "status" => {
                self.model.set(s("model"));
                self.mode.set(s("mode"));
                if m["thinking"].is_string() {
                    self.thinking.set(s("thinking"));
                }
                self.running.set(m["running"] == true);
                if m["branch"].is_string() {
                    self.branch.set(s("branch"));
                }
            }
            "theme" => {
                self.choice.set(s("theme"));
                self.apply_theme();
            }
            "themes" => {
                let mut all = ca_themes::built_in();
                for t in m["items"].as_array().into_iter().flatten() {
                    if let Some(id) = t["id"].as_str() {
                        all.push(ca_themes::Theme { id: id.into(), json: t.clone() });
                    }
                }
                self.themes.set(all);
                self.apply_theme();
            }
            "behind" => send(json!({"type": "state"})),
            _ => {}
        }
    }
}

/// Where two texts differ: the part of the old one to replace (from, to) and what to put there; none when they are
/// the same.
pub fn changed<'a>(old: &str, new: &'a str) -> Option<(usize, usize, &'a str)> {
    if old == new {
        return None;
    }
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(start) {
        start -= 1;
    }
    let most = a.len().min(b.len()) - start;
    let mut end = a.iter().rev().zip(b.iter().rev()).take(most).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(old.len() - end) {
        end -= 1;
    }
    Some((start, old.len() - end, &new[start..new.len() - end]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::View;

    fn asked() -> (Send, Rc<std::cell::RefCell<Vec<Value>>>) {
        let asked = Rc::new(std::cell::RefCell::new(Vec::new()));
        let to = asked.clone();
        (Rc::new(move |v: Value| to.borrow_mut().push(v)), asked)
    }

    #[test]
    fn two_texts_differ_by_their_changed_part_only() {
        assert_eq!(changed("abc", "abc"), None);
        assert_eq!(changed("one\ntwo\nthree\n", "one\n2\nthree\n"), Some((4, 7, "2")));
        assert_eq!(changed("aaa", "aa"), Some((2, 3, "")));
        assert_eq!(changed("aa", "aaa"), Some((2, 2, "a")));
        assert_eq!(changed("", "x"), Some((0, 0, "x")));
        // Never cut inside a character.
        assert_eq!(changed("é", "è"), Some((0, 2, "è")));
    }

    #[test]
    fn a_file_changed_on_disk_is_read_again_or_said_to_have_changed() {
        let ui = Ui::new();
        let (send, asked) = asked();
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "one\ntwo\n", "modified": 5}), &send);
        ui.take(&json!({"type": "file", "path": "b.rs", "text": "b\n", "modified": 5}), &send);
        assert_eq!(ui.active.get_untracked().as_deref(), Some("b.rs"));
        // The same stamp: nothing to do.
        ui.take(&json!({"type": "stats", "items": [{"path": "a.rs", "modified": 5}]}), &send);
        assert!(asked.borrow().is_empty());
        // No changes of its own: read again, and its text taken without its tab being shown for it.
        ui.take(&json!({"type": "stats", "items": [{"path": "a.rs", "modified": 9}]}), &send);
        assert_eq!(*asked.borrow(), [json!({"type": "open", "path": "a.rs"})]);
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "one\n2\n", "modified": 9}), &send);
        let a = ui.tab("a.rs").expect("open");
        assert_eq!(a.text(), "one\n2\n");
        assert!(!a.dirty.get_untracked());
        assert_eq!(ui.active.get_untracked().as_deref(), Some("b.rs"));
        // With changes of its own: said, its text left alone, until the person takes the disk's.
        a.doc.edit_single(Selection::caret(0), "mine ", EditType::InsertChars);
        assert!(a.dirty.get_untracked());
        ui.take(&json!({"type": "stats", "items": [{"path": "a.rs", "modified": 12}]}), &send);
        assert_eq!(ui.on_disk.get_untracked().get("a.rs"), Some(&OnDisk::Changed));
        assert_eq!(asked.borrow().len(), 1);
        ui.take_disk("a.rs", &send);
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "theirs\n", "modified": 12}), &send);
        assert_eq!(a.text(), "theirs\n");
        assert!(ui.on_disk.get_untracked().is_empty());
        // Gone from the disk: said too.
        ui.take(&json!({"type": "stats", "items": [{"path": "b.rs", "gone": true}]}), &send);
        assert_eq!(ui.on_disk.get_untracked().get("b.rs"), Some(&OnDisk::Gone));
        // One's own save is not a change: its stamp is the one known.
        ui.take(&json!({"type": "saved", "path": "b.rs", "modified": 20}), &send);
        assert!(ui.on_disk.get_untracked().is_empty());
        ui.take(&json!({"type": "stats", "items": [{"path": "b.rs", "modified": 20}]}), &send);
        assert_eq!(asked.borrow().len(), 2);
        // Read again and not readable: said, and the file opens as any other the next time it is asked for.
        ui.take(&json!({"type": "stats", "items": [{"path": "b.rs", "modified": 30}]}), &send);
        ui.take(&json!({"type": "file", "path": "b.rs", "error": "not text"}), &send);
        assert!(ui.reloading.get_untracked().is_empty());
        ui.close("b.rs");
        ui.take(&json!({"type": "file", "path": "b.rs", "text": "b again\n", "modified": 30}), &send);
        assert_eq!(ui.active.get_untracked().as_deref(), Some("b.rs"));
    }

    #[test]
    fn a_tab_with_changes_not_saved_is_asked_about_before_it_closes() {
        let ui = Ui::new();
        let (send, asked) = asked();
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "one\n", "modified": 5}), &send);
        ui.take(&json!({"type": "file", "path": "b.rs", "text": "b\n", "modified": 5}), &send);
        ui.close("b.rs");
        assert!(ui.tab("b.rs").is_none(), "nothing to lose: closed");
        let a = ui.tab("a.rs").expect("open");
        a.doc.edit_single(Selection::caret(0), "mine ", EditType::InsertChars);
        ui.close("a.rs");
        assert_eq!(ui.closing.get_untracked().as_deref(), Some("a.rs"));
        assert!(ui.tab("a.rs").is_some());
        // Save: closed once the session says it is saved.
        ui.close_saved.set(ui.closing.get_untracked());
        ui.closing.set(None);
        ui.save_tab("a.rs", &send);
        assert_eq!(asked.borrow().last(), Some(&json!({"type": "save", "path": "a.rs", "text": "mine one\n"})));
        ui.take(&json!({"type": "saved", "path": "a.rs", "modified": 6}), &send);
        assert!(ui.tab("a.rs").is_none());
        assert_eq!(ui.active.get_untracked(), None);
    }

    #[test]
    fn the_look_kept_comes_back_as_it_was() {
        let ui = Ui::new();
        ui.dock_view(View::Search, crate::dock::EDITOR, crate::dock::Side::Right);
        ui.size(2).set(333.0);
        ui.toggle_group_of(View::Terminal);
        let kept = ui.view_state();
        let again = Ui::new();
        let (send, asked) = asked();
        again.restore(&kept, &send);
        assert_eq!(again.dock.get_untracked(), ui.dock.get_untracked());
        assert_eq!(again.size(2).get_untracked(), 333.0);
        assert!(!again.group_shown(View::Terminal), "hidden, as it was");
        // An old layout (three containers): the default dock; the files open again in their order, the one shown
        // last asked once more so it is the one shown.
        let old = json!({"editors": ["a.rs", "b.rs"], "active": "a.rs", "layout": {"primary": ["files"]}});
        let fresh = Ui::new();
        fresh.restore(&old, &send);
        assert_eq!(fresh.dock.get_untracked(), crate::dock::Dock::default());
        let opened: Vec<_> = asked.borrow().iter().filter_map(|v| v["path"].as_str().map(str::to_string)).collect();
        assert_eq!(opened, ["a.rs", "b.rs", "a.rs"]);
    }

    #[test]
    fn the_session_says_its_models_and_how_much_it_thinks() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        ui.take(
            &json!({"type": "state", "model": "m-large", "mode": "ask", "thinking": "default",
                    "models": ["m-small", "m-large"]}),
            &send,
        );
        assert_eq!(ui.models.get_untracked(), ["m-small", "m-large"]);
        assert_eq!(ui.thinking.get_untracked(), "default");
        // A status names the three in force, not the models there are.
        ui.take(&json!({"type": "status", "model": "m-small", "mode": "plan", "thinking": "high"}), &send);
        assert_eq!(ui.thinking.get_untracked(), "high");
        assert_eq!(ui.mode.get_untracked(), "plan");
        assert_eq!(ui.models.get_untracked().len(), 2);
    }

    fn sender() -> (Send, Rc<std::cell::RefCell<Vec<Value>>>) {
        let asked = Rc::new(std::cell::RefCell::new(Vec::new()));
        let to = asked.clone();
        (Rc::new(move |v: Value| to.borrow_mut().push(v)), asked)
    }

    #[test]
    fn open_editors_follow_a_file_that_is_moved_and_close_with_one_that_is_deleted() {
        let ui = Ui::new();
        let (send, _asked) = sender();
        for p in ["src/a.rs", "src/in/b.rs", "srcs.rs"] {
            ui.take(&json!({"type": "file", "path": p, "text": "x"}), &send);
        }
        ui.expanded.set(["src".to_string(), "src/in".to_string()].into());
        ui.take(&json!({"type": "ls", "path": "src/in", "entries": [{"name": "b.rs", "dir": false}]}), &send);
        ui.selected.set("src/in/b.rs".into());
        // A folder renamed: all that is in it follows; a name that only begins as it does stays.
        ui.take(&json!({"type": "renamed", "path": "src", "to": "lib"}), &send);
        let paths = || ui.tabs.with_untracked(|t| t.iter().map(|t| t.path.clone()).collect::<Vec<_>>());
        assert_eq!(paths(), ["lib/a.rs", "lib/in/b.rs", "srcs.rs"]);
        assert_eq!(ui.tab("lib/a.rs").map(|t| t.file), Some("lib/a.rs".to_string()));
        assert_eq!(ui.active.get_untracked().as_deref(), Some("srcs.rs"));
        assert_eq!(ui.selected.get_untracked(), "lib/in/b.rs");
        assert!(ui.expanded.with_untracked(|e| e.contains("lib/in") && e.contains("lib") && e.len() == 2));
        assert!(ui.tree.with_untracked(|t| t.contains_key("lib/in") && !t.contains_key("src/in")));
        ui.active.set(Some("lib/a.rs".into()));
        ui.take(&json!({"type": "renamed", "path": "lib/a.rs", "to": "lib/c.rs"}), &send);
        assert_eq!(ui.active.get_untracked().as_deref(), Some("lib/c.rs"));
        // One that could not be moved: said, and nothing follows.
        ui.take(&json!({"type": "renamed", "path": "srcs.rs", "to": "x.rs", "error": "already there"}), &send);
        assert!(ui.message.get_untracked().contains("already there") && ui.tab("srcs.rs").is_some());
        // A folder deleted: its editors close, and the one shown is another.
        ui.take(&json!({"type": "deleted", "path": "lib", "kept": "/data/deleted/1/lib"}), &send);
        assert_eq!(paths(), ["srcs.rs"]);
        assert_eq!(ui.active.get_untracked().as_deref(), Some("srcs.rs"));
        assert!(ui.selected.get_untracked().is_empty() && ui.expanded.with_untracked(|e| e.is_empty()));
        assert!(ui.message.get_untracked().contains("/data/deleted/1/lib"));
    }

    #[test]
    fn the_file_this_window_asked_for_is_opened_once_made_and_another_faces_is_not() {
        let ui = Ui::new();
        let (send, asked) = sender();
        ui.take(&json!({"type": "created", "path": "theirs.rs", "dir": false}), &send);
        assert!(asked.borrow().is_empty(), "made by another face: not opened here");
        ui.explorer_asked.set(Some("src/new.rs".into()));
        ui.take(&json!({"type": "created", "path": "src/new.rs", "dir": false}), &send);
        assert_eq!(*asked.borrow(), [json!({"type": "open", "path": "src/new.rs"})]);
        assert_eq!(ui.selected.get_untracked(), "src/new.rs");
        assert!(ui.explorer_asked.get_untracked().is_none());
        ui.explorer_asked.set(Some("src/new.rs".into()));
        ui.take(&json!({"type": "created", "path": "src/new.rs", "dir": false, "error": "already there"}), &send);
        assert!(ui.message.get_untracked().contains("already there"));
        assert_eq!(asked.borrow().len(), 1);
    }

    #[test]
    fn a_view_shown_hidden_and_its_group_toggled() {
        let ui = Ui::new();
        assert!(ui.view_shown(View::Files));
        ui.toggle_view(View::Search);
        assert!(ui.view_shown(View::Search) && !ui.view_shown(View::Files));
        ui.toggle_view(View::Search);
        assert!(!ui.group_shown(View::Search), "clicked again: its group hidden");
        ui.reveal(View::Changes);
        assert!(ui.view_shown(View::Changes));
    }
}
