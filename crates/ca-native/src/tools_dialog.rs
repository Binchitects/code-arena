//! The Configure Tools dialog (Agent: Configure Tools…, and Customize tools… in the tools picker): every tool of
//! the session by where it comes from, each with its switch and how it asks, side by side; under its gear, what the
//! model is told of it and how each of its arguments is given. Kept for you in every project, or, with "Only in
//! this project", for you in this project only. The project's own file may only turn a tool off or make it ask.
//!
//! The tools come in the driver's `tool_settings` (asked when the dialog opens, and again when the session is said
//! or a turn ends while it is open); a change sends `tool_settings_set{name, level, settings, key}`, answered with
//! `tool_settings` again or `refused` (shown as a toast). While a turn runs a change waits for its end.

use std::collections::BTreeSet;

use floem::event::{Event, EventListener};
use floem::keyboard::{Key, NamedKey};
use floem::peniko::Brush;
use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{
    Decorators, container, dyn_container, dyn_stack, empty, h_stack, h_stack_from_iter, label, scroll, text_input,
    tooltip, v_stack, v_stack_from_iter,
};
use floem::{AnyView, IntoView, View};
use serde_json::{Map, Value, json};

use crate::agent::tools_picker::CUSTOMIZE;
use crate::commands;
use crate::icons::icon;
use crate::keymap::{Focus, TakesKeys};
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::widgets::popup::{self, At};
use crate::widgets::{MONO_FONT, Theme, button, mix, toast};

/// One argument of a tool, as it is defined.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arg {
    pub name: String,
    pub ty: Option<String>,
    pub about: Option<String>,
}

/// A tool's settings at one level, as they are kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Choice {
    pub enabled: Option<bool>,
    /// "ask" or "allow"; none: as the mode says.
    pub approval: Option<String>,
    pub description: Option<String>,
    pub defaults: Map<String, Value>,
    pub fixed: Map<String, Value>,
}

impl Choice {
    fn of(v: &Value) -> Choice {
        let map = |k: &str| v[k].as_object().cloned().unwrap_or_default();
        Choice {
            enabled: v["enabled"].as_bool(),
            approval: v["approval"].as_str().filter(|a| *a == "ask" || *a == "allow").map(String::from),
            description: v["description"].as_str().filter(|d| !d.trim().is_empty()).map(String::from),
            defaults: map("defaults"),
            fixed: map("fixed"),
        }
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        if let Some(e) = self.enabled {
            o.insert("enabled".into(), json!(e));
        }
        if let Some(a) = &self.approval {
            o.insert("approval".into(), json!(a));
        }
        if let Some(d) = &self.description {
            o.insert("description".into(), json!(d));
        }
        if !self.defaults.is_empty() {
            o.insert("defaults".into(), Value::Object(self.defaults.clone()));
        }
        if !self.fixed.is_empty() {
            o.insert("fixed".into(), Value::Object(self.fixed.clone()));
        }
        Value::Object(o)
    }

    /// Only its switch and how it asks: what Reset leaves.
    fn bare(&self) -> Choice {
        Choice {
            enabled: (self.enabled == Some(false)).then_some(false),
            approval: self.approval.clone(),
            ..Choice::default()
        }
    }

    /// Turned on (as its level below says) or off.
    pub fn switched(&self, on: bool) -> Choice {
        Choice { enabled: (!on).then_some(false), ..self.clone() }
    }

    /// Asking as chosen: "default" leaves it to the mode.
    pub fn asking(&self, how: &str) -> Choice {
        Choice { approval: (how != "default").then(|| how.to_string()), ..self.clone() }
    }
}

/// One tool as the dialog lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub name: String,
    pub group_title: String,
    /// read, edit or shell.
    pub kind: String,
    pub description: String,
    pub arguments: Vec<Arg>,
    pub enabled: bool,
    /// How it asks now: ask, allow, mode or never.
    pub asks: String,
    pub person: Choice,
    pub here: Choice,
    pub project: Choice,
    /// Why it may not run unasked.
    pub locked: Option<String>,
}

impl Entry {
    fn of(v: &Value) -> Option<Entry> {
        let text = |v: &Value| v.as_str().map(String::from);
        Some(Entry {
            name: v["name"].as_str()?.to_string(),
            group_title: text(&v["group_title"]).or_else(|| text(&v["group"])).unwrap_or_default(),
            kind: text(&v["kind"]).unwrap_or_else(|| "read".into()),
            description: text(&v["description"]).unwrap_or_default(),
            arguments: v["arguments"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| {
                    Some(Arg { name: a["name"].as_str()?.into(), ty: text(&a["type"]), about: text(&a["description"]) })
                })
                .collect(),
            enabled: v["enabled"].as_bool().unwrap_or(true),
            asks: text(&v["asks"]).unwrap_or_else(|| "mode".into()),
            person: Choice::of(&v["person"]),
            here: Choice::of(&v["here"]),
            project: Choice::of(&v["project"]),
            locked: text(&v["locked"]),
        })
    }

    /// Its settings at the level being set.
    pub fn at(&self, here: bool) -> &Choice {
        if here { &self.here } else { &self.person }
    }

    fn off_by_project(&self) -> bool {
        self.project.enabled == Some(false)
    }

    /// Whether how it asks may be chosen: not for a tool that only reads.
    fn asks_chosen(&self) -> bool {
        self.kind != "read" && self.asks != "never"
    }

    /// Its tooltip: what it does, where it comes from, how it asks.
    pub fn tip(&self) -> String {
        let kind = match self.kind.as_str() {
            "edit" => "edits files",
            "shell" => "runs commands",
            _ => "reads",
        };
        let asks = match self.asks.as_str() {
            "ask" => "Asks before each call",
            "allow" => "Never asks",
            "never" => "Reads only: never asks",
            _ => "Asks as the mode says",
        };
        let mut parts = vec![kind.to_string()];
        if !self.group_title.is_empty() {
            parts.push(self.group_title.clone());
        }
        parts.push(asks.to_string());
        parts.join(" · ")
    }
}

/// How a tool may ask, as the dialog offers it.
const ASKS: [(&str, &str); 3] = [("default", "As the mode says"), ("ask", "Always asks"), ("allow", "Never asks")];

/// How an argument is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Model,
    Default,
    Fixed,
}

impl How {
    fn words(self) -> &'static str {
        match self {
            How::Model => "The model gives it",
            How::Default => "A default",
            How::Fixed => "Always this",
        }
    }
}

/// An argument's value as typed, as its type takes it: a number, true or false, JSON for an object or a list; else
/// the text.
pub fn typed(text: &str, ty: Option<&str>) -> Value {
    let t = text.trim();
    match ty {
        Some("number" | "integer") => {
            serde_json::from_str::<serde_json::Number>(t).map(Value::Number).unwrap_or_else(|_| json!(t))
        }
        Some("boolean") => match t {
            "true" => json!(true),
            "false" => json!(false),
            _ => json!(t),
        },
        Some("object" | "array") => serde_json::from_str(t).unwrap_or_else(|_| json!(t)),
        _ => json!(text),
    }
}

/// A value as its field shows it: a string as it is, anything else as JSON.
pub fn shown(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The form's settings, from what was chosen under the gear: the switch and how it asks as they were.
pub fn formed(was: &Choice, description: &str, args: &[(Arg, How, String)]) -> Choice {
    let pick = |how: How| -> Map<String, Value> {
        args.iter().filter(|(_, h, _)| *h == how).map(|(a, _, v)| (a.name.clone(), typed(v, a.ty.as_deref()))).collect()
    };
    Choice {
        description: Some(description.trim().to_string()).filter(|d| !d.is_empty()),
        defaults: pick(How::Default),
        fixed: pick(How::Fixed),
        ..was.bare()
    }
}

/// One line of the dialog's list.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// A group's head: its title, how many of its tools are on of how many, and whether it is folded.
    Group { title: String, on: usize, of: usize, folded: bool },
    Tool { entry: Entry, here: bool, saving: bool, open: bool },
    /// The gear's form under its tool, for the form opened `serial`th.
    Form { name: String, serial: u64 },
    /// Nothing found.
    None(String),
}

/// The lines shown: the tools found, by group (folded groups show their head only, unless something is looked
/// for), with the open form under its tool.
pub fn lines(
    tools: &[Entry],
    find: &str,
    folded: &BTreeSet<String>,
    form: Option<(&str, u64)>,
    here: bool,
    saving: &BTreeSet<String>,
) -> Vec<Line> {
    let q = find.trim().to_lowercase();
    let found: Vec<&Entry> = tools
        .iter()
        .filter(|t| {
            q.is_empty()
                || t.name.to_lowercase().contains(&q)
                || t.description.to_lowercase().contains(&q)
                || t.group_title.to_lowercase().contains(&q)
        })
        .collect();
    if found.is_empty() {
        return vec![Line::None(format!("No tool has \u{201c}{}\u{201d} in its name or description.", find.trim()))];
    }
    let mut titles: Vec<&str> = Vec::new();
    for t in &found {
        if !titles.contains(&t.group_title.as_str()) {
            titles.push(&t.group_title);
        }
    }
    let mut out = Vec::new();
    for title in titles {
        let of: Vec<&&Entry> = found.iter().filter(|t| t.group_title == title).collect();
        let is_folded = q.is_empty() && folded.contains(title);
        out.push(Line::Group {
            title: title.to_string(),
            on: of.iter().filter(|t| t.enabled).count(),
            of: of.len(),
            folded: is_folded,
        });
        if is_folded {
            continue;
        }
        for t in of {
            let open = form.is_some_and(|(n, _)| n == t.name);
            out.push(Line::Tool { entry: (*t).clone(), here, saving: saving.contains(&t.name), open });
            if let Some((name, serial)) = form.filter(|_| open) {
                out.push(Line::Form { name: name.to_string(), serial });
            }
        }
    }
    out
}

/// One argument's field in the form.
#[derive(Clone)]
struct ArgField {
    arg: Arg,
    how: RwSignal<How>,
    value: RwSignal<String>,
}

/// The form open under a tool's gear, its fields in a scope of their own that goes when it closes.
#[derive(Clone)]
struct Open {
    name: String,
    serial: u64,
    scope: Scope,
    description: RwSignal<String>,
    args: Vec<ArgField>,
}

/// The dialog's own state.
#[derive(Clone, Copy)]
pub struct ToolsDialog {
    pub open: RwSignal<bool>,
    /// The tools as the session said them; none until it has.
    tools: RwSignal<Option<Vec<Entry>>>,
    /// The project's settings file, when there is one.
    project_file: RwSignal<Option<String>>,
    find: RwSignal<String>,
    /// "Only in this project": the person's settings for this project are the ones set.
    here: RwSignal<bool>,
    folded: RwSignal<BTreeSet<String>>,
    form: RwSignal<Option<Open>>,
    serial: RwSignal<u64>,
    /// The tools whose change waits for an answer.
    saving: RwSignal<BTreeSet<String>>,
}

impl crate::ext::State for ToolsDialog {
    fn new(ui: Ui) -> ToolsDialog {
        ToolsDialog {
            open: ui.scope.create_rw_signal(false),
            tools: ui.scope.create_rw_signal(None),
            project_file: ui.scope.create_rw_signal(None),
            find: ui.scope.create_rw_signal(String::new()),
            here: ui.scope.create_rw_signal(false),
            folded: ui.scope.create_rw_signal(BTreeSet::new()),
            form: ui.scope.create_rw_signal(None),
            serial: ui.scope.create_rw_signal(0),
            saving: ui.scope.create_rw_signal(BTreeSet::new()),
        }
    }
}

fn ask(send: &Send) {
    send(json!({"type": "tool_settings"}));
}

/// Opens the dialog, and asks for the tools.
pub fn open(ui: Ui, send: &Send) {
    ask(send);
    let st = ui.state::<ToolsDialog>();
    if !st.open.get_untracked() {
        st.open.set(true);
    }
}

fn close(st: ToolsDialog) {
    close_form(st);
    if st.open.get_untracked() {
        st.open.set(false);
    }
}

fn close_form(st: ToolsDialog) {
    if let Some(f) = st.form.get_untracked() {
        st.form.set(None);
        f.scope.dispose();
    }
}

/// Opens the gear's form of a tool, with its settings at the level set; again on the same tool, closes it.
fn toggle_form(ui: Ui, name: &str, again: bool) {
    let st = ui.state::<ToolsDialog>();
    let was = st.form.with_untracked(|f| f.as_ref().map(|f| f.name.clone()));
    close_form(st);
    if was.as_deref() == Some(name) && !again {
        return;
    }
    let here = st.here.get_untracked();
    let Some(entry) = st.tools.with_untracked(|t| t.as_ref().and_then(|t| t.iter().find(|e| e.name == name).cloned()))
    else {
        return;
    };
    let c = entry.at(here).clone();
    let scope = ui.scope.create_child();
    let args = entry
        .arguments
        .iter()
        .map(|a| {
            let (how, value) = match (c.fixed.get(&a.name), c.defaults.get(&a.name)) {
                (Some(v), _) => (How::Fixed, shown(v)),
                (None, Some(v)) => (How::Default, shown(v)),
                _ => (How::Model, String::new()),
            };
            ArgField { arg: a.clone(), how: scope.create_rw_signal(how), value: scope.create_rw_signal(value) }
        })
        .collect();
    let serial = st.serial.get_untracked() + 1;
    st.serial.set(serial);
    st.form.set(Some(Open {
        name: name.to_string(),
        serial,
        scope,
        description: scope.create_rw_signal(c.description.clone().unwrap_or_default()),
        args,
    }));
}

/// Sends a tool's settings at the level set.
fn save(ui: Ui, send: &Send, name: &str, choice: Choice) {
    let st = ui.state::<ToolsDialog>();
    let level = if st.here.get_untracked() { "here" } else { "person" };
    st.saving.update(|s| {
        s.insert(name.to_string());
    });
    send(json!({"type": "tool_settings_set", "name": name, "level": level, "settings": choice.to_value(), "key": name}));
}

/// What it adds to the window's registries, once at start: the dialog's layer and its command (in place of the
/// tools picker's stand-in).
pub fn register(_ui: Ui, _send: &Send) {
    layers::add(Place::Over, "tools.configure", 30, layer);
    commands::add(commands::command(CUSTOMIZE, "Agent", "Configure Tools…", open));
}

/// The session's messages that are this part's, taken in. Never taken away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = ui.state::<ToolsDialog>();
    match m["type"].as_str().unwrap_or("") {
        "state" | "turn_end" if st.open.get_untracked() => ask(send),
        "tool_settings" => {
            let tools: Vec<Entry> = m["tools"].as_array().into_iter().flatten().filter_map(Entry::of).collect();
            if st.tools.with_untracked(|was| was.as_ref() != Some(&tools)) {
                st.tools.set(Some(tools));
            }
            let file = m["project_file"].as_str().map(String::from);
            if st.project_file.with_untracked(|was| *was != file) {
                st.project_file.set(file);
            }
            if let Some(n) = m["name"].as_str() {
                done(st, n);
            }
        }
        "refused" if m["of"] == "tool_settings_set" => {
            if let Some(n) = m["key"].as_str() {
                done(st, n);
            }
            if let Some(why) = m["reason"].as_str() {
                toast::error(ui, why);
            }
        }
        "refused" if m["of"] == "tool_enable" => {
            if let Some(why) = m["reason"].as_str() {
                toast::error(ui, why);
            }
        }
        _ => {}
    }
    false
}

fn done(st: ToolsDialog, name: &str) {
    if st.saving.with_untracked(|s| s.contains(name)) {
        st.saving.update(|s| {
            s.remove(name);
        });
    }
}

/// A switch: lit while on; dimmed, and a press does nothing, while it may not be changed.
fn switch(t: Theme, on: bool, able: bool, flip: impl Fn() + 'static) -> impl IntoView {
    let knob = empty().style(move |s| {
        let c = t.get();
        let knob = if on { c.button_fg } else { c.icon };
        s.width(10.0).height(10.0).border_radius(5.0).background(if able { knob } else { mix(knob, c.widget, 0.5) })
    });
    container(knob)
        .on_click_stop(move |_| {
            if able {
                flip();
            }
        })
        .style(move |s| {
            let c = t.get();
            let dim = |colour| if able { colour } else { mix(colour, c.widget, 0.5) };
            s.width(28.0)
                .height(16.0)
                .padding_horiz(3.0)
                .items_center()
                .border(1.0)
                .border_radius(8.0)
                .border_color(dim(if on { c.button_bg } else { c.input_border }))
                .background(dim(if on { c.button_bg } else { c.input_bg }))
                .apply_if(on, |s| s.justify_end())
                .flex_shrink(0.0_f32)
                .apply_if(able, |s| s.cursor(CursorStyle::Pointer))
        })
}

/// A dropdown as VS Code draws one: the choice in force and a chevron; a press opens the choices.
fn dropdown(t: Theme, words: String, width: f64, choices: impl Fn() -> Vec<popup::Item> + 'static, ui: Ui) -> AnyView {
    let view = h_stack((
        label(move || words.clone()).style(|s| s.flex_grow(1.0_f32).min_width(0.0).text_ellipsis()),
        icon("chevron-down", 14.0).style(move |s| s.color(t.get().icon).flex_shrink(0.0_f32)),
    ));
    let id = view.id();
    view.on_click_stop(move |_| popup::open(ui, At::under(id), choices()))
        .style(move |s| {
            let c = t.get();
            s.width(width)
                .height(24.0)
                .gap(4.0)
                .padding_horiz(6.0)
                .items_center()
                .flex_shrink(0.0_f32)
                .font_size(12.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(c.input_border)
                .background(c.input_bg)
                .color(c.input_fg)
                .cursor(CursorStyle::Pointer)
        })
        .into_any()
}

/// A one-line input of the dialog's.
fn input(ui: Ui, text: RwSignal<String>, hint: String, mono: bool) -> AnyView {
    let t = ui.colors;
    let focused = floem::reactive::create_rw_signal(false);
    text_input(text)
        .placeholder(hint)
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| focused.set(false))
        .on_event_stop(EventListener::PointerDown, |_| {})
        .takes_keys(Focus::Input)
        .style(move |s| {
            let c = t.get();
            let s = s
                .width_full()
                .min_width(0.0)
                .height(24.0)
                .padding_horiz(6.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(if focused.get() { c.focus } else { c.input_border })
                .background(c.input_bg)
                .color(c.input_fg)
                .cursor_color(Brush::Solid(c.cursor))
                .apply_if(mono, |s| s.font_family(MONO_FONT.clone()).font_size(12.0));
            crate::widgets::steady_input(s, c.input_bg, Some(c.focus))
        })
        .into_any()
}

/// A small square button with a codicon.
fn small(t: Theme, name: &'static str, lit: bool, on: impl Fn() + 'static) -> impl IntoView {
    container(icon(name, 14.0)).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        s.width(22.0)
            .height(22.0)
            .items_center()
            .justify_center()
            .border_radius(4.0)
            .flex_shrink(0.0_f32)
            .color(c.icon)
            .cursor(CursorStyle::Pointer)
            .apply_if(lit, |s| s.background(c.toolbar_hover))
            .hover(|s| s.background(c.toolbar_hover))
    })
}

/// A group's head: a press folds it or opens it.
fn group_head(ui: Ui, title: String, on: usize, of: usize, folded: bool) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<ToolsDialog>();
    let key = title.clone();
    h_stack((
        icon(if folded { "chevron-right" } else { "chevron-down" }, 16.0).style(move |s| s.color(t.get().icon)),
        label(move || title.clone()).style(|s| s.font_weight(*crate::widgets::SEMIBOLD).text_ellipsis()),
        label(move || format!("{on} of {of} on")).style(move |s| s.font_size(12.0).color(t.get().desc)),
    ))
    .on_click_stop(move |_| {
        st.folded.update(|f| {
            if !f.remove(&key) {
                f.insert(key.clone());
            }
        })
    })
    .style(|s| s.gap(6.0).items_center().width_full().padding_top(10.0).padding_bottom(4.0).cursor(CursorStyle::Pointer))
    .into_any()
}

/// A tool's row: its switch, its name and what it does, how it asks, and its gear.
fn tool_row(ui: Ui, send: Send, e: Entry, here: bool, saving: bool, open: bool) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<ToolsDialog>();
    let choice = e.at(here).clone();
    let by_project = e.off_by_project();
    let (name, flip_name) = (e.name.clone(), e.name.clone());
    let flip_send = send.clone();
    let on = e.enabled;
    let flip_choice = choice.clone();
    let switched = tooltip(
        switch(t, on, !by_project && !saving, move || {
            save(ui, &flip_send, &flip_name, flip_choice.switched(!on));
        }),
        move || label(move || if on { "Turn off" } else { "Turn on" }),
    );
    let tip = e.tip();
    let file = st.project_file.get_untracked().unwrap_or_else(|| ".code-arena/settings.json".into());
    let second = if by_project { format!("Turned off by the project ({file}).") } else { e.description.clone() };
    let shown_name = name.clone();
    let words = v_stack((
        tooltip(
            label(move || shown_name.clone()).style(|s| s.font_family(MONO_FONT.clone()).text_ellipsis()),
            move || label({
                let tip = tip.clone();
                move || tip.clone()
            }),
        ),
        label(move || second.clone()).style(move |s| s.font_size(12.0).color(t.get().desc).text_ellipsis()),
    ))
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).gap(1.0));
    let mut head: Vec<AnyView> = vec![switched.into_any(), words.into_any()];
    if e.asks_chosen() {
        let chosen = choice.approval.clone().unwrap_or_else(|| "default".into());
        let words = ASKS.iter().find(|(v, _)| *v == chosen).map(|(_, w)| *w).unwrap_or("As the mode says");
        let (locked, menu_send, menu_name, menu_choice) = (e.locked.is_some(), send.clone(), name.clone(), choice.clone());
        let choices = move || {
            ASKS.iter()
                .map(|(v, w)| {
                    let (send, name, c, v) = (menu_send.clone(), menu_name.clone(), menu_choice.clone(), *v);
                    popup::entry(w, move || save(ui, &send, &name, c.asking(v)))
                        .radio(v == chosen)
                        .enabled(!saving && !(v == "allow" && locked))
                        .into()
                })
                .collect()
        };
        head.push(dropdown(t, words.into(), 132.0, choices, ui));
    }
    let gear_name = name.clone();
    head.push(
        tooltip(small(t, "settings-gear", open, move || toggle_form(ui, &gear_name, false)), || {
            label(|| "Advanced: what the model is told of it, its arguments")
        })
        .into_any(),
    );
    let head = h_stack_from_iter(head).style(|s| s.gap(8.0).items_center().width_full());
    let mut parts = vec![head.into_any()];
    if let Some(why) = e.locked.clone().filter(|_| choice.approval.as_deref() != Some("ask")) {
        parts.push(
            label(move || why.clone())
                .style(move |s| s.margin_left(36.0).font_size(12.0).color(t.get().desc).text_overflow(TextOverflow::Wrap))
                .into_any(),
        );
    }
    v_stack_from_iter(parts)
        .style(move |s| {
            let c = t.get();
            s.width_full()
                .gap(4.0)
                .padding_horiz(10.0)
                .padding_vert(6.0)
                .border_bottom(1.0)
                .border_color(c.widget_border)
                .hover(|s| s.background(c.list_hover))
        })
        .into_any()
}

/// The gear's form: what the model is told of the tool, how each argument is given, Save and Reset.
fn form_view(ui: Ui, send: Send) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<ToolsDialog>();
    let Some(f) = st.form.get_untracked() else { return empty().into_any() };
    let here = st.here.get_untracked();
    let Some(entry) = st.tools.with_untracked(|t| t.as_ref().and_then(|t| t.iter().find(|e| e.name == f.name).cloned()))
    else {
        return empty().into_any();
    };
    let note = |text: String| {
        label(move || text.clone())
            .style(move |s| s.font_size(12.0).line_height(1.4).color(t.get().desc).text_overflow(TextOverflow::Wrap))
            .into_any()
    };
    let mut parts: Vec<AnyView> = Vec::new();
    if entry.project.approval.as_deref() == Some("ask") {
        parts.push(note("The project makes it ask before each call, whatever is chosen here.".into()));
    }
    parts.push(label(|| "What the model is told of it").style(|s| s.font_size(12.0).font_weight(*crate::widgets::SEMIBOLD)).into_any());
    parts.push(input(ui, f.description, entry.description.clone(), false));
    parts.push(note("Empty: its own words (shown greyed).".into()));
    if !entry.arguments.is_empty() {
        parts.push(
            label(|| "Its arguments").style(|s| s.margin_top(6.0).font_size(12.0).font_weight(*crate::widgets::SEMIBOLD)).into_any(),
        );
        for a in &f.args {
            let (how, value) = (a.how, a.value);
            let shown_name = match &a.arg.ty {
                Some(ty) => format!("{} · {ty}", a.arg.name),
                None => a.arg.name.clone(),
            };
            let about = a.arg.about.clone().unwrap_or_default();
            let named = tooltip(
                label(move || shown_name.clone())
                    .style(|s| s.font_family(MONO_FONT.clone()).font_size(12.0).text_ellipsis()),
                move || label({
                    let about = about.clone();
                    move || about.clone()
                }),
            )
            .style(|s| s.width(160.0).flex_shrink(0.0_f32));
            let pick = dyn_container(
                move || how.get(),
                move |h| {
                    let choices = move || {
                        [How::Model, How::Default, How::Fixed]
                            .into_iter()
                            .map(|v| {
                                popup::entry(v.words(), move || {
                                    if how.get_untracked() != v {
                                        how.set(v);
                                    }
                                })
                                .radio(v == h)
                                .into()
                            })
                            .collect()
                    };
                    dropdown(t, h.words().into(), 144.0, choices, ui)
                },
            );
            let field = container(input(ui, value, String::new(), true))
                .style(move |s| s.flex_grow(1.0_f32).min_width(0.0).apply_if(how.get() == How::Model, |s| s.hide()));
            parts.push(
                h_stack((named, pick, field)).style(|s| s.gap(8.0).items_center().width_full()).into_any(),
            );
        }
        parts.push(note("A fixed argument is not offered to the model; a default fills one it leaves out.".into()));
    }
    let was = entry.at(here).clone();
    let (save_send, reset_send, name, reset_name, args, description) =
        (send.clone(), send, f.name.clone(), f.name.clone(), f.args.clone(), f.description);
    let save_was = was.clone();
    let buttons = h_stack((
        button(t, "Save", true, move || {
            let given: Vec<(Arg, How, String)> =
                args.iter().map(|a| (a.arg.clone(), a.how.get_untracked(), a.value.get_untracked())).collect();
            save(ui, &save_send, &name, formed(&save_was, &description.get_untracked(), &given));
        }),
        button(t, "Reset", false, move || {
            save(ui, &reset_send, &reset_name, was.bare());
            toggle_form(ui, &reset_name, true);
        }),
    ))
    .style(|s| s.gap(8.0).margin_top(6.0));
    parts.push(buttons.into_any());
    v_stack_from_iter(parts)
        .style(move |s| {
            let c = t.get();
            s.width_full()
                .gap(6.0)
                .padding(12.0)
                .border_bottom(1.0)
                .border_color(c.widget_border)
                .background(mix(c.widget, c.input_bg, 0.5))
        })
        .into_any()
}

/// The list of lines, each drawn anew only when what it shows changes.
fn list(ui: Ui, send: Send) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<ToolsDialog>();
    let shown = create_memo(move |_| {
        let form = st.form.with(|f| f.as_ref().map(|f| (f.name.clone(), f.serial)));
        st.tools.with(|tools| {
            tools.as_ref().map(|tools| {
                st.find.with(|find| {
                    st.folded.with(|folded| {
                        st.saving.with(|saving| {
                            lines(tools, find, folded, form.as_ref().map(|(n, s)| (n.as_str(), *s)), st.here.get(), saving)
                        })
                    })
                })
            })
        })
    });
    let loading = label(|| "Reading the tools…")
        .style(move |s| s.color(t.get().desc).apply_if(shown.with(Option::is_some), |s| s.hide()));
    let rows = dyn_stack(
        move || shown.get().unwrap_or_default(),
        |l: &Line| format!("{l:?}"),
        move |l| match l {
            Line::Group { title, on, of, folded } => group_head(ui, title, on, of, folded),
            Line::Tool { entry, here, saving, open } => tool_row(ui, send.clone(), entry, here, saving, open),
            Line::Form { .. } => form_view(ui, send.clone()),
            Line::None(text) => label(move || text.clone()).style(move |s| s.color(t.get().desc).padding_vert(8.0)).into_any(),
        },
    )
    .style(|s| s.flex_col().width_full());
    v_stack((loading, scroll(rows).style(move |s| s.width_full().height(ui.window.get().1 * 0.55))))
        .style(|s| s.width_full())
        .into_any()
}

/// The dialog's box.
fn dialog(ui: Ui, send: Send) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<ToolsDialog>();
    let find = h_stack((
        icon("search", 14.0).style(move |s| s.color(t.get().icon).flex_shrink(0.0_f32)),
        input(ui, st.find, "Find a tool".into(), false),
    ))
    .style(|s| s.gap(6.0).items_center().flex_grow(1.0_f32).min_width(0.0));
    let level = dyn_container(
        move || st.here.get(),
        move |here| {
            switch(t, here, true, move || {
                st.here.set(!here);
                let open = st.form.with_untracked(|f| f.as_ref().map(|f| f.name.clone()));
                if let Some(n) = open {
                    toggle_form(ui, &n, true);
                }
            })
            .into_any()
        },
    );
    let level = h_stack((
        level,
        label(|| "Only in this project").style(move |s| s.font_size(12.0).color(t.get().desc).selectable(false)),
    ))
    .style(|s| s.gap(8.0).items_center().flex_shrink(0.0_f32));
    let foot = h_stack((empty().style(|s| s.flex_grow(1.0_f32)), button(t, "Close", true, move || close(st))))
        .style(|s| s.width_full());
    v_stack((
        label(|| "Configure Tools").style(|s| s.font_size(15.0).font_weight(*crate::widgets::SEMIBOLD)),
        label(|| "Which tools the agent may call, how each asks you first, and what the model is told of it.")
            .style(move |s| s.font_size(12.0).color(t.get().desc).text_overflow(TextOverflow::Wrap)),
        h_stack((find, level)).style(|s| s.gap(16.0).items_center().width_full()),
        list(ui, send),
        foot,
    ))
    .on_event_stop(EventListener::PointerDown, |_| {})
    .style(move |s| {
        let c = t.get();
        s.width(760.0_f64.min(ui.window.get().0 - 40.0).max(240.0))
            .gap(12.0)
            .padding(20.0)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(6.0)
            .background(c.widget)
            .color(c.fg)
            .font_size(13.0)
            .box_shadow_blur(16.0)
            .box_shadow_color(c.shadow)
    })
    .into_any()
}

/// The dialog's layer: the window dimmed under it while open; Escape or a press outside closes it.
fn layer(ui: Ui, send: Send) -> AnyView {
    let st = ui.state::<ToolsDialog>();
    let shown = create_memo(move |_| st.open.get());
    let body = dyn_container(
        move || shown.get(),
        move |open| if open { dialog(ui, send.clone()) } else { empty().into_any() },
    );
    let layer = container(body).keyboard_navigable();
    let id = layer.id();
    create_effect(move |was: Option<bool>| {
        let open = st.open.get();
        if open && was != Some(true) {
            id.request_focus();
        }
        open
    });
    layer
        .on_event_stop(EventListener::PointerDown, move |_| close(st))
        .on_event_cont(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(k) = e
                && k.key.logical_key == Key::Named(NamedKey::Escape)
            {
                close(st);
            }
        })
        .style(move |s| {
            s.absolute()
                .inset(0.0)
                .items_center()
                .justify_center()
                .background(floem::peniko::Color::rgba8(0, 0, 0, 0x4D))
                .apply_if(!st.open.get(), |s| s.hide())
        })
        .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, group: &str, enabled: bool) -> Entry {
        Entry { name: name.into(), group_title: group.into(), enabled, kind: "edit".into(), ..Entry::default() }
    }

    #[test]
    fn a_tool_is_read_as_the_session_says_it() {
        let e = Entry::of(&json!({
            "name": "run_shell", "group": "own", "group_title": "Code Arena's own", "kind": "shell",
            "description": "Runs a command.", "arguments": [{"name": "command", "type": "string", "description": "It."}],
            "enabled": true, "asks": "mode", "person": {"approval": "ask"}, "here": {}, "project": {"enabled": false},
            "locked": null
        }))
        .expect("entry");
        assert_eq!(e.arguments, [Arg { name: "command".into(), ty: Some("string".into()), about: Some("It.".into()) }]);
        assert_eq!(e.at(false).approval.as_deref(), Some("ask"));
        assert!(e.at(true).approval.is_none());
        assert!(e.off_by_project());
        assert_eq!(e.tip(), "runs commands · Code Arena's own · Asks as the mode says");
        assert!(Entry::of(&json!({})).is_none());
    }

    #[test]
    fn tools_are_found_by_name_description_or_group_and_counted_by_group() {
        let mut a = entry("read_file", "Own", true);
        a.description = "Reads a file.".into();
        let tools = vec![a, entry("write_file", "Own", false), entry("lookup", "srv", true)];
        let none = BTreeSet::new();
        let all = lines(&tools, "", &none, None, false, &none);
        assert_eq!(all.len(), 5);
        assert_eq!(all[0], Line::Group { title: "Own".into(), on: 1, of: 2, folded: false });
        let found = lines(&tools, "READS", &none, None, false, &none);
        assert_eq!(found.len(), 2);
        assert_eq!(lines(&tools, "srv", &none, None, false, &none).len(), 2);
        assert_eq!(lines(&tools, "zzz", &none, None, false, &none), [Line::None("No tool has \u{201c}zzz\u{201d} in its name or description.".into())]);
    }

    #[test]
    fn a_folded_group_shows_its_head_unless_something_is_looked_for() {
        let tools = vec![entry("a", "G", true), entry("b", "G", true)];
        let folded: BTreeSet<String> = ["G".to_string()].into();
        let none = BTreeSet::new();
        assert_eq!(lines(&tools, "", &folded, None, false, &none), [Line::Group { title: "G".into(), on: 2, of: 2, folded: true }]);
        assert_eq!(lines(&tools, "a", &folded, None, false, &none).len(), 2);
    }

    #[test]
    fn the_open_form_is_under_its_tool() {
        let tools = vec![entry("a", "G", true), entry("b", "G", true)];
        let saving: BTreeSet<String> = ["b".to_string()].into();
        let l = lines(&tools, "", &BTreeSet::new(), Some(("a", 3)), true, &saving);
        assert!(matches!(&l[1], Line::Tool { open: true, here: true, saving: false, .. }));
        assert_eq!(l[2], Line::Form { name: "a".into(), serial: 3 });
        assert!(matches!(&l[3], Line::Tool { open: false, saving: true, .. }));
    }

    #[test]
    fn the_switch_and_the_asking_keep_the_rest() {
        let c = Choice { description: Some("D.".into()), approval: Some("ask".into()), ..Choice::default() };
        assert_eq!(c.switched(false).to_value(), json!({"enabled": false, "approval": "ask", "description": "D."}));
        assert_eq!(c.switched(false).switched(true), c);
        assert_eq!(c.asking("default").to_value(), json!({"description": "D."}));
        assert_eq!(c.asking("allow").approval.as_deref(), Some("allow"));
    }

    #[test]
    fn the_form_gives_typed_defaults_and_fixed_arguments() {
        let was = Choice { enabled: Some(false), approval: Some("ask".into()), description: Some("Old.".into()), ..Choice::default() };
        let arg = |n: &str, ty: &str| Arg { name: n.into(), ty: Some(ty.into()), about: None };
        let given = vec![
            (arg("times", "integer"), How::Default, "3".to_string()),
            (arg("loud", "boolean"), How::Fixed, "true".to_string()),
            (arg("text", "string"), How::Model, "ignored".to_string()),
        ];
        let c = formed(&was, "  ", &given);
        assert_eq!(c.to_value(), json!({"enabled": false, "approval": "ask", "defaults": {"times": 3}, "fixed": {"loud": true}}));
        assert_eq!(formed(&was, " New. ", &[]).description.as_deref(), Some("New."));
        assert_eq!(was.bare().to_value(), json!({"enabled": false, "approval": "ask"}));
    }

    #[test]
    fn typed_values_take_their_arguments_type() {
        assert_eq!(typed(" 5 ", Some("integer")), json!(5));
        assert_eq!(typed("2.5", Some("number")), json!(2.5));
        assert_eq!(typed("five", Some("number")), json!("five"));
        assert_eq!(typed("false", Some("boolean")), json!(false));
        assert_eq!(typed("[1, 2]", Some("array")), json!([1, 2]));
        assert_eq!(typed("{bad", Some("object")), json!("{bad"));
        assert_eq!(typed(" as is ", None), json!(" as is "));
        assert_eq!(shown(&json!("a")), "a");
        assert_eq!(shown(&json!({"k": 1})), "{\"k\":1}");
    }
}
