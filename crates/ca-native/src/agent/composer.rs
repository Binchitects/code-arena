//! The chat's input, as the old Code Arena's: a box of several lines that grows with what is typed (to 280 px),
//! with its toolbar under the text (the pickers; Stop, and a round button that sends), and a line of hints under
//! the box where there is room for it. Enter sends, Shift+Enter begins a new line; while an answer is written the
//! message waits for it ("Queue"). ↑ and ↓ bring back what was sent before: this session's messages, then what
//! this folder sent (the terminal's too). Escape goes back to what was being typed, then stops the turn.
//!
//! While there is no thread yet the box stands in the middle of the view, three rows tall, under "What should we
//! work on?" and over three cards that each send a first message at once. It is the same box when the thread
//! begins: what is typed in it, and the keys, stay.
//!
//! What is typed is `ui.draft`, kept for the folder with the window's look (`view_state.draft`), so it is there
//! again after a restart. A message the session refuses comes back into the box.
//!
//! The rest of the window reaches the box through `chat_bridge` (installed here), and the chat's other parts add
//! what they draw to the places around it (`slots.rs`).

use std::borrow::Cow;
use std::rc::Rc;
use std::time::{Duration, Instant};

use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::Rect;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::{CursorStyle, FlexDirection, Position, TextOverflow};
use floem::text::{FamilyOwned, Weight};
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::mode::Mode;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::id::EditorId;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::keypress::press::KeyPress;
use floem::views::editor::text::{Styling, WrapMethod};
use floem::views::{
    Decorators, container, empty, h_stack, h_stack_from_iter, label, stack, text_editor_keys, tooltip, v_stack,
    v_stack_from_iter,
};
use floem::{AnyView, IntoView, View, ViewId};
use serde_json::{Value, json};

use super::pickers::pill;
use super::slots::{self, Slot};
use crate::chat_bridge::{self, Bridge, Caret, Piece};
use crate::commands::{self, command};
use crate::icons::icon;
use crate::model::Kind;
use crate::state::{Picker, Send, Ui};
use crate::widgets::{Theme, UI_FONT, bold};

/// The box's text: its size and the height of a row of it.
const FONT: usize = 13;
const ROW: f64 = 18.0;
/// The tallest the text grows before it scrolls.
const TALLEST: f64 = 280.0;
/// The rows the box has with nothing typed: in the middle of the view before there is a thread, and under one.
const ROWS_AT_FIRST: usize = 3;
const ROWS_UNDER_A_THREAD: usize = 1;
/// The widest the box and what is around it are while there is no thread.
const WIDEST_AT_FIRST: f64 = 672.0;
/// From this width on, the cards stand side by side.
const CARDS_IN_A_ROW: f64 = 720.0;
/// The view's widths from which the line of hints has room: its short form, and all of it.
const HINTS_SHORT: f64 = 440.0;
const HINTS_WHOLE: f64 = 600.0;

const ASK: &str = "Ask Code Arena to read, change or run something";
const ASK_MEANWHILE: &str = "Code Arena is working… type the next message: it is sent when this answer ends";

/// What a card of the empty view sends: its icon and its words.
const FIRST_MESSAGES: [(&str, &str); 3] = [
    ("beaker", "How is this project built and tested? Run the tests and tell me what fails."),
    ("checklist", "Find the TODOs and FIXMEs in the code and sum them up by area."),
    ("diff", "Review the uncommitted changes (git diff) and point out problems."),
];

/// The modes the session takes, each with what it lets run without asking.
const MODES: [(&str, &str); 4] = [
    ("ask", "edits and commands ask first"),
    ("auto-edit", "file edits run without asking; commands ask"),
    ("plan", "read-only: no edits, no commands"),
    ("yolo", "nothing asks: edits and commands run at once"),
];

/// The command that stops the turn, under VS Code's id.
pub const STOP: &str = "workbench.action.chat.cancel";

/// The command that chooses a mode.
pub fn mode_command(mode: &str) -> String {
    format!("codeArena.agent.mode.{mode}")
}

/// How long the folder's history is taken as it came before ↑ asks for it again.
const HISTORY_FRESH: Duration = Duration::from_secs(30);
/// The most messages sent that wait to hear whether the session took them.
const MOST_WAITING: usize = 32;
/// How long after the last key what is typed is kept for the folder.
const KEEP_AFTER: Duration = Duration::from_millis(800);

/// A message sent and not yet answered: its key, what was typed for it (empty: nothing of the box's), and the
/// message as it went.
#[derive(Clone, Debug, PartialEq)]
struct Waiting {
    key: String,
    typed: String,
    msg: Value,
}

/// The box's own state.
#[derive(Clone, Copy)]
pub struct Composer {
    /// The box on screen: its editor (none while the Agent view is not drawn).
    live: RwSignal<Option<Editor>>,
    /// The box's frame on screen (its border), for what is placed against it.
    frame: RwSignal<Option<ViewId>>,
    /// Whether the box has the keys.
    focused: RwSignal<bool>,
    /// The keys are to go to the box once it is drawn.
    focus_wanted: RwSignal<bool>,
    /// The rows the text takes as it is drawn (a long line wraps).
    rows: RwSignal<usize>,
    /// The view's width, in the steps the hints and the cards change at.
    width: RwSignal<Width>,
    /// Which message sent before is in the box (none: what the person is typing).
    at: RwSignal<Option<usize>>,
    /// What was being typed when ↑ first brought a message back.
    typing: RwSignal<String>,
    /// What this folder sent before, the newest first (the session's `history`).
    older: RwSignal<Rc<Vec<String>>>,
    /// When the folder's history was last asked for.
    asked: RwSignal<Option<Instant>>,
    /// The messages sent that the session has not answered yet.
    waiting: RwSignal<Vec<Waiting>>,
    /// The number of the next message sent, and what tells this window's messages from another's.
    next: RwSignal<u64>,
    window: RwSignal<u64>,
    /// What is typed as it was last kept for the folder.
    kept: RwSignal<String>,
}

impl crate::ext::State for Composer {
    fn new(ui: Ui) -> Composer {
        let s = ui.scope;
        // Replies go to every window of the session: this one's keys begin with a number of its own.
        let window = u64::from(std::process::id())
            ^ std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or_default();
        Composer {
            live: s.create_rw_signal(None),
            frame: s.create_rw_signal(None),
            focused: s.create_rw_signal(false),
            focus_wanted: s.create_rw_signal(false),
            rows: s.create_rw_signal(1),
            width: s.create_rw_signal(Width::Narrow),
            at: s.create_rw_signal(None),
            typing: s.create_rw_signal(String::new()),
            older: s.create_rw_signal(Rc::new(Vec::new())),
            asked: s.create_rw_signal(None),
            waiting: s.create_rw_signal(Vec::new()),
            next: s.create_rw_signal(1),
            window: s.create_rw_signal(window & 0xff_ffff),
            kept: s.create_rw_signal(String::new()),
        }
    }
}

/// The view's width, as far as what is drawn changes with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Width {
    /// No room for the hints.
    Narrow,
    /// Room for the hints without "↑ for what you sent before".
    Hints,
    /// Room for all the hints.
    AllHints,
    /// Room for the cards side by side.
    Wide,
}

fn width_of(px: f64) -> Width {
    if px >= CARDS_IN_A_ROW {
        Width::Wide
    } else if px >= HINTS_WHOLE {
        Width::AllHints
    } else if px >= HINTS_SHORT {
        Width::Hints
    } else {
        Width::Narrow
    }
}

/// The line of hints at a width (empty: there is no room for it).
fn hints(width: Width) -> &'static str {
    match width {
        Width::Narrow => "",
        Width::Hints => "Enter to send · Shift+Enter for a new line · / for skills and commands",
        Width::AllHints | Width::Wide => {
            "Enter to send · Shift+Enter for a new line · ↑ for what you sent before · / for skills and commands"
        }
    }
}

/// How tall the text is drawn: its rows, no fewer than the box has with nothing typed, no taller than it grows.
fn text_height(rows: usize, least: usize) -> f64 {
    (rows.max(least) as f64 * ROW).min(TALLEST)
}

/// A name with its first letter a capital, as the modes are named.
fn capital(s: &str) -> String {
    let mut rest = s.chars();
    rest.next().map(|c| c.to_uppercase().chain(rest).collect()).unwrap_or_default()
}

/// The line under "What should we work on?": the project and the mode in force.
fn welcome_line(folder: &str, mode: &str) -> String {
    let project = folder.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or("");
    let place = if project.is_empty() { "this folder" } else { project };
    let what = format!("Code Arena reads and changes the code in {place}, and runs commands.");
    match MODES.iter().find(|(name, _)| *name == mode) {
        Some((name, lets)) => format!("{what} Mode {}: {lets}.", capital(name)),
        None => what,
    }
}

/// One list for ↑, the newest first: this session's messages, then the folder's; none empty, and a text sent
/// twice once, where it was sent last.
fn recall_list(here: &[String], older: &[String]) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    for text in here.iter().chain(older).map(|t| t.trim()).filter(|t| !t.is_empty()) {
        if !all.iter().any(|had| had == text) {
            all.push(text.to_string());
        }
    }
    all
}

/// A key that may step through what was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Recall {
    Up,
    Down,
    Escape,
}

/// Where the caret is, for ↑ and ↓: on the text's first row as drawn, on its last, and whether the box holds a
/// one-line message just brought back, untouched (the arrows then step on at once, wrapped or not).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Place {
    first_row: bool,
    last_row: bool,
    untouched: bool,
}

/// What a key does to which message is in the box: none when the key is the box's own (the caret moves);
/// else the message to show (none: what was being typed).
fn recall_step(key: Recall, at: Option<usize>, count: usize, place: Place) -> Option<Option<usize>> {
    match key {
        Recall::Escape => at.map(|_| None),
        Recall::Up => {
            let next = at.map_or(0, |at| at + 1);
            (next < count && (place.untouched || place.first_row)).then_some(Some(next))
        }
        Recall::Down => {
            let at = at?;
            (place.untouched || place.last_row).then_some(at.checked_sub(1))
        }
    }
}

/// This session's own messages, the newest first: what the person typed, not what the session said to itself.
fn sent_here(ui: Ui) -> Vec<String> {
    ui.items.with_untracked(|items| {
        items
            .iter()
            .rev()
            .filter_map(|item| match &item.kind {
                Kind::User(q) | Kind::Queued { question: q, .. } if !q.carried && !q.note => {
                    Some(if q.typed.is_empty() { q.text.clone() } else { q.typed.clone() })
                }
                _ => None,
            })
            .collect()
    })
}

/// The box's text as it is now.
pub(crate) fn text(ui: Ui) -> String {
    match ui.state::<Composer>().live.get_untracked() {
        Some(editor) => editor.text().to_string(),
        None => ui.draft.get_untracked(),
    }
}

/// A part of an editor's text made another, the caret put after it.
fn edit(editor: &Editor, from: usize, to: usize, with: &str) {
    let len = editor.text().len();
    let to = to.min(len);
    let from = from.min(to);
    editor.doc().edit_single(Selection::region(from, to), with, EditType::Other);
    editor.cursor.update(|c| c.set_insert(Selection::caret(from + with.len())));
}

/// The box's text made this, the caret at its end.
fn set_text(ui: Ui, new: &str) {
    if let Some(editor) = ui.state::<Composer>().live.get_untracked() {
        let len = editor.text().len();
        edit(&editor, 0, len, new);
    }
    if ui.draft.with_untracked(|d| d != new) {
        ui.draft.set(new.to_string());
    }
}

/// A part of the box's text (from, to, in bytes) made another.
fn replace(ui: Ui, from: usize, to: usize, with: &str) {
    match ui.state::<Composer>().live.get_untracked() {
        Some(editor) => edit(&editor, from, to, with),
        None => ui.draft.update(|d| {
            let to = to.min(d.len());
            let from = from.min(to);
            if d.is_char_boundary(from) && d.is_char_boundary(to) {
                d.replace_range(from..to, with);
            }
        }),
    }
}

/// The keys go to the box, now if it is drawn, else once it is.
fn focus_soon(ui: Ui) {
    ui.state::<Composer>().focus_wanted.set(true);
    floem::action::exec_after(Duration::from_millis(30), move |_| focus_now(ui));
}

fn focus_now(ui: Ui) {
    let me = ui.state::<Composer>();
    if me.focus_wanted.get_untracked()
        && let Some(id) = me.live.get_untracked().and_then(|e| e.editor_view_id.get_untracked())
    {
        me.focus_wanted.set(false);
        id.request_focus();
    }
}

/// The chat shown, the keys in its box.
fn open_chat(ui: Ui) {
    ui.reveal(crate::layout::View::Agent);
    focus_soon(ui);
}

/// A message goes to the session: these words with what the chat's parts send with the next message (a quote,
/// lines of files, attachments). `typed`: what is put back in the box if the session refuses it. False: there was
/// nothing to send.
fn say(ui: Ui, send: &Send, words: &str, typed: &str) -> bool {
    let me = ui.state::<Composer>();
    let n = me.next.get_untracked();
    let key = format!("{:x}-{n}", me.window.get_untracked());
    let mut msg = json!({"type": "send", "text": words.trim(), "key": key});
    chat_bridge::fill(ui, &mut msg);
    if msg["text"].as_str().is_none_or(|t| t.trim().is_empty()) {
        // No part gave it words: what they took out of the box goes back.
        chat_bridge::came_back(ui, &msg);
        return false;
    }
    me.next.set(n + 1);
    me.waiting.update(|w| {
        w.push(Waiting { key, typed: typed.to_string(), msg: msg.clone() });
        if w.len() > MOST_WAITING {
            w.remove(0);
        }
    });
    send(msg);
    true
}

/// What is in the box is sent (while an answer is written: it waits for it), and the box emptied.
fn submit(ui: Ui, send: &Send) {
    let typed = text(ui).trim().to_string();
    if typed.is_empty() && !chat_bridge::more(ui) {
        return;
    }
    if say(ui, send, &typed, &typed) {
        set_text(ui, "");
        let me = ui.state::<Composer>();
        me.at.set(None);
        me.typing.set(String::new());
    }
}

/// The folder's history asked for: now, or unless it came a moment ago.
fn ask_history(ui: Ui, send: &Send, now: bool) {
    let me = ui.state::<Composer>();
    if now || me.asked.get_untracked().is_none_or(|at| at.elapsed() > HISTORY_FRESH) {
        me.asked.set(Some(Instant::now()));
        send(json!({"type": "history"}));
    }
}

/// ↑, ↓ or Escape in the box: true when it stepped through what was sent before.
fn recall(ui: Ui, send: &Send, key: Recall, place: Place) -> bool {
    let me = ui.state::<Composer>();
    let at = me.at.get_untracked();
    if at.is_none() && key == Recall::Up {
        ask_history(ui, send, false);
    }
    let list = recall_list(&sent_here(ui), &me.older.get_untracked());
    let Some(next) = recall_step(key, at, list.len(), place) else { return false };
    if at.is_none() {
        me.typing.set(text(ui));
    }
    let shown = match next {
        Some(n) => list[n].clone(),
        None => me.typing.get_untracked(),
    };
    me.at.set(next);
    set_text(ui, &shown);
    true
}

/// Where the caret is in an editor, for ↑ and ↓ (`shown`: the message brought back that the box holds, if any).
/// None while words are chosen: the arrows are the box's own then.
fn place_in(editor: &Editor, shown: Option<&str>) -> Option<Place> {
    let (offset, affinity, chosen) = editor.cursor.with_untracked(|c| (c.offset(), c.affinity, c.get_selection()));
    if chosen.is_some_and(|(from, to)| from != to) {
        return None;
    }
    let text = editor.text().to_string();
    let row = editor.vline_of_offset(offset, affinity).get();
    Some(Place {
        first_row: row == 0,
        last_row: row == editor.last_vline().get(),
        untouched: shown.is_some_and(|s| s == text && offset == text.len() && !text.contains('\n')),
    })
}

/// A key as the editor's own keymap has it.
fn own_key(map: &KeypressMap, editor: &Editor, press: &KeyPress, mods: Modifiers) -> CommandExecuted {
    let command = map.keymaps.get(press).or_else(|| {
        let insert = editor.cursor.with_untracked(|c| c.get_mode()) == Mode::Insert;
        let mut press = press.clone();
        press.mods.set(Modifiers::SHIFT, false);
        if insert { map.keymaps.get(&press) } else { None }
    });
    match command {
        Some(command) => editor.doc().run_command(editor, command, Some(1), mods),
        None => CommandExecuted::No,
    }
}

/// A key pressed in the box. The chat's parts are offered it first (a menu open over the box); then Enter sends,
/// ↑ and ↓ step through what was sent, Escape goes back to what was typed or stops the turn; the rest is the
/// editor's, and what it does not take with Ctrl or Alt held is the workbench's. While an input method composes,
/// every key is its own.
fn key_in_box(ui: Ui, send: &Send, map: &KeypressMap, editor: &Editor, press: &KeyPress, mods: Modifiers) -> bool {
    let KeyInput::Keyboard(key, _) = &press.key else {
        return own_key(map, editor, press, mods) == CommandExecuted::Yes;
    };
    let composing = editor.doc().preedit().preedit.with_untracked(|p| p.is_some());
    if !composing {
        if chat_bridge::key(ui, send, key, mods) {
            return true;
        }
        let held = Modifiers::CONTROL | Modifiers::ALT | Modifiers::META | Modifiers::SHIFT;
        if !mods.intersects(held) {
            let me = ui.state::<Composer>();
            let stepped = |which: Recall| {
                let list = recall_list(&sent_here(ui), &me.older.get_untracked());
                let shown = me.at.get_untracked().and_then(|at| list.get(at).cloned());
                place_in(editor, shown.as_deref()).is_some_and(|place| recall(ui, send, which, place))
            };
            match key {
                Key::Named(NamedKey::Enter) => {
                    submit(ui, send);
                    return true;
                }
                // A tab is no part of a message: the key is left to the menus over the box.
                Key::Named(NamedKey::Tab) => return true,
                Key::Named(NamedKey::ArrowUp) if stepped(Recall::Up) => return true,
                Key::Named(NamedKey::ArrowDown) if stepped(Recall::Down) => return true,
                Key::Named(NamedKey::Escape) => {
                    if !stepped(Recall::Escape) {
                        if ui.running.get_untracked() {
                            send(json!({"type": "stop"}));
                        } else {
                            crate::workbench::key(ui, send, key, mods);
                        }
                    }
                    return true;
                }
                _ => {}
            }
        }
    }
    match own_key(map, editor, press, mods) {
        CommandExecuted::No => {
            if !composing && crate::keys_hook::editor(key, mods) {
                crate::workbench::key(ui, send, key, mods);
            }
            false
        }
        _ => true,
    }
}

/// The box's text as it is styled: the workbench's font, and a caret only while the box has the keys.
struct BoxText {
    family: Vec<FamilyOwned>,
    focused: RwSignal<bool>,
}

impl Styling for BoxText {
    fn id(&self) -> u64 {
        0
    }

    fn font_size(&self, _: EditorId, _: usize) -> usize {
        FONT
    }

    fn line_height(&self, _: EditorId, _: usize) -> f32 {
        ROW as f32
    }

    fn font_family(&self, _: EditorId, _: usize) -> Cow<'_, [FamilyOwned]> {
        Cow::Borrowed(&self.family)
    }

    fn weight(&self, _: EditorId, _: usize) -> Weight {
        Weight::NORMAL
    }

    fn paint_caret(&self, _: EditorId, _: usize) -> bool {
        self.focused.get_untracked()
    }
}

/// What the thread's place shows while there is no thread yet: the question and, under it, the project and the
/// mode. The box stands right under it (`input`), then the cards.
pub(super) fn welcome(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let words = v_stack((
        label(|| "What should we work on?").style(move |s| bold(s).font_size(20.0).color(t.get().fg)),
        label(move || welcome_line(&ui.folder.get(), &ui.mode.get())).style(move |s| {
            s.color(t.get().desc)
                .margin_top(8.0)
                .line_height(1.5)
                .max_width_pct(100.0)
                .text_overflow(TextOverflow::Wrap)
        }),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0).max_width(WIDEST_AT_FIRST));
    container(words).style(|s| {
        s.size_full()
            .min_height(0.0)
            .min_width(0.0)
            .items_end()
            .justify_center()
            .padding_horiz(16.0)
            .padding_bottom(10.0)
    })
}

/// A round button of the box's toolbar: the one that sends (`primary`), or Stop. Dimmed while it is not `lit`.
fn round(t: Theme, name: &'static str, primary: bool, lit: impl Fn() -> bool + 'static) -> impl IntoView {
    container(icon(name, 14.0)).style(move |s| {
        let c = t.get();
        let (bg, fg, hover) = if primary {
            (c.button_bg, c.button_fg, c.button_hover)
        } else {
            (c.button2_bg, c.button2_fg, c.button2_hover)
        };
        let s = s.width(22.0).height(22.0).items_center().justify_center().border_radius(11.0);
        // The second colour is the box's own in most themes: a ring tells the button from it.
        let s = s.apply_if(!primary, |s| s.border(1.0).border_color(c.icon.multiply_alpha(0.35)));
        if lit() {
            s.color(fg).background(bg).cursor(CursorStyle::Pointer).hover(|s| s.background(hover))
        } else {
            s.color(fg.multiply_alpha(0.5)).background(bg.multiply_alpha(0.4))
        }
    })
}

/// The cards under the box while there is no thread: each sends its words at once.
fn cards(ui: Ui, send: Send, at_first: impl Fn() -> bool + Copy + 'static) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Composer>();
    let all = FIRST_MESSAGES.map(|(name, words)| {
        let send = send.clone();
        h_stack((
            icon(name, 16.0).style(move |s| s.color(t.get().link).margin_top(1.0).flex_shrink(0.0_f32)),
            label(move || words).style(|s| {
                s.font_family(UI_FONT.clone()).min_width(0.0).line_height(1.4).text_overflow(TextOverflow::Wrap)
            }),
        ))
        .on_click_stop(move |_| {
            // The keys stay in the box: what is typed there goes on.
            if say(ui, &send, words, "") {
                focus_soon(ui);
            }
        })
        .style(move |s| {
            let c = t.get();
            s.gap(8.0)
                .padding(10.0)
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .border(1.0)
                .border_radius(8.0)
                .border_color(c.widget_border)
                .background(c.widget)
                .color(c.desc)
                .cursor(CursorStyle::Pointer)
                .selectable(false)
                .hover(|s| s.color(c.fg).border_color(c.focus.multiply_alpha(0.6)))
        })
        .into_any()
    });
    let row = h_stack_from_iter(all).style(move |s| {
        let side_by_side = me.width.get() == Width::Wide;
        s.gap(8.0).flex_grow(1.0_f32).min_width(0.0).max_width(WIDEST_AT_FIRST).flex_direction(if side_by_side {
            FlexDirection::Row
        } else {
            FlexDirection::Column
        })
    });
    container(row).style(move |s| {
        let s = s.width_full().min_width(0.0).justify_center().padding_horiz(12.0).padding_bottom(12.0);
        if at_first() { s } else { s.hide() }
    })
}

/// The box: its text, with what is sent with the message over it and its toolbar under it; under the box the
/// hints and, while there is no thread, the cards. With it comes the view the keys go back to when a picker's
/// menu closes.
pub(super) fn input(ui: Ui, send: Send, anchor: RwSignal<Rect>) -> (AnyView, ViewId) {
    let t = ui.colors;
    let me = ui.state::<Composer>();
    let focused = me.focused;
    if focused.get_untracked() {
        focused.set(false);
    }
    // Whether there is a thread, told only when that changes.
    let none_yet = create_memo(move |_| ui.items.with(|l| l.is_empty()));
    let at_first = move || none_yet.get();
    // A turn that begins closes a picker left open.
    create_effect(move |_| {
        if ui.running.get() {
            ui.picker.set(None);
        }
    });
    // Whether there is something to send: words, or what a part of the chat sends without any. And whether an
    // answer is being written, told only when that changes (the session says it with every status).
    let can_send = create_memo(move |_| ui.draft.with(|d| !d.trim().is_empty()) || chat_bridge::more(ui));
    let running = create_memo(move |_| ui.running.get());

    let map = KeypressMap::default();
    let keys = send.clone();
    let field = text_editor_keys(ui.draft.get_untracked(), move |editor, press, mods| {
        let editor = editor.get_untracked();
        if key_in_box(ui, &keys, &map, &editor, press, mods) { CommandExecuted::Yes } else { CommandExecuted::No }
    })
    .styling(BoxText { family: FamilyOwned::parse_list(&UI_FONT).collect(), focused })
    .editor_style(move |s| {
        let c = t.get();
        s.hide_gutter(true)
            .wrap_method(WrapMethod::EditorWidth)
            .cursor_color(c.cursor)
            .selection_color(c.selection)
            .phantom_color(c.input_fg)
            .placeholder_color(c.placeholder)
            .preedit_underline_color(c.input_fg)
    })
    .style(move |s| {
        let c = t.get();
        let least = if at_first() { ROWS_AT_FIRST } else { ROWS_UNDER_A_THREAD };
        s.width_full()
            .height(text_height(me.rows.get(), least))
            .background(Color::TRANSPARENT)
            .set(floem::views::editor::Foreground, c.input_fg)
            .color(c.input_fg)
    });
    let editor = field.editor().clone();
    let back = editor.editor_view_id.get_untracked().unwrap_or_else(|| field.id());
    let id = editor.id();
    me.live.set(Some(editor.clone()));
    // The caret at the end of what was typed before the box was drawn.
    editor.cursor.update(|c| c.set_insert(Selection::caret(editor.text().len())));
    {
        // What is typed is the window's draft, and a draft set from outside the box is its text.
        let (ed, rev) = (editor.clone(), editor.doc().cache_rev());
        create_effect(move |_| {
            rev.track();
            let now = ed.text().to_string();
            if ui.draft.with_untracked(|d| *d != now) {
                ui.draft.set(now);
            }
        });
        let ed = editor.clone();
        create_effect(move |_| {
            let want = ui.draft.get();
            if ed.text().to_string() != want {
                let len = ed.text().len();
                edit(&ed, 0, len, &want);
            }
        });
        // The rows the text takes, as the editor lays it out (told when it is another number only).
        let (ed, rev) = (editor.clone(), editor.doc().cache_rev());
        create_effect(move |_| {
            rev.track();
            ed.screen_lines.track();
            ed.viewport.track();
            let rows = ed.last_vline().get() + 1;
            if me.rows.get_untracked() != rows {
                me.rows.set(rows);
            }
        });
        // Whether the box has the keys: its border and its caret say so.
        let (gained, lost, ed) = (editor.editor_view_focused, editor.editor_view_focus_lost, editor.clone());
        create_effect(move |before: Option<()>| {
            gained.track();
            if before.is_some() && !focused.get_untracked() {
                focused.set(true);
            }
        });
        create_effect(move |before: Option<()>| {
            lost.track();
            if before.is_some() && focused.get_untracked() {
                focused.set(false);
            }
        });
        create_effect(move |_| {
            focused.track();
            if let Some(view) = ed.editor_view_id.get_untracked() {
                view.request_paint();
            }
        });
    }
    if me.focus_wanted.get_untracked() {
        floem::action::exec_after(Duration::from_millis(30), move |_| focus_now(ui));
    }
    let field = field.on_cleanup(move || {
        // Gone from the screen (and not for the next one, which is drawn before this one goes).
        if me.live.with_untracked(|l| l.as_ref().is_some_and(|e| e.id() == id)) {
            me.live.set(None);
        }
    });
    // What the empty box says, under the text (the text's place is clear, so a press on it is the editor's).
    let composing = editor.doc().preedit().preedit;
    let ask = label(move || if running.get() { ASK_MEANWHILE } else { ASK }).style(move |s| {
        let c = t.get();
        let s = s
            .position(Position::Absolute)
            .inset_left(0.0)
            .inset_top(0.0)
            .width_full()
            .font_size(FONT as f32)
            .line_height((ROW / FONT as f64) as f32)
            .color(c.placeholder)
            .selectable(false);
        let s = if at_first() {
            s.text_overflow(TextOverflow::Wrap)
        } else {
            s.height(ROW).text_overflow(TextOverflow::Ellipsis)
        };
        if ui.draft.with(|d| d.is_empty()) && composing.with(|p| p.is_none()) { s } else { s.hide() }
    });
    // A press anywhere on the text's place gives the box the keys: a box drawn again (a new session's, in the middle)
    // can take presses on only part of it at first, and the rest of a press went nowhere.
    let typed = stack((ask, field))
        .on_event_cont(floem::event::EventListener::PointerDown, move |_| {
            if !focused.get_untracked() {
                me.focus_wanted.set(true);
                focus_now(ui);
            }
        })
        .style(|s| s.width_full().min_width(0.0).margin_horiz(2.0).margin_vert(3.0));
    // The text, with what is sent with the message over it (no gap of the box's between them: what is there brings
    // its own space, and nothing takes none).
    let text = match slots::column(ui, &send, Slot::BoxTop) {
        Some(over) => v_stack((over, typed)).style(|s| s.width_full().min_width(0.0)).into_any(),
        None => typed.into_any(),
    };
    let stop = {
        let send = send.clone();
        let button = round(t, "debug-stop", false, || true).on_click_stop(move |_| send(json!({"type": "stop"})));
        tooltip(button, || label(|| "Stop")).style(move |s| if running.get() { s } else { s.hide() })
    };
    let go = {
        let send = send.clone();
        let button = round(t, "arrow-up", true, move || can_send.get()).on_click_stop(move |_| {
            // The keys stay in the box, as when Enter sends.
            submit(ui, &send);
            focus_soon(ui);
        });
        // While an answer is written the button is there only with something to queue.
        tooltip(button, move || label(move || if running.get() { "Queue" } else { "Send" }))
            .style(move |s| if running.get() && !can_send.get() { s.hide() } else { s })
    };
    // The toolbar: the pickers, what other parts add after them, the room between, what they add before the
    // buttons, and they.
    let mut tools = vec![
        pill(ui, Picker::Mode, anchor).into_any(),
        pill(ui, Picker::Model, anchor).into_any(),
        pill(ui, Picker::Thinking, anchor).into_any(),
    ];
    tools.extend(slots::row(ui, &send, Slot::BoxLeft));
    tools.push(empty().style(|s| s.flex_grow(1.0_f32)).into_any());
    tools.extend(slots::row(ui, &send, Slot::BoxRight));
    tools.push(stop.into_any());
    tools.push(go.into_any());
    let toolbar = h_stack_from_iter(tools).style(|s| s.gap(2.0).items_center().width_full().min_width(0.0));
    let frame = v_stack_from_iter([text, toolbar.into_any()]).style(move |s| {
        let c = t.get();
        s.flex_grow(1.0_f32)
            .min_width(0.0)
            .padding(6.0)
            .gap(4.0)
            .border(1.0)
            .border_color(if focused.get() { c.focus } else { c.input_border })
            .border_radius(6.0)
            .background(c.input_bg)
            .apply_if(at_first(), |s| s.max_width(WIDEST_AT_FIRST))
    });
    me.frame.set(Some(frame.id()));
    let hint = label(move || hints(me.width.get())).style(move |s| {
        let s = s.font_family(UI_FONT.clone()).font_size(11.0).color(t.get().desc).padding_horiz(8.0).selectable(false);
        if me.width.get() == Width::Narrow { s.hide() } else { s }
    });
    let boxed = v_stack((container(frame).style(|s| s.width_full().min_width(0.0).justify_center()), hint))
        .style(|s| s.width_full().min_width(0.0).items_center().gap(6.0).padding(12.0).padding_top(6.0));
    let room = empty().style(move |s| if at_first() { s.flex_grow(1.0_f32).flex_basis(0.0) } else { s.hide() });
    let all = v_stack((boxed, cards(ui, send.clone(), at_first), room))
        .on_resize(move |rect| {
            let width = width_of(rect.width());
            if me.width.get_untracked() != width {
                me.width.set(width);
            }
        })
        .style(move |s| {
            let s = s.width_full().min_width(0.0).flex_shrink(0.0_f32);
            if at_first() { s.flex_grow(1.0_f32) } else { s }
        });
    (all.into_any(), back)
}

/// The lines of a file sent with the next message, when no part of the chat keeps them: named at the start of
/// the box, as a mention of them is typed.
fn mention_of(piece: &Piece) -> String {
    let lines =
        if piece.start == piece.end { piece.start.to_string() } else { format!("{}-{}", piece.start, piece.end) };
    let path = if piece.path.contains(' ') { format!("\"{}\"", piece.path) } else { piece.path.clone() };
    format!("@{path}:{lines} ")
}

/// Where the caret is in the box and in the window.
fn caret(ui: Ui) -> Option<Caret> {
    let editor = ui.state::<Composer>().live.get_untracked()?;
    let (offset, affinity) = editor.cursor.with_untracked(|c| (c.offset(), c.affinity));
    let (above, below) = editor.points_of_offset(offset, affinity);
    let (origin, seen) = (editor.window_origin.get_untracked(), editor.viewport.get_untracked());
    let (x, top, bottom) = (origin.x + above.x - seen.x0, origin.y + above.y - seen.y0, origin.y + below.y - seen.y0);
    Some(Caret { offset, rect: Rect::new(x, top, x + 1.0, bottom) })
}

/// What is typed is kept for the folder with the window's look, a moment after the last key (and after the turn
/// when one runs), so that it is there again when the window is opened again. `later` is the window's timer.
fn keep_draft(ui: Ui, send: Send, later: impl Fn(Duration, Box<dyn FnOnce()>) + 'static) {
    let me = ui.state::<Composer>();
    create_effect(move |_| {
        let typed = ui.draft.get();
        let running = ui.running.get();
        if !ui.loaded.get() || running || me.kept.with_untracked(|k| *k == typed) {
            return;
        }
        let send = send.clone();
        later(
            KEEP_AFTER,
            Box::new(move || {
                let still = ui.draft.with_untracked(|d| *d == typed);
                if still && !ui.running.get_untracked() && me.kept.with_untracked(|k| *k != typed) {
                    me.kept.set(typed);
                    send(json!({"type": "keep_view_state", "state": ui.view_state()}));
                }
            }),
        );
    });
}

/// What it adds to the window's registries, once at start: how the rest of the window reaches the box, and the
/// chat's commands (Open Chat with the keys in the box, Stop the Turn, a mode chosen).
pub fn register(ui: Ui, send: &Send) {
    chat_bridge::install(Bridge {
        put_in_box: Rc::new(set_text),
        insert_at_start: Rc::new(|ui, text| replace(ui, 0, 0, text)),
        add_piece: Rc::new(|ui, piece| replace(ui, 0, 0, &mention_of(&piece))),
        ask_now: Rc::new(|ui, send, text| {
            say(ui, send, text, "");
        }),
        focus: Rc::new(open_chat),
        caret: Rc::new(caret),
        place: Rc::new(|ui| ui.state::<Composer>().frame.get_untracked().map(|frame| frame.layout_rect())),
        replace: Rc::new(replace),
    });
    commands::add(command(crate::workbench::SHOW_AGENT, "Chat", "Open Chat", |ui, _| open_chat(ui)).keys("Ctrl+Alt+I"));
    commands::add(
        command(STOP, "Agent", "Stop the Turn", |_, send| send(json!({"type": "stop"})))
            .keys("Ctrl+Escape")
            .enabled(|ui| ui.running.get_untracked()),
    );
    for (mode, _) in MODES {
        commands::add(command(&mode_command(mode), "Agent", &format!("Mode {mode}"), move |_, send| {
            send(json!({"type": "mode", "mode": mode}))
        }));
    }
    keep_draft(ui, send.clone(), |after, then| {
        floem::action::exec_after(after, move |_| then());
    });
}

/// The session's messages that are the box's, taken in: the folder's history, what was typed when the window was
/// last open, and what became of a message sent. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let me = ui.state::<Composer>();
    let key = m["key"].as_str().unwrap_or("");
    match m["type"].as_str().unwrap_or("") {
        "state" => {
            // What was typed when the window was last open, once, into a box nothing is typed in yet.
            if !ui.loaded.get_untracked()
                && let Some(typed) = m["view_state"]["draft"].as_str().filter(|d| !d.is_empty())
                && text(ui).is_empty()
            {
                me.kept.set(typed.to_string());
                set_text(ui, typed);
            }
            ask_history(ui, send, true);
        }
        "history" => {
            let items: Vec<String> =
                m["items"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            if me.older.with_untracked(|o| **o != items) {
                me.older.set(Rc::new(items));
            }
        }
        // The session took the message: as a question, or to be read while it answers.
        "user" | "queued" if !key.is_empty() => {
            if me.waiting.with_untracked(|w| w.iter().any(|w| w.key == key)) {
                me.waiting.update(|w| w.retain(|w| w.key != key));
            }
        }
        // It did not: the words go back into the box (unless more was typed since), and what the chat's parts sent
        // with them to each.
        "refused" if m["of"] == "send" && !key.is_empty() => {
            let back = me.waiting.with_untracked(|w| w.iter().find(|w| w.key == key).cloned());
            if let Some(back) = back {
                me.waiting.update(|w| w.retain(|w| w.key != key));
                if !back.typed.is_empty() && text(ui).is_empty() {
                    set_text(ui, &back.typed);
                }
                chat_bridge::came_back(ui, &back.msg);
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use floem::reactive::create_rw_signal;

    use super::*;
    use crate::model::Question;

    fn sender() -> (Send, Rc<RefCell<Vec<Value>>>) {
        let said = Rc::new(RefCell::new(Vec::new()));
        let to = said.clone();
        (Rc::new(move |v| to.borrow_mut().push(v)), said)
    }

    fn list(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn the_text_grows_with_its_rows_from_what_the_box_has_empty_to_where_it_scrolls() {
        assert_eq!(text_height(1, ROWS_UNDER_A_THREAD), 18.0);
        assert_eq!(text_height(1, ROWS_AT_FIRST), 54.0);
        assert_eq!(text_height(4, ROWS_AT_FIRST), 72.0);
        assert_eq!(text_height(15, ROWS_UNDER_A_THREAD), 270.0);
        assert_eq!(text_height(16, ROWS_UNDER_A_THREAD), TALLEST);
        assert_eq!(text_height(400, ROWS_AT_FIRST), TALLEST);
    }

    #[test]
    fn the_hints_lose_parts_as_the_view_narrows_and_the_cards_stand_side_by_side_when_it_is_wide() {
        assert_eq!(hints(width_of(380.0)), "");
        assert_eq!(hints(width_of(460.0)), "Enter to send · Shift+Enter for a new line · / for skills and commands");
        assert!(hints(width_of(640.0)).contains("↑ for what you sent before"));
        assert_eq!(hints(width_of(640.0)), hints(width_of(900.0)));
        assert_eq!((width_of(719.0), width_of(720.0)), (Width::AllHints, Width::Wide));
    }

    #[test]
    fn the_empty_view_names_the_project_and_the_mode() {
        assert_eq!(
            welcome_line("/home/me/shop", "ask"),
            "Code Arena reads and changes the code in shop, and runs commands. Mode Ask: edits and commands ask first."
        );
        assert!(
            welcome_line("C:\\work\\shop\\", "auto-edit").contains("in shop, and runs commands. Mode Auto-edit: file")
        );
        assert!(
            welcome_line("/src/shop", "yolo").ends_with("Mode Yolo: nothing asks: edits and commands run at once.")
        );
        // Before the session has said where it is and in which mode.
        assert_eq!(welcome_line("", ""), "Code Arena reads and changes the code in this folder, and runs commands.");
        for (name, _) in FIRST_MESSAGES {
            assert!(crate::icons::known(name), "{name}");
        }
        assert!(crate::icons::known("arrow-up") && crate::icons::known("debug-stop"));
    }

    #[test]
    fn what_was_sent_is_one_list_the_newest_first_each_text_once() {
        let here = list(&["fix the test", "  ", "run it"]);
        let older = list(&["run it", "/compact", "fix the test ", "hello"]);
        assert_eq!(recall_list(&here, &older), ["fix the test", "run it", "/compact", "hello"]);
        assert!(recall_list(&[], &[]).is_empty());
    }

    #[test]
    fn up_and_down_step_through_what_was_sent_only_from_the_first_and_last_rows() {
        let on = |first_row, last_row, untouched| Place { first_row, last_row, untouched };
        let (first, last, both) = (on(true, false, false), on(false, true, false), on(true, true, false));
        let middle = Place::default();
        // From what is typed: ↑ on the first row brings the newest back; elsewhere it moves the caret.
        assert_eq!(recall_step(Recall::Up, None, 3, both), Some(Some(0)));
        assert_eq!(recall_step(Recall::Up, None, 3, first), Some(Some(0)));
        assert_eq!(recall_step(Recall::Up, None, 3, last), None);
        assert_eq!(recall_step(Recall::Up, None, 3, middle), None);
        // Nothing was sent: the box's own key.
        assert_eq!(recall_step(Recall::Up, None, 0, both), None);
        // On to the older ones, and no further than the oldest.
        assert_eq!(recall_step(Recall::Up, Some(0), 3, both), Some(Some(1)));
        assert_eq!(recall_step(Recall::Up, Some(2), 3, both), None);
        // A message of one line just brought back steps on at once, though it wraps.
        assert_eq!(recall_step(Recall::Up, Some(0), 3, on(false, true, true)), Some(Some(1)));
        assert_eq!(recall_step(Recall::Down, Some(1), 3, on(true, false, true)), Some(Some(0)));
        // ↓ from the last row goes toward the newest, then to what was being typed; typing, it is the box's own.
        assert_eq!(recall_step(Recall::Down, Some(2), 3, last), Some(Some(1)));
        assert_eq!(recall_step(Recall::Down, Some(0), 3, both), Some(None));
        assert_eq!(recall_step(Recall::Down, Some(1), 3, first), None);
        assert_eq!(recall_step(Recall::Down, None, 3, both), None);
        // Escape goes straight back to what was being typed; with nothing brought back it is not the list's.
        assert_eq!(recall_step(Recall::Escape, Some(2), 3, middle), Some(None));
        assert_eq!(recall_step(Recall::Escape, None, 3, both), None);
    }

    #[test]
    fn the_arrows_bring_back_this_sessions_messages_then_the_folders_and_escape_what_was_typed() {
        let ui = Ui::new();
        let (send, said) = sender();
        ui.push(Kind::User(Question::of("first here")));
        ui.push(Kind::Notice("a notice".into()));
        ui.push(Kind::User(Question { text: "the plan's review".into(), carried: true, ..Question::default() }));
        ui.push(Kind::User(Question {
            text: "@a.rs second".into(),
            typed: "second here".into(),
            ..Question::default()
        }));
        ui.push(Kind::Queued { id: Value::Null, question: Question::of("queued here"), later: false });
        assert_eq!(sent_here(ui), ["queued here", "second here", "first here"]);
        take(ui, &json!({"type": "history", "items": ["from the terminal", "first here", "older"]}), &send);
        let both = Place { first_row: true, last_row: true, untouched: false };
        ui.draft.set("half a thought".into());
        let shown = |key| {
            assert!(recall(ui, &send, key, both));
            ui.draft.get_untracked()
        };
        assert_eq!(shown(Recall::Up), "queued here");
        // The folder's history was asked for as ↑ began.
        assert_eq!(*said.borrow(), [json!({"type": "history"})]);
        assert_eq!(shown(Recall::Up), "second here");
        assert_eq!(shown(Recall::Up), "first here");
        assert_eq!(shown(Recall::Up), "from the terminal");
        assert_eq!(shown(Recall::Up), "older");
        assert!(!recall(ui, &send, Recall::Up, both), "nothing older: the box's own key");
        assert_eq!(shown(Recall::Down), "from the terminal");
        assert_eq!(shown(Recall::Escape), "half a thought");
        assert!(!recall(ui, &send, Recall::Down, both) && !recall(ui, &send, Recall::Escape, both));
        // ↓ all the way gives what was being typed back too; asked a moment ago, the history is not asked again.
        assert_eq!(shown(Recall::Up), "queued here");
        assert_eq!(shown(Recall::Down), "half a thought");
        assert_eq!(said.borrow().len(), 1);
    }

    #[test]
    fn enter_sends_what_is_typed_with_a_key_of_this_windows_and_empties_the_box() {
        let ui = Ui::new();
        let (send, said) = sender();
        submit(ui, &send);
        ui.draft.set("   ".into());
        submit(ui, &send);
        assert!(said.borrow().is_empty(), "nothing typed, nothing sent");
        ui.draft.set("  fix the test \n".into());
        submit(ui, &send);
        ui.draft.set("and run it".into());
        submit(ui, &send);
        let sent = said.borrow().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!((&sent[0]["type"], &sent[0]["text"]), (&json!("send"), &json!("fix the test")));
        let keys: Vec<&str> = sent.iter().filter_map(|m| m["key"].as_str()).collect();
        assert!(keys.len() == 2 && keys[0].ends_with("-1") && keys[1].ends_with("-2"), "{keys:?}");
        assert!(ui.draft.get_untracked().is_empty());
        // Another window of the session has other keys.
        let other = Ui::new();
        other.state::<Composer>().window.set(ui.state::<Composer>().window.get_untracked() + 1);
        other.draft.set("fix the test".into());
        submit(other, &send);
        assert_ne!(said.borrow()[2]["key"], sent[0]["key"]);
    }

    #[test]
    fn a_message_the_session_refuses_comes_back_into_the_box_with_what_went_with_it() {
        let ui = Ui::new();
        let (send, said) = sender();
        // A part of the chat with lines of a file waiting to be sent: they go with the message, with or without
        // words, and come back with it.
        ui.message.set("a.rs:1-2".into());
        chat_bridge::sent_with(
            "test.pieces",
            20,
            chat_bridge::with(|ui, msg| {
                let lines = ui.message.get_untracked();
                if !lines.is_empty() {
                    msg["context"] = json!([lines]);
                    if msg["text"] == "" {
                        msg["text"] = json!("Look at these lines.");
                    }
                    ui.message.set(String::new());
                }
            })
            .more(|ui| !ui.message.get_untracked().is_empty())
            .back(|ui, msg| ui.message.set(msg["context"][0].as_str().unwrap_or("").to_string())),
        );
        submit(ui, &send);
        let first = said.borrow()[0].clone();
        assert_eq!((&first["text"], &first["context"]), (&json!("Look at these lines."), &json!(["a.rs:1-2"])));
        assert!(ui.message.get_untracked().is_empty());
        take(ui, &json!({"type": "refused", "of": "send", "reason": "Too much waits.", "key": first["key"]}), &send);
        assert_eq!(ui.message.get_untracked(), "a.rs:1-2", "the lines are back");
        assert!(ui.draft.get_untracked().is_empty(), "no words were typed for it");
        // With words: they come back too. One the session took is forgotten: a refusal named for it does nothing.
        ui.draft.set("explain them".into());
        submit(ui, &send);
        ui.draft.set("and this".into());
        submit(ui, &send);
        let (second, third) = (said.borrow()[1].clone(), said.borrow()[2].clone());
        assert_eq!((&second["text"], &second["context"]), (&json!("explain them"), &json!(["a.rs:1-2"])));
        assert_eq!(third["context"], Value::Null);
        take(ui, &json!({"type": "user", "text": "and this", "q": 2, "key": third["key"]}), &send);
        take(ui, &json!({"type": "refused", "of": "send", "key": third["key"]}), &send);
        assert!(ui.draft.get_untracked().is_empty());
        take(ui, &json!({"type": "refused", "of": "send", "key": second["key"]}), &send);
        assert_eq!(ui.draft.get_untracked(), "explain them");
        assert_eq!(ui.message.get_untracked(), "a.rs:1-2");
        // Another window's message refused, a refusal of something else: not this box's.
        ui.draft.set(String::new());
        take(ui, &json!({"type": "refused", "of": "send", "key": "ffff-1"}), &send);
        take(ui, &json!({"type": "refused", "of": "rewind", "q": 1}), &send);
        assert!(ui.draft.get_untracked().is_empty());
        // More typed since: the words that come back do not take its place.
        ui.draft.set("typed first".into());
        submit(ui, &send);
        ui.draft.set("typed since".into());
        let fourth = said.borrow()[3].clone();
        take(ui, &json!({"type": "queued", "text": "x", "key": "ffff-9"}), &send);
        take(ui, &json!({"type": "refused", "of": "send", "key": fourth["key"]}), &send);
        assert_eq!(ui.draft.get_untracked(), "typed since");
        assert_eq!(ui.message.get_untracked(), "a.rs:1-2");
        // A part that says there is something to send and then gives no words: nothing goes, nothing is lost.
        chat_bridge::sent_with("test.pieces", 20, chat_bridge::with(|_, _| {}).more(|_| true));
        ui.draft.set(String::new());
        submit(ui, &send);
        assert_eq!(said.borrow().len(), 4);
    }

    #[test]
    fn what_was_typed_is_kept_with_the_windows_look_and_is_back_when_the_window_opens_again() {
        type Timers = Rc<RefCell<Vec<Box<dyn FnOnce()>>>>;
        let ui = Ui::new();
        let (send, said) = sender();
        let timers: Timers = Rc::default();
        let to = timers.clone();
        keep_draft(ui, send.clone(), move |_, then| to.borrow_mut().push(then));
        let ring = || {
            let due: Vec<_> = timers.borrow_mut().drain(..).collect();
            due.into_iter().for_each(|then| then());
        };
        let kept = || said.borrow().iter().filter(|m| m["type"] == "keep_view_state").cloned().collect::<Vec<_>>();
        // Before the session's state has come nothing is kept: the draft it brings is not there yet.
        ui.draft.set("too early".into());
        ring();
        assert!(kept().is_empty());
        ui.draft.set(String::new());
        take(ui, &json!({"type": "state", "view_state": {"draft": "half a thought"}}), &send);
        assert_eq!(ui.draft.get_untracked(), "half a thought");
        assert!(said.borrow().contains(&json!({"type": "history"})));
        ui.loaded.set(true);
        ring();
        assert!(kept().is_empty(), "what came back is what is kept already");
        // Typed on: kept once, a moment after the last key, as the window's look with the draft in it.
        ui.draft.set("half a thought, and".into());
        ui.draft.set("half a thought, and more".into());
        ring();
        assert_eq!(kept().len(), 1);
        assert_eq!(kept()[0]["state"]["draft"], "half a thought, and more");
        assert!(kept()[0]["state"]["dock"].is_object(), "with the rest of the look");
        // While a turn runs it waits for the turn's end; sent, the box is kept empty.
        ui.running.set(true);
        ui.draft.set(String::new());
        ring();
        assert_eq!(kept().len(), 1);
        ui.running.set(false);
        ring();
        assert_eq!((kept().len(), &kept()[1]["state"]["draft"]), (2, &json!("")));
        // A later state (the session went back, another window asked) does not put an old draft over the box.
        ui.draft.set("new words".into());
        take(ui, &json!({"type": "state", "view_state": {"draft": "half a thought"}}), &send);
        assert_eq!(ui.draft.get_untracked(), "new words");
    }

    #[test]
    fn the_rest_of_the_window_reaches_the_box_and_the_chats_commands_are_there() {
        let ui = Ui::new();
        let (send, said) = sender();
        register(ui, &send);
        assert!(chat_bridge::installed());
        chat_bridge::put_in_box(ui, "the parser");
        chat_bridge::insert_at_start(ui, "/explain ");
        assert_eq!(ui.draft.get_untracked(), "/explain the parser");
        chat_bridge::replace(ui, 0, 8, "/fix");
        assert_eq!(ui.draft.get_untracked(), "/fix the parser");
        // Lines of a file with no part to keep them: named at the start, as a mention is typed.
        chat_bridge::add_piece(ui, Piece { path: "src/my cart.rs".into(), start: 12, end: 14, text: String::new() });
        chat_bridge::add_piece(ui, Piece { path: "a.rs".into(), start: 3, end: 3, text: String::new() });
        assert_eq!(ui.draft.get_untracked(), "@a.rs:3 @\"src/my cart.rs\":12-14 /fix the parser");
        // A question asked at once leaves what is typed alone.
        chat_bridge::ask_now(ui, &send, "Explain this");
        assert_eq!(said.borrow().last().expect("sent")["text"], "Explain this");
        assert_eq!(ui.draft.get_untracked(), "@a.rs:3 @\"src/my cart.rs\":12-14 /fix the parser");
        // Not drawn: no caret and no place to put a menu against.
        assert!(chat_bridge::caret(ui).is_none() && chat_bridge::place(ui).is_none());
        // Open Chat, under VS Code's id and key, shows the chat; Stop the Turn runs only while a turn does.
        let open = commands::find(crate::workbench::SHOW_AGENT).expect("Open Chat");
        assert_eq!((open.label().as_str(), open.shown_keys()), ("Chat: Open Chat", "Ctrl+Alt+I"));
        ui.toggle_view(crate::layout::View::Agent);
        assert!(!ui.view_shown(crate::layout::View::Agent));
        assert!(commands::run(ui, &send, crate::workbench::SHOW_AGENT));
        assert!(ui.view_shown(crate::layout::View::Agent));
        let stop = commands::find(STOP).expect("Stop the Turn");
        assert_eq!((stop.label().as_str(), stop.shown_keys()), ("Agent: Stop the Turn", "Ctrl+Escape"));
        let stops = || said.borrow().iter().filter(|m| m["type"] == "stop").count();
        assert!(!commands::run(ui, &send, STOP) && stops() == 0);
        ui.running.set(true);
        assert!(commands::run(ui, &send, STOP) && stops() == 1);
        for (mode, _) in MODES {
            let chosen = commands::find(&mode_command(mode)).expect("a mode's command");
            assert_eq!(chosen.label(), format!("Agent: Mode {mode}"));
            assert!(commands::run(ui, &send, &mode_command(mode)));
            assert_eq!(*said.borrow().last().expect("sent"), json!({"type": "mode", "mode": mode}));
        }
    }

    #[test]
    fn the_box_is_built_with_what_was_typed_and_follows_the_draft_both_ways() {
        // A font to lay text out with, where the machine has none.
        crate::icons::load();
        let ui = Ui::new();
        let (send, said) = sender();
        ui.draft.set("typed before the view was drawn".into());
        let _welcome = welcome(ui).into_view();
        let (_view, back) = input(ui, send.clone(), create_rw_signal(Rect::ZERO));
        let me = ui.state::<Composer>();
        let editor = me.live.get_untracked().expect("the box is drawn");
        assert_eq!(editor.editor_view_id.get_untracked(), Some(back));
        assert_eq!(text(ui), "typed before the view was drawn");
        assert_eq!(editor.cursor.with_untracked(|c| c.offset()), text(ui).len());
        // Set from outside the box (a part of the window), typed in the box, replaced by a part.
        ui.draft.set("from outside".into());
        assert_eq!(editor.text().to_string(), "from outside");
        editor.doc().edit_single(Selection::caret(0), "typed ", EditType::InsertChars);
        assert_eq!(ui.draft.get_untracked(), "typed from outside");
        replace(ui, 0, 5, "@src/main.rs");
        assert_eq!(ui.draft.get_untracked(), "@src/main.rs from outside");
        assert_eq!(caret(ui).map(|c| c.offset), Some("@src/main.rs".len()));
        // ↑ on a text of one row brings the last message back; Enter sends what the box holds and empties it.
        ui.push(Kind::User(Question::of("sent before")));
        let place = place_in(&editor, None).expect("a caret");
        assert!(place.first_row && place.last_row && !place.untouched);
        assert!(recall(ui, &send, Recall::Up, place));
        assert_eq!(editor.text().to_string(), "sent before");
        assert!(place_in(&editor, Some("sent before")).is_some_and(|p| p.untouched));
        submit(ui, &send);
        assert_eq!(said.borrow().last().expect("sent")["text"], "sent before");
        assert!(editor.text().is_empty() && ui.draft.get_untracked().is_empty());
        // Words chosen: the arrows are the box's own.
        ui.draft.set("two words".into());
        editor.cursor.update(|c| c.set_insert(Selection::region(0, 3)));
        assert!(place_in(&editor, None).is_none());
    }
}
