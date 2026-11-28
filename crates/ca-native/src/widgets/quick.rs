//! The quick pick: one host for every list to choose from and every line of text asked for, drawn as VS Code's
//! quick input (a box 600 px wide near the top of the window: its title, the input, rows of 22 px) and working as
//! the old Code Arena's: typing narrows the items to those that have every word typed, Enter takes the row lit,
//! Escape or a press elsewhere takes none.
//!
//! ```ignore
//! // A list to choose one from.
//! quick::pick("Select Color Theme")
//!     .placeholder("Select Color Theme (Up/Down Keys to Preview)")
//!     .items(themes.iter().map(|t| quick::item(&t.id, &t.name).group(kind_of(t))).collect())
//!     .active(&current)
//!     .on_active(move |item| if let Some(item) = item { ui.show_theme(&item.id) })
//!     .open(ui, move |item| match item {
//!         Some(item) => keep(&item.id),
//!         None => ui.show_theme(&was),
//!     });
//! // A long list the caller ranks (and cuts) itself, and a text taken as it is typed.
//! quick::pick("Go to File").rank(move |typed| best(typed)).free(|typed| Some(quick::item("go", &format!("Go to {typed}"))))
//!     .most(50).open(ui, |item| …);
//! // A line of text.
//! quick::input("Rename Branch").value("main").validate(|v| v.contains(' ').then(|| "No spaces.".into()))
//!     .open(ui, |name| if let Some(name) = name { rename(&name) });
//! quick::input("Password for guest").password().open(ui, |secret| …);
//! ```
//!
//! An item shows its icon, its label, its description (dimmed, after the label) and its detail (dimmed, at the
//! right); the first item of a group has a line over it and the group's name at its right. The items of a group
//! are listed together, the groups in the order their first items come. At most `most` rows are drawn (200 unless
//! said): the rest is "N more: type to narrow the list.". `on_active` is told the item lit each time it is another
//! (at first too): a live preview. `done` is given the item taken, or none, once, after the widget has closed and
//! the keys are back where `back` says. One at a time: a second answers the first with none, a dialog asked while
//! it is open too, and it cancels a dialog that is open. It is in the main window, and may be opened from anywhere
//! (a command, a menu's row, the palette, a dialog's answer, a message taken in). The first text of an input box is
//! not selected (the text box cannot be told to): the caret is at its end, and Ctrl+A selects it.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use floem::action::exec_after;
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::Rect;
use floem::peniko::{Brush, Color};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::{CursorStyle, Position, TextOverflow};
use floem::text::{Attrs, AttrsList, FamilyOwned, LineHeightValue, TextLayout, Weight};
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, VirtualVector, clip, container, dyn_container, empty, h_stack,
    label, rich_text, scroll, stack, text_input, v_stack, virtual_stack,
};
use floem::{AnyView, IntoView, View, ViewId};
use serde_json::Value;

use crate::colors::Colors;
use crate::icons::icon;
use crate::keymap::TakesKeys;
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::widgets::UI_FONT;

/// The most rows drawn unless the caller says another number (the old Code Arena's).
pub const MOST: usize = 200;
/// VS Code's row, and how many of them the list shows before it scrolls.
const ROW: f64 = 22.0;
const SHOWN: usize = 20;
/// The widget's width, and how far under the window's top it is.
const WIDTH: f64 = 600.0;
const TOP: f64 = 6.0;
/// What an input box says under its box while what is typed may be taken.
const PROMPT: &str = "Press 'Enter' to confirm your input or 'Escape' to cancel";

/// One choice of a pick.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Item {
    /// What the caller knows it by.
    pub id: String,
    pub label: String,
    /// Said after the label, dimmed.
    pub description: String,
    /// Said at the right, dimmed.
    pub detail: String,
    /// A codicon before the label; empty: none.
    pub icon: &'static str,
    /// The group it is listed under ("recently used", "light themes"); empty: none.
    pub group: String,
    /// The parts of the label lit (what was typed, found in it). The host fills it when it narrows the items
    /// itself; a caller that ranks says its own.
    pub lit: Vec<Range<usize>>,
}

/// A choice: what the caller knows it by, and what it shows.
pub fn item(id: &str, label: &str) -> Item {
    Item { id: id.into(), label: label.into(), ..Item::default() }
}

impl Item {
    pub fn description(mut self, text: &str) -> Item {
        self.description = text.into();
        self
    }

    pub fn detail(mut self, text: &str) -> Item {
        self.detail = text.into();
        self
    }

    pub fn icon(mut self, name: &'static str) -> Item {
        self.icon = name;
        self
    }

    pub fn group(mut self, name: &str) -> Item {
        self.group = name.into();
        self
    }

    pub fn lit(mut self, lit: Vec<Range<usize>>) -> Item {
        self.lit = lit;
        self
    }
}

/// The items for what is typed, best first, in place of the host's own narrowing.
type Rank = Rc<dyn Fn(&str) -> Vec<Item>>;
/// What is typed taken as it is: the row offered for it, or none.
type Free = Rc<dyn Fn(&str) -> Option<Item>>;
/// What is wrong with what is typed; none: it may be taken.
type Validate = Rc<dyn Fn(&str) -> Option<String>>;
type OnActive = Rc<dyn Fn(Option<&Item>)>;
/// Who is told the item taken, once.
type Done = Box<dyn FnOnce(Option<Item>)>;

/// A list to choose from, as it is being made.
pub struct Pick {
    title: String,
    placeholder: String,
    items: Vec<Item>,
    active: Option<String>,
    rank: Option<Rank>,
    free: Option<Free>,
    value: String,
    empty: bool,
    on_active: Option<OnActive>,
    most: usize,
    back: Option<ViewId>,
}

/// A list to choose one from. The title is over the input (none: no title).
pub fn pick(title: &str) -> Pick {
    Pick {
        title: title.into(),
        placeholder: String::new(),
        items: Vec::new(),
        active: None,
        rank: None,
        free: None,
        value: String::new(),
        empty: false,
        on_active: None,
        most: MOST,
        back: None,
    }
}

impl Pick {
    /// What the empty input says.
    pub fn placeholder(mut self, text: &str) -> Pick {
        self.placeholder = text.into();
        self
    }

    /// The choices, which typing narrows to those that have every word typed (in the label, the description or
    /// the detail), in their order.
    pub fn items(mut self, items: Vec<Item>) -> Pick {
        self.items = items;
        self
    }

    /// The id of the item lit at first (the first row, when it is not said or not there).
    pub fn active(mut self, id: &str) -> Pick {
        self.active = Some(id.into());
        self
    }

    /// The caller lists what fits what is typed, best first, in place of the host narrowing `items`: for a list
    /// of thousands, ranked and cut by the caller. It is called on each key: it must be quick.
    pub fn rank(mut self, rank: impl Fn(&str) -> Vec<Item> + 'static) -> Pick {
        self.rank = Some(Rc::new(rank));
        self
    }

    /// What is typed can be taken as it is: the row offered for it (after the items), or none. It is given what
    /// is typed without the spaces around it, and is not asked while nothing is typed unless `empty`.
    pub fn free(mut self, free: impl Fn(&str) -> Option<Item> + 'static) -> Pick {
        self.free = Some(Rc::new(free));
        self
    }

    /// The text in the input at first.
    pub fn value(mut self, text: &str) -> Pick {
        self.value = text.into();
        self
    }

    /// `free` is asked while nothing is typed too.
    pub fn empty(mut self) -> Pick {
        self.empty = true;
        self
    }

    /// Told the item lit each time it is another one (and at first); none while no row is lit.
    pub fn on_active(mut self, told: impl Fn(Option<&Item>) + 'static) -> Pick {
        self.on_active = Some(Rc::new(told));
        self
    }

    /// The most rows drawn.
    pub fn most(mut self, most: usize) -> Pick {
        self.most = most.max(1);
        self
    }

    /// The view that has the keys again once the pick has closed (none: no view has them).
    pub fn back(mut self, to: ViewId) -> Pick {
        self.back = Some(to);
        self
    }

    /// The editor drawn has the keys again once the pick has closed, as after VS Code's picks over a file (its
    /// language, indentation or line ending).
    pub fn back_to_editor(mut self, ui: Ui) -> Pick {
        let editor = ui.live.with_untracked(|l| l.as_ref().and_then(|(_, e)| e.editor_view_id.get_untracked()));
        if editor.is_some() {
            self.back = editor;
        }
        self
    }

    /// Opens it. `done` is given the item taken (none: cancelled), once.
    pub fn open(self, ui: Ui, done: impl FnOnce(Option<Item>) + 'static) {
        let hay = self.items.iter().map(hay_of).collect();
        begin(
            ui,
            Asked {
                title: self.title,
                placeholder: self.placeholder,
                items: self.items,
                hay,
                active: self.active,
                rank: self.rank,
                free: self.free,
                empty: self.empty,
                input: None,
                password: false,
                on_active: self.on_active,
                most: self.most,
                back: self.back,
                told: RefCell::new(None),
                done: RefCell::new(Some(Box::new(done))),
            },
            &self.value,
        );
    }
}

/// A line of text asked for, as it is being made.
pub struct Input {
    title: String,
    placeholder: String,
    prompt: String,
    value: String,
    validate: Option<Validate>,
    password: bool,
    back: Option<ViewId>,
}

/// A line of text to ask for, as VS Code's input box. What is typed is taken as it is, an empty answer too, unless
/// `validate` says what is wrong with it.
pub fn input(title: &str) -> Input {
    Input {
        title: title.into(),
        placeholder: String::new(),
        prompt: String::new(),
        value: String::new(),
        validate: None,
        password: false,
        back: None,
    }
}

impl Input {
    pub fn placeholder(mut self, text: &str) -> Input {
        self.placeholder = text.into();
        self
    }

    /// What is asked, said under the box before "Press 'Enter' to confirm…".
    pub fn prompt(mut self, text: &str) -> Input {
        self.prompt = text.into();
        self
    }

    /// The text in the box at first.
    pub fn value(mut self, text: &str) -> Input {
        self.value = text.into();
        self
    }

    /// What is wrong with what is typed (said under the box, and Enter does nothing); none: it may be taken.
    pub fn validate(mut self, validate: impl Fn(&str) -> Option<String> + 'static) -> Input {
        self.validate = Some(Rc::new(validate));
        self
    }

    /// What is typed is a secret: drawn as dots, never as itself, and taken with its spaces.
    pub fn password(mut self) -> Input {
        self.password = true;
        self
    }

    /// The view that has the keys again once the box has closed.
    pub fn back(mut self, to: ViewId) -> Input {
        self.back = Some(to);
        self
    }

    /// Opens it. `done` is given the text typed (none: cancelled), once.
    pub fn open(self, ui: Ui, done: impl FnOnce(Option<String>) + 'static) {
        begin(
            ui,
            Asked {
                title: self.title,
                placeholder: self.placeholder,
                items: Vec::new(),
                hay: Vec::new(),
                active: None,
                rank: None,
                free: None,
                empty: true,
                input: Some((self.prompt, self.validate)),
                password: self.password,
                on_active: None,
                most: 1,
                back: self.back,
                told: RefCell::new(None),
                done: RefCell::new(Some(Box::new(move |item: Option<Item>| done(item.map(|i| i.label))))),
            },
            &self.value,
        );
    }
}

/// What is open: what was asked, and who is told.
struct Asked {
    title: String,
    placeholder: String,
    items: Vec<Item>,
    /// Each item as it is looked through: its label, description and detail in lower case.
    hay: Vec<String>,
    active: Option<String>,
    rank: Option<Rank>,
    free: Option<Free>,
    empty: bool,
    /// An input box: what it asks, and what says whether what is typed may be taken. None: a list.
    input: Option<(String, Option<Validate>)>,
    password: bool,
    on_active: Option<OnActive>,
    most: usize,
    back: Option<ViewId>,
    /// The id `on_active` was last told (none: that no row is lit); nothing yet: not told.
    told: RefCell<Option<Option<String>>>,
    done: RefCell<Option<Done>>,
}

impl Asked {
    /// What is typed as it is taken: a secret whole, anything else without the spaces around it.
    fn typed<'a>(&self, text: &'a str) -> &'a str {
        if self.password { text } else { text.trim() }
    }

    /// What is wrong with what is typed in an input box; none: it may be taken (or this is a list).
    fn wrong(&self, text: &str) -> Option<String> {
        let (_, validate) = self.input.as_ref()?;
        validate.as_ref()?(self.typed(text))
    }
}

/// A row of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub item: Item,
    /// The first of its group: the group's name is at its right.
    pub heads: bool,
    /// A line over it: its group is not the first.
    pub line: bool,
    /// What tells it from another row drawn in its place.
    key: u64,
}

impl Row {
    fn new(item: Item, heads: bool, line: bool) -> Row {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&item, heads, line).hash(&mut h);
        Row { item, heads, line, key: h.finish() }
    }
}

/// An item as it is looked through.
fn hay_of(item: &Item) -> String {
    format!("{} {} {}", item.label, item.description, item.detail).to_lowercase()
}

/// Where the words typed are in a label, for lighting them: each word's first place, in order, none overlapping.
fn lit_of(label: &str, words: &[String]) -> Vec<Range<usize>> {
    let lower = label.to_lowercase();
    // Lower case of another length (rare letters): nothing is lit rather than the wrong letters.
    if lower.len() != label.len() {
        return Vec::new();
    }
    let mut found: Vec<Range<usize>> =
        words.iter().filter_map(|w| lower.find(w.as_str()).map(|at| at..at + w.len())).collect();
    found.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in found {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out.into_iter().filter(|r| label.is_char_boundary(r.start) && label.is_char_boundary(r.end)).collect()
}

/// The items that have every word typed (in the label, the description or the detail), in their order, the words
/// lit in their labels. Nothing typed: all of them.
pub fn narrow(items: &[Item], hay: &[String], typed: &str) -> Vec<Item> {
    let words: Vec<String> = typed.to_lowercase().split_whitespace().map(str::to_string).collect();
    items
        .iter()
        .zip(hay)
        .filter(|(_, hay)| words.iter().all(|w| hay.contains(w.as_str())))
        .map(|(item, _)| {
            let mut item = item.clone();
            item.lit = lit_of(&item.label, &words);
            item
        })
        .collect()
}

/// The rows drawn for the items that fit: at most `most` of them, the items of a group together (the groups in the
/// order their first items come), then the row for what is typed as it is. And how many more fit than are drawn.
pub fn rows_of(fit: Vec<Item>, most: usize, free: Option<Item>) -> (Vec<Row>, usize) {
    let more = fit.len().saturating_sub(most);
    let shown: Vec<Item> = fit.into_iter().take(most).collect();
    let mut groups: Vec<&str> = Vec::new();
    for item in &shown {
        if !groups.contains(&item.group.as_str()) {
            groups.push(&item.group);
        }
    }
    let mut rows = Vec::with_capacity(shown.len() + 1);
    for (g, group) in groups.iter().enumerate() {
        for (n, item) in shown.iter().filter(|i| i.group == *group).enumerate() {
            rows.push(Row::new(item.clone(), n == 0 && !group.is_empty(), n == 0 && g > 0));
        }
    }
    if let Some(free) = free {
        let line = !rows.is_empty();
        rows.push(Row::new(free, false, line));
    }
    (rows, more)
}

/// The row lit after a key: `by` rows on, around the ends as VS Code's.
pub fn step(at: usize, rows: usize, by: isize) -> usize {
    if rows == 0 { 0 } else { (at as isize + by).rem_euclid(rows as isize) as usize }
}

/// The row lit after Page Up or Page Down: a page on, stopping at the ends.
pub fn page(at: usize, rows: usize, down: bool) -> usize {
    let by = SHOWN - 1;
    if down { (at + by).min(rows.saturating_sub(1)) } else { at.saturating_sub(by) }
}

/// The host's state: what is open, what is typed, the rows drawn for it and the one lit.
#[derive(Clone, Copy)]
struct Host {
    open: RwSignal<Option<Rc<Asked>>>,
    /// One more each time a pick opens: the input is made again for it.
    serial: RwSignal<u64>,
    text: RwSignal<String>,
    rows: RwSignal<im::Vector<Row>>,
    more: RwSignal<usize>,
    at: RwSignal<usize>,
    /// The input made for the pick open, and the pick (by its number) whose input has the keys (0: none has).
    field: RwSignal<Option<ViewId>>,
    keys: RwSignal<u64>,
}

impl crate::ext::State for Host {
    fn new(ui: Ui) -> Host {
        Host {
            open: ui.scope.create_rw_signal(None),
            serial: ui.scope.create_rw_signal(0),
            text: ui.scope.create_rw_signal(String::new()),
            rows: ui.scope.create_rw_signal(im::Vector::new()),
            more: ui.scope.create_rw_signal(0),
            at: ui.scope.create_rw_signal(0),
            field: ui.scope.create_rw_signal(None),
            keys: ui.scope.create_rw_signal(0),
        }
    }
}

/// Whether a pick or an input box is open.
pub fn is_open(ui: Ui) -> bool {
    ui.state::<Host>().open.with_untracked(|o| o.is_some())
}

/// The rows made again for what is typed, and the row lit: the item said to be lit at first while nothing was
/// typed since, else the first.
fn refresh(ui: Ui) {
    let host = ui.state::<Host>();
    let Some(asked) = host.open.get_untracked() else { return };
    let text = host.text.get_untracked();
    let typed = asked.typed(&text);
    let (rows, more) = if asked.input.is_some() {
        (Vec::new(), 0)
    } else {
        let fit = match &asked.rank {
            Some(rank) => rank(typed),
            None => narrow(&asked.items, &asked.hay, typed),
        };
        let free = asked.free.as_ref().filter(|_| !typed.is_empty() || asked.empty).and_then(|free| free(typed));
        rows_of(fit, asked.most, free)
    };
    let at = asked.active.as_ref().and_then(|id| rows.iter().position(|r| r.item.id == *id)).unwrap_or(0);
    let rows: im::Vector<Row> = rows.into_iter().collect();
    if host.rows.with_untracked(|was| *was != rows) {
        host.rows.set(rows);
    }
    if host.more.get_untracked() != more {
        host.more.set(more);
    }
    if host.at.get_untracked() != at {
        host.at.set(at);
    }
    tell_active(ui);
}

/// `on_active` told the item lit, when it is another than it was last told.
fn tell_active(ui: Ui) {
    let host = ui.state::<Host>();
    let Some(asked) = host.open.get_untracked() else { return };
    let Some(told) = &asked.on_active else { return };
    let item = host.rows.with_untracked(|r| r.get(host.at.get_untracked()).map(|r| r.item.clone()));
    let id = item.as_ref().map(|i| i.id.clone());
    if asked.told.borrow().as_ref() == Some(&id) {
        return;
    }
    *asked.told.borrow_mut() = Some(id);
    told(item.as_ref());
}

/// Another row lit.
fn light(ui: Ui, at: usize) {
    let host = ui.state::<Host>();
    if host.at.get_untracked() != at {
        host.at.set(at);
        tell_active(ui);
    }
}

/// Opens what was asked: the one open answers none first; the palette and the menus close, and a dialog that is
/// open is cancelled (it is drawn over the pick, which would have the keys unseen).
fn begin(ui: Ui, asked: Asked, value: &str) {
    let host = ui.state::<Host>();
    finish(ui, None);
    crate::palette::close(ui);
    crate::widgets::popup::close(ui);
    crate::widgets::dialog::cancel(ui);
    if ui.menu.get_untracked().is_some() {
        ui.menu.set(None);
    }
    if ui.context.get_untracked().is_some() {
        ui.context.set(None);
    }
    if host.text.with_untracked(|t| t != value) {
        host.text.set(value.to_string());
    }
    host.open.set(Some(Rc::new(asked)));
    host.serial.update(|n| *n += 1);
    refresh(ui);
}

/// The open pick closed with the item taken (none: cancelled): the keys go back, then its asker is told.
fn finish(ui: Ui, item: Option<Item>) {
    let host = ui.state::<Host>();
    let Some(asked) = host.open.get_untracked() else { return };
    host.open.set(None);
    match (asked.back, host.field.get_untracked()) {
        (Some(back), _) => back.request_focus(),
        (None, Some(field)) => field.clear_focus(),
        _ => {}
    }
    let done = asked.done.borrow_mut().take();
    if let Some(done) = done {
        done(item);
    }
}

/// The open pick closed with nothing taken.
pub fn cancel(ui: Ui) {
    finish(ui, None);
}

/// Enter, or a press on a row: the row is taken; in an input box, what is typed, unless something is wrong with it.
fn accept(ui: Ui, at: usize) {
    let host = ui.state::<Host>();
    let Some(asked) = host.open.get_untracked() else { return };
    if asked.input.is_some() {
        let text = host.text.get_untracked();
        if asked.wrong(&text).is_none() {
            finish(ui, Some(item("typed", asked.typed(&text))));
        }
        return;
    }
    if let Some(row) = host.rows.with_untracked(|r| r.get(at).cloned()) {
        finish(ui, Some(row.item));
    }
}

/// A key pressed in the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Down,
    Up,
    PageDown,
    PageUp,
    Take,
    Leave,
}

fn press_of(key: &Key) -> Option<Press> {
    Some(match key {
        Key::Named(NamedKey::ArrowDown) => Press::Down,
        Key::Named(NamedKey::ArrowUp) => Press::Up,
        Key::Named(NamedKey::PageDown) => Press::PageDown,
        Key::Named(NamedKey::PageUp) => Press::PageUp,
        Key::Named(NamedKey::Enter) => Press::Take,
        Key::Named(NamedKey::Escape) => Press::Leave,
        _ => return None,
    })
}

/// A key of the list's, done.
fn press(ui: Ui, key: Press) {
    let host = ui.state::<Host>();
    let (at, n) = (host.at.get_untracked(), host.rows.with_untracked(im::Vector::len));
    match key {
        Press::Down => light(ui, step(at, n, 1)),
        Press::Up => light(ui, step(at, n, -1)),
        Press::PageDown => light(ui, page(at, n, true)),
        Press::PageUp => light(ui, page(at, n, false)),
        Press::Take => accept(ui, at),
        Press::Leave => cancel(ui),
    }
}

/// A secret after a key: a letter typed is added, Backspace takes the last away, Ctrl+V adds what was copied
/// (`pasted` reads it). None: the key is not the secret's.
pub(crate) fn secret_key(text: &str, key: &Key, m: Modifiers, pasted: impl FnOnce() -> Option<String>) -> Option<String> {
    let held = m.contains(Modifiers::CONTROL) || m.contains(Modifiers::META);
    match key {
        Key::Character(c) if held && c.eq_ignore_ascii_case("v") => {
            let more: String = pasted()?.chars().filter(|c| !c.is_control()).collect();
            Some(format!("{text}{more}"))
        }
        Key::Character(c) if held && c.eq_ignore_ascii_case("u") => Some(String::new()),
        Key::Character(c) if !held && !m.contains(Modifiers::ALT) => Some(format!("{text}{c}")),
        Key::Named(NamedKey::Space) if !held => Some(format!("{text} ")),
        Key::Named(NamedKey::Backspace) if held => Some(String::new()),
        Key::Named(NamedKey::Backspace) => {
            let mut text = text.to_string();
            text.pop();
            Some(text)
        }
        _ => None,
    }
}

/// A row's text laid out: the label, the description smaller and dimmed after it, the parts lit.
fn layout_of(item: &Item, c: &Colors, selected: bool) -> TextLayout {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list(&UI_FONT).collect();
    let (fg, lit) = if selected { (c.quick_focus_fg, c.highlight_focus) } else { (c.quick_fg, c.highlight) };
    let dim = if selected { fg.multiply_alpha(0.7) } else { c.desc };
    let base = Attrs::new().family(&family).font_size(13.0).line_height(LineHeightValue::Px(ROW as f32)).color(fg);
    let text =
        if item.description.is_empty() { item.label.clone() } else { format!("{}  {}", item.label, item.description) };
    let mut attrs = AttrsList::new(base);
    if text.len() > item.label.len() {
        attrs.add_span(item.label.len()..text.len(), base.font_size(12.0).color(dim));
    }
    for r in item.lit.iter().filter(|r| r.end <= item.label.len()) {
        attrs.add_span(r.clone(), base.color(lit).weight(Weight::BOLD));
    }
    let mut layout = TextLayout::new();
    layout.set_text(&text, attrs);
    layout
}

fn row_view(ui: Ui, i: usize, row: Row) -> impl IntoView {
    let t = ui.colors;
    let host = ui.state::<Host>();
    let selected = create_memo(move |_| host.at.get() == i);
    let Row { item, heads, line, .. } = row;
    let mark: AnyView = if item.icon.is_empty() {
        empty().into_any()
    } else {
        icon(item.icon, 16.0).style(|s| s.margin_right(6.0).flex_shrink(0.0_f32)).into_any()
    };
    let (detail, group) = (item.detail.clone(), if heads { item.group.clone() } else { String::new() });
    let (has_detail, has_group) = (!detail.is_empty(), !group.is_empty());
    let text = rich_text(move || layout_of(&item, &t.get(), selected.get())).style(|s| s.flex_shrink(0.0_f32));
    h_stack((
        mark,
        clip(text).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).height(ROW)),
        label(move || detail.clone()).style(move |s| {
            let c = t.get();
            s.font_size(12.0)
                .padding_left(8.0)
                .max_width_pct(50.0)
                .text_ellipsis()
                .color(if selected.get() { c.quick_focus_fg.multiply_alpha(0.7) } else { c.desc })
                .apply_if(!has_detail, |s| s.hide())
        }),
        label(move || group.clone()).style(move |s| {
            let c = t.get();
            s.font_size(12.0)
                .padding_left(10.0)
                .flex_shrink(0.0_f32)
                .color(if selected.get() { c.quick_focus_fg } else { c.picker_group_fg })
                .apply_if(!has_group, |s| s.hide())
        }),
    ))
    // On the press, not the release: a press takes the input's focus, which closes the widget.
    .on_event_stop(EventListener::PointerDown, move |_| accept(ui, i))
    .style(move |s| {
        let c = t.get();
        let s = s
            .height(ROW)
            .width_full()
            .min_width(0.0)
            .items_center()
            .padding_horiz(6.0)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .color(c.quick_fg)
            .apply_if(line, |s| s.border_top(1.0).border_color(c.picker_group_border));
        if selected.get() {
            s.border_radius(3.0).background(c.quick_focus_bg).color(c.quick_focus_fg)
        } else {
            s.hover(|s| s.background(c.list_hover))
        }
    })
}

/// The input of the pick that has just opened, made for it: VS Code's text box, or a secret's dots. Its keys are
/// the list's (the arrows, the pages, Enter); it closes the widget when it loses the keys.
fn field(ui: Ui, serial: u64) -> AnyView {
    let t = ui.colors;
    let host = ui.state::<Host>();
    let Some(asked) = host.open.get_untracked() else { return empty().into_any() };
    let mine = move || host.serial.get_untracked() == serial && is_open(ui);
    let got = move || host.keys.set(serial);
    let lost = move || {
        if host.keys.get_untracked() == serial {
            host.keys.set(0);
        }
        // Escape, or a press elsewhere: closed, unless the keys are given back at once (a press in the box).
        exec_after(Duration::ZERO, move |_| {
            if mine() && host.keys.get_untracked() != serial {
                cancel(ui);
            }
        });
    };
    let hint = {
        let text = asked.placeholder.clone();
        label(move || text.clone()).style(move |s| {
            s.position(Position::Absolute)
                .inset_left(6.0)
                .font_size(13.0)
                .selectable(false)
                .color(t.get().placeholder)
                .apply_if(host.text.with(|x| !x.is_empty()), |s| s.hide())
        })
    };
    let id;
    let input: AnyView = if asked.password {
        // A secret is never in a text box (which would draw it, and copy it): its keys are taken here.
        let dots = h_stack((
            label(move || "•".repeat(host.text.with(|x| x.chars().count()).min(80))),
            empty().style(move |s| s.width(1.0).height(16.0).background(t.get().cursor)),
        ))
        .style(|s| s.items_center().padding_horiz(6.0).height_full());
        let view = container(dots).keyboard_navigable();
        id = view.id();
        view.on_event(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(k) = e {
                let (key, m) = (&k.key.logical_key, k.modifiers);
                if let Some(pressed) = press_of(key) {
                    press(ui, pressed);
                } else if let Some(text) =
                    host.text.with_untracked(|was| secret_key(was, key, m, || floem::Clipboard::get_contents().ok()))
                    && host.text.with_untracked(|was| *was != text)
                {
                    host.text.set(text);
                }
            }
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusGained, move |_| got())
        .on_event_cont(EventListener::FocusLost, move |_| lost())
        .takes_keys(crate::keymap::Focus::Input)
        .style(move |s| s.width_full().height(24.0).color(t.get().input_fg))
        .into_any()
    } else {
        let view = text_input(host.text);
        id = view.id();
        view.on_event(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(k) = e
                && let Some(pressed) = press_of(&k.key.logical_key)
                // Escape is the text box's own: it gives up the keys, which closes the widget.
                && pressed != Press::Leave
            {
                press(ui, pressed);
            }
            // No key goes on to the workbench's own while the pick has them (nor Tab to another view).
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusGained, move |_| got())
        .on_event_cont(EventListener::FocusLost, move |_| lost())
        .takes_keys(crate::keymap::Focus::Input)
        .style(move |s| {
            let c = t.get();
            let s = s
                .width_full()
                .height(24.0)
                .padding_horiz(5.0)
                .border(0.0)
                .font_size(13.0)
                .background(Color::TRANSPARENT)
                .color(c.input_fg)
                .cursor_color(Brush::Solid(c.cursor));
            crate::widgets::steady_input(s, Color::TRANSPARENT, None)
        })
        .into_any()
    };
    host.field.set(Some(id));
    // Once the input is in the window, it has the keys.
    exec_after(Duration::ZERO, move |_| {
        if mine() {
            id.request_focus();
        }
    });
    stack((hint, input)).style(|s| s.width_full().items_center()).into_any()
}

/// The quick pick, over the window: for the window's layers. Hidden while none is open.
fn host_view(ui: Ui, _send: Send) -> AnyView {
    let t = ui.colors;
    let host = ui.state::<Host>();
    // The rows follow what is typed.
    create_effect(move |_| {
        host.text.with(|_| ());
        refresh(ui);
    });
    let asked = move || host.open.get();
    let title = label(move || asked().map(|a| a.title.clone()).unwrap_or_default()).style(move |s| {
        let c = t.get();
        s.width_full()
            .height(27.0)
            .items_center()
            .justify_center()
            .font_size(13.0)
            .border_radius(5.0)
            .background(c.quick_title_bg)
            .selectable(false)
            .apply_if(host.open.with(|o| o.as_ref().is_none_or(|a| a.title.is_empty())), |s| s.hide())
    });
    let serial = create_memo(move |_| host.serial.get());
    let wrong = create_memo(move |_| asked().and_then(|a| host.text.with(|text| a.wrong(text))));
    let input = dyn_container(move || serial.get(), move |serial| field(ui, serial)).style(move |s| {
        let c = t.get();
        s.margin_horiz(6.0)
            .margin_top(8.0)
            .margin_bottom(6.0)
            .height(26.0)
            .items_center()
            .border(1.0)
            .border_radius(2.0)
            .border_color(if wrong.get().is_some() { c.input_error } else { c.focus })
            .background(c.input_bg)
    });
    // An input box says under its box what it asks, or what is wrong with what is typed.
    let said = label(move || {
        wrong.get().unwrap_or_else(|| match asked().and_then(|a| a.input.as_ref().map(|(prompt, _)| prompt.clone())) {
            Some(prompt) if !prompt.is_empty() => format!("{prompt} ({PROMPT})"),
            _ => PROMPT.to_string(),
        })
    })
    .style(move |s| {
        let c = t.get();
        let s = s
            .margin_horiz(6.0)
            .margin_bottom(6.0)
            .padding_horiz(6.0)
            .padding_vert(3.0)
            .line_height(1.4)
            .selectable(false)
            .text_overflow(TextOverflow::Wrap)
            .apply_if(host.open.with(|o| o.as_ref().is_none_or(|a| a.input.is_none())), |s| s.hide());
        if wrong.get().is_some() {
            s.margin_top(-7.0).border(1.0).border_color(c.input_error).background(c.input_error_bg).color(c.fg)
        } else {
            s.color(c.quick_fg)
        }
    });
    let none = label(|| "Nothing matches.").style(move |s| {
        let listed = host.open.with(|o| o.as_ref().is_some_and(|a| a.input.is_none()));
        s.height(ROW)
            .margin_bottom(6.0)
            .padding_horiz(12.0)
            .items_center()
            .selectable(false)
            .color(t.get().quick_fg)
            .apply_if(!listed || host.rows.with(|r| !r.is_empty()), |s| s.hide())
    });
    let rows = virtual_stack(
        VirtualDirection::Vertical,
        VirtualItemSize::Fixed(Box::new(|| ROW)),
        move || host.rows.get().enumerate(),
        |(i, r): &(usize, Row)| (*i, r.key),
        move |(i, r): (usize, Row)| row_view(ui, i, r),
    )
    .style(|s| s.flex_col().width_full());
    let list = scroll(rows)
        .ensure_visible(move || {
            let y = host.at.get() as f64 * ROW;
            Rect::new(0.0, y, 1.0, y + ROW)
        })
        // No bar: a press on it would take the input's focus (and close the widget) before anything here knew.
        // The wheel and the arrows scroll it.
        .scroll_style(|s| s.hide_bars(true))
        .style(move |s| {
            let n = host.rows.with(im::Vector::len);
            s.margin_horiz(6.0)
                .height(n.min(SHOWN) as f64 * ROW)
                .apply_if(n > 0, |s| s.margin_bottom(6.0))
                .apply_if(n == 0, |s| s.hide())
        });
    let more = label(move || format!("{} more: type to narrow the list.", host.more.get())).style(move |s| {
        s.margin_horiz(6.0)
            .margin_bottom(6.0)
            .padding_horiz(6.0)
            .font_size(12.0)
            .selectable(false)
            .color(t.get().desc)
            .apply_if(host.more.get() == 0, |s| s.hide())
    });
    let widget = v_stack((title, input, said, none, list, more))
        // A press in the box (not on a row, not in the input) leaves the input its keys.
        .on_event_stop(EventListener::PointerDown, move |_| {
            if let Some(field) = host.field.get_untracked() {
                field.request_focus();
            }
        })
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
            .on_event_stop(EventListener::PointerDown, move |_| cancel(ui))
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
            .apply_if(host.open.with(Option::is_none), |s| s.hide())
    })
    .into_any()
}

/// What it adds to the window's registries, once at start: the host, over the window.
pub fn register(_ui: Ui, _send: &Send) {
    // Under the dialogs, over the hovers.
    layers::add(Place::Over, "quick", 25, host_view);
}

/// The session's messages that are this part's, taken in: none are.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| r.item.label.clone()).collect()
    }

    fn shown(ui: Ui) -> Vec<String> {
        ui.state::<Host>().rows.with_untracked(|r| r.iter().map(|r| r.item.label.clone()).collect())
    }

    fn type_in(ui: Ui, text: &str) {
        ui.state::<Host>().text.set(text.into());
        // As the host's effect does in the window.
        refresh(ui);
    }

    fn themes() -> Vec<Item> {
        vec![
            item("dark+", "Dark+").description("default dark").group("dark themes"),
            item("light+", "Light+").description("default light").group("light themes"),
            item("monokai", "Monokai").group("dark themes").detail("built in"),
            item("solar", "Solarized Light").group("light themes"),
        ]
    }

    #[test]
    fn typing_narrows_the_items_to_those_that_have_every_word() {
        let items = themes();
        let hay: Vec<String> = items.iter().map(hay_of).collect();
        let found = |typed: &str| narrow(&items, &hay, typed).into_iter().map(|i| i.label).collect::<Vec<_>>();
        assert_eq!(found(""), ["Dark+", "Light+", "Monokai", "Solarized Light"], "nothing typed: all, as listed");
        assert_eq!(found("light"), ["Light+", "Solarized Light"]);
        assert_eq!(found("LIGHT sol"), ["Solarized Light"], "every word, in any case");
        assert_eq!(found("default"), ["Dark+", "Light+"], "in the description too");
        assert_eq!(found("built"), ["Monokai"], "and in the detail");
        assert_eq!(found("zzz"), Vec::<String>::new());
        // The words are lit where they are in the label.
        let lit = |typed: &str, n: usize| narrow(&items, &hay, typed)[n].lit.clone();
        assert_eq!(lit("light", 1), vec![10..15]);
        assert_eq!(lit("sol light", 0), [0..3, 10..15]);
        assert_eq!(lit("solar lar", 0), vec![0..5], "words that overlap are lit once");
        assert_eq!(lit("default", 0), Vec::<Range<usize>>::new(), "found in the description: nothing lit");
    }

    #[test]
    fn the_rows_are_cut_to_the_most_drawn_and_a_groups_items_are_listed_together() {
        let (rows, more) = rows_of(themes(), 200, None);
        assert_eq!(labels(&rows), ["Dark+", "Monokai", "Light+", "Solarized Light"]);
        let marks: Vec<(bool, bool)> = rows.iter().map(|r| (r.heads, r.line)).collect();
        assert_eq!(marks, [(true, false), (false, false), (true, true), (false, false)]);
        assert_eq!(more, 0);
        // Cut before they are grouped: the best stay.
        let (rows, more) = rows_of(themes(), 3, None);
        assert_eq!((labels(&rows), more), (vec!["Dark+".to_string(), "Monokai".into(), "Light+".into()], 1));
        // What is typed as it is comes last, under a line; alone, without one.
        let (rows, _) = rows_of(themes(), 1, Some(item("free", "Go to x")));
        assert_eq!(labels(&rows), ["Dark+", "Go to x"]);
        assert!(rows[1].line && !rows[1].heads);
        let (rows, _) = rows_of(Vec::new(), 5, Some(item("free", "Go to x")));
        assert!(!rows[0].line);
        // Items of no group have no heading.
        let (rows, _) = rows_of(vec![item("a", "A"), item("b", "B")], 5, None);
        assert!(rows.iter().all(|r| !r.heads && !r.line));
    }

    #[test]
    fn the_arrows_go_around_the_ends_and_the_pages_stop_at_them() {
        assert_eq!(step(0, 3, 1), 1);
        assert_eq!(step(2, 3, 1), 0);
        assert_eq!(step(0, 3, -1), 2);
        assert_eq!(step(0, 0, 1), 0);
        assert_eq!(page(0, 100, true), 19);
        assert_eq!(page(95, 100, true), 99);
        assert_eq!(page(5, 100, false), 0);
        assert_eq!(page(0, 0, true), 0);
        assert_eq!(press_of(&Key::Named(NamedKey::ArrowDown)), Some(Press::Down));
        assert_eq!(press_of(&Key::Named(NamedKey::ArrowUp)), Some(Press::Up));
        assert_eq!(press_of(&Key::Named(NamedKey::PageDown)), Some(Press::PageDown));
        assert_eq!(press_of(&Key::Named(NamedKey::PageUp)), Some(Press::PageUp));
        assert_eq!(press_of(&Key::Named(NamedKey::Enter)), Some(Press::Take));
        assert_eq!(press_of(&Key::Named(NamedKey::Escape)), Some(Press::Leave));
        assert_eq!(press_of(&Key::Character("a".into())), None);
    }

    #[test]
    fn a_pick_begins_on_the_item_said_tells_the_item_lit_and_gives_the_one_taken_once() {
        let ui = Ui::new();
        let host = ui.state::<Host>();
        let lit = Rc::new(RefCell::new(Vec::<Option<String>>::new()));
        let taken = Rc::new(RefCell::new(Vec::<Option<String>>::new()));
        let (told, done) = (lit.clone(), taken.clone());
        pick("Select Color Theme")
            .items(themes())
            .active("light+")
            .on_active(move |i| told.borrow_mut().push(i.map(|i| i.id.clone())))
            .open(ui, move |i| done.borrow_mut().push(i.map(|i| i.id)));
        assert!(is_open(ui));
        assert_eq!(shown(ui), ["Dark+", "Monokai", "Light+", "Solarized Light"]);
        assert_eq!(host.at.get_untracked(), 2, "the item said, where its group put it");
        assert_eq!(*lit.borrow(), [Some("light+".to_string())], "told at first");
        press(ui, Press::Down);
        press(ui, Press::Down);
        assert_eq!(host.at.get_untracked(), 0, "around the end");
        press(ui, Press::Up);
        assert_eq!(lit.borrow()[1..], [Some("solar".to_string()), Some("dark+".into()), Some("solar".into())]);
        // Typing narrows, and the row lit is the item said while it is there.
        type_in(ui, "light");
        assert_eq!((shown(ui), host.at.get_untracked()), (vec!["Light+".to_string(), "Solarized Light".into()], 0));
        assert_eq!(lit.borrow().last(), Some(&Some("light+".to_string())));
        type_in(ui, "zzz");
        assert_eq!(lit.borrow().last(), Some(&None), "no row is lit");
        press(ui, Press::Take);
        assert!(is_open(ui), "Enter on nothing takes nothing");
        type_in(ui, "mono");
        press(ui, Press::Take);
        assert!(!is_open(ui));
        cancel(ui);
        assert_eq!(*taken.borrow(), [Some("monokai".to_string())], "told once");
    }

    #[test]
    fn a_second_pick_answers_the_first_with_none_and_escape_takes_none() {
        let ui = Ui::new();
        let said = Rc::new(RefCell::new(Vec::<String>::new()));
        let told = |name: &'static str| {
            let said = said.clone();
            move |i: Option<Item>| said.borrow_mut().push(format!("{name}:{}", i.map_or("none".into(), |i| i.id)))
        };
        pick("One").items(vec![item("a", "A")]).open(ui, told("one"));
        pick("Two").items(vec![item("b", "B")]).value("b").open(ui, told("two"));
        assert_eq!(*said.borrow(), ["one:none"]);
        assert_eq!(ui.state::<Host>().text.get_untracked(), "b", "the text it begins with");
        assert_eq!(shown(ui), ["B"]);
        press(ui, Press::Leave);
        assert_eq!(*said.borrow(), ["one:none", "two:none"]);
        assert!(!is_open(ui));
        // The palette closes when a pick opens.
        ui.quick.set(Some(crate::palette::Mode::Commands));
        pick("Three").open(ui, told("three"));
        assert!(ui.quick.get_untracked().is_none());
    }

    #[test]
    fn a_pick_and_a_dialog_are_never_open_together() {
        use crate::widgets::dialog;
        let ui = Ui::new();
        let said = Rc::new(RefCell::new(Vec::<String>::new()));
        let (picked, answered) = (said.clone(), said.clone());
        // A dialog asked while a pick is open: the pick answers none before the dialog is there.
        pick("Branch")
            .items(vec![item("a", "A")])
            .open(ui, move |i| picked.borrow_mut().push(format!("pick:{}", i.map_or("none".into(), |i| i.id))));
        dialog::ask("Delete the branch?")
            .button("Delete")
            .open(ui, move |a| answered.borrow_mut().push(format!("dialog:{:?}", a.button)));
        assert!(!is_open(ui) && dialog::is_open(ui));
        assert_eq!(*said.borrow(), ["pick:none"]);
        // A pick opened while a dialog is open: the dialog is cancelled.
        input("Name").open(ui, |_| {});
        assert!(is_open(ui) && !dialog::is_open(ui));
        assert_eq!(*said.borrow(), ["pick:none", "dialog:None"]);
    }

    #[test]
    fn the_caller_may_rank_and_what_is_typed_may_be_taken_as_it_is() {
        let ui = Ui::new();
        let taken = Rc::new(RefCell::new(None));
        let done = taken.clone();
        pick("Go to File")
            .rank(|typed| (0..500).map(|i| item(&format!("f{i}"), &format!("{typed}{i}.rs"))).collect())
            .free(|typed| (!typed.contains(' ')).then(|| item("free", &format!("Go to {typed}"))))
            .most(50)
            .open(ui, move |i| *done.borrow_mut() = i.map(|i| i.id));
        let host = ui.state::<Host>();
        assert_eq!((shown(ui).len(), host.more.get_untracked()), (50, 450), "nothing typed: free is not asked");
        type_in(ui, "  ab ");
        assert_eq!(shown(ui)[0], "ab0.rs", "the caller is given what is typed without the spaces around it");
        assert_eq!(shown(ui).last().map(String::as_str), Some("Go to ab"));
        type_in(ui, "a b");
        assert_eq!(shown(ui).len(), 50, "free gave no row");
        type_in(ui, "ab");
        press(ui, Press::Up);
        press(ui, Press::Take);
        assert_eq!(*taken.borrow(), Some("free".to_string()));
        // `empty`: free is asked while nothing is typed too.
        pick("Branch").free(|typed| Some(item("typed", typed))).empty().open(ui, |_| {});
        assert_eq!(shown(ui), [""]);
    }

    #[test]
    fn an_input_box_takes_what_is_typed_unless_something_is_wrong_with_it() {
        let ui = Ui::new();
        let host = ui.state::<Host>();
        let taken = Rc::new(RefCell::new(Vec::<Option<String>>::new()));
        let done = taken.clone();
        input("Rename Branch")
            .value("main")
            .validate(|v| v.contains(' ').then(|| "A name has no spaces.".to_string()))
            .open(ui, move |v| done.borrow_mut().push(v));
        assert_eq!(host.text.get_untracked(), "main");
        assert!(shown(ui).is_empty(), "an input box has no rows");
        let asked = host.open.get_untracked().expect("open");
        type_in(ui, "my branch");
        assert_eq!(asked.wrong("my branch").as_deref(), Some("A name has no spaces."));
        press(ui, Press::Take);
        assert!(is_open(ui), "Enter does nothing while it is wrong");
        type_in(ui, "  topic ");
        assert_eq!(asked.wrong("  topic "), None, "checked as it is taken: without the spaces around it");
        press(ui, Press::Take);
        assert_eq!(*taken.borrow(), [Some("topic".to_string())]);
        // An empty answer is an answer; Escape is none.
        let done = taken.clone();
        input("Message").open(ui, move |v| done.borrow_mut().push(v));
        press(ui, Press::Take);
        let done = taken.clone();
        input("Message").open(ui, move |v| done.borrow_mut().push(v));
        press(ui, Press::Leave);
        assert_eq!(taken.borrow()[1..], [Some(String::new()), None]);
    }

    #[test]
    fn a_secret_is_taken_whole_and_its_keys_are_its_own() {
        let ui = Ui::new();
        let taken = Rc::new(RefCell::new(None));
        let done = taken.clone();
        input("Password for guest").password().open(ui, move |v| *done.borrow_mut() = v);
        type_in(ui, " p w ");
        press(ui, Press::Take);
        assert_eq!(taken.borrow().as_deref(), Some(" p w "), "with its spaces");
        let none = Modifiers::empty();
        let key = |text: &str, key: Key, m: Modifiers| secret_key(text, &key, m, || Some("pa\nsted".into()));
        assert_eq!(key("ab", Key::Character("c".into()), none).as_deref(), Some("abc"));
        assert_eq!(key("ab", Key::Character("C".into()), Modifiers::SHIFT).as_deref(), Some("abC"));
        assert_eq!(key("ab", Key::Named(NamedKey::Space), none).as_deref(), Some("ab "));
        assert_eq!(key("abé", Key::Named(NamedKey::Backspace), none).as_deref(), Some("ab"));
        assert_eq!(key("", Key::Named(NamedKey::Backspace), none).as_deref(), Some(""));
        assert_eq!(key("ab", Key::Named(NamedKey::Backspace), Modifiers::CONTROL).as_deref(), Some(""));
        assert_eq!(key("ab", Key::Character("v".into()), Modifiers::CONTROL).as_deref(), Some("abpasted"));
        assert_eq!(key("ab", Key::Character("c".into()), Modifiers::CONTROL), None, "a secret is not copied");
        assert_eq!(key("ab", Key::Named(NamedKey::ArrowLeft), none), None);
        assert_eq!(key("ab", Key::Character("x".into()), Modifiers::ALT), None);
    }

    #[test]
    fn ten_thousand_items_are_narrowed_in_a_moment() {
        let items: Vec<Item> =
            (0..10_000).map(|i| item(&i.to_string(), &format!("Item number {i}")).description("of many")).collect();
        let hay: Vec<String> = items.iter().map(hay_of).collect();
        let began = std::time::Instant::now();
        let fit = narrow(&items, &hay, "number 99 many");
        let (rows, more) = rows_of(fit, MOST, None);
        // A debug build on a busy machine: far under what a key may take even so.
        assert!(began.elapsed() < Duration::from_millis(250), "{:?}", began.elapsed());
        assert_eq!((rows.len(), more), (200, 80), "99, 199…, 990-999, 1990-1999…: 280 fit");
    }

    #[test]
    fn the_host_is_a_layer_of_the_window() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        register(ui, &send);
        assert!(layers::ids(Place::Over).contains(&"quick"));
    }
}
