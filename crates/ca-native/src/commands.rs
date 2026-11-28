//! The window's commands, under VS Code's ids: the palette lists them ("Category: Title" and the keys), the menus
//! name them, and a key runs the one bound to it. A part of the window adds its own from its `register` (see
//! `ext.rs`), once at start:
//!
//! ```ignore
//! commands::add(
//!     commands::command("workbench.action.togglePanel", "View", "Toggle Panel Visibility", |ui, _| …).keys("Ctrl+J"),
//! );
//! ```
//!
//! The conventions the window's parts follow:
//!
//! - An id is VS Code's where VS Code has the command, the old Code Arena's (`codeArena.…`) where only it has.
//! - Keys are written as VS Code shows them: `Ctrl+Shift+P`, a chord `Ctrl+K Ctrl+T`. The first key given is the one
//!   the palette and the menus show. A command may have several (`.keys` again).
//! - `when` false: the command is not there now (not in the palette, not in a menu, its keys do nothing). `enabled`
//!   false: it is there, dimmed in a menu, and does not run.
//! - `hidden`: never in the palette (a key's own command).
//! - A command added again under the same id takes the place of the first, where the first was: a part changes
//!   another's command so, without editing its file.
//! - When several commands have the same key, the one added last whose `when` holds runs (VS Code's "last match
//!   wins").
//!
//! The keys are the keymap's (`keymap/mod.rs`): a command's own keys are its defaults, under VS Code's table and the
//! person's keybindings.json, and the keymap looks a key up in a map made once after commands were added (at the
//! first key pressed), never by reading the list on a key. `keys_of` is a command's key as shown now (the person's,
//! if they gave one), `Command::shown_keys` its first default.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use floem::keyboard::{Key, Modifiers, NamedKey};

use crate::state::{Send, Ui};

/// What a command does.
pub type Run = Rc<dyn Fn(Ui, &Send)>;
/// What says whether a command is there now, or may run now.
pub type Test = Rc<dyn Fn(Ui) -> bool>;

#[derive(Clone)]
pub struct Command {
    pub id: String,
    pub title: String,
    /// Shown before the title in the palette ("View", "File", "Preferences"); none: the title alone.
    pub category: String,
    /// Its default keys, as VS Code writes them; the first is the one shown.
    pub keys: Vec<String>,
    pub when: Option<Test>,
    pub enabled: Option<Test>,
    pub hidden: bool,
    pub run: Run,
}

/// A command, to be given its keys and conditions and then added.
pub fn command(id: &str, category: &str, title: &str, run: impl Fn(Ui, &Send) + 'static) -> Command {
    Command {
        id: id.into(),
        title: title.into(),
        category: category.into(),
        keys: Vec::new(),
        when: None,
        enabled: None,
        hidden: false,
        run: Rc::new(run),
    }
}

impl Command {
    /// One more of its default keys ("Ctrl+B", "Ctrl+K Ctrl+T").
    pub fn keys(mut self, keys: &str) -> Command {
        self.keys.push(keys.into());
        self
    }

    pub fn when(mut self, test: impl Fn(Ui) -> bool + 'static) -> Command {
        self.when = Some(Rc::new(test));
        self
    }

    pub fn enabled(mut self, test: impl Fn(Ui) -> bool + 'static) -> Command {
        self.enabled = Some(Rc::new(test));
        self
    }

    pub fn hidden(mut self) -> Command {
        self.hidden = true;
        self
    }

    /// As the palette names it: "Category: Title".
    pub fn label(&self) -> String {
        if self.category.is_empty() { self.title.clone() } else { format!("{}: {}", self.category, self.title) }
    }

    /// Its first default key as written (none: an empty text); `keys_of` is the one shown now.
    pub fn shown_keys(&self) -> &str {
        self.keys.first().map_or("", String::as_str)
    }

    /// Whether it is there now.
    pub fn offered(&self, ui: Ui) -> bool {
        self.when.as_ref().is_none_or(|w| w(ui))
    }

    /// Whether it may run now.
    pub fn may_run(&self, ui: Ui) -> bool {
        self.offered(ui) && self.enabled.as_ref().is_none_or(|e| e(ui))
    }
}

/// A key with what is held with it, as a binding names it: the key in lower case, a shifted sign as the key it is
/// on ("Ctrl+Shift+`", not "Ctrl+~").
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    pub key: String,
}

/// The key a shifted sign is on, on the keyboard VS Code's defaults are written for.
fn unshifted(c: &str) -> &str {
    match c {
        "~" => "`",
        "!" => "1",
        "@" => "2",
        "#" => "3",
        "$" => "4",
        "%" => "5",
        "^" => "6",
        "&" => "7",
        "*" => "8",
        "(" => "9",
        ")" => "0",
        "_" => "-",
        "+" => "=",
        "{" => "[",
        "}" => "]",
        "|" => "\\",
        ":" => ";",
        "\"" => "'",
        "<" => ",",
        ">" => ".",
        "?" => "/",
        other => other,
    }
}

impl Chord {
    /// A key as it is written in a binding ("Ctrl+Shift+P"); none when it names no key.
    pub fn parse(text: &str) -> Option<Chord> {
        let mut chord = Chord { ctrl: false, shift: false, alt: false, meta: false, key: String::new() };
        // "Ctrl+=" and "Ctrl++" both end in their key: the last part is the key, whatever it is.
        let (mods, key) = match text.strip_suffix("++") {
            Some(mods) => (mods, "+"),
            None => text.rsplit_once('+').unwrap_or(("", text)),
        };
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            match m.to_lowercase().as_str() {
                "ctrl" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" => chord.alt = true,
                "meta" | "cmd" | "win" => chord.meta = true,
                _ => return None,
            }
        }
        chord.key = crate::keymap::key_name(key);
        (!chord.key.is_empty()).then_some(chord)
    }

    /// A key pressed; none for a key that is only held (Ctrl, Shift, Alt).
    pub fn of(key: &Key, m: Modifiers) -> Option<Chord> {
        let shift = m.contains(Modifiers::SHIFT);
        let name = match key {
            Key::Character(c) => {
                let c = c.to_lowercase();
                if shift { unshifted(&c).to_string() } else { c }
            }
            Key::Named(n) => match n {
                NamedKey::Escape => "escape".into(),
                NamedKey::Enter => "enter".into(),
                NamedKey::Tab => "tab".into(),
                NamedKey::Space => "space".into(),
                NamedKey::Backspace => "backspace".into(),
                NamedKey::Delete => "delete".into(),
                NamedKey::Insert => "insert".into(),
                NamedKey::Home => "home".into(),
                NamedKey::End => "end".into(),
                NamedKey::PageUp => "pageup".into(),
                NamedKey::PageDown => "pagedown".into(),
                NamedKey::ArrowUp => "up".into(),
                NamedKey::ArrowDown => "down".into(),
                NamedKey::ArrowLeft => "left".into(),
                NamedKey::ArrowRight => "right".into(),
                NamedKey::F1 => "f1".into(),
                NamedKey::F2 => "f2".into(),
                NamedKey::F3 => "f3".into(),
                NamedKey::F4 => "f4".into(),
                NamedKey::F5 => "f5".into(),
                NamedKey::F6 => "f6".into(),
                NamedKey::F7 => "f7".into(),
                NamedKey::F8 => "f8".into(),
                NamedKey::F9 => "f9".into(),
                NamedKey::F10 => "f10".into(),
                NamedKey::F11 => "f11".into(),
                NamedKey::F12 => "f12".into(),
                _ => return None,
            },
            _ => return None,
        };
        Some(Chord {
            ctrl: m.contains(Modifiers::CONTROL),
            shift,
            alt: m.contains(Modifiers::ALT),
            meta: m.contains(Modifiers::META),
            key: name,
        })
    }
}

/// A binding's keys one after the other ("Ctrl+K Ctrl+T": two); none when any of them names no key.
pub fn parse_keys(text: &str) -> Option<Vec<Chord>> {
    let all: Option<Vec<Chord>> = text.split(' ').filter(|c| !c.is_empty()).map(Chord::parse).collect();
    all.filter(|a| !a.is_empty())
}

#[derive(Default)]
struct Registry {
    /// Each command once, shared with whoever lists or runs it (a list of them all is made at each key typed in
    /// the palette: it copies no text).
    list: Vec<Rc<Command>>,
    by_id: HashMap<String, usize>,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

/// Adds a command; one of the same id takes the first's place.
pub fn add(c: Command) {
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        match r.by_id.get(&c.id).copied() {
            Some(at) => r.list[at] = Rc::new(c),
            None => {
                let at = r.list.len();
                r.by_id.insert(c.id.clone(), at);
                r.list.push(Rc::new(c));
            }
        }
    });
    crate::keymap::stale();
}

/// Every command, in the order added.
pub fn all() -> Vec<Rc<Command>> {
    REGISTRY.with(|r| r.borrow().list.clone())
}

pub fn find(id: &str) -> Option<Rc<Command>> {
    REGISTRY.with(|r| {
        let r = r.borrow();
        r.by_id.get(id).map(|at| r.list[*at].clone())
    })
}

/// The key shown for a command now: the person's if they gave one, else its first default (none, or no such
/// command: an empty text).
pub fn keys_of(id: &str) -> String {
    if find(id).is_none() {
        return String::new();
    }
    crate::keymap::shown_keys(id)
}

/// Runs a command by its id; false when there is none, or it is not there or may not run now.
pub fn run(ui: Ui, send: &Send, id: &str) -> bool {
    // Taken out of the registry first: a command may add commands, or run one.
    match find(id).filter(|c| c.may_run(ui)) {
        Some(c) => {
            (c.run)(ui, send);
            true
        }
        None => false,
    }
}

/// What became of a key pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressed {
    /// A command was bound to it (it ran, unless it may not run now).
    Ran,
    /// It begins a chord: the next key ends it.
    Waiting,
    /// No command has it.
    Free,
}

/// A key pressed in the window, taken to the command bound to it (the keymap's `press`). A key that ends no chord
/// begun is not a command: it does nothing else.
pub fn press(ui: Ui, send: &Send, key: &Key, m: Modifiers) -> Pressed {
    crate::keymap::press(ui, send, key, m)
}

/// The first key of a chord that waits for its second (none: no chord is begun).
pub fn waiting() -> Option<Chord> {
    crate::keymap::waiting()
}

#[cfg(test)]
mod tests {
    use floem::reactive::{SignalGet, SignalUpdate};

    use super::*;

    fn ch(text: &str) -> Chord {
        Chord::parse(text).expect("a key")
    }

    fn key(c: &str) -> Key {
        Key::Character(c.into())
    }

    #[test]
    fn a_key_is_read_as_vs_code_writes_it_and_as_it_is_pressed() {
        assert_eq!(ch("Ctrl+Shift+P"), Chord { ctrl: true, shift: true, alt: false, meta: false, key: "p".into() });
        assert_eq!(ch("F5").key, "f5");
        assert_eq!(ch("Ctrl+=").key, "=");
        assert_eq!(ch("Ctrl++").key, "+");
        assert_eq!(Chord::parse("Hyper+P"), None);
        assert_eq!(Chord::parse("Ctrl+"), None);
        assert_eq!(parse_keys("Ctrl+K Ctrl+T").map(|k| k.len()), Some(2));
        assert_eq!(parse_keys(""), None);
        // Pressed: any case, a shifted sign as the key it is on, a held key alone nothing.
        assert_eq!(Chord::of(&key("P"), Modifiers::CONTROL | Modifiers::SHIFT), Some(ch("Ctrl+Shift+P")));
        assert_eq!(Chord::of(&key("~"), Modifiers::CONTROL | Modifiers::SHIFT), Some(ch("Ctrl+Shift+`")));
        assert_eq!(Chord::of(&Key::Named(NamedKey::F11), Modifiers::empty()), Some(ch("F11")));
        assert_eq!(Chord::of(&Key::Named(NamedKey::Control), Modifiers::CONTROL), None);
    }

    #[test]
    fn a_command_is_named_found_and_run_by_its_id() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        add(command("test.say", "Test", "Say", |ui, _| ui.message.set("said".into())).keys("Ctrl+Alt+9"));
        add(command("test.plain", "", "Plain…", |_, _| {}));
        let c = find("test.say").expect("added");
        assert_eq!(c.label(), "Test: Say");
        assert_eq!(find("test.plain").map(|c| c.label()).as_deref(), Some("Plain…"));
        assert_eq!(keys_of("test.say"), "Ctrl+Alt+9");
        assert_eq!(keys_of("test.none"), "");
        assert!(run(ui, &send, "test.say"));
        assert_eq!(ui.message.get_untracked(), "said");
        assert!(!run(ui, &send, "test.none"));
        // Added again: in the first's place, and it is the one that runs.
        let before: Vec<String> = all().iter().map(|c| c.id.clone()).collect();
        add(command("test.say", "Test", "Say", |ui, _| ui.message.set("said again".into())).keys("Ctrl+Alt+9"));
        assert_eq!(all().iter().map(|c| c.id.clone()).collect::<Vec<_>>(), before);
        assert_eq!(press(ui, &send, &key("9"), Modifiers::CONTROL | Modifiers::ALT), Pressed::Ran);
        assert_eq!(ui.message.get_untracked(), "said again");
    }

    #[test]
    fn a_key_runs_the_last_command_bound_to_it_that_is_there() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let said = move |text: &'static str| move |ui: Ui, _: &Send| ui.message.set(text.into());
        add(command("test.first", "", "First", said("first")).keys("Ctrl+Alt+8"));
        add(command("test.second", "", "Second", said("second"))
            .keys("Ctrl+Alt+8")
            .when(|ui| ui.running.get_untracked()));
        let press8 = || press(ui, &send, &key("8"), Modifiers::CONTROL | Modifiers::ALT);
        assert_eq!(press8(), Pressed::Ran);
        assert_eq!(ui.message.get_untracked(), "first", "the second is not there now");
        ui.running.set(true);
        assert_eq!(press8(), Pressed::Ran);
        assert_eq!(ui.message.get_untracked(), "second");
        // One that may not run now takes its key and does nothing; by its id it does not run either.
        add(command("test.off", "", "Off", said("off")).keys("Ctrl+Alt+7").enabled(|_| false));
        assert_eq!(press(ui, &send, &key("7"), Modifiers::CONTROL | Modifiers::ALT), Pressed::Ran);
        assert_eq!(ui.message.get_untracked(), "second");
        assert!(!run(ui, &send, "test.off"));
        assert_eq!(press(ui, &send, &key("6"), Modifiers::CONTROL | Modifiers::ALT), Pressed::Free);
    }

    #[test]
    fn a_chord_waits_for_its_second_key_and_another_key_ends_it_as_no_command() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let said = move |text: &'static str| move |ui: Ui, _: &Send| ui.message.set(text.into());
        add(command("test.chord", "", "Chord", said("chord")).keys("Ctrl+Alt+K Ctrl+Alt+T"));
        add(command("test.single", "", "Single", said("single")).keys("Ctrl+Alt+B"));
        let p = |c: &str| press(ui, &send, &key(c), Modifiers::CONTROL | Modifiers::ALT);
        assert_eq!(p("k"), Pressed::Waiting);
        assert_eq!(waiting(), Some(ch("Ctrl+Alt+K")));
        // A key held between the two changes nothing.
        assert_eq!(press(ui, &send, &Key::Named(NamedKey::Control), Modifiers::CONTROL), Pressed::Free);
        assert_eq!(p("t"), Pressed::Ran);
        assert_eq!(ui.message.get_untracked(), "chord");
        assert_eq!(waiting(), None);
        // Not the chord's second key: no command, and the key's own does not run.
        assert_eq!(p("k"), Pressed::Waiting);
        assert_eq!(p("b"), Pressed::Ran);
        assert_eq!(ui.message.get_untracked(), "chord");
        assert_eq!(p("t"), Pressed::Free, "no chord is begun any more");
        assert_eq!(p("b"), Pressed::Ran);
        assert_eq!(ui.message.get_untracked(), "single");
    }
}
