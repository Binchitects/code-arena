//! The input's tools picker: the wrench in the box's toolbar (without the word "Tools": the panel is narrow), with
//! how many of the session's tool groups are on (hidden while the model calls no tools). A click opens its popover
//! over the box, "Tools: N of M on": each group
//! (Code Arena's own, the Arena project's, each MCP server's) with its icon, its title, a shield when it asks before
//! each call, an About button that opens what it is for and its functions, and a switch (Code Arena's own is always
//! on). Customize tools… at the foot opens the Configure Tools dialog (`CUSTOMIZE`).
//!
//! The groups come in the driver's `tools` (asked when the session is said, after a turn and when the popover
//! opens); a switch sends `tools_set{group, on, key}`. While a turn runs it waits for the turn's end, and says so.

use std::collections::BTreeSet;

use floem::event::{Event, EventListener};
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{
    Decorators, container, dyn_container, empty, h_stack, label, scroll, tooltip, v_stack, v_stack_from_iter,
};
use floem::{AnyView, IntoView, View, ViewId};
use serde_json::{Value, json};

use crate::agent::pickers::calls_tools;
use crate::agent::slots::{self, Slot};
use crate::commands;
use crate::icons::icon;
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, Theme, toast, window_size};

/// The command Customize tools… runs: the Configure Tools dialog registers it again under this id.
pub const CUSTOMIZE: &str = "codeArena.agent.configureTools";

/// A tool group as the session says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub id: String,
    pub title: String,
    pub description: String,
    pub on: bool,
    /// Always on: no switch.
    pub locked: bool,
    /// Its calls ask the person first.
    pub ask_first: bool,
    pub functions: Vec<String>,
    /// Its tools are the model's; false: they join with the next message.
    pub loaded: bool,
}

impl Group {
    fn of(v: &Value) -> Option<Group> {
        Some(Group {
            id: v["id"].as_str()?.to_string(),
            title: v["title"].as_str().unwrap_or("").to_string(),
            description: v["description"].as_str().unwrap_or("").to_string(),
            on: v["on"].as_bool().unwrap_or(true),
            locked: v["locked"].as_bool().unwrap_or(false),
            ask_first: v["ask_first"].as_bool().unwrap_or(false),
            functions: v["functions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect(),
            loaded: v["loaded"].as_bool().unwrap_or(true),
        })
    }

    /// Its codicon: what kind of group it is.
    fn icon(&self) -> &'static str {
        match self.id.as_str() {
            "Code Arena's own" => "tools",
            "Arena's project" => "project",
            "arena" => "cloud",
            "code-intel" => "graph",
            _ => "server",
        }
    }
}

/// How many groups are on (those always on count), of how many.
pub fn counted(groups: &[Group]) -> (usize, usize) {
    (groups.iter().filter(|g| g.on || g.locked).count(), groups.len())
}

/// The picker's own state.
#[derive(Clone, Copy)]
pub struct Tools {
    pub groups: RwSignal<Vec<Group>>,
    pub open: RwSignal<bool>,
    /// The wrench's button, the popover's anchor.
    anchor: RwSignal<Option<ViewId>>,
    /// The group whose About is open.
    about: RwSignal<Option<String>>,
    /// Groups switched while a turn runs: the switch waits for its end.
    waiting: RwSignal<BTreeSet<String>>,
}

impl crate::ext::State for Tools {
    fn new(ui: Ui) -> Tools {
        Tools {
            groups: ui.scope.create_rw_signal(Vec::new()),
            open: ui.scope.create_rw_signal(false),
            anchor: ui.scope.create_rw_signal(None),
            about: ui.scope.create_rw_signal(None),
            waiting: ui.scope.create_rw_signal(BTreeSet::new()),
        }
    }
}

fn ask(send: &Send) {
    send(json!({"type": "tools"}));
}

/// Turns a group on or off: shown at once, and said again by the session.
fn set(ui: Ui, send: &Send, id: &str, on: bool) {
    let st = ui.state::<Tools>();
    st.groups.update(|all| {
        if let Some(g) = all.iter_mut().find(|g| g.id == id) {
            g.on = on;
        }
    });
    if ui.running.get_untracked() {
        st.waiting.update(|w| {
            w.insert(id.to_string());
        });
    }
    send(json!({"type": "tools_set", "group": id, "on": on, "key": id}));
}

/// What it adds to the window's registries, once at start: the wrench, its popover, and Customize's command (its
/// stand-in until the Configure Tools dialog takes its place).
pub fn register(_ui: Ui, _send: &Send) {
    slots::add(Slot::BoxLeft, "tools.picker", 10, |ui, send| button(ui, send).into_any());
    layers::add(Place::Over, "tools.popover", 40, |ui, send| popover(ui, &send));
    commands::add(commands::command(CUSTOMIZE, "Agent", "Configure Tools…", |ui, _| {
        toast::info(ui, "Turn a group on or off here; MCP servers are set in settings.json under mcpServers.");
    }));
}

/// The session's messages that are this part's, taken in. Never taken away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = ui.state::<Tools>();
    match m["type"].as_str().unwrap_or("") {
        "state" | "turn_end" => ask(send),
        "tools" => {
            let groups: Vec<Group> = m["groups"].as_array().into_iter().flatten().filter_map(Group::of).collect();
            if st.groups.with_untracked(|was| *was != groups) {
                st.groups.set(groups);
            }
            if let Some(g) = m["group"].as_str()
                && st.waiting.with_untracked(|w| w.contains(g))
            {
                st.waiting.update(|w| {
                    w.remove(g);
                });
            }
        }
        "refused" if m["of"] == "tools_set" => {
            if let Some(g) = m["key"].as_str() {
                st.waiting.update(|w| {
                    w.remove(g);
                });
            }
            ask(send);
        }
        _ => {}
    }
    false
}

/// The wrench: "Tools" and how many groups are on; hidden while the model calls none or there are none.
fn button(ui: Ui, _send: Send) -> impl IntoView {
    let t = ui.colors;
    let st = ui.state::<Tools>();
    let shown = create_memo(move |_| (counted(&st.groups.get()), calls_tools(ui)));
    let inner = h_stack((
        icon("tools", 14.0),
        label(move || shown.get().0.0.to_string()).style(move |s| {
            let c = t.get();
            s.font_size(11.0)
                .padding_horiz(5.0)
                .border_radius(8.0)
                .line_height(1.4)
                .background(c.badge_bg)
                .color(c.badge_fg)
                .selectable(false)
        }),
    ));
    let me = inner.id();
    st.anchor.set(Some(me));
    let tip = move || {
        let ((on, all), _) = shown.get();
        format!("Tools: {on} of {all} on")
    };
    tooltip(inner, move || label(tip))
        .on_click_stop(move |_| st.open.update(|o| *o = !*o))
        .style(move |s| {
            let c = t.get();
            let ((_, all), tools) = shown.get();
            s.height(24.0)
                .padding_horiz(6.0)
                .gap(5.0)
                .items_center()
                .border_radius(4.0)
                .color(c.desc)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.toolbar_hover))
                .apply_if(all == 0 || !tools, |s| s.hide())
        })
}

/// A switch: lit while on.
fn switch(t: Theme, on: bool, flip: impl Fn() + 'static) -> impl IntoView {
    let knob = empty().style(move |s| {
        let c = t.get();
        s.width(10.0).height(10.0).border_radius(5.0).background(if on { c.button_fg } else { c.icon })
    });
    container(knob).on_click_stop(move |_| flip()).style(move |s| {
        let c = t.get();
        s.width(28.0)
            .height(16.0)
            .padding_horiz(3.0)
            .items_center()
            .border(1.0)
            .border_radius(8.0)
            .border_color(if on { c.button_bg } else { c.input_border })
            .background(if on { c.button_bg } else { c.input_bg })
            .apply_if(on, |s| s.justify_end())
            .flex_shrink(0.0_f32)
            .cursor(CursorStyle::Pointer)
    })
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

/// The lines of a group's About: what it is for, whether it asks, whether it is always on, its functions.
pub fn about_lines(g: &Group, waiting: bool) -> Vec<String> {
    let mut out = vec![g.description.clone()];
    if g.ask_first {
        out.push("Asks you before each call.".into());
    }
    if g.locked {
        out.push("Always on.".into());
    }
    if !g.loaded {
        out.push("Its tools join with the next message.".into());
    }
    if waiting {
        out.push("Applies when the answer is done.".into());
    }
    out.retain(|l| !l.is_empty());
    out
}

/// One group's row, and its About under it while open.
fn row(ui: Ui, send: Send, g: Group, open: bool, waiting: bool) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<Tools>();
    let (id, title, on, locked) = (g.id.clone(), g.title.clone(), g.on, g.locked);
    let shield = g.ask_first.then(|| {
        tooltip(icon("shield", 13.0).style(move |s| s.color(t.get().desc)), || label(|| "Asks you before each call"))
            .into_any()
    });
    let about_id = id.clone();
    let info = tooltip(
        small(t, "info", open, move || {
            let next = (st.about.get_untracked().as_deref() != Some(about_id.as_str())).then(|| about_id.clone());
            st.about.set(next);
        }),
        {
            let title = title.clone();
            move || {
                let title = title.clone();
                label(move || format!("About {title}"))
            }
        },
    );
    let end: AnyView = if locked {
        label(|| "On").style(move |s| s.width(28.0).justify_end().font_size(11.0).color(t.get().desc)).into_any()
    } else {
        let flip = id.clone();
        tooltip(switch(t, on, move || set(ui, &send, &flip, !on)), move || {
            label(move || if on { "Turn off" } else { "Turn on" })
        })
        .into_any()
    };
    let mut head: Vec<AnyView> = vec![
        icon(g.icon(), 16.0).style(move |s| s.color(t.get().icon).flex_shrink(0.0_f32)).into_any(),
        label(move || title.clone()).style(|s| s.flex_grow(1.0_f32).min_width(0.0).text_ellipsis()).into_any(),
    ];
    head.extend(shield);
    head.push(info.into_any());
    head.push(end);
    let head = floem::views::h_stack_from_iter(head).style(move |s| {
        let c = t.get();
        s.items_center()
            .gap(8.0)
            .padding_horiz(6.0)
            .padding_vert(4.0)
            .border_radius(4.0)
            .width_full()
            .hover(|s| s.background(c.list_hover))
    });
    if !open {
        return head.into_any();
    }
    let mut lines: Vec<AnyView> = about_lines(&g, waiting)
        .into_iter()
        .map(|l| label(move || l.clone()).style(|s| s.text_overflow(TextOverflow::Wrap)).into_any())
        .collect();
    if !g.functions.is_empty() {
        let names = g.functions.join(", ");
        lines.push(
            label(move || names.clone())
                .style(|s| s.font_size(11.0).font_family(MONO_FONT.clone()).text_overflow(TextOverflow::Wrap))
                .into_any(),
        );
    }
    let detail = v_stack_from_iter(lines).style(move |s| {
        let c = t.get();
        s.gap(4.0)
            .margin_left(30.0)
            .margin_right(6.0)
            .margin_bottom(4.0)
            .padding_horiz(8.0)
            .padding_vert(6.0)
            .border_radius(4.0)
            .font_size(12.0)
            .line_height(1.4)
            .color(c.desc)
            .background(c.input_bg)
    });
    v_stack((head, detail)).style(|s| s.width_full()).into_any()
}

/// The width of the popover.
const WIDE: f64 = 288.0;

/// The popover's content, for the groups as they are now.
fn body(ui: Ui, send: Send, groups: Vec<Group>, about: Option<String>, waiting: BTreeSet<String>) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<Tools>();
    let (on, all) = counted(&groups);
    let rows: Vec<AnyView> = groups
        .into_iter()
        .map(|g| {
            let open = about.as_deref() == Some(g.id.as_str());
            let wait = waiting.contains(&g.id);
            row(ui, send.clone(), g, open, wait)
        })
        .collect();
    let customize = h_stack((icon("settings", 14.0), label(|| "Customize tools…")))
        .on_click_stop(move |_| {
            st.open.set(false);
            commands::run(ui, &send, CUSTOMIZE);
        })
        .style(move |s| {
            let c = t.get();
            s.items_center()
                .gap(8.0)
                .padding_horiz(8.0)
                .height(28.0)
                .border_radius(4.0)
                .width_full()
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.list_hover))
        });
    v_stack((
        label(move || format!("Tools: {on} of {all} on")).style(move |s| {
            s.font_size(11.0)
                .font_weight(*crate::widgets::SEMIBOLD)
                .color(t.get().desc)
                .padding_horiz(12.0)
                .padding_top(8.0)
                .padding_bottom(4.0)
        }),
        scroll(v_stack_from_iter(rows).style(|s| s.width_full().gap(1.0).padding_horiz(6.0).padding_bottom(4.0)))
            .style(|s| s.max_height(320.0).width_full()),
        container(customize).style(move |s| {
            s.width_full().padding(4.0).border_top(1.0).border_color(t.get().widget_border)
        }),
    ))
    .style(|s| s.width_full())
    .into_any()
}

/// The popover's layer: over the window while open, its left at the wrench's and its bottom over it; a press
/// outside or Escape closes it. It stays within the window when the wrench is near its right edge.
fn popover(ui: Ui, send: &Send) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<Tools>();
    let send = send.clone();
    let what = create_memo(move |_| {
        st.open.get().then(|| (st.groups.get(), st.about.get(), st.waiting.get()))
    });
    let asked = send.clone();
    let panel = dyn_container(
        move || what.get(),
        move |shown| match shown {
            None => empty().into_any(),
            Some((groups, about, waiting)) => body(ui, send.clone(), groups, about, waiting),
        },
    )
    .on_event_stop(EventListener::PointerDown, |_| {})
    .style(move |s| {
        let c = t.get();
        let (left, bottom) = match st.anchor.get().filter(|_| st.open.get()) {
            Some(id) => {
                let r = id.layout_rect();
                let window = window_size(ui, Some(id));
                (r.x0.min(window.width - WIDE - 8.0).max(8.0), (window.height - r.y0 + 6.0).max(8.0))
            }
            None => (8.0, 8.0),
        };
        s.absolute()
            .inset_left(left)
            .inset_bottom(bottom)
            .width(WIDE)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(6.0)
            .background(c.widget)
            .color(c.fg)
            .font_size(13.0)
            .box_shadow_blur(8.0)
            .box_shadow_color(c.shadow)
            .selectable(false)
    });
    let layer = container(panel).keyboard_navigable();
    let id = layer.id();
    create_effect(move |was: Option<bool>| {
        let open = st.open.get();
        if open && was != Some(true) {
            ask(&asked);
            id.request_focus();
        }
        open
    });
    layer
        .on_event_stop(EventListener::PointerDown, move |_| st.open.set(false))
        .on_event_stop(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(k) = e
                && k.key.logical_key == Key::Named(NamedKey::Escape)
            {
                st.open.set(false);
            }
        })
        .style(move |s| s.absolute().inset(0.0).apply_if(!st.open.get(), |s| s.hide()))
        .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(v: Value) -> Group {
        Group::of(&v).expect("a group")
    }

    #[test]
    fn a_group_is_read_with_its_defaults() {
        let g = group(json!({"id": "fs", "title": "FS", "functions": ["a", "b"]}));
        assert!(g.on && g.loaded && !g.locked && !g.ask_first);
        assert_eq!(g.functions, ["a", "b"]);
        assert_eq!(g.icon(), "server");
        assert_eq!(group(json!({"id": "Code Arena's own"})).icon(), "tools");
        assert!(Group::of(&json!({"title": "no id"})).is_none());
    }

    #[test]
    fn those_always_on_count_as_on() {
        let groups = vec![
            group(json!({"id": "own", "locked": true, "on": false})),
            group(json!({"id": "a", "on": true})),
            group(json!({"id": "b", "on": false})),
        ];
        assert_eq!(counted(&groups), (2, 3));
        assert_eq!(counted(&[]), (0, 0));
    }

    #[test]
    fn the_about_says_what_asks_and_what_is_always_on() {
        let g = group(json!({"id": "x", "description": "Does x.", "ask_first": true, "loaded": false}));
        assert_eq!(
            about_lines(&g, true),
            ["Does x.", "Asks you before each call.", "Its tools join with the next message.", "Applies when the answer is done."]
        );
        let own = group(json!({"id": "own", "locked": true}));
        assert_eq!(about_lines(&own, false), ["Always on."]);
    }
}
