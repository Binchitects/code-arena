//! Arena's chats in the Sessions view, and where the session stands with its own chat there.
//!
//! Under the sessions, a section of its own as VS Code's views have ("ALI'S CHATS", the account's name): the
//! person's chats in Arena, newest first, with a box that finds one by its title's or its folder's words. A chat
//! kept with a session of this folder is left out (the session is in the list above); Code Arena's own chats have
//! the terminal's icon. A click continues the chat here (its session, or a new one with its history), a spinner
//! turning on the row meanwhile. The section's title has Send the Other Sessions Here (each session of the folder
//! that Arena lacks, sent to its chat) and Refresh.
//!
//! In the chat's header, a mark says where the session stands: "In step", "N to send", "Web news" or "Not in step",
//! the whole of it when the pointer rests on it. What the session says between turns (Arena out of reach and back,
//! the web's messages taken in) comes as toasts. The section and the mark are there only while the session is kept
//! with Arena. The driver's side is `ide_sessions/arena.rs`.

use std::time::Duration;

use floem::action::exec_after;
use floem::event::EventListener;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo, create_rw_signal};
use floem::style::CursorStyle;
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, container, dyn_container, h_stack, label, scroll, text_input,
    tooltip, v_stack, virtual_stack,
};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::widgets::{action, bold, caps, line, toast};

/// Send the Other Sessions Here: every session of this folder that Arena lacks, sent to its chat.
pub const SEND_ALL: &str = "codeArena.sessions.sendToArena";
/// The chats read from Arena again.
pub const REFRESH: &str = "codeArena.sessions.refreshArenaChats";

/// What a click says while an answer is written.
const BUSY: &str = "Wait for the answer, or stop it, to continue a chat from Arena.";
/// The most rows the driver sends (more are found by their words).
const MOST: usize = 200;

/// A chat in Arena, as the list shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chat {
    pub id: String,
    pub title: String,
    /// The folder it was worked in, when Code Arena made it.
    pub place: String,
    pub messages: i64,
    /// Code Arena made it.
    pub terminal: bool,
}

/// Where the session stands with its chat in Arena.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Standing {
    pub chat: Option<String>,
    pub url: Option<String>,
    pub account: Option<String>,
    /// Messages Arena does not have yet.
    pub waiting: u64,
    /// Why Arena is not kept up now.
    pub trouble: Option<String>,
    /// The web added messages, not taken in yet.
    pub behind: bool,
    /// Kept here only.
    pub here: bool,
}

/// A row of the chats' list.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Chat(Chat),
    /// A line said in the list's place.
    Note(String),
    /// How many more matched than are listed.
    More(usize),
}

impl Row {
    fn key(&self) -> String {
        match self {
            Row::Chat(c) => format!("c{}|{}|{}", c.id, c.title, c.messages),
            Row::Note(n) => format!("n{n}"),
            Row::More(n) => format!("m{n}"),
        }
    }
}

/// The standing in `state.sync`, `status.sync` or `session_sync.sync`; none while the session is not kept with
/// Arena at all.
pub fn standing_of(v: &Value) -> Option<Standing> {
    if !v.is_object() {
        return None;
    }
    let text = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    Some(Standing {
        chat: text("chat"),
        url: text("url"),
        account: text("account"),
        waiting: v["waiting"].as_u64().unwrap_or(0),
        trouble: text("trouble"),
        behind: v["behind"] == true,
        here: v["here"] == true,
    })
}

/// The header's mark: its icon, its words, and the whole of it for the pointer. None while there is nothing to
/// say (kept here only, or no chat yet).
pub fn mark_of(s: &Standing) -> Option<(&'static str, String, String)> {
    if s.here {
        return None;
    }
    if let Some(why) = &s.trouble {
        return Some((
            "warning",
            "Not in step".into(),
            format!("Not in step with the chat in Arena: {why}. What waits is sent when Arena answers."),
        ));
    }
    if s.behind {
        return Some((
            "cloud-download",
            "Web news".into(),
            "The web chat has new messages: they are taken in before the next answer.".into(),
        ));
    }
    if s.waiting > 0 {
        let n = if s.waiting == 1 { "1 message is".to_string() } else { format!("{} messages are", s.waiting) };
        return Some((
            "cloud-upload",
            format!("{} to send", s.waiting),
            format!("{n} on the way to the chat in Arena."),
        ));
    }
    s.chat.as_ref()?;
    let whose = s.account.as_deref().map(|a| format!(" ({a})")).unwrap_or_default();
    Some(("cloud", "In step".into(), format!("In step with the chat in Arena{whose}: Arena has every message.")))
}

/// The section's title: the account's chats.
pub fn heading(account: Option<&str>) -> String {
    match account.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => format!("{a}'s chats"),
        None => "Chats in Arena".into(),
    }
}

/// The chats in an `arena_chats` message.
pub fn chats_of(m: &Value) -> Vec<Chat> {
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    m["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| Chat {
            id: s(v, "id"),
            title: s(v, "title"),
            place: s(v, "place"),
            messages: v["messages"].as_i64().unwrap_or(0),
            terminal: v["terminal"] == true,
        })
        .collect()
}

/// The rows: the chats, and how many more matched; else why there are none.
pub fn rows(list: Option<&[Chat]>, more: usize, error: Option<&str>, words: &str) -> Vec<Row> {
    if let Some(e) = error {
        return vec![Row::Note(e.to_string())];
    }
    let Some(list) = list else { return vec![Row::Note("Loading chats…".into())] };
    if list.is_empty() {
        let none = if words.trim().is_empty() { "No chats in Arena yet." } else { "No chats found." };
        return vec![Row::Note(none.into())];
    }
    let mut out: Vec<Row> = list.iter().take(MOST).cloned().map(Row::Chat).collect();
    if more > 0 {
        out.push(Row::More(more));
    }
    out
}

/// A chat's title, or what an untitled one is called.
pub fn title_of(c: &Chat) -> String {
    if c.title.trim().is_empty() { "Untitled chat".into() } else { c.title.clone() }
}

/// What the pointer resting on a chat says: "title · N messages · folder".
pub fn tip(c: &Chat) -> String {
    let n = if c.messages == 1 { "1 message".to_string() } else { format!("{} messages", c.messages) };
    let place = if c.place.is_empty() { String::new() } else { format!(" · {}", c.place) };
    format!("{} · {n}{place}", crate::bidi::isolate(&title_of(c)))
}

/// The folder's last name, as a row's description shows it.
fn folder_name(place: &str) -> String {
    place.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or("").to_string()
}

/// The part's state in a window.
#[derive(Clone, Copy)]
struct Arena {
    /// Where the session stands; none while it is not kept with Arena.
    sync: RwSignal<Option<Standing>>,
    account: RwSignal<Option<String>>,
    /// The chats found; none until a list comes.
    chats: RwSignal<Option<Vec<Chat>>>,
    more: RwSignal<usize>,
    error: RwSignal<Option<String>>,
    /// The find box's words, and the keys typed in it (a find goes out a moment after the last).
    words: RwSignal<String>,
    typed: RwSignal<u64>,
    /// The chat being opened, and its spinner's turn.
    opening: RwSignal<Option<String>>,
    spin: RwSignal<u32>,
    /// The section is open.
    open: RwSignal<bool>,
}

impl crate::ext::State for Arena {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        Arena {
            sync: s.create_rw_signal(None),
            account: s.create_rw_signal(None),
            chats: s.create_rw_signal(None),
            more: s.create_rw_signal(0),
            error: s.create_rw_signal(None),
            words: s.create_rw_signal(String::new()),
            typed: s.create_rw_signal(0),
            opening: s.create_rw_signal(None),
            spin: s.create_rw_signal(0),
            open: s.create_rw_signal(true),
        }
    }
}

fn state(ui: Ui) -> Arena {
    ui.state::<Arena>()
}

/// Sets a signal only when the value is another.
fn put<T: PartialEq + 'static>(s: RwSignal<T>, v: T) {
    if s.with_untracked(|old| *old != v) {
        s.set(v);
    }
}

/// The chats asked for with the words in the box.
fn ask(ui: Ui, send: &Send) {
    let words = state(ui).words.get_untracked();
    send(json!({"type": "arena_chats", "query": words.trim()}));
}

/// A chat continued here, unless an answer is being written (then says why).
fn open_chat(ui: Ui, send: &Send, id: &str) {
    let st = state(ui);
    if ui.running.get_untracked() {
        toast::info(ui, BUSY);
        return;
    }
    if st.opening.with_untracked(Option::is_some) {
        return;
    }
    st.opening.set(Some(id.to_string()));
    turn(ui);
    send(json!({"type": "arena_chat_open", "id": id}));
}

/// The spinner turns while a chat is opened: one timer for the window.
fn turn(ui: Ui) {
    let st = state(ui);
    if st.opening.with_untracked(Option::is_none) {
        return;
    }
    st.spin.update(|n| *n = n.wrapping_add(1));
    exec_after(Duration::from_millis(80), move |_| turn(ui));
}

/// Every other session of the folder that Arena lacks, sent.
fn send_all(ui: Ui, send: &Send) {
    if state(ui).sync.with_untracked(Option::is_none) {
        toast::info(ui, "Sign in to Arena, with arena.syncChats on, to send the sessions there.");
        return;
    }
    send(json!({"type": "sessions_send_all"}));
}

/// The section under the sessions; it takes no room while the session is not kept with Arena.
pub fn section(ui: Ui, send: Send) -> AnyView {
    let (t, st) = (ui.colors, state(ui));
    let kept = create_memo(move |_| st.sync.with(Option::is_some));
    // The list is asked for the first time the section is open and the session is kept with Arena.
    {
        let send = send.clone();
        create_effect(move |asked: Option<bool>| {
            let due = kept.get() && st.open.get();
            if due && asked != Some(true) && st.chats.with_untracked(Option::is_none) {
                ask(ui, &send);
                return true;
            }
            asked.unwrap_or(false) || due
        });
    }
    // A find a moment after the last key.
    {
        let send = send.clone();
        create_effect(move |first: Option<()>| {
            st.words.with(|_| ());
            if first.is_none() {
                return;
            }
            st.typed.update(|g| *g += 1);
            let g = st.typed.get_untracked();
            let send = send.clone();
            exec_after(Duration::from_millis(300), move |_| {
                if st.typed.get_untracked() == g {
                    ask(ui, &send);
                }
            });
        });
    }
    let acts = {
        let (a, b) = (send.clone(), send.clone());
        h_stack((
            tooltip(action(t, "cloud-upload", move || send_all(ui, &a)), || label(|| "Send the Other Sessions Here")),
            tooltip(action(t, "refresh", move || ask(ui, &b)), || label(|| "Refresh")),
        ))
        .style(|s| s.gap(2.0))
    };
    let over = create_rw_signal(false);
    let head = h_stack((
        container(dyn_container(move || st.open.get(), |o| icon(if o { "chevron-down" } else { "chevron-right" }, 16.0)))
            .style(|s| s.flex_shrink(0.0_f32)),
        label(move || st.account.with(|a| heading(a.as_deref())).to_uppercase())
            .style(|s| bold(caps(s)).flex_grow(1.0_f32).min_width(0.0).text_ellipsis()),
        container(acts).style(move |s| s.flex_shrink(0.0_f32).apply_if(!over.get() || !st.open.get(), |s| s.hide())),
    ))
    .on_click_stop(move |_| st.open.update(|o| *o = !*o))
    .on_event_cont(EventListener::PointerEnter, move |_| over.set(true))
    .on_event_cont(EventListener::PointerLeave, move |_| over.set(false))
    .style(move |s| {
        let c = t.get();
        line(s, c.section_border, 't')
            .width_full()
            .height(22.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .gap(2.0)
            .padding_left(2.0)
            .padding_right(8.0)
            .background(c.section_bg)
            .color(c.section_fg)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
    });
    let focused = create_rw_signal(false);
    let find = text_input(st.words)
        .placeholder("Find a chat")
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
    let top = container(find).style(|s| s.width_full().padding_horiz(12.0).padding_vert(6.0));
    let items = move || {
        st.chats.with(|l| {
            st.error.with(|e| {
                st.words.with_untracked(|w| {
                    rows(l.as_deref(), st.more.get(), e.as_deref(), w).into_iter().collect::<im::Vector<_>>()
                })
            })
        })
    };
    let list = virtual_stack(
        VirtualDirection::Vertical,
        VirtualItemSize::Fixed(Box::new(|| 22.0)),
        items,
        Row::key,
        move |row| match row {
            Row::Chat(c) => chat_row(ui, send.clone(), c).into_any(),
            Row::Note(n) => note(ui, n, true).into_any(),
            Row::More(n) => note(ui, format!("{n} more: find one by its title."), false).into_any(),
        },
    )
    .style(|s| s.flex_col().width_full());
    let body = v_stack((top, scroll(list).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full())))
        .style(move |s| s.width_full().flex_grow(1.0_f32).min_height(0.0).apply_if(!st.open.get(), |s| s.hide()));
    v_stack((head, body))
        .style(move |s| {
            let open = st.open.get();
            s.width_full()
                .min_height(0.0)
                .apply_if(open, |s| s.flex_grow(1.0_f32).flex_basis(0.0))
                .apply_if(!kept.get(), |s| s.hide())
        })
        .into_any()
}

/// A line in the list's place; `cut`: what comes before its first ": ", the whole of it on hover (an error's
/// details).
fn note(ui: Ui, text: String, cut: bool) -> impl IntoView {
    let t = ui.colors;
    let short = text.split_once(": ").filter(|_| cut).map(|(a, _)| format!("{a}.")).unwrap_or_else(|| text.clone());
    let line = label(move || short.clone())
        .style(move |s| s.padding_horiz(20.0).height(22.0).items_center().text_ellipsis().color(t.get().desc));
    tooltip(line, move || {
        let text = text.clone();
        label(move || text.clone())
    })
}

fn chat_row(ui: Ui, send: Send, c: Chat) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let (title, said, id, place) = (title_of(&c), tip(&c), c.id.clone(), folder_name(&c.place));
    let opening = {
        let id = c.id.clone();
        create_memo(move |_| st.opening.with(|o| o.as_deref() == Some(id.as_str())))
    };
    let glyph = if c.terminal { "terminal" } else { "comment-discussion" };
    let mark = dyn_container(
        move || opening.get(),
        move |busy| {
            if busy {
                icon("loading", 16.0)
                    .style(move |s| {
                        s.apply_if(!crate::accessibility::still(ui), |s| {
                            s.rotate((st.spin.get() % 12) as f64 * std::f64::consts::TAU / 12.0)
                        })
                    })
                    .into_any()
            } else {
                icon(glyph, 16.0).into_any()
            }
        },
    );
    let row = h_stack((
        container(mark).style(|s| s.width(16.0).items_center().justify_center().flex_shrink(0.0_f32)),
        label(move || title.clone()).style(|s| s.min_width(0.0).flex_shrink(1.0_f32).text_ellipsis()),
        label(move || place.clone()).style(move |s| {
            s.min_width(0.0).flex_shrink(10.0_f32).flex_grow(1.0_f32).text_ellipsis().font_size(12.0).color(t.get().desc)
        }),
    ))
    .on_click_stop(move |_| open_chat(ui, &send, &id))
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
            .apply_if(busy, |s| s.color(c.desc))
            .apply_if(!busy, |s| s.cursor(CursorStyle::Pointer).hover(|s| s.background(c.list_hover)))
    });
    tooltip(row, move || {
        let said = if ui.running.get_untracked() { BUSY.to_string() } else { said.clone() };
        label(move || said.clone())
    })
}

/// The chat header's mark: where the session stands with its chat in Arena. A click shows the Sessions view.
pub fn mark(ui: Ui) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let now = create_memo(move |_| st.sync.with(|s| s.as_ref().and_then(mark_of)));
    let glyph = create_memo(move |_| now.with(|m| m.as_ref().map(|m| m.0).unwrap_or("cloud")));
    let trouble = create_memo(move |_| glyph.get() == "warning");
    let body = h_stack((
        dyn_container(move || glyph.get(), |g| icon(g, 14.0)),
        // In step is the cloud alone (its words on hover), so that the view's tabs keep their room.
        label(move || now.with(|m| m.as_ref().map(|m| m.1.clone()).unwrap_or_default()))
            .style(move |s| s.apply_if(glyph.get() == "cloud", |s| s.hide())),
    ))
    .on_click_stop(move |_| ui.reveal(View::Sessions))
    .style(move |s| {
        let c = t.get();
        s.height(22.0)
            .items_center()
            .gap(4.0)
            .padding_horiz(5.0)
            .border_radius(5.0)
            .font_size(11.0)
            .color(if trouble.get() { c.warning } else { c.desc })
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.toolbar_hover))
            .apply_if(now.with(Option::is_none), |s| s.hide())
    });
    tooltip(body, move || label(move || now.with(|m| m.as_ref().map(|m| m.2.clone()).unwrap_or_default())))
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    add(command(SEND_ALL, "Sessions", "Send the Other Sessions Here", send_all)
        .when(|ui| state(ui).sync.with_untracked(Option::is_some)));
    add(command(REFRESH, "Sessions", "Refresh Arena's Chats", ask).hidden());
}

/// The session's messages that are this part's, taken in. Never taken away (false): the Sessions view reads
/// `state` too.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    match m["type"].as_str().unwrap_or("") {
        // Another session, or the thread again: its standing, and the list again (the chat now worked in leaves it).
        "state" => {
            put(st.sync, standing_of(&m["sync"]));
            if st.opening.with_untracked(Option::is_some) {
                st.opening.set(None);
            }
            if st.sync.with_untracked(Option::is_some) && st.chats.with_untracked(Option::is_some) {
                ask(ui, send);
            }
        }
        "status" if m.get("sync").is_some() => put(st.sync, standing_of(&m["sync"])),
        "session_sync" => put(st.sync, standing_of(&m["sync"])),
        "arena_chats" => {
            let words = m["query"].as_str().unwrap_or("");
            if st.words.with_untracked(|w| w.trim() == words) {
                let error = m["error"].as_str().map(str::to_string);
                put(st.chats, if error.is_some() { None } else { Some(chats_of(m)) });
                put(st.more, m["more"].as_u64().unwrap_or(0) as usize);
                put(st.error, error);
            }
            if let Some(a) = m["account"].as_str().filter(|a| !a.is_empty()) {
                put(st.account, Some(a.to_string()));
            }
        }
        "arena_chat_opened" => {
            if m["error"].is_string() || st.opening.with_untracked(|o| o.as_deref() == m["id"].as_str()) {
                st.opening.set(None);
            }
        }
        "refused" if m["of"] == "arena_chat_open" => st.opening.set(None),
        // The marks of the sessions sent are other now.
        "sessions_sent" => send(json!({"type": "sessions"})),
        "session_notice" => {
            let text = m["text"].as_str().unwrap_or("");
            if !text.is_empty() {
                if m["problem"] == true {
                    toast::warning(ui, text);
                } else {
                    toast::info(ui, text);
                }
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing() -> Standing {
        Standing { chat: Some("c1".into()), account: Some("Ali".into()), ..Standing::default() }
    }

    #[test]
    fn the_mark_says_the_worst_first_and_nothing_when_there_is_nothing_to_say() {
        let s = standing();
        let (g, short, full) = mark_of(&s).expect("in step");
        assert_eq!((g, short.as_str()), ("cloud", "In step"));
        assert!(full.contains("(Ali)"));
        let waiting = Standing { waiting: 3, ..s.clone() };
        assert_eq!(mark_of(&waiting).map(|m| m.1), Some("3 to send".into()));
        let one = Standing { waiting: 1, ..s.clone() };
        assert!(mark_of(&one).is_some_and(|m| m.2.starts_with("1 message is")));
        let news = Standing { behind: true, ..waiting.clone() };
        assert_eq!(mark_of(&news).map(|m| m.1), Some("Web news".into()));
        let trouble = Standing { trouble: Some("connection refused".into()), ..news };
        let (g, short, full) = mark_of(&trouble).expect("trouble");
        assert_eq!((g, short.as_str()), ("warning", "Not in step"));
        assert!(full.contains("connection refused"));
        assert_eq!(mark_of(&Standing { here: true, ..trouble }), None);
        assert_eq!(mark_of(&Standing::default()), None, "no chat yet");
    }

    #[test]
    fn the_standing_is_read_whole_and_none_without_arena() {
        let v = json!({"chat": "c1", "url": "https://a/chat/c1", "account": "Ali", "waiting": 2, "trouble": null,
            "behind": true, "here": false});
        let s = standing_of(&v).expect("kept");
        assert_eq!((s.chat.as_deref(), s.waiting, s.behind, s.here), (Some("c1"), 2, true, false));
        assert_eq!((s.trouble, s.account.as_deref()), (None, Some("Ali")));
        assert_eq!(standing_of(&Value::Null), None);
    }

    #[test]
    fn the_list_says_why_it_is_empty_and_how_many_more_there_are() {
        assert_eq!(rows(None, 0, None, ""), vec![Row::Note("Loading chats…".into())]);
        assert_eq!(rows(Some(&[]), 0, None, ""), vec![Row::Note("No chats in Arena yet.".into())]);
        assert_eq!(rows(Some(&[]), 0, None, "parser"), vec![Row::Note("No chats found.".into())]);
        assert_eq!(rows(Some(&[]), 0, Some("Sign in"), ""), vec![Row::Note("Sign in".into())]);
        let c = Chat { id: "a".into(), ..Chat::default() };
        assert_eq!(rows(Some(&[c.clone()]), 4, None, ""), vec![Row::Chat(c), Row::More(4)]);
    }

    #[test]
    fn a_chat_reads_whole_with_its_words_for_the_pointer() {
        let m = json!({"type": "arena_chats", "items": [{"id": "c", "title": "", "place": "/home/me/repo/",
            "messages": 1, "terminal": true}]});
        let list = chats_of(&m);
        assert_eq!(list.len(), 1);
        assert!(list[0].terminal);
        assert_eq!(title_of(&list[0]), "Untitled chat");
        assert!(tip(&list[0]).ends_with("· 1 message · /home/me/repo/"));
        assert_eq!(folder_name(&list[0].place), "repo");
        assert_eq!(heading(Some("Ali")), "Ali's chats");
        assert_eq!(heading(Some(" ")), "Chats in Arena");
    }
}
