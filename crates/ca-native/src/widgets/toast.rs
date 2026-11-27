//! What the window says in its corner, as VS Code's notifications: a toast at the bottom right (its kind's icon,
//! what it says, its buttons, a cross), at most three at a time, each going by itself after a while; and the list
//! of everything said, which the status bar's bell opens (Clear All, Hide). A toast that has gone from the corner
//! stays in the list until it is cleared.
//!
//! ```ignore
//! toast::info(ui, "Copied the address.");
//! toast::error(ui, &format!("Could not rename {path}: {e}"));
//! let id = toast::show(ui, toast::warning_of("Deleted notes.txt: it is kept in the backups.")
//!     .source("Explorer")
//!     .action("Restore", move |ui, send| restore(ui, send)));
//! toast::dismiss(ui, id);
//! ```
//!
//! It may be said from anywhere (a click, a key, a message taken in, a timer). How long a toast stays is VS Code's:
//! 15 s for what is told, 18 s for a warning, 20 s for an error; it stays while the pointer is on it, and for good
//! when it is `sticky` or is an error with buttons. A button runs and the notification is cleared. The toasts and
//! the list are in the main window.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::rc::Rc;
use std::time::Duration;

use floem::action::exec_after;
use floem::event::EventListener;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_rw_signal};
use floem::style::{CursorStyle, FlexWrap, TextOverflow};
use floem::views::{Decorators, container, dyn_stack, empty, h_stack, h_stack_from_iter, label, scroll, v_stack};
use floem::{AnyView, IntoView};
use serde_json::Value;

use crate::colors::Colors;
use crate::icons::icon;
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::widgets::{action, caps};

/// The list shown or hidden, and everything in it cleared: VS Code's commands.
pub const SHOW_LIST: &str = "notifications.showList";
pub const HIDE_LIST: &str = "notifications.hideList";
pub const CLEAR_ALL: &str = "notifications.clearAll";

/// The toasts in the corner at most (VS Code's three), and their width.
const MOST: usize = 3;
const WIDTH: f64 = 450.0;

/// How grave what is said is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    fn icon(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }

    fn colour(self, c: &Colors) -> floem::peniko::Color {
        match self {
            Severity::Info => c.notify_info,
            Severity::Warning => c.notify_warning,
            Severity::Error => c.notify_error,
        }
    }

    /// How long its toast stays in the corner, as VS Code's.
    fn stays(self) -> Duration {
        Duration::from_secs(match self {
            Severity::Info => 15,
            Severity::Warning => 18,
            Severity::Error => 20,
        })
    }
}

/// What a button of a notification does.
pub type Run = Rc<dyn Fn(Ui, &Send)>;

/// Something to say, as it is being made.
#[derive(Clone)]
pub struct Toast {
    severity: Severity,
    text: String,
    /// Who says it ("Explorer", "Git"), small under the words; none: nothing.
    source: String,
    actions: Vec<(String, Run)>,
    sticky: bool,
}

/// Something to say, of a kind.
pub fn of(severity: Severity, text: &str) -> Toast {
    Toast { severity, text: text.into(), source: String::new(), actions: Vec::new(), sticky: false }
}

pub fn info_of(text: &str) -> Toast {
    of(Severity::Info, text)
}

pub fn warning_of(text: &str) -> Toast {
    of(Severity::Warning, text)
}

pub fn error_of(text: &str) -> Toast {
    of(Severity::Error, text)
}

impl Toast {
    /// One more button, after the ones before it. It runs, and the notification is cleared.
    pub fn action(mut self, label: &str, run: impl Fn(Ui, &Send) + 'static) -> Toast {
        self.actions.push((label.into(), Rc::new(run)));
        self
    }

    /// Who says it.
    pub fn source(mut self, source: &str) -> Toast {
        self.source = source.into();
        self
    }

    /// Its toast stays in the corner until it is closed.
    pub fn sticky(mut self) -> Toast {
        self.sticky = true;
        self
    }

    /// Says it (as `toast::show`): its id.
    pub fn show_in(self, ui: Ui) -> u64 {
        show(ui, self)
    }

    /// How long its toast stays; none: until it is closed (said so, or an error with buttons, as VS Code's).
    fn stays(&self) -> Option<Duration> {
        let kept = self.sticky || (self.severity == Severity::Error && !self.actions.is_empty());
        (!kept).then(|| self.severity.stays())
    }
}

/// A notification in the list.
struct Note {
    id: u64,
    toast: Toast,
}

/// Everything said and not cleared, the oldest first; which of them are toasts in the corner now; whether the
/// list is open; how many were said since it was last open (the bell's dot); and the toast the pointer is on.
#[derive(Clone, Copy)]
struct Notes {
    all: RwSignal<Vec<Rc<Note>>>,
    toasts: RwSignal<Vec<u64>>,
    open: RwSignal<bool>,
    fresh: RwSignal<usize>,
    next: RwSignal<u64>,
    pointed: RwSignal<Option<u64>>,
}

impl crate::ext::State for Notes {
    fn new(ui: Ui) -> Notes {
        Notes {
            all: ui.scope.create_rw_signal(Vec::new()),
            toasts: ui.scope.create_rw_signal(Vec::new()),
            open: ui.scope.create_rw_signal(false),
            fresh: ui.scope.create_rw_signal(0),
            next: ui.scope.create_rw_signal(1),
            pointed: ui.scope.create_rw_signal(None),
        }
    }
}

/// Sets a signal only when its value changes.
fn put<T: PartialEq + 'static>(signal: RwSignal<T>, value: T) {
    if signal.with_untracked(|was| *was != value) {
        signal.set(value);
    }
}

/// A notification taken into the list and, unless the list is open, shown as a toast: its id, and how long its
/// toast stays (none: until closed).
fn add(ui: Ui, toast: Toast) -> (u64, Option<Duration>) {
    let s = ui.state::<Notes>();
    let id = s.next.get_untracked();
    s.next.set(id + 1);
    let stays = toast.stays();
    s.all.update(|all| all.push(Rc::new(Note { id, toast })));
    if s.open.get_untracked() {
        // The list is open: it is seen there, and is not new.
        return (id, None);
    }
    s.fresh.update(|n| *n += 1);
    s.toasts.update(|t| t.push(id));
    (id, stays)
}

/// The toast gone from the corner once its time is up; while the pointer is on it, a second more.
fn fade(ui: Ui, id: u64, after: Duration) {
    exec_after(after, move |_| {
        let s = ui.state::<Notes>();
        if s.toasts.with_untracked(|t| !t.contains(&id)) {
            return;
        }
        if s.pointed.get_untracked() == Some(id) {
            fade(ui, id, Duration::from_secs(1));
        } else {
            hide(ui, id);
        }
    });
}

/// Says something: a toast in the corner, kept in the list. Gives the id it is dismissed by.
pub fn show(ui: Ui, toast: Toast) -> u64 {
    let (id, stays) = add(ui, toast);
    if let Some(stays) = stays {
        fade(ui, id, stays);
    }
    id
}

pub fn info(ui: Ui, text: &str) -> u64 {
    show(ui, info_of(text))
}

pub fn warning(ui: Ui, text: &str) -> u64 {
    show(ui, warning_of(text))
}

pub fn error(ui: Ui, text: &str) -> u64 {
    show(ui, error_of(text))
}

/// A toast gone from the corner; it stays in the list.
pub fn hide(ui: Ui, id: u64) {
    let s = ui.state::<Notes>();
    if s.toasts.with_untracked(|t| t.contains(&id)) {
        s.toasts.update(|t| t.retain(|x| *x != id));
    }
    if s.pointed.get_untracked() == Some(id) {
        s.pointed.set(None);
    }
}

/// A notification cleared: gone from the corner and from the list.
pub fn dismiss(ui: Ui, id: u64) {
    let s = ui.state::<Notes>();
    hide(ui, id);
    if s.all.with_untracked(|all| all.iter().any(|n| n.id == id)) {
        s.all.update(|all| all.retain(|n| n.id != id));
        // No more new ones than there are.
        let most = s.all.with_untracked(Vec::len);
        if s.fresh.get_untracked() > most {
            s.fresh.set(most);
        }
    }
}

/// Every notification cleared, and the list closed.
pub fn clear_all(ui: Ui) {
    let s = ui.state::<Notes>();
    put(s.toasts, Vec::new());
    put(s.pointed, None);
    if s.all.with_untracked(|all| !all.is_empty()) {
        s.all.set(Vec::new());
    }
    put(s.fresh, 0);
    put(s.open, false);
}

/// The list opened (the toasts leave the corner: they are in it) or closed.
pub fn show_list(ui: Ui, open: bool) {
    let s = ui.state::<Notes>();
    if open {
        put(s.toasts, Vec::new());
        put(s.pointed, None);
        put(s.fresh, 0);
    }
    put(s.open, open);
}

/// Whether the list is open.
pub fn list_open(ui: Ui) -> bool {
    ui.state::<Notes>().open.get_untracked()
}

/// How many notifications are in the list.
pub fn count(ui: Ui) -> usize {
    ui.state::<Notes>().all.with_untracked(Vec::len)
}

/// What the bell says under the pointer, as VS Code's.
fn bell_tip(open: bool, all: usize, fresh: usize) -> String {
    match (open, all, fresh) {
        (true, ..) => "Hide Notifications".into(),
        (_, 0, _) => "No Notifications".into(),
        (_, _, 0) => "No New Notifications".into(),
        (_, _, 1) => "1 New Notification".into(),
        (_, _, n) => format!("{n} New Notifications"),
    }
}

/// The toasts drawn: the newest first, three at most, none while the list is open.
fn in_corner(all: &[Rc<Note>], toasts: &[u64], open: bool) -> Vec<Rc<Note>> {
    if open {
        return Vec::new();
    }
    all.iter().rev().filter(|n| toasts.contains(&n.id)).take(MOST).cloned().collect()
}

/// A notification drawn, in the corner or in the list: its icon, what it says, a cross; then who says it and its
/// buttons.
fn note_view(ui: Ui, send: Send, note: Rc<Note>, listed: bool) -> impl IntoView {
    let t = ui.colors;
    let s = ui.state::<Notes>();
    let id = note.id;
    let Toast { severity, text, source, actions, .. } = note.toast.clone();
    let over = create_rw_signal(false);
    let head = h_stack((
        icon(severity.icon(), 16.0).style(move |st| st.margin_top(3.0).color(severity.colour(&t.get()))),
        label(move || text.clone()).style(|st| {
            st.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
                .margin_left(8.0)
                .line_height(1.7)
                .text_overflow(TextOverflow::Wrap)
        }),
        // The cross, drawn while the pointer is on the notification.
        container(icon("close", 16.0).style(move |st| st.apply_if(!over.get(), |st| st.hide())))
            .on_click_stop(move |_| dismiss(ui, id))
            .style(move |st| {
                let c = t.get();
                st.width(22.0)
                    .height(22.0)
                    .margin_left(4.0)
                    .flex_shrink(0.0_f32)
                    .items_center()
                    .justify_center()
                    .border_radius(5.0)
                    .cursor(CursorStyle::Pointer)
                    .color(c.icon)
                    .hover(|st| st.background(c.toolbar_hover))
            }),
    ))
    .style(|st| st.width_full().items_start());
    let has_foot = !source.is_empty() || !actions.is_empty();
    let buttons = h_stack_from_iter(actions.into_iter().enumerate().map(move |(i, (name, run))| {
        let send = send.clone();
        label(move || name.clone())
            .on_click_stop(move |_| {
                // Cleared first: what it runs may say something of its own.
                dismiss(ui, id);
                run(ui, &send);
            })
            .style(move |st| {
                let c = t.get();
                let (bg, fg, hover) = if i == 0 {
                    (c.button_bg, c.button_fg, c.button_hover)
                } else {
                    (c.button2_bg, c.button2_fg, c.button2_hover)
                };
                st.padding_horiz(10.0)
                    .padding_vert(3.0)
                    .border_radius(2.0)
                    .font_size(12.0)
                    .background(bg)
                    .color(fg)
                    .cursor(CursorStyle::Pointer)
                    .hover(|st| st.background(hover))
            })
    }))
    .style(|st| st.gap(8.0).justify_end().flex_wrap(FlexWrap::Wrap));
    let foot = h_stack((
        label(move || source.clone()).style(move |st| st.font_size(12.0).color(t.get().desc)),
        empty().style(|st| st.flex_grow(1.0_f32)),
        buttons,
    ))
    .style(move |st| st.width_full().items_center().margin_top(6.0).apply_if(!has_foot, |st| st.hide()));
    v_stack((head, foot))
        .on_event_cont(EventListener::PointerEnter, move |_| {
            over.set(true);
            if !listed {
                put(s.pointed, Some(id));
            }
        })
        .on_event_cont(EventListener::PointerLeave, move |_| {
            over.set(false);
            if !listed && s.pointed.get_untracked() == Some(id) {
                s.pointed.set(None);
            }
        })
        .style(move |st| {
            let c = t.get();
            let st = st.width_full().padding_horiz(10.0).padding_vert(8.0).color(c.notify_fg).selectable(false);
            if listed {
                st.border_top(1.0).border_color(c.notify_border)
            } else {
                st.margin_top(8.0)
                    .border(1.0)
                    .border_color(c.notify_outline)
                    .border_radius(4.0)
                    .background(c.notify_bg)
                    .box_shadow_blur(8.0)
                    .box_shadow_spread(2.0)
                    .box_shadow_color(c.shadow)
            }
        })
}

/// The corner: the list when it is open, else the toasts. For the window's layers; it takes no room while there
/// is nothing in it.
fn corner(ui: Ui, send: Send) -> AnyView {
    let t = ui.colors;
    let s = ui.state::<Notes>();
    let toasts = {
        let send = send.clone();
        dyn_stack(
            move || s.all.with(|all| s.toasts.with(|ids| in_corner(all, ids, s.open.get()))),
            |n: &Rc<Note>| n.id,
            move |n| note_view(ui, send.clone(), n, false),
        )
        .style(|st| st.flex_col().width_full())
    };
    let header = h_stack((
        label(move || if s.all.with(Vec::is_empty) { "NO NEW NOTIFICATIONS" } else { "NOTIFICATIONS" })
            .style(|st| caps(st).flex_grow(1.0_f32)),
        action(t, "clear-all", move || clear_all(ui)),
        action(t, "chevron-down", move || show_list(ui, false)),
    ))
    .style(move |st| {
        let c = t.get();
        st.height(35.0)
            .width_full()
            .items_center()
            .gap(2.0)
            .padding_left(10.0)
            .padding_right(6.0)
            .background(c.notify_header_bg)
            .color(c.notify_header_fg)
    });
    let listed = dyn_stack(
        // The newest first.
        move || s.all.with(|all| all.iter().rev().cloned().collect::<Vec<_>>()),
        |n: &Rc<Note>| n.id,
        move |n| note_view(ui, send.clone(), n, true),
    )
    .style(|st| st.flex_col().width_full());
    let list = v_stack((header, scroll(listed).style(|st| st.width_full().max_height(400.0))))
        // A press in the list is the list's: nothing under it has it.
        .on_event_stop(EventListener::PointerDown, |_| {})
        .style(move |st| {
            let c = t.get();
            st.width_full()
                .border(1.0)
                .border_color(c.notify_outline)
                .border_radius(4.0)
                .background(c.notify_bg)
                .color(c.notify_fg)
                .box_shadow_blur(8.0)
                .box_shadow_spread(2.0)
                .box_shadow_color(c.shadow)
                .apply_if(!s.open.get(), |st| st.hide())
        });
    v_stack((list, toasts))
        .style(move |st| {
            let nothing = !s.open.get() && s.toasts.with(Vec::is_empty);
            st.absolute()
                .inset_right(8.0)
                .inset_bottom(30.0)
                .width(WIDTH)
                .max_width((ui.window.get().0 - 16.0).max(100.0))
                .font_size(13.0)
                .apply_if(nothing, |st| st.hide())
        })
        .into_any()
}

/// The status bar's bell: a dot on it while something new was said; a click opens the list, or closes it.
fn bell(ui: Ui) -> AnyView {
    let t = ui.colors;
    let s = ui.state::<Notes>();
    let dotted = ui.scope.create_memo(move |_| s.fresh.get() > 0);
    let drawn =
        floem::views::dyn_container(move || dotted.get(), |dot| icon(if dot { "bell-dot" } else { "bell" }, 16.0));
    let item = container(drawn)
        .on_click_stop(move |_| show_list(ui, !s.open.get_untracked()))
        .style(move |st| crate::statusbar::item(t, st).cursor(CursorStyle::Pointer));
    floem::views::tooltip(item, move || label(move || bell_tip(s.open.get(), s.all.with(Vec::len), s.fresh.get())))
        .into_any()
}

/// What it adds to the window's registries, once at start: the corner over the window, the bell that opens the
/// list, and VS Code's commands for the list.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    layers::add(Place::Over, "toasts", 50, corner);
    crate::statusbar::add("status.bell", crate::statusbar::Side::Right, 299, |ui, _| bell(ui));
    add(command(SHOW_LIST, "Notifications", "Show Notifications", |ui, _| show_list(ui, true)));
    // Escape hides the list only while no menu of the window's is open: that one closes first.
    add(command(HIDE_LIST, "Notifications", "Hide Notifications", |ui, _| show_list(ui, false))
        .keys("Escape")
        .when(|ui| list_open(ui) && ui.menu.get_untracked().is_none() && ui.context.get_untracked().is_none()));
    add(command(CLEAR_ALL, "Notifications", "Clear All Notifications", |ui, _| clear_all(ui)));
}

/// The session's messages that are this part's, taken in: none are (each part says its own).
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(notes: &[Rc<Note>]) -> Vec<u64> {
        notes.iter().map(|n| n.id).collect()
    }

    fn corner_ids(ui: Ui) -> Vec<u64> {
        let s = ui.state::<Notes>();
        ids(&s.all.with_untracked(|all| s.toasts.with_untracked(|t| in_corner(all, t, s.open.get_untracked()))))
    }

    #[test]
    fn what_is_said_is_a_toast_and_stays_in_the_list_once_its_toast_has_gone() {
        let ui = Ui::new();
        let s = ui.state::<Notes>();
        let (a, stays) = add(ui, info_of("Copied."));
        assert_eq!(stays, Some(Duration::from_secs(15)));
        let (b, _) = add(ui, warning_of("Careful."));
        assert_eq!((count(ui), corner_ids(ui), s.fresh.get_untracked()), (2, vec![b, a], 2), "the newest first");
        hide(ui, a);
        assert_eq!((count(ui), corner_ids(ui)), (2, vec![b]), "gone from the corner, kept in the list");
        hide(ui, a);
        dismiss(ui, b);
        assert_eq!((count(ui), corner_ids(ui), s.fresh.get_untracked()), (1, vec![], 1));
        dismiss(ui, 99);
        assert_eq!(count(ui), 1, "an id that is not there");
    }

    #[test]
    fn three_toasts_at_most_are_in_the_corner_and_the_older_ones_wait_in_the_list() {
        let ui = Ui::new();
        let all: Vec<u64> = (0..5).map(|i| add(ui, info_of(&format!("Note {i}"))).0).collect();
        assert_eq!(corner_ids(ui), [all[4], all[3], all[2]]);
        dismiss(ui, all[4]);
        assert_eq!(corner_ids(ui), [all[3], all[2], all[1]], "an older one takes its place");
        assert_eq!(count(ui), 4);
    }

    #[test]
    fn how_long_a_toast_stays_is_vs_codes() {
        assert_eq!(info_of("a").stays(), Some(Duration::from_secs(15)));
        assert_eq!(warning_of("a").stays(), Some(Duration::from_secs(18)));
        assert_eq!(error_of("a").stays(), Some(Duration::from_secs(20)));
        assert_eq!(info_of("a").sticky().stays(), None);
        assert_eq!(error_of("a").action("Retry", |_, _| {}).stays(), None, "an error with a button waits");
        assert_eq!(warning_of("a").action("Undo", |_, _| {}).stays(), Some(Duration::from_secs(18)));
    }

    #[test]
    fn the_list_takes_the_toasts_out_of_the_corner_and_clear_all_empties_it() {
        let ui = Ui::new();
        let s = ui.state::<Notes>();
        add(ui, info_of("One"));
        add(ui, error_of("Two"));
        show_list(ui, true);
        assert!(list_open(ui));
        assert_eq!((count(ui), corner_ids(ui), s.fresh.get_untracked()), (2, vec![], 0));
        // Said while the list is open: in the list, no toast, not new.
        let (c, stays) = add(ui, info_of("Three"));
        assert_eq!((stays, count(ui), corner_ids(ui), s.fresh.get_untracked()), (None, 3, vec![], 0));
        show_list(ui, false);
        assert_eq!(corner_ids(ui), Vec::<u64>::new(), "closing the list brings no toast back");
        let (d, _) = add(ui, info_of("Four"));
        assert_eq!((corner_ids(ui), s.fresh.get_untracked()), (vec![d], 1));
        assert!(c < d);
        show_list(ui, true);
        clear_all(ui);
        assert_eq!((count(ui), corner_ids(ui), list_open(ui), s.fresh.get_untracked()), (0, vec![], false, 0));
    }

    #[test]
    fn the_bell_says_how_many_are_new() {
        assert_eq!(bell_tip(false, 0, 0), "No Notifications");
        assert_eq!(bell_tip(false, 3, 0), "No New Notifications");
        assert_eq!(bell_tip(false, 3, 1), "1 New Notification");
        assert_eq!(bell_tip(false, 3, 2), "2 New Notifications");
        assert_eq!(bell_tip(true, 3, 2), "Hide Notifications");
    }

    #[test]
    fn the_lists_commands_are_vs_codes_and_escape_hides_it_only_while_it_is_open() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        register(ui, &send);
        assert!(layers::ids(Place::Over).contains(&"toasts"));
        assert!(crate::statusbar::items(crate::statusbar::Side::Right).iter().any(|i| i.id == "status.bell"));
        let hide = crate::commands::find(HIDE_LIST).expect("the command");
        assert_eq!((hide.label().as_str(), hide.shown_keys()), ("Notifications: Hide Notifications", "Escape"));
        assert!(!hide.offered(ui), "not while the list is closed: Escape is the window's");
        assert!(crate::commands::run(ui, &send, SHOW_LIST));
        assert!(list_open(ui) && hide.offered(ui));
        ui.menu.set(Some(0));
        assert!(!hide.offered(ui), "a menu open over the window: Escape closes it first");
        ui.menu.set(None);
        add(ui, info_of("One"));
        assert!(crate::commands::run(ui, &send, CLEAR_ALL));
        assert_eq!((count(ui), list_open(ui)), (0, false));
        assert!(!crate::commands::run(ui, &send, HIDE_LIST));
    }

    #[test]
    fn a_button_runs_with_the_window_and_the_session() {
        let ui = Ui::new();
        let toast = info_of("Deleted.").source("Explorer").action("Restore", |ui, send| {
            ui.message.set("restored".into());
            send(serde_json::json!({"type": "restore"}));
        });
        let said = Rc::new(std::cell::RefCell::new(Vec::new()));
        let to = said.clone();
        let send: Send = Rc::new(move |v| to.borrow_mut().push(v));
        assert_eq!((toast.source.as_str(), toast.actions[0].0.as_str()), ("Explorer", "Restore"));
        (toast.actions[0].1)(ui, &send);
        assert_eq!(ui.message.get_untracked(), "restored");
        assert_eq!(said.borrow()[0]["type"], "restore");
    }
}
