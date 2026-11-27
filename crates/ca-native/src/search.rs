//! The Search view, as VS Code's: the box with its Match Case, Match Whole Word and Use Regular Expression toggles,
//! the replace box under it (the chevron on the left, Ctrl+Shift+H), the files to include and to exclude (the "…"
//! button), then the count and the hits by file (the file's name, its folder dimmed, its count), each match on a
//! row with the match lit. A search goes out 300 ms after the last key; a click on a match opens its file as a
//! preview with the match selected. Replace All asks first and leaves files with changes not saved as they are;
//! a file's row and a match's row replace their own. What was typed is kept with the folder's view state.

use std::collections::HashSet;

use floem::keyboard::{Key, NamedKey};
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect};
use floem::style::CursorStyle;
use floem::views::editor::core::selection::Selection;
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, container, empty, h_stack, label, scroll, text_input, v_stack,
    virtual_stack,
};
use floem::{IntoView, View as _};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layout::View;
use crate::state::{Found, Hit, Send, Ui};
use crate::widgets::{bold, dialog, toast, toggle};

pub const REPLACE_IN_FILES: &str = "workbench.action.replaceInFiles";
pub const FIND_IN_FOLDER: &str = "filesExplorer.findInFolder";
pub const COLLAPSE: &str = "search.action.collapseSearchResults";
pub const EXPAND: &str = "search.action.expandSearchResults";
pub const REFRESH: &str = "search.action.refreshSearchResults";
pub const CLEAR: &str = "search.action.clearSearchResults";

/// What a search asks: the query, its toggles and the files it looks in. A reply carries it back, so an answer
/// to what was asked before is known.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asked {
    pub query: String,
    pub case: bool,
    pub regex: bool,
    pub word: bool,
    pub include: String,
    pub exclude: String,
}

impl Asked {
    fn of(m: &Value) -> Asked {
        let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
        Asked {
            query: s("query"),
            case: m["case"] == true,
            regex: m["regex"] == true,
            word: m["word"] == true,
            include: s("include"),
            exclude: s("exclude"),
        }
    }

    /// The message that asks it (`search`, `replace`).
    fn message(&self, kind: &str) -> Value {
        json!({"type": kind, "query": self.query, "case": self.case, "regex": self.regex, "word": self.word,
            "include": self.include, "exclude": self.exclude})
    }
}

/// The view's own state, besides the query and two toggles the window began with (`Ui::search_*`).
#[derive(Clone, Copy)]
struct Search {
    word: RwSignal<bool>,
    include: RwSignal<String>,
    exclude: RwSignal<String>,
    /// The files to include and to exclude shown ("Toggle Search Details").
    details: RwSignal<bool>,
    /// The replace box shown.
    replacing: RwSignal<bool>,
    replacement: RwSignal<String>,
    /// The search last sent, while its answer has not come.
    waiting: RwSignal<Option<Asked>>,
    /// What the results shown answer (a replace is asked with that, not with what is typed since).
    shown: RwSignal<Option<Asked>>,
    /// The replace sent last, whose answer is awaited.
    replaced: RwSignal<Option<Asked>>,
    /// The files whose matches are folded.
    folded: RwSignal<HashSet<String>>,
    /// Results taken in so far: the rows' keys change with each.
    generation: RwSignal<u64>,
    /// A match to select once its file is on screen: (path, line, col, len).
    select: RwSignal<Option<(String, usize, usize, usize)>>,
    query_box: RwSignal<Option<floem::ViewId>>,
    replace_box: RwSignal<Option<floem::ViewId>>,
    /// The kept state taken in already (once a window).
    restored: RwSignal<bool>,
}

impl crate::ext::State for Search {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        Search {
            word: s.create_rw_signal(false),
            include: s.create_rw_signal(String::new()),
            exclude: s.create_rw_signal(String::new()),
            details: s.create_rw_signal(false),
            replacing: s.create_rw_signal(false),
            replacement: s.create_rw_signal(String::new()),
            waiting: s.create_rw_signal(None),
            shown: s.create_rw_signal(None),
            replaced: s.create_rw_signal(None),
            folded: s.create_rw_signal(HashSet::new()),
            generation: s.create_rw_signal(0),
            select: s.create_rw_signal(None),
            query_box: s.create_rw_signal(None),
            replace_box: s.create_rw_signal(None),
            restored: s.create_rw_signal(false),
        }
    }
}

fn state(ui: Ui) -> Search {
    ui.state::<Search>()
}

/// What is typed now.
fn asked_now(ui: Ui) -> Asked {
    let st = state(ui);
    Asked {
        query: ui.search_query.get_untracked(),
        case: ui.search_case.get_untracked(),
        regex: ui.search_regex.get_untracked(),
        word: st.word.get_untracked(),
        include: st.include.get_untracked(),
        exclude: st.exclude.get_untracked(),
    }
}

/// The search asked now (none for an empty query: the results go).
fn search_now(ui: Ui, send: &Send) {
    let st = state(ui);
    let asked = asked_now(ui);
    if asked.query.is_empty() {
        st.waiting.set(None);
        if ui.search.with_untracked(Option::is_some) {
            ui.search.set(None);
        }
        return;
    }
    send(asked.message("search"));
    st.waiting.set(Some(asked));
}

/// The hits by file, in the order the search found them.
pub fn by_file(hits: &[Hit]) -> Vec<(String, Vec<Hit>)> {
    let mut out: Vec<(String, Vec<Hit>)> = Vec::new();
    for h in hits {
        match out.last_mut() {
            Some((p, v)) if *p == h.path => v.push(h.clone()),
            _ => out.push((h.path.clone(), vec![h.clone()])),
        }
    }
    out
}

/// A row of the results: a file, or one of its matches.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    File { path: String, count: usize, folded: bool },
    Match(Hit),
}

/// The rows drawn: each file, then its matches unless it is folded.
pub fn rows(hits: &[Hit], folded: &HashSet<String>) -> Vec<Row> {
    let mut out = Vec::new();
    for (path, hits) in by_file(hits) {
        let fold = folded.contains(&path);
        out.push(Row::File { path, count: hits.len(), folded: fold });
        if !fold {
            out.extend(hits.into_iter().map(Row::Match));
        }
    }
    out
}

/// The most of a match's line shown before it: a longer start is cut to its end, after "…", so the match shows.
const BEFORE: usize = 26;

/// A hit's text cut around its match: before (its leading spaces gone, a long start cut), the match, after.
pub fn cut(text: &str, at: usize, len: usize) -> (String, String, String) {
    let end = at + len;
    let (Some(before), Some(lit), Some(after)) =
        (text.get(..at), text.get(at..end.min(text.len())), text.get(end.min(text.len())..))
    else {
        return (text.trim_start().to_string(), String::new(), String::new());
    };
    let before = before.trim_start();
    let n = before.chars().count();
    let before =
        if n > BEFORE { format!("…{}", before.chars().skip(n - BEFORE).collect::<String>()) } else { before.into() };
    (before, lit.to_string(), after.to_string())
}

/// "3 results in 2 files", as VS Code counts.
pub fn summary(results: usize, files: usize) -> String {
    let s = |n: usize| if n == 1 { "" } else { "s" };
    format!("{results} result{} in {files} file{}", s(results), s(files))
}

/// Whether a file open in a tab has changes not saved.
fn unsaved(ui: Ui, path: &str) -> bool {
    ui.tab(path).is_some_and(|t| t.dirty.get_untracked())
}

/// Replace All: asked first ("Replace 3 results in 2 files?"); files with changes not saved keep their text.
fn replace_all(ui: Ui, send: &Send) {
    let st = state(ui);
    let Some(asked) = st.shown.get_untracked() else { return };
    let groups = ui.search.with_untracked(|r| r.as_ref().map(|r| by_file(&r.hits)).unwrap_or_default());
    let (kept, left): (Vec<_>, Vec<_>) = groups.into_iter().partition(|(p, _)| !unsaved(ui, p));
    if kept.is_empty() {
        if !left.is_empty() {
            toast::info(ui, "The files with results have changes not saved: save them first.");
        }
        return;
    }
    let results: usize = kept.iter().map(|(_, h)| h.len()).sum();
    let with = st.replacement.get_untracked();
    let mut ask = dialog::ask(&format!("Replace {}?", summary(results, kept.len())));
    if !left.is_empty() {
        ask = ask.detail(&format!(
            "{} file{} with changes not saved {} left as {} {}.",
            left.len(),
            if left.len() == 1 { "" } else { "s" },
            if left.len() == 1 { "is" } else { "are" },
            if left.len() == 1 { "it" } else { "they" },
            if left.len() == 1 { "is" } else { "are" },
        ));
    }
    let send = send.clone();
    let paths: Vec<String> = kept.into_iter().map(|(p, _)| p).collect();
    ask.button("Replace").default().open(ui, move |a| {
        if a.yes() {
            let mut m = asked.message("replace");
            m["replacement"] = json!(with);
            m["paths"] = json!(paths);
            send(m);
            st.replaced.set(Some(asked));
        }
    });
}

/// One file's matches replaced, or one match (`hit`).
fn replace_some(ui: Ui, send: &Send, path: &str, hit: Option<&Hit>) {
    let st = state(ui);
    let Some(asked) = st.shown.get_untracked() else { return };
    if unsaved(ui, path) {
        toast::info(ui, &format!("{path} has changes not saved: save it first."));
        return;
    }
    let mut m = asked.message("replace");
    m["replacement"] = json!(st.replacement.get_untracked());
    match hit {
        Some(h) => m["matches"] = json!([{"path": path, "line": h.line, "col": h.col}]),
        None => m["paths"] = json!([path]),
    }
    send(m);
    st.replaced.set(Some(asked));
}

/// A match opened as a preview, the match selected once its file is on screen.
fn open_match(ui: Ui, send: &Send, h: &Hit) {
    let st = state(ui);
    st.select.set(Some((h.path.clone(), h.line, h.col, h.len)));
    let n = ui.goto.with_untracked(|g| g.as_ref().map(|g| g.2 + 1).unwrap_or(0));
    ui.goto.set(Some((h.path.clone(), h.line, n)));
    crate::editor::tabs::open_preview(ui, &h.path, send);
    select_soon(ui);
}

/// The match waiting selected in its editor, once that is on screen and has gone to its line.
fn select_soon(ui: Ui) {
    floem::action::exec_after(std::time::Duration::from_millis(40), move |_| {
        let st = state(ui);
        let Some((path, line, col, len)) = st.select.get_untracked() else { return };
        let Some(editor) = ui.live.with_untracked(|l| l.as_ref().filter(|(p, _)| *p == path).map(|(_, e)| e.clone()))
        else {
            return;
        };
        if editor.cursor.try_get_untracked().is_none() {
            return;
        }
        st.select.set(None);
        let line = line.saturating_sub(1).min(editor.last_line());
        let start = editor.offset_of_line(line);
        let end_of_line = editor.offset_of_line(line + 1).max(start);
        let from = (start + col).min(end_of_line);
        let to = (from + len).min(end_of_line);
        editor.cursor.update(|c| c.set_insert(Selection::region(from, to)));
    });
}

/// A text box with its toggles, drawn as VS Code's (the focus border when it has the keys).
fn boxed(
    ui: Ui,
    input: impl IntoView + 'static,
    tools: impl IntoView + 'static,
    focused: RwSignal<bool>,
) -> impl IntoView {
    let t = ui.colors;
    h_stack((input, tools)).style(move |s| {
        let c = t.get();
        s.flex_grow(1.0_f32)
            .min_width(0.0)
            .height(26.0)
            .padding_right(3.0)
            .gap(2.0)
            .items_center()
            .border(1.0)
            .border_radius(2.0)
            .border_color(if focused.get() { c.focus } else { c.input_border })
            .background(c.input_bg)
    })
}

/// A text input as the boxes draw it.
fn input(ui: Ui, text: RwSignal<String>, hint: &'static str, focused: RwSignal<bool>) -> floem::views::TextInput {
    let t = ui.colors;
    text_input(text)
        .placeholder(hint)
        .on_event_cont(floem::event::EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(floem::event::EventListener::FocusLost, move |_| focused.set(false))
        .style(move |s| {
            let c = t.get();
            s.flex_grow(1.0_f32)
                .min_width(0.0)
                .padding_horiz(4.0)
                .border(0.0)
                .background(Color::TRANSPARENT)
                .color(c.input_fg)
                .cursor_color(floem::peniko::Brush::Solid(c.cursor))
        })
}

/// A small icon button (the replace chevron, Replace All, the row actions).
fn icon_button(ui: Ui, name: &'static str, tip: &'static str, run: impl Fn() + 'static) -> impl IntoView {
    let t = ui.colors;
    let b = container(icon(name, 16.0)).on_click_stop(move |_| run()).style(move |s| {
        let c = t.get();
        s.width(20.0)
            .height(20.0)
            .items_center()
            .justify_center()
            .border_radius(3.0)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .color(c.side_fg)
            .hover(|s| s.background(c.toolbar_hover))
    });
    floem::views::tooltip(b, move || label(move || tip))
}

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let st = state(ui);
    // Asked a moment after the last key, as VS Code searches while one types.
    let typed = ui.scope.create_rw_signal(0u64);
    {
        let send = send.clone();
        create_effect(move |first: Option<()>| {
            ui.search_query.track();
            ui.search_case.track();
            ui.search_regex.track();
            st.word.track();
            st.include.track();
            st.exclude.track();
            // The view drawn again (moved) asks nothing: the results it had are there.
            if first.is_none() {
                return;
            }
            typed.update(|g| *g += 1);
            let g = typed.get_untracked();
            let send = send.clone();
            floem::action::exec_after(std::time::Duration::from_millis(300), move |_| {
                if typed.get_untracked() == g {
                    search_now(ui, &send);
                    if ui.loaded.get_untracked() && !ui.running.get_untracked() {
                        send(json!({"type": "keep_view_state", "state": ui.view_state()}));
                    }
                }
            });
        });
    }
    let (qf, rf, inf, exf) = (
        ui.scope.create_rw_signal(false),
        ui.scope.create_rw_signal(false),
        ui.scope.create_rw_signal(false),
        ui.scope.create_rw_signal(false),
    );
    let go = send.clone();
    let query = input(ui, ui.search_query, "Search", qf).on_key_down(
        Key::Named(NamedKey::Enter),
        |m| m.is_empty(),
        move |_| search_now(ui, &go),
    );
    st.query_box.set(Some(query.id()));
    let all = send.clone();
    let replace = input(ui, st.replacement, "Replace", rf).on_key_down(
        Key::Named(NamedKey::Enter),
        |m| m.control(),
        move |_| replace_all(ui, &all),
    );
    st.replace_box.set(Some(replace.id()));
    let chevron = h_stack((
        container(icon_button(ui, "chevron-right", "Toggle Replace", move || st.replacing.set(true)))
            .style(move |s| s.apply_if(st.replacing.get(), |s| s.hide())),
        container(icon_button(ui, "chevron-down", "Toggle Replace", move || st.replacing.set(false)))
            .style(move |s| s.apply_if(!st.replacing.get(), |s| s.hide())),
    ))
    .style(|s| s.width(20.0).margin_top(3.0).items_start());
    let all = send.clone();
    let boxes = v_stack((
        boxed(
            ui,
            query,
            h_stack((
                toggle(t, "case-sensitive", ui.search_case, "Match Case"),
                toggle(t, "whole-word", st.word, "Match Whole Word"),
                toggle(t, "regex", ui.search_regex, "Use Regular Expression"),
            ))
            .style(|s| s.gap(2.0)),
            qf,
        ),
        h_stack((
            boxed(ui, replace, empty(), rf),
            icon_button(ui, "replace-all", "Replace All (Ctrl+Enter)", move || replace_all(ui, &all)),
        ))
        .style(move |s| s.gap(4.0).items_center().apply_if(!st.replacing.get(), |s| s.hide())),
    ))
    .style(|s| s.flex_grow(1.0_f32).min_width(0.0).gap(4.0));
    let top = h_stack((chevron, boxes)).style(|s| s.margin_left(4.0).margin_right(12.0).margin_top(4.0).gap(2.0));
    let toggle_details =
        container(icon_button(ui, "ellipsis", "Toggle Search Details", move || st.details.update(|d| *d = !*d)))
            .style(|s| s.width_full().justify_end().padding_right(12.0).padding_top(2.0));
    let caption = |text: &'static str| {
        label(move || text)
            .style(move |s| s.font_size(11.0).padding_top(4.0).padding_bottom(2.0).color(t.get().side_fg))
    };
    let details = v_stack((
        caption("files to include"),
        boxed(ui, input(ui, st.include, "e.g. *.ts, src/**/include", inf), empty(), inf),
        caption("files to exclude"),
        boxed(ui, input(ui, st.exclude, "e.g. *.ts, src/**/exclude", exf), empty(), exf),
    ))
    .style(move |s| s.margin_left(26.0).margin_right(12.0).apply_if(!st.details.get(), |s| s.hide()));
    let said = create_memo_of(ui, move || {
        if st.waiting.with(Option::is_some) && ui.search.with(Option::is_none) {
            return ("Searching…".to_string(), String::new());
        }
        ui.search.with(|r| match r {
            None => (String::new(), String::new()),
            Some(r) if !r.error.is_empty() => (r.error.clone(), String::new()),
            Some(r) if r.hits.is_empty() => ("No results found.".to_string(), String::new()),
            Some(r) => {
                let more = if r.more {
                    "The result set only contains a subset of all matches. Be more specific in your search to narrow down the results."
                } else {
                    ""
                };
                let searching = if st.waiting.with(Option::is_some) { " (searching…)" } else { "" };
                (format!("{}{searching}", summary(r.hits.len(), by_file(&r.hits).len())), more.to_string())
            }
        })
    });
    let summary_line = v_stack((
        label(move || said.get().0),
        label(move || said.get().1).style(move |s| s.apply_if(said.with(|s| s.1.is_empty()), |s| s.hide())),
    ))
    .style(move |s| {
        s.padding_left(22.0)
            .padding_right(12.0)
            .padding_vert(6.0)
            .gap(4.0)
            .font_size(13.0)
            .color(t.get().desc)
            .apply_if(said.with(|s| s.0.is_empty()), |s| s.hide())
    });
    let numbers = crate::settings::now(ui, "search.showLineNumbers", None).as_bool().unwrap_or(true);
    let items = move || {
        let g = st.generation.get();
        st.folded.with(|f| {
            ui.search.with(|r| {
                r.as_ref()
                    .map(|r| rows(&r.hits, f))
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(i, row)| (g, i, row))
                    .collect::<im::Vector<_>>()
            })
        })
    };
    let results = virtual_stack(
        VirtualDirection::Vertical,
        VirtualItemSize::Fixed(Box::new(|| 22.0)),
        items,
        |(g, i, row): &(u64, usize, Row)| (*g, *i, matches!(row, Row::File { folded: true, .. })),
        move |(_, _, row): (u64, usize, Row)| match row {
            Row::File { path, count, folded } => file_row(ui, send.clone(), path, count, folded).into_any(),
            Row::Match(h) => match_row(ui, send.clone(), h, numbers).into_any(),
        },
    )
    .style(|s| s.flex_col().width_full());
    v_stack((
        top,
        toggle_details,
        details,
        summary_line,
        scroll(results).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full()),
    ))
    .style(move |s| s.size_full().min_height(0.0).color(t.get().side_fg))
}

/// A memo made in the window's scope.
fn create_memo_of<T: PartialEq + Clone + 'static>(ui: Ui, f: impl Fn() -> T + 'static) -> floem::reactive::Memo<T> {
    ui.scope.create_memo(move |_| f())
}

fn file_row(ui: Ui, send: Send, path: String, count: usize, folded: bool) -> impl IntoView {
    let t = ui.colors;
    let st = state(ui);
    let (dir, name) = match path.rsplit_once('/') {
        Some((d, n)) => (d.to_string(), n.to_string()),
        None => (String::new(), path.clone()),
    };
    let hovered = ui.scope.create_rw_signal(false);
    let (fold, replaced) = (path.clone(), path.clone());
    h_stack((
        icon(if folded { "chevron-right" } else { "chevron-down" }, 16.0),
        label(move || name.clone()).style(|s| s.flex_shrink(0.0_f32)),
        label(move || dir.clone()).style(move |s| s.font_size(12.0).min_width(0.0).color(t.get().desc).text_ellipsis()),
        empty().style(|s| s.flex_grow(1.0_f32)),
        container(icon_button(ui, "replace-all", "Replace All", move || replace_some(ui, &send, &replaced, None)))
            .style(move |s| s.apply_if(!(hovered.get() && st.replacing.get()), |s| s.hide())),
        label(move || count.to_string()).style(move |s| {
            let c = t.get();
            s.font_size(11.0)
                .padding_horiz(5.0)
                .min_width(18.0)
                .height(16.0)
                .justify_center()
                .items_center()
                .border_radius(8.0)
                .background(c.badge_bg)
                .color(c.badge_fg)
        }),
    ))
    .on_event_cont(floem::event::EventListener::PointerEnter, move |_| hovered.set(true))
    .on_event_cont(floem::event::EventListener::PointerLeave, move |_| hovered.set(false))
    .on_click_stop(move |_| {
        st.folded.update(|f| {
            if !f.remove(&fold) {
                f.insert(fold.clone());
            }
        })
    })
    .style(move |s| {
        let c = t.get();
        s.height(22.0)
            .width_full()
            .items_center()
            .gap(6.0)
            .padding_left(8.0)
            .padding_right(12.0)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.list_hover))
    })
}

fn match_row(ui: Ui, send: Send, h: Hit, numbers: bool) -> impl IntoView {
    let t = ui.colors;
    let st = state(ui);
    let (before, lit, after) = cut(&h.text, h.at, h.len);
    let hovered = ui.scope.create_rw_signal(false);
    let line = h.line;
    let (opened, replaced) = (h.clone(), h.clone());
    let replace_send = send.clone();
    h_stack((
        label(move || line.to_string()).style(move |s| {
            s.font_size(12.0)
                .min_width(28.0)
                .padding_right(6.0)
                .color(t.get().line_number)
                .apply_if(!numbers, |s| s.hide())
        }),
        label(move || before.clone()).style(|s| s.flex_shrink(0.0_f32)),
        label(move || lit.clone()).style(move |s| {
            let c = t.get();
            let s = bold(s).flex_shrink(0.0_f32);
            if st.replacing.get() { s.background(c.diff_removed) } else { s.background(c.find_match_highlight) }
        }),
        label(move || st.replacement.get()).style(move |s| {
            let c = t.get();
            bold(s).flex_shrink(0.0_f32).background(c.diff_added).apply_if(!st.replacing.get(), |s| s.hide())
        }),
        label(move || after.clone()).style(|s| s.min_width(0.0).text_ellipsis()),
        empty().style(|s| s.flex_grow(1.0_f32)),
        container(icon_button(ui, "replace", "Replace", move || {
            replace_some(ui, &replace_send, &replaced.path, Some(&replaced))
        }))
        .style(move |s| s.flex_shrink(0.0_f32).apply_if(!(hovered.get() && st.replacing.get()), |s| s.hide())),
    ))
    .on_event_cont(floem::event::EventListener::PointerEnter, move |_| hovered.set(true))
    .on_event_cont(floem::event::EventListener::PointerLeave, move |_| hovered.set(false))
    .on_click_stop(move |_| open_match(ui, &send, &opened))
    .style(move |s| {
        let c = t.get();
        s.height(22.0)
            .width_full()
            .items_center()
            .padding_left(if numbers { 24.0 } else { 38.0 })
            .padding_right(12.0)
            .font_size(13.0)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.list_hover))
    })
}

/// The view shown with the keys in its query box (all of it selected), or in its replace box.
fn focus(ui: Ui, replace: bool) {
    ui.reveal(View::Search);
    let st = state(ui);
    if replace {
        st.replacing.set(true);
    }
    floem::action::exec_after(std::time::Duration::from_millis(30), move |_| {
        let id = if replace { st.replace_box.get_untracked() } else { st.query_box.get_untracked() };
        if let Some(id) = id {
            id.request_focus();
        }
    });
}

/// Whether a path the Explorer shows is a folder.
fn is_dir(ui: Ui, path: &str) -> bool {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    ui.tree.with_untracked(|t| t.get(parent).is_some_and(|es| es.iter().any(|e| e.dir && e.name == name)))
}

/// What it adds to the window's registries, once at start.
pub fn register(ui: Ui, send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{Menu, item};
    let st = state(ui);
    add(command(crate::workbench::SHOW_SEARCH, "Search", "Find in Files", |ui, _| focus(ui, false))
        .keys("Ctrl+Shift+F"));
    add(command(REPLACE_IN_FILES, "Search", "Replace in Files", |ui, _| focus(ui, true)).keys("Ctrl+Shift+H"));
    add(command(FIND_IN_FOLDER, "Search", "Find in Folder...", move |ui, _| {
        let folder = ui.selected.get_untracked();
        st.include.set(if folder.is_empty() { String::new() } else { format!("./{folder}") });
        st.details.set(true);
        focus(ui, false);
    })
    .when(move |ui| is_dir(ui, &ui.selected.get_untracked())));
    add(command(COLLAPSE, "Search", "Collapse All", move |ui, _| {
        let paths: HashSet<String> = ui.search.with_untracked(|r| {
            r.as_ref().map(|r| by_file(&r.hits).into_iter().map(|(p, _)| p).collect()).unwrap_or_default()
        });
        st.folded.set(paths);
    }));
    add(command(EXPAND, "Search", "Expand All", move |_, _| st.folded.set(HashSet::new())));
    add(command(REFRESH, "Search", "Refresh", search_now));
    add(command(CLEAR, "Search", "Clear Search Results", move |ui, _| {
        ui.search_query.set(String::new());
        st.waiting.set(None);
        ui.search.set(None);
    }));
    let title = |id: &str, icon: &'static str, order: i32| item(id).icon(icon).group("navigation").order(order);
    crate::menus::add(Menu::ViewTitle(View::Search), title(REFRESH, "refresh", 1));
    crate::menus::add(Menu::ViewTitle(View::Search), title(CLEAR, "clear-all", 2));
    crate::menus::add(Menu::ViewTitle(View::Search), title(COLLAPSE, "collapse-all", 3));
    crate::menus::add(Menu::ExplorerRow, item(FIND_IN_FOLDER).group("4_search"));
    // A match waiting to be selected once its file comes on screen.
    ui.scope.create_effect(move |_| {
        ui.live.track();
        if st.select.with_untracked(Option::is_some) {
            select_soon(ui);
        }
    });
    let _ = send;
}

/// What is kept with the folder's view state (`Ui::view_state`).
pub fn kept(ui: Ui) -> Value {
    let st = state(ui);
    let a = asked_now(ui);
    json!({
        "query": a.query, "case": a.case, "regex": a.regex, "word": a.word,
        "include": a.include, "exclude": a.exclude,
        "replace": st.replacement.get_untracked(),
        "replacing": st.replacing.get_untracked(),
        "details": st.details.get_untracked(),
    })
}

fn restore(ui: Ui, v: &Value, send: &Send) {
    let st = state(ui);
    if st.restored.get_untracked() || !v.is_object() {
        return;
    }
    st.restored.set(true);
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    st.word.set(v["word"] == true);
    st.include.set(s("include"));
    st.exclude.set(s("exclude"));
    st.replacement.set(s("replace"));
    st.replacing.set(v["replacing"] == true);
    st.details.set(v["details"] == true || !s("include").is_empty() || !s("exclude").is_empty());
    ui.search_case.set(v["case"] == true);
    ui.search_regex.set(v["regex"] == true);
    ui.search_query.set(s("query"));
    search_now(ui, send);
}

/// The session's messages that are the Search view's, taken in. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "state" => restore(ui, &m["view_state"]["search"], send),
        "search" => {
            let asked = Asked::of(m);
            if st.waiting.with_untracked(|w| w.as_ref() != Some(&asked)) {
                return false; // an answer to what was asked before
            }
            st.waiting.set(None);
            let n = |h: &Value, k: &str| h[k].as_u64().unwrap_or(0) as usize;
            let hits = m["hits"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|h| Hit {
                    path: h["path"].as_str().unwrap_or("").into(),
                    line: h["line"].as_u64().unwrap_or(1) as usize,
                    text: h["text"].as_str().unwrap_or("").into(),
                    col: n(h, "col"),
                    len: n(h, "len"),
                    at: n(h, "at"),
                })
                .collect();
            let error = m["error"].as_str().unwrap_or("").to_string();
            st.shown.set(Some(asked));
            st.generation.update(|g| *g += 1);
            ui.search.set(Some(Found { hits, more: m["more"] == true, error }));
        }
        "replaced" => {
            if st.replaced.with_untracked(|r| r.as_ref() != Some(&Asked::of(m))) {
                return false;
            }
            st.replaced.set(None);
            for e in m["errors"].as_array().into_iter().flatten() {
                let (path, error) = (e["path"].as_str().unwrap_or(""), e["error"].as_str().unwrap_or(""));
                toast::error(
                    ui,
                    &if path.is_empty() { error.to_string() } else { format!("{path} could not be changed: {error}") },
                );
            }
            // The results again, from the files as they are now.
            if let Some(asked) = st.shown.get_untracked() {
                send(asked.message("search"));
                st.waiting.set(Some(asked));
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(p: &str, line: usize) -> Hit {
        Hit { path: p.into(), line, text: String::new(), col: 0, len: 0, at: 0 }
    }

    #[test]
    fn hits_by_file_in_the_order_found() {
        let g = by_file(&[hit("a.rs", 1), hit("a.rs", 4), hit("b/c.rs", 2)]);
        assert_eq!(g.iter().map(|(p, v)| (p.as_str(), v.len())).collect::<Vec<_>>(), [("a.rs", 2), ("b/c.rs", 1)]);
    }

    #[test]
    fn rows_are_each_file_then_its_matches_unless_folded() {
        let hits = [hit("a.rs", 1), hit("a.rs", 4), hit("b/c.rs", 2)];
        let kinds = |f: &HashSet<String>| {
            rows(&hits, f)
                .iter()
                .map(|r| match r {
                    Row::File { path, count, folded } => format!("{path}({count}{})", if *folded { "+" } else { "" }),
                    Row::Match(h) => h.line.to_string(),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(kinds(&HashSet::new()), ["a.rs(2)", "1", "4", "b/c.rs(1)", "2"]);
        assert_eq!(kinds(&["a.rs".to_string()].into()), ["a.rs(2+)", "b/c.rs(1)", "2"]);
    }

    #[test]
    fn a_line_cut_around_its_match_a_long_start_shortened() {
        assert_eq!(cut("    let Cart = 1;", 8, 4), ("let ".into(), "Cart".into(), " = 1;".into()));
        let long = format!("{}needle end", "x".repeat(40));
        let (before, lit, after) = cut(&long, 40, 6);
        assert_eq!(
            (before.chars().count(), before.starts_with('…'), lit.as_str(), after.as_str()),
            (27, true, "needle", " end")
        );
        // Past its end (a line changed meanwhile): the text as it is.
        assert_eq!(cut("ab", 5, 2), ("ab".into(), String::new(), String::new()));
        assert_eq!(cut("é", 1, 1).1, "", "not on a char's edge");
    }

    #[test]
    fn the_count_said_as_vs_code_says_it() {
        assert_eq!(summary(1, 1), "1 result in 1 file");
        assert_eq!(summary(3, 2), "3 results in 2 files");
    }

    #[test]
    fn a_reply_is_known_by_all_it_was_asked_with() {
        let a = Asked { query: "q".into(), word: true, include: "*.rs".into(), ..Asked::default() };
        assert_eq!(Asked::of(&a.message("search")), a);
        let other = json!({"type": "search", "query": "q", "word": false, "include": "*.rs"});
        assert_ne!(Asked::of(&other), a);
    }
}
