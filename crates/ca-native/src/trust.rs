//! Workspace trust, as VS Code's: the first time a folder is opened the window asks whether its authors are
//! trusted, since Code Arena runs some of the folder's own programs (its formatters, language servers, tasks,
//! debuggers, notebook kernels and project tools). Until they are, the folder is in Restricted Mode: an item at the
//! status bar's left and a banner under the title bar say so, and "Workspaces: Manage Workspace Trust" asks again.
//! The answer is the driver's, kept on this computer (`ide_trust.rs`).
//!
//! - `state` with another `folder`: the window asks `trust`.
//! - `trust{folder, trusted, asked}`: the folder's trust; one never answered for is asked about, once a window.
//! - Any reply with `kind: "not_trusted"` (a formatter, a server refused): a toast with "Trust" and "Manage".
//! - It sends `trust_set{trusted, parent}`.

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, h_stack, label};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::widgets::{dialog, toast};

/// The command that asks again.
pub const MANAGE: &str = "workbench.trust.manage";

const QUESTION: &str = "Do you trust the authors of the files in this folder?";
const DETAIL: &str = "Code Arena provides features that may automatically run files in this folder: its formatters, \
language servers, tasks, debuggers, notebook kernels and project tools.\n\nIf you don't trust the authors of these \
files, we recommend to continue in Restricted Mode as the files may be malicious: you can still browse and edit them, \
and nothing of theirs runs.";
const YES: &str = "Yes, I trust the authors";
const NO: &str = "No, I don't trust the authors";
const RESTRICTED: &str = "Restricted Mode";
const BANNER: &str =
    "Restricted Mode is intended for safe code browsing. Trust this folder to enable all features.";
const REFUSED: &str = "This feature is disabled in Restricted Mode.";

/// This part's state in a window.
#[derive(Clone, Copy)]
pub struct Trust {
    /// The folder of the session's last `state`.
    folder: RwSignal<String>,
    /// The folder as the driver names it in `trust`.
    root: RwSignal<String>,
    /// Whether the folder's programs may run; none until the driver has said.
    trusted: RwSignal<Option<bool>>,
    /// The folder this window has asked about at its opening (it is asked once).
    asked_here: RwSignal<String>,
    /// The startup question is open.
    asking: RwSignal<bool>,
    banner_closed: RwSignal<bool>,
    /// The toast of the last refusal, while it may be shown.
    refusal: RwSignal<Option<u64>>,
}

impl crate::ext::State for Trust {
    fn new(ui: Ui) -> Self {
        Trust {
            folder: ui.scope.create_rw_signal(String::new()),
            root: ui.scope.create_rw_signal(String::new()),
            trusted: ui.scope.create_rw_signal(None),
            asked_here: ui.scope.create_rw_signal(String::new()),
            asking: ui.scope.create_rw_signal(false),
            banner_closed: ui.scope.create_rw_signal(false),
            refusal: ui.scope.create_rw_signal(None),
        }
    }
}

fn set_if_new<T: PartialEq + 'static>(s: RwSignal<T>, v: T) {
    if s.with_untracked(|was| *was != v) {
        s.set(v);
    }
}

/// Whether the window is in Restricted Mode: the driver has said the folder is not trusted.
pub fn restricted(ui: Ui) -> bool {
    ui.state::<Trust>().trusted.get() == Some(false)
}

/// Whether a `trust` message wants the question asked in this window: never answered, not trusted, and not asked
/// here already.
fn wants_asking(m: &Value, asked_here: &str) -> bool {
    let folder = m["folder"].as_str().unwrap_or("");
    !folder.is_empty()
        && !m["trusted"].as_bool().unwrap_or(false)
        && !m["asked"].as_bool().unwrap_or(false)
        && folder != asked_here
}

/// The words of the box that trusts the folder a folder is in, when it is in one: VS Code's.
fn parent_box(folder: &str) -> Option<String> {
    let parent = std::path::Path::new(folder).parent()?;
    let name = parent.file_name()?.to_string_lossy().to_string();
    Some(format!("Trust the authors of all files in the parent folder '{name}'"))
}

/// Asks whether the folder's authors are trusted, and sends the answer. Escape answers nothing.
fn ask(ui: Ui, send: Send, startup: bool) {
    let me = ui.state::<Trust>();
    let mut d = dialog::ask(QUESTION).kind(dialog::Kind::Info).detail(DETAIL).button(YES).button(NO).no_cancel();
    if let Some(words) = parent_box(&me.root.get_untracked()) {
        d = d.checkbox(&words, false);
    }
    set_if_new(me.asking, startup);
    d.open(ui, move |a| {
        set_if_new(me.asking, false);
        match a.button {
            Some(0) => send(json!({"type": "trust_set", "trusted": true, "parent": a.checked})),
            Some(1) => send(json!({"type": "trust_set", "trusted": false})),
            _ => {}
        }
    });
}

/// "Manage Workspace Trust": the question again, or, for a trusted folder, whether to take its trust back.
pub fn manage(ui: Ui, send: &Send) {
    let send = send.clone();
    if ui.state::<Trust>().trusted.get_untracked() != Some(true) {
        return ask(ui, send, false);
    }
    dialog::ask("You trust the authors of the files in this folder.")
        .kind(dialog::Kind::Info)
        .detail(
            "All features are enabled. Don't trust it to browse it in Restricted Mode: its formatters, language \
             servers, tasks, debuggers, notebook kernels and project tools stop, and nothing of the folder's runs.",
        )
        .button("Don't Trust")
        .cancel("Close")
        .open(ui, move |a| {
            if a.yes() {
                send(json!({"type": "trust_set", "trusted": false}));
            }
        });
}

/// A program of the folder's was refused: a toast that offers to trust it (one at a time).
fn refused(ui: Ui, m: &Value) {
    let me = ui.state::<Trust>();
    if let Some(id) = me.refusal.get_untracked() {
        toast::dismiss(ui, id);
    }
    let text = m["error"].as_str().filter(|e| !e.is_empty()).unwrap_or(REFUSED);
    let id = toast::warning_of(text)
        .action("Trust", |_, send| send(json!({"type": "trust_set", "trusted": true})))
        .action("Manage", |ui, send| manage(ui, send))
        .source("Workspace Trust")
        .show_in(ui);
    me.refusal.set(Some(id));
}

/// The status bar's "Restricted Mode", in VS Code's prominent colours; a click manages the trust.
fn status_item(ui: Ui, send: Send) -> floem::AnyView {
    let t = ui.colors;
    let item = h_stack((icon("shield", 16.0), label(|| RESTRICTED.to_string())))
        .on_click_stop(move |_| manage(ui, &send))
        .style(move |s| {
            let c = t.get();
            crate::statusbar::item(t, s)
                .cursor(CursorStyle::Pointer)
                .background(c.status_prominent_bg)
                .hover(|s| s.background(c.status_prominent_hover))
                .apply_if(!restricted(ui), |s| s.hide())
        });
    floem::views::tooltip(item, || label(|| BANNER.to_string())).into_any()
}

/// The banner under the title bar while in Restricted Mode, with "Manage" and an X.
fn banner(ui: Ui, send: Send) -> floem::AnyView {
    let t = ui.colors;
    let me = ui.state::<Trust>();
    let link = |words: &'static str| {
        label(move || words.to_string()).style(move |s| {
            let c = t.get();
            s.font_size(12.0).margin_left(10.0).cursor(CursorStyle::Pointer).color(c.banner_fg).hover(|s| s.color(c.banner_icon))
        })
    };
    let x = container(icon("close", 16.0)).on_click_stop(move |_| me.banner_closed.set(true)).style(move |s| {
        let c = t.get();
        s.width(22.0)
            .height(22.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .justify_center()
            .border_radius(5.0)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.background(c.toolbar_hover))
    });
    h_stack((
        icon("shield", 16.0).style(move |s| s.flex_shrink(0.0_f32).margin_right(6.0).color(t.get().banner_icon)),
        label(|| BANNER.to_string()).style(|s| {
            s.flex_shrink(1.0_f32).min_width(0.0).font_size(12.0).text_overflow(TextOverflow::Ellipsis)
        }),
        link("Manage").on_click_stop(move |_| manage(ui, &send)),
        container(floem::views::empty()).style(|s| s.flex_grow(1.0_f32)),
        x,
    ))
    .style(move |s| {
        let c = t.get();
        let shown = restricted(ui) && !me.banner_closed.get();
        s.width_full()
            .height(26.0)
            .padding_horiz(8.0)
            .items_center()
            .background(c.banner_bg)
            .color(c.banner_fg)
            .apply_if(!shown, |s| s.hide())
    })
    .into_any()
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    crate::commands::add(crate::commands::command(MANAGE, "Workspaces", "Manage Workspace Trust", manage));
    crate::statusbar::add("status.workspaceTrust", crate::statusbar::Side::Left, 95, status_item);
    layers::add(Place::Banner, "trust.restricted", 15, banner);
}

/// The session's messages that are this part's, taken in; none is taken away from the rest.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let me = ui.state::<Trust>();
    match m["type"].as_str().unwrap_or("") {
        "state" => {
            if let Some(folder) = m["folder"].as_str().filter(|f| me.folder.with_untracked(|was| was != f)) {
                me.folder.set(folder.to_string());
                set_if_new(me.trusted, None);
                set_if_new(me.banner_closed, false);
                send(json!({"type": "trust"}));
            }
        }
        "trust" => {
            let trusted = m["trusted"].as_bool().unwrap_or(false);
            set_if_new(me.root, m["folder"].as_str().unwrap_or("").to_string());
            set_if_new(me.trusted, Some(trusted));
            if trusted && let Some(id) = me.refusal.get_untracked() {
                toast::dismiss(ui, id);
                me.refusal.set(None);
            }
            if m["asked"].as_bool().unwrap_or(false) && me.asking.get_untracked() {
                // Answered in another window: the question here goes.
                dialog::cancel(ui);
            } else if wants_asking(m, &me.asked_here.get_untracked()) {
                me.asked_here.set(me.root.get_untracked());
                ask(ui, send.clone(), true);
            }
        }
        _ if m["kind"] == "not_trusted" => refused(ui, m),
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    #[test]
    fn a_folder_never_answered_for_is_asked_about_once() {
        let never = json!({"type": "trust", "folder": "/w/repo", "trusted": false, "asked": false});
        assert!(wants_asking(&never, ""));
        assert!(!wants_asking(&never, "/w/repo"));
        assert!(!wants_asking(&json!({"folder": "/w/repo", "trusted": false, "asked": true}), ""));
        assert!(!wants_asking(&json!({"folder": "/w/repo", "trusted": true, "asked": true}), ""));
        assert_eq!(parent_box("/w/repo").as_deref(), Some("Trust the authors of all files in the parent folder 'w'"));
        assert_eq!(parent_box("/repo"), None);
    }

    #[test]
    fn another_folder_asks_its_trust_and_the_answer_sets_restricted_mode() {
        let ui = Ui::new();
        let sent = Rc::new(RefCell::new(Vec::new()));
        let seen = sent.clone();
        let send: Send = Rc::new(move |v| seen.borrow_mut().push(v));
        take(ui, &json!({"type": "state", "folder": "/w/repo"}), &send);
        take(ui, &json!({"type": "state", "folder": "/w/repo"}), &send);
        assert_eq!(*sent.borrow(), vec![json!({"type": "trust"})], "asked once for the folder");
        take(ui, &json!({"type": "trust", "folder": "/w/repo", "trusted": false, "asked": true}), &send);
        assert!(restricted(ui));
        assert!(!dialog::is_open(ui), "a folder answered for is not asked about");
        take(ui, &json!({"type": "trust", "folder": "/w/repo", "trusted": true, "asked": true}), &send);
        assert!(!restricted(ui));
    }

    #[test]
    fn a_folder_never_answered_for_opens_the_question() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        take(ui, &json!({"type": "trust", "folder": "/w/repo", "trusted": false, "asked": false}), &send);
        assert!(dialog::is_open(ui));
        dialog::cancel(ui);
        take(ui, &json!({"type": "trust", "folder": "/w/repo", "trusted": false, "asked": false}), &send);
        assert!(!dialog::is_open(ui), "asked once a window");
    }
}
