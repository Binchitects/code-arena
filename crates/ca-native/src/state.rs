//! Everything the window shows, as signals, and the session's messages taken into them as the page takes them.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith, create_rw_signal};
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use serde_json::{Value, json};

use crate::colors::Colors;
use crate::dock::Dock;
use crate::model::{Item, Kind, Tool};

pub type Send = Rc<dyn Fn(Value)>;

fn s_of(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

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
    pub term: RwSignal<crate::terminal::State>,
    pub term_screen: RwSignal<crate::terminal::Screen>,
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
            term: create_rw_signal(crate::terminal::State::None),
            term_screen: create_rw_signal(crate::terminal::Screen::default()),
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
        }
    }

    fn id(&self) -> u64 {
        let n = self.next.get_untracked();
        self.next.set(n + 1);
        n
    }

    fn push(&self, kind: Kind) {
        let id = self.id();
        self.items.update(|l| l.push(Item { id, kind }));
    }

    /// The answer being written: the last item when it is one, else a new one.
    fn current(&self) -> Item {
        if let Some(last) = self.items.with_untracked(|l| l.last().cloned())
            && matches!(last.kind, Kind::Assistant { .. })
        {
            return last;
        }
        let item = Item { id: self.id(), kind: Kind::assistant() };
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

    pub fn close(&self, path: &str) {
        let tabs = self.tabs.get_untracked();
        let Some(at) = tabs.iter().position(|t| t.path == path) else { return };
        self.tabs.update(|t| {
            t.remove(at);
        });
        if self.active.get_untracked().as_deref() == Some(path) {
            let rest = self.tabs.get_untracked();
            self.active.set(rest.get(at.min(rest.len().saturating_sub(1))).map(|t| t.path.clone()));
        }
    }

    pub fn save(&self, send: &Send) {
        if let Some(path) = self.active.get_untracked()
            && let Some(tab) = self.tab(&path)
            && tab.diff.is_none()
        {
            send(json!({"type": "save", "path": path, "text": tab.text()}));
        }
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

    /// One message from the session, as the page takes it.
    pub fn take(&self, m: &Value, send: &Send) {
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
                self.choice.set(s("theme"));
                self.apply_theme();
                self.items.set(self.thread_of(m));
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
            "ls" => {
                if m["error"].is_string() {
                    return;
                }
                let entries = m["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|e| Entry { name: e["name"].as_str().unwrap_or("").into(), dir: e["dir"] == true })
                    .collect();
                self.tree.update(|t| {
                    t.insert(s("path"), entries);
                });
            }
            // The Explorer's own. Every face is told, so what another face moved or deleted is followed here too;
            // but a file is opened only in the window that asked for it.
            "created" => {
                let path = s("path");
                if !self.explorer_asked.with_untracked(|a| a.as_deref() == Some(path.as_str())) {
                    return;
                }
                self.explorer_asked.set(None);
                match m["error"].as_str() {
                    Some(e) => self.message.set(format!("Could not create {path}: {e}")),
                    None => {
                        self.message.set(String::new());
                        if m["dir"] == true {
                            self.selected.set(path);
                        } else {
                            self.open(&path, send);
                        }
                    }
                }
            }
            "renamed" => {
                let (path, to) = (s("path"), s("to"));
                match m["error"].as_str() {
                    Some(e) => self.message.set(format!("Could not rename {path}: {e}")),
                    None => {
                        self.message.set(String::new());
                        self.moved(&path, &to);
                    }
                }
            }
            "deleted" => {
                let path = s("path");
                match m["error"].as_str() {
                    Some(e) => self.message.set(format!("Could not delete {path}: {e}")),
                    None => {
                        self.gone(&path);
                        self.message.set(match m["kept"].as_str() {
                            Some(kept) => format!("Deleted {path}: it is kept in {kept}"),
                            None => String::new(),
                        });
                    }
                }
            }
            "file" => {
                let path = s("path");
                if let Some(e) = m["error"].as_str() {
                    self.message.set(format!("Could not open {path}: {e}"));
                    return;
                }
                if self.tab(&path).is_none() {
                    let text = s("text");
                    let doc = Rc::new(TextDocument::new(self.scope, text));
                    let dirty = self.scope.create_rw_signal(false);
                    let colours = crate::highlight::Colours::new(doc.clone(), &path);
                    {
                        let (colours, d) = (colours.clone(), doc.clone());
                        doc.add_on_update(move |u| {
                            dirty.set(true);
                            // Parsed again from the first line an edit touched.
                            let first = u.deltas().map(|x| x.summary().0.start()).min().unwrap_or(0);
                            colours.changed_from(d.rope_text().line_of_offset(first));
                        });
                    }
                    self.tabs.update(|t| {
                        t.push(Tab { path: path.clone(), file: path.clone(), doc, dirty, diff: None, colours })
                    });
                }
                self.active.set(Some(path));
            }
            "files" => {
                let paths = || m["paths"].as_array().into_iter().flatten().filter_map(Value::as_str);
                let more = m["more"] == true;
                // Listed again as they were: what is shown stays (the row chosen with it).
                if !self.quick_files.with_untracked(|f| f.holds(paths(), more)) {
                    self.quick_files.set(Rc::new(crate::palette::Files::of(paths(), more)));
                }
            }
            "search" => {
                if m["query"].as_str() != Some(self.search_query.get_untracked().as_str()) {
                    return; // an answer to what was typed before
                }
                if self.search_query.with_untracked(|q| q.is_empty()) {
                    self.search.set(None);
                    return;
                }
                let hits = m["hits"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|h| Hit {
                        path: h["path"].as_str().unwrap_or("").into(),
                        line: h["line"].as_u64().unwrap_or(1) as usize,
                        text: h["text"].as_str().unwrap_or("").into(),
                    })
                    .collect();
                self.search.set(Some(Found { hits, more: m["more"] == true, error: s("error") }));
            }
            "changes" => {
                let text = |v: &Value| v.as_str().map(str::to_string);
                self.changes.set(
                    m["files"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|f| Change { path: s_of(f, "path"), before: text(&f["before"]), after: text(&f["after"]) })
                        .collect(),
                );
            }
            "saved" => {
                let path = s("path");
                match m["error"].as_str() {
                    Some(e) => self.message.set(format!("Could not save {path}: {e}")),
                    None => {
                        if let Some(t) = self.tab(&path) {
                            t.dirty.set(false);
                        }
                        self.message.set(String::new());
                    }
                }
            }
            "term_out" => {
                crate::terminal::output(*self, m["id"].as_u64().unwrap_or(0), m["data"].as_str().unwrap_or(""), send)
            }
            "term_exit" => crate::terminal::ended(*self, m["id"].as_u64().unwrap_or(0), m["code"].as_i64()),
            "user" => self.push(Kind::User(s("text"))),
            "turn_start" => self.running.set(true),
            "text" => {
                if let Kind::Assistant { text, .. } = self.current().kind {
                    text.update(|t| t.push_str(m["delta"].as_str().unwrap_or("")));
                }
            }
            "tool_start" => {
                if let Kind::Assistant { tools, .. } = self.current().kind {
                    tools.update(|t| {
                        t.push(Tool {
                            name: s("name"),
                            subject: s("subject"),
                            done: false,
                            ok: true,
                            text: String::new(),
                        })
                    });
                }
            }
            "tool_end" => {
                if let Kind::Assistant { tools, .. } = self.current().kind {
                    tools.update(|ts| {
                        let (name, subject) = (s("name"), s("subject"));
                        match ts.iter_mut().rev().find(|t| !t.done && t.name == name && t.subject == subject) {
                            Some(t) => {
                                t.done = true;
                                t.ok = m["ok"] == true;
                                t.text = s("text");
                            }
                            None => ts.push(Tool { name, subject, done: true, ok: m["ok"] == true, text: s("text") }),
                        }
                    });
                }
            }
            "notice" => {
                let t = s("text");
                if !(t.starts_with("› ") && t.ends_with("(sent while it worked)")) {
                    self.push(Kind::Notice(t));
                }
            }
            "ask" => self.push(Kind::Ask {
                id: m["id"].as_u64().unwrap_or(0),
                tool: s("tool"),
                subject: s("subject"),
                answer: create_rw_signal(None),
            }),
            "answered" => {
                let id = m["id"].as_u64().unwrap_or(0);
                self.items.with_untracked(|l| {
                    for i in l {
                        if let Kind::Ask { id: a, answer, .. } = &i.kind
                            && *a == id
                        {
                            answer.set(Some(s("answer")));
                        }
                    }
                });
            }
            "queued" => self.push(Kind::Queued(s("text"))),
            "turn_end" => {
                self.running.set(false);
                send(json!({"type": "changes"}));
                self.items.with_untracked(|l| {
                    for i in l {
                        if let Kind::Ask { answer, .. } = &i.kind
                            && answer.get_untracked().is_none()
                        {
                            answer.set(Some("gone".into()));
                        }
                    }
                });
                if m["ok"] != true {
                    if m["stopped"] == true {
                        self.push(Kind::Notice("Stopped.".into()));
                    } else {
                        self.push(Kind::Error(m["error"].as_str().unwrap_or("The turn failed.").to_string()));
                    }
                }
            }
            "tasks" => {
                self.plan.set(m["plan"]["steps"].as_array().map(|steps| {
                    let done = steps.iter().filter(|st| st["status"] == "done").count();
                    (m["plan"]["title"].as_str().unwrap_or("").to_string(), done, steps.len())
                }));
                self.goal.set(
                    m["goal"]["text"].as_str().map(|g| format!("{g} · {}", m["goal"]["status"].as_str().unwrap_or(""))),
                );
                self.jobs.set(
                    m["jobs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|j| j["running"] == true)
                        .map(|j| (j["id"].as_u64().unwrap_or(0), j["command"].as_str().unwrap_or("").to_string()))
                        .collect(),
                );
            }
            "behind" => send(json!({"type": "state"})),
            _ => {}
        }
    }

    /// The thread the session kept, as items.
    fn thread_of(&self, m: &Value) -> Vec<Item> {
        let mut items = Vec::new();
        for msg in m["messages"].as_array().into_iter().flatten() {
            let text = msg["text"].as_str().unwrap_or("").to_string();
            match msg["role"].as_str() {
                Some("user") if !text.starts_with("(Code Arena") => {
                    items.push(Item { id: self.id(), kind: Kind::User(text) })
                }
                Some("assistant") => {
                    let tools: Vec<Tool> = msg["tools"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|t| t.as_str())
                        .map(|n| Tool {
                            name: n.into(),
                            subject: String::new(),
                            done: true,
                            ok: true,
                            text: String::new(),
                        })
                        .collect();
                    if !matches!(items.last(), Some(Item { kind: Kind::Assistant { .. }, .. })) {
                        items.push(Item { id: self.id(), kind: Kind::assistant() });
                    }
                    if let Some(Item { kind: Kind::Assistant { text: t, tools: ts, .. }, .. }) = items.last() {
                        if !text.is_empty() {
                            t.update(|x| {
                                if !x.is_empty() {
                                    x.push_str("\n\n");
                                }
                                x.push_str(&text)
                            });
                        }
                        ts.update(|x| x.extend(tools));
                    }
                }
                _ => {}
            }
        }
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::View;

    #[test]
    fn the_look_kept_comes_back_as_it_was() {
        let ui = Ui::new();
        ui.dock_view(View::Search, crate::dock::EDITOR, crate::dock::Side::Right);
        ui.size(2).set(333.0);
        ui.toggle_group_of(View::Terminal);
        let kept = ui.view_state();
        let again = Ui::new();
        let asked = Rc::new(std::cell::RefCell::new(Vec::new()));
        let send: Send = {
            let asked = asked.clone();
            Rc::new(move |v: Value| asked.borrow_mut().push(v))
        };
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
