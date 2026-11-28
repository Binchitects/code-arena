//! The banners under the title bar, as VS Code draws its banner (26 px, `banner.*` colours, an icon, the words, an
//! X): the session stopped, no model, Arena's broadcasts. And what covers the window while it has nothing to show:
//! the skeleton until the session's first `state`, and "Opening <folder>…" while another folder is opened.
//!
//! - `driver_stopped` (said by the window itself when the session's messages end): a banner that stays; the editor
//!   and its unsaved text stay as they are.
//! - `state`: `no_model` and `signed_out` (the no-model banner, which the person may close), `broadcasts`.
//! - `broadcasts{items: [{id, level, text}]}`: Arena's, the newest first; X sends `broadcast_dismiss{id}`, a
//!   critical one has none.
//! - `settings_changed{models, model_info}`: Arena's settings moved; the pickers offer its models as they are now.

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, dyn_stack, empty, h_stack, label, v_stack};
use floem::IntoView;
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layers::{self, Place};
use crate::state::{Send, Ui};

/// How much a broadcast matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    Info,
    Warning,
    Critical,
}

/// One banner's row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub id: String,
    pub level: Level,
    pub text: String,
}

/// This part's state in a window.
#[derive(Clone, Copy)]
pub struct Banners {
    /// The session's messages ended.
    pub stopped: RwSignal<bool>,
    /// What the no-model banner says (none: the session has a model), and whether the person closed it.
    pub no_model: RwSignal<Option<&'static str>>,
    pub no_model_closed: RwSignal<bool>,
    pub broadcasts: RwSignal<Vec<Shown>>,
    /// The folder being opened, until the session's next `state`.
    pub opening: RwSignal<Option<String>>,
}

impl crate::ext::State for Banners {
    fn new(ui: Ui) -> Self {
        Banners {
            stopped: ui.scope.create_rw_signal(false),
            no_model: ui.scope.create_rw_signal(None),
            no_model_closed: ui.scope.create_rw_signal(false),
            broadcasts: ui.scope.create_rw_signal(Vec::new()),
            opening: ui.scope.create_rw_signal(None),
        }
    }
}

/// The words of the banner while the session stops.
pub const STOPPED: &str = "Code Arena's session has stopped: the editor and your unsaved text stay, but nothing is saved, run or answered until the IDE is opened again.";

/// What the no-model banner says, from `state`; none while the session has a model.
pub fn no_model(m: &Value) -> Option<&'static str> {
    if m["no_model"] != true {
        return None;
    }
    Some(if m["signed_out"] == true {
        "Not signed in to an Arena: the editor, terminal, git and tasks work, the agent has no model. Sign in with code-arena login and open the IDE again."
    } else {
        "The gateway lists no model for your key: the editor, terminal, git and tasks work, the agent has no model."
    })
}

/// Arena's broadcasts as the session lists them.
pub fn broadcasts(items: &Value) -> Vec<Shown> {
    items
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| {
            let level = match b["level"].as_str().unwrap_or("") {
                "critical" => Level::Critical,
                "warning" => Level::Warning,
                _ => Level::Info,
            };
            Some(Shown { id: b["id"].as_str()?.to_string(), level, text: b["text"].as_str()?.to_string() })
        })
        .collect()
}

/// "Opening <folder>…" covers the window until the session's next `state` (Open Folder, Open Recent call it).
// Open Folder, to come, calls it.
#[allow(dead_code)]
pub fn opening(ui: Ui, folder: &str) {
    set_if_new(ui.state::<Banners>().opening, Some(folder.to_string()));
}

fn set_if_new<T: PartialEq + 'static>(s: RwSignal<T>, v: T) {
    if s.with_untracked(|now| *now != v) {
        s.set(v);
    }
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    layers::add(Place::Banner, "banners.stopped", 0, |ui, _| stopped(ui).into_any());
    layers::add(Place::Banner, "banners.no-model", 10, |ui, _| no_model_banner(ui).into_any());
    layers::add(Place::Banner, "banners.broadcasts", 20, |ui, send| broadcasts_banner(ui, send).into_any());
    layers::add(Place::Over, "banners.skeleton", 5, |ui, _| skeleton(ui).into_any());
    layers::add(Place::Over, "banners.opening", 60, |ui, _| opening_cover(ui).into_any());
}

/// The session's messages that are this part's, taken in. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let me = ui.state::<Banners>();
    match m["type"].as_str().unwrap_or("") {
        "driver_stopped" => {
            set_if_new(me.stopped, true);
            set_if_new(me.opening, None);
        }
        "state" => {
            set_if_new(me.no_model, no_model(m));
            if m.get("broadcasts").is_some() {
                set_if_new(me.broadcasts, broadcasts(&m["broadcasts"]));
            }
            set_if_new(me.opening, None);
        }
        "broadcasts" => set_if_new(me.broadcasts, broadcasts(&m["items"])),
        "settings_changed" => {
            let models: Vec<String> =
                m["models"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            if !models.is_empty() {
                set_if_new(ui.models, models);
            }
        }
        _ => {}
    }
    false
}

/// One banner: VS Code's 26 px row, its icon, its words, and an X when it may be closed.
fn row(
    ui: Ui,
    glyph: &'static str,
    colour: impl Fn(&crate::colors::Colors) -> floem::peniko::Color + 'static,
    text: impl Fn() -> String + 'static,
    close: Option<Box<dyn Fn()>>,
) -> impl IntoView {
    let t = ui.colors;
    let closable = close.is_some();
    let x = container(icon("close", 16.0))
        .on_click_stop(move |_| {
            if let Some(c) = &close {
                c();
            }
        })
        .style(move |s| {
            let c = t.get();
            s.width(22.0)
                .height(22.0)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
                .border_radius(5.0)
                .cursor(CursorStyle::Pointer)
                .color(c.banner_fg)
                .hover(|s| s.background(c.toolbar_hover))
                .apply_if(!closable, |s| s.hide())
        });
    h_stack((
        icon(glyph, 16.0).style(move |s| s.flex_shrink(0.0_f32).margin_right(6.0).color(colour(&t.get()))),
        label(text).style(|s| {
            s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).font_size(12.0).text_overflow(TextOverflow::Ellipsis)
        }),
        x,
    ))
    .style(move |s| {
        let c = t.get();
        s.width_full().height(26.0).padding_horiz(8.0).items_center().background(c.banner_bg).color(c.banner_fg)
    })
}

fn stopped(ui: Ui) -> impl IntoView {
    let me = ui.state::<Banners>();
    container(row(ui, "debug-disconnect", |c| c.notify_warning, || STOPPED.to_string(), None))
        .style(move |s| s.width_full().apply_if(!me.stopped.get(), |s| s.hide()))
}

fn no_model_banner(ui: Ui) -> impl IntoView {
    let me = ui.state::<Banners>();
    let close: Box<dyn Fn()> = Box::new(move || me.no_model_closed.set(true));
    container(row(ui, "info", |c| c.banner_icon, move || me.no_model.get().unwrap_or("").to_string(), Some(close)))
        .style(move |s| {
            let shown = me.no_model.get().is_some() && !me.no_model_closed.get() && !me.stopped.get();
            s.width_full().apply_if(!shown, |s| s.hide())
        })
}

fn broadcasts_banner(ui: Ui, send: Send) -> impl IntoView {
    let me = ui.state::<Banners>();
    dyn_stack(
        move || me.broadcasts.get(),
        |b: &Shown| (b.id.clone(), b.level, b.text.clone()),
        move |b| {
            let (glyph, colour): (&'static str, fn(&crate::colors::Colors) -> floem::peniko::Color) = match b.level {
                Level::Info => ("info", |c| c.banner_icon),
                Level::Warning => ("warning", |c| c.notify_warning),
                Level::Critical => ("error", |c| c.notify_error),
            };
            let close: Option<Box<dyn Fn()>> = (b.level != Level::Critical).then(|| {
                let (send, id) = (send.clone(), b.id.clone());
                Box::new(move || {
                    me.broadcasts.update(|l| l.retain(|x| x.id != id));
                    send(json!({"type": "broadcast_dismiss", "id": id}));
                }) as Box<dyn Fn()>
            });
            let text = b.text.clone();
            row(ui, glyph, colour, move || text.clone(), close)
        },
    )
    .style(|s| s.flex_col().width_full())
}

/// The window's shape, drawn plain until the session's first `state`: the activity bar, the side bar with a few
/// rows, the editor, the status bar (under the title bar, which stays usable; the banners' row is drawn over by it
/// only while it shows).
fn skeleton(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let bar = move |w: f64| {
        empty().style(move |s| s.height(10.0).width(w).margin_bottom(12.0).border_radius(3.0).background(t.get().list_hover))
    };
    let side = v_stack((bar(150.0), bar(110.0), bar(170.0), bar(90.0), bar(130.0)))
        .style(move |s| s.width(260.0).height_full().padding(16.0).background(t.get().side_bg));
    let activity = empty().style(move |s| s.width(48.0).height_full().background(t.get().activity_bg));
    let editor = container(label(|| "Starting the session…".to_string()))
        .style(move |s| s.flex_grow(1.0_f32).height_full().items_center().justify_center().color(t.get().desc));
    let body = h_stack((activity, side, editor)).style(|s| s.flex_grow(1.0_f32).width_full());
    let status = empty().style(move |s| s.height(22.0).width_full().background(t.get().status_bg));
    v_stack((body, status)).style(move |s| {
        s.absolute()
            .inset_top(35.0)
            .inset_left(0.0)
            .inset_right(0.0)
            .inset_bottom(0.0)
            .background(t.get().editor_bg)
            // Gone once the session has spoken, or once it has stopped (its banner is then the window's word).
            .apply_if(ui.loaded.get() || ui.state::<Banners>().stopped.get(), |s| s.hide())
    })
}

fn opening_cover(ui: Ui) -> impl IntoView {
    let (t, me) = (ui.colors, ui.state::<Banners>());
    container(label(move || format!("Opening {}…", me.opening.get().unwrap_or_default())).style(|s| s.font_size(14.0)))
        .style(move |s| {
            let c = t.get();
            s.absolute()
                .inset_top(35.0)
                .inset_left(0.0)
                .inset_right(0.0)
                .inset_bottom(0.0)
                .items_center()
                .justify_center()
                .background(c.editor_bg)
                .color(c.desc)
                .apply_if(me.opening.get().is_none(), |s| s.hide())
        })
}

#[cfg(test)]
mod tests {
    use floem::reactive::SignalTrack;

    use super::*;

    #[test]
    fn the_banners_follow_what_the_session_says() {
        let ui = Ui::new();
        let send: Send = std::rc::Rc::new(|_| {});
        let me = ui.state::<Banners>();
        assert!(!take(ui, &json!({"type": "state", "no_model": true, "signed_out": true}), &send));
        assert!(me.no_model.get_untracked().expect("said").starts_with("Not signed in to an Arena"));
        take(ui, &json!({"type": "state", "no_model": false, "broadcasts": [
            {"id": "a", "level": "critical", "text": "Down"}, {"id": "b", "level": "info", "text": "Up"}]}), &send);
        assert_eq!(me.no_model.get_untracked(), None);
        assert_eq!(me.broadcasts.get_untracked().iter().map(|b| b.level).collect::<Vec<_>>(), [Level::Critical, Level::Info]);
        take(ui, &json!({"type": "broadcasts", "items": [{"id": "b", "level": "warning", "text": "Slow"}]}), &send);
        assert_eq!(me.broadcasts.get_untracked(), [Shown { id: "b".into(), level: Level::Warning, text: "Slow".into() }]);
        take(ui, &json!({"type": "settings_changed", "models": ["m1", "m2"]}), &send);
        assert_eq!(ui.models.get_untracked(), ["m1", "m2"]);
    }

    #[test]
    fn the_stopped_banner_comes_when_the_session_ends_and_the_editor_stays() {
        let ui = Ui::new();
        let send: Send = std::rc::Rc::new(|_| {});
        let me = ui.state::<Banners>();
        opening(ui, "/tmp/other");
        assert_eq!(me.opening.get_untracked().as_deref(), Some("/tmp/other"));
        ui.draft.set("unsaved words".into());
        take(ui, &json!({"type": "driver_stopped"}), &send);
        assert!(me.stopped.get_untracked());
        assert_eq!(me.opening.get_untracked(), None);
        assert_eq!(ui.draft.get_untracked(), "unsaved words");
    }

    #[test]
    fn a_banner_s_state_is_set_only_when_it_changes() {
        let ui = Ui::new();
        let me = ui.state::<Banners>();
        let sets = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = sets.clone();
        floem::reactive::create_effect(move |_| {
            me.broadcasts.track();
            seen.set(seen.get() + 1);
        });
        let send: Send = std::rc::Rc::new(|_| {});
        let m = json!({"type": "broadcasts", "items": [{"id": "a", "level": "info", "text": "x"}]});
        take(ui, &m, &send);
        take(ui, &m, &send);
        assert_eq!(sets.get(), 2, "the first run, then one change");
    }
}
