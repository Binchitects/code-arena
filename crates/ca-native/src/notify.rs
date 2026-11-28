//! What the agent tells the person while they look elsewhere: it waits for them (a call to allow), it is ready (the
//! turn ended), or the turn failed. While the window is not in front or the Agent view is not on screen, each is a
//! bubble in the window's corner ("Code Arena tells you", four at most), answered there: Allow, Always or Deny; a
//! reply sent at once. Ready and failed go by themselves after 20 seconds; a click on a bubble's title shows the Agent
//! view. While the window is hidden (minimized), and once Desktop Notifications is on in the Manage menu, the desktop
//! says it too (`notify-send`, `osascript`, PowerShell), and the window asks for the person's attention whenever a
//! bubble comes while it is not in front.

use std::collections::HashSet;
use std::time::Duration;

use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, dyn_stack, empty, h_stack, label, text_input, v_stack};
use floem::{AnyView, IntoView, Urgency, View as _, ViewId, WindowIdExt};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layers::{self, Place};
use crate::layout::View;
use crate::state::{Send, Ui};
use crate::widgets::{bold, button};

/// The Manage menu's Desktop Notifications: turned on, or off.
pub const TOGGLE_DESKTOP: &str = "codeArena.toggleDesktopNotifications";

/// The bubbles in the corner at most, the newest kept.
const MOST: usize = 4;
/// How long a bubble that asks nothing stays.
const QUIET: Duration = Duration::from_secs(20);
const WIDTH: f64 = 320.0;
/// What a desktop notification says at most, in characters.
const DESKTOP_MOST: usize = 200;

/// What a bubble is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum What {
    /// A call waits for a yes: its number, as the session's `ask` says it.
    Waiting {
        call: u64,
    },
    Ready,
    Failed,
}

/// A bubble: what it is about, what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub id: u64,
    pub what: What,
    pub title: String,
    pub detail: String,
}

/// What a message from the session tells the person, when it tells them anything: a call to allow, the turn ended
/// (a turn the person stopped tells them nothing).
pub fn said(m: &Value) -> Option<(What, String, String)> {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    match m["type"].as_str().unwrap_or("") {
        "ask" => Some((
            What::Waiting { call: m["id"].as_u64().unwrap_or(0) },
            format!("Waiting for you: {}", s("tool")),
            s("subject"),
        )),
        "turn_end" if m["ok"] == true => Some((
            What::Ready,
            "Ready: the agent finished".into(),
            "Its answer is in the chat; reply here, or open it.".into(),
        )),
        "turn_end" if m["stopped"] == true => None,
        "turn_end" => {
            let error = s("error");
            Some((
                What::Failed,
                "The turn failed".into(),
                if error.is_empty() { "The turn failed.".into() } else { error },
            ))
        }
        _ => None,
    }
}

/// A bubble put in the corner: in the place of one about the same call, the oldest gone past four.
pub fn put(list: &mut Vec<Notice>, n: Notice) {
    list.retain(|x| !matches!((&x.what, &n.what), (What::Waiting { call: a }, What::Waiting { call: b }) if a == b));
    list.push(n);
    let over = list.len().saturating_sub(MOST);
    list.drain(..over);
}

/// Whether the person sees the chat now: the window in front, the Agent view on screen. The thread says it then.
pub fn looking(window_focused: bool, agent_shown: bool) -> bool {
    window_focused && agent_shown
}

/// The program and its arguments that say a notification on this desktop (`std::env::consts::OS`), none where
/// there is no known way. Nothing goes through a shell: the words are arguments, and where a script holds them
/// (AppleScript, PowerShell) they are quoted as its strings.
pub fn desktop_command(os: &str, title: &str, body: &str) -> Option<(String, Vec<String>)> {
    let body: String = body.chars().take(DESKTOP_MOST).collect();
    match os {
        "macos" => {
            let apple = |t: &str| format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""));
            let script =
                format!("display notification {} with title \"Code Arena\" subtitle {}", apple(&body), apple(title));
            Some(("osascript".into(), vec!["-e".into(), script]))
        }
        "windows" => {
            let ps = |t: &str| format!("'{}'", t.replace('\'', "''"));
            let script = format!(
                "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
                 $t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
                 $x = $t.GetElementsByTagName('text'); \
                 $x.Item(0).AppendChild($t.CreateTextNode({})) > $null; \
                 $x.Item(1).AppendChild($t.CreateTextNode({})) > $null; \
                 [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show([Windows.UI.Notifications.ToastNotification]::new($t))",
                ps(title),
                ps(&body)
            );
            Some(("powershell".into(), vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), script]))
        }
        "linux" | "freebsd" | "openbsd" | "netbsd" | "dragonfly" => {
            Some(("notify-send".into(), vec!["--app-name=Code Arena".into(), "--".into(), title.to_string(), body]))
        }
        _ => None,
    }
}

/// The corner's bubbles and what they need to know.
#[derive(Clone, Copy)]
pub struct Notices {
    list: RwSignal<Vec<Notice>>,
    /// The calls answered (here, in the thread, on another face) or gone with their turn: never asked of again.
    done: RwSignal<HashSet<u64>>,
    /// Whether the window is in front; taken to be until it says otherwise.
    focused: RwSignal<bool>,
    /// The window's attention asked for, to be given back when it comes to the front.
    attention: RwSignal<bool>,
    /// A view of the window's, which knows the window.
    watcher: RwSignal<Option<ViewId>>,
    next: RwSignal<u64>,
}

impl crate::ext::State for Notices {
    fn new(ui: Ui) -> Self {
        Notices {
            list: ui.scope.create_rw_signal(Vec::new()),
            done: ui.scope.create_rw_signal(HashSet::new()),
            focused: ui.scope.create_rw_signal(true),
            attention: ui.scope.create_rw_signal(false),
            watcher: ui.scope.create_rw_signal(None),
            next: ui.scope.create_rw_signal(1),
        }
    }
}

/// The bubbles in the corner now (for the tests).
#[cfg(test)]
fn bubbles(ui: Ui) -> Vec<Notice> {
    ui.state::<Notices>().list.get_untracked()
}

/// A bubble gone, if it is there.
pub fn drop_notice(ui: Ui, id: u64) {
    let s = ui.state::<Notices>();
    if s.list.with_untracked(|l| l.iter().any(|n| n.id == id)) {
        s.list.update(|l| l.retain(|n| n.id != id));
    }
}

/// A call answered: never asked of again, its bubble gone.
fn answered(ui: Ui, call: u64) {
    let s = ui.state::<Notices>();
    if s.done.with_untracked(|d| !d.contains(&call)) {
        s.done.update(|d| {
            d.insert(call);
        });
    }
    let of = |n: &Notice| n.what == What::Waiting { call };
    if s.list.with_untracked(|l| l.iter().any(of)) {
        s.list.update(|l| l.retain(|n| !of(n)));
    }
}

/// Said to the person, unless they look at the chat.
fn tell(ui: Ui, what: What, title: String, detail: String) {
    let s = ui.state::<Notices>();
    if looking(s.focused.get_untracked(), ui.view_shown(View::Agent)) {
        return;
    }
    if let What::Waiting { call } = what
        && s.done.with_untracked(|d| d.contains(&call))
    {
        return;
    }
    let id = s.next.get_untracked();
    s.next.set(id + 1);
    let quiet = !matches!(what, What::Waiting { .. });
    s.list.update(|l| put(l, Notice { id, what, title: title.clone(), detail: detail.clone() }));
    if quiet {
        floem::action::exec_after(QUIET, move |_| drop_notice(ui, id));
    }
    tell_desktop(ui, &title, &detail);
}

/// The window's attention asked for while it is not in front, and the desktop told while it is hidden and the
/// person has Desktop Notifications on.
fn tell_desktop(ui: Ui, title: &str, detail: &str) {
    let s = ui.state::<Notices>();
    let Some(window) = s.watcher.get_untracked().and_then(|v| v.window_id()) else { return };
    if !s.focused.get_untracked() {
        window.request_attention(Urgency::Informational);
        if !s.attention.get_untracked() {
            s.attention.set(true);
        }
    }
    let on = crate::settings::now(ui, "codeArena.desktopNotifications", None) == true;
    if !on || !(window.is_minimized() || !window.is_visible()) {
        return;
    }
    if let Some((program, args)) = desktop_command(std::env::consts::OS, title, detail) {
        std::thread::spawn(move || {
            let _ = std::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }
}

/// The chat shown, and its bubble gone.
fn open_chat(ui: Ui, id: u64) {
    drop_notice(ui, id);
    ui.reveal(View::Agent);
    crate::chat_bridge::focus(ui);
}

/// A bubble drawn: its icon, title and detail (a click shows the chat), a cross; then Allow, Always, Deny for a call,
/// a reply for a finished turn.
fn bubble(ui: Ui, send: Send, n: Notice) -> impl IntoView {
    let t = ui.colors;
    let id = n.id;
    let mark = match n.what {
        What::Waiting { .. } => "shield",
        What::Ready => "pass",
        What::Failed => "error",
    };
    let what = n.what.clone();
    let (title, detail) = (n.title.clone(), n.detail.clone());
    let no_detail = detail.is_empty();
    let head = h_stack((
        icon(mark, 16.0).style(move |s| {
            let c = t.get();
            let colour = match what {
                What::Waiting { .. } => c.notify_warning,
                What::Ready => c.ok,
                What::Failed => c.notify_error,
            };
            s.margin_top(2.0).flex_shrink(0.0_f32).color(colour)
        }),
        v_stack((
            label(move || title.clone()).style(|s| bold(s).text_overflow(TextOverflow::Wrap)),
            label(move || detail.clone()).style(move |s| {
                s.font_size(12.0)
                    .color(t.get().desc)
                    .text_overflow(TextOverflow::Wrap)
                    .apply_if(no_detail, |s| s.hide())
            }),
        ))
        .on_click_stop(move |_| open_chat(ui, id))
        .style(|s| {
            s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).gap(2.0).margin_left(8.0).cursor(CursorStyle::Pointer)
        }),
        container(icon("close", 16.0)).on_click_stop(move |_| drop_notice(ui, id)).style(move |s| {
            let c = t.get();
            s.width(22.0)
                .height(22.0)
                .margin_left(4.0)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
                .border_radius(5.0)
                .cursor(CursorStyle::Pointer)
                .color(c.icon)
                .hover(|s| s.background(c.toolbar_hover))
        }),
    ))
    .style(|s| s.width_full().items_start());
    let foot = match n.what {
        What::Waiting { call } => {
            let reply = move |a: &'static str| {
                let send = send.clone();
                move || {
                    answered(ui, call);
                    send(json!({"type": "answer", "id": call, "answer": a}));
                }
            };
            h_stack((
                button(t, "Allow", true, reply("yes")),
                button(t, "Always", false, reply("always")),
                button(t, "Deny", false, reply("no")),
            ))
            .style(|s| s.gap(6.0).margin_top(8.0))
            .into_any()
        }
        What::Ready => {
            let text = floem::reactive::create_rw_signal(String::new());
            let go = {
                let send = send.clone();
                move || {
                    let said = text.get_untracked().trim().to_string();
                    if !said.is_empty() {
                        drop_notice(ui, id);
                        crate::chat_bridge::ask_now(ui, &send, &said);
                    }
                }
            };
            let enter = go.clone();
            let input = text_input(text)
                .placeholder("Reply…")
                .on_event(EventListener::KeyDown, move |e| {
                    if let Event::KeyDown(k) = e {
                        match &k.key.logical_key {
                            Key::Named(NamedKey::Enter) => enter(),
                            Key::Named(NamedKey::Escape) => drop_notice(ui, id),
                            _ => {}
                        }
                    }
                    // The reply's keys are its own, none the workbench's.
                    EventPropagation::Stop
                })
                .style(move |s| {
                    let c = t.get();
                    let s = s
                        .flex_grow(1.0_f32)
                        .flex_basis(0.0)
                        .min_width(0.0)
                        .height(26.0)
                        .padding_horiz(6.0)
                        .font_size(13.0)
                        .border(1.0)
                        .border_radius(2.0)
                        .border_color(c.input_border)
                        .background(c.input_bg)
                        .color(c.input_fg)
                        .cursor_color(floem::peniko::Brush::Solid(c.cursor));
                    crate::widgets::steady_input(s, c.input_bg, Some(c.focus))
                });
            h_stack((input, button(t, "Send", true, go)))
                .style(|s| s.gap(6.0).items_center().margin_top(8.0))
                .into_any()
        }
        What::Failed => empty().style(|s| s.hide()).into_any(),
    };
    v_stack((head, foot)).style(move |s| {
        let c = t.get();
        s.width_full()
            .margin_top(8.0)
            .padding(10.0)
            .font_size(13.0)
            .color(c.notify_fg)
            .border(1.0)
            .border_color(c.notify_outline)
            .border_radius(4.0)
            .background(c.notify_bg)
            .box_shadow_blur(8.0)
            .box_shadow_spread(2.0)
            .box_shadow_color(c.shadow)
    })
}

/// The corner: the bubbles, newest last; it takes no room while there are none.
fn corner(ui: Ui, send: Send) -> AnyView {
    let s = ui.state::<Notices>();
    let none = ui.scope.create_memo(move |_| s.list.with(Vec::is_empty));
    dyn_stack(move || s.list.get(), |n: &Notice| n.id, move |n| bubble(ui, send.clone(), n))
        .style(move |st| {
            st.flex_col()
                .absolute()
                .inset_right(8.0)
                .inset_bottom(30.0)
                .width(WIDTH)
                .max_width((ui.window.get().0 - 16.0).max(100.0))
                .apply_if(none.get(), |st| st.hide())
        })
        .into_any()
}

/// A view that takes no room and is never hidden: it hears the window come to the front and go behind, and knows
/// the window.
fn watcher(ui: Ui, _send: Send) -> AnyView {
    let s = ui.state::<Notices>();
    let v = empty();
    let vid = v.id();
    s.watcher.set(Some(vid));
    v.on_event_cont(EventListener::WindowGotFocus, move |_| {
        if !s.focused.get_untracked() {
            s.focused.set(true);
        }
        if s.attention.get_untracked() {
            s.attention.set(false);
            if let Some(w) = vid.window_id() {
                w.request_attention(Urgency::Default);
            }
        }
    })
    .on_event_cont(EventListener::WindowLostFocus, move |_| {
        if s.focused.get_untracked() {
            s.focused.set(false);
        }
    })
    .style(|s| s.absolute().width(0.0).height(0.0))
    .into_any()
}

/// What it adds to the window's registries, once at start: the corner over the window (above the toasts, so a call
/// to allow is never under one), what hears the window's focus, and the Manage menu's Desktop Notifications.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{Menu, item};
    layers::add(Place::Over, "notify.watch", 0, watcher);
    layers::add(Place::Over, "notify.bubbles", 55, corner);
    add(command(TOGGLE_DESKTOP, "Code Arena", "Toggle Desktop Notifications", |ui, send| {
        let on = crate::settings::now(ui, "codeArena.desktopNotifications", None) == true;
        send(json!({"type": "set_setting", "key": "codeArena.desktopNotifications", "value": !on}));
    }));
    crate::menus::add(
        Menu::Manage,
        item(TOGGLE_DESKTOP)
            .title("Desktop Notifications")
            .group("6_notifications")
            .checked(|ui| crate::settings::now(ui, "codeArena.desktopNotifications", None) == true),
    );
}

/// The session's messages that tell the person something (a call to allow, the turn's end), and those that answer
/// a call, taken in. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "answered" => answered(ui, m["id"].as_u64().unwrap_or(0)),
        "turn_end" => {
            // The calls not answered are no longer asked (the thread says so): their bubbles go.
            let waiting: Vec<u64> = ui.state::<Notices>().list.with_untracked(|l| {
                l.iter().filter_map(|n| if let What::Waiting { call } = n.what { Some(call) } else { None }).collect()
            });
            for call in waiting {
                answered(ui, call);
            }
        }
        _ => {}
    }
    if let Some((what, title, detail)) = said(m) {
        tell(ui, what, title, detail);
    }
    false
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    fn notice(id: u64, what: What) -> Notice {
        Notice { id, what, title: String::new(), detail: String::new() }
    }

    #[test]
    fn what_the_session_says_is_told_as_vs_code_words_say_it() {
        let ask = said(&json!({"type": "ask", "id": 7, "tool": "run_shell", "subject": "ls"})).expect("said");
        assert_eq!(ask, (What::Waiting { call: 7 }, "Waiting for you: run_shell".into(), "ls".into()));
        assert_eq!(said(&json!({"type": "turn_end", "ok": true})).expect("said").1, "Ready: the agent finished");
        let failed = said(&json!({"type": "turn_end", "ok": false, "error": "no model"})).expect("said");
        assert_eq!((failed.0, failed.2.as_str()), (What::Failed, "no model"));
        assert_eq!(said(&json!({"type": "turn_end", "ok": false})).expect("said").2, "The turn failed.");
        assert!(
            said(&json!({"type": "turn_end", "ok": false, "stopped": true})).is_none(),
            "a stop is the person's own"
        );
        assert!(said(&json!({"type": "text", "text": "hi"})).is_none());
    }

    #[test]
    fn four_bubbles_at_most_and_one_per_call() {
        let mut l = Vec::new();
        for id in 1..=5 {
            put(&mut l, notice(id, What::Ready));
        }
        assert_eq!(l.iter().map(|n| n.id).collect::<Vec<_>>(), [2, 3, 4, 5]);
        put(&mut l, notice(6, What::Waiting { call: 9 }));
        put(&mut l, notice(7, What::Waiting { call: 9 }));
        assert_eq!(l.iter().map(|n| n.id).collect::<Vec<_>>(), [3, 4, 5, 7]);
    }

    #[test]
    fn the_chat_looked_at_only_with_the_window_in_front_and_the_view_shown() {
        assert!(looking(true, true));
        assert!(!looking(false, true) && !looking(true, false) && !looking(false, false));
    }

    #[test]
    fn each_desktop_has_its_command_with_the_words_quoted() {
        let (p, a) = desktop_command("linux", "Ready: the agent finished", "it's \"done\"").expect("said");
        assert_eq!(p, "notify-send");
        assert_eq!(a, ["--app-name=Code Arena", "--", "Ready: the agent finished", "it's \"done\""]);
        let (p, a) = desktop_command("macos", "Waiting for you: run_shell", "say \"hi\" \\ there").expect("said");
        assert_eq!(p, "osascript");
        assert_eq!(
            a[1],
            "display notification \"say \\\"hi\\\" \\\\ there\" with title \"Code Arena\" subtitle \"Waiting for you: run_shell\""
        );
        let (p, a) = desktop_command("windows", "The turn failed", "it's gone").expect("said");
        assert_eq!(p, "powershell");
        assert!(a[3].contains("CreateTextNode('The turn failed')") && a[3].contains("CreateTextNode('it''s gone')"));
        assert!(desktop_command("plan9", "a", "b").is_none());
        // Cut by characters: a long error that is not ASCII does not break in a character.
        let long = "é".repeat(300);
        let (_, a) = desktop_command("linux", "t", &long).expect("said");
        assert_eq!(a[3].chars().count(), DESKTOP_MOST);
    }

    #[test]
    fn a_bubble_is_raised_while_the_chat_is_not_looked_at_and_goes_when_answered() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        layout_hidden(ui);
        take(ui, &json!({"type": "ask", "id": 3, "tool": "run_shell", "subject": "ls"}), &send);
        assert_eq!(bubbles(ui).len(), 1);
        take(ui, &json!({"type": "answered", "id": 3, "answer": "yes"}), &send);
        assert!(bubbles(ui).is_empty());
        // Answered once, never asked again (a face that came back says it again).
        take(ui, &json!({"type": "ask", "id": 3, "tool": "run_shell", "subject": "ls"}), &send);
        assert!(bubbles(ui).is_empty());
        // A call not answered goes with its turn, which tells its end.
        take(ui, &json!({"type": "ask", "id": 4, "tool": "edit_file", "subject": "a.rs"}), &send);
        take(ui, &json!({"type": "turn_end", "ok": true}), &send);
        assert_eq!(bubbles(ui).iter().map(|n| n.what.clone()).collect::<Vec<_>>(), [What::Ready]);
    }

    #[test]
    fn nothing_is_raised_while_the_chat_is_looked_at() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        ui.reveal(View::Agent);
        assert!(ui.view_shown(View::Agent));
        take(ui, &json!({"type": "turn_end", "ok": false, "error": "x"}), &send);
        assert!(bubbles(ui).is_empty());
        ui.state::<Notices>().focused.set(false);
        take(ui, &json!({"type": "turn_end", "ok": false, "error": "x"}), &send);
        assert_eq!(bubbles(ui).len(), 1);
    }

    #[test]
    fn the_manage_menu_checks_desktop_notifications_by_the_setting() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        register(ui, &send);
        let row = crate::menus::rows(ui, crate::menus::Menu::Manage).into_iter().find_map(|r| match r {
            crate::menus::Row::Entry { title, checked, command, .. } if title == "Desktop Notifications" => {
                Some((checked, command))
            }
            _ => None,
        });
        assert_eq!(row, Some((false, Some(TOGGLE_DESKTOP.to_string()))));
    }

    /// The Agent view off screen.
    fn layout_hidden(ui: Ui) {
        if ui.view_shown(View::Agent) {
            ui.toggle_view(View::Agent);
        }
        assert!(!ui.view_shown(View::Agent));
    }
}
