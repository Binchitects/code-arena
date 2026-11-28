//! The compare editor, VS Code's diff editor: two texts side by side, each in an editor of its own (the original
//! left, the modified right), their lines that are the same beside each other (empty, hatched rows fill the side
//! that has fewer lines in a hunk), scrolled together; the lines that differ coloured (`diffEditor.removed…` and
//! `inserted…`) and, inside a changed line, the words that differ more strongly. A hunk of the modified side has
//! VS Code's arrow in its glyph margin, Revert Block, which puts the original's lines back. Narrower than VS
//! Code's 900 px (`diffEditor.renderSideBySideInlineBreakpoint`), or with `diffEditor.renderSideBySide` off or the
//! toolbar's toggle, the two are one column, the inline view.
//!
//! A side is one of:
//!
//! - the file as it is in its editor (`Live`), editable: its text is the open tab's, so Ctrl+S saves the file and
//!   its tab shows the change;
//! - the file as saved on disk (`Saved`, "a.rs (Saved)"), or another file (`File`), read with the driver's `open`
//!   (the reply is taken here, not opened as a tab) and read again when it changes on disk;
//! - the file before the agent changed it (`Before`, "a.rs (before the agent)"), from the Changes view's list;
//! - the clipboard's text, read when the compare is opened (`Clipboard`), or nothing (`Empty`).
//!
//! A compare is a tab of the kind `compare` (`editor::kinds`), its path `compare:` and the two sides
//! ("compare:saved:src/a.rs ↔ live:src/a.rs"); its title "a.rs (Saved) ↔ a.rs". Its two editors are not the
//! file's: their decorations and empty rows are kept under each side's own key (the tab's path, the side and the
//! file, so the language is still the file's), so the file's own tab shows none of them. The two texts are
//! compared off the window's thread (`align.rs`), again 120 ms after the last edit of either.
//!
//! The toolbar and the commands are in `toolbar.rs`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use floem::context::PaintCx;
use floem::kurbo::{Line as Segment, Rect, Stroke, Vec2};
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use floem::views::{Decorators, dyn_container, empty, h_stack, label, stack, v_stack};
use floem::{AnyView, IntoView, View, ViewId};
use floem_renderer::Renderer;
use serde_json::{Value, json};

use crate::decor::{Look, Mark, Tint};
use crate::editor::{folding, kinds};
use crate::state::{Change, Send, Tab, Ui};
use align::Compared;

pub mod align;
pub mod ranges;
pub mod toolbar;

/// The kind of tab a compare is.
pub const KIND: &str = "compare";

/// The owner of the compare's decorations (`decor.rs`).
const OWNER: &str = "compare";

/// Narrower than this, the two sides are one column (VS Code's `renderSideBySideInlineBreakpoint`).
const INLINE_BELOW: f64 = 900.0;

/// What one side of a compare shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The file as its editor has it, changes not saved with it.
    Live(String),
    /// The file as saved on disk.
    Saved(String),
    /// Another file, as on disk.
    File(String),
    /// The file before the agent changed it.
    Before(String),
    /// The clipboard's text when the compare was opened (the number tells two such compares apart).
    Clipboard(u64),
    Empty,
}

impl Source {
    /// How it is written in the tab's path.
    pub fn spec(&self) -> String {
        match self {
            Source::Live(p) => format!("live:{p}"),
            Source::Saved(p) => format!("saved:{p}"),
            Source::File(p) => format!("file:{p}"),
            Source::Before(p) => format!("before:{p}"),
            Source::Clipboard(n) => format!("clipboard:{n}"),
            Source::Empty => "empty".into(),
        }
    }

    pub fn parse(spec: &str) -> Option<Source> {
        if spec == "empty" {
            return Some(Source::Empty);
        }
        let (kind, rest) = spec.split_once(':')?;
        let p = rest.to_string();
        Some(match kind {
            "live" => Source::Live(p),
            "saved" => Source::Saved(p),
            "file" => Source::File(p),
            "before" => Source::Before(p),
            "clipboard" => Source::Clipboard(rest.parse().ok()?),
            _ => return None,
        })
    }

    /// The file it is of (none: the clipboard, nothing).
    pub fn file(&self) -> Option<&str> {
        match self {
            Source::Live(p) | Source::Saved(p) | Source::File(p) | Source::Before(p) => Some(p),
            Source::Clipboard(_) | Source::Empty => None,
        }
    }

    /// Its name in the tab's title and the toolbar: the file's name, with what it is after it.
    pub fn label(&self) -> String {
        let name = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
        match self {
            Source::Live(p) | Source::File(p) => name(p),
            Source::Saved(p) => format!("{} (Saved)", name(p)),
            Source::Before(p) => format!("{} (before the agent)", name(p)),
            Source::Clipboard(_) => "Clipboard".into(),
            Source::Empty => "Empty".into(),
        }
    }

    /// Whether it is read from disk with the driver's `open`.
    fn on_disk(&self) -> Option<&str> {
        match self {
            Source::Saved(p) | Source::File(p) => Some(p),
            _ => None,
        }
    }
}

/// The path of the tab comparing two sides.
pub fn path_of(left: &Source, right: &Source) -> String {
    format!("{KIND}:{} ↔ {}", left.spec(), right.spec())
}

/// A compare tab's two sides, from the rest of its path.
pub fn sides_of(rest: &str) -> Option<(Source, Source)> {
    let (l, r) = rest.split_once(" ↔ ")?;
    Some((Source::parse(l)?, Source::parse(r)?))
}

/// The tab's title: "a.rs (Saved) ↔ a.rs".
pub fn title_of(rest: &str) -> String {
    match sides_of(rest) {
        Some((l, r)) => format!("{} ↔ {}", l.label(), r.label()),
        None => rest.to_string(),
    }
}

/// One side: what it shows, its text, its syntax colours, and the key its marks and empty rows are kept under.
pub struct Half {
    pub source: Source,
    pub doc: Rc<TextDocument>,
    pub colours: Rc<crate::highlight::Colours>,
    pub key: String,
    /// Whether it is the file's own text (edited, saved).
    pub editable: bool,
}

/// A compare open in a tab.
pub struct Entry {
    pub path: String,
    pub left: Half,
    pub right: Half,
    /// The two texts as compared last.
    pub compared: RwSignal<Rc<Compared>>,
    /// The inline view chosen with the toolbar (none: by the setting and the width).
    pub inline: RwSignal<Option<bool>>,
    /// The compare's width, by which it is inline.
    pub width: RwSignal<f64>,
    /// The two editors on screen (none: not shown, or inline).
    pub editors: RefCell<Option<(Editor, Editor)>>,
    /// The comparisons asked for (an older one's answer is dropped), and the edits waited after.
    asked: Cell<u64>,
    edits: Cell<u64>,
}

impl Entry {
    /// The side whose text is the file's own, edited and saved (the modified one, or the original after a swap).
    pub fn live(&self) -> Option<&Half> {
        [&self.right, &self.left].into_iter().find(|h| h.editable)
    }
}

thread_local! {
    static OPEN: RefCell<HashMap<String, Rc<Entry>>> = RefCell::new(HashMap::new());
    /// The files asked of the driver for a compare, each with the compares waiting for it.
    static ASKED: RefCell<HashMap<String, Vec<String>>> = RefCell::new(HashMap::new());
    static CLIPS: Cell<u64> = const { Cell::new(0) };
}

/// A compare open, by its tab's path.
pub fn entry(path: &str) -> Option<Rc<Entry>> {
    OPEN.with(|o| o.borrow().get(path).cloned())
}

/// A compare whose tab is still open; one whose tab was closed is forgotten.
fn living(ui: Ui, path: &str) -> Option<Rc<Entry>> {
    let e = entry(path)?;
    if ui.tab(path).is_some() {
        return Some(e);
    }
    forget(ui, path);
    None
}

/// A compare forgotten: its sides' marks and empty rows with it.
fn forget(ui: Ui, path: &str) {
    let Some(e) = OPEN.with(|o| o.borrow_mut().remove(path)) else { return };
    for half in [&e.left, &e.right] {
        crate::decor::clear(ui, OWNER, &half.key);
        folding::set_fillers(&half.key, Vec::new());
    }
}

/// The compare in the tab shown.
pub fn shown(ui: Ui) -> Option<Rc<Entry>> {
    ui.active.get_untracked().and_then(|p| entry(&p))
}

/// A new number for a compare with the clipboard.
pub fn clip() -> u64 {
    CLIPS.with(|c| {
        c.set(c.get() + 1);
        c.get()
    })
}

/// Two sides compared in a tab, shown (the one already open, when it is).
pub fn open(ui: Ui, send: &Send, left: Source, right: Source) {
    open_with(ui, send, left, right, (None, None));
}

/// The same, with the texts of sides known already (the agent's change says both).
pub fn open_with(ui: Ui, send: &Send, left: Source, right: Source, texts: (Option<String>, Option<String>)) {
    // A file not open in an editor is compared as it is on disk.
    let settle = |s: Source| match s {
        Source::Live(p) if ui.tab(&p).is_none_or(|t| !t.is_file()) => Source::File(p),
        s => s,
    };
    let (left, right) = (settle(left), settle(right));
    let path = path_of(&left, &right);
    // One whose tab was closed is made again: the file's tab may be another since.
    if ui.tab(&path).is_none() {
        forget(ui, &path);
    }
    if entry(&path).is_none() {
        let file = left.file().or(right.file()).unwrap_or("text").to_string();
        let make = |source: Source, side: &str, text: Option<String>| -> Half {
            let key = format!("{path}#{side}/{file}");
            let live = match &source {
                Source::Live(p) => ui.tab(p),
                _ => None,
            };
            let (doc, editable) = match live {
                Some(tab) => (tab.doc.clone(), true),
                None => {
                    let text = text.or_else(|| known_text(ui, &source)).unwrap_or_default();
                    (Rc::new(TextDocument::new(ui.scope, text)), false)
                }
            };
            let colours = crate::highlight::Colours::new(doc.clone(), &key);
            folding::attach(ui, &key, &doc);
            Half { source, doc, colours, key, editable }
        };
        let e = Rc::new(Entry {
            path: path.clone(),
            left: make(left, "original", texts.0),
            right: make(right, "modified", texts.1),
            compared: ui.scope.create_rw_signal(Rc::new(Compared::default())),
            inline: ui.scope.create_rw_signal(None),
            width: ui.scope.create_rw_signal(0.0),
            editors: RefCell::new(None),
            asked: Cell::new(0),
            edits: Cell::new(0),
        });
        for half in [&e.left, &e.right] {
            if let Some(p) = half.source.on_disk()
                && known_text(ui, &half.source).is_none()
            {
                ask(send, p, &path);
            }
            let (colours, weak, p) = (half.colours.clone(), Rc::downgrade(&half.doc), path.clone());
            half.doc.add_on_update(move |u| {
                if let Some(doc) = weak.upgrade() {
                    let first = u.deltas().map(|x| x.summary().0.start()).min().unwrap_or(0);
                    colours.changed_from(doc.rope_text().line_of_offset(first));
                }
                soon(ui, &p);
            });
        }
        OPEN.with(|o| o.borrow_mut().insert(path.clone(), e.clone()));
        run(ui, &e);
    }
    kinds::open(ui, &path);
}

/// A side's text known without asking the driver: the open tab's when it has no changes not saved (what is on disk
/// then), the agent's change's, the clipboard's, nothing.
fn known_text(ui: Ui, s: &Source) -> Option<String> {
    match s {
        Source::Live(p) | Source::Saved(p) | Source::File(p) => {
            ui.tab(p).filter(|t| t.is_file() && !t.dirty.get_untracked()).map(|t| t.text())
        }
        Source::Before(p) => Some(
            ui.changes
                .with_untracked(|c| c.iter().find(|c| c.path == *p).and_then(|c| c.before.clone()))
                .unwrap_or_default(),
        ),
        Source::Clipboard(_) => Some(floem::Clipboard::get_contents().unwrap_or_default()),
        Source::Empty => Some(String::new()),
    }
}

/// A file asked of the driver for a compare.
fn ask(send: &Send, file: &str, compare: &str) {
    let first = ASKED.with(|a| {
        let mut asked = a.borrow_mut();
        let waiting = asked.entry(file.to_string()).or_default();
        let first = waiting.is_empty();
        if !waiting.iter().any(|w| w == compare) {
            waiting.push(compare.to_string());
        }
        first
    });
    if first {
        send(json!({"type": "open", "path": file}));
    }
}

/// A file's changes, as the agent made them: what it was before beside the file (in its editor when it is open).
pub fn open_change(ui: Ui, send: &Send, c: &Change) {
    let open = ui.tab(&c.path).is_some_and(|t| t.is_file());
    let right = if open { Source::Live(c.path.clone()) } else { Source::File(c.path.clone()) };
    let after = if open { None } else { Some(c.after.clone().unwrap_or_default()) };
    open_with(ui, send, Source::Before(c.path.clone()), right, (Some(c.before.clone().unwrap_or_default()), after));
}

/// The file a compare tab shows the agent's change of (its original is the file before the agent).
pub fn before_of(path: &str) -> Option<String> {
    let (kind, rest) = kinds::split(path)?;
    match sides_of(rest).filter(|_| kind == KIND)?.0 {
        Source::Before(p) => Some(p),
        _ => None,
    }
}

/// The agent's changes said again: a compare of one shows what the agent wrote since (the file's side when it is
/// not in an editor; one in an editor has it already).
pub fn changes_said(ui: Ui, now: &[Change]) {
    let open: Vec<Rc<Entry>> = OPEN.with(|o| o.borrow().values().cloned().collect());
    for e in open.into_iter().filter(|e| living(ui, &e.path).is_some()) {
        let Source::Before(p) = &e.left.source else { continue };
        let Some(c) = now.iter().find(|c| c.path == *p) else { continue };
        fill(&e.left.doc, c.before.as_deref().unwrap_or(""));
        if e.right.source == Source::File(p.clone()) {
            fill(&e.right.doc, c.after.as_deref().unwrap_or(""));
        }
    }
}

/// A side's text put in place of what it had (a file read, read again).
fn fill(doc: &TextDocument, text: &str) {
    let now = doc.text();
    if now.to_string() == text {
        return;
    }
    doc.edit_single(Selection::region(0, now.len()), text, EditType::Other);
}

/// The two texts compared again, once the edits have stopped for a moment.
fn soon(ui: Ui, path: &str) {
    let Some(e) = living(ui, path) else { return };
    let n = e.edits.get() + 1;
    e.edits.set(n);
    let path = path.to_string();
    floem::action::exec_after(Duration::from_millis(120), move |_| {
        if let Some(e) = entry(&path)
            && e.edits.get() == n
        {
            run(ui, &e);
        }
    });
}

/// The two texts compared off the window's thread; the answer is taken when it is the last asked for.
pub fn run(ui: Ui, e: &Rc<Entry>) {
    let n = e.asked.get() + 1;
    e.asked.set(n);
    let (left, right) = (e.left.doc.text().to_string(), e.right.doc.text().to_string());
    let trim = crate::settings::now(ui, "diffEditor.ignoreTrimWhitespace", None).as_bool().unwrap_or(true);
    let ms = crate::settings::now(ui, "diffEditor.maxComputationTime", None).as_f64().unwrap_or(5000.0);
    let most = (ms > 0.0).then(|| Duration::from_millis(ms as u64));
    let path = e.path.clone();
    let done = floem::ext_event::create_ext_action(ui.scope, move |c: Compared| {
        if let Some(e) = entry(&path)
            && e.asked.get() == n
        {
            apply(ui, &e, c);
        }
    });
    std::thread::spawn(move || done(align::compare(&left, &right, trim, most)));
}

/// A comparison shown: each side's lines and words marked, the revert arrows, the empty rows.
fn apply(ui: Ui, e: &Rc<Entry>, c: Compared) {
    // Its lines laid out again scroll each side to its caret: one off screen (the side not typed in, scrolled with
    // the other) is put on the first line shown first, so neither side jumps.
    if let Some((l, r)) = e.editors.borrow().clone() {
        for (ed, half) in [(&l, &e.left), (&r, &e.right)] {
            keep_in_view(ed, &half.key);
        }
    }
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let word = |w: &align::Word, bg: fn(&crate::colors::Colors) -> floem::peniko::Color| Mark::Range {
        from: (w.line, w.cols.start),
        to: (w.line, w.cols.end),
        look: Look { bg: Some(Tint::Theme(bg)), ..Look::default() },
        hover: None,
    };
    for h in &c.hunks {
        left.extend(h.left.clone().map(|line| Mark::Line { line, bg: Tint::Theme(|c| c.diff_removed) }));
        right.extend(h.right.clone().map(|line| Mark::Line { line, bg: Tint::Theme(|c| c.diff_added) }));
        if e.right.editable {
            let path = e.path.clone();
            right.push(Mark::Glyph {
                line: h.right.start.min(c.right_lines.saturating_sub(1)),
                icon: "arrow-right",
                tint: Tint::Theme(|c| c.icon),
                title: "Revert Block".into(),
                click: Some(Rc::new(move |ui, _send, line| revert_at(ui, &path, line))),
            });
        }
    }
    left.extend(c.left_words.iter().map(|w| word(w, |c| c.diff_removed_text)));
    right.extend(c.right_words.iter().map(|w| word(w, |c| c.diff_added_text)));
    let (l, r) = align::fillers(&c.hunks);
    folding::set_fillers(&e.left.key, l);
    folding::set_fillers(&e.right.key, r);
    for (half, marks) in [(&e.left, left), (&e.right, right)] {
        crate::decor::set(ui, OWNER, &half.key, marks);
        // The side is not a tab of its own, so the decorations do not lay its lines out again themselves.
        half.doc.cache_rev().update(|r| *r += 1);
    }
    e.compared.set(Rc::new(c));
}

/// An editor's caret put on its first line on screen when it is off screen.
fn keep_in_view(ed: &Editor, key: &str) {
    let height = f64::from(ed.line_height(0));
    let vp = ed.viewport.get_untracked();
    if height <= 0.0 || vp.height() <= 0.0 {
        return;
    }
    let line = ed.offset_to_line_col(ed.cursor.with_untracked(|c| c.offset())).0;
    let map = folding::map(key);
    let row = map.as_ref().map_or(line, |m| m.row_of_line(line)) as f64;
    let (first, last) = ((vp.y0 / height).ceil(), (vp.y1 / height).floor() - 1.0);
    if row >= first && row <= last {
        return;
    }
    let top = first as usize;
    let to = map.as_ref().map_or(top, |m| m.line_of_row(top)).min(ed.last_line());
    let at = ed.offset_of_line(to);
    ed.cursor.update(|c| c.set_insert(Selection::caret(at)));
}

/// Revert Block: the hunk of the modified side that begins on a line given back the original's lines.
pub fn revert_at(ui: Ui, path: &str, line: usize) {
    let Some(e) = entry(path) else { return };
    if !e.right.editable {
        return;
    }
    let c = e.compared.get_untracked();
    let last = c.right_lines.saturating_sub(1);
    let Some(h) = c.hunks.iter().find(|h| h.right.start.min(last) == line) else { return };
    let (left, right) = (e.left.doc.text().to_string(), e.right.doc.text().to_string());
    let (range, text) = align::revert(&left, &right, h);
    e.right.doc.edit_single(Selection::region(range.start, range.end), &text, EditType::Other);
    let _ = ui;
}

/// The caret of both sides put on the next change (or the previous), round the end; the modified side's caret
/// says where from.
pub fn go(_ui: Ui, path: &str, forward: bool) {
    let Some(e) = entry(path) else { return };
    let Some((l, r)) = e.editors.borrow().clone() else { return };
    let c = e.compared.get_untracked();
    let line = r.offset_to_line_col(r.cursor.with_untracked(|c| c.offset())).0;
    let Some(i) = align::next(&c.hunks, line, forward) else { return };
    let h = &c.hunks[i];
    for (ed, line) in [(&l, h.left.start), (&r, h.right.start)] {
        let at = ed.offset_of_line(line.min(ed.last_line()));
        ed.cursor.update(|c| c.set_insert(Selection::caret(at)));
    }
    folding::center(&r, &e.right.key);
}

/// Whether the compare is one column now.
fn is_inline(ui: Ui, e: &Entry) -> bool {
    e.inline.get().unwrap_or_else(|| {
        !crate::settings::flag(ui, "diffEditor.renderSideBySide", None) || e.width.get() < INLINE_BELOW
    })
}

/// What a compare tab draws: its toolbar, then the two sides (or the inline view).
fn view(ui: Ui, send: Send, tab: &Tab) -> AnyView {
    let Some(e) = entry(&tab.path) else {
        return label(|| "This compare is no longer open.")
            .style(move |s| s.padding(20.0).color(ui.colors.get().desc))
            .into_any();
    };
    let theme = crate::highlight::theme_of(&ui.theme_json());
    e.left.colours.set_theme(theme.clone());
    e.right.colours.set_theme(theme);
    let inline = {
        let e = e.clone();
        create_memo(move |_| is_inline(ui, &e))
    };
    let (body_e, width) = (e.clone(), e.width);
    let body = dyn_container(
        move || inline.get(),
        move |one| {
            if one {
                *body_e.editors.borrow_mut() = None;
                inline_view(ui, &body_e)
            } else {
                side_by_side(ui, &body_e)
            }
        },
    )
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
    let cleared = e.clone();
    v_stack((toolbar::bar(ui, send, e.clone(), inline), body))
        .on_resize(move |r| {
            if (width.get_untracked() - r.width()).abs() > 0.5 {
                width.set(r.width());
            }
        })
        .on_cleanup(move || *cleared.editors.borrow_mut() = None)
        .style(|s| s.size_full())
        .into_any()
}

/// The inline view: the lines that went and came, between those both have, as the Changes view's diff.
fn inline_view(ui: Ui, e: &Rc<Entry>) -> AnyView {
    let (compared, e) = (e.compared, e.clone());
    dyn_container(
        move || compared.get(),
        move |c| {
            let lines = align::inline(&e.left.doc.text().to_string(), &e.right.doc.text().to_string(), &c);
            crate::changes::diff_view(ui, Rc::new(lines)).into_any()
        },
    )
    .style(|s| s.size_full())
    .into_any()
}

/// The two editors side by side, scrolled together.
fn side_by_side(ui: Ui, e: &Rc<Entry>) -> AnyView {
    let (left, lv) = side(ui, e, &e.left);
    let (right, rv) = side(ui, e, &e.right);
    // The editor the window's commands (Find, Save, Go to Line) act on: the modified side, or the one clicked.
    ui.live.set(Some((e.path.clone(), right.clone())));
    for (from, to) in [(left.clone(), right.clone()), (right.clone(), left.clone())] {
        let (viewport, path) = (from.viewport, e.path.clone());
        create_effect(move |_| {
            let v = viewport.get();
            let o = to.viewport.get_untracked();
            if (v.y0 - o.y0).abs() > 0.5 {
                to.scroll_to.set(Some(Vec2::new(o.x0, v.y0)));
            }
        });
        let (focus, me) = (from.editor_view_focused, from.clone());
        create_effect(move |first: Option<()>| {
            focus.track();
            if first.is_some() && ui.live.with_untracked(|l| l.as_ref().is_none_or(|(_, ed)| ed.id() != me.id())) {
                ui.live.set(Some((path.clone(), me.clone())));
            }
        });
    }
    *e.editors.borrow_mut() = Some((left, right));
    let t = ui.colors;
    let line = empty().style(move |s| s.width(1.0).height_full().background(t.get().group_border));
    h_stack((lv, line, rv)).style(|s| s.size_full()).into_any()
}

/// One side's editor, with its empty rows hatched under its text.
fn side(ui: Ui, e: &Rc<Entry>, half: &Half) -> (Editor, AnyView) {
    let doc = crate::decor::doc(half.doc.clone(), &half.key);
    let editor = Editor::new(floem::reactive::Scope::current(), doc, half.colours.clone(), false);
    editor.read_only.set(!half.editable);
    let map = Rc::new(KeypressMap::default());
    let ed = folding::view(ui, editor.clone(), &half.doc, &half.key, move |editor, press, mods| {
        if crate::commands::waiting().is_some()
            && let KeyInput::Keyboard(key, _) = &press.key
        {
            let mut ran = false;
            crate::editor::tell(ui, |send| {
                ran = crate::commands::press(ui, send, key, mods) != crate::commands::Pressed::Free;
            });
            if ran {
                return CommandExecuted::Yes;
            }
        }
        if crate::editor::keys::editor_key(ui, press, mods) {
            return CommandExecuted::Yes;
        }
        match crate::editor::keys::default_key(&map, editor, press, mods) {
            CommandExecuted::No => {
                if let KeyInput::Keyboard(key, _) = &press.key
                    && crate::keys_hook::editor(key, mods)
                {
                    crate::editor::tell(ui, |send| crate::workbench::key(ui, send, key, mods));
                }
                CommandExecuted::No
            }
            done => done,
        }
    })
    .style(move |s| crate::editor::editor_look(s, &ui.colors.get(), true));
    let ed = crate::decor::around(ui, ed, &editor, &half.key);
    let hatch = Hatch { id: ViewId::new(), ui, editor: editor.clone(), key: half.key.clone() };
    let id = hatch.id;
    let (viewport, compared, colors) = (editor.viewport, e.compared, ui.colors);
    create_effect(move |_| {
        viewport.track();
        compared.track();
        colors.track();
        id.request_paint();
    });
    let hatch = hatch.style(|s| s.absolute().size_full());
    let view =
        stack((hatch, ed)).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).height_full()).into_any();
    (editor, view)
}

/// A side's empty rows, hatched as VS Code's (`diffEditor.diagonalFill`), under the editor's text.
struct Hatch {
    id: ViewId,
    ui: Ui,
    editor: Editor,
    key: String,
}

impl View for Hatch {
    fn id(&self) -> ViewId {
        self.id
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let Some(size) = self.id.get_size() else { return };
        let Some(map) = folding::map(&self.key) else { return };
        let color = self.ui.colors.get_untracked().diff_fill;
        let top = self.editor.viewport.get_untracked().y0;
        let height = f64::from(self.editor.line_height(0));
        let stroke = Stroke::new(1.0);
        for (row, n) in map.filler_rows() {
            let (y0, y1) = (row as f64 * height - top, (row + n) as f64 * height - top);
            if y1 < 0.0 || y0 > size.height {
                continue;
            }
            let rect = Rect::new(0.0, y0, size.width, y1);
            cx.save();
            cx.clip(&rect);
            let mut x = -(y1 - y0);
            while x < size.width {
                cx.stroke(&Segment::new((x, y1), (x + (y1 - y0), y0)), color, &stroke);
                x += 8.0;
            }
            cx.restore();
        }
    }
}

/// What it adds to the window's registries, once at start: its kind of tab; and every compare is compared again
/// when the ignoring of trim whitespace changes.
pub fn register(ui: Ui, _send: &Send) {
    kinds::add(kinds::kind(KIND, |_ui, rest| title_of(rest), view).icon("diff").save(|ui, send, tab| {
        if let Some(Source::Live(p)) = entry(&tab.path).and_then(|e| e.live().map(|h| h.source.clone())) {
            ui.save_tab(&p, send);
        }
    }));
    let trim = crate::settings::memo(ui, "diffEditor.ignoreTrimWhitespace", || None);
    create_effect(move |first: Option<()>| {
        trim.track();
        if first.is_some() {
            for e in OPEN.with(|o| o.borrow().values().cloned().collect::<Vec<_>>()) {
                if living(ui, &e.path).is_none() {
                    continue;
                }
                run(ui, &e);
            }
        }
    });
}

/// The session's messages that are this part's: a file asked for a compare (taken away, not opened as a tab), and
/// the files that changed on disk, read again for the compares that show them.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "file" => {
            let path = m["path"].as_str().unwrap_or("");
            let Some(waiting) = ASKED.with(|a| a.borrow_mut().remove(path)) else { return false };
            if let Some(err) = m["error"].as_str() {
                ui.message.set(format!("Could not compare {path}: {err}"));
                return true;
            }
            let text = m["text"].as_str().unwrap_or("");
            for e in waiting.iter().filter_map(|w| living(ui, w)) {
                for half in [&e.left, &e.right] {
                    if !half.editable && half.source.on_disk() == Some(path) {
                        fill(&half.doc, text);
                    }
                }
            }
            true
        }
        "stats" | "saved" => {
            let saved = [m.clone()];
            let items = if m["type"] == "saved" { &saved[..] } else { m["items"].as_array().map_or(&[][..], |a| a) };
            for it in items.iter().filter(|it| it["error"].is_null()) {
                let Some(path) = it["path"].as_str() else { continue };
                let shown: Vec<String> = OPEN.with(|o| {
                    o.borrow()
                        .values()
                        .filter(|e| [&e.left, &e.right].iter().any(|h| h.source.on_disk() == Some(path)))
                        .map(|e| e.path.clone())
                        .collect()
                });
                for compare in shown.into_iter().filter(|c| living(ui, c).is_some()) {
                    ask(send, path, &compare);
                }
            }
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compares_path_names_its_two_sides() {
        let (l, r) = (Source::Saved("src/a.rs".into()), Source::Live("src/a.rs".into()));
        let path = path_of(&l, &r);
        assert_eq!(path, "compare:saved:src/a.rs ↔ live:src/a.rs");
        let rest = kinds::split(&path).expect("a kind path").1;
        assert_eq!(sides_of(rest), Some((l, r)));
        assert_eq!(title_of(rest), "a.rs (Saved) ↔ a.rs");
        let rest = kinds::split(&path_of(&Source::Clipboard(3), &Source::Live("b.md".into())))
            .expect("a kind path")
            .1
            .to_string();
        assert_eq!(title_of(&rest), "Clipboard ↔ b.md");
        assert_eq!(Source::Before("x/y.py".into()).label(), "y.py (before the agent)");
        assert_eq!(Source::parse("empty"), Some(Source::Empty));
        assert_eq!(Source::parse("what:x"), None);
    }

    #[test]
    fn compare_with_saved_reads_the_disk_and_reverts_into_the_files_own_text() {
        let ui = Ui::new();
        let asked = Rc::new(RefCell::new(Vec::<Value>::new()));
        let kept = asked.clone();
        let send: Send = Rc::new(move |v: Value| kept.borrow_mut().push(v));
        register(ui, &send);
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "one\ntwo\nthree\n", "modified": 1}), &send);
        let tab = ui.tab("a.rs").expect("the file is open");
        tab.doc.edit_single(Selection::region(4, 7), "2", EditType::Other);
        assert!(tab.dirty.get_untracked());
        asked.borrow_mut().clear();
        open(ui, &send, Source::Saved("a.rs".into()), Source::Live("a.rs".into()));
        let path = "compare:saved:a.rs ↔ live:a.rs";
        assert_eq!(ui.active.get_untracked().as_deref(), Some(path));
        assert_eq!(*asked.borrow(), [json!({"type": "open", "path": "a.rs"})], "the saved text is asked of the disk");
        // The reply is the compare's: taken away, not the file's tab's.
        let reply = json!({"type": "file", "path": "a.rs", "text": "one\ntwo\nthree\n", "modified": 1});
        assert!(take(ui, &reply, &send));
        let e = entry(path).expect("the compare is open");
        assert_eq!(e.left.doc.text().to_string(), "one\ntwo\nthree\n");
        assert!(Rc::ptr_eq(&e.right.doc, &tab.doc), "the modified side is the file's own text");
        // Revert Block puts the saved line back into the file's text.
        apply(ui, &e, align::compare("one\ntwo\nthree\n", "one\n2\nthree\n", true, None));
        assert_eq!(e.compared.get_untracked().hunks.len(), 1);
        revert_at(ui, path, 1);
        assert_eq!(tab.text(), "one\ntwo\nthree\n");
        // Save saves the file.
        asked.borrow_mut().clear();
        ui.save(&send);
        assert_eq!(asked.borrow()[0]["type"], "save");
        assert_eq!(asked.borrow()[0]["path"], "a.rs");
        // Closed, the file closed and opened again, and compared again: the modified side is the new tab's text.
        ui.close_now(path);
        ui.close_now("a.rs");
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "fresh\n", "modified": 2}), &send);
        let tab = ui.tab("a.rs").expect("the file is open again");
        open(ui, &send, Source::Saved("a.rs".into()), Source::Live("a.rs".into()));
        let e = entry(path).expect("the compare is open again");
        assert!(Rc::ptr_eq(&e.right.doc, &tab.doc));
        assert_eq!(e.left.doc.text().to_string(), "fresh\n", "a tab with no changes is what is saved");
    }
}
