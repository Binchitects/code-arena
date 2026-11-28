//! The Help overlay and About, as the old Code Arena's: Help (**Help: Show Help**, the Help menu, the activity bar's
//! Help) opens a sheet at the window's right over the dimmed workbench, with what Code Arena is for, every part of
//! it, the common tasks step by step and the keys as the keymap has them now, and a link to the manual when the
//! session gives one on the web. The '?' in a view's title (and **Help: the Chat** in the chat's More menu) opens it
//! with that view's part first, as "This part: …". Escape, the cross or a click beside it closes it.
//!
//! About (**Help: About**, the activity bar's info button, the status bar's version) is VS Code's About dialog:
//! the name, what it is for, the version, the licence, the folder, where the log is kept and the source, with
//! **Copy**, **View Licence** and **Open Source**. What it says comes from the session's `state` (`version`,
//! `license`, `license_text`, `source`, `log`, `manual`).

use std::rc::Rc;

use floem::action::remove_overlay;
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::{Decorators, container, empty, h_stack, label, scroll, v_stack};
use floem::{IntoView, ViewId};
use serde_json::Value;

use crate::help_text::{self, Part};
use crate::icons::icon;
use crate::layout::{VIEWS, View};
use crate::state::{Send, Ui};
use crate::widgets::{action, caps, cover, dialog, solid, window_size};

/// Help: Show Help, the whole help.
pub const SHOW_HELP: &str = "codeArena.showHelp";
/// VS Code's Help: Documentation, the manual on the web.
pub const DOCUMENTATION: &str = "workbench.action.openDocumentationUrl";
/// VS Code's Help: View License.
pub const LICENSE: &str = "workbench.action.openLicenseUrl";

/// The command that opens the help at a view's part: one per view the help tells of, hidden from the palette.
pub fn show_help_of(v: View) -> String {
    format!("{SHOW_HELP}.{}", v.id())
}

/// What About says, from the session's `state`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub version: String,
    pub license: String,
    pub license_text: String,
    pub source: String,
    pub log: Option<String>,
    pub manual: Option<String>,
}

impl Facts {
    fn of(m: &Value) -> Facts {
        let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
        let some = |k: &str| m[k].as_str().filter(|v| !v.is_empty()).map(str::to_string);
        Facts {
            version: s("version"),
            license: s("license"),
            license_text: s("license_text"),
            source: s("source"),
            log: some("log"),
            manual: some("manual"),
        }
    }

    /// The manual's address when it is on the web (the session names it so only when it knows the Arena's).
    pub fn book(&self) -> Option<&str> {
        self.manual.as_deref().filter(|m| m.starts_with("http://") || m.starts_with("https://"))
    }

    /// About's words under the name, a line for each fact (none for the log while it is not kept).
    pub fn detail(&self, folder: &str) -> String {
        let version = if self.version.is_empty() { env!("CARGO_PKG_VERSION") } else { &self.version };
        let mut lines = vec![help_text::ABOUT.to_string(), String::new(), format!("Version: {version}")];
        if !self.license.is_empty() {
            lines.push(format!("Licence: {}", self.license));
        }
        if !folder.is_empty() {
            lines.push(format!("Folder: {folder}"));
        }
        if let Some(log) = &self.log {
            lines.push(format!("Log: {log}"));
        }
        if !self.source.is_empty() {
            lines.push(format!("Source: {}", self.source));
        }
        lines.join("\n")
    }
}

#[derive(Clone, Copy)]
struct Help {
    /// The overlay while it is open.
    cover: RwSignal<Option<ViewId>>,
    /// The help's Markdown, written when it opens.
    text: RwSignal<String>,
    facts: RwSignal<Rc<Facts>>,
}

impl crate::ext::State for Help {
    fn new(ui: Ui) -> Self {
        Help {
            cover: ui.scope.create_rw_signal(None),
            text: ui.scope.create_rw_signal(String::new()),
            facts: ui.scope.create_rw_signal(Rc::new(Facts::default())),
        }
    }
}

/// The help closed, when it is open.
pub fn close(ui: Ui) {
    let cover = ui.state::<Help>().cover;
    if let Some(id) = cover.get_untracked() {
        cover.set(None);
        remove_overlay(id);
    }
}

/// The help's Markdown as it is now: the part first, the keys read from the keymap.
fn written(ui: Ui, part: Option<&Part>) -> String {
    let facts = ui.state::<Help>().facts.get_untracked();
    help_text::markdown(part, facts.manual.as_deref(), |commands, said| {
        help_text::keys_shown(commands, said, crate::commands::keys_of)
    })
}

/// The help opened over the window, with `part` first when one is asked about.
pub fn open(ui: Ui, part: Option<&'static Part>) {
    close(ui);
    crate::widgets::popup::close(ui);
    crate::widgets::quick::cancel(ui);
    dialog::cancel(ui);
    let st = ui.state::<Help>();
    let md = written(ui, part);
    if st.text.with_untracked(|t| *t != md) {
        st.text.set(md);
    }
    let key = move |key: &Key, _: Modifiers| {
        if matches!(key, Key::Named(NamedKey::Escape)) {
            close(ui);
        }
    };
    let id = cover(
        ui,
        window_size(ui, None),
        true,
        key,
        move || close(ui),
        move |keep| {
            h_stack((empty().style(|s| s.flex_grow(1.0_f32)), solid(sheet(ui, st), keep)))
                .style(|s| s.width_full().height_full())
                .into_any()
        },
    );
    st.cover.set(Some(id));
}

/// The sheet: its title, the help, and the manual at its foot.
fn sheet(ui: Ui, st: Help) -> impl IntoView {
    let t = ui.colors;
    let facts = st.facts.get_untracked();
    let head = v_stack((
        h_stack((
            label(|| "HELP").style(move |s| caps(s).color(t.get().desc).flex_grow(1.0_f32)),
            action(t, "close", move || close(ui)),
        ))
        .style(|s| s.items_center()),
        label(|| "Code Arena").style(|s| s.font_size(18.0).font_weight(floem::text::Weight::SEMIBOLD)),
        label(|| help_text::ABOUT).style(move |s| s.color(t.get().desc).line_height(1.5)),
    ))
    .style(move |s| {
        s.gap(4.0).padding(20.0).padding_bottom(12.0).border_bottom(1.0).border_color(t.get().widget_border)
    });
    let body = scroll(
        container(crate::prose::answer(ui, st.text)).style(|s| s.width_full().min_width(0.0).padding_horiz(20.0)),
    )
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
    let foot = match facts.book().map(str::to_string) {
        Some(book) => h_stack((icon("book", 16.0), label(|| "Code Arena in the manual")))
            .on_click_stop(move |_| crate::citations::browse(&format!("{book}#the-ide")))
            .style(move |s| s.gap(8.0).items_center().color(t.get().link).cursor(CursorStyle::Pointer))
            .into_any(),
        None => label(help_text::no_manual).style(move |s| s.color(t.get().desc)).into_any(),
    };
    let foot = container(foot).style(move |s| s.padding(16.0).border_top(1.0).border_color(t.get().widget_border));
    v_stack((head, body, foot)).style(move |s| {
        let c = t.get();
        s.width(400.0)
            .height_full()
            .background(c.side_bg)
            .color(c.side_fg)
            .border_left(1.0)
            .border_color(c.widget_border)
            .box_shadow_blur(8.0)
            .box_shadow_color(c.shadow)
    })
}

/// About, as VS Code's dialog.
pub fn about(ui: Ui) {
    let facts = ui.state::<Help>().facts.get_untracked();
    let detail = facts.detail(&ui.folder.get_untracked());
    let copied = format!("Code Arena\n{detail}");
    dialog::ask("Code Arena")
        .kind(dialog::Kind::Info)
        .detail(&detail)
        .button("OK")
        .button("Copy")
        .button("View Licence")
        .button("Open Source")
        .no_cancel()
        .open(ui, move |a| match a.button {
            Some(1) => {
                let _ = floem::Clipboard::set_contents(copied);
            }
            Some(2) => licence(ui),
            Some(3) if !facts.source.is_empty() => crate::citations::browse(&facts.source),
            _ => {}
        });
}

/// The licence's whole text.
fn licence(ui: Ui) {
    let facts = ui.state::<Help>().facts.get_untracked();
    let title = if facts.license.is_empty() { "Licence".to_string() } else { format!("{} Licence", facts.license) };
    let text = if facts.license_text.is_empty() { "The session has not said it yet." } else { &facts.license_text };
    dialog::ask(&title).detail(text.trim_end()).button("OK").no_cancel().open(ui, |_| {});
}

/// The help's commands, the '?' in each view's title, and the Help menu's rows.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{HELP, Menu, add as row, item};
    add(command(SHOW_HELP, "Help", "Show Help", |ui, _| open(ui, None)));
    add(command(crate::workbench::ABOUT, "Help", "About", |ui, _| about(ui)));
    add(command(DOCUMENTATION, "Help", "Documentation", |ui, _| {
        if let Some(book) = ui.state::<Help>().facts.get_untracked().book() {
            crate::citations::browse(&format!("{book}#the-ide"));
        }
    })
    .enabled(|ui| ui.state::<Help>().facts.with_untracked(|f| f.book().is_some())));
    add(command(LICENSE, "Help", "View License", |ui, _| licence(ui)));
    for v in VIEWS {
        let Some(part) = help_text::part_of_view(v) else { continue };
        let id = show_help_of(v);
        add(command(&id, "Help", &format!("Help: {}", part.name), move |ui, _| open(ui, Some(part))).hidden());
        row(Menu::ViewTitle(v), item(&id).icon("question").group("z_help").order(100));
    }
    row(Menu::AgentMore, item(&show_help_of(View::Agent)).title("Help: the Chat").group("9_help"));
    let bar = Menu::TitleBar(HELP);
    row(bar, item(crate::workbench::SHOW_COMMANDS).title("Show All Commands").group("1_welcome").order(1));
    row(bar, item(SHOW_HELP).title("Code Arena Help").group("1_welcome").order(2));
    row(bar, item(DOCUMENTATION).group("2_docs"));
    row(bar, item(LICENSE).group("4_legal"));
    // About last, as VS Code's (the window gave it no group, which comes first).
    row(bar, item(crate::workbench::ABOUT).group("z_about"));
}

/// What About and the manual's link say, read from the session's `state`. Never taken away (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "state" {
        let facts = Facts::of(m);
        let kept = ui.state::<Help>().facts;
        if kept.with_untracked(|f| **f != facts) {
            kept.set(Rc::new(facts));
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menus::{Menu, Row, rows};
    use serde_json::json;

    fn titles(ui: Ui, menu: Menu) -> Vec<String> {
        rows(ui, menu)
            .into_iter()
            .map(|r| match r {
                Row::Line => "-".to_string(),
                Row::Entry { title, .. } => title,
            })
            .collect()
    }

    #[test]
    fn about_says_what_the_session_told_and_leaves_out_a_log_not_kept() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let state = json!({"type": "state", "version": "1.2.3", "license": "MIT", "license_text": "MIT License\n",
            "source": "https://example.com/src", "log": null, "manual": "ARENA/help/code-arena"});
        take(ui, &state, &send);
        let facts = ui.state::<Help>().facts.get_untracked();
        let detail = facts.detail("/tmp/work");
        assert!(detail.contains("Version: 1.2.3\nLicence: MIT\nFolder: /tmp/work\nSource: https://example.com/src"));
        assert!(!detail.contains("Log:"));
        assert_eq!(facts.book(), None);
        let mut with_log = state.clone();
        with_log["log"] = json!("/opt/code-arena.log");
        with_log["manual"] = json!("https://arena.example/help/code-arena");
        take(ui, &with_log, &send);
        let facts = ui.state::<Help>().facts.get_untracked();
        assert!(facts.detail("").contains("Log: /opt/code-arena.log"));
        assert_eq!(facts.book(), Some("https://arena.example/help/code-arena"));
    }

    #[test]
    fn the_same_state_again_sets_nothing() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let state = json!({"type": "state", "version": "1"});
        take(ui, &state, &send);
        let before = ui.state::<Help>().facts.get_untracked();
        take(ui, &state, &send);
        assert!(Rc::ptr_eq(&before, &ui.state::<Help>().facts.get_untracked()));
    }

    #[test]
    fn the_help_menu_has_the_help_first_and_about_last() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        crate::workbench::register(ui, &send);
        register(ui, &send);
        let help = titles(ui, Menu::TitleBar(crate::menus::HELP));
        assert_eq!(help.first().map(String::as_str), Some("Show All Commands"));
        assert_eq!(help.get(1).map(String::as_str), Some("Code Arena Help"));
        assert_eq!(help.last().map(String::as_str), Some("About"));
        assert_eq!(help.iter().filter(|t| *t == "About").count(), 1);
        // A view's title has its '?', a view the help does not tell of has none.
        let has_help = |v: View| {
            rows(ui, Menu::ViewTitle(v)).into_iter().any(|r| matches!(r, Row::Entry { icon: "question", .. }))
        };
        assert!(has_help(View::Search));
        assert!(!has_help(View::Plan));
        // The per-view commands are not in the palette.
        let shown = crate::commands::find(&show_help_of(View::Search)).expect("command");
        assert!(shown.hidden);
        assert!(titles(ui, Menu::AgentMore).contains(&"Help: the Chat".to_string()));
    }

    #[test]
    fn the_documentation_waits_for_a_manual_on_the_web() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        register(ui, &send);
        let docs = crate::commands::find(DOCUMENTATION).expect("command");
        assert!(!docs.may_run(ui));
        take(ui, &json!({"type": "state", "manual": "https://arena.example/help/code-arena"}), &send);
        assert!(docs.may_run(ui));
    }
}
