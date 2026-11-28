//! The Memory view: what the agent remembers. A box finds a memory by its words; another keeps one ("Remember
//! something…", at most 500 characters), for this project only or for every project. The memories are grouped as
//! This project, About you and The company's, each pinned ones first, with its kind; the person's own can be
//! forgotten. A line above says when the memories are the ones kept on this machine (not signed in to Arena, or
//! Arena out of reach), and a link opens the project's long memory (`.remember/MEMORY.md`) while there is one.
//!
//! When Arena does not let the person use memory, the view and its command are not in the window. The session's
//! side is the driver's `ide_memory.rs`; what is said back comes as a corner bubble.

use floem::IntoView;
use floem::event::EventListener;
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{
    Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, text_input, v_stack,
};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::widgets::{Theme, action, bold, caps, dialog, toast};

/// Whether the view is in the window.
pub const READY: bool = true;

/// The command that asks for the memories again.
const REFRESH: &str = "codeArena.memory.refresh";
/// The most characters a memory kept here has.
pub const MOST: usize = 500;

/// Said while the memories are the files on this machine, as nothing is signed in to Arena.
const OFF: &str = "Not signed in to Arena: memories are kept on this machine, and go to Arena once you sign in.";
/// Said while Arena did not answer at the session's start.
const UNREACHABLE: &str = "Arena is out of reach: these are the memories kept on this machine. They go to Arena when Code Arena next reaches it.";
/// Said when nothing is remembered.
const EMPTY: &str = "Nothing is remembered yet. The agent keeps what a later session should know; you can keep \
                     something yourself above.";

/// One memory, as the session listed it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Item {
    /// Its number in the session's list (what Forget names).
    pub n: u64,
    pub text: String,
    pub kind: String,
    /// person, project or company.
    pub place: String,
    pub pinned: bool,
    pub own: bool,
}

/// What the session said of its memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Memories {
    pub online: bool,
    pub signed_in: bool,
    pub denied: bool,
    pub project: String,
    pub notebook: Option<String>,
    pub items: Vec<Item>,
}

/// A `memory` reply, read.
pub fn memories_of(m: &Value) -> Memories {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let items = m["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|i| Item {
                    n: i["n"].as_u64().unwrap_or(0),
                    text: s(&i["text"]),
                    kind: s(&i["kind"]),
                    place: s(&i["where"]),
                    pinned: i["pinned"].as_bool().unwrap_or(false),
                    own: i["own"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    Memories {
        online: m["online"].as_bool().unwrap_or(false),
        signed_in: m["signed_in"].as_bool().unwrap_or(false),
        denied: m["denied"].as_bool().unwrap_or(false),
        project: s(&m["project"]),
        notebook: m["notebook"].as_str().filter(|p| !p.is_empty()).map(str::to_string),
        items,
    }
}

/// The line above the memories: where they are kept, when it is not Arena.
pub fn where_line(m: &Memories) -> Option<&'static str> {
    match (m.online, m.signed_in) {
        (true, _) => None,
        (false, true) => Some(UNREACHABLE),
        (false, false) => Some(OFF),
    }
}

/// A group's title, by where its memories are.
pub fn group_title(place: &str) -> &'static str {
    match place {
        "project" => "This project",
        "person" => "About you",
        _ => "The company's",
    }
}

/// Whether a memory has every word typed in its text or its kind (any case).
pub fn matches(i: &Item, find: &str) -> bool {
    let hay = format!("{} {}", i.text, i.kind).to_lowercase();
    find.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

/// A row of the list: a group's title (and how many it has), or a memory.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Row {
    Head(&'static str, usize),
    Memory(Item),
}

/// The rows shown: the memories found, grouped (This project, About you, The company's), pinned first in each.
pub fn rows(items: &[Item], find: &str) -> Vec<Row> {
    let mut out = Vec::new();
    for place in ["project", "person", "company"] {
        let mut group: Vec<&Item> = items
            .iter()
            .filter(|i| {
                if place == "company" { !matches!(i.place.as_str(), "project" | "person") } else { i.place == place }
            })
            .filter(|i| matches(i, find))
            .collect();
        if group.is_empty() {
            continue;
        }
        group.sort_by_key(|i| !i.pinned);
        out.push(Row::Head(group_title(place), group.len()));
        out.extend(group.into_iter().cloned().map(Row::Memory));
    }
    out
}

/// What is typed to remember, cut at the most a memory has.
pub fn capped(text: &str) -> Option<String> {
    (text.chars().count() > MOST).then(|| text.chars().take(MOST).collect())
}

/// What the body shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    /// The session has not said yet.
    Waiting,
    Empty,
    NoneFound,
    List,
}

/// The view's state in a window.
#[derive(Clone, Copy)]
pub struct MemoryState {
    pub got: RwSignal<Option<Memories>>,
    pub find: RwSignal<String>,
    pub draft: RwSignal<String>,
    pub project_only: RwSignal<bool>,
}

impl crate::ext::State for MemoryState {
    fn new(ui: Ui) -> Self {
        MemoryState {
            got: ui.scope.create_rw_signal(None),
            find: ui.scope.create_rw_signal(String::new()),
            draft: ui.scope.create_rw_signal(String::new()),
            project_only: ui.scope.create_rw_signal(true),
        }
    }
}

/// The view's state.
pub fn state(ui: Ui) -> MemoryState {
    ui.state::<MemoryState>()
}

/// Keeps what is typed, for this project only or for every project.
fn remember(ui: Ui, send: &Send) {
    let st = state(ui);
    let text = st.draft.get_untracked().trim().to_string();
    if text.is_empty() {
        toast::info(ui, "Say what to remember.");
        return;
    }
    send(json!({"type": "memory_remember", "text": text, "project": st.project_only.get_untracked()}));
    st.draft.set(String::new());
}

/// Forget, once the person said so.
fn forget(ui: Ui, send: &Send, i: &Item) {
    let send = send.clone();
    let (n, text) = (i.n, i.text.clone());
    dialog::ask("Forget this memory?").detail(&i.text).kind(dialog::Kind::Warning).button("Forget").destructive().open(
        ui,
        move |a| {
            if a.yes() {
                send(json!({"type": "memory_forget", "n": n, "text": text}));
            }
        },
    );
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

/// The view's own content.
pub fn view(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    if st.got.with_untracked(Option::is_none) {
        send(json!({"type": "memory"}));
    }
    // A memory is at most 500 characters: what is typed past it is not kept.
    create_effect(move |_| {
        if let Some(cut) = st.draft.with(|d| capped(d)) {
            st.draft.set(cut);
        }
    });
    let find = input(t, st.find, "Find a memory");
    let go = send.clone();
    let keep = input(t, st.draft, "Remember something\u{2026}").on_key_down(
        Key::Named(NamedKey::Enter),
        |m| m.is_empty(),
        move |_| remember(ui, &go),
    );
    let tick = h_stack((
        container(icon("check", 14.0).style(move |s| s.apply_if(!st.project_only.get(), |s| s.hide()))).style(
            move |s| {
                let c = t.get();
                s.width(18.0)
                    .height(18.0)
                    .items_center()
                    .justify_center()
                    .border(1.0)
                    .border_radius(3.0)
                    .border_color(c.checkbox_border)
                    .background(c.checkbox_bg)
                    .color(c.checkbox_fg)
            },
        ),
        label(|| "For this project only").style(move |s| s.margin_left(6.0).font_size(12.0).color(t.get().side_fg)),
    ))
    .on_click_stop(move |_| st.project_only.update(|b| *b = !*b))
    .style(|s| s.items_center().cursor(CursorStyle::Pointer).selectable(false));
    let count = label(move || st.draft.with(|d| format!("{}/{MOST}", d.chars().count())))
        .style(move |s| s.font_size(11.0).color(t.get().desc).apply_if(st.draft.with(String::is_empty), |s| s.hide()));
    let tick_row =
        h_stack((tick, empty().style(|s| s.flex_grow(1.0_f32)), count)).style(|s| s.width_full().items_center());
    let line = create_memo(move |_| st.got.with(|g| g.as_ref().and_then(where_line)));
    let line = desc(t, move || line.get().unwrap_or("").to_string())
        .style(move |s| s.apply_if(line.get().is_none(), |s| s.hide()));
    let notebook = create_memo(move |_| st.got.with(|g| g.as_ref().and_then(|g| g.notebook.clone())));
    let open = send.clone();
    let book = label(move || format!("The project's long memory: {}", notebook.get().unwrap_or_default()))
        .on_click_stop(move |_| {
            if let Some(p) = notebook.get_untracked() {
                ui.open(&p, &open);
            }
        })
        .style(move |s| {
            let c = t.get();
            s.font_size(12.0)
                .color(c.link)
                .text_overflow(TextOverflow::Wrap)
                .min_width(0.0)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.color(c.link_active))
                .apply_if(notebook.get().is_none(), |s| s.hide())
        });
    let top = v_stack((find, keep, tick_row, line, book))
        .style(|s| s.gap(6.0).width_full().padding_horiz(12.0).padding_top(6.0).padding_bottom(8.0));
    let shown = create_memo(move |_| st.got.with(|g| st.find.with(|f| g.as_ref().map(|g| rows(&g.items, f)))));
    let body = create_memo(move |_| match shown.get() {
        None => Body::Waiting,
        Some(r) if !r.is_empty() => Body::List,
        Some(_) if st.got.with(|g| g.as_ref().is_some_and(|g| g.items.is_empty())) => Body::Empty,
        Some(_) => Body::NoneFound,
    });
    let main = dyn_container(
        move || body.get(),
        move |b| match b {
            Body::Waiting => empty().into_any(),
            Body::Empty => container(desc(t, || EMPTY.into()))
                .style(|s| s.padding_horiz(12.0).width_full().min_width(0.0))
                .into_any(),
            Body::NoneFound => {
                container(desc(t, move || format!("No memory has \u{201c}{}\u{201d}.", st.find.get().trim())))
                    .style(|s| s.padding_horiz(12.0).width_full().min_width(0.0))
                    .into_any()
            }
            Body::List => {
                let send = send.clone();
                scroll(
                    dyn_stack(move || shown.get().unwrap_or_default(), Clone::clone, move |r| row(ui, send.clone(), r))
                        .style(|s| s.flex_col().width_full().padding_bottom(10.0)),
                )
                .style(|s| s.size_full().min_height(0.0))
                .into_any()
            }
        },
    )
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full().min_height(0.0));
    v_stack((top, main)).style(move |s| s.size_full().min_height(0.0).color(t.get().side_fg)).into_any()
}

/// A row: a group's title, or a memory with its pin, its kind and Forget (while the pointer is on it).
fn row(ui: Ui, send: Send, r: Row) -> floem::AnyView {
    let t = ui.colors;
    match r {
        Row::Head(title, n) => {
            let words = format!("{} ({n})", title.to_uppercase());
            label(move || words.clone())
                .style(move |s| {
                    bold(caps(s)).color(t.get().side_fg).padding_horiz(12.0).padding_top(8.0).padding_bottom(2.0)
                })
                .into_any()
        }
        Row::Memory(i) => {
            let over = create_rw_signal(false);
            let pin = icon("pinned", 14.0).style(move |s| s.color(t.get().icon).flex_shrink(0.0_f32).margin_top(2.0));
            let pin = if i.pinned { pin.into_any() } else { empty().style(|s| s.width(14.0)).into_any() };
            let text = i.text.clone();
            let kind = if i.kind.is_empty() { String::new() } else { format!("({})", i.kind) };
            let no_kind = kind.is_empty();
            let words = h_stack((
                label(move || text.clone()).style(move |s| {
                    s.font_size(13.0)
                        .line_height(1.4)
                        .color(t.get().fg)
                        .text_overflow(TextOverflow::Wrap)
                        .min_width(0.0)
                        .flex_grow(1.0_f32)
                        .flex_basis(0.0)
                }),
                label(move || kind.clone()).style(move |s| {
                    s.font_size(12.0)
                        .line_height(1.5)
                        .color(t.get().desc)
                        .flex_shrink(0.0_f32)
                        .apply_if(no_kind, |s| s.hide())
                }),
            ))
            .style(|s| s.gap(6.0).flex_grow(1.0_f32).min_width(0.0));
            let own = i.own;
            let gone = action(t, "close", move || forget(ui, &send, &i));
            let gone = container(floem::views::tooltip(gone, || label(|| "Forget")))
                .style(move |s| s.flex_shrink(0.0_f32).apply_if(!own || !over.get(), |s| s.hide()));
            h_stack((pin, words, gone))
                .on_event_cont(EventListener::PointerEnter, move |_| over.set(true))
                .on_event_cont(EventListener::PointerLeave, move |_| over.set(false))
                .style(move |s| {
                    s.gap(6.0)
                        .width_full()
                        .min_height(22.0)
                        .padding_left(16.0)
                        .padding_right(8.0)
                        .padding_vert(2.0)
                        .items_start()
                        .hover(|s| s.background(t.get().list_hover))
                })
                .into_any()
        }
    }
}

/// The actions in its title: none of its own (Refresh is the title menu's).
pub fn actions(_ui: Ui, _send: Send) -> Option<floem::AnyView> {
    None
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{Menu, item};
    add(command(REFRESH, "Memory", "Refresh", |_, send| send(json!({"type": "memory"}))).hidden());
    crate::menus::add(Menu::ViewTitle(View::Memory), item(REFRESH).icon("refresh").group("navigation").order(1));
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "memory" => {
            let got = memories_of(m);
            ui.withhold(View::Memory, got.denied);
            if st.got.with_untracked(|g| g.as_ref() != Some(&got)) {
                st.got.set(Some(got));
            }
            true
        }
        // What keeping or forgetting one said: a corner bubble, not a line in the chat.
        "notice" if m["of"].as_str().is_some_and(|o| o.starts_with("memory_")) => {
            let text = m["text"].as_str().unwrap_or("");
            if m["error"].as_bool() == Some(true) {
                toast::error(ui, text);
            } else {
                toast::info(ui, text);
            }
            true
        }
        // Another session, the window come again, or an answer written (the agent may have kept one).
        "state" | "turn_end" => {
            send(json!({"type": "memory"}));
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply() -> Value {
        json!({"type": "memory", "online": false, "signed_in": true, "denied": false, "project": "github.com/a/b",
            "notebook": ".remember/MEMORY.md", "items": [
                {"n": 1, "text": "Uses tabs", "kind": "", "where": "person", "pinned": false, "own": true},
                {"n": 2, "text": "Build with make", "kind": "fact", "where": "project", "pinned": false, "own": true},
                {"n": 3, "text": "Release on Mondays", "kind": "fact", "where": "project", "pinned": true, "own": true},
                {"n": 4, "text": "No Friday deploys", "kind": "rule", "where": "company", "pinned": true, "own": false}]})
    }

    #[test]
    fn a_reply_is_read() {
        let m = memories_of(&reply());
        assert_eq!(
            (m.items.len(), m.notebook.as_deref(), m.project.as_str()),
            (4, Some(".remember/MEMORY.md"), "github.com/a/b")
        );
        assert_eq!(
            m.items[3],
            Item {
                n: 4,
                text: "No Friday deploys".into(),
                kind: "rule".into(),
                place: "company".into(),
                pinned: true,
                own: false
            }
        );
        assert_eq!(memories_of(&json!({"type": "memory", "notebook": null})).notebook, None);
    }

    #[test]
    fn memories_are_grouped_with_the_pinned_first() {
        let m = memories_of(&reply());
        let r = rows(&m.items, "");
        let names: Vec<String> = r
            .iter()
            .map(|r| match r {
                Row::Head(t, n) => format!("{t} {n}"),
                Row::Memory(i) => i.n.to_string(),
            })
            .collect();
        assert_eq!(names, ["This project 2", "3", "2", "About you 1", "1", "The company's 1", "4"]);
    }

    #[test]
    fn a_memory_is_found_by_every_word_in_any_case() {
        let m = memories_of(&reply());
        let r = rows(&m.items, "MAKE build");
        assert_eq!(r, vec![Row::Head("This project", 1), Row::Memory(m.items[1].clone())]);
        assert_eq!(rows(&m.items, "rule").len(), 2, "its kind is found too");
        assert!(rows(&m.items, "nothing like it").is_empty());
    }

    #[test]
    fn the_line_says_where_memories_are_kept() {
        let mut m = Memories { signed_in: true, ..Memories::default() };
        assert_eq!(where_line(&m), Some(UNREACHABLE));
        m.signed_in = false;
        assert_eq!(where_line(&m), Some(OFF));
        m.online = true;
        assert_eq!(where_line(&m), None);
    }

    #[test]
    fn what_is_typed_stops_at_five_hundred_characters() {
        assert_eq!(capped(&"é".repeat(MOST)), None);
        assert_eq!(capped(&"é".repeat(MOST + 3)).map(|c| c.chars().count()), Some(MOST));
    }
}
