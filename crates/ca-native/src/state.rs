//! Everything the window shows, as signals, and the session's messages taken into them as the page takes them.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith, create_rw_signal};
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use serde_json::{Value, json};

use crate::colors::Colors;
use crate::layout::Layout;
use crate::model::{Item, Kind, Tool};

pub type Send = Rc<dyn Fn(Value)>;

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

/// An open editor: its file, its text (kept while another tab is shown), whether it has changes not saved.
#[derive(Clone)]
pub struct Tab {
    pub path: String,
    pub doc: Rc<TextDocument>,
    pub dirty: RwSignal<bool>,
}

impl Tab {
    pub fn name(&self) -> String {
        self.path.rsplit('/').next().unwrap_or(&self.path).to_string()
    }

    pub fn text(&self) -> String {
        self.doc.text().to_string()
    }
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
    pub folder: RwSignal<String>,
    pub branch: RwSignal<String>,
    pub plan: RwSignal<Option<(String, usize, usize)>>,
    pub goal: RwSignal<Option<String>>,
    pub jobs: RwSignal<Vec<(u64, String)>>,
    pub draft: RwSignal<String>,

    pub layout: RwSignal<Layout>,
    pub primary_width: RwSignal<f64>,
    pub secondary_width: RwSignal<f64>,
    pub panel_height: RwSignal<f64>,
    /// The title bar's menu open, by its place.
    pub menu: RwSignal<Option<usize>>,
    /// A view's own menu (Move to…), open where it was right-clicked (in the window).
    pub context: RwSignal<Option<(crate::layout::View, f64, f64)>>,
    pub search_query: RwSignal<String>,
    pub search_case: RwSignal<bool>,
    pub search_regex: RwSignal<bool>,
    pub search: RwSignal<Option<Found>>,
    /// A line to show in a file once its editor is drawn: the file, the line (from 1), and a number of its own so
    /// the same line asked again is gone to again.
    pub goto: RwSignal<Option<(String, usize, u64)>>,
    /// Where the pointer is in the window (for a menu opened by a right-click).
    pub pointer: RwSignal<(f64, f64)>,
    /// The layout kept for the folder has been read: from then on a change is kept.
    pub loaded: RwSignal<bool>,

    pub tree: RwSignal<HashMap<String, Vec<Entry>>>,
    pub expanded: RwSignal<HashSet<String>>,
    pub selected: RwSignal<String>,
    pub tabs: RwSignal<Vec<Tab>>,
    pub active: RwSignal<Option<String>>,
    pub caret: RwSignal<(usize, usize)>,
    /// A line for the status bar: what was just done, or went wrong.
    pub message: RwSignal<String>,
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
            folder: create_rw_signal(String::new()),
            branch: create_rw_signal(String::new()),
            plan: create_rw_signal(None),
            goal: create_rw_signal(None),
            jobs: create_rw_signal(Vec::new()),
            draft: create_rw_signal(String::new()),
            layout: create_rw_signal(Layout::default()),
            primary_width: create_rw_signal(260.0),
            secondary_width: create_rw_signal(380.0),
            panel_height: create_rw_signal(230.0),
            menu: create_rw_signal(None),
            context: create_rw_signal(None),
            loaded: create_rw_signal(false),
            pointer: create_rw_signal((0.0, 0.0)),
            search_query: create_rw_signal(String::new()),
            search_case: create_rw_signal(false),
            search_regex: create_rw_signal(false),
            search: create_rw_signal(None),
            goto: create_rw_signal(None),
            tree: create_rw_signal(HashMap::new()),
            expanded: create_rw_signal(HashSet::new()),
            selected: create_rw_signal(String::new()),
            tabs: create_rw_signal(Vec::new()),
            active: create_rw_signal(None),
            caret: create_rw_signal((1, 1)),
            message: create_rw_signal(String::new()),
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

    fn apply_theme(&self) {
        let t = ca_themes::chosen(&self.themes.get_untracked(), &self.choice.get_untracked(), true);
        self.colors.set(Colors::of(&t));
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
        {
            send(json!({"type": "save", "path": path, "text": tab.text()}));
        }
    }

    /// The look kept for this folder: where its views are, its parts' sizes.
    pub fn view_state(&self) -> Value {
        json!({
            "layout": self.layout.get_untracked().to_json(),
            "sizes": {
                "primary": self.primary_width.get_untracked(),
                "secondary": self.secondary_width.get_untracked(),
                "panel": self.panel_height.get_untracked(),
            },
        })
    }

    fn restore(&self, v: &Value) {
        if !v.is_object() {
            return;
        }
        self.layout.set(Layout::normalize(&v["layout"]));
        let size = |k: &str, lo: f64, hi: f64| v["sizes"][k].as_f64().map(|x| x.clamp(lo, hi));
        if let Some(w) = size("primary", 170.0, 800.0) {
            self.primary_width.set(w);
        }
        if let Some(w) = size("secondary", 220.0, 900.0) {
            self.secondary_width.set(w);
        }
        if let Some(h) = size("panel", 100.0, 900.0) {
            self.panel_height.set(h);
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

    /// One message from the session, as the page takes it.
    pub fn take(&self, m: &Value, send: &Send) {
        let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
        match m["type"].as_str().unwrap_or("") {
            "state" => {
                self.model.set(s("model"));
                self.mode.set(s("mode"));
                self.folder.set(s("folder"));
                self.branch.set(s("branch"));
                self.running.set(m["running"] == true);
                self.choice.set(s("theme"));
                self.apply_theme();
                self.items.set(self.thread_of(m));
                if !self.loaded.get_untracked() {
                    self.restore(&m["view_state"]);
                    self.loaded.set(true);
                }
                send(json!({"type": "themes"}));
                send(json!({"type": "ls", "path": ""}));
            }
            "status" => {
                self.model.set(s("model"));
                self.mode.set(s("mode"));
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
                    doc.add_on_update(move |_| dirty.set(true));
                    self.tabs.update(|t| t.push(Tab { path: path.clone(), doc, dirty }));
                }
                self.active.set(Some(path));
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
    use crate::layout::{View, Where};

    #[test]
    fn the_look_kept_comes_back_as_it_was_and_sizes_within_bounds() {
        let ui = Ui::new();
        ui.layout.update(|l| *l = l.moved(View::Search, Where::Panel, None));
        ui.primary_width.set(333.0);
        let kept = ui.view_state();
        let again = Ui::new();
        again.restore(&kept);
        assert_eq!(again.layout.get_untracked(), ui.layout.get_untracked());
        assert_eq!(again.primary_width.get_untracked(), 333.0);
        again.restore(&json!({"layout": {}, "sizes": {"panel": 5.0}}));
        assert_eq!(again.panel_height.get_untracked(), 100.0);
        assert_eq!(again.layout.get_untracked(), Layout::default());
    }
}
