//! The compare editor's toolbar and its commands, as VS Code's: above the two sides, their names ("a.rs (Saved) ↔
//! a.rs") and the lines added and taken away, then Previous and Next Change (Shift+Alt+F5, Alt+F5), the inline
//! view or side by side, Show Whitespace Differences (`diffEditor.ignoreTrimWhitespace`), Swap Left and Right
//! Side, Open File; for the agent's change, Revert and Accept.
//!
//! The commands: Compare Active File with Saved (Ctrl+K D), with Clipboard (Ctrl+K C), Select for Compare and
//! Compare with Selected (the active file or the Explorer's), and the toolbar's own; in the tab's and the
//! Explorer's menus too. What every part has (`register`, `take`) is said in `ext.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use floem::reactive::{Memo, SignalGet, SignalUpdate};
use floem::views::{Decorators, empty, h_stack, h_stack_from_iter, label};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use super::{Entry, Source};
use crate::commands::{self, command};
use crate::menus::{self, Menu, item};
use crate::state::{Send, Ui};
use crate::widgets::action;

pub const WITH_SAVED: &str = "workbench.files.action.compareWithSaved";
pub const WITH_CLIPBOARD: &str = "workbench.files.action.compareWithClipboard";
pub const SELECT: &str = "selectForCompare";
pub const WITH_SELECTED: &str = "compareFiles";
pub const NEXT: &str = "workbench.action.compareEditor.nextChange";
pub const PREVIOUS: &str = "workbench.action.compareEditor.previousChange";
pub const INLINE: &str = "toggle.diff.renderSideBySide";
pub const WHITESPACE: &str = "toggle.diff.ignoreTrimWhitespace";
pub const SWAP: &str = "workbench.action.compareEditor.swapSides";

thread_local! {
    /// The file chosen with Select for Compare.
    static SELECTED: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The file the commands act on: the active tab's.
fn active_file(ui: Ui) -> Option<String> {
    ui.active.get_untracked().filter(|p| ui.tab(p).is_some_and(|t| t.is_file()))
}

/// The file Select for Compare and Compare with Selected act on: the Explorer's row when it has the keys or was
/// right-clicked last (what it has selected), else the active tab's.
fn chosen(ui: Ui) -> Option<String> {
    let selected = ui.selected.get_untracked();
    if !selected.is_empty() && ui.active.get_untracked().as_deref() != Some(&selected) {
        return Some(selected);
    }
    active_file(ui)
}

fn tip(v: impl IntoView + 'static, text: &'static str) -> AnyView {
    floem::views::tooltip(v, move || label(move || text)).into_any()
}

/// The toolbar of a compare.
pub fn bar(ui: Ui, send: Send, e: Rc<Entry>, inline: Memo<bool>) -> impl IntoView {
    let t = ui.colors;
    let title = format!("{} ↔ {}", e.left.source.label(), e.right.source.label());
    let compared = e.compared;
    let counts = move || {
        let (added, removed) = super::align::counts(&compared.get().hunks);
        (added, removed)
    };
    let path = e.path.clone();
    let run = move |id: &'static str| {
        let (send, path) = (send.clone(), path.clone());
        move || {
            if ui.active.get_untracked().as_deref() == Some(&path) {
                commands::run(ui, &send, id);
            }
        }
    };
    let before = match &e.left.source {
        Source::Before(p) => Some(p.clone()),
        _ => None,
    };
    let agent: AnyView = match before {
        Some(p) => {
            let (send, again) = (ui.out.get_untracked(), ui.out.get_untracked());
            let (p, q) = (p.clone(), p);
            h_stack((
                tip(
                    action(t, "discard", move || {
                        if let Some(send) = &send {
                            send(json!({"type": "changes_revert", "path": p}));
                        }
                    }),
                    "Revert",
                ),
                tip(
                    action(t, "check", move || {
                        if let Some(send) = &again {
                            send(json!({"type": "changes_accept", "path": q}));
                        }
                    }),
                    "Accept",
                ),
            ))
            .style(|s| s.gap(2.0))
            .into_any()
        }
        None => empty().into_any(),
    };
    // Stage, Unstage, Revert or Accept the lines chosen, by what the two sides are.
    use super::ranges::{self, Act};
    let picks: Vec<AnyView> = [
        (Act::Stage, ranges::STAGE, "add", "Stage Selected Ranges (Ctrl+K Ctrl+Alt+S)"),
        (Act::Unstage, ranges::UNSTAGE, "remove", "Unstage Selected Ranges (Ctrl+K Ctrl+N)"),
        (Act::Revert, ranges::REVERT, "discard", "Revert Selected Ranges (Ctrl+K Ctrl+R)"),
        (Act::Accept, ranges::ACCEPT, "check-all", "Accept the Agent's Selected Changes (Ctrl+K Ctrl+Alt+A)"),
    ]
    .into_iter()
    .filter(|(act, ..)| ranges::file_for(*act, &e.left.source, &e.right.source, e.right.editable).is_some())
    .map(|(_, id, icon, text)| tip(action(t, icon, run(id)), text))
    .collect();
    let picks = h_stack_from_iter(picks).style(|s| s.gap(2.0));
    let file = e.live().or(Some(&e.right)).and_then(|h| h.source.file().map(str::to_string));
    let opener = match file {
        Some(f) => {
            let send = ui.out.get_untracked();
            tip(
                action(t, "go-to-file", move || {
                    if let Some(send) = &send {
                        ui.open(&f, send);
                    }
                }),
                "Open File",
            )
        }
        None => empty().into_any(),
    };
    h_stack((
        label(move || title.clone()).style(move |s| s.color(t.get().desc).text_ellipsis().min_width(0.0)),
        label(move || format!("+{}", counts().0)).style(move |s| s.margin_left(10.0).color(t.get().added_fg)),
        label(move || format!("−{}", counts().1)).style(move |s| s.margin_left(6.0).color(t.get().removed_fg)),
        empty().style(|s| s.flex_grow(1.0_f32)),
        tip(action(t, "arrow-up", run(PREVIOUS)), "Previous Change (Shift+Alt+F5)"),
        tip(action(t, "arrow-down", run(NEXT)), "Next Change (Alt+F5)"),
        tip(
            crate::widgets::action_lit(
                t,
                move || if inline.get() { "split-horizontal" } else { "compare" },
                run(INLINE),
            ),
            "Toggle Inline View",
        ),
        // Lit while the spaces at the lines' ends are compared too.
        tip(
            action(t, "whitespace", run(WHITESPACE)).style(move |s| {
                let shown = !crate::settings::flag(ui, "diffEditor.ignoreTrimWhitespace", None);
                s.apply_if(shown, |s| s.background(t.get().toolbar_hover))
            }),
            "Show Whitespace Differences",
        ),
        tip(action(t, "arrow-swap", run(SWAP)), "Swap Left and Right Side"),
        picks,
        opener,
        agent,
    ))
    .style(move |s| {
        let c = t.get();
        s.width_full()
            .height(28.0)
            .items_center()
            .gap(2.0)
            .padding_horiz(10.0)
            .font_size(12.0)
            .background(c.editor_bg)
            .border_bottom(1.0)
            .border_color(c.group_border)
    })
}

/// What it adds to the window's registries, once at start: the compare commands, with their keys, and their rows
/// in the tab's and the Explorer's menus.
pub fn register(_ui: Ui, _send: &Send) {
    let is_file = |ui: Ui| active_file(ui).is_some();
    let is_compare = |ui: Ui| super::shown(ui).is_some();
    commands::add(
        command(WITH_SAVED, "File", "Compare Active File with Saved", |ui, send| {
            if let Some(p) = active_file(ui) {
                super::open(ui, send, Source::Saved(p.clone()), Source::Live(p));
            }
        })
        .keys("Ctrl+K D")
        .when(is_file),
    );
    commands::add(
        command(WITH_CLIPBOARD, "File", "Compare Active File with Clipboard", |ui, send| {
            if let Some(p) = active_file(ui) {
                super::open(ui, send, Source::Clipboard(super::clip()), Source::Live(p));
            }
        })
        .keys("Ctrl+K C")
        .when(is_file),
    );
    commands::add(
        command(SELECT, "File", "Select for Compare", |ui, _| {
            if let Some(p) = chosen(ui) {
                SELECTED.with(|s| *s.borrow_mut() = Some(p));
            }
        })
        .when(move |ui| chosen(ui).is_some()),
    );
    commands::add(
        command(WITH_SELECTED, "File", "Compare with Selected", |ui, send| {
            let Some(first) = SELECTED.with(|s| s.borrow().clone()) else { return };
            if let Some(p) = chosen(ui).filter(|p| *p != first) {
                let live =
                    |p: String| if ui.tab(&p).is_some_and(|t| t.is_file()) { Source::Live(p) } else { Source::File(p) };
                super::open(ui, send, live(first), live(p));
            }
        })
        .when(move |ui| SELECTED.with(|s| s.borrow().is_some()) && chosen(ui).is_some()),
    );
    commands::add(
        command(NEXT, "Compare", "Go to Next Change", |ui, _| {
            if let Some(e) = super::shown(ui) {
                super::go(ui, &e.path, true);
            }
        })
        .keys("Alt+F5")
        .when(is_compare),
    );
    commands::add(
        command(PREVIOUS, "Compare", "Go to Previous Change", |ui, _| {
            if let Some(e) = super::shown(ui) {
                super::go(ui, &e.path, false);
            }
        })
        .keys("Shift+Alt+F5")
        .when(is_compare),
    );
    commands::add(
        command(INLINE, "Compare", "Toggle Inline View", |ui, _| {
            if let Some(e) = super::shown(ui) {
                let now = super::is_inline(ui, &e);
                e.inline.set(Some(!now));
            }
        })
        .when(is_compare),
    );
    commands::add(command(WHITESPACE, "Compare", "Toggle Ignore Trim Whitespace", |ui, send| {
        let now = crate::settings::now(ui, "diffEditor.ignoreTrimWhitespace", None).as_bool().unwrap_or(true);
        send(json!({"type": "set_setting", "key": "diffEditor.ignoreTrimWhitespace", "value": !now}));
    }));
    commands::add(
        command(SWAP, "Compare", "Swap Left and Right Side", |ui, send| {
            let Some(e) = super::shown(ui) else { return };
            ui.close_now(&e.path);
            super::open(ui, send, e.right.source.clone(), e.left.source.clone());
        })
        .when(is_compare),
    );
    menus::add(Menu::EditorTab, item(WITH_SAVED).group("3_compare").order(10));
    menus::add(Menu::EditorTab, item(SELECT).group("3_compare").order(20));
    menus::add(Menu::EditorTab, item(WITH_SELECTED).group("3_compare").order(30));
    menus::add(Menu::ExplorerRow, item(SELECT).group("3_compare").order(20));
    menus::add(Menu::ExplorerRow, item(WITH_SELECTED).group("3_compare").order(30));
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}
