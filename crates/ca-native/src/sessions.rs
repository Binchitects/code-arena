//! The Sessions view: this folder's sessions, newest first under Today, Yesterday, Last 7 Days, Last 30 Days and
//! Older, as VS Code's chat history lists its chats. Each row has its title (its words, how many messages and its
//! id when the pointer rests on it), a dot on the one worked in (beating while an answer is written), a cloud when
//! it is kept with a chat in Arena, "not sent" while Arena does not have all of it, and Fork while the pointer is
//! over it. A click takes the session up; while an answer is written the rows are dimmed and say why instead. A
//! box at the top searches the words of the folder's sessions (a click on a message found takes its session up).
//! The foot says the folder, its branch and the version. Bars stand in for the rows until the list comes.
//!
//! New Session (the view's title, the chat's More menu, `/clear` and `/new` in the input), Fork Session and Fork
//! from a Question… are commands. The session's side is the driver's `ide_sessions.rs`.

use std::time::Duration;

use floem::IntoView;
use floem::action::exec_after;
use floem::event::EventListener;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::style::CursorStyle;
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, container, empty, h_stack, label, scroll, text_input, tooltip,
    v_stack, virtual_stack,
};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layout::View;
use crate::model::Kind;
use crate::state::{Send, Ui};
use crate::widgets::{action, quick, toast};

/// Whether the view is in the window.
pub const READY: bool = true;

/// A new session in this one's place (this one is kept).
pub const NEW: &str = "workbench.action.chat.newChat";
/// A fork of the session worked in: all of it, in a new session.
pub const FORK: &str = "codeArena.chat.fork";
/// A fork with what came before one of the session's questions.
pub const FORK_FROM: &str = "codeArena.chat.forkFromQuestion";
/// The list asked again.
pub const REFRESH: &str = "codeArena.sessions.refresh";

/// What a click says while an answer is written.
const BUSY: &str = "Wait for the answer, or stop it, to take up another session.";
/// The list, while the folder has no session.
const NONE_YET: &str = "No sessions in this folder yet.";

/// A session as the list shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub updated: i64,
    /// Days since it was last worked in, by the calendar.
    pub days: i64,
    pub messages: i64,
    /// Kept with a chat in Arena.
    pub chat: bool,
    /// Arena does not have all of it yet.
    pub unsent: bool,
    /// The one worked in.
    pub current: bool,
}

/// A message found by the search.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hit {
    pub id: String,
    pub seq: i64,
    pub text: String,
}

/// A row of the view.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Group(&'static str),
    Session(Session),
    /// A message found, with its session's title.
    Hit(Hit, String),
    /// A line said in the list's place (nothing there, nothing found).
    Note(&'static str),
    /// A bar standing in for a row until the list comes.
    Bar(usize),
}

impl Row {
    /// What the list knows the row by: another key draws it again.
    fn key(&self) -> String {
        match self {
            Row::Group(g) => format!("g{g}"),
            Row::Session(s) => format!("s{}|{}|{}|{}|{}|{}", s.id, s.updated, s.title, s.current, s.chat, s.unsent),
            Row::Hit(h, _) => format!("h{}|{}", h.id, h.seq),
            Row::Note(n) => format!("n{n}"),
            Row::Bar(i) => format!("b{i}"),
        }
    }
}

/// The heading a session is under, by how many days ago it was last worked in.
pub fn group_of(days: i64) -> &'static str {
    match days {
        ..=0 => "Today",
        1 => "Yesterday",
        2..=6 => "Last 7 Days",
        7..=29 => "Last 30 Days",
        _ => "Older",
    }
}

/// A session's title, or what a session that has said nothing yet is called.
pub fn title_of(s: &Session) -> String {
    if s.title.trim().is_empty() { "New Session".into() } else { s.title.clone() }
}

/// What the pointer resting on a row says: "title · N messages · id".
pub fn tip(s: &Session) -> String {
    let n = if s.messages == 1 { "1 message".to_string() } else { format!("{} messages", s.messages) };
    format!("{} · {n} · {}", crate::bidi::isolate(&title_of(s)), s.id)
}

/// The sessions with their headings, in the order they came (newest first).
pub fn rows(list: &[Session]) -> Vec<Row> {
    let mut out = Vec::with_capacity(list.len() + 5);
    let mut last = "";
    for s in list {
        let g = group_of(s.days);
        if g != last {
            out.push(Row::Group(g));
            last = g;
        }
        out.push(Row::Session(s.clone()));
    }
    out
}

/// The rows the view shows: bars until the list comes, the messages found while words are typed, else the list.
pub fn shown(list: Option<&[Session]>, words: &str, found: Option<&(String, Vec<Hit>)>) -> Vec<Row> {
    let Some(list) = list else { return (0..5).map(Row::Bar).collect() };
    if !words.trim().is_empty() {
        return match found {
            Some((w, hits)) if w == words.trim() => {
                if hits.is_empty() {
                    return vec![Row::Note("No results found.")];
                }
                let title = |id: &str| list.iter().find(|s| s.id == id).map(title_of).unwrap_or_default();
                hits.iter().map(|h| Row::Hit(h.clone(), title(&h.id))).collect()
            }
            _ => Vec::new(),
        };
    }
    // The session worked in is listed even before anything is said in it: with nothing said in any, there is none.
    if list.iter().all(|s| s.messages == 0) {
        return vec![Row::Note(NONE_YET)];
    }
    rows(list)
}

/// The list in a `sessions` message.
pub fn sessions_of(m: &Value) -> Vec<Session> {
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    m["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| Session {
            id: s(v, "id"),
            title: s(v, "title"),
            updated: v["updated"].as_i64().unwrap_or(0),
            days: v["days"].as_i64().unwrap_or(0),
            messages: v["messages"].as_i64().unwrap_or(0),
            chat: v["chat"] == true,
            unsent: v["unsent"] == true,
            current: v["current"] == true,
        })
        .collect()
}

/// The messages in a `session_found` message.
pub fn hits_of(m: &Value) -> Vec<Hit> {
    m["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| Hit {
            id: v["id"].as_str().unwrap_or("").into(),
            seq: v["seq"].as_i64().unwrap_or(0),
            text: v["text"].as_str().unwrap_or("").replace('\n', " "),
        })
        .collect()
}

/// The view's state in a window.
#[derive(Clone, Copy)]
struct Sessions {
    /// The folder's sessions; none until the first list comes.
    list: RwSignal<Option<Vec<Session>>>,
    /// The search box's words, and what was found for which words.
    words: RwSignal<String>,
    found: RwSignal<Option<(String, Vec<Hit>)>>,
    /// The version the session says, for the foot.
    version: RwSignal<String>,
    /// The dot of the session worked in, lit or not, while an answer is written.
    beat: RwSignal<bool>,
    /// Counts the keys typed in the search box (a search goes out a moment after the last).
    typed: RwSignal<u64>,
}

impl crate::ext::State for Sessions {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        Sessions {
            list: s.create_rw_signal(None),
            words: s.create_rw_signal(String::new()),
            found: s.create_rw_signal(None),
            version: s.create_rw_signal(String::new()),
            beat: s.create_rw_signal(true),
            typed: s.create_rw_signal(0),
        }
    }
}

fn state(ui: Ui) -> Sessions {
    ui.state::<Sessions>()
}

/// The session worked in, by its id.
fn current(ui: Ui) -> Option<String> {
    state(ui).list.with_untracked(|l| l.as_ref()?.iter().find(|s| s.current).map(|s| s.id.clone()))
}

/// Takes a session up, unless an answer is being written (then says why).
fn resume(ui: Ui, send: &Send, id: &str) {
    if ui.running.get_untracked() {
        toast::info(ui, BUSY);
    } else if current(ui).as_deref() != Some(id) {
        send(json!({"type": "session_resume", "id": id}));
    }
}

/// Sends a request that changes the session, unless an answer is being written.
fn between_turns(ui: Ui, send: &Send, m: Value) {
    if ui.running.get_untracked() {
        toast::info(ui, BUSY);
    } else {
        send(m);
    }
}

/// The session's numbered questions, newest first, to fork from.
fn fork_from(ui: Ui, send: &Send) {
    let items: Vec<quick::Item> = ui.items.with_untracked(|items| {
        items
            .iter()
            .rev()
            .filter_map(|i| match &i.kind {
                Kind::User(q) if q.q > 0 && !q.note => {
                    let words = if q.typed.is_empty() { &q.text } else { &q.typed };
                    let first = words.lines().next().unwrap_or("").chars().take(100).collect::<String>();
                    Some(quick::item(&q.q.to_string(), &first).description(&format!("question {}", q.q)))
                }
                _ => None,
            })
            .collect()
    });
    if items.is_empty() {
        toast::info(ui, "This session has no question to fork from yet.");
        return;
    }
    let send = send.clone();
    quick::pick("Fork from a Question")
        .placeholder("The new session has what came before the question chosen")
        .items(items)
        .open(ui, move |chosen| {
            if let Some(q) = chosen.and_then(|c| c.id.parse::<u64>().ok()) {
                between_turns(ui, &send, json!({"type": "session_fork", "before_q": q}));
            }
        });
}

/// The dot beats while an answer is written: one timer for the window, not one a row.
fn beat(ui: Ui) {
    let st = state(ui);
    if !ui.running.get_untracked() {
        if !st.beat.get_untracked() {
            st.beat.set(true);
        }
        return;
    }
    st.beat.update(|b| *b = !*b);
    exec_after(Duration::from_millis(600), move |_| beat(ui));
}

/// The view's own content.
pub fn view(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    if st.list.with_untracked(Option::is_none) {
        send(json!({"type": "sessions"}));
    }
    // A search a moment after the last key.
    {
        let send = send.clone();
        create_effect(move |first: Option<()>| {
            let words = st.words.get();
            if first.is_none() {
                return;
            }
            st.typed.update(|g| *g += 1);
            let g = st.typed.get_untracked();
            let send = send.clone();
            exec_after(Duration::from_millis(300), move |_| {
                let words = words.trim();
                if st.typed.get_untracked() == g && !words.is_empty() {
                    send(json!({"type": "session_search", "words": words}));
                }
            });
        });
    }
    let focused = create_rw_signal(false);
    let search = text_input(st.words)
        .placeholder("Search sessions")
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| focused.set(false))
        .style(move |s| {
            let c = t.get();
            let s = s
                .flex_grow(1.0_f32)
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
        });
    let top = container(search).style(|s| s.width_full().padding_horiz(12.0).padding_vert(6.0));
    let items = move || {
        st.list.with(|l| {
            st.words
                .with(|w| st.found.with(|f| shown(l.as_deref(), w, f.as_ref()).into_iter().collect::<im::Vector<_>>()))
        })
    };
    let arena = crate::sessions_arena::section(ui, send.clone());
    let list = virtual_stack(
        VirtualDirection::Vertical,
        VirtualItemSize::Fixed(Box::new(|| 22.0)),
        items,
        Row::key,
        move |row| match row {
            Row::Group(g) => group_row(ui, g).into_any(),
            Row::Session(s) => session_row(ui, send.clone(), s).into_any(),
            Row::Hit(h, title) => hit_row(ui, send.clone(), h, title).into_any(),
            Row::Note(n) => label(move || n)
                .style(move |s| s.padding_horiz(20.0).height(22.0).items_center().color(t.get().desc))
                .into_any(),
            Row::Bar(i) => bar_row(ui, i).into_any(),
        },
    )
    .style(|s| s.flex_col().width_full());
    v_stack((top, scroll(list).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full()), arena, foot(ui)))
        .style(move |s| s.size_full().min_height(0.0).color(t.get().side_fg))
        .into_any()
}

/// A heading: Today, Yesterday…
fn group_row(ui: Ui, g: &'static str) -> impl IntoView {
    let t = ui.colors;
    label(move || g).style(move |s| {
        s.height(22.0).width_full().items_center().padding_left(12.0).font_size(11.0).font_bold().color(t.get().desc)
    })
}

/// A bar where a row will be.
fn bar_row(ui: Ui, i: usize) -> impl IntoView {
    let t = ui.colors;
    let width = [70.0, 55.0, 80.0, 45.0, 62.0][i % 5];
    container(empty().style(move |s| {
        s.height(10.0).width_pct(width).border_radius(3.0).background(crate::widgets::mix(
            t.get().side_bg,
            t.get().side_fg,
            0.12,
        ))
    }))
    .style(|s| s.height(22.0).width_full().items_center().padding_horiz(20.0))
}

/// A small mark at a row's right: the cloud, "not sent".
fn pill(ui: Ui, text: &'static str) -> impl IntoView {
    let t = ui.colors;
    label(move || text).style(move |s| {
        let c = t.get();
        s.font_size(10.0)
            .padding_horiz(5.0)
            .height(16.0)
            .items_center()
            .border_radius(8.0)
            .background(c.badge_bg)
            .color(c.badge_fg)
    })
}

fn session_row(ui: Ui, send: Send, s: Session) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let hovered = create_rw_signal(false);
    let (title, said, id, fork_id) = (title_of(&s), tip(&s), s.id.clone(), s.id.clone());
    let is_current = s.current;
    let dot = empty().style(move |s| {
        let c = t.get();
        let lit = !ui.running.get() || st.beat.get();
        s.width(8.0)
            .height(8.0)
            .border_radius(4.0)
            .background(if lit { c.progress } else { Color::TRANSPARENT })
            .border(1.0)
            .border_color(c.progress)
            .apply_if(!is_current, |s| s.hide())
    });
    let mark = container(icon("comment-discussion", 16.0)).style(move |s| s.apply_if(is_current, |s| s.hide()));
    let (chat, unsent, said_nothing) = (s.chat, s.unsent, s.messages == 0);
    let fork_send = send.clone();
    let fork = container(action(t, "repo-forked", move || {
        between_turns(ui, &fork_send, json!({"type": "session_fork", "id": fork_id}))
    }))
    .style(move |s| s.apply_if(!hovered.get() || said_nothing, |s| s.hide()));
    let row = h_stack((
        container((dot, mark)).style(|s| s.width(16.0).items_center().justify_center().flex_shrink(0.0_f32)),
        label(move || title.clone()).style(move |s| {
            let s = s.min_width(0.0).flex_grow(1.0_f32).text_ellipsis();
            if is_current { s.font_bold() } else { s }
        }),
        container(pill(ui, "not sent")).style(move |s| s.apply_if(!unsent, |s| s.hide())),
        container(icon("cloud", 14.0)).style(move |s| s.color(t.get().desc).apply_if(!chat, |s| s.hide())),
        fork,
    ))
    .on_event_cont(EventListener::PointerEnter, move |_| hovered.set(true))
    .on_event_cont(EventListener::PointerLeave, move |_| hovered.set(false))
    .on_click_stop(move |_| resume(ui, &send, &id))
    .style(move |s| {
        let c = t.get();
        let busy = ui.running.get();
        s.height(22.0)
            .width_full()
            .items_center()
            .gap(6.0)
            .padding_left(14.0)
            .padding_right(8.0)
            .font_size(13.0)
            .selectable(false)
            .apply_if(is_current, |s| s.background(c.list_inactive))
            .apply_if(busy && !is_current, |s| s.color(c.desc))
            .apply_if(!busy && !is_current, |s| s.cursor(CursorStyle::Pointer).hover(|s| s.background(c.list_hover)))
    });
    tooltip(row, move || {
        let said = if ui.running.get_untracked() && !is_current { BUSY.to_string() } else { said.clone() };
        label(move || said.clone())
    })
}

fn hit_row(ui: Ui, send: Send, h: Hit, title: String) -> impl IntoView {
    let t = ui.colors;
    let (text, id) = (h.text.clone(), h.id.clone());
    let row = h_stack((
        icon("comment", 16.0),
        label(move || text.clone()).style(|s| s.min_width(0.0).flex_grow(1.0_f32).text_ellipsis()),
    ))
    .on_click_stop(move |_| resume(ui, &send, &id))
    .style(move |s| {
        let c = t.get();
        s.height(22.0)
            .width_full()
            .items_center()
            .gap(6.0)
            .padding_left(14.0)
            .padding_right(8.0)
            .font_size(13.0)
            .selectable(false)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.background(c.list_hover))
    });
    // The session it is in, when the pointer rests on it.
    tooltip(row, move || {
        let title = title.clone();
        label(move || title.clone())
    })
}

/// The foot: the folder, its branch, the version.
fn foot(ui: Ui) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let line = |name: &'static str, text: Box<dyn Fn() -> String>| {
        h_stack((icon(name, 14.0), label(text).style(|s| s.min_width(0.0).text_ellipsis())))
            .style(|s| s.gap(6.0).items_center().height(18.0).min_width(0.0))
    };
    let folder = line("folder", Box::new(move || ui.folder.get()));
    let branch = container(line("git-branch", Box::new(move || ui.branch.get())))
        .style(move |s| s.apply_if(ui.branch.with(String::is_empty), |s| s.hide()));
    let version = line("info", Box::new(move || format!("code-arena {}", st.version.get())));
    v_stack((folder, branch, version)).style(move |s| {
        let c = t.get();
        crate::widgets::line(s, c.side_border, 't')
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .font_size(11.0)
            .color(c.desc)
    })
}

/// The actions in its title: none of its own (New Session and Refresh are the title menu's).
pub fn actions(_ui: Ui, _send: Send) -> Option<floem::AnyView> {
    None
}

/// What it adds to the window's registries, once at start.
pub fn register(ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{Menu, item};
    add(command(NEW, "Chat", "New Session", |ui, send| between_turns(ui, send, json!({"type": "session_new"}))));
    add(command(FORK, "Chat", "Fork Session", |ui, send| between_turns(ui, send, json!({"type": "session_fork"}))));
    add(command(FORK_FROM, "Chat", "Fork from a Question...", fork_from));
    add(command(REFRESH, "Sessions", "Refresh", |_, send| send(json!({"type": "sessions"}))).hidden());
    let title = |id: &str, icon: &'static str, order: i32| item(id).icon(icon).group("navigation").order(order);
    crate::menus::add(Menu::ViewTitle(View::Sessions), title(NEW, "add", 1));
    crate::menus::add(Menu::ViewTitle(View::Sessions), title(REFRESH, "refresh", 2));
    crate::menus::add(Menu::AgentMore, item(NEW).group("1_session").order(10));
    crate::menus::add(Menu::AgentMore, item(FORK).group("1_session").order(20));
    crate::menus::add(Menu::AgentMore, item(FORK_FROM).group("1_session").order(30));
    // The dot beats from the moment an answer is begun.
    ui.scope.create_effect(move |was: Option<bool>| {
        let now = ui.running.get();
        if now && was != Some(true) {
            beat(ui);
        }
        now
    });
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "sessions" => {
            let list = sessions_of(m);
            if st.list.with_untracked(|l| l.as_ref() != Some(&list)) {
                st.list.set(Some(list));
            }
        }
        "session_found" => {
            let words = m["words"].as_str().unwrap_or("").to_string();
            if st.words.with_untracked(|w| w.trim() == words) {
                st.found.set(Some((words, hits_of(m))));
            }
        }
        // Another session, or the window come again: its list; and when an answer has been written, the list
        // says the session's new place and count.
        "state" => {
            let v = m["version"].as_str().unwrap_or("").to_string();
            if st.version.with_untracked(|old| *old != v) {
                st.version.set(v);
            }
            send(json!({"type": "sessions"}));
        }
        "turn_end" => send(json!({"type": "sessions"})),
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, days: i64) -> Session {
        Session { id: id.into(), title: format!("about {id}"), days, messages: 2, ..Session::default() }
    }

    #[test]
    fn sessions_go_under_the_heading_of_the_day_they_were_last_worked_in() {
        assert_eq!(
            [0, 1, 2, 6, 7, 29, 30].map(group_of),
            ["Today", "Yesterday", "Last 7 Days", "Last 7 Days", "Last 30 Days", "Last 30 Days", "Older"]
        );
        let list = [session("a", 0), session("b", 0), session("c", 3), session("d", 40)];
        let r = rows(&list);
        let heads: Vec<_> = r.iter().filter_map(|r| if let Row::Group(g) = r { Some(*g) } else { None }).collect();
        assert_eq!(heads, ["Today", "Last 7 Days", "Older"]);
        assert_eq!(r.len(), 7);
        assert_eq!(r[1], Row::Session(list[0].clone()));
    }

    #[test]
    fn the_view_shows_bars_then_the_list_or_what_was_found_or_says_there_is_none() {
        assert!(shown(None, "", None).iter().all(|r| matches!(r, Row::Bar(_))));
        assert_eq!(shown(Some(&[]), "", None), [Row::Note(NONE_YET)]);
        let fresh = Session { id: "n".into(), current: true, ..Session::default() };
        assert_eq!(shown(Some(&[fresh]), "", None), [Row::Note(NONE_YET)]);
        let list = [session("a", 0)];
        assert_eq!(shown(Some(&list), "", None).len(), 2);
        // Typed words: nothing until what was found for them comes; an answer to other words is not shown.
        assert!(shown(Some(&list), "parser", None).is_empty());
        let found = ("pars".to_string(), vec![Hit { id: "a".into(), seq: 3, text: "the parser".into() }]);
        assert!(shown(Some(&list), "parser", Some(&found)).is_empty());
        let found = ("parser".to_string(), found.1);
        assert_eq!(shown(Some(&list), " parser ", Some(&found)), [Row::Hit(found.1[0].clone(), "about a".into())]);
        assert_eq!(shown(Some(&list), "parser", Some(&("parser".into(), vec![]))), [Row::Note("No results found.")]);
    }

    #[test]
    fn a_row_says_its_title_count_and_id_and_a_session_with_nothing_said_has_a_name() {
        let mut s = session("abc", 0);
        assert_eq!(tip(&s), "about abc · 2 messages · abc");
        s.messages = 1;
        s.title = " ".into();
        assert_eq!(tip(&s), "New Session · 1 message · abc");
    }

    #[test]
    fn the_lists_the_session_sends_are_read_whole() {
        let m = json!({"type": "sessions", "items": [{"id": "a", "title": "t", "updated": 5, "days": 1,
            "messages": 4, "chat": true, "unsent": true, "current": true}]});
        let want = Session {
            id: "a".into(),
            title: "t".into(),
            updated: 5,
            days: 1,
            messages: 4,
            chat: true,
            unsent: true,
            current: true,
        };
        assert_eq!(sessions_of(&m), [want]);
        let f = json!({"type": "session_found", "words": "x", "items": [{"id": "a", "seq": 2, "text": "one\ntwo"}]});
        assert_eq!(hits_of(&f), [Hit { id: "a".into(), seq: 2, text: "one two".into() }]);
    }
}
