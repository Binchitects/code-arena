//! The status bar's items of the agent and of the window, as the old Code Arena's status bar has them and in VS
//! Code's look: on the left the remote item ("Local", which offers Open Folder and Open Recent) and a spinner with
//! the count of the commands running with no time limit (which shows them in the chat); on the right the last
//! turn ("12 s · 8.6K in (73% cached) · 1.2K out"), the agent's mode (red in Yolo; it opens the mode pick), the
//! model (it opens the model pick), a dot for the session's connection and "code-arena <version>" (About). Each is
//! clickable and says what it is in its tooltip.
//!
//! What they show comes from the session: `state` (the version), `turn_end` (the last turn's time and tokens),
//! `driver_stopped` (said by the window when the session's messages end), and the window's own signals (the mode,
//! the model, the jobs running). Nothing is taken away from the parts after this one.

use std::time::Duration;

use floem::IntoView;
use floem::action::exec_after;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, h_stack, label, tooltip};
use serde_json::Value;

use crate::agent::answer::{duration, usage_line, usage_tip};
use crate::commands::{self, command};
use crate::ext::State;
use crate::icons::icon;
use crate::model::Usage;
use crate::state::{Picker, Send, Ui};
use crate::statusbar::{self, Side};
use crate::widgets::{quick, toast};

/// The commands the status bar's mode and model open, as VS Code's chat names them.
pub const PICK_MODE: &str = "workbench.action.chat.openModePicker";
pub const PICK_MODEL: &str = "workbench.action.chat.openModelPicker";
/// What the remote item offers, when their parts have added them.
const OPEN_FOLDER: &str = "workbench.action.files.openFolder";
const OPEN_RECENT: &str = "workbench.action.openRecent";
/// How often the spinners turn.
const TICK: Duration = Duration::from_millis(100);

/// What the items keep of the session.
#[derive(Clone, Copy)]
struct Items {
    /// The session's version (`state.version`); empty until it says it.
    version: RwSignal<String>,
    /// The last turn that ended well: its time and tokens.
    last: RwSignal<Option<Usage>>,
    /// Whether the session's messages have ended.
    stopped: RwSignal<bool>,
    /// The spinners' count, and whether its timer runs.
    tick: RwSignal<u64>,
    ticking: RwSignal<bool>,
}

impl State for Items {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        Items {
            version: s.create_rw_signal(String::new()),
            last: s.create_rw_signal(None),
            stopped: s.create_rw_signal(false),
            tick: s.create_rw_signal(0),
            ticking: s.create_rw_signal(false),
        }
    }
}

/// The version as the item says it: the session's, else the window's own.
fn version_text(version: &str) -> String {
    let v = if version.is_empty() { env!("CARGO_PKG_VERSION") } else { version };
    format!("code-arena {v}")
}

/// The last turn's line: "12 s · 8.6K in (73% cached) · 1.2K out" (what it does not say is left out).
fn last_turn_line(u: &Usage) -> String {
    let mut parts = Vec::new();
    if u.ms > 0 {
        parts.push(duration(u.ms));
    }
    if u.prompt + u.completion > 0 {
        parts.push(usage_line(u));
    }
    parts.join(" \u{b7} ")
}

/// The last turn's tooltip, exact: "The last turn took 12 s: 8,600 tokens in, …; 1,200 tokens out".
fn last_turn_tip(u: &Usage) -> String {
    let took = if u.ms > 0 { format!(" took {}", duration(u.ms)) } else { String::new() };
    format!("The last turn{took}: {}", usage_tip(u))
}

/// The jobs item's words for its tooltip: "2 commands running with no time limit: …".
fn jobs_tip(n: usize) -> String {
    let what = if n == 1 { "command" } else { "commands" };
    format!("{n} {what} running with no time limit: their output and Stop are in the chat")
}

/// The mode item's tooltip.
fn mode_tip(mode: &str) -> String {
    format!("Agent mode {mode}: how much runs without asking. Click to change it")
}

/// The connection dot's tooltip.
fn connection_tip(stopped: bool) -> &'static str {
    if stopped { "The session has stopped: the window no longer hears from it" } else { "Connected to the session" }
}

/// The last turn's time and tokens as `turn_end` says them, the turn's own time when the tokens' has none; none
/// for a turn that did not end well or says no tokens.
fn usage_of(m: &Value) -> Option<Usage> {
    if m["ok"] != true {
        return None;
    }
    let mut u = Usage::from_json(&m["usage"])?;
    if u.ms == 0 {
        u.ms = m["ms"].as_u64().unwrap_or(0);
    }
    Some(u)
}

/// Turns the spinners while a turn or a job with no time limit runs, and stops once neither does.
fn clock(ui: Ui, me: Items) {
    if me.ticking.get_untracked() {
        return;
    }
    me.ticking.set(true);
    fn go(ui: Ui, me: Items) {
        exec_after(TICK, move |_| {
            if !ui.running.get_untracked() && ui.jobs.with_untracked(Vec::is_empty) {
                me.ticking.set(false);
                return;
            }
            me.tick.update(|n| *n += 1);
            go(ui, me);
        });
    }
    go(ui, me);
}

/// A spinner's turn now: none when motion is reduced.
fn turned(ui: Ui, me: Items) -> f64 {
    if crate::accessibility::still(ui) {
        return 0.0;
    }
    (me.tick.get() % 12) as f64 * std::f64::consts::TAU / 12.0
}

/// An item: what it shows, run on a click, said in its tooltip, in the bar's look.
fn clickable(
    view: impl IntoView + 'static,
    click: impl Fn() + 'static,
    tip: impl Fn() -> String + 'static,
) -> floem::views::Tooltip {
    let tip = std::rc::Rc::new(tip);
    tooltip(view.on_click_stop(move |_| click()), move || {
        let tip = tip.clone();
        label(move || tip())
    })
}

/// The remote item's pick: Open Folder and Open Recent, those that are there; a note when neither is.
fn remote_pick(ui: Ui, send: &Send) {
    let rows: Vec<quick::Item> =
        [(OPEN_FOLDER, "Open Folder...", "folder-opened"), (OPEN_RECENT, "Open Recent...", "history")]
            .into_iter()
            .filter(|(id, _, _)| commands::find(id).is_some())
            .map(|(id, title, glyph)| quick::item(id, title).icon(glyph).detail(&commands::keys_of(id)))
            .collect();
    if rows.is_empty() {
        toast::info(ui, "This window is local: it works on the session's folder on this machine.");
        return;
    }
    let send = send.clone();
    quick::pick("Local").placeholder("Select an option to open a folder").items(rows).open(ui, move |item| {
        if let Some(item) = item {
            commands::run(ui, &send, &item.id);
        }
    });
}

/// What it adds to the window's registries, once at start: the items, and the commands the mode and the model
/// open (in the palette as Chat's).
pub fn register(ui: Ui, _send: &Send) {
    let me = ui.state::<Items>();
    commands::add(command(PICK_MODE, "Chat", "Open Mode Picker", |ui, send| {
        crate::agent::pickers::quick(ui, send, Picker::Mode)
    }));
    commands::add(command(PICK_MODEL, "Chat", "Open Model Picker", |ui, send| {
        crate::agent::pickers::quick(ui, send, Picker::Model)
    }));

    statusbar::add("status.remote", Side::Left, 90, |ui, send| {
        let t = ui.colors;
        clickable(
            container(icon("remote", 16.0)).style(|s| s.height_full().items_center()),
            move || remote_pick(ui, &send),
            || "Local: Open Folder or Open Recent".into(),
        )
        .style(move |s| {
            let c = t.get();
            s.height_full()
                .padding_horiz(10.0)
                .items_center()
                .background(c.remote_bg)
                .color(c.remote_fg)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.status_prominent_hover))
        })
        .into_any()
    });

    statusbar::add("status.jobs", Side::Left, 140, move |ui, send| {
        let t = ui.colors;
        let n = create_memo(move |_| ui.jobs.with(Vec::len));
        create_effect(move |_| {
            if n.get() > 0 {
                clock(ui, me);
            }
        });
        clickable(
            h_stack((
                icon("loading", 16.0).style(move |s| s.rotate(turned(ui, me))),
                label(move || n.get().to_string()),
            ))
            .style(|s| s.gap(4.0).items_center()),
            move || {
                commands::run(ui, &send, crate::workbench::SHOW_AGENT);
            },
            move || jobs_tip(n.get()),
        )
        .style(move |s| statusbar::item(t, s).cursor(CursorStyle::Pointer).apply_if(n.get() == 0, |s| s.hide()))
        .into_any()
    });

    statusbar::add("status.lastTurn", Side::Right, 285, move |ui, send| {
        let t = ui.colors;
        let line = create_memo(move |_| me.last.with(|u| u.as_ref().map(last_turn_line).unwrap_or_default()));
        clickable(
            h_stack((icon("pulse", 16.0), label(move || line.get()))).style(|s| s.gap(4.0).items_center()),
            move || {
                commands::run(ui, &send, crate::workbench::SHOW_AGENT);
            },
            move || me.last.with(|u| u.as_ref().map(last_turn_tip).unwrap_or_default()),
        )
        .style(move |s| {
            statusbar::item(t, s).cursor(CursorStyle::Pointer).apply_if(line.with(String::is_empty), |s| s.hide())
        })
        .into_any()
    });

    statusbar::add("status.mode", Side::Right, 290, |ui, send| {
        let t = ui.colors;
        let mode = create_memo(move |_| crate::agent::pickers::shown(ui, Picker::Mode));
        let yolo = move || mode.with(|m| m.eq_ignore_ascii_case("yolo"));
        clickable(
            h_stack((icon("shield", 16.0), label(move || mode.get()))).style(|s| s.gap(4.0).items_center()),
            move || {
                commands::run(ui, &send, PICK_MODE);
            },
            move || mode_tip(&mode.get()),
        )
        .style(move |s| {
            let c = t.get();
            statusbar::item(t, s)
                .cursor(CursorStyle::Pointer)
                .apply_if(yolo(), |s| s.background(c.status_error_bg).color(c.status_error_fg))
                .apply_if(mode.with(String::is_empty), |s| s.hide())
        })
        .into_any()
    });

    statusbar::add("status.model", Side::Right, 291, move |ui, send| {
        let t = ui.colors;
        let model = create_memo(move |_| crate::agent::pickers::shown(ui, Picker::Model));
        let running = create_memo(move |_| ui.running.get());
        create_effect(move |_| {
            if running.get() {
                clock(ui, me);
            }
        });
        clickable(
            h_stack((
                floem::views::dyn_container(
                    move || running.get(),
                    move |r| {
                        if r {
                            icon("loading", 16.0).style(move |s| s.rotate(turned(ui, me))).into_any()
                        } else {
                            icon("sparkle", 16.0).into_any()
                        }
                    },
                ),
                label(move || model.get()).style(|s| s.min_width(0.0).text_overflow(TextOverflow::Ellipsis)),
            ))
            .style(|s| s.gap(4.0).items_center().min_width(0.0)),
            move || {
                commands::run(ui, &send, PICK_MODEL);
            },
            move || format!("The agent's model: {}. Click to change it", model.get()),
        )
        .style(move |s| {
            statusbar::item(t, s)
                .cursor(CursorStyle::Pointer)
                .min_width(0.0)
                .max_width(240.0)
                .apply_if(model.with(String::is_empty), |s| s.hide())
        })
        .into_any()
    });

    statusbar::add("status.connection", Side::Right, 297, move |ui, send| {
        let t = ui.colors;
        let stopped = create_memo(move |_| me.stopped.get());
        clickable(
            floem::views::dyn_container(
                move || stopped.get(),
                |s| if s { icon("debug-disconnect", 14.0) } else { icon("circle-filled", 10.0) }.into_any(),
            ),
            move || {
                commands::run(ui, &send, crate::layout::View::Output.command());
            },
            move || connection_tip(stopped.get()).to_string(),
        )
        .style(move |s| {
            let c = t.get();
            statusbar::item(t, s)
                .cursor(CursorStyle::Pointer)
                .apply_if(stopped.get(), |s| s.background(c.status_error_bg).color(c.status_error_fg))
        })
        .into_any()
    });

    statusbar::add("status.version", Side::Right, 298, move |ui, send| {
        let t = ui.colors;
        clickable(
            h_stack((icon("info", 16.0), label(move || version_text(&me.version.get()))))
                .style(|s| s.gap(4.0).items_center()),
            move || {
                commands::run(ui, &send, crate::workbench::ABOUT);
            },
            || "About Code Arena: version, licence and source".into(),
        )
        .style(move |s| statusbar::item(t, s).cursor(CursorStyle::Pointer))
        .into_any()
    });
}

/// The session's messages that are this part's, taken in: the version (`state`), the last turn (`turn_end`) and
/// the end of the session's messages (`driver_stopped`). Never taken away (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let me = ui.state::<Items>();
    match m["type"].as_str().unwrap_or("") {
        "state" => {
            if let Some(v) = m["version"].as_str()
                && !me.version.with_untracked(|old| old == v)
            {
                me.version.set(v.to_string());
            }
        }
        "turn_end" => {
            if let Some(u) = usage_of(m)
                && me.last.with_untracked(|old| old.as_ref() != Some(&u))
            {
                me.last.set(Some(u));
            }
        }
        "driver_stopped" if !me.stopped.get_untracked() => me.stopped.set(true),
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn usage(prompt: u64, cached: u64, completion: u64, ms: u64) -> Usage {
        Usage { prompt, cached, completion, ms, cost: None }
    }

    #[test]
    fn the_last_turn_line_says_its_time_and_tokens_and_leaves_out_what_is_not_said() {
        assert_eq!(
            last_turn_line(&usage(8600, 6278, 1200, 12_000)),
            "12 s \u{b7} 8.6K in (73% cached) \u{b7} 1.2K out"
        );
        assert_eq!(last_turn_line(&usage(0, 0, 0, 3_000)), "3 s");
        assert_eq!(last_turn_line(&usage(900, 0, 40, 0)), "900 in \u{b7} 40 out");
        assert!(
            last_turn_tip(&usage(8600, 6278, 1200, 12_000)).starts_with("The last turn took 12 s: 8,600 tokens in")
        );
    }

    #[test]
    fn a_turn_end_gives_its_usage_with_the_turns_own_time_and_a_failed_one_none() {
        let ok = json!({"type": "turn_end", "ok": true, "ms": 4000, "usage": {"prompt": 10, "completion": 2}});
        assert_eq!(usage_of(&ok), Some(usage(10, 0, 2, 4000)));
        let own =
            json!({"type": "turn_end", "ok": true, "ms": 4000, "usage": {"prompt": 10, "completion": 2, "ms": 3500}});
        assert_eq!(usage_of(&own).map(|u| u.ms), Some(3500));
        assert_eq!(usage_of(&json!({"type": "turn_end", "ok": false, "ms": 10})), None);
        assert_eq!(usage_of(&json!({"type": "turn_end", "ok": true, "ms": 10})), None);
    }

    #[test]
    fn the_words_of_the_jobs_the_mode_the_version_and_the_connection() {
        assert!(jobs_tip(1).starts_with("1 command running with no time limit"));
        assert!(jobs_tip(3).starts_with("3 commands running"));
        assert_eq!(mode_tip("Yolo"), "Agent mode Yolo: how much runs without asking. Click to change it");
        assert_eq!(version_text("1.2.3"), "code-arena 1.2.3");
        assert_eq!(version_text(""), format!("code-arena {}", env!("CARGO_PKG_VERSION")));
        assert_eq!(connection_tip(false), "Connected to the session");
        assert!(connection_tip(true).starts_with("The session has stopped"));
    }

    #[test]
    fn its_messages_are_taken_in_and_never_taken_away() {
        let ui = Ui::new();
        let send: Send = std::rc::Rc::new(|_| {});
        let me = ui.state::<Items>();
        assert!(!take(ui, &json!({"type": "state", "version": "9.9.9"}), &send));
        assert_eq!(me.version.get_untracked(), "9.9.9");
        let end = json!({"type": "turn_end", "ok": true, "ms": 2000, "usage": {"prompt": 5, "completion": 1}});
        assert!(!take(ui, &end, &send));
        assert_eq!(me.last.get_untracked(), Some(usage(5, 0, 1, 2000)));
        assert!(!take(ui, &json!({"type": "turn_end", "ok": false}), &send));
        assert_eq!(me.last.get_untracked(), Some(usage(5, 0, 1, 2000)), "a failed turn keeps the last good one");
        assert!(!me.stopped.get_untracked());
        assert!(!take(ui, &json!({"type": "driver_stopped"}), &send));
        assert!(me.stopped.get_untracked());
    }
}
