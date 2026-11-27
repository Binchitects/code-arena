//! The kinds of editor tab that are not a file's text: a picture, a preview, the settings, a compare, a notebook.
//! A part adds its kind from its `register` (see `ext.rs`), and opens a tab of it by its path, which begins with
//! the kind's name and a colon:
//!
//! ```ignore
//! kinds::add(
//!     kinds::kind("preview", |_ui, rest| format!("Preview {rest}"), |ui, send, tab| preview(ui, send, tab).into_any())
//!         .icon("open-preview")
//!         .kept(|ui, _send, path| kinds::open(ui, path)),
//! );
//! kinds::open(ui, "preview:README.md");
//! ```
//!
//! What a kind says of its tabs:
//!
//! - `title`: the tab's name, from the rest of its path (what is after the colon). It is read where it is drawn,
//!   so a signal it reads names the tab again when it changes.
//! - `view`: what is drawn in the editor's place while the tab is the one shown. It is built again each time the
//!   tab is shown: what must outlive that (a scroll's place, a text being typed) is kept in the part's own state
//!   (`ui.state::<T>()`), by the tab's path.
//! - `icon`: a codicon before the name (none: no icon, as a file's tab).
//! - `save`: what Save (Ctrl+S, and the question asked when the tab closes) does. The tab's `dirty` is the part's to
//!   set; once saved it says so with `kinds::saved(ui, path)`, which also closes a tab that waited for it.
//! - `kept`: its tabs are kept with the folder's layout and made again at the next start, each by this (which opens
//!   it, at once or when what it shows has come), among the files' tabs where they were. Without it a tab of the
//!   kind is gone at the next start.
//!
//! A tab of a kind is in `ui.tabs` with the files' tabs (its `kind` is the kind's name, its `file` the rest of its
//! path), so the tabs, Close and the close question are the same for all. It is not a file to the rest of the
//! window: it is not watched on disk, not in Quick Open, not renamed with a folder, and `Tab::is_file` is false.
//! The status bar's items of a file (the line, the language) are not shown for it.
//!
//! A kind added again under the same name takes the place of the first.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text_document::TextDocument;
use floem::views::{Decorators, empty};
use floem::{AnyView, IntoView};
use serde_json::Value;

use crate::state::{Send, Tab, Ui};

/// A tab's name, from the rest of its path.
pub type Title = Rc<dyn Fn(Ui, &str) -> String>;
/// What is drawn for a tab.
pub type Draw = Rc<dyn Fn(Ui, Send, &Tab) -> AnyView>;
/// What saves a tab.
pub type Save = Rc<dyn Fn(Ui, &Send, &Tab)>;
/// What makes a kept tab again, by its path.
pub type Restore = Rc<dyn Fn(Ui, &Send, &str)>;

#[derive(Clone)]
pub struct Kind {
    /// Its name: what its tabs' paths begin with, before the colon.
    pub name: &'static str,
    pub icon: &'static str,
    pub title: Title,
    pub view: Draw,
    pub save: Option<Save>,
    pub restore: Option<Restore>,
}

/// A kind of tab, to be given its icon and what saves and keeps it, and then added.
pub fn kind(
    name: &'static str,
    title: impl Fn(Ui, &str) -> String + 'static,
    view: impl Fn(Ui, Send, &Tab) -> AnyView + 'static,
) -> Kind {
    Kind { name, icon: "", title: Rc::new(title), view: Rc::new(view), save: None, restore: None }
}

impl Kind {
    pub fn icon(mut self, icon: &'static str) -> Kind {
        self.icon = icon;
        self
    }

    pub fn save(mut self, save: impl Fn(Ui, &Send, &Tab) + 'static) -> Kind {
        self.save = Some(Rc::new(save));
        self
    }

    /// Its tabs are kept with the layout, and each made again by this at the next start.
    pub fn kept(mut self, restore: impl Fn(Ui, &Send, &str) + 'static) -> Kind {
        self.restore = Some(Rc::new(restore));
        self
    }
}

thread_local! {
    static KINDS: RefCell<Vec<Kind>> = const { RefCell::new(Vec::new()) };
}

/// Adds a kind of tab; one of the same name takes the first's place.
pub fn add(kind: Kind) {
    KINDS.with(|k| {
        let mut kinds = k.borrow_mut();
        match kinds.iter().position(|old| old.name == kind.name) {
            Some(at) => kinds[at] = kind,
            None => kinds.push(kind),
        }
    });
}

pub fn find(name: &str) -> Option<Kind> {
    KINDS.with(|k| k.borrow().iter().find(|kind| kind.name == name).cloned())
}

/// A tab's kind (none: a file's tab, or a kind that was not added).
pub fn of(tab: &Tab) -> Option<Kind> {
    tab.kind.and_then(find)
}

/// A path's two parts: the kind's name and the rest ("preview:docs/a.md": "preview" and "docs/a.md").
pub fn split(path: &str) -> Option<(&str, &str)> {
    path.split_once(':')
}

/// The tab of this path shown: made when it is not open yet. Nothing when its kind was not added.
pub fn open(ui: Ui, path: &str) {
    let Some((name, rest)) = split(path) else { return };
    let Some(kind) = find(name) else { return };
    if ui.tab(path).is_none() {
        let doc = Rc::new(TextDocument::new(ui.scope, ""));
        let tab = Tab {
            path: path.to_string(),
            file: rest.to_string(),
            colours: crate::highlight::Colours::new(doc.clone(), ""),
            doc,
            dirty: ui.scope.create_rw_signal(false),
            diff: None,
            kind: Some(kind.name),
        };
        ui.tabs.update(|t| t.push(tab));
    }
    if ui.active.with_untracked(|a| a.as_deref() != Some(path)) {
        ui.active.set(Some(path.to_string()));
    }
}

/// A tab's name as its tab shows it: its kind's, or the file's.
pub fn title(ui: Ui, tab: &Tab) -> String {
    match of(tab) {
        Some(kind) => (kind.title)(ui, &tab.file),
        None => tab.name(),
    }
}

/// A tab's icon (none: an empty name).
pub fn icon(tab: &Tab) -> &'static str {
    of(tab).map_or("", |k| k.icon)
}

/// What is drawn in the editor's place for a tab of a kind (nothing, for a kind that was not added).
pub fn draw(ui: Ui, tab: &Tab) -> AnyView {
    let send: Send = ui.out.get_untracked().unwrap_or_else(|| Rc::new(|_| {}));
    match of(tab) {
        Some(kind) => (kind.view)(ui, send, tab),
        None => empty().style(|s| s.size_full()).into_any(),
    }
}

/// Save, for a tab of a kind: its kind's way (nothing, when it has none).
pub fn save(ui: Ui, send: &Send, tab: &Tab) {
    if let Some(save) = of(tab).and_then(|k| k.save) {
        save(ui, send, tab);
    }
}

/// A tab of a kind was saved: it has no changes any more, and closes if it waited for that.
pub fn saved(ui: Ui, path: &str) {
    if let Some(tab) = ui.tab(path) {
        tab.dirty.set(false);
    }
    if ui.close_saved.with_untracked(|c| c.as_deref() == Some(path)) {
        ui.close_saved.set(None);
        ui.close_now(path);
    }
}

/// The open tabs that are kept with the layout, by their paths, in their order.
pub fn kept(ui: Ui) -> Vec<String> {
    ui.tabs.with_untracked(|tabs| {
        tabs.iter().filter(|t| of(t).is_some_and(|k| k.restore.is_some())).map(|t| t.path.clone()).collect()
    })
}

/// A kept tab made again, by its kind. False when its kind is not there, or keeps no tab.
pub fn restore(ui: Ui, send: &Send, path: &str) -> bool {
    match split(path).and_then(|(name, _)| find(name)).and_then(|k| k.restore) {
        Some(restore) => {
            restore(ui, send, path);
            true
        }
        None => false,
    }
}

/// What a kept layout's tabs wait for while its files come from the session: the files asked for and not here yet,
/// the order the tabs were in (the kept ones are made at once and the files come one by one, so the order is put
/// right as each comes), and the kept tab that was the one shown (each file is shown as it comes, and that tab is
/// shown again after the last of them; none: a file was the one shown).
#[derive(Clone, Default)]
struct Waiting {
    files: Vec<String>,
    order: Vec<String>,
    shown: Option<String>,
}

#[derive(Clone, Copy)]
struct Restoring(RwSignal<Option<Waiting>>);

impl crate::ext::State for Restoring {
    fn new(ui: Ui) -> Restoring {
        Restoring(ui.scope.create_rw_signal(None))
    }
}

/// The tabs kept with a layout (its `kind_tabs`) made again, each by its kind. Once the layout's files have come
/// the tabs are in the order they were kept in (its `tabs`), and the tab shown last is shown again when it was one
/// of these.
pub fn restore_all(ui: Ui, send: &Send, kept: &Value) {
    let paths = |key: &str| -> Vec<String> {
        kept[key].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
    };
    let tabs = paths("kind_tabs");
    if tabs.is_empty() {
        return;
    }
    for path in &tabs {
        restore(ui, send, path);
    }
    let shown = kept["active"].as_str().filter(|a| tabs.iter().any(|t| t == a)).map(str::to_string);
    let files = paths("editors");
    if !files.is_empty() {
        ui.state::<Restoring>().0.set(Some(Waiting { files, order: paths("tabs"), shown }));
    } else if let Some(shown) = shown
        && ui.tab(&shown).is_some()
    {
        ui.active.set(Some(shown));
    }
}

/// The tabs put in a kept order: those it names as it has them, the others after them as they are. The tabs are
/// set only when they are not in that order already.
fn in_order(ui: Ui, order: &[String]) {
    let at = |path: &str| order.iter().position(|o| o == path).unwrap_or(usize::MAX);
    if ui.tabs.with_untracked(|tabs| tabs.windows(2).any(|w| at(&w[0].path) > at(&w[1].path))) {
        ui.tabs.update(|tabs| tabs.sort_by_key(|t| at(&t.path)));
    }
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's message of a file read, after the editor has taken it in, while a kept layout's files are coming:
/// the tabs are put in the order they were kept in, and after the last of the files the kept tab that was shown is
/// shown again. Never taken away (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] != "file" {
        return false;
    }
    let waiting = ui.state::<Restoring>().0;
    if waiting.with_untracked(|w| w.is_none()) {
        return false;
    }
    let path = m["path"].as_str().unwrap_or("");
    let (mut order, mut done) = (Vec::new(), None);
    waiting.update(|w| {
        let Some(now) = w.as_mut() else { return };
        now.files.retain(|f| f != path);
        if now.files.is_empty() {
            done = w.take();
        } else {
            order = now.order.clone();
        }
    });
    in_order(ui, done.as_ref().map_or(&order, |d| &d.order));
    if let Some(shown) = done.and_then(|d| d.shown)
        && ui.tab(&shown).is_some()
    {
        ui.active.set(Some(shown));
    }
    false
}

/// Whether the tab shown is a file's text (or a file's changes), not a tab of a kind: what the status bar's items
/// of a file are shown by.
pub fn text_shown(ui: Ui) -> bool {
    ui.active.with(|a| {
        a.as_ref().is_some_and(|p| ui.tabs.with(|tabs| tabs.iter().any(|t| t.path == *p && t.kind.is_none())))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sender() -> (Send, Rc<RefCell<Vec<Value>>>) {
        let said = Rc::new(RefCell::new(Vec::new()));
        let to = said.clone();
        (Rc::new(move |v: Value| to.borrow_mut().push(v)), said)
    }

    fn test_kinds() {
        add(kind("test-preview", |_, rest| format!("Preview {rest}"), |_, _, _| empty().into_any())
            .icon("open-preview")
            .save(|ui, send, tab| {
                send(json!({"type": "test_save", "path": tab.file}));
                saved(ui, &tab.path);
            })
            .kept(|ui, _, path| open(ui, path)));
        add(kind("test-once", |_, _| "Once".to_string(), |_, _, _| empty().into_any()));
    }

    #[test]
    fn a_tab_of_a_kind_is_among_the_tabs_and_is_not_a_file() {
        let ui = Ui::new();
        let (send, said) = sender();
        test_kinds();
        open(ui, "no-such-kind:x");
        assert!(ui.tabs.with_untracked(|t| t.is_empty()), "a kind that was not added opens nothing");
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "fn a() {}\n", "modified": 1}), &send);
        open(ui, "test-preview:docs/a.md");
        open(ui, "test-preview:docs/a.md");
        assert_eq!(ui.tabs.with_untracked(|t| t.len()), 2, "opened once");
        assert_eq!(ui.active.get_untracked().as_deref(), Some("test-preview:docs/a.md"));
        let tab = ui.tab("test-preview:docs/a.md").expect("open");
        assert!(!tab.is_file() && ui.tab("a.rs").is_some_and(|t| t.is_file()));
        assert_eq!(title(ui, &tab), "Preview docs/a.md");
        assert_eq!(icon(&tab), "open-preview");
        assert_eq!(title(ui, &ui.tab("a.rs").expect("open")), "a.rs");
        assert!(!text_shown(ui));
        // Its kind saves it, the session is not asked to write a file of its path.
        tab.dirty.set(true);
        ui.save(&send);
        assert_eq!(said.borrow().last(), Some(&json!({"type": "test_save", "path": "docs/a.md"})));
        assert!(!tab.dirty.get_untracked());
        // Closed with changes not saved: asked first, and Save closes it once its kind has saved it.
        tab.dirty.set(true);
        ui.close(&tab.path);
        assert_eq!(ui.closing.get_untracked().as_deref(), Some(tab.path.as_str()));
        ui.close_saved.set(Some(tab.path.clone()));
        ui.closing.set(None);
        ui.save_tab(&tab.path, &send);
        assert!(ui.tab(&tab.path).is_none());
        assert_eq!(ui.active.get_untracked().as_deref(), Some("a.rs"));
        assert!(text_shown(ui));
        // A folder renamed or deleted takes the files' tabs with it, not a kind's.
        open(ui, "test-once:a.rs");
        ui.moved("a.rs", "b.rs");
        ui.gone("b.rs");
        assert_eq!(
            ui.tabs.with_untracked(|t| t.iter().map(|t| t.path.clone()).collect::<Vec<_>>()),
            ["test-once:a.rs"]
        );
    }

    #[test]
    fn the_tabs_a_kind_keeps_are_kept_with_the_layout_and_made_again() {
        let ui = Ui::new();
        let (send, _) = sender();
        test_kinds();
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "", "modified": 1}), &send);
        open(ui, "test-once:1");
        open(ui, "test-preview:README.md");
        assert_eq!(kept(ui), ["test-preview:README.md"]);
        let state = ui.view_state();
        assert_eq!(state["editors"], json!(["a.rs"]), "the files, as before");
        assert_eq!(state["kind_tabs"], json!(["test-preview:README.md"]));
        assert_eq!(state["tabs"], json!(["a.rs", "test-preview:README.md"]), "the order of those that are kept");
        assert_eq!(state["active"], json!("test-preview:README.md"));
        // The next start: the files are asked of the session, the kept tab is made by its kind and shown.
        let again = Ui::new();
        let (send, said) = sender();
        again.take(&json!({"type": "state", "view_state": state}), &send);
        let asked: Vec<Value> = said.borrow().iter().filter(|m| m["type"] == "open").cloned().collect();
        assert_eq!(asked, [json!({"type": "open", "path": "a.rs"})], "a kind's tab is not asked for as a file");
        assert!(again.tab("test-preview:README.md").is_some() && again.tab("test-once:1").is_none());
        // The file comes and is shown, as every file that comes; it was the last asked for, so the tab that was
        // shown when the layout was kept is shown again.
        again.take(&json!({"type": "file", "path": "a.rs", "text": "", "modified": 1}), &send);
        let paths = |ui: Ui| ui.tabs.with_untracked(|t| t.iter().map(|t| t.path.clone()).collect::<Vec<_>>());
        assert_eq!(paths(again), ["a.rs", "test-preview:README.md"], "in the order they were kept in");
        assert_eq!(again.active.get_untracked().as_deref(), Some("test-preview:README.md"));
        // A file opened after that is shown, as ever.
        again.take(&json!({"type": "file", "path": "b.rs", "text": "", "modified": 1}), &send);
        assert_eq!(again.active.get_untracked().as_deref(), Some("b.rs"));
        assert!(!restore(again, &send, "test-once:1") && !restore(again, &send, "no-such-kind:x"));
    }

    #[test]
    fn kept_tabs_come_back_among_the_files_tabs_where_they_were() {
        let ui = Ui::new();
        let (send, _) = sender();
        test_kinds();
        let file = |ui: Ui, send: &Send, path: &str| {
            ui.take(&json!({"type": "file", "path": path, "text": "", "modified": 1}), send)
        };
        let paths = |ui: Ui| ui.tabs.with_untracked(|t| t.iter().map(|t| t.path.clone()).collect::<Vec<_>>());
        file(ui, &send, "a.rs");
        open(ui, "test-preview:a.md");
        file(ui, &send, "b.rs");
        open(ui, "test-preview:b.md");
        file(ui, &send, "c.rs");
        let order = ["a.rs", "test-preview:a.md", "b.rs", "test-preview:b.md", "c.rs"];
        assert_eq!(paths(ui), order);
        let state = ui.view_state();
        assert_eq!(state["tabs"], json!(order));
        // The next start: the kept tabs are there at once, the files come one by one, each in its place; a file
        // that is gone from disk leaves the others in their order, and one another window opened comes after.
        let again = Ui::new();
        let (send, _) = sender();
        again.take(&json!({"type": "state", "view_state": state}), &send);
        assert_eq!(paths(again), ["test-preview:a.md", "test-preview:b.md"]);
        file(again, &send, "a.rs");
        assert_eq!(paths(again), ["a.rs", "test-preview:a.md", "test-preview:b.md"]);
        file(again, &send, "other.rs");
        again.take(&json!({"type": "file", "path": "b.rs", "error": "No such file"}), &send);
        assert_eq!(paths(again), ["a.rs", "test-preview:a.md", "test-preview:b.md", "other.rs"]);
        file(again, &send, "c.rs");
        assert_eq!(paths(again), ["a.rs", "test-preview:a.md", "test-preview:b.md", "c.rs", "other.rs"]);
        // A file was the one shown: it is, once it has come (asked for once more, last).
        file(again, &send, "c.rs");
        assert_eq!(again.active.get_untracked().as_deref(), Some("c.rs"));
        // The files have all come: a tab opened from then on goes last, wherever a kept order had it.
        again.close_now("a.rs");
        file(again, &send, "a.rs");
        assert_eq!(paths(again).last().map(String::as_str), Some("a.rs"));
        // A layout kept before the order was (no `tabs`): the tabs stay as they come.
        let mut old = ui.view_state();
        old.as_object_mut().expect("an object").remove("tabs");
        let third = Ui::new();
        third.take(&json!({"type": "state", "view_state": old}), &send);
        file(third, &send, "a.rs");
        assert_eq!(paths(third), ["test-preview:a.md", "test-preview:b.md", "a.rs"]);
    }
}
