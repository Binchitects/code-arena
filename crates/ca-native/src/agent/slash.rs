//! The input's `/` menu, "Skills and commands", as the old Code Arena's: `/` at the start of the box (or after the
//! `/names` chosen there: several go together) offers the chat's own commands, the project's and the person's
//! commands and the skills, those whose name starts with what is typed first, then those that hold it. ↑ and ↓
//! choose (the list scrolls to keep the chosen one in sight), Enter or Tab puts `/name ` in the box for the words
//! after it, and Escape leaves it as typed. A name typed whole, Enter sends as it is. Escape with the list shut lets
//! a `/name` chosen at the start go, and keeps the words after it.
//!
//! The list is offered only for what is typed: a message brought back with ↑ keeps the arrows and Enter for the
//! box. The keys stay in the box all the while; it offers them here first (`chat_bridge::on_key`). The list is
//! drawn over the window (`layers.rs`), its foot against the box's top.
//!
//! What is offered is the session's `library` (asked for when the session's state comes, and again when the list
//! opens and the last answer is old).

use std::rc::Rc;
use std::time::{Duration, Instant};

use floem::IntoView;
use floem::event::EventListener;
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::Rect;
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect};
use floem::style::{CursorStyle, Position, TextOverflow};
use floem::views::{Decorators, dyn_stack, h_stack, label, scroll, v_stack};
use serde_json::{Value, json};

use crate::chat_bridge;
use crate::icons::icon;
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::widgets::MONO_FONT;

/// A row's height, and the most rows shown before the list scrolls.
const ROW: f64 = 24.0;
const SHOWN: usize = 12;
/// The list's heading and its padding above and below the rows.
const HEADING: f64 = 24.0;
const PADDING: f64 = 4.0;
/// How long what was listed is taken as it stands.
const FRESH: Duration = Duration::from_secs(30);
const TITLE: &str = "Skills and commands";

/// What a name offered is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The chat's own (`/clear`).
    Chat,
    /// A command of the project's or the person's.
    Command,
    Skill,
}

/// A name offered.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Entry {
    pub name: String,
    pub description: String,
    /// What a command takes after its name.
    pub hint: String,
    /// Where it was read from ("yours", "this project (.code-arena)").
    pub source: String,
    pub kind: Kind,
}

/// The `/name` being typed: where its `/` is and where the caret is (bytes), and what is typed after the `/`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Typing {
    from: usize,
    to: usize,
    word: String,
}

/// The menu's own state.
#[derive(Clone, Copy)]
pub struct Slash {
    /// What is offered, as the session listed it: the chat's own, the commands, then the skills.
    all: RwSignal<Rc<Vec<Entry>>>,
    /// When the library was last asked for.
    asked: RwSignal<Option<Instant>>,
    /// The box's text as the person last typed it (the list is offered only for that).
    typed: RwSignal<Option<String>>,
    /// The text the list was shut for with Escape: it stays shut until the text changes.
    dismissed: RwSignal<Option<String>>,
    typing: RwSignal<Option<Typing>>,
    /// The names that match what is typed, in the order shown; none: the list is shut.
    items: RwSignal<Rc<Vec<Entry>>>,
    /// The chosen row.
    at: RwSignal<usize>,
    /// The box's place in the window, as it was when the list opened.
    under: RwSignal<Rect>,
}

impl crate::ext::State for Slash {
    fn new(ui: Ui) -> Slash {
        let s = ui.scope;
        Slash {
            all: s.create_rw_signal(Rc::new(Vec::new())),
            asked: s.create_rw_signal(None),
            typed: s.create_rw_signal(None),
            dismissed: s.create_rw_signal(None),
            typing: s.create_rw_signal(None),
            items: s.create_rw_signal(Rc::new(Vec::new())),
            at: s.create_rw_signal(0),
            under: s.create_rw_signal(Rect::ZERO),
        }
    }
}

/// A signal set to a value only when it holds another.
fn set_if<T: PartialEq + 'static>(signal: RwSignal<T>, value: T) {
    if signal.with_untracked(|v| *v != value) {
        signal.set(value);
    }
}

/// A character a `/name` is made of.
fn name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '-')
}

/// The `/name` being typed before the caret: at the start of the text, or after `/names` chosen there, each with
/// white space after it. Its `/`'s place and what follows it so far.
fn typing(text: &str, caret: usize) -> Option<(usize, &str)> {
    let before = text.get(..caret)?;
    let mut at = 0;
    loop {
        let rest = before[at..].strip_prefix('/')?;
        let name = rest.find(|c: char| !name_char(c)).map_or(rest, |end| &rest[..end]);
        let after = &rest[name.len()..];
        if after.is_empty() {
            return Some((at, name));
        }
        let spaces = after.len() - after.trim_start().len();
        if name.is_empty() || spaces == 0 {
            return None;
        }
        at += 1 + name.len() + spaces;
    }
}

/// The length of the `/name` chosen at the start of the text with the white space after it, if there is one.
fn chosen_name(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('/')?;
    let name = rest.find(|c: char| !name_char(c)).unwrap_or(rest.len());
    let after = &rest[name..];
    let spaces = after.len() - after.trim_start().len();
    (name > 0 && (after.is_empty() || spaces > 0)).then_some(1 + name + spaces)
}

/// The names offered for what is typed after the `/`: those that start with it, then those that hold it, each
/// name once (the first listed: a command before a skill of its name).
fn ranked(all: &[Entry], word: &str) -> Vec<Entry> {
    let q = word.to_lowercase();
    let lower: Vec<String> = all.iter().map(|e| e.name.to_lowercase()).collect();
    let starting = all.iter().zip(&lower).filter(|(_, n)| n.starts_with(&q));
    let holding = all.iter().zip(&lower).filter(|(_, n)| !n.starts_with(&q) && n.contains(&q));
    let mut seen = std::collections::HashSet::new();
    starting.chain(holding).filter(|(e, _)| seen.insert(e.name.as_str())).map(|(e, _)| e.clone()).collect()
}

/// What the session listed (`library`), as the menu offers it.
fn entries(m: &Value) -> Vec<Entry> {
    let of = |key: &str, kind: Kind| -> Vec<Entry> {
        m[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let text = |k: &str| e[k].as_str().unwrap_or_default().to_string();
                let name = e["name"].as_str().filter(|n| !n.is_empty())?.to_string();
                Some(Entry { name, description: text("description"), hint: text("hint"), source: text("source"), kind })
            })
            .collect()
    };
    let mut all = of("builtins", Kind::Chat);
    all.extend(of("commands", Kind::Command));
    all.extend(of("skills", Kind::Skill));
    all
}

/// The box's text and its caret.
fn now(ui: Ui) -> (String, usize) {
    let text = ui.draft.get_untracked();
    let caret = chat_bridge::caret(ui).map_or(text.len(), |c| c.offset).min(text.len());
    (text, caret)
}

/// The list made what the box's text offers: `edited`, the person has just typed it.
fn refresh(ui: Ui, send: &Send, edited: bool, text: &str, caret: usize) {
    let me = ui.state::<Slash>();
    if edited {
        set_if(me.typed, Some(text.to_string()));
    }
    let typed = me.typed.with_untracked(|t| t.as_deref() == Some(text));
    let dismissed = me.dismissed.with_untracked(|d| d.as_deref() == Some(text));
    let found = if typed && !dismissed { typing(text, caret) } else { None };
    let items = match found {
        Some((_, word)) => me.all.with_untracked(|all| ranked(all, word)),
        None => Vec::new(),
    };
    set_if(me.typing, found.map(|(from, word)| Typing { from, to: caret, word: word.to_string() }));
    if me.items.with_untracked(|old| **old != items) {
        set_if(me.at, 0);
        me.items.set(Rc::new(items));
    }
    if found.is_some() {
        if me.asked.get_untracked().is_none_or(|at| at.elapsed() > FRESH) {
            ask(ui, send);
        }
        if let Some(place) = chat_bridge::place(ui) {
            set_if(me.under, place);
        }
    }
}

/// The list made what the box offers once the box has acted on the key (its caret moved).
fn refresh_soon(ui: Ui, send: &Send, edited: bool) {
    let send = send.clone();
    floem::action::exec_after(Duration::ZERO, move |_| {
        let (text, caret) = now(ui);
        refresh(ui, &send, edited, &text, caret);
    });
}

/// The library asked for.
fn ask(ui: Ui, send: &Send) {
    ui.state::<Slash>().asked.set(Some(Instant::now()));
    send(json!({"type": "library"}));
}

/// The list shut, until the text changes.
fn shut(ui: Ui, text: &str) {
    let me = ui.state::<Slash>();
    set_if(me.dismissed, Some(text.to_string()));
    set_if(me.typing, None);
    if me.items.with_untracked(|i| !i.is_empty()) {
        me.items.set(Rc::new(Vec::new()));
    }
}

/// A row chosen: `/name ` in the place of what was typed for it (and the white space after the caret), the caret
/// after it.
fn pick(ui: Ui, n: usize) {
    let me = ui.state::<Slash>();
    let (Some(at), Some(entry)) = (me.typing.get_untracked(), me.items.with_untracked(|i| i.get(n).cloned())) else {
        return;
    };
    let text = ui.draft.get_untracked();
    let after = text.get(at.to..).unwrap_or_default();
    let to = at.to + (after.len() - after.trim_start().len());
    me.typed.set(None);
    shut(ui, "");
    chat_bridge::replace(ui, at.from, to, &format!("/{} ", entry.name));
}

/// Whether a key may change the box's text (the list is offered for what is typed).
fn edits(key: &Key) -> bool {
    matches!(key, Key::Character(_) | Key::Named(NamedKey::Space | NamedKey::Backspace | NamedKey::Delete))
}

/// A key pressed in the box, offered here first: true when it was the list's.
fn key(ui: Ui, send: &Send, key: &Key, mods: Modifiers) -> bool {
    let me = ui.state::<Slash>();
    let n = me.items.with_untracked(|i| i.len());
    let plain = !mods.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::META | Modifiers::SHIFT);
    if plain && n > 0 {
        match key {
            Key::Named(NamedKey::ArrowDown) => {
                me.at.update(|at| *at = (*at + 1) % n);
                return true;
            }
            Key::Named(NamedKey::ArrowUp) => {
                me.at.update(|at| *at = (*at + n - 1) % n);
                return true;
            }
            Key::Named(NamedKey::Enter | NamedKey::Tab) => {
                let at = me.at.get_untracked().min(n - 1);
                let whole = me.typing.with_untracked(|t| {
                    me.items.with_untracked(|i| t.as_ref().is_some_and(|t| i[at].name.eq_ignore_ascii_case(&t.word)))
                });
                // The whole name is typed already: Enter is the box's (it sends); Tab still completes.
                if whole && *key == Key::Named(NamedKey::Enter) {
                    shut(ui, "");
                    return false;
                }
                pick(ui, at);
                return true;
            }
            Key::Named(NamedKey::Escape) => {
                shut(ui, &ui.draft.get_untracked());
                return true;
            }
            _ => {}
        }
    }
    if plain && n == 0 && *key == Key::Named(NamedKey::Escape) {
        let text = ui.draft.get_untracked();
        if let Some(len) = chosen_name(&text) {
            // A /name chosen at the start: it goes, the words after it stay.
            set_if(me.dismissed, Some(text[len..].to_string()));
            chat_bridge::replace(ui, 0, len, "");
            return true;
        }
    }
    refresh_soon(ui, send, edits(key));
    false
}

/// The list, over the window with its foot against the box's top, as wide as the box within it.
fn menu(ui: Ui) -> impl IntoView {
    let me = ui.state::<Slash>();
    let t = ui.colors;
    let rows = dyn_stack(
        move || me.items.get().iter().cloned().enumerate().collect::<Vec<_>>(),
        |(n, e): &(usize, Entry)| (*n, e.clone()),
        move |(n, e): (usize, Entry)| row(ui, n, e),
    )
    .style(|s| s.flex_col().width_full());
    let list = scroll(rows)
        .ensure_visible(move || {
            let y = me.at.get() as f64 * ROW;
            Rect::new(0.0, y, 1.0, y + ROW)
        })
        .scroll_style(|s| s.hide_bars(true))
        .style(move |s| s.width_full().height(me.items.with(|i| i.len().min(SHOWN)) as f64 * ROW));
    let heading = label(|| TITLE).style(move |s| {
        s.height(HEADING).items_center().padding_horiz(8.0).font_size(11.0).color(t.get().desc).selectable(false)
    });
    v_stack((heading, list))
        // A press on the list leaves the box its keys.
        .on_event_stop(EventListener::PointerDown, move |_| chat_bridge::focus(ui))
        .style(move |s| {
            let c = t.get();
            let n = me.items.with(|i| i.len());
            let under = me.under.get();
            let (width, _) = ui.window.get();
            let height = HEADING + n.min(SHOWN) as f64 * ROW + 2.0 * PADDING + 2.0;
            let left = (under.x0 + 8.0).max(4.0);
            s.position(Position::Absolute)
                .inset_left(left)
                .inset_top((under.y0 - height - 4.0).max(4.0))
                .width((under.width() - 16.0).min(width - left - 4.0).max(160.0))
                .padding_vert(PADDING)
                .padding_horiz(4.0)
                .border(1.0)
                .border_color(c.menu_border)
                .border_radius(6.0)
                .background(c.menu_bg)
                .box_shadow_blur(8.0)
                .box_shadow_color(c.shadow)
                .color(c.menu_fg)
                .font_size(13.0)
                .z_index(100)
                .apply_if(n == 0 || under == Rect::ZERO, |s| s.hide())
        })
}

/// A row of the list: what it is, `/name`, what a command takes, and what it does.
fn row(ui: Ui, n: usize, e: Entry) -> impl IntoView {
    let me = ui.state::<Slash>();
    let t = ui.colors;
    let lit = move || me.at.get() == n;
    let sign = match e.kind {
        Kind::Skill => "sparkle",
        Kind::Command | Kind::Chat => "symbol-event",
    };
    let name = format!("/{}", e.name);
    let (hint, description) = (e.hint.clone(), e.description.lines().next().unwrap_or_default().to_string());
    let dim = move |s: floem::style::Style| {
        let c = t.get();
        s.color(if lit() { c.menu_selected_fg } else { c.desc })
    };
    h_stack((
        icon(sign, 14.0).style(move |s| dim(s).flex_shrink(0.0_f32)),
        label(move || name.clone()).style(|s| s.flex_shrink(0.0_f32).font_family(MONO_FONT.to_string())),
        label(move || hint.clone()).style(move |s| {
            dim(s).flex_shrink(0.0_f32).apply_if(e.hint.is_empty(), |s| s.hide()).font_family(MONO_FONT.to_string())
        }),
        label(move || description.clone())
            .style(move |s| dim(s).min_width(0.0).flex_shrink(1.0_f32).text_overflow(TextOverflow::Ellipsis)),
    ))
    .on_event_stop(EventListener::PointerDown, move |_| {
        pick(ui, n);
        chat_bridge::focus(ui);
    })
    .on_event_cont(EventListener::PointerEnter, move |_| set_if(me.at, n))
    .style(move |s| {
        let c = t.get();
        s.height(ROW)
            .width_full()
            .items_center()
            .gap(6.0)
            .padding_horiz(6.0)
            .border_radius(4.0)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .apply_if(lit(), |s| s.background(c.menu_selected).color(c.menu_selected_fg))
    })
}

/// What it adds to the window's registries, once at start: the list's keys in the box, the list over the window,
/// and the list kept to what the box's text offers.
pub fn register(ui: Ui, send: &Send) {
    chat_bridge::on_key("slash", 10, key);
    layers::add(Place::Over, "slash", 45, |ui, _| menu(ui).into_any());
    let send = send.clone();
    create_effect(move |_| {
        ui.draft.track();
        refresh_soon(ui, &send, false);
    });
}

/// The session's messages that are this part's, taken in: the library listed (its own), and the session's state,
/// when it is asked for.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str().unwrap_or_default() {
        "state" => {
            ask(ui, send);
            false
        }
        "library" => {
            let me = ui.state::<Slash>();
            set_if(me.all, Rc::new(entries(m)));
            let (text, caret) = now(ui);
            refresh(ui, send, false, &text, caret);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn entry(name: &str, kind: Kind) -> Entry {
        Entry { name: name.into(), description: String::new(), hint: String::new(), source: "yours".into(), kind }
    }

    fn names(list: &[Entry]) -> Vec<&str> {
        list.iter().map(|e| e.name.as_str()).collect()
    }

    fn sender() -> (Send, Rc<RefCell<Vec<Value>>>) {
        let said = Rc::new(RefCell::new(Vec::new()));
        let to = said.clone();
        (Rc::new(move |v| to.borrow_mut().push(v)), said)
    }

    #[test]
    fn a_name_is_typed_at_the_start_or_after_names_chosen_there() {
        assert_eq!(typing("/", 1), Some((0, "")));
        assert_eq!(typing("/rev", 4), Some((0, "rev")));
        assert_eq!(typing("/rev", 2), Some((0, "r")));
        assert_eq!(typing("/review /te", 11), Some((8, "te")));
        assert_eq!(typing("/review  /git:co", 16), Some((9, "git:co")));
        // Not at the start, a word between, the name ended, the caret elsewhere.
        assert_eq!(typing("fix /rev", 8), None);
        assert_eq!(typing("/review the /te", 15), None);
        assert_eq!(typing("/review ", 8), None);
        assert_eq!(typing("//x", 3), None);
        assert_eq!(typing("/a", 9), None);
    }

    #[test]
    fn the_name_chosen_at_the_start_goes_with_the_space_after_it() {
        assert_eq!(chosen_name("/review  the cart"), Some(9));
        assert_eq!(chosen_name("/review"), Some(7));
        assert_eq!(chosen_name("/ x"), None);
        assert_eq!(chosen_name("review it"), None);
        assert_eq!(chosen_name("/rev,x"), None);
    }

    #[test]
    fn names_that_start_with_what_is_typed_come_first_and_a_command_hides_a_skill_of_its_name() {
        let all = vec![
            entry("clear", Kind::Chat),
            entry("pre-review", Kind::Command),
            entry("review", Kind::Command),
            entry("Review", Kind::Skill),
            entry("review", Kind::Skill),
            entry("tests", Kind::Skill),
        ];
        let got = ranked(&all, "rev");
        assert_eq!(names(&got), ["review", "Review", "pre-review"]);
        assert_eq!(got[0].kind, Kind::Command);
        assert_eq!(ranked(&all, "").len(), 5);
        assert!(ranked(&all, "zzz").is_empty());
    }

    #[test]
    fn what_the_session_lists_is_offered_its_own_first_then_the_commands_then_the_skills() {
        let got = entries(&json!({
            "type": "library",
            "skills": [{"name": "tests", "description": "Write tests", "source": "yours", "path": "/x/SKILL.md"}],
            "commands": [{"name": "review", "description": "Review it", "hint": "FILE", "source": "yours"}, {"name": ""}],
            "builtins": [{"name": "clear", "description": "Start a new session"}],
        }));
        assert_eq!(names(&got), ["clear", "review", "tests"]);
        assert_eq!((got[1].hint.as_str(), got[1].kind), ("FILE", Kind::Command));
        assert_eq!(got[2].kind, Kind::Skill);
    }

    #[test]
    fn the_list_opens_for_what_is_typed_and_not_for_a_message_brought_back() {
        let ui = Ui::new();
        let (send, said) = sender();
        let me = ui.state::<Slash>();
        me.all.set(Rc::new(vec![entry("review", Kind::Command), entry("tests", Kind::Skill)]));
        refresh(ui, &send, true, "/", 1);
        assert_eq!(names(&me.items.get_untracked()), ["review", "tests"]);
        // The library is asked for as the list opens, once while it is fresh.
        refresh(ui, &send, true, "/rev", 4);
        assert_eq!(said.borrow().iter().filter(|m| m["type"] == "library").count(), 1);
        assert_eq!(names(&me.items.get_untracked()), ["review"]);
        // A text the box was given, not typed: shut.
        refresh(ui, &send, false, "/review it", 4);
        assert!(me.items.get_untracked().is_empty());
        // Shut with Escape: it stays shut until the text changes.
        ui.draft.set("/te".into());
        refresh(ui, &send, true, "/te", 3);
        assert_eq!(names(&me.items.get_untracked()), ["tests"]);
        assert!(key(ui, &send, &Key::Named(NamedKey::Escape), Modifiers::empty()));
        refresh(ui, &send, false, "/te", 3);
        assert!(me.items.get_untracked().is_empty());
        refresh(ui, &send, true, "/tes", 4);
        assert_eq!(names(&me.items.get_untracked()), ["tests"]);
    }

    #[test]
    fn the_arrows_choose_round_the_list_and_enter_on_a_whole_name_is_the_boxs() {
        let ui = Ui::new();
        let (send, _) = sender();
        let me = ui.state::<Slash>();
        me.all.set(Rc::new(vec![entry("tests", Kind::Command), entry("test-all", Kind::Skill)]));
        refresh(ui, &send, true, "/test", 5);
        let down = Key::Named(NamedKey::ArrowDown);
        assert!(key(ui, &send, &down, Modifiers::empty()));
        assert_eq!(me.at.get_untracked(), 1);
        assert!(key(ui, &send, &down, Modifiers::empty()));
        assert_eq!(me.at.get_untracked(), 0);
        assert!(key(ui, &send, &Key::Named(NamedKey::ArrowUp), Modifiers::empty()));
        assert_eq!(me.at.get_untracked(), 1);
        // Typed whole: Enter sends what is typed, and the list shuts.
        refresh(ui, &send, true, "/tests", 6);
        assert!(!key(ui, &send, &Key::Named(NamedKey::Enter), Modifiers::empty()));
        assert!(me.items.get_untracked().is_empty());
    }

    #[test]
    fn library_replies_are_this_parts_and_the_state_asks_for_one() {
        let ui = Ui::new();
        let (send, said) = sender();
        assert!(!take(ui, &json!({"type": "state"}), &send));
        assert_eq!(said.borrow().last().cloned(), Some(json!({"type": "library"})));
        assert!(take(ui, &json!({"type": "library", "commands": [{"name": "review"}]}), &send));
        assert_eq!(names(&ui.state::<Slash>().all.get_untracked()), ["review"]);
    }
}
