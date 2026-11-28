//! The breadcrumbs under a group's tabs, as VS Code's: the folders of the file the group shows, the file, then the
//! symbols the caret is in, a chevron between each. A click on a part opens a list under it, as the old Code
//! Arena's: a folder's (and the file's) the files of that folder to open ("No files here, only folders." when it has
//! none), a symbol's the symbols beside it with their kinds, to go to. Typing filters the list; Up and Down move in
//! it, Enter takes a row, Escape closes it.
//!
//! `breadcrumbs.enabled` shows them, `breadcrumbs.filePath` the folders, `breadcrumbs.symbolPath` the symbols. Focus
//! and Select Breadcrumbs (Ctrl+Shift+.) gives the last part the keys and opens its list, Focus Breadcrumbs
//! (Ctrl+Shift+;) gives it the keys only; then Left and Right move between the parts, Enter, Space or Down open one's
//! list and Escape gives the keys back to the editor.
//!
//! Other parts put buttons at the bar's right end (Preview, Open as Notebook, Open Workspace) with `end`, from their
//! `register`:
//!
//! ```ignore
//! breadcrumbs::end("open-preview", "Open Preview to the Side", |_ui, file| file.ends_with(".md"), |ui, send, file| …);
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use floem::IntoView;
use floem::action::remove_overlay;
use floem::event::{Event, EventListener};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::Rect;
use floem::reactive::{
    Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo, create_rw_signal,
};
use floem::style::CursorStyle;
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, container, dyn_container, dyn_stack, empty, h_stack, label, scroll,
    stack_from_iter, v_stack, virtual_stack,
};
use floem::{View, ViewId};
use serde_json::Value;

use crate::commands::{add, command};
use crate::icons::icon;
use crate::outline::{self, Kind, Symbol};
use crate::state::{Send, Ui};
use crate::widgets::{cover, solid, window_size};

/// What a part of the breadcrumbs is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    /// A folder of the file's path (`src/ui`), listing its files.
    Folder(String),
    /// The file, listing the files beside it.
    File(String),
    /// A symbol the caret is in (its place among the file's symbols), listing the symbols beside it.
    Symbol(usize),
}

/// A part as drawn: its text and its icon.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Crumb {
    pub text: String,
    pub icon: &'static str,
    pub part: Part,
}

/// The parts of a file's path: its folders (when `folders` is on), then the file.
pub fn path_crumbs(file: &str, folders: bool) -> Vec<Crumb> {
    let names: Vec<&str> = file.split('/').filter(|n| !n.is_empty()).collect();
    let Some((last, before)) = names.split_last() else { return Vec::new() };
    let mut out = Vec::new();
    if folders {
        for n in 0..before.len() {
            out.push(Crumb { text: before[n].to_string(), icon: "folder", part: Part::Folder(before[..=n].join("/")) });
        }
    }
    out.push(Crumb {
        text: last.to_string(),
        icon: crate::explorer::icon_of(last, false, false),
        part: Part::File(file.to_string()),
    });
    out
}

/// The folder a file is in ("" for the project's top).
pub fn folder_of(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(f, _)| f)
}

/// The rows of a folder's list: its files, in the order given, each with its path. None but folders: none.
pub fn files_in(folder: &str, entries: &[crate::state::Entry]) -> Vec<(String, String)> {
    entries
        .iter()
        .filter(|e| !e.dir)
        .map(|e| (e.name.clone(), if folder.is_empty() { e.name.clone() } else { format!("{folder}/{}", e.name) }))
        .collect()
}

/// Whether a right-end button is shown for a file.
pub type When = Rc<dyn Fn(Ui, &str) -> bool>;
/// What a right-end button does for the file shown.
pub type Press = Rc<dyn Fn(Ui, &Send, &str)>;

/// What another part puts at the bar's right end: an action shown while `when` holds for the file shown.
#[derive(Clone)]
pub struct End {
    pub icon: &'static str,
    pub tip: &'static str,
    pub when: When,
    pub run: Press,
}

thread_local! {
    static ENDS: RefCell<Vec<End>> = const { RefCell::new(Vec::new()) };
}

/// A button at the right end of the breadcrumbs, from a part's `register`; one with the same icon and tip takes the
/// first's place.
#[allow(dead_code)] // Called by the parts to come (Preview, Open as Notebook, Open Workspace).
pub fn end(
    icon: &'static str,
    tip: &'static str,
    when: impl Fn(Ui, &str) -> bool + 'static,
    run: impl Fn(Ui, &Send, &str) + 'static,
) {
    let e = End { icon, tip, when: Rc::new(when), run: Rc::new(run) };
    ENDS.with(|all| {
        let mut all = all.borrow_mut();
        match all.iter_mut().find(|o| o.icon == icon && o.tip == tip) {
            Some(o) => *o = e,
            None => all.push(e),
        }
    });
}

fn ends() -> Vec<End> {
    ENDS.with(|all| all.borrow().clone())
}

/// The breadcrumbs' state: the settings they follow, what Focus Breadcrumbs asked last (a count, and whether a list
/// opens), the folder waited for, and the list open.
#[derive(Clone, Copy)]
struct Crumbs {
    enabled: Memo<Value>,
    file_path: Memo<Value>,
    symbol_path: Memo<Value>,
    asked: RwSignal<(u64, bool)>,
    waiting: RwSignal<Option<(String, ViewId, Option<ViewId>)>>,
    open: RwSignal<Option<ViewId>>,
    /// The part the open list is under.
    from: RwSignal<Option<ViewId>>,
    typed: RwSignal<String>,
    lit: RwSignal<usize>,
}

impl crate::ext::State for Crumbs {
    fn new(ui: Ui) -> Crumbs {
        Crumbs {
            enabled: crate::settings::memo(ui, "breadcrumbs.enabled", || None),
            file_path: crate::settings::memo(ui, "breadcrumbs.filePath", || None),
            symbol_path: crate::settings::memo(ui, "breadcrumbs.symbolPath", || None),
            asked: ui.scope.create_rw_signal((0, false)),
            waiting: ui.scope.create_rw_signal(None),
            open: ui.scope.create_rw_signal(None),
            from: ui.scope.create_rw_signal(None),
            typed: ui.scope.create_rw_signal(String::new()),
            lit: ui.scope.create_rw_signal(0),
        }
    }
}

/// Whether the breadcrumbs show the symbols the caret is in (the Outline's symbols are found for them then).
pub fn shows_symbols(ui: Ui) -> bool {
    let c = ui.state::<Crumbs>();
    c.enabled.with(|v| v.as_bool() == Some(true)) && c.symbol_path.with(|v| v.as_str() != Some("off"))
}

pub(super) fn breadcrumbs(ui: Ui, active: Memo<Option<String>>, focused: Memo<bool>) -> impl IntoView {
    let t = ui.colors;
    let c = ui.state::<Crumbs>();
    // Shown for a file's text (or its changes), not for a tab of a kind, while `breadcrumbs.enabled` is on.
    let file = create_memo(move |_| {
        active.with(|a| a.as_ref().and_then(|p| ui.tab(p)).filter(|t| t.kind.is_none()).map(|t| (t.path, t.file)))
    });
    let shown = create_memo(move |_| {
        let on = c.enabled.with(|v| v.as_bool() == Some(true));
        file.get().filter(|_| on).map(|f| (f, c.file_path.with(|v| v.as_str() != Some("off"))))
    });
    dyn_container(
        move || shown.get(),
        move |shown| match shown {
            Some(((path, file), folders)) => bar(ui, path, file, folders, focused).into_any(),
            None => empty().into_any(),
        },
    )
    .style(move |s| {
        let c = t.get();
        s.width_full()
            .font_size(13.0)
            .color(c.breadcrumb)
            .background(c.breadcrumb_bg)
            .selectable(false)
            .apply_if(shown.with(Option::is_none), |s| s.hide())
    })
}

/// The bar for one file: its path's parts, the symbols the caret is in, and the right end's buttons.
fn bar(ui: Ui, path: String, file: String, folders: bool, focused: Memo<bool>) -> impl IntoView {
    let t = ui.colors;
    let c = ui.state::<Crumbs>();
    let parts = path_crumbs(&file, folders);
    let first = parts.len();
    let keyed_parts = parts.clone();
    // The symbols the caret is in, while this bar's file is the one with the keys (the caret and the symbols are
    // the focused editor's).
    let at = path.clone();
    let chain = create_memo(move |_| {
        if !shows_symbols(ui) || ui.active.with(|a| a.as_deref() != Some(at.as_str())) {
            return Vec::new();
        }
        let line = ui.caret.get().0;
        ui.symbols.with(|(p, s)| {
            if *p != at {
                return Vec::new();
            }
            outline::chain(s, line)
                .into_iter()
                .map(|i| Crumb { text: s[i].name.clone(), icon: s[i].kind.icon(), part: Part::Symbol(i) })
                .collect::<Vec<_>>()
        })
    });
    // The part with the keys while the bar has them, and each part's view (to open its list under it).
    let lit = create_rw_signal(None::<usize>);
    let ids: Rc<RefCell<HashMap<usize, ViewId>>> = Rc::new(RefCell::new(HashMap::new()));
    let count = move || first + chain.with(Vec::len);
    let row = h_stack((
        stack_from_iter(parts.into_iter().enumerate().map(|(n, p)| crumb(ui, n, p, lit, ids.clone(), count))),
        {
            let ids = ids.clone();
            dyn_stack(
                move || chain.get().into_iter().enumerate().collect::<Vec<_>>(),
                |(n, p): &(usize, Crumb)| (*n, p.clone()),
                move |(n, p)| crumb(ui, first + n, p, lit, ids.clone(), count),
            )
        },
        empty().style(|s| s.flex_grow(1.0_f32)),
        stack_from_iter(ends().into_iter().map(|e| {
            let (when, run, file) = (e.when.clone(), e.run.clone(), file.clone());
            let shown_for = file.clone();
            floem::views::tooltip(
                crate::widgets::action(t, e.icon, move || super::tell(ui, |send| run(ui, send, &file))),
                move || label(move || e.tip),
            )
            .style(move |s| s.apply_if(!when(ui, &shown_for), |s| s.hide()))
        }))
        .style(|s| s.gap(2.0).padding_right(8.0)),
    ))
    .style(|s| s.height(22.0).width_full().items_center().padding_left(12.0));
    let id = row.id();
    let (opened_parts, opened_ids) = (keyed_parts.clone(), ids.clone());
    // Focus Breadcrumbs: the focused group's bar has the keys, its last part lit (and its list opened).
    create_effect(move |before: Option<u64>| {
        let (n, select) = c.asked.get();
        if before.is_some_and(|b| b != n) && focused.get_untracked() {
            let last = count().saturating_sub(1);
            lit.set(Some(last));
            id.request_focus();
            if select {
                open(ui, last, &chain.get_untracked(), &opened_parts, &opened_ids, id);
            }
        }
        n
    });
    row.keyboard_navigable()
        .on_event_cont(EventListener::FocusLost, move |_| {
            if lit.get_untracked().is_some() && c.open.get_untracked().is_none() {
                lit.set(None);
            }
        })
        .on_event_stop(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return };
            let n = count();
            let Some(at) = lit.get_untracked().filter(|_| n > 0) else { return };
            match &k.key.logical_key {
                Key::Named(NamedKey::ArrowLeft) => lit.set(Some(at.saturating_sub(1))),
                Key::Named(NamedKey::ArrowRight) => lit.set(Some((at + 1).min(n - 1))),
                Key::Named(NamedKey::Home) => lit.set(Some(0)),
                Key::Named(NamedKey::End) => lit.set(Some(n - 1)),
                Key::Named(NamedKey::Enter | NamedKey::Space | NamedKey::ArrowDown) => {
                    open(ui, at, &chain.get_untracked(), &keyed_parts, &ids, id)
                }
                Key::Named(NamedKey::Escape) => {
                    lit.set(None);
                    if let Some(p) = ui.active.get_untracked()
                        && let Some(editor) = super::live(ui, &p)
                    {
                        super::focus(&editor);
                    }
                }
                _ => {}
            }
        })
        .style(|s| s.width_full())
}

/// One part: its icon (a symbol's in its kind's colour), its text, and the chevron after it but the last.
fn crumb(
    ui: Ui,
    n: usize,
    p: Crumb,
    lit: RwSignal<Option<usize>>,
    ids: Rc<RefCell<HashMap<usize, ViewId>>>,
    count: impl Fn() -> usize + Copy + 'static,
) -> impl IntoView {
    let t = ui.colors;
    let c = ui.state::<Crumbs>();
    let kind = match p.part {
        Part::Symbol(i) => ui.symbols.with_untracked(|(_, s)| s.get(i).map(|s| s.kind)),
        _ => None,
    };
    let (text, glyph) = (p.text.clone(), p.icon);
    let part = h_stack((
        icon(glyph, 16.0).style(move |s| {
            let c = t.get();
            s.color(kind.map_or(c.icon, |k| k.colour(&c)))
        }),
        label(move || text.clone()).style(|s| s.text_ellipsis()),
    ));
    let id = part.id();
    ids.borrow_mut().insert(n, id);
    let bar_lit = move || lit.get() == Some(n);
    let opened = move || c.from.get() == Some(id);
    h_stack((
        part.on_click_stop(move |_| {
            show_list(ui, &p, id, None);
        })
        .style(move |s| {
            let c = t.get();
            let on = bar_lit() || opened();
            s.items_center()
                .gap(4.0)
                .height(20.0)
                .padding_horiz(2.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(if bar_lit() { c.focus } else { floem::peniko::Color::TRANSPARENT })
                .cursor(CursorStyle::Pointer)
                .apply_if(on, |s| s.color(c.breadcrumb_focus))
                .hover(|s| s.color(c.breadcrumb_focus))
        }),
        icon("chevron-right", 16.0).style(move |s| s.apply_if(n + 1 >= count(), |s| s.hide())),
    ))
    .style(|s| s.items_center().min_width(0.0))
}

/// Opens the list of the n-th part (from the keys).
fn open(ui: Ui, n: usize, chain: &[Crumb], parts: &[Crumb], ids: &RefCell<HashMap<usize, ViewId>>, bar: ViewId) {
    let p = if n < parts.len() { parts.get(n) } else { chain.get(n - parts.len()) };
    let Some(p) = p.cloned() else { return };
    let under = ids.borrow().get(&n).copied().unwrap_or(bar);
    show_list(ui, &p, under, Some(bar));
}

/// A row of a part's list.
#[derive(Clone)]
struct Pick {
    text: String,
    icon: &'static str,
    kind: Option<Kind>,
    /// Said dimmed after the text (a symbol's kind).
    detail: &'static str,
    /// The file or symbol the part is: the row the list opens on.
    here: bool,
    run: Rc<dyn Fn()>,
}

/// Shows a part's list under it: a folder's at once when the Explorer has read the folder, else once it is read.
fn show_list(ui: Ui, p: &Crumb, under: ViewId, back: Option<ViewId>) {
    match &p.part {
        Part::Folder(folder) => files_list(ui, folder.clone(), None, under, back),
        Part::File(file) => files_list(ui, folder_of(file).to_string(), Some(file.clone()), under, back),
        Part::Symbol(i) => {
            let Some(path) = ui.active.get_untracked() else { return };
            let picks = ui.symbols.with_untracked(|(_, s)| symbol_picks(ui, &path, s, *i));
            picker(ui, under, picks, "No symbols here.", back);
        }
    }
}

fn symbol_picks(ui: Ui, path: &str, symbols: &[Symbol], i: usize) -> Vec<Pick> {
    outline::siblings(symbols, i)
        .into_iter()
        .map(|j| {
            let (s, path) = (&symbols[j], path.to_string());
            let line = s.line;
            Pick {
                text: s.name.clone(),
                icon: s.kind.icon(),
                kind: Some(s.kind),
                detail: s.kind.word(),
                here: j == i,
                run: Rc::new(move || super::tell(ui, |send| ui.open_at(&path, line, send))),
            }
        })
        .collect()
}

/// A folder's files, the file given lit; read from the session when the window has not read the folder yet.
fn files_list(ui: Ui, folder: String, here: Option<String>, under: ViewId, back: Option<ViewId>) {
    match ui.tree.with_untracked(|t| t.get(&folder).cloned()) {
        Some(entries) => picker(ui, under, file_picks(ui, &folder, &entries, here.as_deref()), NO_FILES, back),
        None => {
            ui.state::<Crumbs>().waiting.set(Some((folder.clone(), under, back)));
            super::tell(ui, |send| send(serde_json::json!({"type": "ls", "path": folder})));
        }
    }
}

const NO_FILES: &str = "No files here, only folders.";

fn file_picks(ui: Ui, folder: &str, entries: &[crate::state::Entry], here: Option<&str>) -> Vec<Pick> {
    files_in(folder, entries)
        .into_iter()
        .map(|(name, path)| Pick {
            icon: crate::explorer::icon_of(&name, false, false),
            here: here == Some(path.as_str()),
            text: name,
            kind: None,
            detail: "",
            run: Rc::new(move || super::tell(ui, |send| ui.open(&path, send))),
        })
        .collect()
}

/// The rows a typed filter leaves, by their place in the list.
fn filtered(picks: &[Pick], typed: &str) -> im::Vector<usize> {
    (0..picks.len()).filter(|&i| outline::matches(&picks[i].text, typed)).collect()
}

/// The list open closed, the keys back where they were.
fn close_list(ui: Ui, back: Option<ViewId>) {
    let c = ui.state::<Crumbs>();
    if let Some(id) = c.open.get_untracked() {
        c.open.set(None);
        c.from.set(None);
        remove_overlay(id);
        if let Some(b) = back {
            b.request_focus();
        }
    }
}

/// The most rows shown at once; more scroll.
const MOST_ROWS: usize = 12;
const ROW: f64 = 22.0;

/// Opens a list of rows under a part, as VS Code's breadcrumbs picker: 300 px wide, 22 px rows, the part's own row
/// lit; a click or Enter takes a row.
fn picker(ui: Ui, under: ViewId, picks: Vec<Pick>, none: &'static str, back: Option<ViewId>) {
    let t = ui.colors;
    let c = ui.state::<Crumbs>();
    close_list(ui, None);
    if crate::widgets::popup::is_open(ui) {
        crate::widgets::popup::close(ui);
    }
    let window = window_size(ui, Some(under));
    let by = under.layout_rect();
    let picks = Rc::new(picks);
    if c.typed.with_untracked(|t| !t.is_empty()) {
        c.typed.set(String::new());
    }
    c.lit.set(picks.iter().position(|p| p.here).unwrap_or(0));
    let width = 300.0_f64.min(window.width - 16.0);
    let height = (picks.len().clamp(1, MOST_ROWS) as f64) * ROW + 2.0;
    let left = by.x0.min(window.width - width - 8.0).max(0.0);
    let top = if by.y1 + height + 8.0 > window.height { (by.y0 - height - 2.0).max(0.0) } else { by.y1 + 2.0 };
    let keyed = picks.clone();
    let take = move |i: usize| {
        if let Some(p) = keyed.get(i) {
            let run = p.run.clone();
            close_list(ui, None);
            run();
        }
    };
    let take_key = take.clone();
    let key_picks = picks.clone();
    let key = move |k: &Key, m: Modifiers| {
        let shown = c.typed.with_untracked(|t| filtered(&key_picks, t));
        let n = shown.len();
        let at = c.lit.get_untracked().min(n.saturating_sub(1));
        let to = |i: usize| {
            if c.lit.get_untracked() != i {
                c.lit.set(i);
            }
        };
        match k {
            Key::Named(NamedKey::ArrowDown) => to((at + 1).min(n.saturating_sub(1))),
            Key::Named(NamedKey::ArrowUp) => to(at.saturating_sub(1)),
            Key::Named(NamedKey::PageDown) => to((at + MOST_ROWS).min(n.saturating_sub(1))),
            Key::Named(NamedKey::PageUp) => to(at.saturating_sub(MOST_ROWS)),
            Key::Named(NamedKey::Home) => to(0),
            Key::Named(NamedKey::End) => to(n.saturating_sub(1)),
            Key::Named(NamedKey::Enter) => {
                if let Some(&i) = shown.get(at) {
                    take_key(i);
                }
            }
            Key::Named(NamedKey::Escape) => close_list(ui, back),
            Key::Named(NamedKey::Backspace) => {
                if c.typed.with_untracked(|t| !t.is_empty()) {
                    c.typed.update(|t| {
                        t.pop();
                    });
                    to(0);
                }
            }
            Key::Character(ch) if !m.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::META) => {
                c.typed.update(|t| t.push_str(ch));
                to(0);
            }
            _ => {}
        }
    };
    let id = cover(
        ui,
        window,
        false,
        key,
        move || close_list(ui, None),
        move |keep| {
            let drawn = picks.clone();
            let shown = create_memo(move |_| c.typed.with(|t| filtered(&drawn, t)));
            let drawn = picks.clone();
            let list = virtual_stack(
                VirtualDirection::Vertical,
                VirtualItemSize::Fixed(Box::new(|| ROW)),
                move || shown.get().into_iter().enumerate().collect::<im::Vector<_>>(),
                |(n, i): &(usize, usize)| (*n, *i),
                move |(n, i)| {
                    let p = drawn[i].clone();
                    let take = take.clone();
                    let (text, kind) = (p.text.clone(), p.kind);
                    h_stack((
                        icon(p.icon, 16.0).style(move |s| {
                            let c = t.get();
                            s.color(kind.map_or(c.icon, |k| k.colour(&c)))
                        }),
                        label(move || text.clone()).style(|s| s.padding_left(6.0).text_ellipsis().min_width(0.0)),
                        label(move || p.detail).style(move |s| s.padding_left(8.0).color(t.get().desc)),
                    ))
                    .on_click_stop(move |_| take(i))
                    .on_event_cont(EventListener::PointerEnter, move |_| {
                        if c.lit.get_untracked() != n {
                            c.lit.set(n);
                        }
                    })
                    .style(move |s| {
                        let c = t.get();
                        let lit = ui.state::<Crumbs>().lit.get() == n;
                        s.height(ROW)
                            .width_full()
                            .items_center()
                            .padding_horiz(8.0)
                            .cursor(CursorStyle::Pointer)
                            .apply_if(lit, |s| s.background(c.list_selected).color(c.list_selected_fg))
                    })
                },
            )
            .style(|s| s.flex_col().width_full());
            let empty_text =
                label(move || if c.typed.with(String::is_empty) { none } else { "No results." }).style(move |s| {
                    s.height(ROW)
                        .padding_horiz(8.0)
                        .items_center()
                        .color(t.get().desc)
                        .apply_if(shown.with(|r| !r.is_empty()), |s| s.hide())
                });
            let typed = label(move || c.typed.get()).style(move |s| {
                let c = t.get();
                s.absolute()
                    .inset_top(2.0)
                    .inset_right(4.0)
                    .padding_horiz(6.0)
                    .border(1.0)
                    .border_radius(2.0)
                    .border_color(c.focus)
                    .background(c.input_bg)
                    .color(c.input_fg)
                    .apply_if(c_typed_empty(ui), |s| s.hide())
            });
            let body = floem::views::stack((
                v_stack((
                    empty_text,
                    scroll(list)
                        .ensure_visible(move || {
                            let at = ui.state::<Crumbs>().lit.get() as f64 * ROW;
                            Rect::new(0.0, at, 1.0, at + ROW)
                        })
                        .style(|s| s.size_full()),
                ))
                .style(|s| s.size_full()),
                typed,
            ))
            .style(move |s| {
                let c = t.get();
                s.absolute()
                    .inset_left(left)
                    .inset_top(top)
                    .width(width)
                    .height(height)
                    .border(1.0)
                    .border_color(c.widget_border)
                    .background(c.breadcrumb_picker)
                    .box_shadow_blur(8.0)
                    .box_shadow_color(c.shadow)
                    .font_size(13.0)
                    .color(c.fg)
            });
            container(solid(body, keep)).style(|s| s.size_full()).into_any()
        },
    );
    c.open.set(Some(id));
    c.from.set(Some(under));
}

/// Whether nothing is typed in the list open (read where it is followed).
fn c_typed_empty(ui: Ui) -> bool {
    ui.state::<Crumbs>().typed.with(String::is_empty)
}

/// VS Code's commands for the breadcrumbs' keys.
pub const FOCUS_AND_SELECT: &str = "breadcrumbs.focusAndSelect";
pub const FOCUS: &str = "breadcrumbs.focus";

/// What it adds to the window's registries, once at start: Focus and Select Breadcrumbs, Focus Breadcrumbs.
pub fn register(ui: Ui, _send: &Send) {
    let _ = ui.state::<Crumbs>();
    let shown = |ui: Ui| {
        ui.state::<Crumbs>().enabled.with_untracked(|v| v.as_bool() == Some(true))
            && ui.active.with_untracked(|a| a.as_ref().and_then(|p| ui.tab(p)).is_some_and(|t| t.kind.is_none()))
    };
    add(command(FOCUS_AND_SELECT, "", "Focus and Select Breadcrumbs", |ui, _| ask(ui, true))
        .keys("Ctrl+Shift+.")
        .when(shown));
    add(command(FOCUS, "", "Focus Breadcrumbs", |ui, _| ask(ui, false)).keys("Ctrl+Shift+;").when(shown));
}

fn ask(ui: Ui, select: bool) {
    ui.state::<Crumbs>().asked.update(|(n, s)| {
        *n += 1;
        *s = select;
    });
}

/// The session's messages that are this part's, taken in: a folder read that a part's list waits for (the
/// Explorer takes it too). Never taken away from the rest of the window.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] != "ls" {
        return false;
    }
    let c = ui.state::<Crumbs>();
    let Some((folder, under, back)) = c.waiting.get_untracked() else { return false };
    if m["path"].as_str() != Some(folder.as_str()) {
        return false;
    }
    c.waiting.set(None);
    if let Some(e) = m["error"].as_str() {
        crate::widgets::toast::error(ui, &format!("Could not list {folder}: {e}"));
        return false;
    }
    let entries: Vec<crate::state::Entry> = m["entries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|e| crate::state::Entry { name: e["name"].as_str().unwrap_or("").into(), dir: e["dir"] == true })
        .collect();
    let here = ui.active.get_untracked().and_then(|p| ui.tab(&p)).map(|t| t.file);
    picker(ui, under, file_picks(ui, &folder, &entries, here.as_deref()), NO_FILES, back);
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Entry;

    #[test]
    fn a_files_path_is_its_folders_then_the_file() {
        let all = path_crumbs("src/ui/app.rs", true);
        let parts: Vec<_> = all.iter().map(|c| (c.text.as_str(), c.part.clone())).collect();
        assert_eq!(
            parts,
            [
                ("src", Part::Folder("src".into())),
                ("ui", Part::Folder("src/ui".into())),
                ("app.rs", Part::File("src/ui/app.rs".into())),
            ]
        );
        assert_eq!(all[0].icon, "folder");
        assert_eq!(all[2].icon, "file-code");
        // breadcrumbs.filePath off: the file alone.
        assert_eq!(path_crumbs("src/ui/app.rs", false).len(), 1);
        assert_eq!(path_crumbs("top.md", true)[0].part, Part::File("top.md".into()));
        assert!(path_crumbs("", true).is_empty());
        assert_eq!(folder_of("src/ui/app.rs"), "src/ui");
        assert_eq!(folder_of("top.md"), "");
    }

    #[test]
    fn a_folders_list_has_its_files_only() {
        let e = |name: &str, dir| Entry { name: name.into(), dir };
        let entries = [e("lib", true), e("a.rs", false), e("b.md", false)];
        assert_eq!(
            files_in("src", &entries),
            [("a.rs".to_string(), "src/a.rs".to_string()), ("b.md".to_string(), "src/b.md".to_string())]
        );
        assert_eq!(files_in("", &entries)[0].1, "a.rs");
        assert!(files_in("src", &[e("only", true)]).is_empty(), "then the list says there are only folders");
    }

    #[test]
    fn the_symbols_the_caret_is_in_and_those_beside_one() {
        let text = "class A:\n    def a(self):\n        pass\n    def b(self):\n        pass\nclass B:\n    pass\n";
        let all = outline::symbols("x.py", text);
        let names = |ix: Vec<usize>| ix.into_iter().map(|i| all[i].name.clone()).collect::<Vec<_>>();
        assert_eq!(names(outline::chain(&all, 5)), ["A", "b"]);
        assert_eq!(names(outline::chain(&all, 6)), ["B"]);
        assert!(outline::chain(&all, 0).is_empty(), "before the first symbol: none");
        assert_eq!(names(outline::siblings(&all, 2)), ["a", "b"]);
        assert_eq!(names(outline::siblings(&all, 0)), ["A", "B"]);
    }
}
