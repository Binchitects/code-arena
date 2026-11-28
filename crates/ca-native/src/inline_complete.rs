//! The model's completion in the editor, as ghost text taken with Tab (`complete::ghost`). Asked 300 ms after typing
//! pauses, with the 6000 characters before the caret and the 2000 after it (`complete{id, path, prefix, suffix}`, the
//! driver's `ide_complete`); the answer (`completion{id, text}`) is shown only when it is still the newest asked and
//! the caret and the text are where they were. Not asked after nothing but blanks, in a file over 1 MB, with a
//! selection, or while the agent works (asked once it is done); answers are kept for the same text around the caret,
//! and typing the ghost's own next letters keeps the rest of it without asking again. A key past its limit (429)
//! rests the asking for 30 s. On when `codeArena.inlineCompletion.enabled` (the person's own) and
//! `editor.inlineSuggest.enabled` are; the status bar's "Complete" / "Complete off" turns it on and off, and pulses
//! while an answer is awaited.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use floem::action::exec_after;
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::editor::Editor;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::cursor::CursorMode;
use floem::views::{Decorators, h_stack, label, tooltip};
use floem::IntoView;
use serde_json::{Value, json};

use crate::ext::State;
use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::statusbar::{self, Side};

/// How long typing must pause before a completion is asked for.
const PAUSE: Duration = Duration::from_millis(300);
/// The most of the text before the caret and after it sent, in characters.
const MOST_PREFIX: usize = 6_000;
const MOST_SUFFIX: usize = 2_000;
/// No completion in a file larger than this, in bytes.
const MOST_FILE: usize = 1_000_000;
/// How long the asking rests after the key's limit, when the gateway does not say.
const REST: Duration = Duration::from_secs(30);
/// The answers kept, the newest last.
const MOST_KEPT: usize = 32;
/// The status item's pulse while an answer is awaited.
const PULSE: Duration = Duration::from_millis(500);

pub const SETTING: &str = "codeArena.inlineCompletion.enabled";
const SUGGEST: &str = "editor.inlineSuggest.enabled";

#[derive(Clone, Copy)]
struct Inline {
    /// An answer is awaited.
    waiting: RwSignal<bool>,
    /// The pulse's half shown, while waiting.
    dim: RwSignal<bool>,
    /// What the last answer said that went wrong (the item's tooltip).
    error: RwSignal<String>,
}

impl State for Inline {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        Inline { waiting: s.create_rw_signal(false), dim: s.create_rw_signal(false), error: s.create_rw_signal(String::new()) }
    }
}

/// A request asked: its file, the caret's offset and the edits counted then, and its text's key.
#[derive(Clone)]
struct Asked {
    id: u64,
    path: String,
    offset: usize,
    edited: u64,
    key: u64,
}

/// A suggestion shown: its file, where it begins and its whole text (typing its next letters keeps the rest).
#[derive(Clone)]
struct Shown {
    path: String,
    start: usize,
    text: String,
}

thread_local! {
    /// The pause's count: only the last edit's timer asks.
    static PAUSED: Cell<u64> = const { Cell::new(0) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
    static ASKED: RefCell<Option<Asked>> = const { RefCell::new(None) };
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
    static KEPT: RefCell<VecDeque<(u64, String)>> = const { RefCell::new(VecDeque::new()) };
    static RESTING: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The agent was working when it was time to ask: asked once it is done.
    static PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Whether completions are on for a file's language.
fn enabled(ui: Ui, path: &str) -> bool {
    let lang = crate::language::of(ui, path);
    crate::settings::now(ui, SETTING, None).as_bool() != Some(false)
        && crate::settings::now(ui, SUGGEST, lang).as_bool() != Some(false)
}

/// The editor shown, when its cursor is a caret (no selection, one cursor).
fn live(ui: Ui) -> Option<(String, Editor, usize)> {
    let (path, editor) = ui.live.get_untracked()?;
    let caret = editor.cursor.try_with_untracked(|c| match c.map(|c| (&c.mode, c.offset())) {
        Some((CursorMode::Insert(s), at)) if s.is_caret() && s.regions().len() == 1 => Some(at),
        _ => None,
    })?;
    Some((path, editor, caret))
}

/// The last `most` characters of a text.
fn tail(text: &str, most: usize) -> &str {
    if most == 0 {
        return "";
    }
    text.char_indices().rev().nth(most - 1).map_or(text, |(at, _)| &text[at..])
}

/// The first `most` characters of a text.
fn head(text: &str, most: usize) -> &str {
    text.char_indices().nth(most).map_or(text, |(at, _)| &text[..at])
}

/// The text before the caret and after it, cut to their most (read from whole lines, then cut by characters).
fn around(editor: &Editor, offset: usize) -> (String, String) {
    let rope = editor.rope_text();
    let len = rope.len();
    let from = rope.offset_of_line(rope.line_of_offset(offset.saturating_sub(MOST_PREFIX * 4)));
    let to = rope.line_end_offset(rope.line_of_offset((offset + MOST_SUFFIX * 4).min(len)), true).max(offset);
    let before = rope.slice_to_cow(from..offset);
    let after = rope.slice_to_cow(offset..to.min(len));
    (tail(&before, MOST_PREFIX).to_string(), head(&after, MOST_SUFFIX).to_string())
}

/// What an answer is kept by: its file and the text around the caret.
fn key_of(path: &str, prefix: &str, suffix: &str) -> u64 {
    let mut h = DefaultHasher::new();
    (path, prefix, suffix).hash(&mut h);
    h.finish()
}

fn kept(key: u64) -> Option<String> {
    KEPT.with(|k| k.borrow().iter().rev().find(|(k, _)| *k == key).map(|(_, t)| t.clone()))
}

fn keep(key: u64, text: &str) {
    KEPT.with(|k| {
        let mut k = k.borrow_mut();
        k.retain(|(had, _)| *had != key);
        k.push_back((key, text.to_string()));
        while k.len() > MOST_KEPT {
            k.pop_front();
        }
    });
}

/// What of a suggestion is left when what was typed since it began is its own start: the rest, when some is left.
fn typed_through<'a>(text: &'a str, typed: &str) -> Option<&'a str> {
    (!typed.is_empty() && typed.len() < text.len()).then(|| text.strip_prefix(typed)).flatten()
}

/// Whether the text before the caret gives the model nothing to go on.
fn blank(prefix: &str) -> bool {
    prefix.trim().is_empty()
}

/// Shows a suggestion at the caret of the editor shown.
fn show(ui: Ui, path: &str, editor: &Editor, offset: usize, start: usize, text: &str, whole: &str) {
    let (line, col) = editor.offset_to_line_col(offset);
    crate::complete::ghost(ui, path, line, col, text);
    SHOWN.with(|s| *s.borrow_mut() = Some(Shown { path: path.to_string(), start, text: whole.to_string() }));
}

fn set_if<T: PartialEq + Clone + 'static>(s: RwSignal<T>, v: T) {
    if s.with_untracked(|now| *now != v) {
        s.set(v);
    }
}

/// An edit of an open file: the ghost's next letters typed keep the rest of it; otherwise a completion is asked
/// once typing pauses.
fn edited(ui: Ui, send: &Send) {
    let n = PAUSED.with(|p| {
        p.set(p.get() + 1);
        p.get()
    });
    // The caret moves after the edit is told: read on the next turn of the loop.
    exec_after(Duration::ZERO, move |_| through(ui));
    let send = send.clone();
    exec_after(PAUSE, move |_| {
        if PAUSED.with(Cell::get) == n {
            ask(ui, &send);
        }
    });
}

/// The ghost's next letters typed: the rest of it shown at once.
fn through(ui: Ui) {
    let Some(shown) = SHOWN.with(|s| s.borrow().clone()) else { return };
    let Some((path, editor, offset)) = live(ui) else { return };
    let rest = (path == shown.path && offset > shown.start)
        .then(|| {
            let typed = editor.rope_text().slice_to_cow(shown.start..offset).to_string();
            typed_through(&shown.text, &typed).map(str::to_string)
        })
        .flatten();
    match rest {
        Some(rest) => {
            if crate::complete::ghost_shown().is_none_or(|(p, o, t)| p != path || o != offset || t != rest) {
                show(ui, &path, &editor, offset, shown.start, &rest, &shown.text);
            }
        }
        None => {
            SHOWN.with(|s| s.borrow_mut().take());
        }
    }
}

/// Asks for a completion at the caret of the editor shown, when it may be asked.
fn ask(ui: Ui, send: &Send) {
    let Some((path, editor, offset)) = live(ui) else { return };
    if !enabled(ui, &path) || editor.rope_text().len() > MOST_FILE {
        return;
    }
    if crate::complete::ghost_shown().is_some_and(|(p, o, _)| p == path && o == offset) {
        return;
    }
    if RESTING.with(Cell::get).is_some_and(|until| Instant::now() < until) {
        return;
    }
    if ui.running.get_untracked() {
        PENDING.with(|p| p.set(true));
        return;
    }
    let (prefix, suffix) = around(&editor, offset);
    if blank(&prefix) {
        return;
    }
    let key = key_of(&path, &prefix, &suffix);
    if let Some(text) = kept(key) {
        if !text.is_empty() {
            show(ui, &path, &editor, offset, offset, &text, &text);
        }
        return;
    }
    let id = NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let asked = Asked { id, path: path.clone(), offset, edited: ui.edited.get_untracked(), key };
    ASKED.with(|a| *a.borrow_mut() = Some(asked));
    send(json!({"type": "complete", "id": id, "path": path, "prefix": prefix, "suffix": suffix}));
    let me = ui.state::<Inline>();
    if !me.waiting.get_untracked() {
        me.waiting.set(true);
        pulse(ui, me);
    }
}

/// The status item's pulse, while an answer is awaited (none when motion is reduced).
fn pulse(ui: Ui, me: Inline) {
    exec_after(PULSE, move |_| {
        let on = me.waiting.get_untracked();
        set_if(me.dim, on && !crate::accessibility::still(ui) && !me.dim.get_untracked());
        if on {
            pulse(ui, me);
        }
    });
}

/// An answer: shown when it is the newest asked and the caret and the text are where they were.
fn answered(ui: Ui, m: &Value) {
    let Some(asked) = ASKED.with(|a| a.borrow().clone()) else { return };
    if m["id"].as_u64() != Some(asked.id) {
        return;
    }
    ASKED.with(|a| a.borrow_mut().take());
    let me = ui.state::<Inline>();
    set_if(me.waiting, false);
    set_if(me.dim, false);
    if let Some(error) = m["error"].as_str() {
        if let Some(secs) = m["retry_after"].as_u64() {
            let rest = if secs == 0 { REST } else { Duration::from_secs(secs) };
            RESTING.with(|r| r.set(Some(Instant::now() + rest)));
        }
        set_if(me.error, error.to_string());
        return;
    }
    set_if(me.error, String::new());
    let text = m["text"].as_str().unwrap_or("");
    keep(asked.key, text);
    let Some((path, editor, offset)) = live(ui) else { return };
    if text.is_empty() || path != asked.path || offset != asked.offset || ui.edited.get_untracked() != asked.edited {
        return;
    }
    show(ui, &path, &editor, offset, offset, text, text);
}

/// Turns completions on or off, as the status item does (on, inline suggestions are turned on too).
fn toggle(ui: Ui, send: &Send) {
    let on = crate::settings::now(ui, SETTING, None).as_bool() != Some(false);
    send(json!({"type": "set_setting", "key": SETTING, "value": !on}));
    if !on && crate::settings::now(ui, SUGGEST, None).as_bool() == Some(false) {
        send(json!({"type": "set_setting", "key": SUGGEST, "value": true}));
    }
    if on && let Some((path, _)) = ui.live.get_untracked() {
        crate::complete::ghost_clear(ui, &path);
    }
}

/// The status item's words and tooltip.
fn words(on: bool) -> &'static str {
    if on { "Complete" } else { "Complete off" }
}

fn tip(on: bool, error: &str) -> String {
    let said = if on {
        "Code completion is on: what you type around the cursor is sent to the model, and its answer shows as grey \
         text (Tab takes it). Click to turn it off."
    } else {
        "Code completion is off. Click to turn it on."
    };
    if error.is_empty() { said.to_string() } else { format!("{said}\n\nThe last completion failed: {error}") }
}

/// What it adds to the window's registries, once at start.
pub fn register(ui: Ui, send: &Send) {
    {
        let send = send.clone();
        // Not on the first run: that is the effect made, not an edit.
        ui.scope.create_effect(move |was: Option<()>| {
            ui.edited.track();
            if was.is_some() {
                edited(ui, &send);
            }
        });
    }
    {
        let send = send.clone();
        ui.scope.create_effect(move |was: Option<bool>| {
            let running = ui.running.get();
            if was == Some(true) && !running && PENDING.with(|p| p.replace(false)) {
                ask(ui, &send);
            }
            running
        });
    }
    statusbar::add("status.complete", Side::Right, 270, |ui, send| {
        let t = ui.colors;
        let me = ui.state::<Inline>();
        let on = ui.scope.create_memo(move |_| crate::settings::setting(ui, SETTING, None).as_bool() != Some(false));
        let editor = ui.scope.create_memo(move |_| ui.active.with(Option::is_some));
        let item = h_stack((icon("sparkle", 14.0), label(move || words(on.get()))))
            .style(|s| s.gap(4.0).items_center())
            .on_click_stop(move |_| toggle(ui, &send));
        tooltip(item, move || label(move || me.error.with(|e| tip(on.get(), e))))
            .style(move |s| {
                statusbar::item(t, s)
                    .cursor(CursorStyle::Pointer)
                    .apply_if(me.dim.get(), |s| s.color(t.get().status_fg.multiply_alpha(0.5)))
                    .apply_if(!editor.get(), |s| s.hide())
            })
            .into_any()
    });
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "completion" {
        answered(ui, m);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_around_the_caret_is_cut_by_characters() {
        assert_eq!(tail("abcdé", 2), "dé");
        assert_eq!(tail("ab", 5), "ab");
        assert_eq!(tail("ab", 0), "");
        assert_eq!(head("éabc", 2), "éa");
        assert_eq!(head("ab", 5), "ab");
    }

    #[test]
    fn nothing_is_asked_after_blanks_only() {
        assert!(blank("  \n\t"));
        assert!(blank(""));
        assert!(!blank("\n  x"));
    }

    #[test]
    fn the_ghosts_next_letters_typed_keep_the_rest() {
        assert_eq!(typed_through("foo(bar)", "fo"), Some("o(bar)"));
        assert_eq!(typed_through("foo", "x"), None);
        assert_eq!(typed_through("foo", ""), None);
        assert_eq!(typed_through("foo", "foo"), None, "all of it typed: nothing left");
    }

    #[test]
    fn answers_are_kept_by_the_text_around_the_caret_newest_last() {
        let a = key_of("a.rs", "let x = ", ";");
        assert_ne!(a, key_of("b.rs", "let x = ", ";"));
        keep(a, "1");
        assert_eq!(kept(a).as_deref(), Some("1"));
        for n in 0..MOST_KEPT as u64 {
            keep(n + 1000, "");
        }
        assert_eq!(kept(a), None, "the oldest goes first");
    }

    #[test]
    fn the_status_item_says_whether_it_is_on() {
        assert_eq!(words(true), "Complete");
        assert_eq!(words(false), "Complete off");
        assert!(tip(true, "").ends_with("Click to turn it off."));
        assert!(tip(true, "slow down").contains("The last completion failed: slow down"));
    }
}
