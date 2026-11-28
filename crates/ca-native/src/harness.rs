//! The Harness view: how the agent is run, in five sections that fold as the Explorer's do.
//!
//! - Hooks: the hooks that run (their event, their command, where each was declared). While the project has hooks
//!   of its own that are not trusted, they are listed too, dimmed, with Trust Project Hooks.
//! - Permissions: the rules allowed without asking and refused in every mode. A rule is added for this session
//!   (Allow or Deny), or taken away; settings.json keeps one for good.
//! - Sandbox: what commands in the sandbox may do, or that there is none.
//! - Plugins: the plugins installed, each turned on or off for you, or removed; Install Plugin… takes a folder or a
//!   git URL.
//! - Output Style: how the agent talks; the one picked is in force from the next message.
//!
//! Run Doctor (the view's title) opens what `code-arena doctor` finds in an editor tab. The session's side is the
//! driver's `ide_harness.rs`; what is said back comes as a corner bubble.

use floem::IntoView;
use floem::event::EventListener;
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{
    Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, text_input, v_stack,
};
use serde_json::{Value, json};

use crate::editor::kinds;
use crate::icons::icon;
use crate::state::{Send, Tab, Ui};
use crate::widgets::{MONO_FONT, Theme, action, bold, button, caps, dialog, quick, toast};

/// Whether the view is in the window.
pub const READY: bool = true;

/// The command that asks for everything again.
const REFRESH: &str = "codeArena.harness.refresh";
/// The command that opens Doctor in a tab.
pub const DOCTOR: &str = "codeArena.harness.doctor";
/// The command that installs a plugin.
pub const INSTALL: &str = "codeArena.harness.installPlugin";
/// The command that trusts the project's hooks.
pub const TRUST: &str = "codeArena.harness.trustProjectHooks";
/// The tab Doctor is shown in.
const DOCTOR_TAB: &str = "harness:doctor";

/// The messages this view asks for, each time it asks again.
const ASKED: [&str; 5] = ["hooks", "permissions", "sandbox", "plugins", "output_styles"];
/// The requests whose notices are this view's.
const OURS: [&str; 6] =
    ["hooks_trust", "permission_set", "plugin_set", "plugin_install", "plugin_remove", "output_style_set"];

/// A hook, as the session listed it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Hook {
    pub event: String,
    pub command: String,
    pub from: String,
}

/// What the session said of its hooks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hooks {
    pub items: Vec<Hook>,
    /// Whether the project's own hooks run; none: the project has none.
    pub trusted: Option<bool>,
    /// The project's hooks that do not run until trusted.
    pub waiting: Vec<Hook>,
}

/// A permission rule: allowed without asking, or refused in every mode.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Perm {
    pub rule: String,
    pub allow: bool,
}

/// An installed plugin.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub description: String,
    pub parts: Vec<String>,
    /// Enabled in the settings in force.
    pub on: bool,
    /// In your own `plugins.enabled`.
    pub yours: bool,
}

/// An output style.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Style {
    pub name: String,
    pub description: String,
    pub from: String,
    pub current: bool,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn hooks_in(v: &Value) -> Vec<Hook> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|h| Hook { event: text(&h["event"]), command: text(&h["command"]), from: text(&h["from"]) })
        .collect()
}

/// A `hooks` message read.
pub fn hooks_of(m: &Value) -> Hooks {
    Hooks { items: hooks_in(&m["items"]), trusted: m["trusted"].as_bool(), waiting: hooks_in(&m["waiting"]) }
}

/// A `permissions` message read.
pub fn perms_of(m: &Value) -> Vec<Perm> {
    m["rules"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| Perm { rule: text(&r["rule"]), allow: r["allow"].as_bool().unwrap_or(false) })
        .collect()
}

/// A `sandbox` message read: whether there is one, and what it lets commands do.
pub fn sandbox_of(m: &Value) -> (bool, Vec<String>) {
    (m["on"].as_bool().unwrap_or(false), m["text"].as_array().into_iter().flatten().map(text).collect())
}

/// A `plugins` message read.
pub fn plugins_of(m: &Value) -> Vec<Plugin> {
    m["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| Plugin {
            name: text(&p["name"]),
            version: text(&p["version"]),
            description: text(&p["description"]),
            parts: p["parts"].as_array().into_iter().flatten().map(text).collect(),
            on: p["on"].as_bool().unwrap_or(false),
            yours: p["yours"].as_bool().unwrap_or(false),
        })
        .collect()
}

/// An `output_styles` message read, the one in force marked.
pub fn styles_of(m: &Value) -> Vec<Style> {
    let current = m["current"].as_str().unwrap_or("default");
    m["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let name = text(&s["name"]);
            Style {
                current: name.eq_ignore_ascii_case(current),
                name,
                description: text(&s["description"]),
                from: text(&s["from"]),
            }
        })
        .collect()
}

/// What a plugin's row says under its name: its version and what it holds.
pub fn plugin_detail(p: &Plugin) -> String {
    let holds = if p.parts.is_empty() { "nothing Code Arena reads".to_string() } else { p.parts.join(", ") };
    let mut out = if p.version.is_empty() { holds } else { format!("v{} \u{b7} {holds}", p.version) };
    if p.on && !p.yours {
        out.push_str(" \u{b7} enabled by the project");
    }
    out
}

/// What is wrong with a rule as typed; none: it may be sent. A tool's name, with a pattern in brackets.
pub fn rule_problem(rule: &str) -> Option<&'static str> {
    let rule = rule.trim();
    let (tool, rest) = match rule.split_once('(') {
        Some((t, r)) => (t, Some(r)),
        None => (rule, None),
    };
    if tool.is_empty() || !tool.chars().all(|c| c.is_ascii_alphanumeric() || "_-*.".contains(c)) {
        return Some("A rule is a tool's name, with a pattern if you like: run_shell(npm test*), read_file(.env*).");
    }
    match rest {
        Some(r) if !r.ends_with(')') || r.len() < 2 => Some("The pattern ends with ')': run_shell(npm test*)."),
        _ => None,
    }
}

/// What is wrong with a plugin's source as typed; none: it may be sent.
pub fn source_problem(source: &str) -> Option<&'static str> {
    if source.trim().is_empty() { Some("A folder (/path/to/plugin) or a git URL (https://…).") } else { None }
}

/// A section's place among the five.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Part {
    Hooks,
    Permissions,
    Sandbox,
    Plugins,
    Styles,
}

impl Part {
    const ALL: [Part; 5] = [Part::Hooks, Part::Permissions, Part::Sandbox, Part::Plugins, Part::Styles];

    fn title(self) -> &'static str {
        match self {
            Part::Hooks => "Hooks",
            Part::Permissions => "Permissions",
            Part::Sandbox => "Sandbox",
            Part::Plugins => "Plugins",
            Part::Styles => "Output Style",
        }
    }
}

/// The view's state in a window.
#[derive(Clone, Copy)]
pub struct HarnessState {
    pub hooks: RwSignal<Option<Hooks>>,
    pub perms: RwSignal<Option<Vec<Perm>>>,
    pub sandbox: RwSignal<Option<(bool, Vec<String>)>>,
    pub plugins: RwSignal<Option<Vec<Plugin>>>,
    pub styles: RwSignal<Option<Vec<Style>>>,
    /// What Doctor found; none: it is running, or was not asked.
    pub doctor: RwSignal<Option<String>>,
    /// The rule being typed.
    pub draft: RwSignal<String>,
    /// The sections folded, by their place.
    pub folded: RwSignal<[bool; 5]>,
}

impl crate::ext::State for HarnessState {
    fn new(ui: Ui) -> Self {
        HarnessState {
            hooks: ui.scope.create_rw_signal(None),
            perms: ui.scope.create_rw_signal(None),
            sandbox: ui.scope.create_rw_signal(None),
            plugins: ui.scope.create_rw_signal(None),
            styles: ui.scope.create_rw_signal(None),
            doctor: ui.scope.create_rw_signal(None),
            draft: ui.scope.create_rw_signal(String::new()),
            folded: ui.scope.create_rw_signal([false; 5]),
        }
    }
}

/// The view's state.
pub fn state(ui: Ui) -> HarnessState {
    ui.state::<HarnessState>()
}

/// Sets a signal only when the value is another.
fn put<T: PartialEq + 'static>(s: RwSignal<Option<T>>, v: T) {
    if s.with_untracked(|o| o.as_ref() != Some(&v)) {
        s.set(Some(v));
    }
}

/// Asks for every section again.
fn ask_all(send: &Send) {
    for k in ASKED {
        send(json!({"type": k}));
    }
}

/// Sends the rule typed, allowed or refused.
fn add_rule(ui: Ui, send: &Send, allow: bool) {
    let st = state(ui);
    let rule = st.draft.get_untracked().trim().to_string();
    if let Some(why) = rule_problem(&rule) {
        toast::info(ui, why);
        return;
    }
    send(json!({"type": "permission_set", "rule": rule, "allow": allow}));
    st.draft.set(String::new());
}

/// Install Plugin…: a folder or a git URL.
pub fn install(ui: Ui, send: &Send) {
    let send = send.clone();
    quick::input("Install Plugin")
        .placeholder("A folder or a git URL")
        .prompt("The plugin's folder (/path/to/plugin) or git repository (https://…, git@…)")
        .validate(|s| source_problem(s).map(str::to_string))
        .open(ui, move |got| {
            if let Some(source) = got {
                toast::info(ui, &format!("Installing {}\u{2026}", source.trim()));
                send(json!({"type": "plugin_install", "source": source.trim()}));
            }
        });
}

/// Trust Project Hooks, once the person said so.
pub fn trust(ui: Ui, send: &Send) {
    let send = send.clone();
    let n = state(ui).hooks.with_untracked(|h| h.as_ref().map_or(0, |h| h.waiting.len()));
    let detail = format!(
        "The project declares {n} hook{} that run{} commands on your machine. They run from the next message; a \
         change to them asks again.",
        if n == 1 { "" } else { "s" },
        if n == 1 { "s" } else { "" }
    );
    dialog::ask("Trust the project's hooks?").detail(&detail).kind(dialog::Kind::Warning).button("Trust").open(
        ui,
        move |a| {
            if a.yes() {
                send(json!({"type": "hooks_trust"}));
            }
        },
    );
}

/// Remove, once the person said so.
fn remove_plugin(ui: Ui, send: &Send, name: &str) {
    let (send, name) = (send.clone(), name.to_string());
    dialog::ask(&format!("Remove the plugin {name}?"))
        .detail("Its folder is deleted from Code Arena's data. A new session goes without it.")
        .kind(dialog::Kind::Warning)
        .button("Remove")
        .destructive()
        .open(ui, move |a| {
            if a.yes() {
                send(json!({"type": "plugin_remove", "name": name}));
            }
        });
}

/// Doctor in its tab, run again.
pub fn doctor(ui: Ui, send: &Send) {
    state(ui).doctor.set(None);
    send(json!({"type": "doctor"}));
    kinds::open(ui, DOCTOR_TAB);
}

/// Wrapped words, in the description's colour.
fn desc(t: Theme, words: impl Fn() -> String + 'static) -> impl IntoView {
    label(words).style(move |s| {
        s.font_size(12.0)
            .line_height(1.4)
            .color(t.get().desc)
            .text_overflow(TextOverflow::Wrap)
            .min_width(0.0)
            .max_width_full()
    })
}

/// A line of words under a section's title.
fn note(t: Theme, words: String) -> floem::AnyView {
    container(desc(t, move || words.clone()))
        .style(|s| s.padding_left(20.0).padding_right(12.0).padding_vert(3.0).width_full().min_width(0.0))
        .into_any()
}

/// A checkbox drawn as VS Code's: 18 px, ticked while `on`.
fn checkbox(t: Theme, on: bool) -> impl IntoView {
    container(icon("check", 14.0).style(move |s| s.apply_if(!on, |s| s.hide()))).style(move |s| {
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
    })
}

/// A row's two lines: its name and what is said of it.
fn two_lines(t: Theme, top: String, under: String, dim: bool) -> impl IntoView {
    let no_under = under.is_empty();
    v_stack((
        label(move || top.clone()).style(move |s| {
            let c = t.get();
            s.font_size(13.0)
                .line_height(1.4)
                .color(if dim { c.desc } else { c.fg })
                .text_overflow(TextOverflow::Wrap)
                .width_full()
                .max_width_full()
                .min_width(0.0)
        }),
        label(move || under.clone()).style(move |s| {
            s.font_size(12.0)
                .line_height(1.4)
                .color(t.get().desc)
                .text_overflow(TextOverflow::Wrap)
                .width_full()
                .max_width_full()
                .min_width(0.0)
                .apply_if(no_under, |s| s.hide())
        }),
    ))
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0))
}

/// A row with an action at its end shown while the pointer is on it.
fn row(
    t: Theme,
    lead: impl IntoView + 'static,
    body: impl IntoView + 'static,
    end: Option<floem::AnyView>,
) -> floem::AnyView {
    let over = create_rw_signal(false);
    let end = match end {
        Some(e) => container(e).style(move |s| s.flex_shrink(0.0_f32).apply_if(!over.get(), |s| s.hide())).into_any(),
        None => empty().into_any(),
    };
    h_stack((lead, body, end))
        .on_event_cont(EventListener::PointerEnter, move |_| over.set(true))
        .on_event_cont(EventListener::PointerLeave, move |_| over.set(false))
        .style(move |s| {
            s.gap(6.0)
                .width_full()
                .min_width(0.0)
                .min_height(22.0)
                .padding_left(20.0)
                .padding_right(8.0)
                .padding_vert(3.0)
                .items_start()
                .hover(|s| s.background(t.get().list_hover))
        })
        .into_any()
}

/// A codicon at a row's start.
fn lead(t: Theme, name: &'static str, warn: bool) -> impl IntoView {
    icon(name, 16.0).style(move |s| {
        let c = t.get();
        s.color(if warn { c.warning } else { c.icon }).flex_shrink(0.0_f32).margin_top(1.0)
    })
}

fn hook_row(t: Theme, h: &Hook, waiting: bool) -> floem::AnyView {
    let top = format!("{}: {}", h.event, h.command);
    row(
        t,
        lead(t, if waiting { "workspace-untrusted" } else { "symbol-event" }, waiting),
        two_lines(t, top, h.from.clone(), waiting),
        None,
    )
}

/// The Hooks section.
fn hooks_body(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    let got = create_memo(move |_| st.hooks.get());
    dyn_container(
        move || got.get(),
        move |h| {
            let Some(h) = h else { return empty().into_any() };
            let mut rows: Vec<floem::AnyView> = h.items.iter().map(|i| hook_row(t, i, false)).collect();
            if rows.is_empty() {
                rows.push(note(t, "No hooks run in this session.".into()));
            }
            if !h.waiting.is_empty() {
                let n = h.waiting.len();
                rows.push(note(
                    t,
                    format!(
                        "This project has {n} hook{} that do{} not run until you trust {}:",
                        if n == 1 { "" } else { "s" },
                        if n == 1 { "es" } else { "" },
                        if n == 1 { "it" } else { "them" }
                    ),
                ));
                rows.extend(h.waiting.iter().map(|i| hook_row(t, i, true)));
                let go = send.clone();
                rows.push(
                    container(button(t, "Trust Project Hooks", true, move || trust(ui, &go)))
                        .style(|s| s.padding_left(20.0).padding_vert(6.0))
                        .into_any(),
                );
            } else if h.trusted == Some(true) {
                rows.push(note(t, "The project's hooks are trusted as they are: a change to them asks again.".into()));
            }
            v_stack_from(rows)
        },
    )
    .style(|s| s.width_full().min_width(0.0))
    .into_any()
}

/// Views one under the other.
fn v_stack_from(rows: Vec<floem::AnyView>) -> floem::AnyView {
    floem::views::stack_from_iter(rows).style(|s| s.flex_col().width_full().min_width(0.0)).into_any()
}

/// A text box of the view.
fn input(t: Theme, value: RwSignal<String>, placeholder: &'static str) -> floem::views::TextInput {
    let focused = create_rw_signal(false);
    text_input(value)
        .placeholder(placeholder)
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| focused.set(false))
        .style(move |s| {
            let c = t.get();
            let s = s
                .width_full()
                .min_width(0.0)
                .height(26.0)
                .padding_horiz(6.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(if focused.get() { c.focus } else { c.input_border })
                .background(c.input_bg)
                .color(c.input_fg)
                .cursor_color(floem::peniko::Brush::Solid(c.cursor));
            crate::widgets::steady_input(s, c.input_bg, Some(c.focus))
        })
}

/// The Permissions section.
fn perms_body(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    let go = send.clone();
    let typed = input(t, st.draft, "Add a rule: run_shell(npm test*)").on_key_down(
        Key::Named(NamedKey::Enter),
        |m| m.is_empty(),
        move |_| add_rule(ui, &go, true),
    );
    let (a, d) = (send.clone(), send.clone());
    let buttons = h_stack((
        button(t, "Allow", true, move || add_rule(ui, &a, true)),
        button(t, "Deny", false, move || add_rule(ui, &d, false)),
    ))
    .style(|s| s.gap(6.0));
    let top = v_stack((typed, buttons))
        .style(|s| s.gap(6.0).width_full().padding_left(20.0).padding_right(12.0).padding_vert(4.0));
    let got = create_memo(move |_| st.perms.get().unwrap_or_default());
    let list = dyn_stack(
        move || got.get(),
        Clone::clone,
        move |p: Perm| {
            let send = send.clone();
            let rule = p.rule.clone();
            let gone = action(t, "close", move || send(json!({"type": "permission_set", "rule": rule, "allow": null})));
            let gone = floem::views::tooltip(gone, || label(|| "Remove for This Session")).into_any();
            let under = if p.allow { "Allowed without asking" } else { "Refused in every mode" };
            row(
                t,
                lead(t, if p.allow { "pass" } else { "circle-slash" }, false),
                two_lines(t, p.rule, under.into(), false),
                Some(gone),
            )
        },
    )
    .style(|s| s.flex_col().width_full());
    let none = desc(t, || "No rules: the mode decides what is asked.".into())
        .style(move |s| s.padding_left(20.0).padding_vert(3.0).apply_if(!got.with(Vec::is_empty), |s| s.hide()));
    v_stack((top, list, none)).style(|s| s.width_full()).into_any()
}

/// The Sandbox section.
fn sandbox_body(ui: Ui) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    let got = create_memo(move |_| st.sandbox.get());
    dyn_container(
        move || got.get(),
        move |s| match s {
            None => empty().into_any(),
            Some((on, lines)) => {
                let mut rows = vec![row(
                    t,
                    lead(t, if on { "shield" } else { "unlock" }, !on),
                    two_lines(t, lines.first().cloned().unwrap_or_default(), String::new(), false),
                    None,
                )];
                let rest: Vec<String> = lines.iter().skip(1).map(|l| l.trim().to_string()).collect();
                if !rest.is_empty() {
                    let words = rest.join("\n");
                    rows.push(
                        label(move || words.clone())
                            .style(move |s| {
                                s.font_family(MONO_FONT.clone())
                                    .font_size(12.0)
                                    .line_height(1.5)
                                    .color(t.get().desc)
                                    .padding_left(42.0)
                                    .padding_right(12.0)
                                    .text_overflow(TextOverflow::Wrap)
                                    .max_width_full()
                                    .min_width(0.0)
                            })
                            .into_any(),
                    );
                }
                v_stack_from(rows)
            }
        },
    )
    .style(|s| s.width_full().min_width(0.0))
    .into_any()
}

/// The Plugins section.
fn plugins_body(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    let go = send.clone();
    let add = container(button(t, "Install Plugin\u{2026}", false, move || install(ui, &go)))
        .style(|s| s.padding_left(20.0).padding_vert(4.0));
    let got = create_memo(move |_| st.plugins.get().unwrap_or_default());
    let list = dyn_stack(
        move || got.get(),
        Clone::clone,
        move |p: Plugin| {
            let (on_send, gone_send) = (send.clone(), send.clone());
            let (name, gone_name) = (p.name.clone(), p.name.clone());
            let on = p.yours;
            let tick = container(checkbox(t, p.on))
                .on_click_stop(move |_| on_send(json!({"type": "plugin_set", "name": name, "on": !on})))
                .style(|s| s.cursor(CursorStyle::Pointer).margin_top(1.0));
            let tick = floem::views::tooltip(tick, move || label(move || if on { "Disable" } else { "Enable" }));
            let gone = action(t, "trash", move || remove_plugin(ui, &gone_send, &gone_name));
            let gone = floem::views::tooltip(gone, || label(|| "Remove")).into_any();
            let top = if p.description.is_empty() {
                p.name.clone()
            } else {
                format!("{} \u{2014} {}", p.name, p.description)
            };
            row(t, tick, two_lines(t, top, plugin_detail(&p), !p.on), Some(gone))
        },
    )
    .style(|s| s.flex_col().width_full());
    let none =
        desc(t, || "No plugins yet. A plugin bundles commands, sub-agents, skills, hooks and MCP servers.".into())
            .style(move |s| {
                s.padding_left(20.0)
                    .padding_right(12.0)
                    .padding_vert(3.0)
                    .apply_if(st.plugins.with(|p| p.as_ref().is_none_or(|p| !p.is_empty())), |s| s.hide())
            });
    v_stack((none, list, add)).style(|s| s.width_full()).into_any()
}

/// The Output Style section.
fn styles_body(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    let got = create_memo(move |_| st.styles.get().unwrap_or_default());
    dyn_stack(
        move || got.get(),
        Clone::clone,
        move |s: Style| {
            let send = send.clone();
            let (name, current) = (s.name.clone(), s.current);
            let under =
                if s.from.is_empty() { s.description.clone() } else { format!("{} ({})", s.description, s.from) };
            let dot = lead(t, if current { "circle-filled" } else { "circle" }, false);
            container(row(t, dot, two_lines(t, s.name, under, false), None))
                .on_click_stop(move |_| {
                    if !current {
                        send(json!({"type": "output_style_set", "name": name}));
                    }
                })
                .style(|s| s.width_full().cursor(CursorStyle::Pointer))
        },
    )
    .style(|s| s.flex_col().width_full())
    .into_any()
}

/// A section's title: its chevron, its name in capitals, its count; a click folds or opens it.
fn header(ui: Ui, part: Part, at: usize) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let folded = create_memo(move |_| st.folded.with(|f| f[at]));
    let count = create_memo(move |_| match part {
        Part::Hooks => st.hooks.with(|h| h.as_ref().map(|h| h.items.len())),
        Part::Permissions => st.perms.with(|p| p.as_ref().map(Vec::len)),
        Part::Plugins => st.plugins.with(|p| p.as_ref().map(Vec::len)),
        Part::Sandbox | Part::Styles => None,
    });
    let warn = create_memo(move |_| {
        part == Part::Hooks && st.hooks.with(|h| h.as_ref().is_some_and(|h| !h.waiting.is_empty()))
    });
    let chevron = dyn_container(move || folded.get(), |f| icon(if f { "chevron-right" } else { "chevron-down" }, 16.0));
    h_stack((
        chevron.style(move |s| s.color(t.get().icon)),
        label(move || part.title().to_uppercase()).style(move |s| bold(caps(s)).margin_left(2.0)),
        label(move || count.get().map(|n| format!(" ({n})")).unwrap_or_default())
            .style(move |s| caps(s).color(t.get().desc)),
        empty().style(|s| s.flex_grow(1.0_f32)),
        lead(t, "warning", true).style(move |s| s.margin_right(8.0).apply_if(!warn.get(), |s| s.hide())),
    ))
    .on_click_stop(move |_| st.folded.update(|f| f[at] = !f[at]))
    .style(move |s| {
        let c = t.get();
        s.width_full()
            .height(22.0)
            .items_center()
            .padding_left(2.0)
            .color(c.side_fg)
            .cursor(CursorStyle::Pointer)
            .border_top(if at == 0 { 0.0 } else { 1.0 })
            .border_color(c.section_border)
    })
}

/// The view's own content.
pub fn view(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    if st.hooks.with_untracked(Option::is_none) {
        ask_all(&send);
    }
    let sections: Vec<floem::AnyView> = Part::ALL
        .into_iter()
        .enumerate()
        .map(|(at, part)| {
            let body = match part {
                Part::Hooks => hooks_body(ui, send.clone()),
                Part::Permissions => perms_body(ui, send.clone()),
                Part::Sandbox => sandbox_body(ui),
                Part::Plugins => plugins_body(ui, send.clone()),
                Part::Styles => styles_body(ui, send.clone()),
            };
            let folded = create_memo(move |_| st.folded.with(|f| f[at]));
            let body = container(body)
                .style(move |s| s.width_full().min_width(0.0).padding_bottom(6.0).apply_if(folded.get(), |s| s.hide()));
            v_stack((header(ui, part, at), body)).style(|s| s.width_full().min_width(0.0)).into_any()
        })
        .collect();
    scroll(v_stack_from(sections).style(|s| s.padding_bottom(10.0)))
        .style(move |s| s.size_full().min_height(0.0).color(t.get().side_fg))
        .into_any()
}

/// Doctor's tab: what it found, or that it runs.
fn doctor_view(ui: Ui, send: Send, _tab: &Tab) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    if st.doctor.with_untracked(Option::is_none) {
        send(json!({"type": "doctor"}));
    }
    let again = send.clone();
    let top = h_stack((
        label(|| "Doctor").style(move |s| bold(s).font_size(13.0)),
        empty().style(|s| s.flex_grow(1.0_f32)),
        floem::views::tooltip(
            action(t, "refresh", move || {
                state(ui).doctor.set(None);
                again(json!({"type": "doctor"}));
            }),
            || label(|| "Run Again"),
        ),
    ))
    .style(|s| s.width_full().items_center().padding_bottom(8.0));
    let words = label(move || st.doctor.get().unwrap_or_else(|| "Running Doctor\u{2026}".into())).style(move |s| {
        let c = t.get();
        s.font_family(MONO_FONT.clone())
            .font_size(13.0)
            .line_height(1.5)
            .color(c.editor_fg)
            .text_overflow(TextOverflow::Wrap)
            .min_width(0.0)
    });
    scroll(v_stack((top, words)).style(|s| s.width_full().padding(16.0)))
        .style(move |s| s.size_full().background(t.get().editor_bg))
        .into_any()
}

/// The actions in its title: none of its own (Run Doctor, Install Plugin… and Refresh are the title menu's).
pub fn actions(_ui: Ui, _send: Send) -> Option<floem::AnyView> {
    None
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::layout::View;
    use crate::menus::{Menu, item};
    add(command(REFRESH, "Harness", "Refresh", |_, send| ask_all(send)).hidden());
    add(command(DOCTOR, "Harness", "Run Doctor", doctor));
    add(command(INSTALL, "Harness", "Install Plugin...", install));
    add(command(TRUST, "Harness", "Trust Project Hooks", |ui, send| {
        if state(ui).hooks.with_untracked(|h| h.as_ref().is_some_and(|h| !h.waiting.is_empty())) {
            trust(ui, send);
        } else {
            toast::info(ui, "This project has no hooks waiting to be trusted.");
        }
    }));
    let title = Menu::ViewTitle(View::Harness);
    crate::menus::add(title.clone(), item(DOCTOR).icon("pulse").group("navigation").order(1));
    crate::menus::add(title.clone(), item(INSTALL).icon("add").group("navigation").order(2));
    crate::menus::add(title, item(REFRESH).icon("refresh").group("navigation").order(3));
    kinds::add(kinds::kind("harness", |_, _| "Doctor".into(), doctor_view).icon("pulse"));
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "hooks" => put(st.hooks, hooks_of(m)),
        "permissions" => put(st.perms, perms_of(m)),
        "sandbox" => put(st.sandbox, sandbox_of(m)),
        "plugins" => put(st.plugins, plugins_of(m)),
        "output_styles" => put(st.styles, styles_of(m)),
        "doctor" => put(st.doctor, m["text"].as_str().unwrap_or("").to_string()),
        // What a change said: a corner bubble, not a line in the chat.
        "notice" if m["of"].as_str().is_some_and(|o| OURS.contains(&o)) => {
            let text = m["text"].as_str().unwrap_or("");
            if m["error"].as_bool() == Some(true) {
                toast::error(ui, text);
            } else {
                toast::info(ui, text);
            }
        }
        // Another session, or the window come again: the hooks, rules and style may be others. Only once the view
        // has asked once (Doctor, which asks the network, never here).
        "state" => {
            if st.hooks.with_untracked(Option::is_some) {
                ask_all(send);
            }
            return false;
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hooks_are_read_with_the_project_s_waiting_ones() {
        let h = hooks_of(
            &json!({"type": "hooks", "items": [{"event": "Stop", "command": "say done", "from": "your settings.json"}],
            "trusted": false, "waiting": [{"event": "PostToolUse", "command": "cargo fmt", "from": "the project's .code-arena/settings.json"}]}),
        );
        assert_eq!(
            h.items[0],
            Hook { event: "Stop".into(), command: "say done".into(), from: "your settings.json".into() }
        );
        assert_eq!((h.trusted, h.waiting.len()), (Some(false), 1));
        assert_eq!(hooks_of(&json!({"type": "hooks", "items": [], "trusted": null, "waiting": []})).trusted, None);
    }

    #[test]
    fn the_rules_the_sandbox_and_the_styles_are_read() {
        let p = perms_of(
            &json!({"rules": [{"rule": "read_file", "allow": true}, {"rule": "run_shell(rm*)", "allow": false}]}),
        );
        assert_eq!(
            p,
            [Perm { rule: "read_file".into(), allow: true }, Perm { rule: "run_shell(rm*)".into(), allow: false }]
        );
        assert_eq!(
            sandbox_of(&json!({"on": true, "text": ["a", "b"]})),
            (true, vec!["a".to_string(), "b".to_string()])
        );
        let s = styles_of(
            &json!({"current": "Learning", "items": [{"name": "default", "description": "d", "from": "built in"},
            {"name": "learning", "description": "l", "from": "built in"}]}),
        );
        assert_eq!(s.iter().map(|s| s.current).collect::<Vec<_>>(), [false, true]);
    }

    #[test]
    fn a_plugin_says_its_version_what_it_holds_and_who_enabled_it() {
        let p = plugins_of(
            &json!({"items": [{"name": "fmt", "version": "1.2", "description": "Formats", "parts": ["hooks", "skills"], "on": true, "yours": false}]}),
        );
        assert_eq!(plugin_detail(&p[0]), "v1.2 \u{b7} hooks, skills \u{b7} enabled by the project");
        let bare = Plugin {
            name: "x".into(),
            version: String::new(),
            description: String::new(),
            parts: vec![],
            on: false,
            yours: false,
        };
        assert_eq!(plugin_detail(&bare), "nothing Code Arena reads");
    }

    #[test]
    fn a_rule_is_a_tool_s_name_with_a_pattern_in_brackets() {
        assert_eq!(rule_problem("run_shell(npm test*)"), None);
        assert_eq!(rule_problem("read_file"), None);
        assert_eq!(rule_problem("mcp__arena__*"), None);
        assert!(rule_problem("").is_some());
        assert!(rule_problem("run shell").is_some());
        assert!(rule_problem("run_shell(npm").is_some());
        assert!(source_problem("  ").is_some());
        assert_eq!(source_problem("https://example.com/p.git"), None);
    }
}
