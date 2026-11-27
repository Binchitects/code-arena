//! The input's @ mentions, as VS Code's chat suggests files and folders after `@`.
//!
//! An `@` typed at the start of the box or after a space opens a list over the caret: the project's files and the
//! folders they are in, ranked as Quick Open ranks them for what follows the `@`, fifty at a time ("Show N more",
//! or ↓ past the last row), "N of M matches" over them and a note when the project has more files than were
//! listed. ↑ and ↓ choose, Enter or Tab puts `@path ` in place of what was typed (`@"a b.txt" ` for a name with
//! spaces), Escape closes the list. A range of lines typed after the name (`@cart:12-30`) is kept with the path
//! chosen, and the lines are what is sent. A whole path (`/`, `~/`, `C:/`) lists that folder on this machine: a
//! folder chosen goes in, a file is put in the box whole. Only what is typed opens the list: text put in the box
//! by the rest of the window does not.
//!
//! Under a question, a chip for each file, folder or range of lines its mentions sent with it, as the session
//! says (`attached`); a chip opens its file.
//!
//! The session's messages: `files` (the project's, as Quick Open takes them), `folders{path, key}` →
//! `folders{path, dir, parent, entries, more, error, key}`, and `attached{q, labels}` after a `user`.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use floem::action::exec_after;
use floem::event::EventListener;
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::Rect;
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::{CursorStyle, FlexWrap, Position};
use floem::views::{Decorators, clip, dyn_stack, empty, h_stack, label, rich_text, scroll, v_stack};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use super::slots::{ItemSlot, add_item};
use crate::icons::icon;
use crate::layers::{self, Place};
use crate::model::Kind;
use crate::palette::{Files, Query, Row, layout_of, path_row, positions};
use crate::state::{Send, Ui};

/// The rows listed at a time, and a row's height, as VS Code's suggest widget has them.
const PAGE: usize = 50;
const ROW: f64 = 22.0;
/// The rows shown before the list scrolls, and the list's width.
const SHOWN: usize = 12;
const WIDTH: f64 = 430.0;

/// A mention being typed: where its `@` is in the box's text, where what it names ends (in bytes), what follows
/// the `@` up to the caret (without a quote it begins with), and whether it begins with one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Typed {
    pub at: usize,
    pub end: usize,
    pub query: String,
    pub quoted: bool,
}

/// What a mention ends at, as the session reads one (`ca_agent::mentions::find`).
fn ends(c: char) -> bool {
    c.is_whitespace() || matches!(c, ',' | ')' | ']' | '`' | '"' | '\'')
}

/// The mention the caret is in, when it is in one: an `@` at the start or after a space, `(` or `[`, and no end
/// between it and the caret (in quotes, no closing quote). `me@example.com` is no mention.
pub fn typed_at(text: &str, caret: usize) -> Option<Typed> {
    if caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let before = &text[..caret];
    let at = before.rfind('@')?;
    let starts = before[..at].chars().next_back().is_none_or(|c| c.is_whitespace() || matches!(c, '(' | '['));
    if !starts {
        return None;
    }
    let rest = &before[at + 1..];
    if let Some(inside) = rest.strip_prefix('"') {
        return (!inside.contains(['"', '\n'])).then(|| Typed { at, end: caret, query: inside.into(), quoted: true });
    }
    if rest.chars().any(ends) {
        return None;
    }
    // What names it goes on after the caret to its end.
    let end = caret + text[caret..].find(ends).unwrap_or(text.len() - caret);
    Some(Typed { at, end, query: rest.into(), quoted: false })
}

/// What was typed as the path looked for and a range of lines after it (`:12-30`, `:12`, a `:` begun), which is
/// kept with the path chosen.
pub fn lines_of(query: &str) -> (&str, &str) {
    match query.rfind(':') {
        Some(i) if i > 0 && query[i + 1..].chars().all(|c| c.is_ascii_digit() || c == '-') => {
            (&query[..i], &query[i..])
        }
        _ => (query, ""),
    }
}

/// A whole path typed (`/`, `~/`, `C:/`): the folder it names as typed, its last `/` with it, and the name begun
/// in it.
pub fn whole_path(query: &str) -> Option<(&str, &str)> {
    let b = query.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\');
    if !(query.starts_with('/') || query.starts_with("~/") || drive) {
        return None;
    }
    let cut = query.rfind(['/', '\\'])? + 1;
    Some((&query[..cut], &query[cut..]))
}

/// A mention as it is put in the box: `@path ` and its lines, in quotes when the path has a space (or what else
/// ends a mention). `done: false`: a folder gone into, its name left open for what is typed next.
pub fn mention(path: &str, lines: &str, done: bool) -> String {
    let quoted = path.contains(ends);
    match (quoted, done) {
        (true, true) => format!("@\"{path}{lines}\" "),
        (true, false) => format!("@\"{path}"),
        (false, true) => format!("@{path}{lines} "),
        (false, false) => format!("@{path}"),
    }
}

/// What the list offers for the project: its files, then the folders they are in (each once, without the `/`).
pub struct Pool {
    pub all: Files,
    /// How many of `all` are files: the rest are folders.
    pub files: usize,
}

impl Pool {
    pub fn of(listed: &Files) -> Pool {
        let n = listed.len();
        let (mut seen, mut folders) = (HashSet::new(), Vec::new());
        for i in 0..n {
            let path = listed.path(i);
            for (j, _) in path.match_indices('/') {
                if seen.insert(&path[..j]) {
                    folders.push(&path[..j]);
                }
            }
        }
        let all = Files::of((0..n).map(|i| listed.path(i)).chain(folders), listed.more);
        Pool { all, files: n }
    }
}

/// What choosing a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// The path put in the box (a folder's too), the list closed.
    Put(String),
    /// A folder on this machine gone into: its path as it is typed, the list kept open on it.
    Into(String),
    /// Fifty more rows.
    More,
}

/// A row of the list: what it shows, whether it is a folder, and what choosing it does.
#[derive(Clone)]
pub struct Entry {
    pub row: Row,
    pub folder: bool,
    pub choice: Choice,
}

/// The rows for the project: the best `pages` fifties for what was typed, "Show N more" after them while there
/// are more; and how many match.
pub fn project_rows(pool: &Pool, path: &str, pages: usize) -> (Vec<Entry>, usize) {
    let q = Query::of(path);
    let total = pool.all.count(&q);
    let shown = pages.max(1) * PAGE;
    let mut rows: Vec<Entry> = pool
        .all
        .rank(&q, shown)
        .into_iter()
        .map(|i| {
            let path = pool.all.path(i);
            Entry { row: path_row(path, &q), folder: i >= pool.files, choice: Choice::Put(path.to_string()) }
        })
        .collect();
    if total > shown {
        rows.push(more_row((total - shown).min(PAGE)));
    }
    (rows, total)
}

/// "Show N more".
fn more_row(n: usize) -> Entry {
    Entry { row: Row::new(&format!("Show {n} more"), "", Vec::new(), "", crate::palette::Pick::File(String::new())), folder: false, choice: Choice::More }
}

/// A folder on this machine as the session listed it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    /// As it was asked for (typed), and as a whole path.
    pub path: String,
    pub dir: String,
    /// Its entries: a name, and whether it is a folder.
    pub entries: Vec<(String, bool)>,
    pub more: bool,
    pub error: Option<String>,
}

impl Listing {
    pub fn from_json(m: &Value) -> Listing {
        let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
        Listing {
            path: s("path"),
            dir: s("dir"),
            entries: m["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| Some((e["name"].as_str()?.to_string(), e["dir"] == true)))
                .collect(),
            more: m["more"] == true,
            error: m["error"].as_str().map(str::to_string),
        }
    }
}

/// The rows for a folder on this machine: its entries that match the name begun, the folders first as listed.
pub fn folder_rows(listing: &Listing, begun: &str, pages: usize) -> (Vec<Entry>, usize) {
    let names = Files::of(listing.entries.iter().map(|(n, _)| n.as_str()), false);
    let q = Query::of(begun);
    let total = names.count(&q);
    let shown = pages.max(1) * PAGE;
    // The folders first, then the files, each as well as it matches.
    let mut found = names.rank(&q, shown);
    found.sort_by_key(|&i| !listing.entries[i].1);
    let dir = listing.dir.trim_end_matches('/');
    let mut rows: Vec<Entry> = found
        .into_iter()
        .map(|i| {
            let (name, folder) = &listing.entries[i];
            let choice = if *folder {
                Choice::Into(format!("{}{name}/", listing.path))
            } else {
                Choice::Put(format!("{dir}/{name}"))
            };
            let row = Row::new(name, "", positions(name, &q), "", crate::palette::Pick::File(String::new()));
            Entry { row, folder: *folder, choice }
        })
        .collect();
    if total > shown {
        rows.push(more_row((total - shown).min(PAGE)));
    }
    (rows, total)
}

/// "N of M matches" over the rows (empty: none to say).
pub fn count_line(shown: usize, total: usize) -> String {
    match total {
        0 => String::new(),
        1 => "1 match".into(),
        _ if shown >= total => format!("{total} matches"),
        _ => format!("{shown} of {total} matches"),
    }
}

/// The part's own state.
#[derive(Clone, Copy)]
pub struct Mentions {
    /// The mention the list is open for (none: closed).
    open: RwSignal<Option<Typed>>,
    /// Where the caret was in the window when it was last typed at.
    caret: RwSignal<Rect>,
    rows: RwSignal<Rc<Vec<Entry>>>,
    /// How many match, and what is said under the rows (a note, an error, "No matching files").
    total: RwSignal<usize>,
    note: RwSignal<String>,
    /// The row chosen, and the fifties shown.
    at: RwSignal<usize>,
    pages: RwSignal<usize>,
    /// The project's files with their folders, and the list they were made from.
    pool: RwSignal<Option<(Rc<Files>, Rc<Pool>)>>,
    /// The folder on this machine listed last, and the one asked for with its key.
    listing: RwSignal<Option<Rc<Listing>>>,
    asked: RwSignal<(String, String)>,
    next_key: RwSignal<u64>,
    /// What each question's mentions sent with it, by its number.
    attached: RwSignal<HashMap<u64, Rc<Vec<String>>>>,
}

impl crate::ext::State for Mentions {
    fn new(ui: Ui) -> Mentions {
        let s = ui.scope;
        Mentions {
            open: s.create_rw_signal(None),
            caret: s.create_rw_signal(Rect::ZERO),
            rows: s.create_rw_signal(Rc::new(Vec::new())),
            total: s.create_rw_signal(0),
            note: s.create_rw_signal(String::new()),
            at: s.create_rw_signal(0),
            pages: s.create_rw_signal(1),
            pool: s.create_rw_signal(None),
            listing: s.create_rw_signal(None),
            asked: s.create_rw_signal((String::new(), String::new())),
            next_key: s.create_rw_signal(1),
            attached: s.create_rw_signal(HashMap::new()),
        }
    }
}

/// The project's files with their folders, made again only when the files were listed again.
fn pool(ui: Ui) -> Rc<Pool> {
    let me = ui.state::<Mentions>();
    let listed = ui.quick_files.get_untracked();
    if let Some((from, pool)) = me.pool.get_untracked()
        && Rc::ptr_eq(&from, &listed)
    {
        return pool;
    }
    let made = Rc::new(Pool::of(&listed));
    me.pool.set(Some((listed, made.clone())));
    made
}

/// The rows made again for the mention the list is open for.
fn refresh(ui: Ui, send: &Send) {
    let me = ui.state::<Mentions>();
    let Some(typed) = me.open.get_untracked() else { return };
    let pages = me.pages.get_untracked();
    let (path, _) = lines_of(&typed.query);
    let (rows, total, note) = match whole_path(path) {
        Some((dir, begun)) => match me.listing.get_untracked().filter(|l| l.path == dir) {
            Some(listing) => {
                let (rows, total) = folder_rows(&listing, begun, pages);
                let note = match (&listing.error, listing.more) {
                    (Some(e), _) => e.clone(),
                    (None, true) => "The folder has more entries than are listed.".into(),
                    (None, false) if rows.is_empty() => "No matching files".into(),
                    _ => String::new(),
                };
                (rows, total, note)
            }
            None => {
                if me.asked.with_untracked(|(asked, _)| asked != dir) {
                    let key = format!("mention-{}", me.next_key.get_untracked());
                    me.next_key.update(|n| *n += 1);
                    me.asked.set((dir.to_string(), key.clone()));
                    send(json!({"type": "folders", "path": dir, "key": key}));
                }
                (Vec::new(), 0, "Listing the folder…".into())
            }
        },
        None => {
            let pool = pool(ui);
            let (rows, total) = project_rows(&pool, path, pages);
            let note = if !ui.quick_files.with_untracked(|f| f.listed) {
                "Listing the project's files…".into()
            } else if rows.is_empty() {
                "No matching files".into()
            } else if pool.all.more {
                "The project has more files than were listed: type more of a path to find the others.".into()
            } else {
                String::new()
            };
            (rows, total, note)
        }
    };
    if me.at.get_untracked() >= rows.len() {
        me.at.set(0);
    }
    me.rows.set(Rc::new(rows));
    me.total.set(total);
    if me.note.with_untracked(|n| *n != note) {
        me.note.set(note);
    }
}

fn close(ui: Ui) {
    let me = ui.state::<Mentions>();
    if me.open.with_untracked(Option::is_some) {
        me.open.set(None);
        me.rows.set(Rc::new(Vec::new()));
    }
}

/// After a key typed in the box: the list opened, kept or closed for the mention the caret is now in.
fn follow(ui: Ui, send: &Send) {
    let me = ui.state::<Mentions>();
    let found = crate::chat_bridge::caret(ui)
        .and_then(|c| ui.draft.with_untracked(|text| typed_at(text, c.offset)).map(|t| (t, c.rect)));
    let Some((typed, rect)) = found else {
        close(ui);
        return;
    };
    let was = me.open.get_untracked();
    if was.is_none() {
        // Listed again each time the list opens (files come and go); the list had before is ranked meanwhile.
        send(json!({"type": "files"}));
    }
    if was.as_ref().map(|w| &w.query) != Some(&typed.query) {
        me.at.set(0);
        me.pages.set(1);
    }
    me.caret.set(rect);
    if was.as_ref() != Some(&typed) {
        me.open.set(Some(typed));
    }
    refresh(ui, send);
}

/// A row chosen.
fn choose(ui: Ui, send: &Send, n: usize) {
    let me = ui.state::<Mentions>();
    let (Some(typed), Some(entry)) = (me.open.get_untracked(), me.rows.with_untracked(|r| r.get(n).cloned())) else {
        return;
    };
    let (_, lines) = lines_of(&typed.query);
    match entry.choice {
        Choice::More => {
            me.pages.update(|p| *p += 1);
            refresh(ui, send);
        }
        Choice::Put(path) => {
            close(ui);
            crate::chat_bridge::replace(ui, typed.at, typed.end, &mention(&path, lines, true));
        }
        Choice::Into(path) => {
            crate::chat_bridge::replace(ui, typed.at, typed.end, &mention(&path, "", false));
            follow(ui, send);
        }
    }
    crate::chat_bridge::focus(ui);
}

/// ↑ or ↓ in the list: ↓ past the last row shows fifty more when there are, else goes round.
fn step(ui: Ui, send: &Send, by: isize) {
    let me = ui.state::<Mentions>();
    let n = me.rows.with_untracked(|r| r.len());
    if n == 0 {
        return;
    }
    let at = me.at.get_untracked();
    let last_is_more = me.rows.with_untracked(|r| r.last().is_some_and(|e| e.choice == Choice::More));
    if by > 0 && last_is_more && at + 2 >= n {
        me.pages.update(|p| *p += 1);
        refresh(ui, send);
        me.at.set((at + 1).min(me.rows.with_untracked(|r| r.len()).saturating_sub(1)));
        return;
    }
    me.at.set((at as isize + by).rem_euclid(n as isize) as usize);
}

/// A key pressed in the box: the list's when it is open (↑, ↓, Enter, Tab, Escape); any other is the box's, and
/// once it has acted the list follows what was typed.
fn key(ui: Ui, send: &Send, key: &Key, mods: Modifiers) -> bool {
    let me = ui.state::<Mentions>();
    let open = me.open.with_untracked(Option::is_some) && !me.rows.with_untracked(|r| r.is_empty());
    let plain = !mods.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::META | Modifiers::SHIFT);
    if plain && me.open.with_untracked(Option::is_some) && *key == Key::Named(NamedKey::Escape) {
        close(ui);
        return true;
    }
    if open && plain {
        match key {
            Key::Named(NamedKey::ArrowDown) => {
                step(ui, send, 1);
                return true;
            }
            Key::Named(NamedKey::ArrowUp) => {
                step(ui, send, -1);
                return true;
            }
            Key::Named(NamedKey::Enter | NamedKey::Tab) => {
                choose(ui, send, me.at.get_untracked());
                return true;
            }
            _ => {}
        }
    }
    // Only what is typed opens it: a letter, or a key that edits or moves through the text.
    let edits = matches!(key, Key::Character(_))
        || matches!(
            key,
            Key::Named(
                NamedKey::Backspace
                    | NamedKey::Delete
                    | NamedKey::ArrowLeft
                    | NamedKey::ArrowRight
                    | NamedKey::Home
                    | NamedKey::End
                    | NamedKey::Space
            )
        );
    let ctrl = mods.intersects(Modifiers::CONTROL | Modifiers::META);
    if (edits && !ctrl) || me.open.with_untracked(Option::is_some) {
        let send = send.clone();
        exec_after(Duration::ZERO, move |_| follow(ui, &send));
    }
    false
}

/// A row: its icon, its name and folder with the letters typed lit; chosen on the press (the box keeps the keys).
fn row_view(ui: Ui, send: Send, n: usize, entry: Entry) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Mentions>();
    let selected = create_memo(move |_| me.at.get() == n);
    let more = entry.choice == Choice::More;
    let name = if more { "" } else if entry.folder { "folder" } else { "file" };
    let row = entry.row.clone();
    h_stack((
        icon(name, 16.0).style(move |s| s.flex_shrink(0.0_f32).apply_if(more, |s| s.hide())),
        clip(rich_text(move || layout_of(&row, &t.get(), selected.get())).style(|s| s.flex_shrink(0.0_f32)))
            .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).height(ROW)),
    ))
    .on_event_stop(EventListener::PointerDown, move |_| choose(ui, &send, n))
    .on_event_cont(EventListener::PointerEnter, move |_| {
        if me.at.get_untracked() != n {
            me.at.set(n);
        }
    })
    .style(move |s| {
        let c = t.get();
        let s = s
            .height(ROW)
            .width_full()
            .min_width(0.0)
            .items_center()
            .gap(6.0)
            .padding_horiz(6.0)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .color(c.quick_fg);
        if selected.get() { s.background(c.quick_focus_bg).color(c.quick_focus_fg) } else { s }
    })
}

/// The list over the caret (under it when there is no room over it).
fn menu(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Mentions>();
    {
        // The files or a folder came while the list is open: its rows made again.
        let send = send.clone();
        create_effect(move |_| {
            ui.quick_files.track();
            me.listing.track();
            if me.open.with_untracked(Option::is_some) {
                refresh(ui, &send);
            }
        });
    }
    let shown = create_memo(move |_| me.rows.with(|r| r.iter().filter(|e| e.choice != Choice::More).count()));
    let count = label(move || count_line(shown.get(), me.total.get())).style(move |s| {
        s.font_size(11.0)
            .height(ROW)
            .items_center()
            .padding_horiz(8.0)
            .color(t.get().desc)
            .apply_if(me.total.get() == 0, |s| s.hide())
    });
    let rows = dyn_stack(
        move || me.rows.get().iter().cloned().enumerate().collect::<Vec<_>>(),
        |(n, e): &(usize, Entry)| (*n, e.row.key),
        move |(n, e): (usize, Entry)| row_view(ui, send.clone(), n, e),
    )
    .style(|s| s.flex_col().width_full());
    let list = scroll(rows)
        .ensure_visible(move || {
            let y = me.at.get() as f64 * ROW;
            Rect::new(0.0, y, 1.0, y + ROW)
        })
        .scroll_style(|s| s.hide_bars(true))
        .style(move |s| {
            let n = me.rows.with(|r| r.len());
            s.width_full().height(n.min(SHOWN) as f64 * ROW).apply_if(n == 0, |s| s.hide())
        });
    let note = label(move || me.note.get()).style(move |s| {
        s.font_size(11.0)
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .color(t.get().desc)
            .text_ellipsis()
            .max_width_full()
            .apply_if(me.note.with(String::is_empty), |s| s.hide())
    });
    v_stack((count, list, note)).style(move |s| {
        let c = t.get();
        let n = me.rows.with(|r| r.len()).min(SHOWN);
        let height = n as f64 * ROW
            + if me.total.get() > 0 { ROW } else { 0.0 }
            + if me.note.with(String::is_empty) { 0.0 } else { ROW + 2.0 }
            + 10.0;
        let caret = me.caret.get();
        let top = if caret.y0 - height - 4.0 >= 0.0 { caret.y0 - height - 4.0 } else { caret.y1 + 4.0 };
        s.position(Position::Absolute)
            .inset_left((caret.x0 - 8.0).max(4.0))
            .inset_top(top)
            .width(WIDTH)
            .padding_vert(4.0)
            .border(1.0)
            .border_color(c.menu_border)
            .border_radius(6.0)
            .background(c.quick_bg)
            .box_shadow_blur(8.0)
            .box_shadow_color(c.shadow)
            .font_size(13.0)
            .z_index(100)
            .apply_if(me.open.with(Option::is_none), |s| s.hide())
    })
}

/// A chip's file and line, from its label (`src/a.rs:12-30`): none for a folder.
fn chip_target(label: &str) -> Option<(String, usize)> {
    if label.ends_with('/') {
        return None;
    }
    let (path, lines) = lines_of(label);
    let first = lines.trim_start_matches(':').split('-').next().and_then(|n| n.parse().ok()).unwrap_or(0);
    Some((path.to_string(), first))
}

/// The chips under a question of what its mentions sent with it.
fn chips(ui: Ui, send: Send, q: u64) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Mentions>();
    let labels = create_memo(move |_| me.attached.with(|a| a.get(&q).cloned().unwrap_or_default()));
    let list = dyn_stack(
        move || labels.get().iter().cloned().enumerate().collect::<Vec<_>>(),
        |(n, l): &(usize, String)| (*n, l.clone()),
        move |(_, l): (usize, String)| {
            let target = chip_target(&l);
            let folder = target.is_none();
            let send = send.clone();
            h_stack((
                icon(if folder { "folder" } else { "file" }, 14.0),
                label(move || l.clone()).style(|s| s.font_size(12.0).text_ellipsis()),
            ))
            .on_click_stop(move |_| match &target {
                Some((path, line)) if *line > 0 => ui.open_at(path, *line, &send),
                Some((path, _)) => ui.open(path, &send),
                None => {}
            })
            .style(move |s| {
                let c = t.get();
                s.gap(4.0)
                    .items_center()
                    .padding_horiz(6.0)
                    .padding_vert(1.0)
                    .border(1.0)
                    .border_color(c.widget_border)
                    .border_radius(4.0)
                    .color(c.link)
                    .max_width_full()
                    .apply_if(!folder, |s| s.cursor(CursorStyle::Pointer).hover(|s| s.background(c.list_hover)))
            })
        },
    )
    .style(|s| s.flex_row().flex_wrap(FlexWrap::Wrap).gap(4.0).justify_end());
    v_stack((label(|| "Attached").style(move |s| s.font_size(11.0).color(t.get().desc)), list)).style(move |s| {
        s.gap(2.0).items_end().margin_top(4.0).max_width_pct(85.0).apply_if(labels.with(|l| l.is_empty()), |s| s.hide())
    })
}

/// What it adds to the window's registries, once at start: the list's keys and its layer, and the chips under a
/// question.
pub fn register(_ui: Ui, _send: &Send) {
    crate::chat_bridge::on_key("mentions", 20, key);
    layers::add(Place::Over, "mentions", 45, |ui, send| menu(ui, send).into_any());
    add_item(ItemSlot::QuestionFoot, "mentions.attached", 15, |ui, send, item| match &item.kind {
        Kind::User(q) if !q.note && q.q > 0 => chips(ui, send, q.q).into_any(),
        _ => empty().style(|s| s.hide()).into_any(),
    });
}

/// The session's messages that are this part's, taken in: a folder listed for the list, and what a question's
/// mentions sent. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let me = ui.state::<Mentions>();
    match m["type"].as_str().unwrap_or("") {
        "folders" => {
            let asked = me.asked.with_untracked(|(path, key)| m["path"] == path.as_str() && m["key"] == key.as_str());
            if asked {
                me.listing.set(Some(Rc::new(Listing::from_json(m))));
            }
        }
        "attached" => {
            let labels: Vec<String> =
                m["labels"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            if let Some(q) = m["q"].as_u64().filter(|_| !labels.is_empty()) {
                me.attached.update(|a| {
                    a.insert(q, Rc::new(labels));
                });
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str], more: bool) -> Files {
        Files::of(paths.iter().copied(), more)
    }

    #[test]
    fn a_mention_begins_at_the_start_or_after_a_space_and_ends_where_the_session_ends_one() {
        let t = |text: &str, caret: usize| typed_at(text, caret).map(|t| (t.at, t.end, t.query, t.quoted));
        assert_eq!(t("@", 1), Some((0, 1, String::new(), false)));
        assert_eq!(t("see @src/ma", 11), Some((4, 11, "src/ma".into(), false)));
        assert_eq!(t("(@a", 3), Some((1, 3, "a".into(), false)));
        // What names it goes on after the caret.
        assert_eq!(t("see @src/main.rs now", 8), Some((4, 16, "src".into(), false)));
        assert_eq!(t("me@example.com", 14), None);
        assert_eq!(t("@src/a.rs done", 14), None);
        assert_eq!(t("@\"my no", 7), Some((0, 7, "my no".into(), true)));
        assert_eq!(t("@\"my notes.md\" and", 18), None);
        assert_eq!(t("no at here", 5), None);
    }

    #[test]
    fn a_range_of_lines_and_a_whole_path_are_told_apart_from_the_name() {
        assert_eq!(lines_of("cart:12-30"), ("cart", ":12-30"));
        assert_eq!(lines_of("cart:"), ("cart", ":"));
        assert_eq!(lines_of("C:/Users"), ("C:/Users", ""));
        assert_eq!(lines_of(":12"), (":12", ""));
        assert_eq!(whole_path("/etc/ho"), Some(("/etc/", "ho")));
        assert_eq!(whole_path("~/"), Some(("~/", "")));
        assert_eq!(whole_path("C:/Users/a"), Some(("C:/Users/", "a")));
        assert_eq!(whole_path("C:\\x"), Some(("C:\\", "x")));
        assert_eq!(whole_path("src/a"), None);
        assert_eq!(whole_path("~x"), None);
    }

    #[test]
    fn a_mention_is_put_in_quotes_when_its_path_has_a_space() {
        assert_eq!(mention("src/a.rs", "", true), "@src/a.rs ");
        assert_eq!(mention("src/a.rs", ":12-30", true), "@src/a.rs:12-30 ");
        assert_eq!(mention("my notes.md", ":3", true), "@\"my notes.md:3\" ");
        assert_eq!(mention("/home/me/", "", false), "@/home/me/");
        assert_eq!(mention("/my docs/", "", false), "@\"/my docs/");
        // Typed again after it, the quoted folder is still the mention.
        assert_eq!(typed_at("@\"/my docs/", 11).map(|t| t.query), Some("/my docs/".into()));
    }

    #[test]
    fn the_folders_the_files_are_in_are_offered_with_them_once_each() {
        let pool = Pool::of(&files(&["src/a.rs", "src/agent/b.rs", "README.md"], false));
        let all: Vec<&str> = (0..pool.all.len()).map(|i| pool.all.path(i)).collect();
        assert_eq!(all, ["src/a.rs", "src/agent/b.rs", "README.md", "src", "src/agent"]);
        assert_eq!(pool.files, 3);
        let (rows, total) = project_rows(&pool, "agent", 1);
        assert_eq!(total, 2);
        assert!(rows.iter().any(|e| e.folder && e.choice == Choice::Put("src/agent".into())));
    }

    #[test]
    fn fifty_are_listed_at_a_time_and_the_count_says_how_many_match() {
        let paths: Vec<String> = (0..130).map(|i| format!("src/f{i}.rs")).collect();
        let pool = Pool::of(&Files::of(paths.iter().map(String::as_str), true));
        let (rows, total) = project_rows(&pool, "f", 1);
        assert_eq!((rows.len(), total), (51, 130));
        assert_eq!(rows[50].choice, Choice::More);
        assert_eq!(rows[50].row.name, "Show 50 more");
        let (rows, _) = project_rows(&pool, "f", 2);
        assert_eq!(rows.last().map(|e| e.row.name.as_str()), Some("Show 30 more"));
        let (rows, _) = project_rows(&pool, "f", 3);
        assert!(rows.iter().all(|e| e.choice != Choice::More) && rows.len() == 130);
        assert!(pool.all.more);
        assert_eq!(count_line(50, 130), "50 of 130 matches");
        assert_eq!(count_line(3, 3), "3 matches");
        assert_eq!(count_line(1, 1), "1 match");
        assert_eq!(count_line(0, 0), "");
    }

    #[test]
    fn a_folder_on_this_machine_lists_its_folders_to_go_into_and_its_files_to_put() {
        let listing = Listing::from_json(&json!({
            "type": "folders", "path": "~/", "dir": "/home/me",
            "entries": [{"name": "docs", "dir": true}, {"name": "dots", "dir": false}, {"name": "x.md", "dir": false}],
        }));
        let (rows, total) = folder_rows(&listing, "do", 1);
        assert_eq!(total, 2);
        assert_eq!(rows[0].choice, Choice::Into("~/docs/".into()));
        assert_eq!(rows[1].choice, Choice::Put("/home/me/dots".into()));
        let (all, total) = folder_rows(&listing, "", 1);
        assert_eq!((all.len(), total), (3, 3));
    }

    #[test]
    fn a_chip_opens_its_file_at_its_first_line() {
        assert_eq!(chip_target("src/a.rs:12-30"), Some(("src/a.rs".into(), 12)));
        assert_eq!(chip_target("src/a.rs"), Some(("src/a.rs".into(), 0)));
        assert_eq!(chip_target("src/"), None);
    }

    #[test]
    fn a_folder_listed_is_taken_only_when_it_is_the_one_asked_for() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let me = ui.state::<Mentions>();
        me.asked.set(("/tmp/".into(), "mention-1".into()));
        take(ui, &json!({"type": "folders", "path": "/tmp/", "key": "mention-0", "dir": "/tmp"}), &send);
        assert!(me.listing.get_untracked().is_none());
        take(ui, &json!({"type": "folders", "path": "/tmp/", "key": "mention-1", "dir": "/tmp"}), &send);
        assert_eq!(me.listing.get_untracked().map(|l| l.dir.clone()), Some("/tmp".into()));
        take(ui, &json!({"type": "attached", "q": 2, "labels": ["src/a.rs:1-3", "src/"]}), &send);
        assert_eq!(me.attached.with_untracked(|a| a.get(&2).map(|l| l.len())), Some(2));
    }
}
