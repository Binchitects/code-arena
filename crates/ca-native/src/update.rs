//! A newer Code Arena from Arena, as the old Code Arena's status bar offers it and in VS Code's words: when the
//! session says one is there (`state.update`, `update_checked`), a toast says "Code Arena X is available from
//! Arena (this is Y)." with Update Now, and the status bar's Update item stays on the right. A click downloads
//! and puts it in place (`update_install`): the item counts the download (`update_progress`), then says
//! "Updated: restart" (`update_done`), and a click on it starts the new program and closes this one.
//!
//! Its commands are VS Code's: Check for Updates... (also in the Manage menu), Install Update and Restart to
//! Update. Nothing is taken away from the parts after this one.

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::CursorStyle;
use floem::views::{Decorators, dyn_container, h_stack, label, tooltip};
use serde_json::{Value, json};

use crate::commands::{self, command};
use crate::ext::State;
use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::statusbar::{self, Side};
use crate::widgets::toast;

/// VS Code's update commands.
pub const CHECK: &str = "update.checkForUpdate";
pub const INSTALL: &str = "update.installUpdate";
pub const RESTART: &str = "update.restart";

/// Where an update stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    /// None is offered.
    #[default]
    None,
    /// A newer version is there to install.
    Available { version: String, said: String },
    /// It is coming down: how far, in percent (and what was said of it, for a download that fails).
    Downloading { version: String, pct: u8, said: String },
    /// It is in place: the program to start again.
    Installed { version: String, program: String, said: String },
}

impl Phase {
    /// What the item shows (none for `None`).
    pub fn text(&self) -> String {
        match self {
            Phase::None => String::new(),
            Phase::Available { .. } => "Update".into(),
            Phase::Downloading { pct, .. } => format!("Updating {pct}%"),
            Phase::Installed { .. } => "Updated: restart".into(),
        }
    }

    /// What the item says in its tooltip.
    pub fn tip(&self) -> String {
        match self {
            Phase::None => String::new(),
            Phase::Available { said, .. } => format!("{said} Click to download and install it"),
            Phase::Downloading { version, pct, .. } => format!("Downloading Code Arena {version} from Arena: {pct}%"),
            Phase::Installed { said, .. } => format!("{said} Click to restart"),
        }
    }

    fn kind(&self) -> u8 {
        match self {
            Phase::None => 0,
            Phase::Available { .. } => 1,
            Phase::Downloading { .. } => 2,
            Phase::Installed { .. } => 3,
        }
    }
}

/// What the session's message makes of where an update stands; none when it changes nothing.
pub fn next(now: &Phase, m: &Value) -> Option<Phase> {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    let available = |v: &Value| {
        let version = v["version"].as_str().unwrap_or("").to_string();
        let said = v["said"].as_str().unwrap_or("").to_string();
        (v["available"].as_bool() == Some(true) && !version.is_empty()).then_some(Phase::Available { version, said })
    };
    let next = match m["type"].as_str().unwrap_or("") {
        // An update in place or coming down is not undone by another look.
        "state" | "update_checked" if matches!(now, Phase::Downloading { .. } | Phase::Installed { .. }) => None,
        "state" => {
            let u = &m["update"];
            if !u.is_object() {
                return None;
            }
            Some(available(u).unwrap_or_default())
        }
        "update_checked" => Some(available(m).unwrap_or_default()),
        "update_progress" => {
            let pct = m["pct"].as_u64().unwrap_or(0).min(100) as u8;
            let said = match now {
                Phase::Available { said, .. } | Phase::Downloading { said, .. } => said.clone(),
                _ => String::new(),
            };
            Some(Phase::Downloading { version: s("version"), pct, said })
        }
        "update_done" if m["error"].is_string() => match now {
            Phase::Downloading { version, said, .. } => {
                Some(Phase::Available { version: version.clone(), said: said.clone() })
            }
            _ => None,
        },
        "update_done" => Some(Phase::Installed { version: s("version"), program: s("program"), said: s("said") }),
        _ => None,
    };
    next.filter(|n| n != now)
}

/// The part's state: where the update stands, and the version a toast told of.
#[derive(Clone, Copy)]
struct Update {
    phase: RwSignal<Phase>,
    told: RwSignal<String>,
}

impl State for Update {
    fn new(ui: Ui) -> Self {
        Update { phase: ui.scope.create_rw_signal(Phase::None), told: ui.scope.create_rw_signal(String::new()) }
    }
}

/// Asks the session to download and put the update in place.
fn install(send: &Send) {
    send(json!({"type": "update_install"}));
}

/// The new program started with this one's arguments, and this window closed (asking for unsaved files first).
fn restart(ui: Ui, send: &Send, program: &str) {
    match std::process::Command::new(program).args(std::env::args_os().skip(1)).spawn() {
        Ok(_) => {
            commands::run(ui, send, crate::workbench::QUIT);
        }
        Err(e) => {
            toast::error(ui, &format!("Code Arena could not be started again ({e}): close it and start it yourself."));
        }
    }
}

/// What a click on the item, or Restart to Update, does now.
fn act(ui: Ui, send: &Send) {
    match ui.state::<Update>().phase.get_untracked() {
        Phase::Available { .. } => install(send),
        Phase::Installed { program, .. } => restart(ui, send, &program),
        Phase::Downloading { .. } | Phase::None => {}
    }
}

/// What it adds to the window's registries, once at start: the status bar's item, the commands, and Check for
/// Updates... in the Manage menu.
pub fn register(_ui: Ui, _send: &Send) {
    let phase_is = |k: u8| move |ui: Ui| ui.state::<Update>().phase.with_untracked(|p| p.kind() == k);
    commands::add(command(CHECK, "Code Arena", "Check for Updates...", |_, send| {
        send(json!({"type": "update_check"}));
    }));
    commands::add(command(INSTALL, "Code Arena", "Install Update", |_, send| install(send)).enabled(phase_is(1)));
    commands::add(command(RESTART, "Code Arena", "Restart to Update", act).enabled(phase_is(3)));
    {
        use crate::menus::{Menu, add, item};
        add(Menu::Manage, item(CHECK).title("Check for Updates...").group("7_update").order(1));
    }

    statusbar::add("status.update", Side::Right, 150, |ui, send| {
        let t = ui.colors;
        let me = ui.state::<Update>();
        let kind = create_memo(move |_| me.phase.with(Phase::kind));
        let text = create_memo(move |_| me.phase.with(Phase::text));
        let item = h_stack((
            dyn_container(
                move || kind.get(),
                |k| {
                    match k {
                        2 => icon("cloud-download", 16.0),
                        3 => icon("debug-restart", 16.0),
                        _ => icon("arrow-circle-up", 16.0),
                    }
                    .into_any()
                },
            ),
            label(move || text.get()),
        ))
        .style(|s| s.gap(4.0).items_center())
        .on_click_stop(move |_| act(ui, &send));
        tooltip(item, move || label(move || me.phase.with(Phase::tip)))
            .style(move |s| {
                statusbar::item(t, s)
                    .cursor(CursorStyle::Pointer)
                    .apply_if(kind.get() == 0, |s| s.hide())
                    .apply_if(kind.get() == 2, |s| s.cursor(CursorStyle::Default))
            })
            .into_any()
    });
}

/// The toast a message brings, if any.
fn tell(ui: Ui, m: &Value, now: &Phase) {
    let me = ui.state::<Update>();
    match m["type"].as_str().unwrap_or("") {
        "state" | "update_checked" => {
            if let Phase::Available { version, said } = now
                && !me.told.with_untracked(|t| t == version)
            {
                me.told.set(version.clone());
                toast::show(
                    ui,
                    toast::info_of(said)
                        .source("Code Arena")
                        .action("Update Now", |_, send| install(send))
                        .action("Later", |_, _| {}),
                );
            } else if m["asked"].as_bool() == Some(true) && !matches!(now, Phase::Available { .. }) {
                match m["error"].as_str() {
                    Some(e) => toast::error(ui, e),
                    None => toast::info(ui, "There are currently no updates available."),
                };
            }
        }
        "update_done" => match m["error"].as_str() {
            Some(e) => {
                toast::error(ui, e);
            }
            None => {
                let said = m["said"].as_str().unwrap_or("Code Arena is updated: restart to run it.");
                toast::show(ui, toast::info_of(said).source("Code Arena").action("Restart", act).sticky());
            }
        },
        _ => {}
    }
}

/// The session's messages that are this part's, taken in: `state.update`, `update_checked`, `update_progress`
/// and `update_done`. Never taken away (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let me = ui.state::<Update>();
    let kind = m["type"].as_str().unwrap_or("");
    if !matches!(kind, "state" | "update_checked" | "update_progress" | "update_done") {
        return false;
    }
    let was = me.phase.get_untracked();
    let now = match next(&was, m) {
        Some(n) => {
            me.phase.set(n.clone());
            n
        }
        None => was,
    };
    tell(ui, m, &now);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available(v: &str) -> Phase {
        Phase::Available { version: v.into(), said: format!("Code Arena {v} is available from Arena (this is 1.0.0).") }
    }

    #[test]
    fn an_update_goes_from_offered_to_downloaded_to_in_place() {
        let said = "Code Arena 2.0.0 is available from Arena (this is 1.0.0).";
        let state = json!({"type": "state", "update": {"available": true, "version": "2.0.0", "said": said}});
        assert_eq!(next(&Phase::None, &state), Some(available("2.0.0")));
        assert_eq!(next(&available("2.0.0"), &state), None, "the same again changes nothing");
        let p = json!({"type": "update_progress", "version": "2.0.0", "pct": 40});
        let down = Phase::Downloading { version: "2.0.0".into(), pct: 40, said: said.into() };
        assert_eq!(next(&available("2.0.0"), &p), Some(down.clone()));
        assert_eq!(next(&down, &p), None);
        assert_eq!(next(&down, &state), None, "a look while it comes down leaves it");
        let done = json!({"type": "update_done", "version": "2.0.0", "program": "/opt/code-arena", "said": "Code Arena 1.0.0 → 2.0.0: restart to run it."});
        let installed = next(&down, &done).expect("installed");
        assert_eq!(installed.text(), "Updated: restart");
        assert_eq!(installed.tip(), "Code Arena 1.0.0 → 2.0.0: restart to run it. Click to restart");
        assert_eq!(next(&installed, &json!({"type": "update_checked", "available": false})), None);
    }

    #[test]
    fn a_failed_download_offers_it_again_and_nothing_newer_offers_nothing() {
        let said = "Code Arena 2.0.0 is available from Arena (this is 1.0.0).";
        let down = Phase::Downloading { version: "2.0.0".into(), pct: 10, said: said.into() };
        let failed = json!({"type": "update_done", "error": "The download did not match what Arena lists"});
        assert_eq!(next(&down, &failed), Some(available("2.0.0")));
        assert_eq!(next(&Phase::None, &failed), None);
        let none = json!({"type": "update_checked", "available": false, "asked": true});
        assert_eq!(next(&available("2.0.0"), &none), Some(Phase::None));
        assert_eq!(next(&Phase::None, &json!({"type": "state", "version": "1.0.0"})), None);
        assert_eq!(next(&Phase::None, &json!({"type": "state", "update": {"available": false}})), None);
    }

    #[test]
    fn the_items_words() {
        assert_eq!(available("2.0.0").text(), "Update");
        assert_eq!(
            available("2.0.0").tip(),
            "Code Arena 2.0.0 is available from Arena (this is 1.0.0). Click to download and install it"
        );
        assert_eq!(Phase::Downloading { version: "2.0.0".into(), pct: 7, said: String::new() }.text(), "Updating 7%");
        assert_eq!(Phase::None.text(), "");
    }

    #[test]
    fn its_messages_are_taken_in_and_never_taken_away() {
        let ui = Ui::new();
        let send: Send = std::rc::Rc::new(|_| {});
        let m = json!({"type": "update_checked", "available": true, "version": "3.0.0", "said": "x"});
        assert!(!take(ui, &m, &send));
        assert_eq!(ui.state::<Update>().phase.get_untracked().kind(), 1);
        assert_eq!(toast::count(ui), 1, "the newer version is told");
        assert!(!take(ui, &json!({"type": "state", "update": m.clone()}), &send));
        assert_eq!(toast::count(ui), 1, "and told once");
        assert!(!take(ui, &json!({"type": "turn_end"}), &send));
    }
}
