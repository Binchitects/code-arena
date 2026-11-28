//! The Settings editor, as VS Code's (Preferences: Open Settings, Ctrl+,): every setting Code Arena reads, drawn
//! from the session's `settings_schema`, with a search above, the User and Workspace tabs, the sections at the left
//! and a row for each setting: its title ("Editor: Font Size"), what it does, and the control its kind has (a box
//! to tick, a choice, a number or a text taken on Enter or when it loses the keys, or "Edit in settings.json" for a
//! list or an object).
//!
//! The search reads words (of the title, the key, the description) and VS Code's filters: `@modified` (what the
//! tab's file sets), `@id:editor.tabSize` (those settings, by their keys, a comma between), `@lang:python` (the
//! settings a language may have of its own, shown and set for it). A row set in the tab's file has the modified
//! bar at its left, says where else it is set ("Also modified in: Workspace"), and its gear's menu resets it or
//! copies its key or itself as JSON. A value the environment gives wins, and the row says so; one this edition
//! cannot change says why. What a settings file holds that was not taken is said under its setting, and what is
//! no setting at all above the rows.
//!
//! A value changed is written at once (`set_setting`, with `where` the tab's file and `lang` the language of
//! `@lang:`), the file's comments kept; the person's own file loses a setting set back to its default. The session
//! then says the settings again, and the editor asks for its schema again while its tab is open.
//!
//! What every part has (`register`, `take`) is said in `ext.rs`.

use std::rc::Rc;
use std::time::Duration;

use floem::action::exec_after;
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::peniko::{Brush, Color};
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo, create_rw_signal};
use floem::style::{CursorStyle, Position};
use floem::views::{
    Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, text_input, v_stack,
    v_stack_from_iter,
};
use floem::{AnyView, IntoView, View, ViewId};
use serde_json::{Map, Value, json};

use crate::commands::{self, command};
use crate::editor::kinds;
use crate::icons::icon;
use crate::keymap::{Focus, TakesKeys};
use crate::state::{Send, Ui};
use crate::widgets::{SEMIBOLD, popup, toast};

/// The editor's tab.
pub const TAB: &str = "settings:editor";
const KIND: &str = "settings";
/// VS Code's commands that open the editor on the person's settings and on the project's.
pub const OPEN_USER: &str = "workbench.action.openGlobalSettings";
pub const OPEN_WORKSPACE: &str = "workbench.action.openWorkspaceSettings";
/// The first section, as VS Code's: the settings most change.
const COMMON: &str = "Commonly Used";
/// The heading of the agent's own sections.
const AGENT: &str = "Agent";
/// VS Code's commonly used settings that Code Arena has, and the agent's own most changed.
const COMMONLY_USED: &[&str] = &[
    "files.autoSave",
    "editor.fontSize",
    "editor.fontFamily",
    "editor.tabSize",
    "editor.renderWhitespace",
    "editor.cursorStyle",
    "editor.multiCursorModifier",
    "editor.insertSpaces",
    "editor.wordWrap",
    "files.exclude",
    "files.associations",
    "workbench.editor.enablePreview",
    "workbench.colorTheme",
    "agent.model",
    "agent.mode",
];

/// One setting as the session says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub key: String,
    /// `bool`, `string`, `integer`, `number`, `enum`, `bool_or`, `string_list`, `number_list`, `object`.
    pub kind: String,
    /// The words of a choice.
    pub words: Vec<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub default: Value,
    pub description: String,
    pub section: String,
    /// `user` (the person's alone), `project`, `language`.
    pub scope: String,
    pub env: Option<String>,
    /// In force (null for what is not said: the agent's objects).
    pub value: Value,
    /// `default`, `user`, `project`, `workspace`, `env:NAME`, `flag`.
    pub source: String,
    /// What each file sets it to itself, by layer.
    pub written: Map<String, Value>,
    /// The layers whose files set it.
    pub layers: Vec<String>,
    /// Why this edition cannot change it.
    pub locked: Option<String>,
}

impl Entry {
    fn from_json(m: &Value) -> Entry {
        let text = |k: &str| m[k].as_str().unwrap_or("").to_string();
        let words = |k: &str| {
            m[k].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()
        };
        Entry {
            key: text("key"),
            kind: text("kind"),
            words: words("enum"),
            min: m["min"].as_f64(),
            max: m["max"].as_f64(),
            default: m["default"].clone(),
            description: text("description"),
            section: text("section"),
            scope: text("scope"),
            env: m["env"].as_str().map(str::to_string),
            value: m["value"].clone(),
            source: text("source"),
            written: m["written"].as_object().cloned().unwrap_or_default(),
            layers: words("in"),
            locked: m["locked"].as_str().map(str::to_string),
        }
    }

    /// Whether a tab may set it: the Workspace tab not what is the person's alone.
    fn for_target(&self, target: Target) -> bool {
        target == Target::User || self.scope != "user"
    }
}

/// What the editor is drawn from: the session's `settings_schema`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schema {
    pub entries: Vec<Entry>,
    pub problems: Vec<super::Problem>,
    /// By layer, what its file holds that is no setting.
    pub unknown: Map<String, Value>,
    pub files: Map<String, Value>,
}

impl Schema {
    pub fn from_json(m: &Value) -> Schema {
        let text = |v: &Value| v.as_str().unwrap_or("").to_string();
        Schema {
            entries: m["entries"].as_array().into_iter().flatten().map(Entry::from_json).collect(),
            problems: m["problems"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| super::Problem { key: text(&p["key"]), file: text(&p["file"]), error: text(&p["error"]) })
                .collect(),
            unknown: m["unknown"].as_object().cloned().unwrap_or_default(),
            files: m["files"].as_object().cloned().unwrap_or_default(),
        }
    }

    fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.key == key)
    }

    /// What a tab's file holds that is no setting.
    fn unknown_in(&self, target: Target) -> Vec<String> {
        let of = |layer: &str| {
            let keys = self.unknown.get(layer).and_then(Value::as_array);
            keys.into_iter().flatten().filter_map(Value::as_str).map(str::to_string)
        };
        match target {
            Target::User => of("user").collect(),
            Target::Workspace => of("workspace").chain(of("project")).collect(),
        }
    }
}

/// The tab shown: whose settings are read and written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The person's settings.json, for every project.
    User,
    /// The project's .vscode/settings.json.
    Workspace,
}

impl Target {
    /// Its layer, as `set_setting`'s `where` names it.
    fn layer(self) -> &'static str {
        match self {
            Target::User => "user",
            Target::Workspace => "workspace",
        }
    }
}

/// A layer as the editor names it to the person.
fn layer_name(layer: &str) -> &'static str {
    match layer {
        "user" => "User",
        "workspace" => "Workspace",
        _ => "Project (.code-arena)",
    }
}

/// A search, read: its words and VS Code's filters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub words: Vec<String>,
    pub modified: bool,
    pub ids: Vec<String>,
    pub lang: Option<String>,
}

impl Query {
    fn is_empty(&self) -> bool {
        self.words.is_empty() && !self.modified && self.ids.is_empty() && self.lang.is_none()
    }
}

/// A search's text read: `@modified`, `@id:a,b`, `@lang:python` and words (the case not minded).
pub fn query(text: &str) -> Query {
    let mut q = Query::default();
    for part in text.split_whitespace() {
        if part.eq_ignore_ascii_case("@modified") {
            q.modified = true;
        } else if let Some(ids) = part.strip_prefix("@id:") {
            q.ids.extend(ids.split(',').filter(|i| !i.is_empty()).map(str::to_string));
        } else if let Some(lang) = part.strip_prefix("@lang:") {
            q.lang = Some(lang.to_lowercase()).filter(|l| !l.is_empty());
        } else {
            q.words.push(part.to_lowercase());
        }
    }
    q
}

/// A key's part as words, as VS Code's: `fontSize` "Font Size", `autoSave` "Auto Save".
fn wordify(part: &str) -> String {
    let mut out = String::new();
    let mut was_lower = false;
    for (i, c) in part.chars().enumerate() {
        if c.is_uppercase() && was_lower {
            out.push(' ');
        }
        if i == 0 { out.extend(c.to_uppercase()) } else { out.push(c) }
        was_lower = c.is_lowercase() || c.is_ascii_digit();
    }
    out
}

/// A setting's title as VS Code's: its category (the key's parts before the last, " › " between) and its name.
/// `workbench.editor.enablePreview` is "Workbench › Editor" and "Enable Preview".
pub fn title(key: &str) -> (String, String) {
    let parts: Vec<&str> = key.split('.').collect();
    let (last, rest) = parts.split_last().expect("a key has a part");
    (rest.iter().map(|p| wordify(p)).collect::<Vec<_>>().join(" › "), wordify(last))
}

/// The value a tab shows: what its file sets, else what is beneath it (the Workspace tab shows the person's and the
/// project's own), else the default. For a language, its own value in force.
pub fn shown_value(e: &Entry, target: Target, lang_own: Option<&Value>) -> Value {
    if let Some(v) = lang_own {
        return v.clone();
    }
    let layers: &[&str] = match target {
        Target::User => &["user"],
        Target::Workspace => &["workspace", "project", "user"],
    };
    layers.iter().find_map(|l| e.written.get(*l)).cloned().unwrap_or_else(|| e.default.clone())
}

/// Whether the tab's file sets it (for a language: whether the language has its own).
pub fn modified(e: &Entry, target: Target, lang_own: Option<&Value>, lang: bool) -> bool {
    if lang {
        return lang_own.is_some();
    }
    e.layers.iter().any(|l| l == target.layer())
}

/// The other files that set it, by the names the person knows them by.
pub fn also_in(e: &Entry, target: Target) -> Vec<&'static str> {
    e.layers.iter().filter(|l| *l != target.layer()).map(|l| layer_name(l)).collect()
}

/// Whether a setting is one the search finds, in a tab.
pub fn matches(e: &Entry, q: &Query, target: Target, is_modified: bool) -> bool {
    if !e.for_target(target) || (q.modified && !is_modified) || (q.lang.is_some() && e.scope != "language") {
        return false;
    }
    if !q.ids.is_empty() && !q.ids.iter().any(|i| i.eq_ignore_ascii_case(&e.key)) {
        return false;
    }
    if q.words.is_empty() {
        return true;
    }
    let (category, name) = title(&e.key);
    let hay = format!("{category} {name} {} {}", e.key, e.description).to_lowercase();
    q.words.iter().all(|w| hay.contains(w.as_str()))
}

/// The sections at the left, in order, each with its depth: Commonly Used, the IDE's sections, then the agent's
/// under one heading.
pub fn sections(entries: &[Entry]) -> Vec<(String, usize)> {
    let mut out = vec![(COMMON.to_string(), 0)];
    let mut agent = Vec::new();
    for e in entries {
        let ide = ca_settings::ide_schema::SECTIONS.contains(&e.section.as_str());
        let to = if ide { &mut out } else { &mut agent };
        if !e.section.is_empty() && !to.iter().any(|(s, _)| *s == e.section) {
            to.push((e.section.clone(), usize::from(!ide)));
        }
    }
    if !agent.is_empty() {
        out.push((AGENT.to_string(), 0));
        out.extend(agent);
    }
    out
}

/// Whether a setting is under a section of the left (the agent's heading has all the agent's own).
fn in_section(e: &Entry, section: &str) -> bool {
    match section {
        COMMON => COMMONLY_USED.contains(&e.key.as_str()),
        AGENT => !ca_settings::ide_schema::SECTIONS.contains(&e.section.as_str()),
        s => e.section == s,
    }
}

/// A text typed for a number or a text setting, as its value; why it cannot be one, in VS Code's words.
pub fn parse(e: &Entry, text: &str) -> Result<Value, String> {
    let bounds = |n: f64| -> Result<(), String> {
        if let Some(min) = e.min.filter(|m| n < *m) {
            return Err(format!("Value must be greater than or equal to {min}."));
        }
        if let Some(max) = e.max.filter(|m| n > *m) {
            return Err(format!("Value must be less than or equal to {max}."));
        }
        Ok(())
    };
    match e.kind.as_str() {
        "integer" => {
            let n: i64 = text.trim().parse().map_err(|_| match text.trim().parse::<f64>() {
                Ok(_) => "Value must be an integer.".to_string(),
                Err(_) => "Value must be a number.".to_string(),
            })?;
            bounds(n as f64)?;
            Ok(Value::from(n))
        }
        "number" => {
            let n: f64 = text.trim().parse().map_err(|_| "Value must be a number.".to_string())?;
            bounds(n)?;
            // A whole number is written as one, as a person types it.
            Ok(if n.fract() == 0.0 && n.abs() < 1e15 { Value::from(n as i64) } else { Value::from(n) })
        }
        "string" if e.value.is_null() && e.default.is_null() && text.is_empty() => Ok(Value::Null),
        _ => Ok(Value::String(text.to_string())),
    }
}

/// A value as its input shows it.
fn as_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

/// What is sent to keep a value in a tab's file (null: the setting taken out of it). The person's file loses a
/// setting set back to its default, as VS Code's; a project's keeps it, since it wins over the person's.
pub fn set_message(e: &Entry, value: &Value, target: Target, lang: Option<&str>) -> Value {
    let value = if target == Target::User && lang.is_none() && *value == e.default { Value::Null } else { value.clone() };
    let mut m = json!({"type": "set_setting", "key": e.key, "value": value, "where": target.layer()});
    if let Some(lang) = lang {
        m["lang"] = Value::from(lang);
    }
    m
}

/// A row of the list: a section's heading, or a setting by its key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Row {
    Heading(String),
    Setting(String),
}

/// The rows shown: with no search, the section picked's settings under its heading; with one, every setting found,
/// each section's under its heading.
fn rows(schema: &Schema, q: &Query, target: Target, section: &str, lang_of: impl Fn(&str) -> Option<Value>) -> Vec<Row> {
    let lang = q.lang.is_some();
    let mut out = Vec::new();
    let mut last = String::new();
    let common = section == COMMON && q.is_empty();
    let found = schema.entries.iter().filter(|e| {
        let own = if lang { lang_of(&e.key) } else { None };
        (!q.is_empty() || in_section(e, section)) && matches(e, q, target, modified(e, target, own.as_ref(), lang))
    });
    let mut found: Vec<&Entry> = found.collect();
    if common {
        found.sort_by_key(|e| COMMONLY_USED.iter().position(|k| *k == e.key));
    }
    for e in found {
        let heading = if common { COMMON } else { e.section.as_str() };
        if heading != last {
            last = heading.to_string();
            out.push(Row::Heading(last.clone()));
        }
        out.push(Row::Setting(e.key.clone()));
    }
    out
}

/// The editor's own, kept while its tab is not shown.
#[derive(Clone, Copy)]
pub struct Editor {
    /// Set only when what the session says differs.
    schema: RwSignal<Rc<Schema>>,
    search: RwSignal<String>,
    target: RwSignal<Target>,
    section: RwSignal<String>,
    input: RwSignal<Option<ViewId>>,
}

impl crate::ext::State for Editor {
    fn new(ui: Ui) -> Editor {
        Editor {
            schema: ui.scope.create_rw_signal(Rc::new(Schema::default())),
            search: ui.scope.create_rw_signal(String::new()),
            target: ui.scope.create_rw_signal(Target::User),
            section: ui.scope.create_rw_signal(COMMON.to_string()),
            input: ui.scope.create_rw_signal(None),
        }
    }
}

/// The editor opened on a tab, with a search when one is given (`@id:editor.tabSize`, `@modified`): its tab made or
/// shown, and the schema asked for again.
pub fn open(ui: Ui, send: &Send, target: Target, search: &str) {
    let s = ui.state::<Editor>();
    kinds::open(ui, TAB);
    send(json!({"type": "settings_schema"}));
    if s.target.get_untracked() != target {
        s.target.set(target);
    }
    if s.search.with_untracked(|t| t != search) {
        s.search.set(search.to_string());
    }
    focus(s.input);
}

fn focus(id: RwSignal<Option<ViewId>>) {
    exec_after(Duration::ZERO, move |_| {
        if let Some(id) = id.get_untracked() {
            id.request_focus();
        }
    });
}

fn copy(ui: Ui, text: &str) {
    if floem::Clipboard::set_contents(text.to_string()).is_err() {
        toast::warning(ui, "Could not copy to the clipboard.");
    }
}

/// The settings file a tab writes, opened in its editor.
fn open_file(ui: Ui, send: &Send, target: Target) {
    match target {
        Target::User => super::open_user(ui, send, "settings.json"),
        Target::Workspace => super::open_workspace(ui, send),
    }
}

/// The language of `@lang:` in the search, if any.
fn lang_of_search(s: Editor) -> Option<String> {
    s.search.with(|t| query(t).lang)
}

/// A row's gear menu, as VS Code's.
fn menu(ui: Ui, send: &Send, e: &Entry, target: Target, lang: Option<String>, is_modified: bool, at: popup::At) {
    let (a, b, send) = (e.clone(), e.clone(), send.clone());
    let value = {
        let own = lang.as_ref().and_then(|l| own_value(ui, &e.key, l));
        shown_value(e, target, own.as_ref())
    };
    let as_json = format!("\"{}\": {}", e.key, serde_json::to_string(&value).unwrap_or_default());
    let items: Vec<popup::Item> = vec![
        popup::entry("Reset Setting", move || {
            let mut m = set_message(&a, &Value::Null, target, lang.as_deref());
            m["value"] = Value::Null;
            send(m);
        })
        .enabled(is_modified && e.locked.is_none())
        .into(),
        popup::line(),
        popup::entry("Copy Setting ID", move || copy(ui, &b.key)).into(),
        popup::entry("Copy Setting as JSON", move || copy(ui, &as_json)).into(),
    ];
    popup::open(ui, at, items);
}

/// A language's own value of a setting, as the session said it last. Followed.
fn own_value(ui: Ui, key: &str, lang: &str) -> Option<Value> {
    ui.state::<super::Settings>().said.with(|s| s.by_lang.get(lang).and_then(|o| o.get(key)).cloned())
}

/// What a row draws that changes: its value, whether the tab's file sets it, and the language it is for.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    entry: Entry,
    target: Target,
    lang: Option<String>,
    value: Value,
    modified: bool,
}

/// A choice's control: the word in force in a box with a chevron, its menu the words.
fn choice(ui: Ui, send: Send, shown: Memo<Option<Shown>>) -> AnyView {
    let t = ui.colors;
    let text = label(move || shown.with(|s| s.as_ref().map(|s| as_text(&s.value)).unwrap_or_default()))
        .style(|s| s.flex_grow(1.0_f32).text_ellipsis().min_width(0.0));
    let view = h_stack((text, icon("chevron-down", 16.0))).style(move |s| {
        let c = t.get();
        s.min_width(200.0)
            .max_width(320.0)
            .height(26.0)
            .padding_left(8.0)
            .padding_right(4.0)
            .items_center()
            .border(1.0)
            .border_radius(2.0)
            .border_color(c.settings_dropdown_border)
            .background(c.settings_dropdown_bg)
            .color(c.menu_fg)
            .cursor(CursorStyle::Pointer)
    });
    let id = view.id();
    view.on_click_stop(move |_| {
        let Some(sh) = shown.get_untracked() else { return };
        if sh.entry.locked.is_some() {
            return;
        }
        let mut words: Vec<Value> = sh.entry.words.iter().map(|w| Value::from(w.as_str())).collect();
        if sh.entry.kind == "bool_or" {
            words.splice(0..0, [Value::Bool(true), Value::Bool(false)]);
        }
        let items = words
            .into_iter()
            .map(|w| {
                let (send, e, lang, on) = (send.clone(), sh.entry.clone(), sh.lang.clone(), sh.value == w);
                let target = sh.target;
                let name = as_text(&w);
                popup::entry(&name, move || send(set_message(&e, &w, target, lang.as_deref()))).radio(on).into()
            })
            .collect();
        popup::open(ui, popup::At::under(id), items);
    })
    .into_any()
}

/// A number's or a text's control: taken on Enter or when it loses the keys, refused with VS Code's words.
fn typed(ui: Ui, send: Send, shown: Memo<Option<Shown>>) -> AnyView {
    let t = ui.colors;
    let buffer = create_rw_signal(String::new());
    let focused = create_rw_signal(false);
    // What the session says is put in the box while it does not have the keys.
    floem::reactive::create_effect(move |_| {
        let now = shown.with(|s| s.as_ref().map(|s| as_text(&s.value)).unwrap_or_default());
        if !focused.get_untracked() && buffer.with_untracked(|b| *b != now) {
            buffer.set(now);
        }
    });
    let error = create_memo(move |_| {
        let text = buffer.get();
        shown.with(|s| s.as_ref().and_then(|s| parse(&s.entry, &text).err()))
    });
    // What was sent last, with the value it was sent over: Enter then the keys lost send it once.
    let sent = create_rw_signal(None::<(Value, Value)>);
    let commit = move || {
        let Some(sh) = shown.get_untracked() else { return };
        let text = buffer.get_untracked();
        match parse(&sh.entry, &text) {
            Ok(v) if v != sh.value && sh.entry.locked.is_none() => {
                let now = Some((v.clone(), sh.value.clone()));
                if sent.with_untracked(|s| *s != now) {
                    send(set_message(&sh.entry, &v, sh.target, sh.lang.as_deref()));
                    sent.set(now);
                }
            }
            Ok(_) => {}
            // Not taken: the box shows what is kept.
            Err(_) => buffer.set(as_text(&sh.value)),
        }
    };
    let on_enter = commit.clone();
    let input = text_input(buffer)
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            match &k.key.logical_key {
                Key::Named(NamedKey::Enter) => on_enter(),
                Key::Named(NamedKey::Escape) => {
                    buffer.set(shown.with_untracked(|s| s.as_ref().map(|s| as_text(&s.value)).unwrap_or_default()))
                }
                _ => return EventPropagation::Continue,
            }
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| {
            focused.set(false);
            commit();
        })
        .takes_keys(Focus::Input)
        .style(move |s| {
            let c = t.get();
            let wrong = error.with(Option::is_some);
            let number = shown.with(|s| s.as_ref().is_some_and(|s| matches!(s.entry.kind.as_str(), "integer" | "number")));
            let border = if wrong { c.input_error } else if focused.get() { c.focus } else { c.input_border };
            let s = s
                .width(if number { 200.0 } else { 500.0 })
                .max_width_full()
                .height(26.0)
                .padding_horiz(6.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(border)
                .background(c.input_bg)
                .color(c.input_fg)
                .cursor_color(Brush::Solid(c.cursor));
            crate::widgets::steady_input(s, c.input_bg, Some(border))
        });
    let message = label(move || error.get().unwrap_or_default()).style(move |s| {
        let c = t.get();
        s.padding_horiz(6.0)
            .padding_vert(4.0)
            .font_size(12.0)
            .border(1.0)
            .border_color(c.input_error)
            .background(c.input_error_bg)
            .color(c.editor_fg)
            .apply_if(error.with(Option::is_none), |s| s.hide())
    });
    v_stack((input, message)).style(|s| s.items_start().width_full().min_width(0.0)).into_any()
}

/// A link of the row's: VS Code's `textLink`.
fn link(ui: Ui, words: &'static str, on: impl Fn() + 'static) -> impl IntoView {
    let t = ui.colors;
    label(move || words)
        .on_click_stop(move |_| on())
        .style(move |s| s.color(t.get().link).cursor(CursorStyle::Pointer).selectable(false))
}

/// One setting's row.
fn setting_row(ui: Ui, send: Send, key: String) -> AnyView {
    let t = ui.colors;
    let s = ui.state::<Editor>();
    let k = key.clone();
    let shown = create_memo(move |_| {
        let target = s.target.get();
        let lang = lang_of_search(s);
        s.schema.with(|sc| {
            let entry = sc.get(&k)?.clone();
            let own = lang.as_ref().and_then(|l| own_value(ui, &k, l));
            let value = shown_value(&entry, target, own.as_ref());
            let modified = modified(&entry, target, own.as_ref(), lang.is_some());
            Some(Shown { entry, target, lang, value, modified })
        })
    });
    let Some(first) = shown.get_untracked() else { return empty().into_any() };
    let e = first.entry;
    let (category, name) = title(&e.key);
    let is_modified = move || shown.with(|s| s.as_ref().is_some_and(|s| s.modified));
    let bar = empty().style(move |s| {
        let c = t.get();
        s.position(Position::Absolute)
            .inset_left(0.0)
            .inset_top(12.0)
            .inset_bottom(12.0)
            .width(2.0)
            .background(c.settings_modified)
            .apply_if(!is_modified(), |s| s.hide())
    });
    let hovered = create_rw_signal(false);
    let gear_send = send.clone();
    let gear = container(icon("settings-gear", 16.0)).style(move |s| {
        let c = t.get();
        s.position(Position::Absolute)
            .inset_left(4.0)
            .inset_top(10.0)
            .width(22.0)
            .height(22.0)
            .items_center()
            .justify_center()
            .border_radius(5.0)
            .color(c.icon)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.background(c.toolbar_hover))
            .apply_if(!hovered.get(), |s| s.hide())
    });
    let gear_id = gear.id();
    let gear = gear.on_click_stop(move |_| {
        if let Some(sh) = shown.get_untracked() {
            menu(ui, &gear_send, &sh.entry, sh.target, sh.lang.clone(), sh.modified, popup::At::under(gear_id));
        }
    });
    // "(Modified in: …)", the language's, the environment's, the edition's.
    let notes = label(move || {
        shown.with(|sh| {
            let Some(sh) = sh else { return String::new() };
            let mut said = Vec::new();
            if let Some(lang) = &sh.lang {
                said.push(format!("For {lang}"));
            }
            let also = also_in(&sh.entry, sh.target);
            if !also.is_empty() && sh.lang.is_none() {
                said.push(format!("Also modified in: {}", also.join(", ")));
            }
            said.join(" · ")
        })
    })
    .style(move |s| s.margin_left(8.0).color(t.get().desc).font_size(12.0).text_ellipsis().min_width(0.0));
    let heading = h_stack((
        label(move || if category.is_empty() { String::new() } else { format!("{category}: ") })
            .style(|s| s.flex_shrink(0.0_f32)),
        label(move || name.clone()).style(|s| s.font_weight(*SEMIBOLD).flex_shrink(0.0_f32)),
        notes,
    ))
    .style(move |s| s.items_center().color(t.get().settings_header).min_width(0.0));
    let description = e.description.clone();
    let desc = label(move || description.clone()).style(move |s| {
        s.color(t.get().desc).line_height(1.4).margin_top(4.0).max_width_full()
    });
    let send_c = send.clone();
    let control: AnyView = match e.kind.as_str() {
        "bool" => {
            let on = move || shown.with(|s| s.as_ref().is_some_and(|s| s.value == Value::Bool(true)));
            let mark = container(icon("check", 14.0).style(move |s| s.apply_if(!on(), |s| s.hide()))).style(move |s| {
                let c = t.get();
                s.width(18.0)
                    .height(18.0)
                    .flex_shrink(0.0_f32)
                    .items_center()
                    .justify_center()
                    .border(1.0)
                    .border_radius(3.0)
                    .border_color(c.checkbox_border)
                    .background(c.checkbox_bg)
                    .color(c.checkbox_fg)
            });
            let description = e.description.clone();
            h_stack((mark, label(move || description.clone()).style(move |s| s.color(t.get().desc).line_height(1.4))))
                .on_click_stop(move |_| {
                    if let Some(sh) = shown.get_untracked().filter(|s| s.entry.locked.is_none()) {
                        let to = Value::Bool(sh.value != Value::Bool(true));
                        send_c(set_message(&sh.entry, &to, sh.target, sh.lang.as_deref()));
                    }
                })
                .style(|s| s.gap(8.0).items_start().margin_top(6.0).cursor(CursorStyle::Pointer))
                .into_any()
        }
        "enum" | "bool_or" => choice(ui, send_c, shown),
        "string" | "integer" | "number" => typed(ui, send_c, shown),
        _ => {
            let send = send_c.clone();
            link(ui, "Edit in settings.json", move || {
                open_file(ui, &send, s.target.get_untracked());
            })
            .into_any()
        }
    };
    let control = container(control).style(|s| s.margin_top(8.0).width_full().min_width(0.0));
    // What wins over it, and what in a file was not taken.
    let k = key.clone();
    let warning = label(move || {
        let mut said = Vec::new();
        shown.with(|sh| {
            let Some(sh) = sh else { return };
            if let Some(name) = sh.entry.source.strip_prefix("env:") {
                said.push(format!("The environment variable {name} sets it for this run, and wins over the files."));
            } else if sh.entry.source == "flag" {
                said.push("The command line sets it for this run, and wins over the files.".to_string());
            }
            if let Some(why) = &sh.entry.locked {
                said.push(why.clone());
            }
        });
        s.schema.with(|sc| {
            for p in sc.problems.iter().filter(|p| p.key == k) {
                said.push(format!("{}: {}", super::name_of(&p.file), p.error));
            }
        });
        said.join("\n")
    })
    .style(move |s| {
        let quiet = shown.with(|sh| {
            sh.as_ref().is_some_and(|sh| {
                sh.entry.locked.is_none() && sh.entry.source != "flag" && !sh.entry.source.starts_with("env:")
            })
        }) && no_problem(ui, &key);
        s.margin_top(6.0).color(t.get().warning).font_size(12.0).line_height(1.4).apply_if(quiet, |s| s.hide())
    });
    let bool_row = e.kind == "bool";
    let body = v_stack((heading, desc.style(move |s| s.apply_if(bool_row, |s| s.hide())), control, warning))
        .style(|s| s.width_full().min_width(0.0));
    let menu_send = send.clone();
    container(h_stack((bar, gear, body)).style(|s| s.width_full()))
        .on_event_cont(EventListener::PointerEnter, move |_| hovered.set(true))
        .on_event_cont(EventListener::PointerLeave, move |_| hovered.set(false))
        .on_secondary_click_stop(move |_| {
            if let Some(sh) = shown.get_untracked() {
                let (x, y) = ui.pointer.get_untracked();
                menu(ui, &menu_send, &sh.entry, sh.target, sh.lang.clone(), sh.modified, popup::At::point(gear_id, x, y));
            }
        })
        .style(move |s| {
            let c = t.get();
            s.width_full()
                .padding_left(32.0)
                .padding_right(16.0)
                .padding_vert(12.0)
                .font_size(13.0)
                .hover(|s| s.background(c.settings_row_hover))
        })
        .into_any()
}

/// Whether no settings file has a problem with this setting. Followed.
fn no_problem(ui: Ui, key: &str) -> bool {
    ui.state::<Editor>().schema.with(|sc| !sc.problems.iter().any(|p| p.key == key))
}

/// A section's heading among the rows.
fn heading_row(ui: Ui, name: String) -> AnyView {
    let t = ui.colors;
    label(move || name.clone())
        .style(move |s| {
            s.font_size(20.0)
                .font_weight(*SEMIBOLD)
                .color(t.get().settings_header)
                .padding_left(32.0)
                .padding_top(16.0)
                .padding_bottom(4.0)
                .selectable(false)
        })
        .into_any()
}

/// The search box, with the number of settings found.
fn search_box(ui: Ui, found: Memo<Option<usize>>) -> impl IntoView {
    let t = ui.colors;
    let s = ui.state::<Editor>();
    let focused = create_rw_signal(false);
    let input = text_input(s.search)
        .placeholder("Search settings")
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            if matches!(k.key.logical_key, Key::Named(NamedKey::Escape)) && s.search.with_untracked(|t| !t.is_empty()) {
                s.search.set(String::new());
                return EventPropagation::Stop;
            }
            EventPropagation::Continue
        })
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| focused.set(false))
        .takes_keys(Focus::Input)
        .style(move |st| {
            let c = t.get();
            let st = crate::widgets::steady_input(st, Color::TRANSPARENT, None);
            st.flex_grow(1.0_f32)
                .min_width(0.0)
                .padding_horiz(6.0)
                .border(0.0)
                .font_size(14.0)
                .background(Color::TRANSPARENT)
                .color(c.input_fg)
                .cursor_color(Brush::Solid(c.cursor))
        });
    s.input.set(Some(input.id()));
    let count = label(move || match found.get() {
        None => String::new(),
        Some(0) => "No Settings Found".to_string(),
        Some(1) => "1 Setting Found".to_string(),
        Some(n) => format!("{n} Settings Found"),
    })
    .style(move |s| s.padding_horiz(6.0).font_size(12.0).color(t.get().desc).selectable(false));
    let clear = crate::widgets::action(t, "clear-all", move || {
        if s.search.with_untracked(|t| !t.is_empty()) {
            s.search.set(String::new());
        }
    });
    h_stack((input, count, clear)).style(move |st| {
        let c = t.get();
        st.width_full()
            .height(30.0)
            .padding_right(4.0)
            .items_center()
            .border(1.0)
            .border_color(if focused.get() { c.focus } else { c.input_border })
            .background(c.input_bg)
    })
}

/// The User and Workspace tabs, and the action that opens the tab's file.
fn targets(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let s = ui.state::<Editor>();
    let tab = move |target: Target, name: &'static str| {
        label(move || name)
            .on_click_stop(move |_| {
                if s.target.get_untracked() != target {
                    s.target.set(target);
                }
            })
            .style(move |st| {
                let c = t.get();
                let on = s.target.get() == target;
                st.padding_horiz(2.0)
                    .padding_vert(6.0)
                    .margin_right(16.0)
                    .font_size(13.0)
                    .selectable(false)
                    .cursor(CursorStyle::Pointer)
                    .border_bottom(1.0)
                    .border_color(if on { c.panel_title_border } else { Color::TRANSPARENT })
                    .color(if on { c.panel_title } else { c.panel_title_inactive })
            })
    };
    let open_json = container(icon("go-to-file", 16.0))
        .on_click_stop(move |_| open_file(ui, &send, s.target.get_untracked()))
        .style(move |s| {
            let c = t.get();
            s.width(22.0)
                .height(22.0)
                .items_center()
                .justify_center()
                .border_radius(5.0)
                .color(c.icon)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.toolbar_hover))
        });
    let open_json = floem::views::tooltip(open_json, || label(|| "Open Settings (JSON)"));
    h_stack((
        tab(Target::User, "User"),
        tab(Target::Workspace, "Workspace"),
        empty().style(|s| s.flex_grow(1.0_f32)),
        open_json,
    ))
    .style(move |s| s.width_full().items_center().margin_top(8.0).border_bottom(1.0).border_color(t.get().panel_border))
}

/// What the tab's file holds that is no setting, and what could not be read of it, above the rows.
fn banner(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let s = ui.state::<Editor>();
    let said = create_memo(move |_| {
        let target = s.target.get();
        s.schema.with(|sc| {
            let mut lines: Vec<String> = sc
                .problems
                .iter()
                .filter(|p| p.key.is_empty())
                .map(|p| format!("{}: {}", super::name_of(&p.file), p.error))
                .collect();
            let unknown = sc.unknown_in(target);
            if !unknown.is_empty() {
                let shown: Vec<&str> = unknown.iter().take(8).map(String::as_str).collect();
                let more = if unknown.len() > 8 { format!(" and {} more", unknown.len() - 8) } else { String::new() };
                lines.push(format!(
                    "Other settings in settings.json, which Code Arena does not read: {}{more}",
                    shown.join(", ")
                ));
            }
            lines
        })
    });
    dyn_container(
        move || said.get(),
        move |lines| {
            if lines.is_empty() {
                return empty().into_any();
            }
            let send = send.clone();
            let mut views: Vec<AnyView> = lines
                .into_iter()
                .map(|l| {
                    h_stack((
                        icon("warning", 16.0).style(move |s| s.color(t.get().warning).margin_right(6.0).flex_shrink(0.0_f32)),
                        label(move || l.clone()).style(|s| s.min_width(0.0)),
                    ))
                    .style(|s| s.items_start())
                    .into_any()
                })
                .collect();
            views.push(
                link(ui, "Open Settings (JSON)", move || open_file(ui, &send, s.target.get_untracked()))
                    .style(|s| s.margin_left(22.0))
                    .into_any(),
            );
            v_stack_from_iter(views)
                .style(move |s| {
                    let c = t.get();
                    s.width_full()
                        .margin_top(8.0)
                        .padding(8.0)
                        .gap(4.0)
                        .font_size(12.0)
                        .border(1.0)
                        .border_color(c.notice_border)
                        .background(c.notice_bg)
                })
                .into_any()
        },
    )
}

/// The sections at the left: the one picked shows its settings while nothing is searched.
fn toc(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let s = ui.state::<Editor>();
    let all = create_memo(move |_| s.schema.with(|sc| sections(&sc.entries)));
    let searching = create_memo(move |_| s.search.with(|t| !t.trim().is_empty()));
    let list = dyn_stack(
        move || all.get(),
        |(name, _)| name.clone(),
        move |(name, depth)| {
            let (n, lit) = (name.clone(), name.clone());
            label(move || name.clone())
                .on_click_stop(move |_| {
                    if s.section.with_untracked(|x| *x != n) {
                        s.section.set(n.clone());
                    }
                    if s.search.with_untracked(|t| !t.is_empty()) {
                        s.search.set(String::new());
                    }
                })
                .style(move |st| {
                    let c = t.get();
                    let on = !searching.get() && s.section.with(|x| *x == lit);
                    st.width_full()
                        .height(22.0)
                        .padding_left(8.0 + 12.0 * depth as f64)
                        .items_center()
                        .font_size(13.0)
                        .selectable(false)
                        .text_ellipsis()
                        .cursor(CursorStyle::Pointer)
                        .color(if on { c.settings_header } else { c.desc })
                        .apply_if(on, |s| s.font_weight(*SEMIBOLD))
                        .hover(|s| s.background(c.list_hover).color(c.settings_header))
                })
        },
    )
    .style(|s| s.flex_col().width_full());
    scroll(list).style(|s| s.width(180.0).flex_shrink(0.0_f32).height_full().padding_top(12.0))
}

/// The editor drawn in its tab.
fn view(ui: Ui, send: Send) -> AnyView {
    let t = ui.colors;
    let s = ui.state::<Editor>();
    let shown = create_memo(move |_| {
        let target = s.target.get();
        let q = s.search.with(|t| query(t));
        let section = s.section.get();
        let lang = q.lang.clone();
        s.schema.with(|sc| {
            rows(sc, &q, target, &section, |key| lang.as_ref().and_then(|l| own_value(ui, key, l)))
        })
    });
    let found = create_memo(move |_| {
        let searching = s.search.with(|t| !t.trim().is_empty());
        searching.then(|| shown.with(|r| r.iter().filter(|r| matches!(r, Row::Setting(_))).count()))
    });
    let row_send = send.clone();
    let list = dyn_stack(
        move || shown.get(),
        |row| row.clone(),
        move |row| match row {
            Row::Heading(name) => heading_row(ui, name),
            Row::Setting(key) => setting_row(ui, row_send.clone(), key),
        },
    )
    .style(|s| s.flex_col().width_full().padding_bottom(24.0));
    let loading = label(|| "Reading the settings…")
        .style(move |st| st.padding(32.0).color(t.get().desc).apply_if(s.schema.with(|sc| !sc.entries.is_empty()), |s| s.hide()));
    let rows_view = scroll(v_stack((loading, list)).style(|s| s.width_full().max_width(1000.0)))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).height_full().min_width(0.0));
    let body = h_stack((toc(ui), rows_view)).style(|s| s.width_full().flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0));
    let header = v_stack((search_box(ui, found), targets(ui, send.clone()), banner(ui, send.clone())))
        .style(|s| s.width_full().max_width(1180.0).padding_horiz(24.0).padding_top(12.0));
    focus(s.input);
    v_stack((header, body))
        .style(move |st| {
            let c = t.get();
            st.size_full().background(c.editor_bg).color(c.editor_fg).font_size(13.0)
        })
        .into_any()
}

/// The editor's kind of tab and its commands: Open Settings (Ctrl+,) and VS Code's Open User Settings and Open
/// Workspace Settings.
pub fn register(_ui: Ui, _send: &Send) {
    kinds::add(
        kinds::kind(KIND, |_ui, _rest| "Settings".to_string(), |ui, send, _tab| view(ui, send))
            .icon("settings-gear")
            .kept(|ui, send, _path| {
                kinds::open(ui, TAB);
                send(json!({"type": "settings_schema"}));
            }),
    );
    commands::add(command(crate::manage::OPEN_SETTINGS, "Preferences", "Open Settings", |ui, send| {
        let target = ui.state::<Editor>().target.get_untracked();
        open(ui, send, target, "")
    }));
    commands::add(command(OPEN_USER, "Preferences", "Open User Settings", |ui, send| {
        open(ui, send, Target::User, "")
    }));
    commands::add(command(OPEN_WORKSPACE, "Preferences", "Open Workspace Settings", |ui, send| {
        open(ui, send, Target::Workspace, "")
    }));
}

/// The session's messages that are this part's, taken in: the schema kept when it is another, and asked for again
/// when the settings are said while the editor's tab is open. None is taken away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "settings_schema" => {
            let s = ui.state::<Editor>();
            let schema = Schema::from_json(m);
            if s.schema.with_untracked(|was| **was != schema) {
                s.schema.set(Rc::new(schema));
            }
        }
        "settings" if ui.tab(TAB).is_some() => send(json!({"type": "settings_schema"})),
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn window() -> (Ui, Send, Rc<RefCell<Vec<Value>>>) {
        let ui = Ui::new();
        let told = Rc::new(RefCell::new(Vec::new()));
        let send: Send = {
            let to = told.clone();
            Rc::new(move |v: Value| to.borrow_mut().push(v))
        };
        ui.out.set(Some(send.clone()));
        super::super::register(ui, &send);
        register(ui, &send);
        (ui, send, told)
    }

    fn schema() -> Value {
        json!({
            "type": "settings_schema",
            "entries": [
                {"key": "editor.fontSize", "kind": "number", "min": 6, "max": 100, "default": 13, "description": "The font size in pixels.",
                 "section": "Text Editor", "scope": "language", "value": 16, "source": "workspace",
                 "written": {"user": 15, "workspace": 16}, "in": ["user", "workspace"]},
                {"key": "editor.tabSize", "kind": "integer", "min": 1, "max": 100, "default": 4, "description": "The number of spaces a tab is.",
                 "section": "Text Editor", "scope": "language", "value": 4, "source": "default", "written": {}, "in": []},
                {"key": "workbench.editor.enablePreview", "kind": "bool", "default": true, "description": "Opens files in preview.",
                 "section": "Workbench", "scope": "project", "value": true, "source": "default", "written": {}, "in": []},
                {"key": "agent.mode", "kind": "enum", "enum": ["ask", "plan", "auto"], "default": "ask", "description": "How the agent works.",
                 "section": "Agent", "scope": "user", "env": "CODE_ARENA_MODE", "value": "plan", "source": "env:CODE_ARENA_MODE", "written": {}, "in": []},
                {"key": "mcpServers", "kind": "object", "default": {}, "description": "The MCP servers.",
                 "section": "MCP servers", "scope": "user", "value": null, "source": "user", "written": {}, "in": ["user"]},
                {"key": "arena.syncChats", "kind": "bool", "default": false, "description": "Keeps chats in Arena.",
                 "section": "Arena", "scope": "user", "value": false, "source": "default", "written": {}, "in": [],
                 "locked": "arena.syncChats needs Farzan Arena."}
            ],
            "problems": [{"key": "editor.tabSize", "file": "/home/me/.config/code-arena/settings.json", "error": "\"editor.tabSize\" takes a whole number."}],
            "unknown": {"user": ["my.own"], "project": [], "workspace": []},
            "files": {"user": "/home/me/.config/code-arena/settings.json"}
        })
    }

    fn keys(rows: Vec<Row>) -> Vec<String> {
        rows.into_iter()
            .map(|r| match r {
                Row::Heading(h) => format!("# {h}"),
                Row::Setting(k) => k,
            })
            .collect()
    }

    #[test]
    fn a_search_reads_words_and_vs_codes_filters() {
        assert_eq!(
            query("@modified  Font @id:editor.fontSize,editor.tabSize @lang:Python size"),
            Query {
                words: vec!["font".into(), "size".into()],
                modified: true,
                ids: vec!["editor.fontSize".into(), "editor.tabSize".into()],
                lang: Some("python".into()),
            }
        );
        assert!(query("  ").is_empty());
        assert_eq!(title("editor.fontSize"), ("Editor".into(), "Font Size".into()));
        assert_eq!(title("workbench.editor.enablePreview"), ("Workbench › Editor".into(), "Enable Preview".into()));
        assert_eq!(title("mcpServers"), (String::new(), "Mcp Servers".into()));
        assert_eq!(title("editor.minimap.maxColumn"), ("Editor › Minimap".into(), "Max Column".into()));
    }

    #[test]
    fn the_rows_are_the_section_picked_or_what_the_search_finds_under_their_sections() {
        let s = Schema::from_json(&schema());
        assert_eq!(s.entries.len(), 6);
        let names: Vec<String> = sections(&s.entries).into_iter().map(|(n, d)| format!("{d}{n}")).collect();
        assert_eq!(names, ["0Commonly Used", "0Text Editor", "0Workbench", "0Agent", "1Agent", "1MCP servers", "1Arena"]);
        let none = |_: &str| None;
        let rows_of = |q: &str, target: Target, section: &str| keys(rows(&s, &query(q), target, section, none));
        // Commonly Used, in its own order.
        assert_eq!(
            rows_of("", Target::User, COMMON),
            ["# Commonly Used", "editor.fontSize", "editor.tabSize", "workbench.editor.enablePreview", "agent.mode"]
        );
        assert_eq!(rows_of("", Target::User, "Workbench"), ["# Workbench", "workbench.editor.enablePreview"]);
        // The agent's heading has every one of its sections.
        assert_eq!(
            rows_of("", Target::User, AGENT),
            ["# Agent", "agent.mode", "# MCP servers", "mcpServers", "# Arena", "arena.syncChats"]
        );
        // A search looks in every section, by the title, the key and the description.
        assert_eq!(rows_of("font size", Target::User, "Workbench"), ["# Text Editor", "editor.fontSize"]);
        assert_eq!(rows_of("size", Target::User, "Workbench"), ["# Text Editor", "editor.fontSize", "editor.tabSize"]);
        assert_eq!(rows_of("spaces", Target::User, COMMON), ["# Text Editor", "editor.tabSize"]);
        assert_eq!(rows_of("@id:agent.mode", Target::User, COMMON), ["# Agent", "agent.mode"]);
        // @modified: what the tab's file sets.
        assert_eq!(
            rows_of("@modified", Target::User, COMMON),
            ["# Text Editor", "editor.fontSize", "# MCP servers", "mcpServers"]
        );
        // The Workspace tab has no setting that is the person's alone.
        assert_eq!(rows_of("@modified", Target::Workspace, COMMON), ["# Text Editor", "editor.fontSize"]);
        assert!(rows_of("agent", Target::Workspace, COMMON).is_empty());
        // @lang: the settings a language may have, modified when it has its own.
        let python = |k: &str| (k == "editor.tabSize").then(|| json!(2));
        assert_eq!(
            keys(rows(&s, &query("@lang:python"), Target::User, COMMON, python)),
            ["# Text Editor", "editor.fontSize", "editor.tabSize"]
        );
        assert_eq!(
            keys(rows(&s, &query("@lang:python @modified"), Target::User, COMMON, python)),
            ["# Text Editor", "editor.tabSize"]
        );
        assert_eq!(s.unknown_in(Target::User), ["my.own"]);
        assert!(s.unknown_in(Target::Workspace).is_empty());
        // Nothing said yet: nothing unknown.
        assert!(Schema::default().unknown_in(Target::Workspace).is_empty());
    }

    #[test]
    fn a_tab_shows_its_files_value_says_where_else_it_is_set_and_writes_its_file() {
        let s = Schema::from_json(&schema());
        let size = s.get("editor.fontSize").expect("font size");
        assert_eq!(shown_value(size, Target::User, None), json!(15));
        assert_eq!(shown_value(size, Target::Workspace, None), json!(16));
        assert_eq!(shown_value(size, Target::User, Some(&json!(12))), json!(12));
        let tab = s.get("editor.tabSize").expect("tab size");
        assert_eq!(shown_value(tab, Target::Workspace, None), json!(4));
        assert!(modified(size, Target::User, None, false) && !modified(tab, Target::User, None, false));
        assert!(modified(tab, Target::User, Some(&json!(2)), true) && !modified(size, Target::User, None, true));
        assert_eq!(also_in(size, Target::User), ["Workspace"]);
        assert_eq!(also_in(size, Target::Workspace), ["User"]);
        // Set back to its default: taken out of the person's file, kept in a project's (it wins over the person's).
        assert_eq!(
            set_message(size, &json!(13), Target::User, None),
            json!({"type": "set_setting", "key": "editor.fontSize", "value": null, "where": "user"})
        );
        assert_eq!(
            set_message(size, &json!(13), Target::Workspace, None),
            json!({"type": "set_setting", "key": "editor.fontSize", "value": 13, "where": "workspace"})
        );
        assert_eq!(
            set_message(tab, &json!(2), Target::User, Some("python")),
            json!({"type": "set_setting", "key": "editor.tabSize", "value": 2, "where": "user", "lang": "python"})
        );
    }

    #[test]
    fn a_typed_value_is_refused_in_vs_codes_words() {
        let s = Schema::from_json(&schema());
        let (size, tab) = (s.get("editor.fontSize").expect("size"), s.get("editor.tabSize").expect("tab"));
        assert_eq!(parse(size, "14"), Ok(json!(14)));
        assert_eq!(parse(size, " 13.5 "), Ok(json!(13.5)));
        assert_eq!(parse(size, "5"), Err("Value must be greater than or equal to 6.".into()));
        assert_eq!(parse(size, "101"), Err("Value must be less than or equal to 100.".into()));
        assert_eq!(parse(size, "big"), Err("Value must be a number.".into()));
        assert_eq!(parse(tab, "2.5"), Err("Value must be an integer.".into()));
        assert_eq!(parse(tab, "x"), Err("Value must be a number.".into()));
        assert_eq!(parse(tab, "8"), Ok(json!(8)));
        let text = Entry { kind: "string".into(), default: json!(""), value: json!(""), ..Entry::default() };
        assert_eq!(parse(&text, "Fira Code"), Ok(json!("Fira Code")));
        assert_eq!(as_text(&json!("on")), "on");
        assert_eq!(as_text(&json!(4)), "4");
        assert_eq!(as_text(&Value::Null), "");
    }

    #[test]
    fn ctrl_comma_opens_the_editor_which_asks_for_its_schema_while_its_tab_is_open() {
        let (ui, send, told) = window();
        let last = || told.borrow().last().cloned();
        assert!(commands::run(ui, &send, crate::manage::OPEN_SETTINGS));
        assert_eq!(ui.active.get_untracked().as_deref(), Some(TAB));
        assert_eq!(last(), Some(json!({"type": "settings_schema"})));
        let tab = ui.tab(TAB).expect("its tab");
        assert_eq!(kinds::title(ui, &tab), "Settings");
        take(ui, &schema(), &send);
        let s = ui.state::<Editor>();
        assert_eq!(s.schema.get_untracked().entries.len(), 6);
        // The same schema again changes nothing.
        let was = s.schema.get_untracked();
        take(ui, &schema(), &send);
        assert!(Rc::ptr_eq(&was, &s.schema.get_untracked()));
        // The settings said again: the schema is asked for again, while the tab is open.
        told.borrow_mut().clear();
        take(ui, &json!({"type": "settings", "values": {}}), &send);
        assert_eq!(last(), Some(json!({"type": "settings_schema"})));
        let _view = kinds::draw(ui, &tab);
        // Open Workspace Settings, with a search.
        open(ui, &send, Target::Workspace, "@modified");
        assert_eq!((s.target.get_untracked(), s.search.get_untracked()), (Target::Workspace, "@modified".to_string()));
        assert!(commands::run(ui, &send, OPEN_USER));
        assert_eq!((s.target.get_untracked(), s.search.get_untracked()), (Target::User, String::new()));
        ui.close(TAB);
        told.borrow_mut().clear();
        take(ui, &json!({"type": "settings", "values": {}}), &send);
        assert!(told.borrow().is_empty());
    }
}
