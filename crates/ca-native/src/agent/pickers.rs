//! The input's pickers, as VS Code's chat input has them: the mode, the model and how much it thinks, each a pill
//! opening a menu over itself.
//!
//! What they offer is the session's (`state`: `models` and `model_info`, `thinking_levels`, `modes`), with the
//! window's own lists when a session says none. A model's row says what the gateway knows of it (its window, its
//! longest answer, whether it calls tools, thinks, sees images); one that is not loaded and does not load when
//! asked cannot be chosen. The thinking pill is there only while the model thinks.
//!
//! The pickers work while a turn runs: the session takes the choice when the turn ends (`deferred`), so until its
//! `status` says it the pill shows what was chosen and the menu says it applies to the next turn.

use std::rc::Rc;

use floem::event::EventListener;
use floem::keyboard::{Key, NamedKey};
use floem::kurbo::Rect;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect};
use floem::style::{CursorStyle, Position, TextOverflow};
use floem::views::{Decorators, dyn_stack, empty, h_stack, label, scroll, stack, v_stack};
use floem::{IntoView, View, ViewId};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::state::{Picker, Send, Ui};
use crate::widgets::toast;

/// A menu's least and most width.
const MENU_WIDTH: f64 = 160.0;
const MENU_MOST: f64 = 460.0;

/// The modes the session takes (what runs without asking), and how much it may think, when a session says none.
const MODES: &[(&str, &str)] = &[
    ("ask", "Edits and commands ask first"),
    ("auto-edit", "File edits run without asking; commands ask"),
    ("plan", "Read-only: no edits, no commands"),
    ("yolo", "Nothing asks: edits and commands run at once"),
];
const THINKING: &[&str] = &["default", "off", "low", "medium", "high", "xhigh"];

/// What the menus say while a turn runs.
const NEXT_TURN: &str = "Applies to the next turn";
/// What the model menu says when there is no model.
const NO_MODEL: &str = "The gateway lists no model.";
/// What choosing Yolo warns of.
const YOLO_WARNING: &str = "Yolo mode: edits and commands now run without asking. Choose Ask to be asked again.";

/// A model as the session describes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelRow {
    pub id: String,
    /// Its window and its longest answer, in tokens.
    pub context: Option<u64>,
    pub max_output: Option<u64>,
    pub tools: bool,
    pub thinking: bool,
    pub vision: bool,
    /// Whether it is loaded now (none: not said), and whether it is loaded when asked for.
    pub loaded: Option<bool>,
    pub on_request: bool,
}

impl ModelRow {
    fn from_json(v: &Value) -> Option<ModelRow> {
        let flag = |k: &str, d: bool| v[k].as_bool().unwrap_or(d);
        Some(ModelRow {
            id: v["id"].as_str()?.to_string(),
            context: v["context"].as_u64(),
            max_output: v["max_output"].as_u64(),
            tools: flag("tools", true),
            thinking: flag("thinking", true),
            vision: flag("vision", false),
            loaded: v["loaded"].as_bool(),
            on_request: flag("on_request", false),
        })
    }

    /// Whether it may be chosen: not when it is not loaded and is not loaded when asked.
    fn can_choose(&self) -> bool {
        self.loaded != Some(false) || self.on_request
    }

    /// What its row says of it, as the old Code Arena's badges.
    fn badges(&self) -> String {
        let mut said = Vec::new();
        match self.loaded {
            Some(false) if self.on_request => said.push("Loads when asked".to_string()),
            Some(false) => said.push("Not loaded now".to_string()),
            _ => {}
        }
        if let Some(c) = self.context {
            said.push(format!("{} context", tokens(c)));
        }
        if let Some(o) = self.max_output {
            said.push(format!("answers up to {}", tokens(o)));
        }
        for (has, word) in [(self.tools, "Tools"), (self.thinking, "Thinks"), (self.vision, "Sees images")] {
            if has {
                said.push(word.to_string());
            }
        }
        said.join(" · ")
    }
}

/// A count of tokens as a menu says it: 128K, 200K, 1M.
fn tokens(n: u64) -> String {
    const K: u64 = 1024;
    let short = |n: u64, by: u64, unit: &str| {
        let tenths = (n * 10 + by / 2) / by;
        if tenths % 10 == 0 { format!("{}{unit}", tenths / 10) } else { format!("{}.{}{unit}", tenths / 10, tenths % 10) }
    };
    match n {
        n if n >= K * K && n % (K * K) == 0 => format!("{}M", n / (K * K)),
        n if n >= 1_000_000 => short(n, 1_000_000, "M"),
        n if n >= K && n % K == 0 => format!("{}K", n / K),
        n if n >= 1000 => short(n, 1000, "K"),
        n => n.to_string(),
    }
}

/// The pickers' own state: what the session offers, and what was chosen while a turn ran.
#[derive(Clone, Copy)]
pub struct Pickers {
    infos: RwSignal<Vec<ModelRow>>,
    levels: RwSignal<Vec<String>>,
    modes: RwSignal<Vec<(String, String)>>,
    /// Chosen while a turn ran, not yet in force: the session takes it when the turn ends.
    pending: RwSignal<Vec<(Picker, String)>>,
}

impl crate::ext::State for Pickers {
    fn new(ui: Ui) -> Self {
        Pickers {
            infos: ui.scope.create_rw_signal(Vec::new()),
            levels: ui.scope.create_rw_signal(Vec::new()),
            modes: ui.scope.create_rw_signal(Vec::new()),
            pending: ui.scope.create_rw_signal(Vec::new()),
        }
    }
}

/// How a row's name is drawn: the mode that lets everything run in red, the read-only one in the accent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Tone {
    Plain,
    Danger,
    Accent,
}

fn tone(which: Picker, v: &str) -> Tone {
    match (which, v) {
        (Picker::Mode, "yolo") => Tone::Danger,
        (Picker::Mode, "plan") => Tone::Accent,
        _ => Tone::Plain,
    }
}

/// The colour a tone draws with, `plain` for the plain one.
fn tone_color(c: &crate::colors::Colors, tone: Tone, plain: Color) -> Color {
    match tone {
        Tone::Plain => plain,
        Tone::Danger => c.error,
        Tone::Accent => c.focus,
    }
}

/// One row of a menu.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Opt {
    value: String,
    name: String,
    detail: String,
    enabled: bool,
}

/// A name with its first letter a capital, as a menu shows it.
fn capital(s: &str) -> String {
    let mut rest = s.chars();
    rest.next().map(|c| c.to_uppercase().chain(rest).collect()).unwrap_or_default()
}

/// The choice made for a picker while a turn ran, if one waits.
fn pending_of(list: &[(Picker, String)], which: Picker) -> Option<String> {
    list.iter().find(|(w, _)| *w == which).map(|(_, v)| v.clone())
}

/// What a picker has in force, or what was chosen for it while a turn ran.
fn current(ui: Ui, which: Picker) -> String {
    let me = ui.state::<Pickers>();
    if let Some(v) = me.pending.with(|p| pending_of(p, which)) {
        return v;
    }
    match which {
        Picker::Mode => ui.mode.get(),
        Picker::Model => ui.model.get(),
        Picker::Thinking => ui.thinking.get(),
    }
}

/// Whether the model in force thinks: as the session describes it, and yes when it does not say.
fn thinks(infos: &[ModelRow], model: &str) -> bool {
    infos.iter().find(|m| m.id == model).is_none_or(|m| m.thinking)
}

/// What a picker offers. The model in force is listed even when the session did not name it among its models.
fn options(ui: Ui, which: Picker) -> Vec<Opt> {
    let me = ui.state::<Pickers>();
    let opt = |value: &str, detail: &str| Opt {
        value: value.to_string(),
        name: capital(value),
        detail: detail.to_string(),
        enabled: true,
    };
    match which {
        Picker::Mode => {
            let theirs = me.modes.get_untracked();
            if theirs.is_empty() {
                MODES.iter().map(|(m, d)| opt(m, d)).collect()
            } else {
                theirs.iter().map(|(m, d)| opt(m, d)).collect()
            }
        }
        Picker::Thinking => {
            let theirs = me.levels.get_untracked();
            let all: Vec<String> =
                if theirs.is_empty() { THINKING.iter().map(|l| l.to_string()).collect() } else { theirs };
            all.iter().map(|l| opt(l, if l == "default" { "The model's own" } else { "" })).collect()
        }
        Picker::Model => {
            let (mut all, now) = (ui.models.get_untracked(), current(ui, Picker::Model));
            if !now.is_empty() && !all.contains(&now) {
                all.insert(0, now);
            }
            model_options(&all, &me.infos.get_untracked())
        }
    }
}

/// The model menu's rows: each model with what the session says of it.
fn model_options(all: &[String], infos: &[ModelRow]) -> Vec<Opt> {
    all.iter()
        .map(|id| {
            let info = infos.iter().find(|m| &m.id == id);
            Opt {
                value: id.clone(),
                name: id.clone(),
                detail: info.map(ModelRow::badges).unwrap_or_default(),
                enabled: info.is_none_or(ModelRow::can_choose),
            }
        })
        .collect()
}

/// What tells the session a picker's choice.
fn chosen(which: Picker, v: &str) -> serde_json::Value {
    match which {
        Picker::Mode => json!({"type": "mode", "mode": v}),
        Picker::Model => json!({"type": "model", "model": v}),
        Picker::Thinking => json!({"type": "thinking", "level": v}),
    }
}

/// A picker's choice, made: told to the session, and while a turn runs kept to show until the session says it.
fn choose(ui: Ui, send: &Send, which: Picker, v: &str) {
    if which == Picker::Mode && v == "yolo" && current(ui, which) != "yolo" {
        toast::warning(ui, YOLO_WARNING);
    }
    if ui.running.get_untracked() {
        ui.state::<Pickers>().pending.update(|p| {
            p.retain(|(w, _)| *w != which);
            p.push((which, v.to_string()));
        });
    }
    send(chosen(which, v));
}

/// The choices still waited for once the session says what is in force (`status`): one it now has is not; the
/// others still are (at a turn's end it says what it had before it takes those that waited).
fn settled(pending: &[(Picker, String)], status: &Value) -> Vec<(Picker, String)> {
    let said = |which: Picker| {
        let k = match which {
            Picker::Mode => "mode",
            Picker::Model => "model",
            Picker::Thinking => "thinking",
        };
        status[k].as_str().map(str::to_string)
    };
    pending.iter().filter(|(w, v)| said(*w).as_deref() != Some(v.as_str())).cloned().collect()
}

/// The picker a request the session refused was for (`refused{of}`).
fn picker_of(name: &str) -> Option<Picker> {
    match name {
        "mode" => Some(Picker::Mode),
        "model" => Some(Picker::Model),
        "thinking" => Some(Picker::Thinking),
        _ => None,
    }
}

/// One of the input's pickers, as VS Code's chat input has them: what is in force and a chevron, opening its menu
/// over itself. They open while a turn runs too: the choice applies to the next turn. The thinking pill is there
/// only while the model thinks; the mode that lets everything run is red, the read-only one in the accent.
pub(super) fn pill(ui: Ui, which: Picker, anchor: RwSignal<Rect>) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Pickers>();
    let view = h_stack((
        label(move || {
            let now = current(ui, which);
            match which {
                Picker::Mode if now.is_empty() => "Agent".to_string(),
                Picker::Thinking if now.is_empty() => "Thinking".to_string(),
                Picker::Thinking => format!("Thinking: {}", capital(&now)),
                _ => named(which, &now),
            }
        })
        .style(|s| s.min_width(0.0).text_overflow(TextOverflow::Ellipsis)),
        icon("chevron-down", 12.0),
    ));
    let id = view.id();
    view.on_event_stop(EventListener::PointerDown, move |_| {
        if ui.picker.get_untracked() == Some(which) {
            ui.picker.set(None);
            return;
        }
        let now = current(ui, which);
        ui.picker_at.set(options(ui, which).iter().position(|o| o.value == now).unwrap_or(0));
        anchor.set(id.layout_rect());
        ui.picker.set(Some(which));
    })
    .style(move |s| {
        let c = t.get();
        let now = current(ui, which);
        let hidden = which == Picker::Thinking && !me.infos.with(|i| thinks(i, &current(ui, Picker::Model)));
        let lit = ui.picker.get() == Some(which);
        let waits = me.pending.with(|p| pending_of(p, which).is_some());
        s.gap(2.0)
            .items_center()
            .min_width(0.0)
            .padding_horiz(6.0)
            .height(22.0)
            .border_radius(5.0)
            .font_size(12.0)
            .selectable(false)
            .apply_if(which != Picker::Model, |s| s.flex_shrink(0.0_f32))
            .color(tone_color(&c, tone(which, &now), c.desc))
            .apply_if(waits, |s| s.font_style(floem::text::Style::Italic))
            .cursor(CursorStyle::Pointer)
            .apply_if(lit, |s| s.background(c.toolbar_hover))
            .hover(|s| s.background(c.toolbar_hover))
            .apply_if(hidden, |s| s.hide())
    })
}

/// An option as the pill names it: a model by its id, the others with a capital.
fn named(which: Picker, v: &str) -> String {
    if which == Picker::Model { v.to_string() } else { capital(v) }
}

/// The open picker's menu, as VS Code's: 26 px items, a check by the one in force and what each is said of dimmed
/// beside it, in a layer over the whole view (so every item is within the view's bounds and takes the pointer),
/// placed over the pill that opened it, and a note under them while a turn runs. The menu takes the keys while it
/// is open (the arrows, Enter, Escape) and closes on a press anywhere else: in the view, on the layer; elsewhere in
/// the window, by losing the keys. `frame` is the view, whose place in the window is the layer's (a hidden layer
/// has none), and `back` the input, which the keys go back to.
pub(super) fn menu(ui: Ui, send: Send, anchor: RwSignal<Rect>, frame: ViewId, back: ViewId) -> impl IntoView {
    let t = ui.colors;
    let close = move || {
        ui.picker.set(None);
        back.request_focus();
    };
    let pick: Rc<dyn Fn(Picker, Opt)> = Rc::new(move |which, o| {
        if o.enabled {
            close();
            choose(ui, &send, which, &o.value);
        }
    });
    let items = {
        let pick = pick.clone();
        dyn_stack(
            move || {
                let which = ui.picker.get();
                let all = which.map(|w| options(ui, w)).unwrap_or_default();
                all.into_iter().enumerate().filter_map(|(n, o)| Some((n, which?, o))).collect::<Vec<_>>()
            },
            |item: &(usize, Picker, Opt)| item.clone(),
            move |(n, which, o): (usize, Picker, Opt)| {
                let (pick, value, enabled) = (pick.clone(), o.value.clone(), o.enabled);
                let (name, detail, tint) = (o.name.clone(), o.detail.clone(), tone(which, &o.value));
                let no_detail = o.detail.is_empty();
                h_stack((
                    icon("check", 16.0)
                        .style(move |s| s.apply_if(current(ui, which) != value, |s| s.color(Color::TRANSPARENT))),
                    label(move || name.clone()).style(move |s| {
                        let c = t.get();
                        let lit = ui.picker_at.get() == n && enabled;
                        s.flex_shrink(0.0_f32).apply_if(!lit, |s| s.color(tone_color(&c, tint, c.menu_fg)))
                    }),
                    label(move || detail.clone()).style(move |s| {
                        let c = t.get();
                        let lit = ui.picker_at.get() == n && enabled;
                        s.min_width(0.0)
                            .margin_left(6.0)
                            .font_size(12.0)
                            .text_overflow(TextOverflow::Ellipsis)
                            .apply_if(!lit, |s| s.color(c.desc))
                            .apply_if(no_detail, |s| s.hide())
                    }),
                ))
                // On the press: a press takes the keys from the menu, and so would close it before a click.
                .on_event_stop(EventListener::PointerDown, move |_| pick(which, o.clone()))
                .on_event_cont(EventListener::PointerEnter, move |_| {
                    if enabled {
                        ui.picker_at.set(n)
                    }
                })
                .style(move |s| {
                    let c = t.get();
                    let s = s
                        .height(26.0)
                        .items_center()
                        .gap(6.0)
                        .padding_left(6.0)
                        .padding_right(20.0)
                        .margin_horiz(4.0)
                        .border_radius(4.0)
                        .color(c.menu_fg);
                    if enabled {
                        s.cursor(CursorStyle::Pointer).apply_if(ui.picker_at.get() == n, |s| {
                            s.background(c.menu_selected).color(c.menu_selected_fg)
                        })
                    } else {
                        s.color(c.menu_fg.multiply_alpha(0.4))
                    }
                })
            },
        )
        .style(|s| s.flex_col().min_width(MENU_WIDTH).padding_vert(4.0))
    };
    let step = move |by: isize| {
        let all = ui.picker.get_untracked().map(|w| options(ui, w)).unwrap_or_default();
        let at = ui.picker_at.get_untracked();
        if let Some(next) = stepped(&all, at, by) {
            ui.picker_at.set(next);
        }
    };
    // The note under the rows: there is no model, or what is chosen now waits for the turn to end.
    let foot = label(move || {
        if ui.picker.get() == Some(Picker::Model) && ui.models.get().is_empty() && ui.model.get().is_empty() {
            NO_MODEL.to_string()
        } else {
            NEXT_TURN.to_string()
        }
    })
    .style(move |s| {
        let c = t.get();
        let none = ui.picker.get() == Some(Picker::Model) && ui.models.get().is_empty() && ui.model.get().is_empty();
        s.padding_horiz(10.0)
            .padding_vert(5.0)
            .font_size(12.0)
            .color(c.desc)
            .border_top(1.0)
            .border_color(c.menu_separator)
            .apply_if(!none && !ui.running.get(), |s| s.hide())
    });
    let behind = empty().on_event_stop(EventListener::PointerDown, move |_| close()).style(|s| s.size_full());
    let list = v_stack((scroll(items).style(|s| s.min_height(0.0).flex_shrink(1.0_f32)), foot))
        .keyboard_navigable()
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| close())
        .on_key_down(Key::Named(NamedKey::ArrowDown), |_| true, move |_| step(1))
        .on_key_down(Key::Named(NamedKey::ArrowUp), |_| true, move |_| step(-1))
        .on_key_down(
            Key::Named(NamedKey::Enter),
            |_| true,
            move |_| {
                if let Some(which) = ui.picker.get_untracked()
                    && let Some(o) = options(ui, which).get(ui.picker_at.get_untracked())
                {
                    pick(which, o.clone());
                }
            },
        )
        .on_event_cont(EventListener::FocusLost, move |_| ui.picker.set(None))
        .style(move |s| {
            let c = t.get();
            // Over the pill, its left at the pill's, and within the view on every side.
            let (pill, view) = (anchor.get(), frame.layout_rect());
            let left = (pill.x0 - view.x0).min(view.width() - MENU_WIDTH - 12.0).max(4.0);
            s.position(Position::Absolute)
                .inset_left(left)
                .inset_bottom(view.y1 - pill.y0 + 4.0)
                .max_width((view.width() - left - 8.0).clamp(MENU_WIDTH, MENU_MOST))
                .max_height((pill.y0 - view.y0 - 12.0).clamp(60.0, 360.0))
                .border(1.0)
                .border_color(c.menu_border)
                .border_radius(6.0)
                .background(c.menu_bg)
                .box_shadow_blur(8.0)
                .box_shadow_color(c.shadow)
                .font_size(13.0)
                .selectable(false)
        });
    let id = list.id();
    create_effect(move |_| {
        if ui.picker.get().is_some() {
            id.request_focus();
        }
    });
    stack((behind, list)).style(move |s| {
        s.position(Position::Absolute).inset(0.0).z_index(100).apply_if(ui.picker.get().is_none(), |s| s.hide())
    })
}

/// The row the arrows go to from `at`, over the rows that cannot be chosen (none when none can).
fn stepped(all: &[Opt], at: usize, by: isize) -> Option<usize> {
    let n = all.len() as isize;
    (1..=n).map(|k| (at as isize + by * k).rem_euclid(n) as usize).find(|&i| all[i].enabled)
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in: what the pickers offer (`state`), and what is in force
/// (`status`, `refused`), which ends the wait for a choice made while a turn ran. Never taken away (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let me = ui.state::<Pickers>();
    match m["type"].as_str().unwrap_or("") {
        "state" => {
            let infos: Vec<ModelRow> =
                m["model_info"].as_array().into_iter().flatten().filter_map(ModelRow::from_json).collect();
            let levels: Vec<String> =
                m["thinking_levels"].as_array().into_iter().flatten().filter_map(|l| l.as_str().map(str::to_string)).collect();
            let modes: Vec<(String, String)> = m["modes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| Some((v["name"].as_str()?.to_string(), v["description"].as_str().unwrap_or("").to_string())))
                .collect();
            set_if_new(me.infos, infos);
            set_if_new(me.levels, levels);
            set_if_new(me.modes, modes);
            set_if_new(me.pending, Vec::new());
        }
        "status" => {
            let left = me.pending.with_untracked(|p| settled(p, m));
            set_if_new(me.pending, left);
        }
        "refused" => {
            if let Some(which) = m["of"].as_str().and_then(picker_of) {
                me.pending.update(|p| p.retain(|(w, _)| *w != which));
            }
        }
        _ => {}
    }
    false
}

/// Sets a signal only when the value is another.
fn set_if_new<T: PartialEq + 'static>(signal: RwSignal<T>, value: T) {
    if signal.with_untracked(|v| *v != value) {
        signal.set(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menus_names_begin_with_a_capital() {
        assert_eq!(capital("auto-edit"), "Auto-edit");
        assert_eq!(capital("xhigh"), "Xhigh");
        assert_eq!(capital(""), "");
    }

    #[test]
    fn a_count_of_tokens_is_said_short() {
        assert_eq!(tokens(131072), "128K");
        assert_eq!(tokens(200000), "200K");
        assert_eq!(tokens(1048576), "1M");
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(32768), "32K");
        assert_eq!(tokens(8192), "8K");
        assert_eq!(tokens(1500), "1.5K");
        assert_eq!(tokens(512), "512");
    }

    #[test]
    fn a_models_row_says_what_the_session_knows_of_it() {
        let m = ModelRow::from_json(&json!({"id": "qwen", "context": 131072, "max_output": 16384, "vision": true}))
            .expect("a row");
        assert_eq!(m.badges(), "128K context · answers up to 16K · Tools · Thinks · Sees images");
        assert!(m.can_choose());
        let cold = ModelRow::from_json(&json!({"id": "big", "tools": false, "thinking": false, "loaded": false}))
            .expect("a row");
        assert_eq!((cold.badges().as_str(), cold.can_choose()), ("Not loaded now", false));
        let lazy = ModelRow { on_request: true, ..cold.clone() };
        assert_eq!((lazy.badges().as_str(), lazy.can_choose()), ("Loads when asked", true));
        assert!(ModelRow::from_json(&json!({"context": 1})).is_none(), "a row has an id");
    }

    #[test]
    fn a_model_the_session_does_not_describe_has_no_badges_and_may_be_chosen() {
        let infos = [ModelRow { id: "cold".into(), loaded: Some(false), ..Default::default() }];
        let rows = model_options(&["cold".into(), "other".into()], &infos);
        assert_eq!((rows[0].enabled, rows[0].detail.as_str()), (false, "Not loaded now"));
        assert_eq!((rows[1].enabled, rows[1].detail.as_str()), (true, ""));
    }

    #[test]
    fn the_thinking_pick_is_there_only_while_the_model_thinks() {
        let infos = [
            ModelRow { id: "deep".into(), thinking: true, ..Default::default() },
            ModelRow { id: "quick".into(), thinking: false, ..Default::default() },
        ];
        assert!(thinks(&infos, "deep"));
        assert!(!thinks(&infos, "quick"));
        assert!(thinks(&infos, "unknown"), "a model not described may think");
    }

    #[test]
    fn a_choice_made_during_a_turn_waits_until_the_session_has_it() {
        let pending = vec![(Picker::Mode, "plan".to_string()), (Picker::Model, "small".to_string())];
        // At the turn's end the session first says what it had.
        let old = json!({"type": "status", "mode": "ask", "model": "big", "thinking": "default"});
        assert_eq!(settled(&pending, &old), pending);
        // Then it takes the mode that waited.
        let mode = json!({"type": "status", "mode": "plan", "model": "big", "thinking": "default"});
        assert_eq!(settled(&pending, &mode), [(Picker::Model, "small".to_string())]);
        let both = json!({"type": "status", "mode": "plan", "model": "small", "thinking": "default"});
        assert!(settled(&pending, &both).is_empty());
        assert_eq!(pending_of(&pending, Picker::Model).as_deref(), Some("small"));
        assert_eq!(pending_of(&pending, Picker::Thinking), None);
        assert_eq!((picker_of("thinking"), picker_of("send")), (Some(Picker::Thinking), None));
    }

    #[test]
    fn the_arrows_pass_over_what_cannot_be_chosen() {
        let o = |enabled| Opt { value: String::new(), name: String::new(), detail: String::new(), enabled };
        let all = [o(true), o(false), o(true)];
        assert_eq!(stepped(&all, 0, 1), Some(2));
        assert_eq!(stepped(&all, 2, 1), Some(0));
        assert_eq!(stepped(&all, 0, -1), Some(2));
        assert_eq!(stepped(&[o(false)], 0, 1), None);
        assert_eq!(stepped(&[], 0, 1), None);
    }

    #[test]
    fn the_modes_that_let_more_run_are_drawn_apart() {
        assert_eq!(tone(Picker::Mode, "yolo"), Tone::Danger);
        assert_eq!(tone(Picker::Mode, "plan"), Tone::Accent);
        assert_eq!(tone(Picker::Mode, "ask"), Tone::Plain);
        assert_eq!(tone(Picker::Model, "yolo"), Tone::Plain);
    }
}
