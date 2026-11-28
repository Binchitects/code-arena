//! The Keyboard Shortcuts editor, as VS Code's (Preferences: Open Keyboard Shortcuts, Ctrl+K Ctrl+S): every command
//! with the keys it has now, one row for each of its rules and one for a command with none, in a table of Command,
//! Keybinding, When and Source. A search narrows it by words (of the title, the id, the keys, the `when`, the
//! source), by keys written in quotes (`"ctrl+k ctrl+s"`), or by keys pressed while Record Keys is on.
//!
//! A row's keys are changed in "Keys for …" (a double-click, Enter, or Ctrl+K Ctrl+K), one key or a chord of two,
//! Enter saving and Escape giving up, with the other commands of those keys named. The row's menu also adds keys,
//! changes the `when` (read before it is written), removes or resets the keys and copies the rule or the command's
//! id. What is changed is written into keybindings.json as VS Code writes it: a rule of the person's is changed in
//! its place; a default one is taken away by a rule of `-command` and the new one added after; the file's comments
//! and the rules not touched stay as they were, to the byte. The rules of the file that do nothing (what could not
//! be read, a command there is not) are said above the table.

use std::rc::Rc;
use std::time::Duration;

use floem::action::exec_after;
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::Rect;
use floem::peniko::Color;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, Position};
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, container, empty, h_stack, h_stack_from_iter, scroll, text_input,
    virtual_stack,
};
use floem::{AnyView, IntoView, View, ViewId};
use serde_json::{Value, json};

use super::{Os, Rule, Source, chord_label, os};
use crate::commands::{self, Chord, parse_keys};
use crate::editor::kinds;
use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::{Theme, popup, quick, toast};

/// A text drawn, as floem's `label` (the keymap's own `label` names keys).
fn label<S: std::fmt::Display + 'static>(text: impl Fn() -> S + 'static) -> floem::views::Label {
    floem::views::label(text)
}

/// What VS Code puts at the top of a keybindings.json it makes.
const HEADER: &str = "// Place your key bindings in this file to override the defaults\n";

/// What VS Code says when keybindings.json cannot be written into.
pub const UNWRITABLE: &str = "Unable to write into the keybindings configuration file. Please open it to correct \
errors/warnings in the file and try again.";

/// A key as keybindings.json has it: `ctrl+shift+p`, a chord's two keys with a space between; on macOS `cmd`, on
/// Windows `win`, elsewhere `meta` for the system's key.
pub fn key_text(keys: &[Chord], os: Os) -> String {
    let meta = match os {
        Os::Mac => "cmd",
        Os::Windows => "win",
        Os::Linux => "meta",
    };
    keys.iter()
        .map(|c| {
            let mut parts: Vec<&str> = Vec::new();
            for (on, name) in [(c.ctrl, "ctrl"), (c.shift, "shift"), (c.alt, "alt"), (c.meta, meta)] {
                if on {
                    parts.push(name);
                }
            }
            parts.push(&c.key);
            parts.join("+")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A key's parts as the table draws them, each in its own box: `Ctrl`, `Shift`, `P` (on macOS `⇧`, `⌘`, `P`).
pub fn key_parts(c: &Chord, os: Os) -> Vec<String> {
    let alone = Chord { ctrl: false, shift: false, alt: false, meta: false, key: c.key.clone() };
    let key = chord_label(&alone, os);
    let mods: Vec<(bool, &str)> = if os == Os::Mac {
        vec![(c.ctrl, "⌃"), (c.alt, "⌥"), (c.shift, "⇧"), (c.meta, "⌘")]
    } else {
        let meta = if os == Os::Windows { "Win" } else { "Super" };
        vec![(c.ctrl, "Ctrl"), (c.shift, "Shift"), (c.alt, "Alt"), (c.meta, meta)]
    };
    let mut parts: Vec<String> = mods.into_iter().filter(|(on, _)| *on).map(|(_, n)| n.to_string()).collect();
    parts.push(key);
    parts
}

/// A row of the table: a rule in force, or a command with no keys.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Its place among all the rows, the same while the rules are.
    pub uid: usize,
    pub command: String,
    /// "Category: Title"; empty for a rule whose command there is not.
    pub title: String,
    pub keys: Option<Vec<Chord>>,
    /// The keys as shown ("Ctrl+K Ctrl+S").
    pub key_label: String,
    /// The keys as keybindings.json has them ("ctrl+k ctrl+s").
    pub key_text: String,
    pub when: String,
    pub source: Source,
    /// What the search reads, in lower case.
    hay: String,
}

impl Entry {
    /// The command's title, else its id (a rule of the person's whose command there is not).
    pub fn name(&self) -> &str {
        if self.title.is_empty() { &self.command } else { &self.title }
    }

    /// Whether its command is there.
    pub fn known(&self) -> bool {
        !self.title.is_empty()
    }

    /// The rule as keybindings.json writes it, for Copy.
    pub fn copied(&self) -> String {
        let mut o = serde_json::Map::new();
        o.insert("key".into(), json!(self.key_text));
        o.insert("command".into(), json!(self.command));
        if !self.when.is_empty() {
            o.insert("when".into(), json!(self.when));
        }
        serde_json::to_string_pretty(&Value::Object(o)).unwrap_or_default()
    }
}

/// The name a rule's source has in the table.
pub fn source_name(s: Source) -> &'static str {
    match s {
        Source::Default => "Default",
        Source::Extension => "Extension",
        Source::User => "User",
    }
}

/// A command as the table needs it: its id, its title ("Category: Title") and whether it is hidden.
pub type Known = (String, String, bool);

/// The table's rows: a row for each rule in force (a default rule only of a command that is there), then one for
/// each command shown in the palette that has none; those with keys first, each part by title, then by id.
pub fn entries(rules: &[Rule], known: &[Known], os: Os) -> Vec<Entry> {
    use std::collections::{HashMap, HashSet};
    let titles: HashMap<&str, &str> = known.iter().map(|(id, title, _)| (id.as_str(), title.as_str())).collect();
    let mut bound: HashSet<&str> = HashSet::new();
    let mut all = Vec::new();
    let make = |command: &str, title: &str, keys: Option<Vec<Chord>>, when: &str, source: Source| {
        let key_label = keys.as_deref().map(|k| super::label(k, os)).unwrap_or_default();
        let key_text = keys.as_deref().map(|k| key_text(k, os)).unwrap_or_default();
        let hay = format!("{title}\n{command}\n{key_label}\n{key_text}\n{when}\n{}", source_name(source)).to_lowercase();
        Entry {
            uid: 0,
            command: command.into(),
            title: title.into(),
            keys,
            key_label,
            key_text,
            when: when.into(),
            source,
            hay,
        }
    };
    for r in rules {
        let title = titles.get(r.command.as_str()).copied();
        if title.is_none() && r.source != Source::User {
            continue;
        }
        bound.insert(&r.command);
        all.push(make(&r.command, title.unwrap_or(""), Some(r.keys.clone()), &r.when_text, r.source));
    }
    for (id, title, hidden) in known {
        if !hidden && !bound.contains(id.as_str()) {
            all.push(make(id, title, None, "", Source::Default));
        }
    }
    all.sort_by(|a, b| {
        (a.keys.is_none(), a.name().to_lowercase(), &a.command).cmp(&(b.keys.is_none(), b.name().to_lowercase(), &b.command))
    });
    for (n, e) in all.iter_mut().enumerate() {
        e.uid = n;
    }
    all
}

/// What a search asks: words each found somewhere in a row, keys (in quotes, or recorded) a row's keys are or
/// begin with, a source (`@source:user`) and a command's id (`@command:id`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    words: Vec<String>,
    keys: Vec<String>,
    source: Option<String>,
    command: Option<String>,
}

/// A search's text read: `"…"` is keys, `@source:` and `@command:` are what VS Code's are, the rest words.
pub fn query(text: &str) -> Query {
    let mut q = Query::default();
    let mut rest = String::new();
    let mut quoted = text.split('"');
    // Every second part is in quotes.
    let mut inside = false;
    for part in quoted.by_ref() {
        if inside {
            let keys = part.trim().to_lowercase();
            if !keys.is_empty() {
                // Keys written as shown ("Ctrl+K") read as keybindings.json has them.
                let keys = parse_keys(&keys).map(|k| key_text(&k, os())).unwrap_or(keys);
                q.keys.push(keys);
            }
        } else {
            rest.push_str(part);
            rest.push(' ');
        }
        inside = !inside;
    }
    for word in rest.split_whitespace() {
        let lower = word.to_lowercase();
        if let Some(s) = lower.strip_prefix("@source:") {
            q.source = Some(s.into());
        } else if let Some(c) = word.strip_prefix("@command:") {
            q.command = Some(c.into());
        } else {
            q.words.push(lower);
        }
    }
    q
}

/// Whether a row is one a search asks for.
pub fn matches(e: &Entry, q: &Query) -> bool {
    q.words.iter().all(|w| e.hay.contains(w.as_str()))
        && q.keys.iter().all(|k| e.key_text == *k || e.key_text.starts_with(&format!("{k} ")))
        && q.source.as_ref().is_none_or(|s| source_name(e.source).eq_ignore_ascii_case(s))
        && q.command.as_ref().is_none_or(|c| e.command == *c)
}

/// The other commands bound to these keys, by name, once each (for "Keys for …").
pub fn others(all: &[Entry], keys: &[Chord], not: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for e in all {
        if e.command != not && e.keys.as_deref() == Some(keys) && !names.iter().any(|n| n == e.name()) {
            names.push(e.name().to_string());
        }
    }
    names
}

/// The rules of the person's that do nothing because their command there is not (what could not be read at all is
/// the keymap's `problems`).
pub fn idle(all: &[Entry]) -> Vec<String> {
    all.iter()
        .filter(|e| e.source == Source::User && !e.known())
        .map(|e| format!("{}: \"{}\" is not a command.", e.key_label, e.command))
        .collect()
}

/// Where keybindings.json's list and its rules are in its text: the `[` and `]`, and each rule from its first byte to
/// after its last.
#[derive(Debug, PartialEq)]
struct Spans {
    open: usize,
    close: usize,
    items: Vec<(usize, usize)>,
}

/// Past spaces and comments from `at`.
fn skip(b: &[u8], mut at: usize) -> usize {
    while at < b.len() {
        match b[at] {
            b' ' | b'\t' | b'\r' | b'\n' => at += 1,
            b'/' if b.get(at + 1) == Some(&b'/') => {
                while at < b.len() && b[at] != b'\n' {
                    at += 1;
                }
            }
            b'/' if b.get(at + 1) == Some(&b'*') => {
                at += 2;
                while at < b.len() && !(b[at] == b'*' && b.get(at + 1) == Some(&b'/')) {
                    at += 1;
                }
                at = (at + 2).min(b.len());
            }
            _ => break,
        }
    }
    at
}

/// After the value that begins at `at` (an object, a list, a string, a word).
fn value_end(b: &[u8], mut at: usize) -> Option<usize> {
    let mut depth = 0usize;
    while at < b.len() {
        match b[at] {
            b'"' => {
                at += 1;
                while at < b.len() && b[at] != b'"' {
                    at += if b[at] == b'\\' { 2 } else { 1 };
                }
                at += 1;
            }
            b'/' if matches!(b.get(at + 1), Some(b'/' | b'*')) => at = skip(b, at),
            b'{' | b'[' => {
                depth += 1;
                at += 1;
            }
            b'}' | b']' if depth == 0 => return Some(at),
            b'}' | b']' => {
                depth -= 1;
                at += 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            b',' | b' ' | b'\t' | b'\r' | b'\n' if depth == 0 => return Some(at),
            _ => at += 1,
        }
    }
    (depth == 0).then_some(at)
}

/// The list's places in the text; none when the text is not a list that can be read.
fn spans(text: &str) -> Option<Spans> {
    if !matches!(ca_settings::jsonc::parse(text), Ok(Value::Array(_))) {
        return None;
    }
    let b = text.as_bytes();
    let open = skip(b, 0);
    if b.get(open) != Some(&b'[') {
        return None;
    }
    let mut items = Vec::new();
    let mut at = open + 1;
    loop {
        at = skip(b, at);
        match b.get(at)? {
            b']' => return Some(Spans { open, close: at, items }),
            b',' => at += 1,
            _ => {
                let end = value_end(b, at)?;
                items.push((at, end));
                at = end;
            }
        }
    }
}

/// A rule as VS Code writes it into keybindings.json, at the list's first level (four spaces in).
fn written(key: &str, command: &str, when: &str) -> String {
    let s = |v: &str| serde_json::to_string(v).unwrap_or_default();
    let mut lines = vec![format!("        \"key\": {}", s(key)), format!("        \"command\": {}", s(command))];
    if !when.is_empty() {
        lines.push(format!("        \"when\": {}", s(when)));
    }
    format!("{{\n{}\n    }}", lines.join(",\n"))
}

/// The text with a rule added at the end of the list.
fn append(text: &str, rule: &str) -> Option<String> {
    if text.trim().is_empty() {
        return Some(format!("{HEADER}[\n    {rule}\n]\n"));
    }
    let sp = spans(text)?;
    let out = match sp.items.last() {
        Some(&(_, end)) => format!("{},\n    {rule}{}", &text[..end], &text[end..]),
        None if text[sp.open + 1..sp.close].trim().is_empty() => {
            format!("{}\n    {rule}\n{}", &text[..sp.open + 1], &text[sp.close..])
        }
        // Only comments in the list: the rule goes after them.
        None => {
            let inner = text[sp.open + 1..sp.close].trim_end();
            format!("{}{inner}\n    {rule}\n{}", &text[..sp.open + 1], &text[sp.close..])
        }
    };
    Some(out)
}

/// The text with its rule at `at` written again.
fn replace(text: &str, at: usize, rule: &str) -> Option<String> {
    let (start, end) = *spans(text)?.items.get(at)?;
    Some(format!("{}{rule}{}", &text[..start], &text[end..]))
}

/// The text without its rule at `at`, nor the comma before it (the first: the one after it).
fn remove(text: &str, at: usize) -> Option<String> {
    let sp = spans(text)?;
    let (start, end) = *sp.items.get(at)?;
    let b = text.as_bytes();
    let (from, to) = if at > 0 {
        (sp.items[at - 1].1, end)
    } else if sp.items.len() > 1 {
        // The first: from its line's start to past the comma after it and the end of that line.
        let line = text[..start].rfind('\n').map_or(0, |n| n + 1);
        let from = if text[line..start].trim().is_empty() { line } else { start };
        let comma = skip(b, end);
        let mut to = if b.get(comma) == Some(&b',') { comma + 1 } else { end };
        if let Some(n) = text[to..].find('\n').filter(|n| text[to..to + n].trim().is_empty()) {
            to += n + 1;
        }
        (from, to)
    } else {
        // The only rule: what is between it and the `[` goes too, and a comma after it.
        let after = skip(b, end);
        let to = if b.get(after) == Some(&b',') { after + 1 } else { end };
        (sp.open + 1, to)
    };
    Some(format!("{}{}", &text[..from], &text[to..]))
}

/// The places of the person's rules that are this row's: of its command, keys and `when`, as `read_user` reads
/// them.
fn places(text: &str, os: Os, e: &Entry) -> Vec<usize> {
    let Some(sp) = spans(text) else { return Vec::new() };
    let own = match os {
        Os::Mac => "mac",
        Os::Linux => "linux",
        Os::Windows => "win",
    };
    sp.items
        .iter()
        .enumerate()
        .filter(|(_, (start, end))| {
            let Ok(v) = ca_settings::jsonc::parse(&text[*start..*end]) else { return false };
            let s = |k: &str| v.get(k).and_then(Value::as_str).map(|s| s.trim().to_string());
            let key = s(own).or_else(|| s("key"));
            s("command").as_deref() == Some(e.command.as_str())
                && key.and_then(|k| parse_keys(&k)) == e.keys
                && s("when").unwrap_or_default() == e.when
        })
        .map(|(n, _)| n)
        .collect()
}

/// A change to a row's keys. A default's keys are taken away (`-command`) before its new rule is written: a rule
/// that takes keys away takes them from every rule before it, the person's too.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Its keys are these instead.
    Keys(Vec<Chord>),
    /// Its command has these keys too.
    Add(Vec<Chord>),
    /// Its keys apply where this holds instead.
    When(String),
    Remove,
    /// The person's own rules of it are taken out, and those that took its defaults away.
    Reset,
}

/// keybindings.json's text with a row changed, as VS Code writes it; none when the file cannot be read or the change
/// is not one the row has.
pub fn edit(text: &str, os: Os, e: &Entry, change: &Change) -> Option<String> {
    if !text.trim().is_empty() {
        spans(text)?;
    }
    let mine = places(text, os, e).last().copied().filter(|_| e.source == Source::User);
    let not = format!("-{}", e.command);
    let take_away = |text: &str, keys: &[Chord]| append(text, &written(&key_text(keys, os), &not, &e.when));
    match change {
        Change::Keys(keys) | Change::Add(keys) if keys.is_empty() => None,
        Change::Add(keys) => append(text, &written(&key_text(keys, os), &e.command, &e.when)),
        Change::Keys(keys) => {
            let rule = written(&key_text(keys, os), &e.command, &e.when);
            match (mine, &e.keys) {
                (Some(at), _) => replace(text, at, &rule),
                (None, Some(old)) if e.source != Source::User => append(&take_away(text, old)?, &rule),
                (None, Some(_)) => None,
                (None, None) => append(text, &written(&key_text(keys, os), &e.command, "")),
            }
        }
        Change::When(when) => {
            let when = when.trim();
            let old = e.keys.as_deref()?;
            let rule = written(&key_text(old, os), &e.command, when);
            match mine {
                Some(at) => replace(text, at, &rule),
                None if e.source != Source::User => append(&take_away(text, old)?, &rule),
                None => None,
            }
        }
        Change::Remove => match (mine, &e.keys) {
            (Some(at), _) => remove(text, at),
            (None, Some(old)) if e.source != Source::User => take_away(text, old),
            _ => None,
        },
        Change::Reset => {
            let sp = spans(text)?;
            let mut gone: Vec<usize> = mine.into_iter().collect();
            for (n, (start, end)) in sp.items.iter().enumerate() {
                let v = ca_settings::jsonc::parse(&text[*start..*end]).ok();
                let s = |k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(Value::as_str).unwrap_or("").trim();
                if s("command") == not && (s("when").is_empty() || s("when") == e.when) {
                    gone.push(n);
                }
            }
            if gone.is_empty() {
                return None;
            }
            gone.sort_unstable();
            gone.dedup();
            let mut out = text.to_string();
            for at in gone.into_iter().rev() {
                out = remove(&out, at)?;
            }
            Some(out)
        }
    }
}

/// Whether keybindings.json's text can be written into (an empty file can: it is made).
pub fn writable(text: &str) -> bool {
    text.trim().is_empty() || spans(text).is_some()
}

/// The commands as the table needs them.
pub fn known() -> Vec<Known> {
    commands::all().iter().map(|c| (c.id.clone(), c.label(), c.hidden)).collect()
}

/// The editor's tab: there is one.
pub const TAB: &str = "keybindings:shortcuts";
const KIND: &str = "keybindings";
/// VS Code's command that opens keybindings.json.
pub const OPEN_JSON: &str = "workbench.action.openGlobalKeybindingsFile";
const DEFINE: &str = "keybindings.editor.defineKeybinding";
const DEFINE_WHEN: &str = "keybindings.editor.defineWhenExpression";
const RECORD: &str = "keybindings.editor.recordSearchKeys";
const ROW: f64 = 24.0;
/// The table's columns, by their parts of the width after the 24 px of the row's action.
const COLUMNS: [f32; 4] = [4.0, 2.5, 2.0, 1.5];

/// "Keys for …" open: for a row, adding keys or changing them, with the keys pressed so far.
#[derive(Clone, Debug, PartialEq)]
struct Define {
    entry: Entry,
    add: bool,
    keys: Vec<Chord>,
}

/// The editor's own, kept while its tab is not shown.
#[derive(Clone, Copy)]
pub struct Shortcuts {
    /// keybindings.json's text as last read or written; none while it is not read, or could not be.
    text: RwSignal<Option<Rc<String>>>,
    /// Counted up each time the file is read: the rules in force may be others.
    read: RwSignal<u64>,
    search: RwSignal<String>,
    recording: RwSignal<bool>,
    recorded: RwSignal<Vec<Chord>>,
    selected: RwSignal<Option<Entry>>,
    define: RwSignal<Option<Define>>,
    /// The table has the keys.
    focused: RwSignal<bool>,
    table: RwSignal<Option<ViewId>>,
    input: RwSignal<Option<ViewId>>,
    recorder: RwSignal<Option<ViewId>>,
}

impl crate::ext::State for Shortcuts {
    fn new(ui: Ui) -> Self {
        Shortcuts {
            text: ui.scope.create_rw_signal(None),
            read: ui.scope.create_rw_signal(0),
            search: ui.scope.create_rw_signal(String::new()),
            recording: ui.scope.create_rw_signal(false),
            recorded: ui.scope.create_rw_signal(Vec::new()),
            selected: ui.scope.create_rw_signal(None),
            define: ui.scope.create_rw_signal(None),
            focused: ui.scope.create_rw_signal(false),
            table: ui.scope.create_rw_signal(None),
            input: ui.scope.create_rw_signal(None),
            recorder: ui.scope.create_rw_signal(None),
        }
    }
}

/// The editor opened (its tab made, or shown), keybindings.json asked for again.
pub fn open(ui: Ui, send: &Send) {
    kinds::open(ui, TAB);
    send(json!({"type": "user_file", "name": super::FILE}));
}

fn focus(id: RwSignal<Option<ViewId>>) {
    exec_after(Duration::ZERO, move |_| {
        if let Some(id) = id.get_untracked() {
            id.request_focus();
        }
    });
}

/// A row changed, written into keybindings.json (the keymap reads it again once it is saved).
fn apply(ui: Ui, send: &Send, e: &Entry, change: Change) {
    let s = ui.state::<Shortcuts>();
    let Some(text) = s.text.get_untracked() else {
        toast::warning(ui, UNWRITABLE);
        return;
    };
    if ui.tab(&crate::settings::user_tab(super::FILE)).is_some_and(|t| t.dirty.get_untracked()) {
        toast::warning(ui, "keybindings.json has changes in its editor that are not saved: save or revert them first.");
        return;
    }
    if !writable(&text) {
        toast::error_of(UNWRITABLE)
            .action("Open Keyboard Shortcuts (JSON)", |ui, send| crate::settings::open_user(ui, send, super::FILE))
            .show_in(ui);
        return;
    }
    if let Some(new) = edit(&text, os(), e, &change) {
        send(json!({"type": "save_user", "name": super::FILE, "text": new}));
        s.text.set(Some(Rc::new(new)));
    }
}

/// "Keys for …" opened for the row picked.
fn define(ui: Ui, add: bool) {
    let s = ui.state::<Shortcuts>();
    if let Some(entry) = s.selected.get_untracked() {
        s.define.set(Some(Define { entry, add, keys: Vec::new() }));
    }
}

/// Change When Expression: asked, read before it is written.
fn change_when(ui: Ui, send: &Send) {
    let s = ui.state::<Shortcuts>();
    let Some(e) = s.selected.get_untracked().filter(|e| e.keys.is_some()) else { return };
    let send = send.clone();
    let back = s.table.get_untracked();
    let mut ask = quick::input("Change When Expression")
        .placeholder("when expression, for example: editorTextFocus")
        .value(&e.when)
        .validate(|v| super::when::parse(v).err());
    if let Some(back) = back {
        ask = ask.back(back);
    }
    ask.open(ui, move |v| {
        if let Some(v) = v.filter(|v| v.trim() != e.when) {
            apply(ui, &send, &e, Change::When(v));
        }
    });
}

fn copy(ui: Ui, text: &str) {
    if floem::Clipboard::set_contents(text.to_string()).is_err() {
        toast::warning(ui, "Could not copy to the clipboard.");
    }
}

/// The row's menu, as VS Code's.
fn menu(ui: Ui, send: &Send, e: &Entry, at: popup::At) {
    let s = ui.state::<Shortcuts>();
    let bound = e.keys.is_some();
    let (a, b, c, d, f) = (e.clone(), e.clone(), e.clone(), send.clone(), send.clone());
    let (g, h) = (send.clone(), e.clone());
    let key_text = e.key_text.clone();
    let user = e.source == Source::User;
    let items: Vec<popup::Item> = vec![
        popup::entry("Copy", move || copy(ui, &a.copied())).keys("Ctrl+C").into(),
        popup::entry("Copy Command ID", move || copy(ui, &b.command)).into(),
        popup::entry("Copy Command Title", move || copy(ui, &c.title)).enabled(e.known()).into(),
        popup::line(),
        popup::entry(if bound { "Change Keybinding..." } else { "Add Keybinding..." }, move || define(ui, false))
            .keys(&commands::keys_of(DEFINE))
            .into(),
        popup::entry("Add Keybinding...", move || define(ui, true)).enabled(bound).into(),
        popup::entry("Change When Expression...", move || change_when(ui, &d))
            .keys(&commands::keys_of(DEFINE_WHEN))
            .enabled(bound)
            .into(),
        popup::line(),
        popup::entry("Remove Keybinding", move || apply(ui, &f, &h, Change::Remove)).keys("Delete").enabled(bound).into(),
        {
            let e = e.clone();
            popup::entry("Reset Keybinding", move || apply(ui, &g, &e, Change::Reset)).enabled(user).into()
        },
        popup::line(),
        popup::entry("Show Same Keybindings", move || s.search.set(format!("\"{key_text}\""))).enabled(bound).into(),
    ];
    popup::open(ui, at, items);
}

/// Keys drawn as VS Code's keybinding label: each part in a box, `+` between (none on macOS), a space between a
/// chord's two.
fn key_boxes(t: Theme, keys: &[Chord]) -> AnyView {
    let os = os();
    let mut views: Vec<AnyView> = Vec::new();
    for (n, c) in keys.iter().enumerate() {
        if n > 0 {
            views.push(empty().style(|s| s.width(6.0)).into_any());
        }
        for (i, part) in key_parts(c, os).into_iter().enumerate() {
            if i > 0 && os != Os::Mac {
                views.push(label(|| "+").style(|s| s.padding_horiz(2.0)).into_any());
            }
            views.push(
                label(move || part.clone())
                    .style(move |s| {
                        let c = t.get();
                        s.font_size(11.0)
                            .padding_horiz(3.0)
                            .padding_vert(1.0)
                            .border(1.0)
                            .border_radius(3.0)
                            .border_color(c.key_border)
                            .border_bottom(1.0)
                            .background(c.key_bg)
                            .color(c.key_fg)
                            .selectable(false)
                    })
                    .into_any(),
            );
        }
    }
    h_stack_from_iter(views).style(|s| s.items_center()).into_any()
}

/// A column's cell of a row (or of the header).
fn cell(view: impl IntoView + 'static, column: usize) -> impl IntoView {
    container(view).style(move |s| {
        s.flex_grow(COLUMNS[column]).flex_basis(0.0).min_width(0.0).padding_right(10.0).items_center().height_full()
    })
}

fn row_view(ui: Ui, send: Send, e: Entry, odd: bool) -> AnyView {
    let t = ui.colors;
    let s = ui.state::<Shortcuts>();
    let (uid, command) = (e.uid, e.command.clone());
    let picked = move || s.selected.with(|p| p.as_ref().is_some_and(|p| p.uid == uid && p.command == command));
    let p2 = picked.clone();
    let action = container(icon(if e.keys.is_some() { "edit" } else { "add" }, 16.0))
        .on_click_stop(move |_| define(ui, false))
        .style(move |s| s.width(24.0).height_full().items_center().justify_center().apply_if(!p2(), |s| s.hide()));
    let known = e.known();
    let (name, id) = (e.name().to_string(), e.command.clone());
    let p3 = picked.clone();
    let title = h_stack((
        label(move || name.clone()).style(move |s| {
            s.text_ellipsis().min_width(0.0).apply_if(!known, |s| s.color(t.get().warning))
        }),
        label(move || id.clone()).style(move |s| {
            s.margin_left(8.0).text_ellipsis().min_width(0.0).color(t.get().desc).apply_if(!known || !p3(), |s| s.hide())
        }),
    ))
    .style(|s| s.items_center().min_width(0.0));
    let keys = match &e.keys {
        Some(k) => key_boxes(t, k),
        None => empty().into_any(),
    };
    let when = e.when.clone();
    let source = source_name(e.source);
    let on_menu = e.clone();
    let on_pick = e.clone();
    let send2 = send.clone();
    h_stack((
        action,
        cell(title, 0),
        cell(keys, 1),
        cell(label(move || when.clone()).style(|s| s.text_ellipsis().min_width(0.0)), 2),
        cell(label(move || source), 3),
    ))
    .on_click_stop(move |_| {
        if s.selected.with_untracked(|p| p.as_ref() != Some(&on_pick)) {
            s.selected.set(Some(on_pick.clone()));
        }
        focus(s.table);
    })
    .on_double_click_stop(move |_| define(ui, false))
    .on_secondary_click_stop(move |_| {
        if s.selected.with_untracked(|p| p.as_ref() != Some(&on_menu)) {
            s.selected.set(Some(on_menu.clone()));
        }
        let Some(id) = s.table.get_untracked() else { return };
        let (x, y) = ui.pointer.get_untracked();
        menu(ui, &send2, &on_menu, popup::At::point(id, x, y).back(id));
    })
    .style(move |s| {
        let c = t.get();
        let lit = picked();
        let mine = lit && ui.state::<Shortcuts>().focused.get();
        s.height(ROW)
            .width_full()
            .items_center()
            .font_size(13.0)
            .selectable(false)
            .apply_if(odd, |s| s.background(c.key_table_rows))
            .hover(|s| s.background(c.list_hover))
            .apply_if(lit, |s| s.background(c.list_inactive))
            .apply_if(mine, |s| s.background(c.list_selected).color(c.list_selected_fg))
    })
    .into_any()
}

/// "Keys for …": the keys pressed (a key, or a chord of two), Enter writing them and Escape giving up.
fn define_box(ui: Ui, send: Send, all: Memo<(u64, Rc<Vec<Entry>>)>) -> AnyView {
    let t = ui.colors;
    let s = ui.state::<Shortcuts>();
    let pressed = create_memo(move |_| s.define.with(|d| d.as_ref().map(|d| d.keys.clone()).unwrap_or_default()));
    let title = label(move || s.define.with(|d| d.as_ref().map(|d| format!("Keys for {}", d.entry.name())).unwrap_or_default()))
        .style(|s| s.font_bold().margin_bottom(6.0).text_ellipsis());
    let keys = floem::views::dyn_container(move || pressed.get(), move |k| key_boxes(t, &k)).style(move |s| {
        let c = t.get();
        s.width_full()
            .height(28.0)
            .padding_horiz(6.0)
            .items_center()
            .border(1.0)
            .border_color(c.focus)
            .background(c.input_bg)
            .color(c.input_fg)
    });
    let others = label(move || {
        let keys = pressed.get();
        let Some(not) = s.define.with(|d| d.as_ref().map(|d| d.entry.command.clone())) else { return String::new() };
        if keys.is_empty() {
            return String::new();
        }
        let names = all.with(|(_, all)| others(all, &keys, &not));
        match names.len() {
            0 => String::new(),
            1 => format!("1 existing command has this keybinding: {}", names[0]),
            n => format!("{n} existing commands have this keybinding: {}", names.join(", ")),
        }
    })
    .style(move |s| s.margin_top(6.0).font_size(12.0).color(t.get().warning));
    let close = move || {
        if s.define.with_untracked(Option::is_some) {
            s.define.set(None);
        }
        focus(s.table);
    };
    let view = floem::views::v_stack((
        title,
        label(|| "Press desired key combination and then press ENTER.").style(|s| s.margin_bottom(8.0)),
        keys,
        others,
    ))
    .keyboard_navigable();
    let id = view.id();
    let box_view = view
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Stop };
            let key = crate::accessibility::numpad(&k.key.logical_key, k.key.location).unwrap_or(k.key.logical_key.clone());
            let plain = k.modifiers.is_empty();
            match key {
                Key::Named(NamedKey::Escape) if plain => close(),
                Key::Named(NamedKey::Enter) if plain => {
                    if let Some(d) = s.define.get_untracked().filter(|d| !d.keys.is_empty()) {
                        s.define.set(None);
                        let change = if d.add { Change::Add(d.keys) } else { Change::Keys(d.keys) };
                        apply(ui, &send, &d.entry, change);
                        focus(s.table);
                    }
                }
                key => {
                    if let Some(c) = Chord::of(&key, k.modifiers) {
                        s.define.update(|d| {
                            if let Some(d) = d {
                                if d.keys.len() >= 2 {
                                    d.keys.clear();
                                }
                                d.keys.push(c);
                            }
                        });
                    }
                }
            }
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            if s.define.with_untracked(Option::is_some) {
                s.define.set(None);
            }
        })
        .style(move |s| {
            let c = t.get();
            s.width(420.0)
                .padding(12.0)
                .font_size(13.0)
                .background(c.widget)
                .color(c.fg)
                .border(1.0)
                .border_color(c.widget_border)
                .box_shadow_blur(8.0)
                .box_shadow_color(c.shadow)
        });
    exec_after(Duration::ZERO, move |_| id.request_focus());
    box_view.into_any()
}

/// The search box: the words typed, or the keys pressed while Record Keys is on.
fn search_box(ui: Ui, count: Memo<usize>) -> impl IntoView {
    let t = ui.colors;
    let s = ui.state::<Shortcuts>();
    let input = text_input(s.search)
        .placeholder("Type to search in keybindings")
        .on_event_cont(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return };
            match &k.key.logical_key {
                Key::Named(NamedKey::Escape) if s.search.with_untracked(|t| !t.is_empty()) => s.search.set(String::new()),
                Key::Named(NamedKey::ArrowDown) => focus(s.table),
                _ => {}
            }
        })
        .style(move |st| {
            let c = t.get();
            let st = crate::widgets::steady_input(st, Color::TRANSPARENT, None);
            st.flex_grow(1.0_f32)
                .min_width(0.0)
                .padding_horiz(4.0)
                .border(0.0)
                .background(Color::TRANSPARENT)
                .color(c.input_fg)
                .cursor_color(floem::peniko::Brush::Solid(c.cursor))
                .apply_if(s.recording.get(), |s| s.hide())
        });
    s.input.set(Some(input.id()));
    let recorder = label(move || {
        let text = s.search.get();
        if text.is_empty() { "Recording Keys. Press Escape to exit".to_string() } else { text }
    })
    .keyboard_navigable();
    s.recorder.set(Some(recorder.id()));
    let recorder = recorder
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Stop };
            let key = crate::accessibility::numpad(&k.key.logical_key, k.key.location).unwrap_or(k.key.logical_key.clone());
            let alt_k = k.modifiers == Modifiers::ALT && matches!(&key, Key::Character(c) if c.eq_ignore_ascii_case("k"));
            if (matches!(key, Key::Named(NamedKey::Escape)) && k.modifiers.is_empty()) || alt_k {
                s.recording.set(false);
            } else if let Some(c) = Chord::of(&key, k.modifiers) {
                let mut keys = s.recorded.get_untracked();
                if keys.len() >= 2 {
                    keys.clear();
                }
                keys.push(c);
                s.search.set(format!("\"{}\"", key_text(&keys, os())));
                s.recorded.set(keys);
            }
            EventPropagation::Stop
        })
        .style(move |st| {
            let c = t.get();
            let empty = s.search.with(String::is_empty);
            st.flex_grow(1.0_f32)
                .min_width(0.0)
                .padding_horiz(4.0)
                .color(if empty { c.placeholder } else { c.input_fg })
                .apply_if(!s.recording.get(), |s| s.hide())
        });
    let results = label(move || match count.get() {
        1 => "1 result".to_string(),
        n => format!("{n} results"),
    })
    .style(move |s| s.padding_horiz(6.0).font_size(12.0).color(t.get().desc).selectable(false));
    let toggle = crate::widgets::toggle(t, "record-keys", s.recording, "Record Keys (Alt+K)");
    let focused = s.recording;
    h_stack((
        h_stack((input, recorder, results, toggle))
            .style(move |st| {
                let c = t.get();
                st.flex_grow(1.0_f32)
                    .min_width(0.0)
                    .height(26.0)
                    .padding_right(2.0)
                    .items_center()
                    .border(1.0)
                    .border_color(if focused.get() { c.focus } else { c.input_border })
                    .background(c.input_bg)
            }),
        crate::widgets::action(t, "clear-all", move || {
            if s.search.with_untracked(|t| !t.is_empty()) {
                s.search.set(String::new());
            }
        })
        .style(|s| s.margin_left(4.0)),
    ))
    .style(|s| s.items_center().width_full())
}

/// The editor drawn in its tab.
fn view(ui: Ui, send: Send) -> AnyView {
    let t = ui.colors;
    let s = ui.state::<Shortcuts>();
    let all = create_memo(move |prev: Option<&(u64, Rc<Vec<Entry>>)>| {
        s.read.track();
        let now = entries(&super::rules(), &known(), os());
        match prev {
            Some((n, was)) if **was == now => (*n, was.clone()),
            Some((n, _)) => (n + 1, Rc::new(now)),
            None => (0, Rc::new(now)),
        }
    });
    let shown = create_memo(move |_| {
        let q = s.search.with(|t| query(t));
        all.with(|(_, all)| all.iter().enumerate().filter(|(_, e)| matches(e, &q)).map(|(n, _)| n).collect::<Vec<_>>())
    });
    let count = create_memo(move |_| shown.with(Vec::len));
    // The place of the row picked among those shown.
    let at = create_memo(move |_| {
        let picked = s.selected.get()?;
        all.with(|(_, all)| shown.with(|sh| sh.iter().position(|n| all[*n].uid == picked.uid && all[*n].command == picked.command)))
    });
    let problems = ui.state::<super::Keymap>().problems;
    let send_w = send.clone();
    let send_j = send.clone();
    let warnings = create_memo(move |_| {
        let mut w: Vec<String> = problems.get().to_vec();
        w.extend(all.with(|(_, all)| idle(all)));
        w
    });
    let warning = floem::views::dyn_container(
        move || warnings.get(),
        move |w| {
            if w.is_empty() {
                return empty().into_any();
            }
            let n = w.len();
            let head = if n == 1 {
                "1 rule of keybindings.json does nothing:".to_string()
            } else {
                format!("{n} rules of keybindings.json do nothing:")
            };
            let mut lines: Vec<AnyView> = vec![
                h_stack((
                    icon("warning", 16.0).style(move |s| s.color(t.get().warning).margin_right(6.0)),
                    label(move || head.clone()),
                ))
                .style(|s| s.items_center())
                .into_any(),
            ];
            for line in w.iter().take(5).cloned() {
                lines.push(label(move || line.clone()).style(|s| s.margin_left(22.0).text_ellipsis()).into_any());
            }
            if n > 5 {
                lines.push(label(move || format!("and {} more", n - 5)).style(|s| s.margin_left(22.0)).into_any());
            }
            lines.push(
                label(|| "Open Keyboard Shortcuts (JSON)")
                    .on_click_stop({
                        let send = send_w.clone();
                        move |_| crate::settings::open_user(ui, &send, super::FILE)
                    })
                    .style(move |s| s.margin_left(22.0).color(t.get().link).cursor(CursorStyle::Pointer))
                    .into_any(),
            );
            floem::views::v_stack_from_iter(lines)
                .style(move |s| {
                    let c = t.get();
                    s.width_full().margin_top(8.0).padding(8.0).font_size(12.0).border(1.0).border_color(c.warning)
                })
                .into_any()
        },
    );
    let header = h_stack((
        empty().style(|s| s.width(24.0)),
        cell(label(|| "Command"), 0),
        cell(label(|| "Keybinding"), 1),
        cell(label(|| "When"), 2),
        cell(label(|| "Source"), 3),
    ))
    .style(move |s| {
        s.height(ROW).width_full().items_center().font_bold().selectable(false).background(t.get().key_table_header)
    });
    let send2 = send.clone();
    let rows = virtual_stack(
        VirtualDirection::Vertical,
        VirtualItemSize::Fixed(Box::new(|| ROW)),
        move || {
            let n = all.with(|(n, _)| *n);
            shown.with(|sh| sh.iter().enumerate().map(|(i, at)| (n, *at, i % 2 == 1)).collect::<im::Vector<_>>())
        },
        |item: &(u64, usize, bool)| *item,
        move |(_, at, odd)| {
            let e = all.with_untracked(|(_, all)| all[at].clone());
            row_view(ui, send2.clone(), e, odd)
        },
    )
    .style(|s| s.flex_col().width_full());
    let pick = move |to: usize| {
        let e = all.with_untracked(|(_, all)| shown.with_untracked(|sh| sh.get(to).map(|n| all[*n].clone())));
        if let Some(e) = e
            && s.selected.with_untracked(|p| p.as_ref() != Some(&e))
        {
            s.selected.set(Some(e));
        }
    };
    let send3 = send.clone();
    let table = scroll(rows)
        .ensure_visible(move || {
            let y = at.get().unwrap_or(0) as f64 * ROW;
            Rect::new(0.0, y, 1.0, y + ROW)
        })
        .keyboard_navigable()
        .on_event_cont(EventListener::FocusGained, move |_| {
            s.focused.set(true);
            if s.selected.with_untracked(Option::is_none) {
                pick(0);
            }
        })
        .on_event_cont(EventListener::FocusLost, move |_| s.focused.set(false))
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            let m = k.modifiers;
            let n = count.get_untracked();
            let now = at.get_untracked();
            let step = |by: isize| {
                let from = now.map_or(-1, |a| a as isize);
                pick((from + by).clamp(0, n.saturating_sub(1) as isize) as usize)
            };
            match &k.key.logical_key {
                Key::Named(NamedKey::ArrowDown) if m.is_empty() => step(1),
                Key::Named(NamedKey::ArrowUp) if m.is_empty() && now == Some(0) => focus(s.input),
                Key::Named(NamedKey::ArrowUp) if m.is_empty() => step(-1),
                Key::Named(NamedKey::PageDown) if m.is_empty() => step(20),
                Key::Named(NamedKey::PageUp) if m.is_empty() => step(-20),
                Key::Named(NamedKey::Home) if m.is_empty() => pick(0),
                Key::Named(NamedKey::End) if m.is_empty() => pick(n.saturating_sub(1)),
                Key::Named(NamedKey::Enter) if m.is_empty() => define(ui, false),
                Key::Named(NamedKey::Delete) if m.is_empty() => {
                    if let Some(e) = s.selected.get_untracked() {
                        apply(ui, &send3, &e, Change::Remove);
                    }
                }
                Key::Character(c) if m == Modifiers::CONTROL && c.eq_ignore_ascii_case("c") => {
                    if let Some(e) = s.selected.get_untracked() {
                        copy(ui, &e.copied());
                    }
                }
                _ => return EventPropagation::Continue,
            }
            EventPropagation::Stop
        })
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full().min_height(0.0));
    s.table.set(Some(table.id()));
    let open_define = create_memo(move |_| s.define.with(|d| d.as_ref().map(|d| (d.entry.uid, d.add))));
    let define_layer = floem::views::dyn_container(
        move || open_define.get(),
        move |d| match d {
            Some(_) => define_box(ui, send.clone(), all),
            None => empty().into_any(),
        },
    )
    .style(|s| s.position(Position::Absolute).inset_top(60.0).width_full().justify_center());
    let json =crate::widgets::action(t, "go-to-file", move || crate::settings::open_user(ui, &send_j, super::FILE));
    let json = floem::views::tooltip(json, || label(|| "Open Keyboard Shortcuts (JSON)"));
    let body = floem::views::v_stack((
        h_stack((search_box(ui, count), json.style(|s| s.margin_left(2.0)))).style(|s| s.width_full().items_center()),
        warning,
        header.style(|s| s.margin_top(10.0)),
        table,
    ))
    .style(move |s| {
        s.size_full().padding_horiz(24.0).padding_top(12.0).min_height(0.0).color(t.get().fg).background(t.get().editor_bg)
    });
    focus(if s.recording.get_untracked() { s.recorder } else { s.input });
    floem::views::stack((body, define_layer)).style(|s| s.size_full()).into_any()
}

/// What it adds to the window's registries, once at start: the editor's kind of tab and its commands.
pub fn register(ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    kinds::add(
        kinds::kind(KIND, |_ui, _rest| "Keyboard Shortcuts".to_string(), |ui, send, _tab| view(ui, send))
            .kept(|ui, send, _path| open(ui, send)),
    );
    let s = ui.state::<Shortcuts>();
    let shown = move |ui: Ui| ui.active.with_untracked(|a| a.as_deref() == Some(TAB));
    add(command(crate::manage::OPEN_KEYBINDINGS, "Preferences", "Open Keyboard Shortcuts", open));
    add(command(OPEN_JSON, "Preferences", "Open Keyboard Shortcuts (JSON)", |ui, send| {
        crate::settings::open_user(ui, send, super::FILE)
    }));
    add(command(DEFINE, "", "Define Keybinding", |ui, _| define(ui, false))
        .keys("Ctrl+K Ctrl+K")
        .when(move |_| s.focused.get_untracked())
        .hidden());
    add(command(DEFINE_WHEN, "", "Change When Expression", change_when)
        .keys("Ctrl+K Ctrl+E")
        .when(move |_| s.focused.get_untracked())
        .hidden());
    add(command(RECORD, "", "Record Keys", move |_, _| s.recording.update(|r| *r = !*r)).keys("Alt+K").when(shown).hidden());
    // Record Keys turned on or off: what was recorded is forgotten, and the box it is in has the keys.
    ui.scope.create_effect(move |was: Option<bool>| {
        let on = s.recording.get();
        if was.is_some_and(|w| w != on) {
            s.recorded.set(Vec::new());
            if on && s.search.with_untracked(|t| !t.is_empty()) {
                s.search.set(String::new());
            }
            focus(if on { s.recorder } else { s.input });
        }
        on
    });
}

/// The session's messages that are this part's, taken in: keybindings.json's text kept as it is read (the keymap
/// reads its rules first), and a save of it that failed said. None is taken away.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    if m["name"].as_str() != Some(super::FILE) {
        return false;
    }
    let s = ui.state::<Shortcuts>();
    match m["type"].as_str().unwrap_or("") {
        "user_file" => {
            let text = m["error"].is_null().then(|| Rc::new(m["text"].as_str().unwrap_or("").to_string()));
            if s.text.with_untracked(|t| *t != text) {
                s.text.set(text);
            }
            s.read.update(|n| *n += 1);
        }
        "saved_user" => {
            if let Some(e) = m["error"].as_str() {
                toast::error(ui, &format!("Could not write keybindings.json: {e}"));
                send(json!({"type": "user_file", "name": super::FILE}));
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::super::{read_user, with_user};
    use super::*;

    fn keys(text: &str) -> Vec<Chord> {
        parse_keys(text).expect("keys")
    }

    fn rule(k: &str, command: &str, when: &str, source: Source) -> Rule {
        Rule {
            keys: keys(k),
            command: command.into(),
            when: (!when.is_empty()).then(|| Rc::new(super::super::when::parse(when).expect("when"))),
            when_text: when.into(),
            source,
            written: None,
        }
    }

    fn known_ones() -> Vec<Known> {
        vec![
            ("a.open".into(), "View: Open A".into(), false),
            ("b.close".into(), "View: Close B".into(), false),
            ("c.none".into(), "Help: Nothing".into(), false),
            ("d.hidden".into(), "Hidden".into(), true),
        ]
    }

    fn defaults() -> Vec<Rule> {
        vec![
            rule("ctrl+k ctrl+o", "a.open", "", Source::Default),
            rule("ctrl+w", "b.close", "editorFocus", Source::Default),
            rule("ctrl+q", "gone.command", "", Source::Default),
        ]
    }

    /// The rows once keybindings.json is this text.
    fn rows_with(text: &str) -> Vec<Entry> {
        let read = read_user(text, Os::Linux);
        assert!(read.problems.is_empty(), "{:?}", read.problems);
        entries(&with_user(defaults(), &read.rules), &known_ones(), Os::Linux)
    }

    fn row<'a>(all: &'a [Entry], command: &str) -> &'a Entry {
        all.iter().find(|e| e.command == command).expect(command)
    }

    fn keys_of(all: &[Entry], command: &str) -> Vec<String> {
        all.iter().filter(|e| e.command == command).map(|e| e.key_text.clone()).collect()
    }

    #[test]
    fn every_default_key_is_written_as_it_is_read_back() {
        for os in [Os::Linux, Os::Mac, Os::Windows] {
            for r in super::super::defaults::rules(os) {
                let text = key_text(&r.keys, os);
                assert_eq!(parse_keys(&text).as_ref(), Some(&r.keys), "{text} on {os:?}");
            }
        }
        assert_eq!(key_text(&keys("Ctrl+Shift+`"), Os::Linux), "ctrl+shift+`");
        assert_eq!(key_text(&keys("ctrl+k ctrl+s"), Os::Mac), "ctrl+k ctrl+s");
        assert_eq!(key_text(&keys("cmd+="), Os::Mac), "cmd+=");
        assert_eq!(key_parts(&keys("ctrl+shift+p")[0], Os::Linux), ["Ctrl", "Shift", "P"]);
        assert_eq!(key_parts(&keys("cmd+shift+p")[0], Os::Mac), ["⇧", "⌘", "P"]);
    }

    #[test]
    fn the_rows_are_the_rules_then_the_commands_without_keys() {
        let all = rows_with("");
        let shown: Vec<(&str, &str, &str)> =
            all.iter().map(|e| (e.name(), e.key_label.as_str(), source_name(e.source))).collect();
        // A default of a command not there is left out; a hidden command without keys too.
        assert_eq!(
            shown,
            [
                ("View: Close B", "Ctrl+W", "Default"),
                ("View: Open A", "Ctrl+K Ctrl+O", "Default"),
                ("Help: Nothing", "", "Default"),
            ]
        );
        assert_eq!(all.iter().map(|e| e.uid).collect::<Vec<_>>(), [0, 1, 2]);
        // A rule of the person's whose command is not there is shown, and said to do nothing.
        let all = rows_with(r#"[{"key": "ctrl+e", "command": "no.such"}]"#);
        assert_eq!(row(&all, "no.such").name(), "no.such");
        assert_eq!(idle(&all), ["Ctrl+E: \"no.such\" is not a command."]);
        assert_eq!(
            row(&all, "b.close").copied(),
            "{\n  \"key\": \"ctrl+w\",\n  \"command\": \"b.close\",\n  \"when\": \"editorFocus\"\n}"
        );
    }

    #[test]
    fn a_search_reads_words_keys_and_sources() {
        let all = rows_with(r#"[{"key": "ctrl+e", "command": "c.none"}]"#);
        let found = |text: &str| -> Vec<&str> {
            let q = query(text);
            all.iter().filter(|e| matches(e, &q)).map(|e| e.command.as_str()).collect()
        };
        assert_eq!(found("open"), ["a.open"]);
        assert_eq!(found("view b.cl"), ["b.close"]);
        assert_eq!(found("editorfocus"), ["b.close"]);
        assert_eq!(found("ctrl+w"), ["b.close"]);
        assert_eq!(found("@source:user"), ["c.none"]);
        assert_eq!(found("@command:a.open"), ["a.open"]);
        // Keys in quotes: the row's keys are these, or begin with them.
        assert_eq!(found("\"ctrl+k\""), ["a.open"]);
        assert_eq!(found("\"Ctrl+K Ctrl+O\""), ["a.open"]);
        assert_eq!(found("\"ctrl+k ctrl+x\""), Vec::<&str>::new());
        assert_eq!(found("\"ctrl\""), Vec::<&str>::new());
        assert_eq!(found("").len(), all.len());
        assert_eq!(others(&all, &keys("ctrl+w"), "a.open"), ["View: Close B"]);
        assert!(others(&all, &keys("ctrl+w"), "b.close").is_empty());
    }

    #[test]
    fn the_lists_places_are_found_past_comments_and_strings() {
        let text = "// mine\n[\n  /* one, ] */ {\"key\": \"ctrl+]\", \"command\": \"a\"},\n  {\"command\": \"b // c\"},\n]\n";
        let sp = spans(text).expect("spans");
        let items: Vec<&str> = sp.items.iter().map(|(s, e)| &text[*s..*e]).collect();
        assert_eq!(items, ["{\"key\": \"ctrl+]\", \"command\": \"a\"}", "{\"command\": \"b // c\"}"]);
        assert_eq!(&text[sp.close..], "]\n");
        assert!(spans("{}").is_none() && spans("[ {").is_none());
    }

    #[test]
    fn a_rule_is_added_at_the_end_and_the_rest_kept() {
        let rule = written("ctrl+e", "a", "");
        assert_eq!(append("", &rule).unwrap(), format!("{HEADER}[\n    {rule}\n]\n"));
        assert_eq!(append("[]", &rule).unwrap(), format!("[\n    {rule}\n]"));
        assert_eq!(append("// x\n[\n]\n", &rule).unwrap(), format!("// x\n[\n    {rule}\n]\n"));
        assert_eq!(append("[\n  // only this\n]", &rule).unwrap(), format!("[\n  // only this\n    {rule}\n]"));
        let one = "[\n    // first\n    { \"key\": \"f1\", \"command\": \"x\" } // after\n]";
        assert_eq!(
            append(one, &rule).unwrap(),
            format!("[\n    // first\n    {{ \"key\": \"f1\", \"command\": \"x\" }},\n    {rule} // after\n]")
        );
        // A trailing comma stays where it was.
        let trailing = "[\n    {\"key\": \"f1\", \"command\": \"x\"},\n]";
        let out = append(trailing, &rule).unwrap();
        assert_eq!(out, format!("[\n    {{\"key\": \"f1\", \"command\": \"x\"}},\n    {rule},\n]"));
        assert!(ca_settings::jsonc::parse(&out).is_ok());
        assert_eq!(append("{}", &rule), None);
    }

    #[test]
    fn a_rule_is_taken_out_with_its_comma() {
        let text = "[\n    {\"command\": \"a\"},\n    // b's\n    {\"command\": \"b\"},\n    {\"command\": \"c\"}\n]";
        assert_eq!(remove(text, 1).unwrap(), "[\n    {\"command\": \"a\"},\n    {\"command\": \"c\"}\n]");
        assert_eq!(remove(text, 2).unwrap(), "[\n    {\"command\": \"a\"},\n    // b's\n    {\"command\": \"b\"}\n]");
        assert_eq!(remove(text, 0).unwrap(), "[\n    // b's\n    {\"command\": \"b\"},\n    {\"command\": \"c\"}\n]");
        assert_eq!(remove("// keep\n[\n    {\"command\": \"a\"},\n]\n", 0).unwrap(), "// keep\n[\n]\n");
        assert_eq!(remove(text, 3), None);
    }

    #[test]
    fn a_default_changed_is_taken_away_and_the_new_keys_added() {
        let all = rows_with("");
        let e = row(&all, "b.close");
        let text = edit("", Os::Linux, e, &Change::Keys(keys("ctrl+alt+w"))).expect("written");
        assert_eq!(
            text,
            format!(
                "{HEADER}[\n    {},\n    {}\n]\n",
                written("ctrl+w", "-b.close", "editorFocus"),
                written("ctrl+alt+w", "b.close", "editorFocus")
            )
        );
        let all = rows_with(&text);
        assert_eq!(keys_of(&all, "b.close"), ["ctrl+alt+w"]);
        assert_eq!(row(&all, "b.close").source, Source::User);
        // Changed again: the person's rule is written again in its place.
        let again = edit(&text, Os::Linux, row(&all, "b.close"), &Change::Keys(keys("f9"))).expect("again");
        assert_eq!(keys_of(&rows_with(&again), "b.close"), ["f9"]);
        assert_eq!(again.matches("b.close").count(), 2);
    }

    #[test]
    fn keys_are_added_removed_and_reset() {
        let all = rows_with("");
        // A command with no keys is given some; another key added to one that has.
        let text = edit("", Os::Linux, row(&all, "c.none"), &Change::Keys(keys("f2"))).unwrap();
        let text = edit(&text, Os::Linux, row(&rows_with(&text), "a.open"), &Change::Add(keys("f3"))).unwrap();
        let all = rows_with(&text);
        assert_eq!(keys_of(&all, "c.none"), ["f2"]);
        assert_eq!(keys_of(&all, "a.open"), ["ctrl+k ctrl+o", "f3"]);
        // The person's removed: its rule is taken out of the file; a default's: taken away by `-command`.
        let user = all.iter().find(|e| e.command == "a.open" && e.source == Source::User).unwrap();
        let text = edit(&text, Os::Linux, user, &Change::Remove).unwrap();
        let all = rows_with(&text);
        assert_eq!(keys_of(&all, "a.open"), ["ctrl+k ctrl+o"]);
        let text = edit(&text, Os::Linux, row(&all, "a.open"), &Change::Remove).unwrap();
        let all = rows_with(&text);
        assert_eq!(keys_of(&all, "a.open"), [""]);
        assert!(text.contains("\"-a.open\""));
        // Reset of a default changed: its rules of the person's go, and it has its default again.
        let text = edit(&text, Os::Linux, row(&all, "b.close"), &Change::Keys(keys("f4"))).unwrap();
        let text = edit(&text, Os::Linux, row(&rows_with(&text), "b.close"), &Change::Reset).unwrap();
        let all = rows_with(&text);
        assert_eq!(keys_of(&all, "b.close"), ["ctrl+w"]);
        assert_eq!(row(&all, "b.close").source, Source::Default);
        assert!(!text.contains("b.close"));
        // A default has nothing of the person's to reset.
        assert_eq!(edit(&text, Os::Linux, row(&all, "b.close"), &Change::Reset), None);
    }

    #[test]
    fn a_when_changed_is_written_and_a_file_that_cannot_be_read_is_not() {
        let all = rows_with("");
        let text = edit("", Os::Linux, row(&all, "a.open"), &Change::When(" terminalFocus ".into())).unwrap();
        let all = rows_with(&text);
        let e = row(&all, "a.open");
        assert_eq!((e.key_text.as_str(), e.when.as_str(), e.source), ("ctrl+k ctrl+o", "terminalFocus", Source::User));
        assert_eq!(keys_of(&all, "a.open").len(), 1);
        let text = edit(&text, Os::Linux, e, &Change::When(String::new())).unwrap();
        assert_eq!(row(&rows_with(&text), "a.open").when, "");
        // No keys, no `when` to change; a file that is not a list is never written into.
        assert_eq!(edit("", Os::Linux, row(&all, "c.none"), &Change::When("x".into())), None);
        assert_eq!(edit("{ \"a\": 1 }", Os::Linux, row(&all, "a.open"), &Change::Remove), None);
        assert_eq!(edit("[ {", Os::Linux, row(&all, "a.open"), &Change::Add(keys("f1"))), None);
    }
}
