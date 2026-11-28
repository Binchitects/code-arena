//! The keymap: which command a key (or a chord of two) runs, as VS Code resolves it. Its rules come from three
//! places, the later winning: VS Code's default table (`defaults.rs`, each rule with its `when`), the commands' own
//! keys (`commands::Command::keys`, as each part gave them), and the person's `keybindings.json`. A rule of the
//! person's whose command is `-id` takes away the rules before it of that command (of that key, if it names one,
//! and of that `when`, if it gives one).
//!
//! - A key is resolved against the rules whose first key it is: the last whose `when` holds and whose command is
//!   there now wins. A chord's rule begins the chord (the status bar says it waits for the second key); a single
//!   key's runs. The second key resolves against the chord's rules; one that ends none is "not a command" (said
//!   for 4 s) and does nothing else.
//! - Keys are written as VS Code writes them, in any case: `ctrl+k ctrl+s`, `shift+alt+right`, a key by where it
//!   is (`[KeyA]`, `[Minus]`) or by its Windows name (`oem_1`); a key by where it is is read as the key it is on a
//!   US keyboard. `mac`, `linux` and `win` in a rule of the person's win over `key` on that system.
//! - On macOS a default key with Ctrl has Cmd instead, and a key is shown as macOS shows it (`⇧⌘P`).
//! - The context keys a `when` reads are worked out when a key is pressed, never kept up to date as the window
//!   changes (see `value`); a part adds its own with `context_key` (`inDebugMode` once there is a debugger).
//!   Which part of the window has the keys is told by the views that take them, through `tracked`.
//!
//! keybindings.json is asked for at start, and read again when it is saved or changes on disk. A rule that cannot
//! be read is left out and said (`problems`, and a notification when what cannot be read is other than before).

// The parts to come call what the window's first parts do not yet (a part's context keys, the Keyboard Shortcuts
// editor's list of what could not be read).
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use floem::event::EventListener;
use floem::keyboard::{Key, Modifiers};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::Decorators;
use floem::{IntoView, View};
use serde_json::{Value, json};

use crate::commands::{self, Chord, Pressed, parse_keys};
use crate::state::{Send, Ui};

pub mod defaults;
pub mod editor;
pub mod when;

/// The name of the person's file of keys.
pub const FILE: &str = "keybindings.json";

/// How long "not a command" is said.
const SAID_FOR: Duration = Duration::from_secs(4);

/// The system the keys are for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    Mac,
    Windows,
}

/// The system the window runs on.
pub fn os() -> Os {
    if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    }
}

/// Where a rule comes from, the later winning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Default,
    Extension,
    User,
}

/// A key rule: its keys run its command where its `when` holds.
#[derive(Clone, Debug)]
pub struct Rule {
    pub keys: Vec<Chord>,
    pub command: String,
    pub when: Option<Rc<when::Expr>>,
    /// The `when` as written (a rule taking another away compares it).
    pub when_text: String,
    pub source: Source,
    /// The keys as the command gave them ("Shift+Alt+Right"): shown as they are, off macOS.
    pub written: Option<String>,
}

impl Rule {
    fn holds(&self, key: &dyn Fn(&str) -> Value) -> bool {
        self.when.as_ref().is_none_or(|w| w.holds(key))
    }
}

/// A key's name as VS Code writes it, as `Chord` names the key pressed: in lower case, a key by where it is
/// (`[KeyA]`) and by its Windows name (`oem_1`) as the key it is on a US keyboard.
pub fn key_name(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    if let Some(code) = lower.strip_prefix('[').and_then(|c| c.strip_suffix(']')) {
        if let Some(letter) = code.strip_prefix("key").filter(|l| l.len() == 1) {
            return letter.into();
        }
        if let Some(digit) = code.strip_prefix("digit").filter(|d| d.len() == 1) {
            return digit.into();
        }
        let named = match code {
            "minus" => "-",
            "equal" => "=",
            "bracketleft" => "[",
            "bracketright" => "]",
            "backslash" | "intlbackslash" => "\\",
            "semicolon" => ";",
            "quote" => "'",
            "comma" => ",",
            "period" => ".",
            "slash" => "/",
            "backquote" => "`",
            "arrowup" => "up",
            "arrowdown" => "down",
            "arrowleft" => "left",
            "arrowright" => "right",
            other => other,
        };
        return named.into();
    }
    let named = match lower.as_str() {
        "oem_1" => ";",
        "oem_plus" => "=",
        "oem_comma" => ",",
        "oem_minus" => "-",
        "oem_period" => ".",
        "oem_2" => "/",
        "oem_3" => "`",
        "oem_4" => "[",
        "oem_5" | "oem_102" => "\\",
        "oem_6" => "]",
        "oem_7" => "'",
        "esc" => "escape",
        "return" => "enter",
        "uparrow" => "up",
        "downarrow" => "down",
        "leftarrow" => "left",
        "rightarrow" => "right",
        other => other,
    };
    named.into()
}

/// A default key as it is on macOS: Ctrl is Cmd.
pub fn for_mac(keys: &[Chord]) -> Vec<Chord> {
    keys.iter()
        .map(|c| {
            let mut c = c.clone();
            if c.ctrl && !c.meta {
                c.ctrl = false;
                c.meta = true;
            }
            c
        })
        .collect()
}

/// A key's own name as it is shown: a letter in capitals, `UpArrow`, `PageDown`, `F5`.
fn key_label(key: &str) -> String {
    match key {
        "escape" => "Escape".into(),
        "enter" => "Enter".into(),
        "tab" => "Tab".into(),
        "space" => "Space".into(),
        "backspace" => "Backspace".into(),
        "delete" => "Delete".into(),
        "insert" => "Insert".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        "up" => "UpArrow".into(),
        "down" => "DownArrow".into(),
        "left" => "LeftArrow".into(),
        "right" => "RightArrow".into(),
        k => k.to_uppercase(),
    }
}

/// One key as shown: `Ctrl+Shift+P` (Linux and Windows, Super and Win for the system's key), `⇧⌘P` on macOS.
pub fn chord_label(c: &Chord, os: Os) -> String {
    let key = key_label(&c.key);
    if os == Os::Mac {
        let mut s = String::new();
        for (on, sign) in [(c.ctrl, '⌃'), (c.alt, '⌥'), (c.shift, '⇧'), (c.meta, '⌘')] {
            if on {
                s.push(sign);
            }
        }
        return s + &key;
    }
    let meta = if os == Os::Windows { "Win" } else { "Super" };
    let mut parts: Vec<&str> = Vec::new();
    for (on, name) in [(c.ctrl, "Ctrl"), (c.shift, "Shift"), (c.alt, "Alt"), (c.meta, meta)] {
        if on {
            parts.push(name);
        }
    }
    parts.push(&key);
    parts.join("+")
}

/// Keys as shown, a chord's two with a space between.
pub fn label(keys: &[Chord], os: Os) -> String {
    keys.iter().map(|c| chord_label(c, os)).collect::<Vec<_>>().join(" ")
}

/// A rule of the person's, as keybindings.json gives it: its keys (none: a `-id` for all of the command's), its
/// command (`-id` takes rules away), its `when`.
#[derive(Clone, Debug)]
pub struct UserRule {
    pub keys: Option<Vec<Chord>>,
    pub command: String,
    pub when: Option<Rc<when::Expr>>,
    pub when_text: String,
}

/// keybindings.json read: the rules it has, and what could not be read.
#[derive(Clone, Debug, Default)]
pub struct Read {
    pub rules: Vec<UserRule>,
    pub problems: Vec<String>,
}

/// The person's rules from keybindings.json's text (comments and trailing commas allowed, as VS Code's). An empty
/// file has none.
pub fn read_user(text: &str, os: Os) -> Read {
    let mut read = Read::default();
    if text.trim().is_empty() {
        return read;
    }
    let all = match ca_settings::jsonc::parse(text) {
        Ok(Value::Array(all)) => all,
        Ok(_) => {
            read.problems.push("keybindings.json is not a list of rules ([ … ]).".into());
            return read;
        }
        Err(e) => {
            read.problems.push(format!("keybindings.json cannot be read: {e}"));
            return read;
        }
    };
    let own = match os {
        Os::Mac => "mac",
        Os::Linux => "linux",
        Os::Windows => "win",
    };
    for (n, rule) in all.iter().enumerate() {
        let n = n + 1;
        let Some(o) = rule.as_object() else {
            read.problems.push(format!("Rule {n} is not an object."));
            continue;
        };
        let command = o.get("command").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let key = o.get(own).or_else(|| o.get("key")).and_then(Value::as_str).map(str::trim);
        let named = |what: &str| match key {
            Some(k) => format!("Rule {n} ({k}): {what}"),
            None => format!("Rule {n}: {what}"),
        };
        if command.is_empty() || command == "-" {
            read.problems.push(named("it has no \"command\"."));
            continue;
        }
        let keys = match key {
            Some(k) => match parse_keys(k) {
                Some(keys) if keys.len() <= 2 => Some(keys),
                Some(_) => {
                    read.problems.push(named("a rule has one key or a chord of two."));
                    continue;
                }
                None => {
                    read.problems.push(named(&format!("\"{k}\" is not a key.")));
                    continue;
                }
            },
            None if command.starts_with('-') => None,
            None => {
                read.problems.push(named("it has no \"key\"."));
                continue;
            }
        };
        let when_text = o.get("when").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let when = if when_text.is_empty() {
            None
        } else {
            match when::parse(&when_text) {
                Ok(e) => Some(Rc::new(e)),
                Err(e) => {
                    read.problems.push(named(&format!("its \"when\" cannot be read: {e}.")));
                    continue;
                }
            }
        };
        read.rules.push(UserRule { keys, command, when, when_text });
    }
    read
}

/// The rules in force: those given, in order, then the person's, each `-id` taking away what it names before it.
pub fn with_user(mut rules: Vec<Rule>, user: &[UserRule]) -> Vec<Rule> {
    for u in user {
        if let Some(gone) = u.command.strip_prefix('-') {
            rules.retain(|r| {
                let same = r.command == gone
                    && u.keys.as_ref().is_none_or(|k| *k == r.keys)
                    && (u.when_text.is_empty() || u.when_text == r.when_text);
                !same
            });
            continue;
        }
        if let Some(keys) = &u.keys {
            rules.push(Rule {
                keys: keys.clone(),
                command: u.command.clone(),
                when: u.when.clone(),
                when_text: u.when_text.clone(),
                source: Source::User,
                written: None,
            });
        }
    }
    rules
}

/// Of the rules bound to keys (in order), the one that wins: the last whose `when` holds and whose command is there.
pub fn pick<'a>(
    rules: impl DoubleEndedIterator<Item = &'a Rule>,
    key: &dyn Fn(&str) -> Value,
    there: &dyn Fn(&str) -> bool,
) -> Option<&'a Rule> {
    rules.rev().find(|r| r.holds(key) && there(&r.command))
}

/// The rules in force, by their first key, and the person's.
#[derive(Default)]
struct Table {
    rules: Vec<Rule>,
    /// The rules whose first key it is, by their places, in order.
    by_first: HashMap<Chord, Vec<usize>>,
    by_command: HashMap<String, Vec<usize>>,
    user: Vec<UserRule>,
    /// Commands or rules were added since the table was made: it is made again before a key is looked up.
    stale: bool,
}

impl Table {
    fn make(&mut self) {
        if !std::mem::take(&mut self.stale) {
            return;
        }
        self.rules = with_user(given(os()), &self.user);
        self.by_first.clear();
        self.by_command.clear();
        for (at, r) in self.rules.iter().enumerate() {
            self.by_first.entry(r.keys[0].clone()).or_default().push(at);
            self.by_command.entry(r.command.clone()).or_default().push(at);
        }
    }
}

/// The rules before the person's: the default table's, then the commands' own keys, in order.
pub fn given(os: Os) -> Vec<Rule> {
    let mac = |keys: Vec<Chord>| if os == Os::Mac { for_mac(&keys) } else { keys };
    let mut rules = defaults::rules(os);
    for c in commands::all() {
        for text in &c.keys {
            if let Some(keys) = parse_keys(text) {
                rules.push(Rule {
                    keys: mac(keys),
                    command: c.id.clone(),
                    when: None,
                    when_text: String::new(),
                    source: Source::Default,
                    written: Some(text.clone()),
                });
            }
        }
    }
    rules
}

/// The rules in force now, in order (the later winning), the person's among them.
pub fn rules() -> Vec<Rule> {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        t.make();
        t.rules.clone()
    })
}

/// Which part of the window has the keys, as the views that take them say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    None,
    /// A file's editor.
    Editor,
    /// A box to type in (the find widget's, the palette's, the composer).
    Input,
    Terminal,
    /// The Explorer's tree.
    Explorer,
}

/// A context key a part works out.
type ContextKey = Rc<dyn Fn(Ui) -> Value>;

thread_local! {
    static TABLE: RefCell<Table> = RefCell::new(Table { stale: true, ..Table::default() });
    /// The first key of a chord, pressed: the next key ends it.
    static PENDING: RefCell<Option<Chord>> = const { RefCell::new(None) };
    /// What has the keys, and which of the views that say so (a view losing the keys after another took them
    /// changes nothing).
    static FOCUS: Cell<(Focus, u64)> = const { Cell::new((Focus::None, 0)) };
    static NEXT_VIEW: Cell<u64> = const { Cell::new(1) };
    static CONTEXT: RefCell<HashMap<String, ContextKey>> = RefCell::new(HashMap::new());
}

/// Commands were added (`commands::add`): the table is made again before the next key.
pub fn stale() {
    TABLE.with(|t| t.borrow_mut().stale = true);
}

/// The person's rules, in force from the next key on.
pub fn set_user(rules: Vec<UserRule>) {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        t.user = rules;
        t.stale = true;
    });
}

/// A context key of a part's own, worked out when a key is pressed; one of the window's of the same name gives way.
pub fn context_key(name: &str, value: impl Fn(Ui) -> Value + 'static) {
    CONTEXT.with(|c| c.borrow_mut().insert(name.into(), Rc::new(value)));
}

/// A view that takes the keys says so: while it has them, the context keys say what it is.
pub fn tracked<V: View + 'static>(v: V, focus: Focus) -> V {
    let me = NEXT_VIEW.with(|n| n.replace(n.get() + 1));
    v.on_event_cont(EventListener::FocusGained, move |_| FOCUS.with(|f| f.set((focus, me)))).on_event_cont(
        EventListener::FocusLost,
        move |_| {
            FOCUS.with(|f| {
                if f.get().1 == me {
                    f.set((Focus::None, 0));
                }
            })
        },
    )
}

/// `tracked` in a chain of a view's calls: `.takes_keys(Focus::Explorer)`.
pub trait TakesKeys: View + Sized + 'static {
    fn takes_keys(self, focus: Focus) -> Self {
        tracked(self, focus)
    }
}

impl<V: View + 'static> TakesKeys for V {}

/// What has the keys now.
pub fn focus() -> Focus {
    FOCUS.with(|f| f.get().0)
}

/// A context key's value now, as a `when` reads it.
pub fn value(ui: Ui, key: &str) -> Value {
    if let Some(own) = CONTEXT.with(|c| c.borrow().get(key).cloned()) {
        return own(ui);
    }
    use crate::layout::View;
    let f = focus();
    let active = || ui.active.get_untracked().and_then(|p| ui.tab(&p));
    let file = || active().filter(|t| t.is_file()).map(|t| t.file);
    let shown = |v: View| ui.group_shown(v);
    match key {
        "editorTextFocus" | "editorFocus" => json!(f == Focus::Editor),
        "textInputFocus" | "inputFocus" => json!(matches!(f, Focus::Editor | Focus::Input)),
        "terminalFocus" => json!(f == Focus::Terminal),
        "filesExplorerFocus" | "explorerViewletFocus" | "listFocus" => json!(f == Focus::Explorer),
        "sideBarVisible" => json!(shown(View::Files)),
        "panelVisible" => json!(shown(View::Terminal)),
        "auxiliaryBarVisible" => json!(shown(View::Agent)),
        "editorIsOpen" => json!(ui.active.with_untracked(Option::is_some)),
        "activeEditor" => json!(active().map_or(String::new(), |t| match t.kind {
            Some(kind) => kind.to_string(),
            None if t.diff.is_some() => "workbench.editors.textDiffEditor".into(),
            None => "workbench.editors.files.textFileEditor".into(),
        })),
        "textCompareEditorVisible" | "isInDiffEditor" | "textCompareEditorActive" => {
            json!(active().is_some_and(|t| t.diff.is_some() || t.kind.is_some_and(|k| k.contains("compare"))))
        }
        "activeViewlet" => json!(
            ui.drawn()
                .with_untracked(|d| {
                    let g = d.side_bar()?;
                    match d.find(g) {
                        Some(crate::dock::Node::Tabs { active, .. }) if !ui.hidden(g) => {
                            Some(active.command().to_string())
                        }
                        _ => None,
                    }
                })
                .unwrap_or_default()
        ),
        "resourceLangId" | "editorLangId" => {
            json!(file().map_or("", |p| crate::language::now(ui, &p)))
        }
        "resourceFilename" => json!(file().map_or(String::new(), |p| p.rsplit('/').next().unwrap_or("").into())),
        "resourceExtname" => json!(file().map_or(String::new(), |p| {
            let name = p.rsplit('/').next().unwrap_or("");
            name.rfind('.').filter(|at| *at > 0).map_or(String::new(), |at| name[at..].to_string())
        })),
        "resourcePath" | "resource" => json!(file().unwrap_or_default()),
        "inDebugMode" => json!(false),
        "debugState" => json!("inactive"),
        "inQuickOpen" => json!(ui.quick.with_untracked(Option::is_some)),
        "isLinux" => json!(os() == Os::Linux),
        "isMac" => json!(os() == Os::Mac),
        "isWindows" => json!(os() == Os::Windows),
        "isWeb" => json!(false),
        k => match k.strip_prefix("config.") {
            Some(setting) => ui.state::<crate::settings::Settings>().said.with_untracked(|s| s.value(setting, None)),
            None => Value::Null,
        },
    }
}

/// The rule a key runs now, of those whose first key it is (a chord's rule: the chord begins).
fn first_rule(ui: Ui, chord: &Chord) -> Option<Rule> {
    let rules: Vec<Rule> = TABLE.with(|t| {
        let mut t = t.borrow_mut();
        t.make();
        t.by_first.get(chord).map(|at| at.iter().map(|i| t.rules[*i].clone()).collect()).unwrap_or_default()
    });
    pick(rules.iter(), &|k| value(ui, k), &|id| there(ui, id)).cloned()
}

/// The rule a chord of two runs now.
fn chord_rule(ui: Ui, keys: &[Chord]) -> Option<Rule> {
    let rules: Vec<Rule> = TABLE.with(|t| {
        let mut t = t.borrow_mut();
        t.make();
        t.by_first
            .get(&keys[0])
            .map(|at| at.iter().map(|i| &t.rules[*i]).filter(|r| r.keys == keys).cloned().collect())
            .unwrap_or_default()
    });
    pick(rules.iter(), &|k| value(ui, k), &|id| there(ui, id)).cloned()
}

fn there(ui: Ui, id: &str) -> bool {
    commands::find(id).is_some_and(|c| c.offered(ui))
}

/// The command bound to a key now (none: no rule's command is there, or the key begins a chord).
pub fn command_of(ui: Ui, key: &Key, m: Modifiers) -> Option<String> {
    let chord = Chord::of(key, m)?;
    first_rule(ui, &chord).filter(|r| r.keys.len() == 1).map(|r| r.command)
}

/// Whether a key begins a chord now.
pub fn begins_chord(ui: Ui, key: &Key, m: Modifiers) -> bool {
    Chord::of(key, m).and_then(|c| first_rule(ui, &c)).is_some_and(|r| r.keys.len() > 1)
}

/// Whether a key is bound to a command now, or begins a chord.
pub fn bound(ui: Ui, key: &Key, m: Modifiers) -> bool {
    Chord::of(key, m).and_then(|c| first_rule(ui, &c)).is_some()
}

/// The keys shown for a command: the person's last rule of it, else its first default; empty when it has none.
pub fn shown_keys(id: &str) -> String {
    let os = os();
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        t.make();
        let Some(at) = t.by_command.get(id) else { return String::new() };
        let rules: Vec<&Rule> = at.iter().map(|i| &t.rules[*i]).collect();
        let chosen = rules
            .iter()
            .rev()
            .find(|r| r.source == Source::User)
            .or_else(|| rules.iter().find(|r| r.written.is_some()))
            .or_else(|| rules.first());
        match chosen {
            Some(r) => match (&r.written, os) {
                (Some(w), Os::Linux | Os::Windows) => w.clone(),
                _ => label(&r.keys, os),
            },
            None => String::new(),
        }
    })
}

/// The first key of a chord that waits for its second (none: no chord is begun).
pub fn waiting() -> Option<Chord> {
    PENDING.with(|p| p.borrow().clone())
}

/// The keymap's own in the window: what the status bar says of a chord, and what keybindings.json had wrong.
#[derive(Clone, Copy)]
pub struct Keymap {
    /// What the chord item says (empty: nothing).
    pub chord: RwSignal<String>,
    /// Counted up at each "not a command": a later one is not cleared by an earlier one's timer.
    said: RwSignal<u64>,
    /// The rules of keybindings.json that could not be read, as said.
    pub problems: RwSignal<Rc<Vec<String>>>,
}

impl crate::ext::State for Keymap {
    fn new(ui: Ui) -> Self {
        Keymap {
            chord: ui.scope.create_rw_signal(String::new()),
            said: ui.scope.create_rw_signal(0),
            problems: ui.scope.create_rw_signal(Rc::new(Vec::new())),
        }
    }
}

/// What could not be read of keybindings.json.
pub fn problems(ui: Ui) -> Rc<Vec<String>> {
    ui.state::<Keymap>().problems.get_untracked()
}

fn say(ui: Ui, text: String) {
    let k = ui.state::<Keymap>();
    if k.chord.with_untracked(|c| *c != text) {
        k.chord.set(text);
    }
}

fn set_pending(ui: Ui, chord: Option<Chord>) {
    let os = os();
    if let Some(first) = &chord {
        say(ui, format!("({}) was pressed. Waiting for the second key of the chord…", chord_label(first, os)));
    }
    PENDING.with(|p| *p.borrow_mut() = chord);
}

/// A key pressed in the window, taken to the command bound to it (`commands::press` is this).
pub fn press(ui: Ui, send: &Send, key: &Key, m: Modifiers) -> Pressed {
    let Some(chord) = Chord::of(key, m) else { return Pressed::Free };
    if let Some(first) = waiting() {
        let keys = [first.clone(), chord.clone()];
        if let Some(rule) = chord_rule(ui, &keys) {
            set_pending(ui, None);
            say(ui, String::new());
            run(ui, send, &rule.command);
            return Pressed::Ran;
        }
        // The same first key again (a key one view handed on and another was given too) still waits.
        if chord == first {
            return Pressed::Waiting;
        }
        set_pending(ui, None);
        let os = os();
        say(
            ui,
            format!("The key combination ({}, {}) is not a command.", chord_label(&first, os), chord_label(&chord, os)),
        );
        let k = ui.state::<Keymap>();
        let n = k.said.get_untracked() + 1;
        k.said.set(n);
        floem::action::exec_after(SAID_FOR, move |_| {
            if k.said.get_untracked() == n && waiting().is_none() {
                say(ui, String::new());
            }
        });
        return Pressed::Ran;
    }
    match first_rule(ui, &chord) {
        Some(rule) if rule.keys.len() > 1 => {
            set_pending(ui, Some(chord));
            Pressed::Waiting
        }
        Some(rule) => {
            run(ui, send, &rule.command);
            Pressed::Ran
        }
        None => Pressed::Free,
    }
}

/// Runs a rule's command, when it may run now.
fn run(ui: Ui, send: &Send, id: &str) {
    if let Some(c) = commands::find(id).filter(|c| c.may_run(ui)) {
        (c.run)(ui, send);
    }
}

/// keybindings.json's text taken in: its rules in force, what could not be read said when it is news.
fn take_user(ui: Ui, read: Read) {
    set_user(read.rules);
    let k = ui.state::<Keymap>();
    if k.problems.with_untracked(|p| **p == read.problems) {
        return;
    }
    if let Some(first) = read.problems.first() {
        let n = read.problems.len();
        let more = if n > 1 { format!(" (and {} more)", n - 1) } else { String::new() };
        crate::widgets::toast::warning(ui, &format!("keybindings.json: {first}{more}"));
    }
    k.problems.set(Rc::new(read.problems));
}

/// What it adds to the window's registries, once at start: the status bar's chord item.
pub fn register(_ui: Ui, _send: &Send) {
    crate::statusbar::add("status.chord", crate::statusbar::Side::Left, 160, |ui, _send| {
        let t = ui.colors;
        let text = ui.state::<Keymap>().chord;
        floem::views::label(move || text.get())
            .style(move |s| crate::statusbar::item(t, s).apply_if(text.with(String::is_empty), |s| s.hide()))
            .into_any()
    });
}

/// The session's messages that are this part's, taken in: keybindings.json asked for when the session is there,
/// and read when it comes, is saved, or changes on disk. None is taken away from the parts after it (the user
/// file's tab takes it too).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let name = m["name"].as_str();
    match m["type"].as_str().unwrap_or("") {
        "state" => send(json!({"type": "user_file", "name": FILE})),
        "saved_user" if name == Some(FILE) && m.get("error").is_none() => {
            send(json!({"type": "user_file", "name": FILE}))
        }
        "user_file" if name == Some(FILE) => match m["error"].as_str() {
            Some(e) => take_user(ui, Read { rules: Vec::new(), problems: vec![format!("cannot be read: {e}")] }),
            None => take_user(ui, read_user(m["text"].as_str().unwrap_or(""), os())),
        },
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use floem::keyboard::NamedKey;

    use super::*;

    fn ch(text: &str) -> Chord {
        Chord::parse(text).expect("a key")
    }

    fn rule(keys: &str, command: &str, when: &str) -> Rule {
        Rule {
            keys: parse_keys(keys).expect("keys"),
            command: command.into(),
            when: (!when.is_empty()).then(|| Rc::new(when::parse(when).expect("when"))),
            when_text: when.into(),
            source: Source::Default,
            written: None,
        }
    }

    #[test]
    fn keys_are_read_as_vs_code_writes_them() {
        assert_eq!(ch("ctrl+shift+p"), ch("Ctrl+Shift+P"));
        assert_eq!(ch("ctrl+[KeyA]"), ch("Ctrl+A"));
        assert_eq!(ch("ctrl+[Digit1]").key, "1");
        assert_eq!(ch("ctrl+[Minus]").key, "-");
        assert_eq!(ch("ctrl+[Backquote]").key, "`");
        assert_eq!(ch("ctrl+oem_1").key, ";");
        assert_eq!(ch("ctrl+oem_plus").key, "=");
        assert_eq!(ch("ctrl+oem_2").key, "/");
        assert_eq!(ch("shift+alt+[ArrowRight]"), ch("Shift+Alt+Right"));
        assert_eq!(ch("esc").key, "escape");
        assert_eq!(parse_keys("ctrl+k ctrl+s").map(|k| k.len()), Some(2));
        assert_eq!(ch("cmd+p"), ch("Meta+P"));
    }

    #[test]
    fn keys_are_shown_as_each_system_shows_them_and_ctrl_is_cmd_on_a_mac() {
        let p = parse_keys("Ctrl+Shift+P").expect("keys");
        assert_eq!(label(&p, Os::Linux), "Ctrl+Shift+P");
        assert_eq!(label(&for_mac(&p), Os::Mac), "⇧⌘P");
        assert_eq!(label(&parse_keys("ctrl+k ctrl+s").expect("keys"), Os::Windows), "Ctrl+K Ctrl+S");
        assert_eq!(label(&for_mac(&parse_keys("ctrl+k ctrl+s").expect("keys")), Os::Mac), "⌘K ⌘S");
        assert_eq!(label(&parse_keys("shift+alt+right").expect("keys"), Os::Linux), "Shift+Alt+RightArrow");
        assert_eq!(label(&parse_keys("ctrl+alt+shift+meta+f5").expect("keys"), Os::Mac), "⌃⌥⇧⌘F5");
        assert_eq!(label(&parse_keys("meta+e").expect("keys"), Os::Linux), "Super+E");
        assert_eq!(label(&parse_keys("meta+e").expect("keys"), Os::Windows), "Win+E");
        // A key with Cmd already keeps its Ctrl.
        assert_eq!(label(&for_mac(&parse_keys("ctrl+cmd+f").expect("keys")), Os::Mac), "⌃⌘F");
    }

    #[test]
    fn the_persons_rules_are_read_and_what_cannot_be_read_is_said() {
        let text = r#"// my keys
        [
            { "key": "ctrl+alt+p", "command": "workbench.action.showCommands" },
            { "key": "ctrl+k ctrl+x", "command": "a.b", "when": "editorTextFocus && !inDebugMode" },
            { "key": "ctrl+b", "command": "-workbench.action.toggleSidebarVisibility" },
            { "command": "-x.y" },
            { "key": "ctrl+e", "mac": "cmd+e", "command": "e" },
            { "key": "ctrl+e" },
            { "key": "hyper+q", "command": "q" },
            { "key": "ctrl+q", "command": "q", "when": "a &&" },
            { "command": "q" },
            { "key": "a b c", "command": "q" },
            3,
        ]"#;
        let read = read_user(text, Os::Linux);
        assert_eq!(read.rules.len(), 5, "{:?}", read.problems);
        assert_eq!(read.rules[1].keys.as_ref().map(Vec::len), Some(2));
        assert!(read.rules[1].when.is_some());
        assert!(read.rules[3].keys.is_none());
        assert_eq!(read.rules[4].keys, parse_keys("ctrl+e"));
        assert_eq!(read_user(text, Os::Mac).rules[4].keys, parse_keys("cmd+e"));
        assert_eq!(read.problems.len(), 6, "{:?}", read.problems);
        assert_eq!(read.problems[0], "Rule 6 (ctrl+e): it has no \"command\".");
        assert_eq!(read.problems[1], "Rule 7 (hyper+q): \"hyper+q\" is not a key.");
        assert!(read.problems[2].starts_with("Rule 8 (ctrl+q): its \"when\" cannot be read"));
        assert_eq!(read.problems[5], "Rule 11 is not an object.");
        assert!(read_user("", Os::Linux).problems.is_empty());
        assert!(read_user("  \n", Os::Linux).rules.is_empty());
        assert_eq!(read_user("{}", Os::Linux).problems.len(), 1);
        assert_eq!(read_user("[ {", Os::Linux).problems.len(), 1);
    }

    #[test]
    fn the_last_rule_that_holds_wins_and_a_minus_rule_takes_rules_away() {
        let defaults = vec![
            rule("ctrl+b", "toggle", ""),
            rule("ctrl+b", "bold", "editorTextFocus"),
            rule("ctrl+e", "explorer", ""),
            rule("ctrl+e", "explorer", "sideBarVisible"),
            rule("ctrl+k ctrl+t", "theme", ""),
        ];
        let user = read_user(
            r#"[
                { "key": "ctrl+e", "command": "-explorer", "when": "sideBarVisible" },
                { "command": "-theme" },
                { "key": "ctrl+k ctrl+t", "command": "mine" },
                { "key": "ctrl+t", "command": "gone" },
            ]"#,
            Os::Linux,
        );
        let rules = with_user(defaults, &user.rules);
        let on = |key: &'static str| move |k: &str| serde_json::json!(k == key);
        let all = |_: &str| true;
        let of = |keys: &str, ctx: &dyn Fn(&str) -> Value, there: &dyn Fn(&str) -> bool| {
            let keys = parse_keys(keys).expect("keys");
            pick(rules.iter().filter(|r| r.keys == keys), ctx, there).map(|r| r.command.clone())
        };
        assert_eq!(of("ctrl+b", &on("none"), &all).as_deref(), Some("toggle"));
        assert_eq!(of("ctrl+b", &on("editorTextFocus"), &all).as_deref(), Some("bold"));
        // A command that is not there now gives way to the one before it.
        assert_eq!(of("ctrl+b", &on("editorTextFocus"), &|c| c != "bold").as_deref(), Some("toggle"));
        // Only the rule of that `when` was taken away.
        assert_eq!(rules.iter().filter(|r| r.command == "explorer").count(), 1);
        assert_eq!(of("ctrl+k ctrl+t", &on("x"), &all).as_deref(), Some("mine"));
        assert!(!rules.iter().any(|r| r.command == "theme"));
        assert_eq!(rules.last().map(|r| r.source), Some(Source::User));
    }

    #[test]
    fn a_chord_says_it_waits_and_a_key_that_ends_none_is_not_a_command() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let said = move |text: &'static str| move |ui: Ui, _: &Send| ui.message.set(text.into());
        commands::add(commands::command("test.km.chord", "", "Chord", said("chord")).keys("Ctrl+Alt+K Ctrl+Alt+U"));
        commands::add(commands::command("test.km.single", "", "Single", said("single")).keys("Ctrl+Alt+J"));
        let p = |c: &str| press(ui, &send, &Key::Character(c.into()), Modifiers::CONTROL | Modifiers::ALT);
        let chord = ui.state::<Keymap>().chord;
        assert_eq!(p("k"), Pressed::Waiting);
        assert_eq!(chord.get_untracked(), "(Ctrl+Alt+K) was pressed. Waiting for the second key of the chord…");
        // The same key given again still waits; a key held changes nothing.
        assert_eq!(p("k"), Pressed::Waiting);
        assert_eq!(press(ui, &send, &Key::Named(NamedKey::Alt), Modifiers::ALT), Pressed::Free);
        assert_eq!(p("j"), Pressed::Ran);
        assert_eq!(ui.message.get_untracked(), "", "the single key's command did not run");
        assert_eq!(chord.get_untracked(), "The key combination (Ctrl+Alt+K, Ctrl+Alt+J) is not a command.");
        assert_eq!(waiting(), None);
        assert_eq!(p("j"), Pressed::Ran);
        assert_eq!(ui.message.get_untracked(), "single");
        assert_eq!(p("k"), Pressed::Waiting);
        assert_eq!(p("u"), Pressed::Ran);
        assert_eq!(ui.message.get_untracked(), "chord");
        assert_eq!(chord.get_untracked(), "");
        // The person's rule wins, and the default is shown no more.
        assert_eq!(shown_keys("test.km.single"), "Ctrl+Alt+J");
        set_user(read_user(r#"[{"key": "ctrl+alt+[KeyY]", "command": "test.km.chord"}]"#, Os::Linux).rules);
        assert_eq!(p("y"), Pressed::Ran);
        assert_eq!(shown_keys("test.km.chord"), "Ctrl+Alt+Y");
        assert_eq!(
            command_of(ui, &Key::Character("j".into()), Modifiers::CONTROL | Modifiers::ALT).as_deref(),
            Some("test.km.single")
        );
        assert!(begins_chord(ui, &Key::Character("k".into()), Modifiers::CONTROL | Modifiers::ALT));
        set_user(Vec::new());
    }

    #[test]
    fn when_clauses_read_the_context_keys_as_they_are_when_the_key_is_pressed() {
        let ui = Ui::new();
        assert_eq!(value(ui, "editorIsOpen"), json!(false));
        assert_eq!(value(ui, "activeEditor"), json!(""));
        assert_eq!(value(ui, "isLinux"), json!(os() == Os::Linux));
        assert_eq!(value(ui, "nothing.of.the.kind"), Value::Null);
        assert_eq!(value(ui, "terminalFocus"), json!(false));
        context_key("test.km.mine", |_| json!("yes"));
        assert_eq!(value(ui, "test.km.mine"), json!("yes"));
        FOCUS.with(|f| f.set((Focus::Terminal, 9)));
        assert_eq!(value(ui, "terminalFocus"), json!(true));
        assert_eq!(value(ui, "inputFocus"), json!(false));
        FOCUS.with(|f| f.set((Focus::Editor, 9)));
        assert_eq!(value(ui, "editorTextFocus"), json!(true));
        assert_eq!(value(ui, "inputFocus"), json!(true));
        FOCUS.with(|f| f.set((Focus::None, 0)));
    }

    #[test]
    fn keybindings_json_is_asked_for_read_and_its_problems_said_once() {
        let ui = Ui::new();
        let sent = Rc::new(RefCell::new(Vec::new()));
        let s = sent.clone();
        let send: Send = Rc::new(move |m| s.borrow_mut().push(m));
        take(ui, &json!({"type": "state"}), &send);
        assert_eq!(sent.borrow().last(), Some(&json!({"type": "user_file", "name": FILE})));
        let bad = json!({"type": "user_file", "name": FILE, "text": "[{\"key\": \"ctrl+q\"}]", "modified": 1});
        take(ui, &bad, &send);
        assert_eq!(problems(ui).len(), 1);
        let toasts = crate::widgets::toast::count(ui);
        take(ui, &bad, &send);
        assert_eq!(crate::widgets::toast::count(ui), toasts, "said once");
        // A file that is not there has no rules and nothing wrong.
        take(ui, &json!({"type": "user_file", "name": FILE, "text": "", "modified": null}), &send);
        assert!(problems(ui).is_empty());
        // Another of the person's files is not this one.
        take(ui, &json!({"type": "user_file", "name": "settings.json", "text": "{"}), &send);
        assert!(problems(ui).is_empty());
        sent.borrow_mut().clear();
        take(ui, &json!({"type": "saved_user", "name": FILE, "modified": 2}), &send);
        assert_eq!(sent.borrow().as_slice(), [json!({"type": "user_file", "name": FILE})]);
        set_user(Vec::new());
    }
}
