//! VS Code's quick input: Quick Open on the project's files (Ctrl+P), the Command Palette (Ctrl+Shift+P, or ">"
//! typed in Quick Open) and the colour theme pick (Ctrl+K Ctrl+T), drawn as its widget: a box 600 px wide near the
//! top of the window, an input, then rows of 22 px with the letters that matched lit. The files are ranked as the
//! page ranks them, on each key, and only the best fifty are drawn.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::Rect;
use floem::peniko::{Brush, Color};
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo, create_rw_signal};
use floem::style::{CursorStyle, Position};
use floem::text::{Attrs, AttrsList, FamilyOwned, LineHeightValue, TextLayout, Weight};
use floem::views::{
    Decorators, clip, dyn_stack, empty, h_stack, h_stack_from_iter, label, rich_text, scroll, stack, text_input,
    v_stack,
};
use floem::{AnyView, IntoView, View};
use serde_json::{Value, json};

use crate::colors::Colors;
use crate::commands::Command;
use crate::state::{Send, Ui};
use crate::widgets::{Theme, UI_FONT};

/// What the quick input is opened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Files,
    Commands,
    Themes,
    /// ":", "@" and "?" typed first (`quick_modes`).
    Line,
    Symbols,
    Help,
}

/// The most rows ranked and drawn.
pub const MOST: usize = 50;
/// VS Code's row, and how many of them the list shows before it scrolls.
const ROW: f64 = 22.0;
const SHOWN: usize = 20;
/// The widget's width, and how far under the window's top it is (over the command centre, as VS Code's).
const WIDTH: f64 = 600.0;
const TOP: f64 = 6.0;

/// The commands the palette lists now: every command that is there and is not a key's own, in the order added.
pub fn commands(ui: Ui) -> Vec<Rc<Command>> {
    crate::commands::all().into_iter().filter(|c| !c.hidden && c.offered(ui)).collect()
}

/// What was typed, as it is looked for: lower case, without its spaces.
pub struct Query {
    chars: Vec<char>,
    /// The same as bytes, when every letter is ASCII (as nearly all that is typed is): looked for in one pass.
    bytes: Option<Vec<u8>>,
    /// Which bytes are in it (see `mask`).
    mask: u64,
}

impl Query {
    pub fn of(text: &str) -> Query {
        let chars: Vec<char> = text.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
        let bytes: Option<Vec<u8>> = chars.iter().map(|c| u8::try_from(*c).ok().filter(u8::is_ascii)).collect();
        let mask = bytes.as_deref().map_or(0, mask);
        Query { chars, bytes, mask }
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

/// Which bytes a text has, as 64 bits: a letter or a digit its own, the others shared. A text that lacks one of the
/// query's cannot match it, and is passed over without being read.
fn mask(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0, |m, b| {
        m | 1
            << match *b {
                b'a'..=b'z' => b - b'a',
                b'0'..=b'9' => 26 + b - b'0',
                _ => 36 + b % 28,
            }
    })
}

/// A text as it is ranked: where it is among the texts in lower case (made once, not on each key), its length as
/// the page counts it, where the file's name begins in it, and which bytes it has.
struct Lower {
    from: u32,
    to: u32,
    len: u32,
    name_at: u32,
    mask: u64,
}

impl Lower {
    /// The text put in lower case after the others.
    fn of(text: &str, all: &mut String) -> Lower {
        let from = all.len();
        // Most paths are ASCII: no text of their own is made for them.
        if text.is_ascii() {
            all.push_str(text);
            all[from..].make_ascii_lowercase();
        } else {
            all.push_str(&text.to_lowercase());
        }
        let lower = &all[from..];
        Lower {
            from: from as u32,
            to: all.len() as u32,
            len: if lower.is_ascii() { lower.len() } else { lower.encode_utf16().count() } as u32,
            name_at: lower.rfind('/').map_or(0, |i| i + 1) as u32,
            mask: mask(lower.as_bytes()),
        }
    }

    /// The page's `score`: the query's letters found in order, a letter worth 1, 3 when it follows the one before
    /// it, and 2 more in the file's name; a hundred times that, less the text's length. None: not all are there.
    fn score(&self, all: &str, q: &Query) -> Option<i64> {
        if q.is_empty() {
            return Some(0);
        }
        let (name_at, mut at, mut total) = (self.name_at as usize, 0usize, 0i64);
        let text = &all[self.from as usize..self.to as usize];
        match &q.bytes {
            // One pass over the text's bytes, the letter looked for going on as each is found (a byte of ASCII is
            // never part of another letter's).
            Some(bytes) => {
                if q.mask & !self.mask != 0 {
                    return None;
                }
                let (mut k, mut want) = (0, bytes[0]);
                for (i, b) in text.bytes().enumerate() {
                    if b == want {
                        total += if i == at { 3 } else { 1 } + if i >= name_at { 2 } else { 0 };
                        at = i + 1;
                        k += 1;
                        if k == bytes.len() {
                            return Some(total * 100 - i64::from(self.len));
                        }
                        want = bytes[k];
                    }
                }
                None
            }
            None => {
                for &c in &q.chars {
                    let i = at + text[at..].find(c)?;
                    total += if i == at { 3 } else { 1 } + if i >= name_at { 2 } else { 0 };
                    at = i + c.len_utf8();
                }
                Some(total * 100 - i64::from(self.len))
            }
        }
    }
}

/// How well `text` matches `path`, as the page's `score` says it: -1 when it does not (the ranking's own measure,
/// kept to test it against the page's numbers).
#[cfg(test)]
pub fn score(path: &str, text: &str) -> i64 {
    let mut all = String::new();
    Lower::of(path, &mut all).score(&all, &Query::of(text)).unwrap_or(-1)
}

/// Where the query's letters are in `text` as it is written (the same ones `score` found), each as its bytes.
pub fn positions(text: &str, q: &Query) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut want = q.chars.iter().copied().peekable();
    for (i, c) in text.char_indices() {
        if want.peek().is_none() {
            break;
        }
        let mut lit = false;
        for l in c.to_lowercase() {
            if want.peek() == Some(&l) {
                want.next();
                lit = true;
            }
        }
        if lit {
            out.push(i..i + c.len_utf8());
        }
    }
    out
}

/// Texts to rank: the project's files, as the session listed them (or the commands' labels, the themes' names).
#[derive(Default)]
pub struct Files {
    paths: Vec<String>,
    /// Every path in lower case, one after the other (read from end to end on each key), and where each is in it.
    lower: String,
    each: Vec<Lower>,
    /// There are more files than were listed.
    pub more: bool,
    /// The session has answered (none listed yet is not none there).
    pub listed: bool,
}

impl Files {
    pub fn of<'a>(paths: impl Iterator<Item = &'a str>, more: bool) -> Files {
        let paths: Vec<String> = paths.map(str::to_string).collect();
        let mut lower = String::with_capacity(paths.iter().map(String::len).sum());
        let each = paths.iter().map(|p| Lower::of(p, &mut lower)).collect();
        Files { paths, lower, each, more, listed: true }
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    /// The text at a place, as listed.
    pub fn path(&self, i: usize) -> &str {
        &self.paths[i]
    }

    /// How many of the texts match the query (all of them, for none typed): counted without ranking them.
    pub fn count(&self, q: &Query) -> usize {
        if q.is_empty() {
            return self.len();
        }
        self.each.iter().filter(|l| l.score(&self.lower, q).is_some()).count()
    }

    /// These very files are the ones held (listed again and nothing changed: the rows and the row chosen stay).
    pub fn holds<'a>(&self, paths: impl Iterator<Item = &'a str>, more: bool) -> bool {
        self.listed && self.more == more && self.paths.iter().map(String::as_str).eq(paths)
    }

    /// The best `most` for the query, best first (which ones, by their place): every text is scored, and only the
    /// best are kept and put in order (never the whole list). Equal scores keep the order listed, as the page's
    /// sort does. A text that matches is kept however long it is (the page drops a long path that a short query
    /// matches, its score under 0).
    pub fn rank(&self, q: &Query, most: usize) -> Vec<usize> {
        if q.is_empty() || most == 0 {
            return (0..self.len().min(most)).collect();
        }
        let order = |a: &(i64, u32), b: &(i64, u32)| b.0.cmp(&a.0).then(a.1.cmp(&b.1));
        let mut best: Vec<(i64, u32)> = Vec::with_capacity(most * 2);
        // The last of the best so far, once there are `most`: one that comes after it is not kept at all.
        let mut last: Option<(i64, u32)> = None;
        for (i, l) in self.each.iter().enumerate() {
            let Some(s) = l.score(&self.lower, q) else { continue };
            let one = (s, i as u32);
            if last.is_some_and(|last| order(&one, &last).is_gt()) {
                continue;
            }
            best.push(one);
            if best.len() == most * 2 {
                best.select_nth_unstable_by(most - 1, order);
                best.truncate(most);
                last = Some(best[most - 1]);
            }
        }
        best.sort_unstable_by(order);
        best.truncate(most);
        best.into_iter().map(|(_, i)| i as usize).collect()
    }
}

/// What choosing a row does.
#[derive(Clone)]
pub enum Pick {
    File(String),
    /// A file opened at a line and a column (from 1).
    FileAt(String, usize, usize),
    /// A line and a column of the file shown.
    Line(usize, usize),
    /// A mode's prefix typed ("?" lists them).
    Prefix(String),
    /// A row that only says something.
    Nothing,
    /// A command, by its id.
    Command(String),
    Theme(String),
}

/// A row of the list: its name, what is said dimmed after it (a file's folder), the letters lit (in the name and
/// the folder written one after the other, two spaces between), a command's keys.
#[derive(Clone)]
pub struct Row {
    pub name: String,
    pub detail: String,
    pub lit: Vec<Range<usize>>,
    pub keys: String,
    pub pick: Pick,
    /// The group it heads, said at its right ("recently used"); empty: it heads none.
    pub group: String,
    /// A line over it: the group it heads is not the first.
    pub line: bool,
    /// A symbol's kind, drawn as its icon before the name.
    pub kind: Option<crate::outline::Kind>,
    /// What tells it from another row drawn in its place.
    pub(crate) key: u64,
}

/// What separates the name from the folder in the row's text.
const BETWEEN: &str = "  ";

impl Row {
    pub(crate) fn new(name: &str, detail: &str, lit: Vec<Range<usize>>, keys: &str, pick: Pick) -> Row {
        let mut row = Row {
            name: name.into(),
            detail: detail.into(),
            lit,
            keys: keys.into(),
            pick,
            group: String::new(),
            line: false,
            kind: None,
            key: 0,
        };
        row.rekey();
        row
    }

    fn rekey(&mut self) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&self.name, &self.detail, &self.lit, &self.keys, &self.group, self.line, self.kind).hash(&mut h);
        self.key = h.finish();
    }

    /// The row heading a group: its name at the right, a line over it when another group is before it.
    pub fn heading(mut self, group: &str, line: bool) -> Row {
        self.group = group.into();
        self.line = line;
        self.rekey();
        self
    }

    /// The row of a symbol: its kind's icon before the name.
    pub fn kind(mut self, kind: crate::outline::Kind) -> Row {
        self.kind = Some(kind);
        self.rekey();
        self
    }

    /// The row as one text: the name, then the folder.
    pub fn text(&self) -> String {
        if self.detail.is_empty() { self.name.clone() } else { format!("{}{BETWEEN}{}", self.name, self.detail) }
    }
}

/// The files' rows for what was typed: the best fifty, the file's name then its folder (none opened before:
/// Quick Open's own are `quick_modes::file_rows`).
#[cfg(test)]
pub fn file_rows(files: &Files, text: &str) -> Vec<Row> {
    let q = Query::of(text);
    files.rank(&q, MOST).into_iter().map(|i| path_row(&files.paths[i], &q)).collect()
}

/// A path's row: its last name, then the folder it is in, the query's letters lit in each.
pub fn path_row(path: &str, q: &Query) -> Row {
    let (dir, name_at) = match path.rfind('/') {
        Some(i) => (&path[..i], i + 1),
        None => ("", 0),
    };
    let name = &path[name_at..];
    // A letter found in the name is lit there, one found in the folder there (after the name).
    let lit = positions(path, q)
        .into_iter()
        .filter_map(|r| {
            if r.start >= name_at {
                Some(r.start - name_at..r.end - name_at)
            } else if r.end <= dir.len() {
                let by = name.len() + BETWEEN.len();
                Some(r.start + by..r.end + by)
            } else {
                None
            }
        })
        .collect();
    Row::new(name, dir, lit, "", Pick::File(path.to_string()))
}

/// The commands' rows for what was typed after the ">", none run before (the palette's own are
/// `quick_modes::command_rows`).
#[cfg(test)]
pub fn command_rows(all: &[Rc<Command>], text: &str) -> Vec<Row> {
    crate::quick_modes::command_rows(all, text, &[])
}

/// The themes' rows, and the row to begin on: the theme in force when nothing is typed.
pub fn theme_rows(all: &[ca_themes::Theme], text: &str, current: &str) -> (Vec<Row>, usize) {
    let q = Query::of(text);
    let names = Files::of(all.iter().map(|t| t.name()), false);
    let order = names.rank(&q, MOST);
    let at = order.iter().position(|i| all[*i].id == current).unwrap_or(0);
    let rows = order
        .into_iter()
        .map(|i| Row::new(all[i].name(), "", positions(all[i].name(), &q), "", Pick::Theme(all[i].id.clone())))
        .collect();
    (rows, if q.is_empty() { at } else { 0 })
}

/// What the quick input shows now: the themes, or the mode what is typed begins with (">" the commands, ":" a
/// line, "@" a symbol, "?" the modes), else the files.
pub fn shown(ui: Ui) -> Option<Mode> {
    let opened = ui.quick.get()?;
    Some(match opened {
        Mode::Themes => Mode::Themes,
        _ => ui.quick_text.with(|t| crate::quick_modes::mode_of(t)),
    })
}

/// The rows made again for what is typed (on each key, and when the files are listed), the first one chosen.
pub fn refresh(ui: Ui) {
    let Some(mode) = shown(ui) else { return };
    let text = ui.quick_text.get();
    let rest = crate::quick_modes::rest_of(&text, mode);
    let recent = crate::quick_modes::recent(ui);
    let (rows, at) = match mode {
        Mode::Files => {
            let rows = recent.files.with(|r| ui.quick_files.with(|f| crate::quick_modes::file_rows(f, &text, r)));
            let at = ui.active.with_untracked(|a| crate::quick_modes::first_chosen(&rows, &text, a.as_deref()));
            (rows, at)
        }
        Mode::Commands => (recent.commands.with(|r| crate::quick_modes::command_rows(&commands(ui), rest, r)), 0),
        Mode::Line => (crate::quick_modes::line_rows(rest, ui.caret.get(), crate::quick_modes::lines_shown(ui)), 0),
        Mode::Symbols => {
            let rows = match crate::quick_modes::symbols_shown(ui) {
                None => crate::quick_modes::symbol_rows(None, rest),
                Some(None) => Vec::new(),
                Some(Some(s)) => crate::quick_modes::symbol_rows(Some(&s), rest),
            };
            (rows, 0)
        }
        Mode::Help => (crate::quick_modes::help_rows(rest), 0),
        Mode::Themes => {
            let was = ui.quick_theme.get_untracked().unwrap_or_else(|| ui.choice.get_untracked());
            ui.themes.with(|all| theme_rows(all, &text, &ca_themes::chosen(all, &was, true).id))
        }
    };
    ui.quick_rows.set(Rc::new(rows));
    ui.quick_at.set(at);
}

/// The theme of the row chosen, shown while the themes are open (VS Code's preview on the arrow keys).
pub fn preview(ui: Ui) {
    let at = ui.quick_at.get();
    let pick = ui.quick_rows.with(|r| r.get(at).map(|r| r.pick.clone()));
    if ui.quick.get() == Some(Mode::Themes)
        && let Some(Pick::Theme(id)) = pick
        && ui.choice.get_untracked() != id
    {
        ui.show_theme(&id);
    }
}

/// The row chosen moved up or down, around the ends as VS Code's.
pub fn step(ui: Ui, by: isize) {
    let n = ui.quick_rows.with_untracked(|r| r.len()) as isize;
    if n > 0 {
        ui.quick_at.update(|at| *at = (*at as isize + by).rem_euclid(n) as usize);
    }
}

/// Opens the quick input on the files, the commands or the themes (again, when it is open on another).
pub fn open(ui: Ui, send: &Send, mode: Mode) {
    back(ui);
    ui.menu.set(None);
    ui.context.set(None);
    // "@" typed in any mode reads the file's symbols again (it may have been edited since).
    crate::quick_modes::forget_symbols(ui);
    if ui.quick.get_untracked().is_none() {
        crate::quick_modes::opened(ui);
    }
    match mode {
        Mode::Files => {
            // Listed again each time (files come and go); the list had before is ranked meanwhile.
            send(json!({"type": "files"}));
            ui.quick_text.set(String::new());
        }
        Mode::Commands => ui.quick_text.set(">".into()),
        Mode::Line => ui.quick_text.set(":".into()),
        Mode::Symbols => ui.quick_text.set("@".into()),
        Mode::Help => ui.quick_text.set("?".into()),
        Mode::Themes => {
            ui.quick_theme.set(Some(ui.choice.get_untracked()));
            ui.quick_text.set(String::new());
        }
    }
    ui.quick.set(Some(if mode == Mode::Themes { Mode::Themes } else { Mode::Files }));
}

/// The theme that was in force shown again, when a theme pick is left without a choice.
fn back(ui: Ui) {
    if let Some(was) = ui.quick_theme.get_untracked() {
        ui.quick_theme.set(None);
        if ui.choice.get_untracked() != was {
            ui.show_theme(&was);
        }
    }
}

pub fn close(ui: Ui) {
    if ui.quick.get_untracked().is_some() {
        back(ui);
        ui.quick.set(None);
    }
}

/// The row chosen: its file opened, its command run, its theme kept, its line gone to, its mode typed.
pub fn accept(ui: Ui, send: &Send, at: usize) {
    accept_in(ui, send, at, false);
}

/// The row chosen, a file to the side (Ctrl+Enter) when `side`.
pub fn accept_in(ui: Ui, send: &Send, at: usize, side: bool) {
    let Some(pick) = ui.quick_rows.with_untracked(|r| r.get(at).map(|r| r.pick.clone())) else { return };
    match pick {
        Pick::File(path) | Pick::FileAt(path, ..) if side => {
            close(ui);
            crate::editor::groups::open_to_side(ui, &path, send);
        }
        Pick::File(path) => {
            close(ui);
            ui.open(&path, send);
        }
        Pick::FileAt(path, line, col) => {
            close(ui);
            crate::quick_modes::open_at(ui, send, &path, line, col);
        }
        Pick::Line(line, col) => {
            close(ui);
            if let Some(path) = ui.active.get_untracked() {
                crate::quick_modes::open_at(ui, send, &path, line, col);
            }
        }
        Pick::Prefix(prefix) => ui.quick_text.set(prefix),
        Pick::Nothing => {}
        Pick::Command(id) => {
            close(ui);
            crate::quick_modes::ran(ui, send, &id);
            // The keys back where they were (the editor), then the command: one that gives them to a view keeps them.
            crate::quick_modes::give_back(ui);
            crate::commands::run(ui, send, &id);
        }
        Pick::Theme(id) => {
            ui.quick_theme.set(None);
            ui.show_theme(&id);
            send(json!({"type": "set_setting", "key": "workbench.colorTheme", "value": id}));
            close(ui);
        }
    }
}

/// A row's text laid out: the name, the folder smaller and dimmed, the letters that matched lit and bold.
pub(crate) fn layout_of(row: &Row, c: &Colors, selected: bool) -> TextLayout {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list(&UI_FONT).collect();
    let (fg, lit) = if selected { (c.quick_focus_fg, c.highlight_focus) } else { (c.quick_fg, c.highlight) };
    let dim = if selected { fg.multiply_alpha(0.7) } else { c.desc };
    let base = Attrs::new().family(&family).font_size(13.0).line_height(LineHeightValue::Px(ROW as f32)).color(fg);
    let small = base.font_size(12.0).color(dim);
    let text = row.text();
    let mut attrs = AttrsList::new(base);
    if text.len() > row.name.len() {
        attrs.add_span(row.name.len()..text.len(), small);
    }
    for r in &row.lit {
        let of = if r.start >= row.name.len() { small } else { base };
        attrs.add_span(r.clone(), of.color(lit).weight(Weight::BOLD));
    }
    let mut layout = TextLayout::new();
    layout.set_text(&text, attrs);
    layout
}

/// A command's keys as VS Code draws them: each key in its own box, "+" between the keys held together.
fn keys_view(t: Theme, keys: &str) -> impl IntoView + use<> {
    let mut parts: Vec<AnyView> = Vec::new();
    for (n, chord) in keys.split(' ').filter(|c| !c.is_empty()).enumerate() {
        if n > 0 {
            parts.push(empty().style(|s| s.width(4.0)).into_any());
        }
        for (k, key) in chord.split('+').enumerate() {
            if k > 0 {
                parts.push(label(|| "+").style(|s| s.font_size(11.0)).into_any());
            }
            let key = key.to_string();
            parts.push(
                label(move || key.clone())
                    .style(move |s| {
                        let c = t.get();
                        s.font_size(11.0)
                            .line_height(1.0)
                            .padding_horiz(5.0)
                            .padding_vert(3.0)
                            .border(1.0)
                            .border_radius(3.0)
                            .border_color(c.key_border)
                            .background(c.key_bg)
                            .color(c.key_fg)
                    })
                    .into_any(),
            );
        }
    }
    h_stack_from_iter(parts).style(|s| s.items_center().gap(2.0).padding_left(8.0).flex_shrink(0.0_f32))
}

fn row_view(ui: Ui, send: Send, i: usize, row: Row) -> impl IntoView {
    let t = ui.colors;
    let selected: Memo<bool> = create_memo(move |_| ui.quick_at.get() == i);
    let keys = keys_view(t, &row.keys);
    let (group, line, heads) = (row.group.clone(), row.line, !row.group.is_empty());
    let mark: AnyView = match row.kind {
        Some(k) => crate::icons::icon(k.icon(), 16.0)
            .style(move |s| s.margin_right(6.0).flex_shrink(0.0_f32).color(k.colour(&t.get())))
            .into_any(),
        None => empty().into_any(),
    };
    // The group's name at the right of the row heading it, as VS Code's quick pick.
    let heading = label(move || group.clone()).style(move |s| {
        let c = t.get();
        s.font_size(12.0)
            .padding_left(10.0)
            .flex_shrink(0.0_f32)
            .color(if selected.get() { c.quick_focus_fg } else { c.picker_group_fg })
            .apply_if(!heads, |s| s.hide())
    });
    let text = rich_text(move || layout_of(&row, &t.get(), selected.get())).style(|s| s.flex_shrink(0.0_f32));
    let text = clip(text).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).height(ROW));
    h_stack((mark, text, keys, heading))
        // On the press, not the release: a press takes the input's focus, which closes the widget.
        .on_event_stop(EventListener::PointerDown, move |_| accept(ui, &send, i))
        .style(move |s| {
            let c = t.get();
            let s = s
                .height(ROW)
                .width_full()
                .min_width(0.0)
                .items_center()
                .padding_horiz(6.0)
                .border_radius(3.0)
                .cursor(CursorStyle::Pointer)
                .selectable(false)
                .color(c.quick_fg)
                .apply_if(line, |s| s.border_top(1.0).border_color(c.picker_group_border));
            if selected.get() { s.background(c.quick_focus_bg) } else { s.hover(|s| s.background(c.list_hover)) }
        })
}

/// The quick input, over the window: for the root's stack. Hidden while it is closed.
pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    create_effect(move |_| refresh(ui));
    create_effect(move |_| preview(ui));

    let focused = create_rw_signal(false);
    let keys = send.clone();
    let input = text_input(ui.quick_text)
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            let m = k.modifiers;
            match &k.key.logical_key {
                Key::Named(NamedKey::ArrowDown) => step(ui, 1),
                Key::Named(NamedKey::ArrowUp) => step(ui, -1),
                // Ctrl+Enter: the file opened to the side.
                Key::Named(NamedKey::Enter) => {
                    accept_in(ui, &keys, ui.quick_at.get_untracked(), m.contains(Modifiers::CONTROL))
                }
                Key::Character(c) if m.contains(Modifiers::CONTROL) && c.eq_ignore_ascii_case("p") => {
                    open(ui, &keys, if m.contains(Modifiers::SHIFT) { Mode::Commands } else { Mode::Files })
                }
                _ => {}
            }
            // No key goes on to the workbench's own while the quick input has them (nor Tab to another view).
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| {
            // Escape, or a press elsewhere: closed, unless the focus is given back at once (a press in the box, a
            // command that opens the quick input again).
            focused.set(false);
            floem::action::exec_after(Duration::ZERO, move |_| {
                if !focused.get_untracked() {
                    close(ui);
                }
            });
        })
        .style(move |s| {
            let c = t.get();
            s.width_full()
                .height(24.0)
                .padding_horiz(5.0)
                .border(0.0)
                .font_size(13.0)
                .background(Color::TRANSPARENT)
                .color(c.input_fg)
                .cursor_color(Brush::Solid(c.cursor))
        });
    let input_id = input.id();
    create_effect(move |_| match ui.quick.get() {
        Some(_) => input_id.request_focus(),
        None => input_id.clear_focus(),
    });

    let hint = label(move || match ui.quick.get() {
        Some(Mode::Themes) => "Select Color Theme (Up/Down Keys to Preview)",
        _ => "Search files by name (append : to go to line or @ to go to symbol)",
    })
    .style(move |s| {
        s.position(Position::Absolute)
            .inset_left(6.0)
            .font_size(13.0)
            .selectable(false)
            .color(t.get().placeholder)
            .apply_if(ui.quick_text.with(|x| !x.is_empty()), |s| s.hide())
    });
    let field = stack((hint, input)).style(move |s| {
        let c = t.get();
        s.margin_horiz(6.0)
            .margin_top(8.0)
            .margin_bottom(6.0)
            .height(26.0)
            .items_center()
            .border(1.0)
            .border_radius(2.0)
            .border_color(c.focus)
            .background(c.input_bg)
    });

    let none = label(move || match shown(ui) {
        Some(Mode::Files) if !ui.quick_files.with(|f| f.listed) => "Listing the files…",
        Some(Mode::Commands) => "No matching commands",
        _ => "No matching results",
    })
    .style(move |s| {
        s.height(ROW)
            .margin_bottom(6.0)
            .padding_horiz(12.0)
            .items_center()
            .selectable(false)
            .color(t.get().quick_fg)
            .apply_if(ui.quick_rows.with(|r| !r.is_empty()), |s| s.hide())
    });

    let rows = dyn_stack(
        move || ui.quick_rows.get().iter().cloned().enumerate().collect::<Vec<_>>(),
        |(i, r): &(usize, Row)| (*i, r.key),
        move |(i, r): (usize, Row)| row_view(ui, send.clone(), i, r),
    )
    .style(|s| s.flex_col().width_full());
    let list = scroll(rows)
        .ensure_visible(move || {
            let y = ui.quick_at.get() as f64 * ROW;
            Rect::new(0.0, y, 1.0, y + ROW)
        })
        // No bar: a press on it would take the input's focus (and close the widget) before anything here knew.
        // The wheel and the arrows scroll it.
        .scroll_style(|s| s.hide_bars(true))
        .style(move |s| {
            let n = ui.quick_rows.with(|r| r.len());
            s.margin_horiz(6.0)
                .height(n.min(SHOWN) as f64 * ROW)
                .apply_if(n > 0, |s| s.margin_bottom(6.0))
                .apply_if(n == 0, |s| s.hide())
        });

    // The session lists so many files and no more: said under them.
    let more = label(|| "The folder has more files than are listed here.").style(move |s| {
        let c = t.get();
        s.height(ROW)
            .margin_bottom(6.0)
            .padding_horiz(12.0)
            .items_center()
            .font_size(12.0)
            .selectable(false)
            .color(c.desc)
            .apply_if(shown(ui) != Some(Mode::Files) || !ui.quick_files.with(|f| f.more), |s| s.hide())
    });
    let widget = v_stack((field, none, list, more))
        // A press in the box (not on a row, not in the input) leaves the input its focus.
        .on_event_stop(EventListener::PointerDown, move |_| input_id.request_focus())
        .style(move |s| {
            let c = t.get();
            s.width(WIDTH)
                .max_width((ui.window.get().0 - 20.0).max(100.0))
                .margin_top(TOP)
                .border(1.0)
                .border_radius(6.0)
                .border_color(c.quick_border)
                .background(c.quick_bg)
                .color(c.quick_fg)
                .font_size(13.0)
                .box_shadow_blur(8.0)
                .box_shadow_spread(2.0)
                .box_shadow_color(c.shadow)
        });
    stack((
        empty()
            .on_event_stop(EventListener::PointerDown, move |_| close(ui))
            .style(|s| s.position(Position::Absolute).inset(0.0).size_full()),
        widget,
    ))
    .style(move |s| {
        s.position(Position::Absolute)
            .inset(0.0)
            .size_full()
            .flex_col()
            .items_center()
            .z_index(200)
            .apply_if(ui.quick.get().is_none(), |s| s.hide())
    })
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are Quick Open's, taken in. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "files" {
        let paths = || m["paths"].as_array().into_iter().flatten().filter_map(Value::as_str);
        let more = m["more"] == true;
        // Listed again as they were: what is shown stays (the row chosen with it).
        if !ui.quick_files.with_untracked(|f| f.holds(paths(), more)) {
            ui.quick_files.set(Rc::new(crate::palette::Files::of(paths(), more)));
        }
    }
    false
}
#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::Instant;

    use serde_json::Value;

    use super::*;

    fn sender() -> (Send, Rc<RefCell<Vec<Value>>>) {
        let said = Rc::new(RefCell::new(Vec::new()));
        let to = said.clone();
        (Rc::new(move |v: Value| to.borrow_mut().push(v)), said)
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| if r.detail.is_empty() { r.name.clone() } else { format!("{}/{}", r.detail, r.name) })
            .collect()
    }

    #[test]
    fn the_score_is_the_pages() {
        // Nothing typed matches everything, evenly; a letter that is not there, nothing.
        assert_eq!(score("src/main.rs", ""), 0);
        assert_eq!(score("src/main.rs", "  "), 0);
        assert_eq!(score("src/main.rs", "x"), -1);
        assert_eq!(score("src/main.rs", "sm"), (3 + 1 + 2) * 100 - 11);
        // In the name, each letter 2 more; one that follows the one before it 3, not 1.
        assert_eq!(score("src/main.rs", "main"), (1 + 3 * 3 + 4 * 2) * 100 - 11);
        // The first letter of the text counts as following.
        assert_eq!(score("main.rs", "m"), (3 + 2) * 100 - 7);
        assert_eq!(score("a/b.rs", "A B"), (3 + 1 + 2) * 100 - 6);
        // Letters in order only.
        assert_eq!(score("ab", "ba"), -1);
        // Any case, and letters that are not ASCII.
        assert_eq!(score("Src/Größe.RS", "größe"), (1 + 3 * 4 + 5 * 2) * 100 - 12);
        // A long path a short query matches is under 0 on the page too (it is kept in the list here).
        assert_eq!(score(&format!("{}/x", "d".repeat(200)), "d"), 300 - 202);
        assert_eq!(score(&format!("{}/x", "d".repeat(400)), "x"), 300 - 402);
    }

    #[test]
    fn the_best_match_is_first_and_equal_ones_stay_as_listed() {
        let files =
            Files::of(["state/deep/other.rs", "crates/ca-native/src/state.rs", "src/state.rs"].into_iter(), false);
        // 2088 (most of it in the name, the path short), 1481 (all together, in the folder), 1471.
        assert_eq!(
            names(&file_rows(&files, "state")),
            ["src/state.rs", "state/deep/other.rs", "crates/ca-native/src/state.rs"]
        );
        assert_eq!(names(&file_rows(&files, "zzz")), Vec::<String>::new());
        // Nothing typed: as listed. Equal scores: as listed.
        assert_eq!(names(&file_rows(&files, "")).len(), 3);
        let same = Files::of(["b/x.rs", "a/x.rs", "c/x.rs"].into_iter(), false);
        assert_eq!(names(&file_rows(&same, "x")), ["b/x.rs", "a/x.rs", "c/x.rs"]);
    }

    #[test]
    fn only_the_best_are_kept_and_they_are_the_ones_a_full_sort_gives() {
        let paths: Vec<String> =
            (0..3000).map(|i| format!("dir{}/sub{}/file{}.rs", i % 17, i % 5, i * 7919 % 3000)).collect();
        let files = Files::of(paths.iter().map(String::as_str), false);
        for text in ["f", "file1", "d1s3", "sub", "rs", "9"] {
            let q = Query::of(text);
            let mut all: Vec<(i64, usize)> =
                paths.iter().enumerate().map(|(i, p)| (score(p, text), i)).filter(|x| x.0 >= 0).collect();
            all.sort_by_key(|a| std::cmp::Reverse(a.0));
            let want: Vec<usize> = all.into_iter().take(MOST).map(|x| x.1).collect();
            assert_eq!(files.rank(&q, MOST), want, "{text}");
        }
        assert_eq!(files.rank(&Query::of(""), MOST), (0..MOST).collect::<Vec<_>>());
        assert!(files.rank(&Query::of("f"), 0).is_empty());
    }

    #[test]
    fn the_letters_that_matched_are_lit_in_the_name_and_in_the_folder() {
        let files = Files::of(["src/Main.rs", "top.rs"].into_iter(), false);
        let rows = file_rows(&files, "sm");
        assert_eq!(rows.len(), 1);
        let text = rows[0].text();
        assert_eq!(text, "Main.rs  src");
        let lit: Vec<&str> = rows[0].lit.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(lit, ["s", "M"], "the folder's letter after the name's, as they are written");
        // The same letters the score found: the first ones, in order.
        assert_eq!(positions("crates/ca-cli/src/ide.rs", &Query::of("cide")), [0..1, 12..13, 19..20, 20..21]);
        // A letter whose lower case is longer, or of several bytes: lit whole, on its own bounds.
        assert_eq!(positions("Größe.rs", &Query::of("öe")), [2..4, 6..7]);
        assert_eq!(positions("İx", &Query::of("x")), vec![2..3]);
    }

    #[test]
    fn the_commands_are_all_there_in_their_order_and_found_by_their_letters() {
        let ui = Ui::new();
        let (send, _) = sender();
        crate::workbench::register(ui, &send);
        let all = commands(ui);
        let rows = command_rows(&all, "");
        let labels: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        // The palette as the window began with it, to the letter and in this order, and the one command it lacked
        // for a view it had: Show Outline.
        assert_eq!(
            labels,
            [
                "File: Save",
                "View: Close Editor",
                "Go to File…",
                "View: Toggle Primary Side Bar Visibility",
                "View: Toggle Secondary Side Bar Visibility",
                "View: Toggle Panel Visibility",
                "View: Show Explorer",
                "View: Show Search",
                "View: Show the Agent's Changes",
                "View: Show Outline",
                "Terminal: Focus Terminal",
                "Terminal: Create New Terminal",
                "Terminal: Kill the Active Terminal Instance",
                "Terminal: Clear",
                "View: Show Agent",
                "Preferences: Color Theme",
                "Help: About",
                "View: Reset View Locations",
                "View: Float Explorer",
                "View: Float Search",
                "View: Float Changes",
                "View: Float Outline",
                "View: Float Agent",
                "View: Float Terminal",
            ]
        );
        let keys: Vec<&str> = rows.iter().map(|r| r.keys.as_str()).filter(|k| !k.is_empty()).collect();
        assert_eq!(
            keys,
            [
                "Ctrl+S",
                "Ctrl+W",
                "Ctrl+P",
                "Ctrl+B",
                "Ctrl+Alt+B",
                "Ctrl+J",
                "Ctrl+Shift+E",
                "Ctrl+Shift+F",
                "Ctrl+`",
                "Ctrl+Shift+`",
                "Ctrl+Alt+I",
                "Ctrl+K Ctrl+T"
            ]
        );
        let found = command_rows(&all, " color theme");
        assert_eq!(found[0].name, "Preferences: Color Theme");
        assert_eq!(found[0].keys, "Ctrl+K Ctrl+T");
        assert_eq!(found[0].lit.len(), 10);
    }

    #[test]
    fn a_command_chosen_runs_by_its_id_and_one_added_is_listed() {
        use crate::layout::View;
        let ui = Ui::new();
        let (send, said) = sender();
        // With no command added, nothing (and no panic).
        open(ui, &send, Mode::Commands);
        refresh(ui);
        assert_eq!(shown(ui), Some(Mode::Commands));
        ui.quick_text.set(">toggle panel".into());
        refresh(ui);
        accept(ui, &send, 0);
        assert!(ui.group_shown(View::Terminal));
        close(ui);
        crate::workbench::register(ui, &send);
        let run = |typed: &str| {
            open(ui, &send, Mode::Commands);
            ui.quick_text.set(typed.into());
            refresh(ui);
            accept(ui, &send, 0);
        };
        run(">toggle panel");
        assert!(!ui.group_shown(View::Terminal));
        run(">toggle primary");
        assert!(!ui.group_shown(View::Files));
        run(">toggle secondary");
        assert!(!ui.group_shown(View::Agent));
        run(">show explorer");
        assert!(ui.view_shown(View::Files));
        run(">show search");
        assert!(ui.view_shown(View::Search));
        // Before the agent's changes: once those have run, they come first for "show agent" (run last, and a match).
        run(">show agent");
        assert!(ui.view_shown(View::Agent));
        run(">agent's changes");
        assert!(ui.view_shown(View::Changes));
        run(">focus terminal");
        assert!(ui.view_shown(View::Terminal));
        assert!(said.borrow().is_empty(), "these commands ask nothing of the session");
        crate::commands::add(
            crate::commands::command("test.added", "Test", "Added", |ui, _| ui.message.set("added".into()))
                .keys("Ctrl+K A"),
        );
        run(">test added");
        assert_eq!(ui.message.get_untracked(), "added");
        // A key's own command, and one that is not there now, are not listed.
        crate::commands::add(crate::commands::command("test.hidden", "Test", "Never Listed", |_, _| {}).hidden());
        crate::commands::add(
            crate::commands::command("test.later", "Test", "Listed While It Works", |_, _| {})
                .when(|ui| ui.running.get_untracked()),
        );
        let labels = |ui: Ui| commands(ui).iter().map(|c| c.label()).collect::<Vec<_>>();
        assert!(!labels(ui).iter().any(|l| l == "Test: Never Listed" || l == "Test: Listed While It Works"));
        ui.running.set(true);
        assert!(labels(ui).iter().any(|l| l == "Test: Listed While It Works"));
    }

    #[test]
    fn quick_open_lists_the_files_and_opens_the_one_chosen() {
        let ui = Ui::new();
        let (send, said) = sender();
        open(ui, &send, Mode::Files);
        assert_eq!(said.borrow()[0], json!({"type": "files"}));
        assert!(!ui.quick_files.get_untracked().listed);
        ui.take(&json!({"type": "files", "paths": ["src/a.rs", "src/b.rs", "c.rs"], "more": false}), &send);
        refresh(ui);
        assert_eq!(ui.quick_rows.get_untracked().len(), 3);
        ui.quick_text.set("b".into());
        refresh(ui);
        assert_eq!(names(&ui.quick_rows.get_untracked()), ["src/b.rs"]);
        // The arrows go around the ends.
        ui.quick_text.set(String::new());
        refresh(ui);
        step(ui, -1);
        assert_eq!(ui.quick_at.get_untracked(), 2);
        step(ui, 1);
        assert_eq!(ui.quick_at.get_untracked(), 0);
        step(ui, 1);
        accept(ui, &send, ui.quick_at.get_untracked());
        assert_eq!(said.borrow().last(), Some(&json!({"type": "open", "path": "src/b.rs"})));
        assert_eq!(ui.quick.get_untracked(), None);
        // ">" typed in it: the commands.
        open(ui, &send, Mode::Files);
        ui.quick_text.set(">".into());
        assert_eq!(shown(ui), Some(Mode::Commands));
    }

    #[test]
    fn a_theme_is_previewed_on_the_arrows_kept_when_chosen_and_gone_back_from_when_not() {
        let ui = Ui::new();
        // The theme the settings name, not the system's.
        let mut values = serde_json::Map::new();
        values.insert("window.autoDetectColorScheme".into(), json!(false));
        let follow = crate::settings::Said { values, ..Default::default() };
        ui.state::<crate::settings::Settings>().said.set(Rc::new(follow));
        let (send, said) = sender();
        let before = ui.colors.get_untracked();
        open(ui, &send, Mode::Themes);
        refresh(ui);
        preview(ui);
        let rows = ui.quick_rows.get_untracked();
        assert_eq!(rows.len(), ui.themes.get_untracked().len());
        // It begins on the theme in force (Dark Modern, none being chosen), which looks as before.
        assert_eq!(rows[ui.quick_at.get_untracked()].name, "Dark Modern");
        assert_eq!(ui.colors.get_untracked(), before);
        let light = rows.iter().position(|r| r.name == "Light Modern").expect("Light Modern");
        ui.quick_at.set(light);
        preview(ui);
        assert!(!ui.colors.get_untracked().dark, "shown while it is the row chosen");
        close(ui);
        assert_eq!(ui.colors.get_untracked(), before);
        assert_eq!(ui.choice.get_untracked(), "");
        assert!(said.borrow().is_empty());
        // Chosen: kept in the settings.
        open(ui, &send, Mode::Themes);
        ui.quick_text.set("light modern".into());
        refresh(ui);
        preview(ui);
        accept(ui, &send, 0);
        assert_eq!(
            said.borrow().last(),
            Some(&json!({"type": "set_setting", "key": "workbench.colorTheme", "value": "light-modern"}))
        );
        assert!(!ui.colors.get_untracked().dark);
        assert_eq!(ui.quick.get_untracked(), None);
    }

    /// Fifty thousand paths as a large project's: folders some levels deep, names of several words.
    fn many() -> Vec<String> {
        let dirs = ["src", "crates", "web", "tests", "docs", "internal", "vendor", "packages"];
        let words = ["state", "editor", "layout", "index", "model", "view", "render", "parser", "client", "theme"];
        let exts = ["rs", "ts", "tsx", "json", "md", "toml"];
        (0..50_000usize)
            .map(|i| {
                format!(
                    "{}/{}-{}/{}/{}_{}_{}.{}",
                    dirs[i % dirs.len()],
                    words[i / 7 % words.len()],
                    i % 211,
                    words[i / 3 % words.len()],
                    words[i % words.len()],
                    words[i / 11 % words.len()],
                    i,
                    exts[i % exts.len()]
                )
            })
            .collect()
    }

    #[test]
    fn a_key_ranks_fifty_thousand_paths_well_within_a_frame() {
        let paths = many();
        let from = Instant::now();
        let files = Files::of(paths.iter().map(String::as_str), false);
        println!("the list taken (once, when the session answers): {:?}", from.elapsed());
        assert_eq!(files.len(), 50_000);
        let from = Instant::now();
        assert!(files.holds(paths.iter().map(String::as_str), false));
        println!("the same list known for the same: {:?}", from.elapsed());
        assert!(!files.holds(paths.iter().skip(1).map(String::as_str), false));
        let mut worst = Duration::ZERO;
        let recent: Vec<String> = paths.iter().step_by(1000).take(crate::quick_modes::RECENT_FILES).cloned().collect();
        for text in
            ["s", "st", "state", "stateeditor", "src/state", "editor42.ts", "crates layout render 499", "zzzz", "e"]
        {
            let mut took: Vec<Duration> = (0..9)
                .map(|_| {
                    let from = Instant::now();
                    // As Quick Open ranks them: the files opened last too.
                    let rows = crate::quick_modes::file_rows(&files, text, &recent);
                    let took = from.elapsed();
                    assert!(rows.len() <= MOST);
                    took
                })
                .collect();
            took.sort();
            println!("{text:?}: least {:?}, median {:?}, most {:?}", took[0], took[4], took[8]);
            worst = worst.max(took[0]);
        }
        println!("the slowest query, at its least: {worst:?}");
        // A frame is 16 ms; the whole of a key's work on the list (ranked, the rows made) is to be a small part.
        assert!(worst < Duration::from_millis(8), "{worst:?}");
    }
}
